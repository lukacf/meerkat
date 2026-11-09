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
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use meerkat_meta::{
    BigQuerySessionStoreAdapter, BigQuerySessionStoreError, DiscoverySpec, MetaConfig,
    SessionPersistenceRow, handle_meta_rpc_json, start_meta_runtime,
};
use serde_json::{Value, json};

#[path = "support/bigquery_http_mock.rs"]
mod bigquery_http_mock;

use bigquery_http_mock::{MockHttpResponse, MockHttpServer};

/// The environment variable that names the GCP project the real-BigQuery row
/// creates its scratch dataset in; the row skips when it is unset.
const REAL_BQ_PROJECT_ENV: &str = "MEERKAT_TEST_BQ_PROJECT";
const DEFAULT_BQ_API_BASE_URL: &str = "https://bigquery.googleapis.com/bigquery/v2";

/// The `insertAll` path of the phase F table on a mock's base URL.
const PHASE_F_INSERT_ALL_PATH: &str =
    "/bigquery/v2/projects/phase-f-project/datasets/phase_f_dataset/tables/phase_f_table/insertAll";

fn sample_write_rows() -> Vec<SessionPersistenceRow> {
    vec![SessionPersistenceRow {
        session_id: "sample-session".to_string(),
        updated_at_ms: 10_000,
        deleted: false,
        payload: json!({"step":"create"}),
        ..Default::default()
    }]
}

fn start_phase_f_rpc_runtime() -> meerkat_meta::MetaRuntimeHandle {
    start_meta_runtime(
        MetaConfig {
            modules: vec![],
            discovery: DiscoverySpec {
                namespace: "phase_f_rpc".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        vec![],
        Duration::from_millis(250),
    )
    .expect("start runtime")
}

fn call_rpc(
    runtime: &mut meerkat_meta::MetaRuntimeHandle,
    request: Value,
    timeout: Duration,
) -> Value {
    let response_line = handle_meta_rpc_json(
        runtime,
        &serde_json::to_string(&request).expect("serialize rpc request"),
        timeout,
    );
    serde_json::from_str(&response_line).expect("parse rpc response")
}

#[tokio::test]
async fn phase_f_contract_native_bigquery_transport_request_shape_and_query_parsing() {
    let writes = vec![
        SessionPersistenceRow {
            session_id: "s1".to_string(),
            updated_at_ms: 1_000,
            deleted: false,
            payload: json!({"step":"create"}),
            ..Default::default()
        },
        SessionPersistenceRow {
            session_id: "s1".to_string(),
            updated_at_ms: 2_000,
            deleted: true,
            payload: json!({}),
            ..Default::default()
        },
        SessionPersistenceRow {
            session_id: "s2".to_string(),
            updated_at_ms: 1_500,
            deleted: false,
            payload: json!({"step":"create"}),
            ..Default::default()
        },
        SessionPersistenceRow {
            session_id: "s2".to_string(),
            updated_at_ms: 3_000,
            deleted: false,
            payload: json!({"step":"update","version":2}),
            ..Default::default()
        },
    ];
    // read_latest_rows now uses server-side QUALIFY dedup; mock returns already-deduped rows
    let latest_query_rows = json!({
        "jobComplete": true,
        "rows": [
            {"f":[{"v":"s1"},{"v":"2000"},{"v":"true"},{"v":"{}"},{"v":"{}"}]},
            {"f":[{"v":"s2"},{"v":"3000"},{"v":"false"},{"v":"{\"step\":\"update\",\"version\":2}"}]}
        ]
    });
    // read_live_rows uses server-side QUALIFY dedup + deleted=false filter
    let live_query_rows = json!({
        "jobComplete": true,
        "rows": [
            {"f":[{"v":"s2"},{"v":"3000"},{"v":"false"},{"v":"{\"step\":\"update\",\"version\":2}"}]}
        ]
    });

    let server = MockHttpServer::start(vec![
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(latest_query_rows),
        MockHttpResponse::json(live_query_rows),
    ]);
    let store = BigQuerySessionStoreAdapter::new_native("phase_f_dataset", "phase_f_table")
        .with_project_id("phase-f-project")
        .with_access_token("phase-f-token")
        .with_api_base_url(format!("{}/bigquery/v2", server.base_url()));

    store
        .stream_insert_rows(&writes)
        .await
        .expect("insertAll contract call should succeed");
    let latest = store
        .read_latest_rows()
        .await
        .expect("read latest via query API");
    let live = store
        .read_live_rows()
        .await
        .expect("read live via query API");

    assert_eq!(
        latest,
        vec![
            SessionPersistenceRow {
                session_id: "s1".to_string(),
                updated_at_ms: 2_000,
                deleted: true,
                payload: json!({}),
                ..Default::default()
            },
            SessionPersistenceRow {
                session_id: "s2".to_string(),
                updated_at_ms: 3_000,
                deleted: false,
                payload: json!({"step":"update","version":2}),
                ..Default::default()
            },
        ]
    );
    assert_eq!(
        live,
        vec![SessionPersistenceRow {
            session_id: "s2".to_string(),
            updated_at_ms: 3_000,
            deleted: false,
            payload: json!({"step":"update","version":2}),
            ..Default::default()
        }]
    );

    let requests = server.captured_requests();
    assert_eq!(requests.len(), 3);

    let insert_request = &requests[0];
    assert_eq!(insert_request.method, "POST");
    assert_eq!(
        insert_request.path,
        "/bigquery/v2/projects/phase-f-project/datasets/phase_f_dataset/tables/phase_f_table/insertAll"
    );
    assert_eq!(
        insert_request
            .headers
            .get("authorization")
            .expect("insert authorization header"),
        "Bearer phase-f-token"
    );
    let insert_body: Value =
        serde_json::from_str(&insert_request.body).expect("parse insert request body");
    let inserted_rows = insert_body["rows"]
        .as_array()
        .expect("insert request rows array");
    assert_eq!(inserted_rows.len(), writes.len());
    // MK-008: insertId must not be present
    for row in inserted_rows {
        assert!(
            row.get("insertId").is_none(),
            "insertId must be removed from BQ streaming inserts"
        );
    }
    assert_eq!(
        insert_body["rows"][0]["json"]["session_id"],
        Value::String("s1".to_string())
    );
    assert_eq!(
        insert_body["rows"][0]["json"]["payload"],
        Value::String("{\"step\":\"create\"}".to_string())
    );
    assert_eq!(insert_body["rows"][1]["json"]["deleted"], Value::Bool(true));

    let query_request = &requests[1];
    assert_eq!(query_request.method, "POST");
    assert_eq!(
        query_request.path,
        "/bigquery/v2/projects/phase-f-project/queries"
    );
    let query_body: Value =
        serde_json::from_str(&query_request.body).expect("parse query request body");
    let query_text = query_body["query"].as_str().expect("query body query text");
    assert!(query_text.contains("SELECT session_id, updated_at_ms, deleted, payload"));
    assert!(query_text.contains("phase-f-project.phase_f_dataset.phase_f_table"));
    // MK-009: read_latest_rows must use server-side QUALIFY dedup
    assert!(
        query_text.contains("QUALIFY ROW_NUMBER()"),
        "read_latest_rows query must use QUALIFY for server-side dedup"
    );
}

#[test]
fn phase_f_rpc_builtin_bigquery_path_uses_native_adapter() {
    let query_rows = json!({
        "rows": [
            {"f":[{"v":"rpc-session"},{"v":"1234"},{"v":"false"},{"v":"{\"kind\":\"rpc\"}"}]}
        ]
    });
    let server = MockHttpServer::start(vec![MockHttpResponse::json(query_rows)]);
    let mut runtime = start_phase_f_rpc_runtime();

    let request = json!({
        "jsonrpc":"2.0",
        "id":"phase-f-rpc-bq-1",
        "method":"meta/session_store/bigquery",
        "params":{
            "operation":"read_latest_rows",
            "dataset":"phase_f_dataset",
            "table":"phase_f_table",
            "project_id":"phase-f-project",
            "access_token":"phase-f-token",
            "api_base_url": format!("{}/bigquery/v2", server.base_url())
        }
    });
    let response = call_rpc(&mut runtime, request, Duration::from_secs(1));
    assert!(
        response.get("error").is_none(),
        "unexpected RPC error: {response}"
    );
    assert_eq!(response["result"]["operation"], "read_latest_rows");
    assert_eq!(
        response["result"]["rows"],
        json!([{
            "session_id":"rpc-session",
            "updated_at_ms":1234,
            "deleted":false,
            "payload":{"kind":"rpc"},"labels":{}
        }])
    );

    let requests = server.captured_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(
        requests[0].path,
        "/bigquery/v2/projects/phase-f-project/queries"
    );
}

#[test]
fn phase_f_rpc_capabilities_discover_bigquery_session_store_builtin() {
    let mut runtime = start_phase_f_rpc_runtime();
    let capabilities = call_rpc(
        &mut runtime,
        json!({
            "jsonrpc":"2.0",
            "id":"phase-f-rpc-caps-1",
            "method":"meta/capabilities",
            "params":{}
        }),
        Duration::from_secs(1),
    );
    let methods = capabilities["result"]["methods"]
        .as_array()
        .expect("capabilities methods array");
    assert!(
        methods
            .iter()
            .any(|method| method == "meta/session_store/bigquery"),
        "meta/capabilities must advertise meta/session_store/bigquery"
    );
}

#[test]
fn phase_f_rpc_bigquery_stream_insert_rows_issues_insert_all_request() {
    let server = MockHttpServer::start(vec![MockHttpResponse::json(json!({}))]);
    let mut runtime = start_phase_f_rpc_runtime();
    let rows = vec![
        SessionPersistenceRow {
            session_id: "rpc-s1".to_string(),
            updated_at_ms: 101,
            deleted: false,
            payload: json!({"step":"create"}),
            ..Default::default()
        },
        SessionPersistenceRow {
            session_id: "rpc-s1".to_string(),
            updated_at_ms: 202,
            deleted: true,
            payload: json!({}),
            ..Default::default()
        },
    ];

    let response = call_rpc(
        &mut runtime,
        json!({
            "jsonrpc":"2.0",
            "id":"phase-f-rpc-bq-insert",
            "method":"meta/session_store/bigquery",
            "params":{
                "operation":"stream_insert_rows",
                "dataset":"phase_f_dataset",
                "table":"phase_f_table",
                "project_id":"phase-f-project",
                "access_token":"phase-f-token",
                "api_base_url": format!("{}/bigquery/v2", server.base_url()),
                "rows": rows
            }
        }),
        Duration::from_secs(1),
    );
    assert!(
        response.get("error").is_none(),
        "unexpected RPC error: {response}"
    );
    assert_eq!(response["result"]["operation"], "stream_insert_rows");
    assert_eq!(response["result"]["accepted"], true);
    assert_eq!(response["result"]["inserted_rows"], 2);

    let requests = server.captured_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].path,
        "/bigquery/v2/projects/phase-f-project/datasets/phase_f_dataset/tables/phase_f_table/insertAll"
    );
    let insert_body: Value =
        serde_json::from_str(&requests[0].body).expect("parse insert request body");
    let rpc_inserted_rows = insert_body["rows"].as_array().expect("insert rows array");
    assert_eq!(rpc_inserted_rows.len(), 2);
    // MK-008: insertId must not be present
    for row in rpc_inserted_rows {
        assert!(
            row.get("insertId").is_none(),
            "insertId must be removed from BQ streaming inserts"
        );
    }
    assert_eq!(insert_body["rows"][0]["json"]["session_id"], "rpc-s1");
    assert_eq!(insert_body["rows"][0]["json"]["updated_at_ms"], "101");
    assert_eq!(
        insert_body["rows"][0]["json"]["payload"],
        "{\"step\":\"create\"}"
    );
    assert_eq!(insert_body["rows"][1]["json"]["deleted"], true);
}

