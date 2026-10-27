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

use meerkat_meta::BigQueryGcConfig;
use meerkat_meta::{BigQuerySessionStoreAdapter, BigQuerySessionStoreError};
use serde_json::{Value, json};

#[path = "support/bigquery_http_mock.rs"]
mod bigquery_http_mock;

use bigquery_http_mock::{MockHttpResponse, MockHttpServer};

fn build_mock_adapter(server: &MockHttpServer) -> BigQuerySessionStoreAdapter {
    BigQuerySessionStoreAdapter::new_native("gc_dataset", "gc_table")
        .with_project_id("gc-project")
        .with_access_token("gc-token")
        .with_api_base_url(format!("{}/bigquery/v2", server.base_url()))
}

#[tokio::test]
async fn gc_superseded_rows_sends_correct_dml_query() {
    let server = MockHttpServer::start(vec![MockHttpResponse::json(json!({
        "jobComplete": true,
        "numDmlAffectedRows": "3"
    }))]);
    let store = build_mock_adapter(&server);

    let deleted = store
        .gc_superseded_rows()
        .await
        .expect("gc_superseded_rows should succeed");

    assert_eq!(deleted, 3);

    let requests = server.captured_requests();
    assert_eq!(requests.len(), 1);

    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/bigquery/v2/projects/gc-project/queries");
    assert_eq!(
        request
            .headers
            .get("authorization")
            .expect("authorization header"),
        "Bearer gc-token"
    );

    let body: Value = serde_json::from_str(&request.body).expect("parse request body");
    assert_eq!(body["useLegacySql"], json!(false));

    let query = body["query"].as_str().expect("query string");
    assert!(
        query.contains("DELETE FROM"),
        "expected DELETE FROM in query, got: {query}"
    );
    assert!(
        query.contains("`gc-project.gc_dataset.gc_table`"),
        "expected backtick-quoted table ref, got: {query}"
    );
    assert!(
        query.contains("NOT IN"),
        "expected NOT IN subquery, got: {query}"
    );
    assert!(
        query.contains("MAX(updated_at_ms)"),
        "expected MAX(updated_at_ms) in subquery, got: {query}"
    );
    assert!(
        query.contains("GROUP BY session_id"),
        "expected GROUP BY session_id, got: {query}"
    );
}

#[tokio::test]
async fn truncate_sessions_sends_correct_truncate_query() {
    let server = MockHttpServer::start(vec![MockHttpResponse::json(json!({
        "jobComplete": true
    }))]);
    let store = build_mock_adapter(&server);

    store
        .truncate_sessions()
        .await
        .expect("truncate_sessions should succeed");

    let requests = server.captured_requests();
    assert_eq!(requests.len(), 1);

    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/bigquery/v2/projects/gc-project/queries");
    assert_eq!(
        request
            .headers
            .get("authorization")
            .expect("authorization header"),
        "Bearer gc-token"
    );

    let body: Value = serde_json::from_str(&request.body).expect("parse request body");
    assert_eq!(body["useLegacySql"], json!(false));

    let query = body["query"].as_str().expect("query string");
    assert!(
        query.contains("TRUNCATE TABLE"),
        "expected TRUNCATE TABLE in query, got: {query}"
    );
    assert!(
        query.contains("`gc-project.gc_dataset.gc_table`"),
        "expected backtick-quoted table ref, got: {query}"
    );
}

#[tokio::test]
async fn gc_superseded_rows_returns_affected_row_count() {
    let server = MockHttpServer::start(vec![MockHttpResponse::json(json!({
        "jobComplete": true,
        "numDmlAffectedRows": "7"
    }))]);
    let store = build_mock_adapter(&server);

    let deleted = store
        .gc_superseded_rows()
        .await
        .expect("gc_superseded_rows should succeed");
    assert_eq!(deleted, 7);
}

#[tokio::test]
async fn gc_superseded_rows_returns_zero_when_no_affected_rows_field() {
    let server = MockHttpServer::start(vec![MockHttpResponse::json(json!({
        "jobComplete": true
    }))]);
    let store = build_mock_adapter(&server);

    let deleted = store
        .gc_superseded_rows()
        .await
        .expect("gc_superseded_rows should succeed");
    assert_eq!(deleted, 0);
}

#[tokio::test]
async fn gc_superseded_rows_propagates_api_error_on_non_success_status() {
    let server = MockHttpServer::start(vec![MockHttpResponse {
        status_code: 403,
        content_type: "application/json".to_string(),
        body: json!({"error":{"message":"access denied"}}).to_string(),
        response_delay: Duration::from_millis(0),
    }]);
    let store = build_mock_adapter(&server);

    let err = store
        .gc_superseded_rows()
        .await
        .expect_err("gc should fail on 403");
    match err {
        BigQuerySessionStoreError::Api(msg) => {
            assert!(
                msg.contains("status 403"),
                "expected status 403, got: {msg}"
            );
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn truncate_sessions_propagates_api_error_on_non_success_status() {
    let server = MockHttpServer::start(vec![MockHttpResponse {
        status_code: 500,
        content_type: "application/json".to_string(),
        body: json!({"error":{"message":"internal error"}}).to_string(),
        response_delay: Duration::from_millis(0),
    }]);
    let store = build_mock_adapter(&server);

    let err = store
        .truncate_sessions()
        .await
        .expect_err("truncate should fail on 500");
    match err {
        BigQuerySessionStoreError::Api(msg) => {
            assert!(
                msg.contains("status 500"),
                "expected status 500, got: {msg}"
            );
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[test]
fn gc_config_defaults_to_six_hours() {
    let config = BigQueryGcConfig::default();
    assert_eq!(config.interval, Duration::from_hours(6));
}

#[test]
fn gc_config_custom_interval() {
    let config = BigQueryGcConfig {
        interval: Duration::from_mins(5),
    };
    assert_eq!(config.interval, Duration::from_mins(5));
}

#[tokio::test]
async fn gc_superseded_rows_requires_project_id() {
    let store = BigQuerySessionStoreAdapter::new_native("gc_dataset", "gc_table")
        .with_access_token("gc-token");

    let err = store
        .gc_superseded_rows()
        .await
        .expect_err("missing project_id should fail");
    match err {
        BigQuerySessionStoreError::Configuration(msg) => {
            assert!(
                msg.contains("missing BigQuery project_id"),
                "expected project_id error, got: {msg}"
            );
        }
        other => panic!("expected Configuration error, got {other:?}"),
    }
}

#[tokio::test]
async fn truncate_sessions_requires_access_token() {
    let store = BigQuerySessionStoreAdapter::new_native("gc_dataset", "gc_table")
        .with_project_id("gc-project");

    let err = store
        .truncate_sessions()
        .await
        .expect_err("missing access_token should fail");
    match err {
        BigQuerySessionStoreError::Configuration(msg) => {
            assert!(
                msg.contains("missing BigQuery access token"),
                "expected access token error, got: {msg}"
            );
        }
        other => panic!("expected Configuration error, got {other:?}"),
    }
}
