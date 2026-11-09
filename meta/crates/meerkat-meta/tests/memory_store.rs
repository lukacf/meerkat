#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::time::Duration;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
};

use meerkat_meta::{
    DiscoverySpec, LocalJsonMemoryBackendConfig, MemoryBackendConfig, MetaConfig, RuntimeOptions,
    handle_meta_rpc_json, start_meta_runtime, start_meta_runtime_with_options,
};
use serde_json::{Value, json};
use tempfile::tempdir;

fn parse_response(line: &str) -> Value {
    serde_json::from_str(line).expect("valid rpc response json")
}

fn store_record_count(response: &Value, store: &str) -> Option<u64> {
    response["result"]["stores"]
        .as_array()
        .and_then(|stores| {
            stores
                .iter()
                .find(|entry| entry.get("store").and_then(Value::as_str) == Some(store))
        })
        .and_then(|entry| entry.get("record_count"))
        .and_then(Value::as_u64)
}

fn runtime_for_phase13() -> meerkat_meta::MetaRuntimeHandle {
    start_meta_runtime(
        MetaConfig {
            modules: vec![],
            discovery: DiscoverySpec {
                namespace: "phase13".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        vec![],
        Duration::from_secs(1),
    )
    .expect("runtime starts")
}

// The local-JSON ledger backend with its optional health gate: `endpoint`
// is health-checked before every ledger read and write.
fn runtime_for_phase13_with_memory_backend(
    endpoint: &str,
    state_path: &str,
) -> meerkat_meta::MetaRuntimeHandle {
    start_meta_runtime_with_options(
        MetaConfig {
            modules: vec![],
            discovery: DiscoverySpec {
                namespace: "phase13".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        vec![],
        Duration::from_secs(1),
        RuntimeOptions {
            memory_backend: Some(MemoryBackendConfig::LocalJson(
                LocalJsonMemoryBackendConfig {
                    state_path: state_path.to_string(),
                    health_check_endpoint: Some(endpoint.to_string()),
                },
            )),
            ..RuntimeOptions::default()
        },
    )
    .expect("runtime with memory backend starts")
}

#[test]
fn memory_query_query_param_substring_filters_assertions_and_conflicts() {
    let mut runtime = runtime_for_phase13();
    let indexed = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"query-filter-index-1",
            "method":"meta/memory/index",
            "params":{
                "entity":"delivery",
                "topic":"email_send",
                "store":"todo",
                "fact":"double-check Recipient Consent"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    assert!(indexed["result"]["assertion_id"].is_string());
    assert_eq!(indexed["result"]["conflict_active"], json!(false));
    let indexed_conflict = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"query-filter-index-2",
            "method":"meta/memory/index",
            "params":{
                "entity":"router",
                "topic":"deploy",
                "store":"todo",
                "fact":"validate rollback checklist",
                "conflict":true,
                "conflict_reason":"facts disagree on rollback"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    assert_eq!(indexed_conflict["result"]["conflict_active"], json!(true));

    let matched = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"query-filter-match","method":"meta/memory/query","params":{"query":"recipient consent"}}"#,
        Duration::from_secs(1),
    ));
    let matched_conflict = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"query-filter-conflict","method":"meta/memory/query","params":{"query":"ROLLBACK"}}"#,
        Duration::from_secs(1),
    ));
    let unmatched = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"query-filter-none","method":"meta/memory/query","params":{"query":"no-such-substring"}}"#,
        Duration::from_secs(1),
    ));
    runtime.shutdown();

    assert_eq!(
        (
            matched["result"]["assertions"].as_array().map(Vec::len),
            matched["result"]["assertions"][0]["fact"].clone(),
            matched["result"]["conflicts"].clone(),
        ),
        (Some(1), json!("double-check Recipient Consent"), json!([]),)
    );
    assert_eq!(
        (
            matched_conflict["result"]["assertions"]
                .as_array()
                .map(Vec::len),
            matched_conflict["result"]["conflicts"]
                .as_array()
                .map(Vec::len),
            matched_conflict["result"]["conflicts"][0]["reason"].clone(),
        ),
        (Some(1), Some(1), json!("facts disagree on rollback"))
    );
    assert_eq!(
        (
            unmatched["result"]["assertions"].clone(),
            unmatched["result"]["conflicts"].clone(),
        ),
        (json!([]), json!([]))
    );
}