#[test]
fn phase_f_rpc_bigquery_read_rows_and_read_live_rows_semantics() {
    // read_rows returns all raw rows (no server-side dedup)
    let all_query_rows = json!({
        "rows": [
            {"f":[{"v":"s1"},{"v":"1000"},{"v":"false"},{"v":"{\"step\":\"create\"}"},{"v":"{}"},{"v":"{}"}]},
            {"f":[{"v":"s1"},{"v":"2000"},{"v":"true"},{"v":"{}"},{"v":"{}"}]},
            {"f":[{"v":"s2"},{"v":"1500"},{"v":"false"},{"v":"{\"step\":\"create\"}"},{"v":"{}"},{"v":"{}"}]},
            {"f":[{"v":"s2"},{"v":"3000"},{"v":"false"},{"v":"{\"step\":\"update\",\"version\":2}"}]}
        ]
    });
    // read_live_rows uses server-side QUALIFY dedup + deleted=false filter
    let live_query_rows = json!({
        "rows": [
            {"f":[{"v":"s2"},{"v":"3000"},{"v":"false"},{"v":"{\"step\":\"update\",\"version\":2}"}]}
        ]
    });
    let server = MockHttpServer::start(vec![
        MockHttpResponse::json(all_query_rows),
        MockHttpResponse::json(live_query_rows),
    ]);
    let mut runtime = start_phase_f_rpc_runtime();

    let read_all = call_rpc(
        &mut runtime,
        json!({
            "jsonrpc":"2.0",
            "id":"phase-f-rpc-bq-read-all",
            "method":"meta/session_store/bigquery",
            "params":{
                "operation":"read_rows",
                "dataset":"phase_f_dataset",
                "table":"phase_f_table",
                "project_id":"phase-f-project",
                "access_token":"phase-f-token",
                "api_base_url": format!("{}/bigquery/v2", server.base_url())
            }
        }),
        Duration::from_secs(1),
    );
    let read_live = call_rpc(
        &mut runtime,
        json!({
            "jsonrpc":"2.0",
            "id":"phase-f-rpc-bq-read-live",
            "method":"meta/session_store/bigquery",
            "params":{
                "operation":"read_live_rows",
                "dataset":"phase_f_dataset",
                "table":"phase_f_table",
                "project_id":"phase-f-project",
                "access_token":"phase-f-token",
                "api_base_url": format!("{}/bigquery/v2", server.base_url())
            }
        }),
        Duration::from_secs(1),
    );

    assert!(
        read_all.get("error").is_none(),
        "unexpected read_rows error: {read_all}"
    );
    assert_eq!(read_all["result"]["operation"], "read_rows");
    assert_eq!(
        read_all["result"]["rows"],
        json!([
            {"session_id":"s1","updated_at_ms":1000,"deleted":false,"payload":{"step":"create"},"labels":{}},
            {"session_id":"s1","updated_at_ms":2000,"deleted":true,"payload":{},"labels":{}},
            {"session_id":"s2","updated_at_ms":1500,"deleted":false,"payload":{"step":"create"},"labels":{}},
            {"session_id":"s2","updated_at_ms":3000,"deleted":false,"payload":{"step":"update","version":2},"labels":{}}
        ])
    );

    assert!(
        read_live.get("error").is_none(),
        "unexpected read_live_rows error: {read_live}"
    );
    assert_eq!(read_live["result"]["operation"], "read_live_rows");
    assert_eq!(
        read_live["result"]["rows"],
        json!([
            {"session_id":"s2","updated_at_ms":3000,"deleted":false,"payload":{"step":"update","version":2},"labels":{}}
        ])
    );

    let requests = server.captured_requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request.path == "/bigquery/v2/projects/phase-f-project/queries")
    );
}