struct HealthEndpointServer {
    endpoint: String,
    hit_count: Arc<AtomicUsize>,
    paths: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HealthEndpointServer {
    fn start(max_requests: Option<usize>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind health endpoint server");
        listener
            .set_nonblocking(true)
            .expect("set nonblocking listener");
        let endpoint = format!(
            "http://{}",
            listener.local_addr().expect("listener address")
        );
        let hit_count = Arc::new(AtomicUsize::new(0));
        let paths = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_hit_count = Arc::clone(&hit_count);
        let thread_paths = Arc::clone(&paths);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buffer = [0_u8; 4096];
                        let bytes_read = stream.read(&mut buffer).unwrap_or(0);
                        if bytes_read > 0 {
                            let request =
                                String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
                            let request_path = request
                                .lines()
                                .next()
                                .and_then(|line| line.split_whitespace().nth(1))
                                .unwrap_or("")
                                .to_string();
                            thread_paths
                                .lock()
                                .expect("lock request paths")
                                .push(request_path);
                        }
                        thread_hit_count.fetch_add(1, Ordering::Relaxed);
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        );
                        if let Some(limit) = max_requests {
                            if thread_hit_count.load(Ordering::Relaxed) >= limit {
                                break;
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            endpoint,
            hit_count,
            paths,
            stop,
            thread: Some(thread),
        }
    }

    fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    fn hit_count(&self) -> usize {
        self.hit_count.load(Ordering::Relaxed)
    }

    fn paths(&self) -> Vec<String> {
        self.paths.lock().expect("lock request paths").clone()
    }
}

impl Drop for HealthEndpointServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(addr) = self.endpoint.strip_prefix("http://") {
            let _ = TcpStream::connect(addr);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
#[ignore]
fn phase13_memory_rpc_index_query_and_store_counts_are_wired() {
    let mut runtime = runtime_for_phase13();
    let stores_before = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-stores-before","method":"meta/memory/stores","params":{}}"#,
        Duration::from_secs(1),
    ));
    let indexed_default = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-index-default",
            "method":"meta/memory/index",
            "params":{
                "entity":"Router",
                "topic":"Deploy",
                "fact":"rollback_plan_missing",
                "metadata":{"source":"test"},
                "conflict":true,
                "conflict_reason":"assertion_conflict"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let indexed_vector = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-index-vector",
            "method":"meta/memory/index",
            "params":{
                "entity":"router",
                "topic":"deploy",
                "store":"vector",
                "fact":"embedding:9f3a",
                "metadata":{"source":"test"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let indexed_timeline_conflict = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-index-timeline-conflict",
            "method":"meta/memory/index",
            "params":{
                "entity":"router",
                "topic":"deploy",
                "store":"timeline",
                "conflict":true,
                "conflict_reason":"timeline_mismatch"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let indexed_todo = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-index-todo",
            "method":"meta/memory/index",
            "params":{
                "entity":"router",
                "topic":"deploy",
                "store":"todo",
                "fact":"validate-rollback-checklist",
                "metadata":{"source":"test","priority":"high"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let indexed_top_of_mind = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-index-top-of-mind",
            "method":"meta/memory/index",
            "params":{
                "entity":"router",
                "topic":"deploy",
                "store":"top_of_mind",
                "fact":"rollback path is primary risk",
                "metadata":{"source":"test","urgency":"immediate"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let queried = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-query","method":"meta/memory/query","params":{"entity":"router","topic":"deploy"}}"#,
        Duration::from_secs(1),
    ));
    let queried_vector = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-query-vector","method":"meta/memory/query","params":{"entity":"router","topic":"deploy","store":"vector"}}"#,
        Duration::from_secs(1),
    ));
    let queried_timeline = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-query-timeline","method":"meta/memory/query","params":{"entity":"router","topic":"deploy","store":"timeline"}}"#,
        Duration::from_secs(1),
    ));
    let queried_todo = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-query-todo","method":"meta/memory/query","params":{"entity":"router","topic":"deploy","store":"todo"}}"#,
        Duration::from_secs(1),
    ));
    let queried_top_of_mind = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-query-top-of-mind","method":"meta/memory/query","params":{"entity":"router","topic":"deploy","store":"top_of_mind"}}"#,
        Duration::from_secs(1),
    ));
    let stores_after = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-stores-after","method":"meta/memory/stores","params":{}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(
        (
            store_record_count(&stores_before, "knowledge_graph"),
            store_record_count(&stores_before, "vector"),
            store_record_count(&stores_before, "timeline"),
            store_record_count(&stores_before, "todo"),
            store_record_count(&stores_before, "top_of_mind"),
        ),
        (Some(0), Some(0), Some(0), Some(0), Some(0))
    );
    assert!(indexed_default["result"]["assertion_id"].is_string());
    assert_eq!(
        (
            indexed_default["result"]["store"].clone(),
            indexed_vector["result"]["store"].clone(),
            indexed_timeline_conflict["result"]["store"].clone(),
            indexed_todo["result"]["store"].clone(),
            indexed_top_of_mind["result"]["store"].clone(),
        ),
        (
            json!("knowledge_graph"),
            json!("vector"),
            json!("timeline"),
            json!("todo"),
            json!("top_of_mind"),
        )
    );
    assert_eq!(
        (
            queried["result"]["assertions"].as_array().map(Vec::len),
            queried["result"]["conflicts"].as_array().map(Vec::len),
            queried_vector["result"]["assertions"][0]["store"].clone(),
            queried_vector["result"]["assertions"][0]["fact"].clone(),
            queried_vector["result"]["conflicts"].clone(),
            queried_timeline["result"]["assertions"].clone(),
            queried_timeline["result"]["conflicts"][0]["store"].clone(),
            queried_timeline["result"]["conflicts"][0]["reason"].clone(),
        ),
        (
            Some(4),
            Some(2),
            json!("vector"),
            json!("embedding:9f3a"),
            json!([]),
            json!([]),
            json!("timeline"),
            json!("timeline_mismatch"),
        )
    );
    assert_eq!(
        (
            queried_todo["result"]["assertions"][0]["store"].clone(),
            queried_todo["result"]["assertions"][0]["fact"].clone(),
            queried_todo["result"]["conflicts"].clone(),
            queried_top_of_mind["result"]["assertions"][0]["store"].clone(),
            queried_top_of_mind["result"]["assertions"][0]["fact"].clone(),
            queried_top_of_mind["result"]["conflicts"].clone(),
        ),
        (
            json!("todo"),
            json!("validate-rollback-checklist"),
            json!([]),
            json!("top_of_mind"),
            json!("rollback path is primary risk"),
            json!([]),
        )
    );
    assert_eq!(
        (
            store_record_count(&stores_after, "knowledge_graph"),
            store_record_count(&stores_after, "vector"),
            store_record_count(&stores_after, "timeline"),
            store_record_count(&stores_after, "todo"),
            store_record_count(&stores_after, "top_of_mind"),
        ),
        (Some(2), Some(1), Some(1), Some(1), Some(1))
    );
}