/// `timeout_ms` reaches the store's HTTP timeout: BigQuery answers the insert
/// 200 ms after it has it, and the 25 ms timeout fails the call first; with
/// the default 30 s timeout the insert would succeed. The store runs the
/// request on a runtime of its own, so the clock cannot be paused here, but the
/// order holds however the threads are scheduled: the timeout's timer starts
/// before the request goes out, and reqwest checks it before the answer.
#[test]
fn phase_f_rpc_bigquery_timeout_ms_propagates_to_store_http_timeout() {
    let server = MockHttpServer::start(vec![
        MockHttpResponse::json(json!({})).with_delay(Duration::from_millis(200)),
    ]);
    let mut runtime = start_phase_f_rpc_runtime();

    let response = call_rpc(
        &mut runtime,
        json!({
            "jsonrpc":"2.0",
            "id":"phase-f-rpc-bq-timeout",
            "method":"meta/session_store/bigquery",
            "params":{
                "operation":"stream_insert_rows",
                "dataset":"phase_f_dataset",
                "table":"phase_f_table",
                "project_id":"phase-f-project",
                "access_token":"phase-f-token",
                "api_base_url": format!("{}/bigquery/v2", server.base_url()),
                "timeout_ms": 25,
                "rows": sample_write_rows()
            }
        }),
        Duration::from_secs(1),
    );
    let insert_url = format!("{}{PHASE_F_INSERT_ALL_PATH}", server.base_url());
    assert_eq!(
        response,
        json!({
            "jsonrpc": "2.0",
            "id": "phase-f-rpc-bq-timeout",
            "error": {
                "code": -32011,
                "message": format!(
                    "BigQuery session store request failed: {}",
                    reqwest_timeout_error(&insert_url)
                ),
            },
        })
    );
}

#[tokio::test]
async fn phase_f_rpc_bigquery_invalid_params_return_minus_32602() {
    let mut runtime = start_phase_f_rpc_runtime();
    let missing_rows = call_rpc(
        &mut runtime,
        json!({
            "jsonrpc":"2.0",
            "id":"phase-f-rpc-bq-invalid-rows",
            "method":"meta/session_store/bigquery",
            "params":{
                "operation":"stream_insert_rows",
                "dataset":"phase_f_dataset",
                "table":"phase_f_table",
                "project_id":"phase-f-project",
                "access_token":"phase-f-token"
            }
        }),
        Duration::from_secs(1),
    );
    assert_eq!(missing_rows["error"]["code"], -32602);

    let invalid_timeout = call_rpc(
        &mut runtime,
        json!({
            "jsonrpc":"2.0",
            "id":"phase-f-rpc-bq-invalid-timeout",
            "method":"meta/session_store/bigquery",
            "params":{
                "operation":"read_rows",
                "dataset":"phase_f_dataset",
                "table":"phase_f_table",
                "timeout_ms": 0
            }
        }),
        Duration::from_secs(1),
    );
    assert_eq!(invalid_timeout["error"]["code"], -32602);
}

#[tokio::test]
async fn phase_f_rpc_bigquery_store_and_api_failures_return_minus_32011() {
    let mut runtime = start_phase_f_rpc_runtime();
    let missing_project = call_rpc(
        &mut runtime,
        json!({
            "jsonrpc":"2.0",
            "id":"phase-f-rpc-bq-store-config-error",
            "method":"meta/session_store/bigquery",
            "params":{
                "operation":"read_rows",
                "dataset":"phase_f_dataset",
                "table":"phase_f_table",
                "access_token":"phase-f-token"
            }
        }),
        Duration::from_secs(1),
    );
    assert_eq!(missing_project["error"]["code"], -32011);

    let api_server = MockHttpServer::start(vec![MockHttpResponse {
        status_code: 503,
        content_type: "application/json".to_string(),
        body: json!({"error":{"message":"service unavailable"}}).to_string(),
        response_delay: Duration::from_millis(0),
    }]);
    let api_failure = call_rpc(
        &mut runtime,
        json!({
            "jsonrpc":"2.0",
            "id":"phase-f-rpc-bq-store-api-error",
            "method":"meta/session_store/bigquery",
            "params":{
                "operation":"read_rows",
                "dataset":"phase_f_dataset",
                "table":"phase_f_table",
                "project_id":"phase-f-project",
                "access_token":"phase-f-token",
                "api_base_url": format!("{}/bigquery/v2", api_server.base_url())
            }
        }),
        Duration::from_secs(1),
    );
    assert_eq!(api_failure["error"]["code"], -32011);
    let api_message = api_failure["error"]["message"]
        .as_str()
        .expect("api failure message");
    assert!(
        api_message.contains("status 503"),
        "expected surfaced BigQuery API status in RPC error, got: {api_message}"
    );
}

#[tokio::test]
async fn phase_f_bigquery_missing_project_or_token_configuration_returns_configuration_error() {
    let writes = sample_write_rows();

    let missing_project =
        BigQuerySessionStoreAdapter::new_native("phase_f_dataset", "phase_f_table")
            .with_access_token("phase-f-token")
            .stream_insert_rows(&writes)
            .await
            .expect_err("missing project_id should fail");
    assert_configuration_error_contains(missing_project, "missing BigQuery project_id");

    let missing_token = BigQuerySessionStoreAdapter::new_native("phase_f_dataset", "phase_f_table")
        .with_project_id("phase-f-project")
        .stream_insert_rows(&writes)
        .await
        .expect_err("missing access token should fail");
    assert_configuration_error_contains(missing_token, "missing BigQuery access token");
}

#[tokio::test]
async fn phase_f_bigquery_insert_all_row_level_errors_are_not_silently_accepted() {
    let server = MockHttpServer::start(vec![MockHttpResponse::json(json!({
        "insertErrors": [
            {"index":0,"errors":[{"reason":"invalid","message":"row rejected"}]}
        ]
    }))]);
    let store = BigQuerySessionStoreAdapter::new_native("phase_f_dataset", "phase_f_table")
        .with_project_id("phase-f-project")
        .with_access_token("phase-f-token")
        .with_api_base_url(format!("{}/bigquery/v2", server.base_url()));

    let err = store
        .stream_insert_rows(&sample_write_rows())
        .await
        .expect_err("insertErrors should bubble as API failure");
    assert_api_error_contains(err, "insertAll returned row errors");
}

#[tokio::test]
async fn phase_f_bigquery_non_success_http_statuses_surface_api_error() {
    let server = MockHttpServer::start(vec![MockHttpResponse {
        status_code: 503,
        content_type: "application/json".to_string(),
        body: json!({"error":{"message":"service unavailable"}}).to_string(),
        response_delay: Duration::from_millis(0),
    }]);
    let store = BigQuerySessionStoreAdapter::new_native("phase_f_dataset", "phase_f_table")
        .with_project_id("phase-f-project")
        .with_access_token("phase-f-token")
        .with_api_base_url(format!("{}/bigquery/v2", server.base_url()));

    let err = store
        .stream_insert_rows(&sample_write_rows())
        .await
        .expect_err("non-2xx status should fail");
    assert_api_error_contains(err, "status 503");
}

#[tokio::test]
async fn phase_f_bigquery_malformed_query_rows_and_payloads_return_parse_failures() {
    let malformed_rows_server = MockHttpServer::start(vec![MockHttpResponse::json(json!({
        "rows": [
            {"bad":"shape"}
        ]
    }))]);
    let malformed_rows_store =
        BigQuerySessionStoreAdapter::new_native("phase_f_dataset", "phase_f_table")
            .with_project_id("phase-f-project")
            .with_access_token("phase-f-token")
            .with_api_base_url(format!("{}/bigquery/v2", malformed_rows_server.base_url()));
    let malformed_rows_err = malformed_rows_store
        .read_rows()
        .await
        .expect_err("query rows missing row.f should fail parsing");
    assert_invalid_query_error_contains(malformed_rows_err, "missing row.f cell array");

    let malformed_payload_server = MockHttpServer::start(vec![MockHttpResponse::json(json!({
        "rows": [
            {"f":[{"v":"s1"},{"v":"2000"},{"v":"false"},{"v":"{not-json}"}]}
        ]
    }))]);
    let malformed_payload_store =
        BigQuerySessionStoreAdapter::new_native("phase_f_dataset", "phase_f_table")
            .with_project_id("phase-f-project")
            .with_access_token("phase-f-token")
            .with_api_base_url(format!(
                "{}/bigquery/v2",
                malformed_payload_server.base_url()
            ));
    let malformed_payload_err = malformed_payload_store
        .read_rows()
        .await
        .expect_err("invalid payload JSON should fail parsing");
    assert_invalid_query_error_contains(malformed_payload_err, "payload JSON parse failed");
}

/// A request that BigQuery has not answered within the adapter's HTTP timeout
/// fails with the timeout error, and a refused connection with the connect
/// error. The mock holds the insert unanswered, and the paused clock does not
/// move until the mock has it, so the 25 ms timeout ends an insert in flight
/// however long it takes to go out. While the refused connection is tried a
/// blocking task holds the paused clock still, so only the refusal can end
/// that insert.
#[tokio::test(start_paused = true)]
async fn phase_f_bigquery_timeout_and_connection_failures_surface_http_errors() {
    let (timeout_server, insert_receipt) = MockHttpServer::start_holding();
    let http_timeout = Duration::from_millis(25);
    let timeout_store = BigQuerySessionStoreAdapter::new_native("phase_f_dataset", "phase_f_table")
        .with_project_id("phase-f-project")
        .with_access_token("phase-f-token")
        .with_api_base_url(format!("{}/bigquery/v2", timeout_server.base_url()))
        .with_http_timeout(http_timeout);
    let rows = sample_write_rows();
    let started = tokio::time::Instant::now();
    let insert = timeout_store.stream_insert_rows(&rows);
    tokio::pin!(insert);
    // The paused clock does not advance while a blocking task runs, so the
    // timeout cannot pass before the mock has the insert.
    let insert_received = tokio::task::spawn_blocking(move || insert_receipt.wait());
    tokio::select! {
        biased;
        received = insert_received => received.expect("the mock captures the insert"),
        result = &mut insert => panic!("the insert ended before the mock had it: {result:?}"),
    }
    let timeout_err = insert.await.expect_err("the held insert times out");
    let elapsed = started.elapsed();
    assert!(
        (http_timeout..=http_timeout + Duration::from_millis(1)).contains(&elapsed),
        "the insert failed {elapsed:?} after it started, not at its {http_timeout:?} timeout"
    );
    assert_eq!(
        timeout_err,
        BigQuerySessionStoreError::Http(reqwest_timeout_error(&format!(
            "{}{PHASE_F_INSERT_ALL_PATH}",
            timeout_server.base_url()
        )))
    );
    assert_eq!(
        timeout_server
            .captured_requests()
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        vec![PHASE_F_INSERT_ALL_PATH]
    );

    let (_refusing_socket, base_url) = refusing_bigquery_api_base_url();
    let connection_store =
        BigQuerySessionStoreAdapter::new_native("phase_f_dataset", "phase_f_table")
            .with_project_id("phase-f-project")
            .with_access_token("phase-f-token")
            .with_api_base_url(base_url.clone())
            .with_http_timeout(Duration::from_millis(250));
    let (insert_done, clock_released) = std::sync::mpsc::channel::<()>();
    // Holds the paused clock still until the insert ends, so the 250 ms
    // timeout cannot fire first.
    let clock_hold = tokio::task::spawn_blocking(move || {
        let _ = clock_released.recv();
    });
    let connection_err = connection_store
        .stream_insert_rows(&rows)
        .await
        .expect_err("connection failure should surface as HTTP error");
    drop(insert_done);
    clock_hold.await.expect("the clock hold ends");
    let BigQuerySessionStoreError::Http(message) = &connection_err else {
        panic!("expected an HTTP error, got {connection_err:?}");
    };
    let insert_url = format!(
        "{base_url}/projects/phase-f-project/datasets/phase_f_dataset/tables/phase_f_table/insertAll"
    );
    assert!(
        message.starts_with(&format!(
            "reqwest::Error {{ kind: Request, url: {insert_url:?}, source: "
        )) && message.contains("kind: ConnectionRefused"),
        "expected the refused connection to {insert_url}, got `{message}`"
    );
}

#[test]
#[ignore] // requires real BigQuery credentials and network
fn phase_f_real_bigquery_integration_streaming_dedup_tombstone_semantics() {
    // Self-skip without a project or credentials (same idiom as the env-gated
    // tests in gateway_external.rs): a contributor without GCP access sees
    // SKIP, not FAIL, when running the ignored lane.
    let Some(project) = real_bigquery_project() else {
        eprintln!(
            "skipping phase_f_real_bigquery_integration_streaming_dedup_tombstone_semantics \
             because {REAL_BQ_PROJECT_ENV} is not set: set it to a GCP project in which the \
             credentials may create and delete BigQuery datasets"
        );
        return;
    };
    let Some(access_token) = bigquery_access_token() else {
        eprintln!(
            "skipping phase_f_real_bigquery_integration_streaming_dedup_tombstone_semantics \
             because no BigQuery credentials are available: set BIGQUERY_ACCESS_TOKEN (or \
             GOOGLE_OAUTH_ACCESS_TOKEN), or run `gcloud auth application-default login` for \
             project `{project}`"
        );
        return;
    };
    let api_base_url = std::env::var("BIGQUERY_API_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BQ_API_BASE_URL.to_string());

    let dataset_id = format!("phase_f_{}_{}", std::process::id(), unix_ms());
    let table_id = "sessions_phase_f";
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("build reqwest client");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime for BigQuery integration test");

    create_dataset(&client, &api_base_url, &project, &dataset_id, &access_token).unwrap_or_else(
        |err| {
            panic!(
                "failed to create dataset `{}` in project `{}` via BigQuery API: {}",
                dataset_id, project, err
            )
        },
    );
    let _cleanup = DatasetCleanupGuard::new(
        client.clone(),
        api_base_url.clone(),
        project.clone(),
        dataset_id.clone(),
        access_token.clone(),
    );

    create_table(
        &client,
        &api_base_url,
        &project,
        &dataset_id,
        table_id,
        &access_token,
    )
    .unwrap_or_else(|err| {
        panic!(
            "failed to create table `{}.{}` in project `{}` via BigQuery API: {}",
            dataset_id, table_id, project, err
        )
    });

    let store = BigQuerySessionStoreAdapter::new_native(dataset_id.clone(), table_id.to_string())
        .with_project_id(project)
        .with_access_token(access_token)
        .with_api_base_url(api_base_url);

    let writes = vec![
        SessionPersistenceRow {
            session_id: "s1".to_string(),
            updated_at_ms: 1_000,
            deleted: false,
            payload: json!({"step":"create"}),
            ..Default::default()
        },
        SessionPersistenceRow {
            session_id: "s1".to_string(),
            updated_at_ms: 2_000,
            deleted: true,
            payload: json!({}),
            ..Default::default()
        },
        SessionPersistenceRow {
            session_id: "s2".to_string(),
            updated_at_ms: 1_500,
            deleted: false,
            payload: json!({"step":"create"}),
            ..Default::default()
        },
        SessionPersistenceRow {
            session_id: "s2".to_string(),
            updated_at_ms: 3_000,
            deleted: false,
            payload: json!({"step":"update","version":2}),
            ..Default::default()
        },
    ];

    runtime
        .block_on(store.stream_insert_rows(&writes))
        .expect("stream insert rows into real BigQuery table");

    let expected_latest = vec![
        SessionPersistenceRow {
            session_id: "s1".to_string(),
            updated_at_ms: 2_000,
            deleted: true,
            payload: json!({}),
            ..Default::default()
        },
        SessionPersistenceRow {
            session_id: "s2".to_string(),
            updated_at_ms: 3_000,
            deleted: false,
            payload: json!({"step":"update","version":2}),
            ..Default::default()
        },
    ];
    let expected_live = vec![SessionPersistenceRow {
        session_id: "s2".to_string(),
        updated_at_ms: 3_000,
        deleted: false,
        payload: json!({"step":"update","version":2}),
        ..Default::default()
    }];

    let deadline = Instant::now() + Duration::from_mins(1);
    loop {
        let latest = runtime
            .block_on(store.read_latest_rows())
            .expect("query latest rows from real BigQuery");
        let live = runtime
            .block_on(store.read_live_rows())
            .expect("query live rows from real BigQuery");
        if latest == expected_latest && live == expected_live {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "BigQuery streaming rows did not converge to expected dedup+tombstone semantics within 60s.\nlatest={:?}\nlive={:?}",
                latest, live
            );
        }
        thread::sleep(Duration::from_secs(2));
    }
}