#[test]
#[ignore]
fn phase13_health_checked_memory_backend_persists_across_runtime_restart() {
    let temp = tempdir().expect("temp dir");
    let state_path = temp.path().join("memory-ledger-state.json");
    let endpoint_server = HealthEndpointServer::start(None);

    let mut first_runtime = runtime_for_phase13_with_memory_backend(
        endpoint_server.endpoint(),
        state_path.to_str().expect("state path as string"),
    );
    let first_index = parse_response(&handle_meta_rpc_json(
        &mut first_runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-ledger-index",
            "method":"meta/memory/index",
            "params":{
                "entity":"delivery",
                "topic":"email_send",
                "store":"todo",
                "fact":"double-check recipient consent",
                "conflict":true,
                "conflict_reason":"risk_signal"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    first_runtime.shutdown();

    let mut second_runtime = runtime_for_phase13_with_memory_backend(
        endpoint_server.endpoint(),
        state_path.to_str().expect("state path as string"),
    );
    let queried = parse_response(&handle_meta_rpc_json(
        &mut second_runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-ledger-query","method":"meta/memory/query","params":{"entity":"delivery","topic":"email_send","store":"todo"}}"#,
        Duration::from_secs(1),
    ));
    let stores = parse_response(&handle_meta_rpc_json(
        &mut second_runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-ledger-stores","method":"meta/memory/stores","params":{}}"#,
        Duration::from_secs(1),
    ));
    second_runtime.shutdown();

    assert_eq!(
        (
            first_index["result"]["store"].clone(),
            queried["result"]["assertions"][0]["store"].clone(),
            queried["result"]["assertions"][0]["fact"].clone(),
            queried["result"]["conflicts"][0]["reason"].clone(),
            store_record_count(&stores, "todo"),
            store_record_count(&stores, "knowledge_graph"),
        ),
        (
            json!("todo"),
            json!("todo"),
            json!("double-check recipient consent"),
            json!("risk_signal"),
            Some(2),
            Some(0),
        )
    );
    assert!(endpoint_server.hit_count() >= 3);
    assert!(
        endpoint_server
            .paths()
            .iter()
            .all(|path| path == "/v1/health")
    );
}

#[test]
#[ignore]
fn phase13_health_checked_memory_backend_endpoint_failure_maps_to_typed_rpc_error() {
    let temp = tempdir().expect("temp dir");
    let state_path = temp.path().join("memory-ledger-state.json");
    let endpoint_server = HealthEndpointServer::start(Some(1));

    let mut runtime = runtime_for_phase13_with_memory_backend(
        endpoint_server.endpoint(),
        state_path.to_str().expect("state path as string"),
    );

    thread::sleep(Duration::from_millis(100));
    let indexed = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-ledger-index-failure",
            "method":"meta/memory/index",
            "params":{
                "entity":"delivery",
                "topic":"email_send",
                "store":"todo",
                "fact":"attempt write after endpoint down"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let queried = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-ledger-query-after-failure","method":"meta/memory/query","params":{"entity":"delivery","topic":"email_send","store":"todo"}}"#,
        Duration::from_secs(1),
    ));
    let gated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-ledger-gating-after-failure",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"send_email",
                "actor_id":"agent-r2",
                "risk_tier":"r2",
                "entity":"delivery",
                "topic":"email_send"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    runtime.shutdown();

    // Regression: -32012 (memory backend unavailable) must NOT collide
    // with -32010 (mob_events stale cursor). SDKs branch on -32010 to
    // raise MobEventsStaleError; a memory failure carrying -32010 would
    // be misclassified.
    assert_eq!(
        indexed["error"]["code"],
        json!(-32012),
        "memory backend failure must use -32012, not -32010"
    );
    assert_ne!(
        indexed["error"]["code"],
        json!(-32010),
        "memory backend code must be distinct from MOB_EVENTS_STALE_CURSOR_CODE"
    );
    assert!(
        indexed["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .starts_with("Memory backend unavailable:")
    );
    assert_eq!(
        (
            queried["result"]["assertions"].clone(),
            queried["result"]["conflicts"].clone(),
            gated["result"]["outcome"].clone(),
            gated["result"]["fallback_reason"].clone(),
            gated["result"]["pending_id"].clone(),
        ),
        (
            json!([]),
            json!([]),
            json!("allowed_with_audit"),
            Value::Null,
            Value::Null,
        )
    );
}

#[test]
#[ignore]
fn phase13_r2_and_r3_missing_context_cannot_bypass_memory_conflicts() {
    let mut runtime = runtime_for_phase13();
    let _index_conflict = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-context-index",
            "method":"meta/memory/index",
            "params":{
                "entity":"delivery",
                "topic":"email_send",
                "conflict":true,
                "conflict_reason":"unverified_claim"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let r2_missing = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-r2-missing-context",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"send_email",
                "actor_id":"agent-r2",
                "risk_tier":"r2"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let r3_missing_topic = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-r3-missing-context",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"send_sms",
                "actor_id":"agent-r3",
                "risk_tier":"r3",
                "entity":"delivery",
                "requested_approver":"lead",
                "approval_recipient":"approvals@example.com",
                "approval_channel":"transactional"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-context-pending","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    let audit = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-context-audit","method":"meta/gating/audit","params":{"limit":20}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    let entries = audit["result"]["entries"]
        .as_array()
        .expect("audit entries");
    let context_blocks = entries
        .iter()
        .filter(|entry| {
            entry.get("event_type") == Some(&json!("conflict_blocked"))
                && entry.get("detail").and_then(|detail| detail.get("reason"))
                    == Some(&json!("memory_conflict_context_missing"))
        })
        .collect::<Vec<_>>();

    assert_eq!(
        (
            r2_missing["result"]["outcome"].clone(),
            r2_missing["result"]["fallback_reason"].clone(),
            r2_missing["result"]["pending_id"].clone(),
            r3_missing_topic["result"]["outcome"].clone(),
            r3_missing_topic["result"]["fallback_reason"].clone(),
            r3_missing_topic["result"]["pending_id"].clone(),
            pending["result"]["pending"].clone(),
            context_blocks.len(),
            context_blocks[0]["detail"]["missing_context"]["entity"].clone(),
            context_blocks[0]["detail"]["missing_context"]["topic"].clone(),
            context_blocks[1]["detail"]["missing_context"]["entity"].clone(),
            context_blocks[1]["detail"]["missing_context"]["topic"].clone(),
        ),
        (
            json!("safe_draft"),
            json!("memory_conflict_context_missing"),
            Value::Null,
            json!("safe_draft"),
            json!("memory_conflict_context_missing"),
            Value::Null,
            json!([]),
            2,
            json!(true),
            json!(true),
            json!(false),
            json!(true),
        )
    );
}

#[test]
#[ignore]
fn phase13_r2_gating_is_blocked_by_memory_conflict_with_audit_reason() {
    let mut runtime = runtime_for_phase13();
    let _index_conflict = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-r2-index",
            "method":"meta/memory/index",
            "params":{
                "entity":"delivery",
                "topic":"email_send",
                "conflict":true,
                "conflict_reason":"unverified_claim"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let gated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-r2-gating",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"send_email",
                "actor_id":"agent-r2",
                "risk_tier":"r2",
                "entity":"delivery",
                "topic":"email_send"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let audit = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-r2-audit","method":"meta/gating/audit","params":{"limit":10}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    let entries = audit["result"]["entries"]
        .as_array()
        .expect("audit entries");
    let conflict_blocked = entries
        .iter()
        .find(|entry| entry.get("event_type") == Some(&json!("conflict_blocked")))
        .expect("conflict_blocked entry");

    assert_eq!(
        (
            gated["result"]["outcome"].clone(),
            gated["result"]["fallback_reason"].clone(),
            gated["result"]["pending_id"].clone(),
            conflict_blocked["detail"]["reason"].clone(),
            conflict_blocked["detail"]["conflict"]["entity"].clone(),
            conflict_blocked["detail"]["conflict"]["topic"].clone(),
        ),
        (
            json!("safe_draft"),
            json!("memory_conflict"),
            Value::Null,
            json!("memory_conflict"),
            json!("delivery"),
            json!("email_send"),
        )
    );
}

#[test]
#[ignore]
fn phase13_r3_conflict_blocks_pending_creation() {
    let mut runtime = runtime_for_phase13();
    let _index_conflict = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-r3-index",
            "method":"meta/memory/index",
            "params":{
                "entity":"router",
                "topic":"prod_deploy",
                "conflict":true,
                "conflict_reason":"facts_disagree"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let gated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"phase13-r3-gating",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"deploy_prod_router",
                "actor_id":"agent-r3",
                "risk_tier":"r3",
                "entity":"router",
                "topic":"prod_deploy",
                "requested_approver":"lead",
                "approval_recipient":"approvals@example.com",
                "approval_channel":"transactional"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase13-r3-pending","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(
        (
            gated["result"]["outcome"].clone(),
            gated["result"]["pending_id"].clone(),
            pending["result"]["pending"].clone(),
        ),
        (json!("safe_draft"), Value::Null, json!([]))
    );
}