#[derive(Debug)]
struct DatasetCleanupGuard {
    client: reqwest::blocking::Client,
    api_base_url: String,
    project_id: String,
    dataset_id: String,
    access_token: String,
}

impl DatasetCleanupGuard {
    fn new(
        client: reqwest::blocking::Client,
        api_base_url: String,
        project_id: String,
        dataset_id: String,
        access_token: String,
    ) -> Self {
        Self {
            client,
            api_base_url,
            project_id,
            dataset_id,
            access_token,
        }
    }
}

impl Drop for DatasetCleanupGuard {
    fn drop(&mut self) {
        let endpoint = format!(
            "{}/projects/{}/datasets/{}?deleteContents=true",
            self.api_base_url.trim_end_matches('/'),
            self.project_id,
            self.dataset_id
        );
        let request = self
            .client
            .request(reqwest::Method::DELETE, &endpoint)
            .bearer_auth(&self.access_token);
        match request.send() {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => {
                let status = response.status();
                let body = response
                    .text()
                    .unwrap_or_else(|_| "<unable to read response body>".to_string());
                eprintln!(
                    "phase_f cleanup warning: failed to delete dataset `{}` (status {}): {}",
                    self.dataset_id,
                    status.as_u16(),
                    body
                );
            }
            Err(err) => {
                eprintln!(
                    "phase_f cleanup warning: failed to delete dataset `{}`: {}",
                    self.dataset_id, err
                );
            }
        }
    }
}

fn create_dataset(
    client: &reqwest::blocking::Client,
    api_base_url: &str,
    project_id: &str,
    dataset_id: &str,
    access_token: &str,
) -> Result<(), String> {
    let endpoint = format!(
        "{}/projects/{}/datasets",
        api_base_url.trim_end_matches('/'),
        project_id
    );
    let body = json!({
        "datasetReference": {
            "projectId": project_id,
            "datasetId": dataset_id,
        },
        "location": "US",
        "description": "meerkat-meta phase_f integration test dataset",
    });
    send_bigquery_json_request(
        client,
        reqwest::Method::POST,
        &endpoint,
        access_token,
        Some(body),
    )
    .map(|_| ())
}

fn create_table(
    client: &reqwest::blocking::Client,
    api_base_url: &str,
    project_id: &str,
    dataset_id: &str,
    table_id: &str,
    access_token: &str,
) -> Result<(), String> {
    let endpoint = format!(
        "{}/projects/{}/datasets/{}/tables",
        api_base_url.trim_end_matches('/'),
        project_id,
        dataset_id
    );
    let body = json!({
        "tableReference": {
            "projectId": project_id,
            "datasetId": dataset_id,
            "tableId": table_id,
        },
        "schema": {
            "fields": [
                {"name":"session_id","type":"STRING","mode":"REQUIRED"},
                {"name":"updated_at_ms","type":"INT64","mode":"REQUIRED"},
                {"name":"deleted","type":"BOOL","mode":"REQUIRED"},
                {"name":"payload","type":"STRING","mode":"NULLABLE"},
                {"name":"labels_json","type":"STRING","mode":"NULLABLE"}
            ]
        }
    });
    send_bigquery_json_request(
        client,
        reqwest::Method::POST,
        &endpoint,
        access_token,
        Some(body),
    )
    .map(|_| ())
}

fn send_bigquery_json_request(
    client: &reqwest::blocking::Client,
    method: reqwest::Method,
    endpoint: &str,
    access_token: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let mut request = client
        .request(method.clone(), endpoint)
        .bearer_auth(access_token)
        .header("accept", "application/json");
    if let Some(body) = body {
        request = request
            .header("content-type", "application/json")
            .json(&body);
    }

    let response = request
        .send()
        .map_err(|err| format!("request {} {} failed: {}", method.as_str(), endpoint, err))?;
    let status = response.status();
    let text = response.text().map_err(|err| {
        format!(
            "read response {} {} failed: {}",
            method.as_str(),
            endpoint,
            err
        )
    })?;
    if !status.is_success() {
        return Err(format!(
            "{} {} failed with status {}: {}",
            method.as_str(),
            endpoint,
            status.as_u16(),
            text
        ));
    }

    if text.trim().is_empty() {
        Ok(json!({}))
    } else {
        serde_json::from_str::<Value>(&text).map_err(|err| {
            format!(
                "invalid JSON response for {} {}: {}",
                method.as_str(),
                endpoint,
                err
            )
        })
    }
}

fn assert_configuration_error_contains(error: BigQuerySessionStoreError, expected: &str) {
    match error {
        BigQuerySessionStoreError::Configuration(message) => {
            assert!(
                message.contains(expected),
                "expected configuration error to contain `{expected}`, got `{message}`"
            );
        }
        other => panic!("expected configuration error, got {other:?}"),
    }
}

fn assert_api_error_contains(error: BigQuerySessionStoreError, expected: &str) {
    match error {
        BigQuerySessionStoreError::Api(message) => {
            assert!(
                message.contains(expected),
                "expected API error to contain `{expected}`, got `{message}`"
            );
        }
        other => panic!("expected API error, got {other:?}"),
    }
}

fn assert_invalid_query_error_contains(error: BigQuerySessionStoreError, expected: &str) {
    match error {
        BigQuerySessionStoreError::InvalidQueryResponse(message) => {
            assert!(
                message.contains(expected),
                "expected invalid query error to contain `{expected}`, got `{message}`"
            );
        }
        other => panic!("expected invalid query response error, got {other:?}"),
    }
}

/// The adapter's error text for a request to `url` that reqwest gave up on at
/// its timeout.
fn reqwest_timeout_error(url: &str) -> String {
    format!("reqwest::Error {{ kind: Request, url: {url:?}, source: TimedOut }}")
}

/// A loopback BigQuery base URL whose port refuses connections while the
/// returned socket lives: the socket holds the port bound, so nothing else
/// takes it, and does not listen.
fn refusing_bigquery_api_base_url() -> (tokio::net::TcpSocket, String) {
    let socket = tokio::net::TcpSocket::new_v4().expect("create a TCP socket");
    socket
        .bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .expect("bind a loopback port");
    let address = socket.local_addr().expect("resolve the bound address");
    (socket, format!("http://{address}/bigquery/v2"))
}

/// The GCP project named by [`REAL_BQ_PROJECT_ENV`], or `None` when it is
/// unset or blank: the real-BigQuery row then skips.
fn real_bigquery_project() -> Option<String> {
    std::env::var(REAL_BQ_PROJECT_ENV)
        .ok()
        .map(|project| project.trim().to_string())
        .filter(|project| !project.is_empty())
}

/// Resolve a BigQuery bearer token from the environment or gcloud ADC.
///
/// `None` means "no GCP credentials here" and the caller must SKIP, not
/// FAIL: contributors without access to the test project (and CI runners)
/// still run the ignored lane via `--ignored`, and a missing-prerequisite
/// panic reads as a red test rather than an environment gap.
fn bigquery_access_token() -> Option<String> {
    for key in [
        "BIGQUERY_ACCESS_TOKEN",
        "GOOGLE_OAUTH_ACCESS_TOKEN",
        "GOOGLE_ACCESS_TOKEN",
    ] {
        if let Ok(token) = std::env::var(key) {
            let token = token.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }

    if let Ok(output) = Command::new("gcloud")
        .args(["auth", "application-default", "print-access-token"])
        .output()
    {
        if output.status.success() {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                return Some(token);
            }
        }
    }

    None
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn mirror_update() -> meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorUpdate {
    use meerkat_meta::unified_runtime::continuity_mirror::{
        ContinuityMirrorRecord, ContinuityMirrorSession, ContinuityMirrorUpdate,
    };
    ContinuityMirrorUpdate {
        sequence: MIRROR_SEQUENCE,
        records: vec![
            ContinuityMirrorRecord {
                identity: "agent:alice".to_string(),
                agent_runtime_id: "rt:agent:alice:0".to_string(),
                session_id: "s-alice".to_string(),
                generation: 0,
                checkpoint_version: 0,
            },
            ContinuityMirrorRecord {
                identity: "agent:bob".to_string(),
                agent_runtime_id: "rt:agent:bob:2".to_string(),
                session_id: "s-bob".to_string(),
                generation: 2,
                checkpoint_version: 0,
            },
        ],
        changed_sessions: vec![ContinuityMirrorSession {
            session_id: "s-alice".to_string(),
            data: "eyJ2IjoxfQ==".to_string(),
        }],
        session_ids: std::collections::BTreeSet::from(["s-alice".to_string(), "s-bob".to_string()]),
    }
}

fn mirror_sink(
    server: &MockHttpServer,
) -> meerkat_meta::unified_runtime::continuity_mirror_bigquery::BigQueryMirrorSink {
    meerkat_meta::unified_runtime::continuity_mirror_bigquery::BigQueryMirrorSink::new(
        BigQuerySessionStoreAdapter::new_native("mirror_dataset", "unused_table")
            .with_project_id("mirror-project")
            .with_access_token("mirror-token")
            .with_api_base_url(format!("{}/bigquery/v2", server.base_url())),
        "continuity",
    )
}

const MIRROR_QUERY_PATH: &str = "/bigquery/v2/projects/mirror-project/queries";

/// The sequence of `mirror_update()`: 2026-10-20 in wall-clock milliseconds.
const MIRROR_SEQUENCE: u64 = 1_792_454_400_000;

fn mirror_table_path(name: &str) -> String {
    format!("/bigquery/v2/projects/mirror-project/datasets/mirror_dataset/tables/{name}/insertAll")
}

fn mirror_request_paths(requests: &[bigquery_http_mock::CapturedHttpRequest]) -> Vec<String> {
    requests
        .iter()
        .map(|request| request.path.clone())
        .collect()
}

fn mirror_request_rows(request: &bigquery_http_mock::CapturedHttpRequest) -> Vec<Value> {
    let body: Value = serde_json::from_str(&request.body).expect("insert body");
    body["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["json"].clone())
        .collect()
}

fn mirror_query_text(request: &bigquery_http_mock::CapturedHttpRequest) -> String {
    let body: Value = serde_json::from_str(&request.body).expect("query body");
    body["query"].as_str().expect("query text").to_string()
}

fn mirror_snapshot_ms(row: &Value) -> u64 {
    row["snapshot_ms"]
        .as_str()
        .expect("snapshot_ms")
        .parse::<u64>()
        .expect("decimal snapshot_ms")
}

/// The answer to the newest-snapshot query when `<p>_snapshots` is empty.
fn mirror_no_snapshot_yet() -> MockHttpResponse {
    MockHttpResponse::json(json!({"jobComplete": true, "rows": [{"f": [{"v": null}]}]}))
}

/// The first `put` reads the newest snapshots row, then streams the changed
/// sessions, then every record, then one snapshots row naming the shared
/// `snapshot_ms` and the row counts, each into its own table of the dataset;
/// a session row carries its data as part 0 of 1.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_writes_sessions_records_then_the_snapshot_row() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let server = MockHttpServer::start(vec![
        mirror_no_snapshot_yet(),
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(json!({})),
    ]);
    mirror_sink(&server)
        .put(&mirror_update())
        .await
        .expect("the snapshot streams into BigQuery");

    let requests = server.captured_requests();
    assert_eq!(
        mirror_request_paths(&requests),
        vec![
            MIRROR_QUERY_PATH.to_string(),
            mirror_table_path("continuity_sessions"),
            mirror_table_path("continuity_records"),
            mirror_table_path("continuity_snapshots"),
        ]
    );
    assert_eq!(
        mirror_query_text(&requests[0]),
        "SELECT MAX(snapshot_ms) FROM `mirror-project.mirror_dataset.continuity_snapshots`"
    );
    let sessions = mirror_request_rows(&requests[1]);
    let records = mirror_request_rows(&requests[2]);
    let snapshot = mirror_request_rows(&requests[3]);
    let snapshot_ms = snapshot[0]["snapshot_ms"].clone();
    assert_eq!(snapshot_ms, json!(MIRROR_SEQUENCE.to_string()));
    assert_eq!(
        snapshot,
        vec![json!({
            "snapshot_ms": snapshot_ms.clone(),
            "records": 2,
            "sessions": 2,
            "changed_sessions": 1,
        })]
    );
    assert_eq!(
        sessions,
        vec![json!({
            "session_id": "s-alice",
            "data": "eyJ2IjoxfQ==",
            "part": 0,
            "parts": 1,
            "snapshot_ms": snapshot_ms.clone(),
        })]
    );
    assert_eq!(
        records,
        vec![
            json!({
                "identity": "agent:alice",
                "agent_runtime_id": "rt:agent:alice:0",
                "session_id": "s-alice",
                "generation": 0,
                "checkpoint_version": 0,
                "snapshot_ms": snapshot_ms.clone(),
            }),
            json!({
                "identity": "agent:bob",
                "agent_runtime_id": "rt:agent:bob:2",
                "session_id": "s-bob",
                "generation": 2,
                "checkpoint_version": 0,
                "snapshot_ms": snapshot_ms,
            }),
        ]
    );
    assert!(requests.iter().all(|request| {
        request.headers.get("authorization").map(String::as_str) == Some("Bearer mirror-token")
    }));
}

/// A snapshots row the table already holds above the update's sequence (a
/// wall clock that stepped back across a restart) numbers the next `put`
/// above it.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_numbers_puts_above_the_newest_snapshot_it_finds() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let ahead = MIRROR_SEQUENCE + 86_400_000;
    let server = MockHttpServer::start(vec![
        MockHttpResponse::json(json!({
            "jobComplete": true,
            "rows": [{"f": [{"v": ahead.to_string()}]}],
        })),
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(json!({})),
    ]);
    mirror_sink(&server)
        .put(&mirror_update())
        .await
        .expect("the snapshot streams into BigQuery");

    let requests = server.captured_requests();
    assert_eq!(
        mirror_snapshot_ms(&mirror_request_rows(&requests[3])[0]),
        ahead + 1
    );
}

/// A table insert that BigQuery answers with row errors fails the `put`, and
/// no snapshots row is written, so a reader never takes the partial rows.
/// The next `put` of the same update writes its rows again under a newer
/// `snapshot_ms`, and does not read the newest snapshots row again.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_writes_no_snapshot_row_when_an_insert_fails() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let mut responses = vec![
        mirror_no_snapshot_yet(),
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(json!({
            "insertErrors": [{"index": 0, "errors": [{"reason": "invalid"}]}]
        })),
    ];
    responses.extend((0..3).map(|_| MockHttpResponse::json(json!({}))));
    let server = MockHttpServer::start(responses);
    let sink = mirror_sink(&server);
    let error = sink
        .put(&mirror_update())
        .await
        .expect_err("row errors fail the put");
    assert!(error.contains("continuity_records"), "{error}");
    sink.put(&mirror_update())
        .await
        .expect("the next put streams the snapshot again");

    let requests = server.captured_requests();
    assert_eq!(
        mirror_request_paths(&requests),
        vec![
            MIRROR_QUERY_PATH.to_string(),
            mirror_table_path("continuity_sessions"),
            mirror_table_path("continuity_records"),
            mirror_table_path("continuity_sessions"),
            mirror_table_path("continuity_records"),
            mirror_table_path("continuity_snapshots"),
        ]
    );
    let first = mirror_snapshot_ms(&mirror_request_rows(&requests[1])[0]);
    let second = mirror_snapshot_ms(&mirror_request_rows(&requests[3])[0]);
    assert!(first < second);
    assert_eq!(
        second,
        mirror_snapshot_ms(&mirror_request_rows(&requests[5])[0])
    );
}

/// Session data longer than 4 MiB goes out as rows of at most 4 MiB, numbered
/// by `part` with the count in `parts`, that join back into the data.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_splits_long_session_data_into_parts_that_rejoin() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    const PART_BYTES: usize = 4 * 1024 * 1024;
    let server = MockHttpServer::start(vec![
        mirror_no_snapshot_yet(),
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(json!({})),
    ]);
    let mut update = mirror_update();
    let data = format!("{}QUJD", "A".repeat(PART_BYTES));
    update.changed_sessions[0].data = data.clone();
    mirror_sink(&server)
        .put(&update)
        .await
        .expect("the long session streams into BigQuery");

    let requests = server.captured_requests();
    assert_eq!(
        mirror_request_paths(&requests)[1],
        mirror_table_path("continuity_sessions")
    );
    let sessions = mirror_request_rows(&requests[1]);
    assert_eq!(sessions.len(), 2);
    for (index, row) in sessions.iter().enumerate() {
        assert_eq!(row["session_id"], json!("s-alice"));
        assert_eq!(row["part"], json!(index));
        assert_eq!(row["parts"], json!(2));
    }
    let pieces: Vec<&str> = sessions
        .iter()
        .map(|row| row["data"].as_str().expect("data"))
        .collect();
    assert_eq!(pieces[0].len(), PART_BYTES);
    assert_eq!(pieces.concat(), data);
}

/// A `put` that has not completed within its deadline fails, and writes no
/// snapshots row. The mock holds the put's first request, the newest-snapshot
/// query, unanswered, and the paused clock does not move until the mock has
/// it, so the deadline passes while the put waits on BigQuery, however long
/// the query takes to go out.
#[tokio::test(start_paused = true)]
async fn continuity_mirror_bigquery_sink_fails_a_put_past_its_deadline() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let (server, query_receipt) = MockHttpServer::start_holding();
    let deadline = Duration::from_millis(200);
    let sink = mirror_sink(&server).with_put_deadline(deadline);
    let update = mirror_update();
    let started = tokio::time::Instant::now();
    let put = sink.put(&update);
    tokio::pin!(put);
    // The paused clock does not advance while a blocking task runs, so the
    // deadline cannot pass before the mock has the query.
    let query_received = tokio::task::spawn_blocking(move || query_receipt.wait());
    tokio::select! {
        biased;
        received = query_received => received.expect("the mock captures the query"),
        result = &mut put => panic!("the put ended before the mock had its query: {result:?}"),
    }
    let error = put.await.expect_err("the put misses its deadline");
    assert_eq!(
        error,
        format!("the BigQuery mirror put did not complete within {deadline:?}")
    );
    let elapsed = started.elapsed();
    assert!(
        (deadline..=deadline + Duration::from_millis(1)).contains(&elapsed),
        "the put failed {elapsed:?} after it started, not at its {deadline:?} deadline"
    );
    assert_eq!(
        mirror_request_paths(&server.captured_requests()),
        vec![MIRROR_QUERY_PATH.to_string()]
    );
}

/// A `put` with no changed session writes no session row: only the records
/// and the snapshots row, which still counts every session the records name.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_writes_only_records_when_no_session_changed() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let server = MockHttpServer::start(vec![
        mirror_no_snapshot_yet(),
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(json!({})),
    ]);
    let mut update = mirror_update();
    update.changed_sessions.clear();
    mirror_sink(&server)
        .put(&update)
        .await
        .expect("the records stream into BigQuery");

    let requests = server.captured_requests();
    assert_eq!(
        mirror_request_paths(&requests),
        vec![
            MIRROR_QUERY_PATH.to_string(),
            mirror_table_path("continuity_records"),
            mirror_table_path("continuity_snapshots"),
        ]
    );
    let snapshot = mirror_request_rows(&requests[2]);
    assert_eq!(snapshot[0]["sessions"], json!(2));
    assert_eq!(snapshot[0]["changed_sessions"], json!(0));
}

/// A token source handing out `token-1`, `token-2`, ... in fetch order, each
/// valid for `lifetime`, and the count of its fetches.
fn mirror_counting_token_source(
    lifetime: Duration,
) -> (
    meerkat_meta::BigQueryAccessTokenSource,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    let fetches = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&fetches);
    let source = meerkat_meta::BigQueryAccessTokenSource::new(move || {
        let fetch = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        async move {
            Ok(meerkat_meta::BigQueryAccessToken {
                token: format!("token-{fetch}"),
                expires_at: Some(SystemTime::now() + lifetime),
            })
        }
    });
    (source, fetches)
}

fn mirror_sink_with_token_source(
    server: &MockHttpServer,
    source: meerkat_meta::BigQueryAccessTokenSource,
) -> meerkat_meta::unified_runtime::continuity_mirror_bigquery::BigQueryMirrorSink {
    meerkat_meta::unified_runtime::continuity_mirror_bigquery::BigQueryMirrorSink::new(
        BigQuerySessionStoreAdapter::new_native("mirror_dataset", "unused_table")
            .with_project_id("mirror-project")
            .with_access_token("fixed-token")
            .with_access_token_source(source)
            .with_api_base_url(format!("{}/bigquery/v2", server.base_url())),
        "continuity",
    )
}

fn mirror_bearers(server: &MockHttpServer) -> Vec<String> {
    server
        .captured_requests()
        .iter()
        .map(|request| {
            request
                .headers
                .get("authorization")
                .cloned()
                .expect("authorization header")
        })
        .collect()
}

fn mirror_unauthorized() -> MockHttpResponse {
    MockHttpResponse {
        status_code: 401,
        ..MockHttpResponse::json(json!({
            "error": {"code": 401, "message": "Request had invalid authentication credentials."}
        }))
    }
}

/// A token source's token is cached: one fetch serves every request of a
/// `put`, and it wins over a fixed token.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_caches_the_token_source_token() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let server = MockHttpServer::start(vec![
        mirror_no_snapshot_yet(),
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(json!({})),
    ]);
    let (source, fetches) = mirror_counting_token_source(Duration::from_hours(1));
    mirror_sink_with_token_source(&server, source)
        .put(&mirror_update())
        .await
        .expect("the snapshot streams into BigQuery");

    assert_eq!(
        mirror_bearers(&server),
        vec!["Bearer token-1".to_string(); 4]
    );
    assert_eq!(fetches.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// A token that expires while the mirror runs is replaced: the next `put`
/// completes with the token the source fetches anew.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_puts_with_a_new_token_after_the_old_one_expires() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let mut responses = vec![mirror_no_snapshot_yet()];
    responses.extend((0..6).map(|_| MockHttpResponse::json(json!({}))));
    let server = MockHttpServer::start(responses);
    let (source, fetches) = mirror_counting_token_source(Duration::from_millis(400));
    let sink = mirror_sink_with_token_source(&server, source.with_refresh_margin(Duration::ZERO));
    sink.put(&mirror_update())
        .await
        .expect("the first put streams with the first token");
    tokio::time::sleep(Duration::from_millis(600)).await;
    let mut update = mirror_update();
    update.sequence += 1;
    sink.put(&update)
        .await
        .expect("the next put streams with a new token");

    let requests = server.captured_requests();
    assert_eq!(
        mirror_request_paths(&requests)[4..],
        [
            mirror_table_path("continuity_sessions"),
            mirror_table_path("continuity_records"),
            mirror_table_path("continuity_snapshots"),
        ]
    );
    let mut expected = vec!["Bearer token-1".to_string(); 4];
    expected.extend(vec!["Bearer token-2".to_string(); 3]);
    assert_eq!(mirror_bearers(&server), expected);
    assert_eq!(fetches.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(sink.auth_failures(), 0);
}

/// An insert BigQuery refuses with 401 was not applied, so the adapter
/// fetches a new token and sends that insert once more; the `put` completes
/// with its snapshots row last.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_sends_a_refused_insert_again_with_a_new_token() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let server = MockHttpServer::start(vec![
        mirror_no_snapshot_yet(),
        mirror_unauthorized(),
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(json!({})),
    ]);
    let (source, fetches) = mirror_counting_token_source(Duration::from_hours(1));
    let sink = mirror_sink_with_token_source(&server, source);
    sink.put(&mirror_update())
        .await
        .expect("the put completes with the new token");

    assert_eq!(
        mirror_request_paths(&server.captured_requests()),
        vec![
            MIRROR_QUERY_PATH.to_string(),
            mirror_table_path("continuity_sessions"),
            mirror_table_path("continuity_sessions"),
            mirror_table_path("continuity_records"),
            mirror_table_path("continuity_snapshots"),
        ]
    );
    assert_eq!(
        mirror_bearers(&server),
        vec![
            "Bearer token-1".to_string(),
            "Bearer token-1".to_string(),
            "Bearer token-2".to_string(),
            "Bearer token-2".to_string(),
            "Bearer token-2".to_string(),
        ]
    );
    assert_eq!(fetches.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(sink.auth_failures(), 0);
}

/// A 401 that comes back after the refresh fails the `put` as an access token
/// failure, with no token text in the error, and is counted.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_fails_a_put_when_bigquery_refuses_the_new_token_too() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let server = MockHttpServer::start(vec![mirror_unauthorized(), mirror_unauthorized()]);
    let (source, _fetches) = mirror_counting_token_source(Duration::from_hours(1));
    let sink = mirror_sink_with_token_source(&server, source);
    let error = sink
        .put(&mirror_update())
        .await
        .expect_err("a second 401 fails the put");
    assert!(
        error.contains("BigQuery access token refused or unavailable"),
        "{error}"
    );
    assert!(error.contains("again after a refresh"), "{error}");
    assert!(!error.contains("token-"), "{error}");
    assert_eq!(server.captured_requests().len(), 2);
    assert_eq!(sink.auth_failures(), 1);
}

/// A token source that fails fails the `put` as an access token failure
/// before any request is sent.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_fails_a_put_when_the_token_source_fails() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let server = MockHttpServer::start(Vec::new());
    let sink = mirror_sink_with_token_source(
        &server,
        meerkat_meta::BigQueryAccessTokenSource::new(|| async {
            Err("the metadata server did not answer".to_string())
        }),
    );
    let error = sink
        .put(&mirror_update())
        .await
        .expect_err("the token source fails the put");
    assert!(
        error.contains("BigQuery access token refused or unavailable"),
        "{error}"
    );
    assert!(
        error.contains("the metadata server did not answer"),
        "{error}"
    );
    assert!(server.captured_requests().is_empty());
    assert_eq!(sink.auth_failures(), 1);
}

/// Only a 401 is sent again: a 500 fails the `put` after one attempt, with no
/// new token.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_sends_a_failed_request_once() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let server = MockHttpServer::start(vec![MockHttpResponse {
        status_code: 500,
        ..MockHttpResponse::json(json!({"error": {"code": 500}}))
    }]);
    let (source, fetches) = mirror_counting_token_source(Duration::from_hours(1));
    let sink = mirror_sink_with_token_source(&server, source);
    let error = sink
        .put(&mirror_update())
        .await
        .expect_err("a 500 fails the put");
    assert!(error.contains("status 500"), "{error}");
    assert_eq!(server.captured_requests().len(), 1);
    assert_eq!(fetches.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(sink.auth_failures(), 0);
}

fn mirror_blob(
    blob_id: &str,
) -> meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorBlob {
    meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorBlob {
        blob_id: blob_id.to_string(),
        media_type: "image/png".to_string(),
        data: "iVBORw0KGgo=".to_string(),
    }
}

fn mirror_blob_id(fill: &str) -> String {
    format!("sha256:{}", fill.repeat(64))
}

/// The first `put_blob` reads which blobs the blobs table already holds a
/// complete copy of; a blob it holds is not written again, a new one is
/// written once as part rows, and a repeated id makes no request.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_puts_each_blob_once_and_skips_blobs_bigquery_holds() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let held = mirror_blob_id("a");
    let fresh = mirror_blob_id("b");
    let server = MockHttpServer::start(vec![
        MockHttpResponse::json(json!({
            "jobComplete": true,
            "rows": [{"f": [{"v": held.clone()}]}, {"f": [{"v": held.clone()}]}],
        })),
        MockHttpResponse::json(json!({})),
    ]);
    let sink = mirror_sink(&server);
    sink.put_blob(&mirror_blob(&held))
        .await
        .expect("a blob BigQuery holds is kept");
    sink.put_blob(&mirror_blob(&fresh))
        .await
        .expect("a new blob streams into BigQuery");
    sink.put_blob(&mirror_blob(&fresh))
        .await
        .expect("a repeated blob id is kept");

    let requests = server.captured_requests();
    assert_eq!(
        mirror_request_paths(&requests),
        vec![
            MIRROR_QUERY_PATH.to_string(),
            mirror_table_path("continuity_blobs"),
        ]
    );
    assert_eq!(
        mirror_query_text(&requests[0]),
        "SELECT blob_id FROM `mirror-project.mirror_dataset.continuity_blobs` \
         GROUP BY blob_id, snapshot_ms HAVING COUNT(*) = ANY_VALUE(parts)"
    );
    let rows = mirror_request_rows(&requests[1]);
    assert_eq!(rows.len(), 1);
    let snapshot_ms = rows[0]["snapshot_ms"].clone();
    assert!(
        snapshot_ms
            .as_str()
            .is_some_and(|ms| ms.parse::<u64>().is_ok())
    );
    assert_eq!(
        rows[0],
        json!({
            "blob_id": fresh,
            "media_type": "image/png",
            "data": "iVBORw0KGgo=",
            "part": 0,
            "parts": 1,
            "snapshot_ms": snapshot_ms,
        })
    );
}

/// The held blob ids query waits for an unfinished job and reads every
/// result page before the sink decides which blobs to write.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_reads_every_page_of_the_held_blob_ids() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let first = mirror_blob_id("c");
    let second = mirror_blob_id("d");
    let job = json!({"projectId": "mirror-project", "jobId": "job-1", "location": "EU"});
    let server = MockHttpServer::start(vec![
        MockHttpResponse::json(json!({"jobComplete": false, "jobReference": job.clone()})),
        MockHttpResponse::json(json!({
            "jobComplete": true,
            "jobReference": job.clone(),
            "rows": [{"f": [{"v": first.clone()}]}],
            "pageToken": "page-2",
        })),
        MockHttpResponse::json(json!({
            "jobComplete": true,
            "jobReference": job,
            "rows": [{"f": [{"v": second.clone()}]}],
        })),
    ]);
    let sink = mirror_sink(&server);
    sink.put_blob(&mirror_blob(&second))
        .await
        .expect("a blob on the last page is kept");
    sink.put_blob(&mirror_blob(&first))
        .await
        .expect("a blob on the first page is kept");

    let requests = server.captured_requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| format!("{} {}", request.method, request.path))
            .collect::<Vec<_>>(),
        vec![
            format!("POST {MIRROR_QUERY_PATH}"),
            format!("GET {MIRROR_QUERY_PATH}/job-1?maxResults=10000&location=EU"),
            format!("GET {MIRROR_QUERY_PATH}/job-1?maxResults=10000&location=EU&pageToken=page-2"),
        ]
    );
}

/// A blob whose insert fails is written again by the next `put_blob` of its
/// id, without reading the held ids again.
#[tokio::test]
async fn continuity_mirror_bigquery_sink_writes_a_blob_again_after_its_insert_fails() {
    use meerkat_meta::unified_runtime::continuity_mirror::ContinuityMirrorSink as _;
    let blob_id = mirror_blob_id("e");
    let server = MockHttpServer::start(vec![
        MockHttpResponse::json(json!({"jobComplete": true})),
        MockHttpResponse::json(json!({
            "insertErrors": [{"index": 0, "errors": [{"reason": "invalid"}]}]
        })),
        MockHttpResponse::json(json!({})),
    ]);
    let sink = mirror_sink(&server);
    let error = sink
        .put_blob(&mirror_blob(&blob_id))
        .await
        .expect_err("row errors fail the blob put");
    assert!(error.contains("continuity_blobs"), "{error}");
    sink.put_blob(&mirror_blob(&blob_id))
        .await
        .expect("the next blob put writes the blob");

    assert_eq!(
        mirror_request_paths(&server.captured_requests()),
        vec![
            MIRROR_QUERY_PATH.to_string(),
            mirror_table_path("continuity_blobs"),
            mirror_table_path("continuity_blobs"),
        ]
    );
}
