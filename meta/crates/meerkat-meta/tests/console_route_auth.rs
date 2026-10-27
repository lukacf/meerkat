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
use meerkat_meta::{
    AuthPolicy, AuthProvider, BigQueryNaming, ConsolePolicy, ConsoleRestJsonRequest,
    RuntimeDecisionInputs, RuntimeOpsPolicy, TrustedOidcRuntimeConfig,
    build_runtime_decision_state, handle_console_rest_json_route,
};
use serde_json::{Value, json};

fn release_json() -> String {
    include_str!("../assets/release-targets.json").to_string()
}

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U4LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
            .to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}

fn trusted_toml() -> String {
    r#"
[[modules]]
id = "router"
command = "router-bin"
args = ["--mode", "fast"]
restart_policy = "always"

[[modules]]
id = "delivery"
command = "delivery-bin"
args = ["--sink", "test"]
restart_policy = "on_failure"
"#
    .to_string()
}

fn decision_state() -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "phase0_contract_dataset".to_string(),
            table: "phase0_contract_table".to_string(),
        },
        trusted_meta_toml: trusted_toml(),
        auth: AuthPolicy {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: vec!["alice@example.com".to_string()],
        },
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth: false,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: release_json(),
    })
    .expect("decision state builds")
}

async fn voice_rpc(
    state: meerkat_meta::RuntimeDecisionState,
    method: &str,
    authenticated: bool,
) -> (axum::http::StatusCode, Value) {
    voice_rpc_with_params(
        state,
        method,
        authenticated,
        json!({"identity":"agent-a", "request_id":"request-a"}),
        None,
    )
    .await
}

async fn voice_rpc_with_params(
    mut state: meerkat_meta::RuntimeDecisionState,
    method: &str,
    authenticated: bool,
    params: Value,
    access: Option<meerkat_meta::AccessController>,
) -> (axum::http::StatusCode, Value) {
    use tower::ServiceExt;
    state.trusted_oidc.discovery_json = json!({
        "issuer": "https://trusted.meta.localhost",
        "jwks_uri": "https://trusted.meta.localhost/.well-known/jwks.json"
    })
    .to_string();
    let app = meerkat_meta::console_json_router_with_aggregator_and_access(
        state,
        meerkat_meta::MetaConsoleAggregator::new(std::sync::Arc::new(
            meerkat_meta::InMemoryConsoleLogStore::default(),
        )),
        access,
    )
    .layer(axum::Extension(
        meerkat_meta::console_voice::ConsoleVoiceController::default(),
    ));
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header("content-type", "application/json");
    if authenticated {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        header.kid = Some("kid-current".to_string());
        let token = jsonwebtoken::encode(
            &header,
            &json!({
                "iss": "https://trusted.meta.localhost",
                "aud": "meerkat-console",
                "sub": "alice@example.com",
                "email": "alice@example.com",
                "provider": "google_oauth",
                "exp": chrono::Utc::now().timestamp() + 300
            }),
            &jsonwebtoken::EncodingKey::from_secret(b"phase8-trusted-current-secret"),
        )
        .expect("token");
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .oneshot(
            request
                .body(axum::body::Body::from(
                    json!({
                        "jsonrpc": "2.0", "id": 1, "method": method,
                        "params": params
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    (status, serde_json::from_slice(&body).expect("json"))
}

#[tokio::test]
async fn console_experience_storage_scope_tracks_authenticated_principal() {
    use tower::ServiceExt;
    let mut state = decision_state();
    state.console.require_app_auth = true;
    state.auth.email_allowlist.push("bob@example.com".into());
    state.trusted_oidc.discovery_json = json!({
        "issuer": "https://trusted.meta.localhost",
        "jwks_uri": "https://trusted.meta.localhost/.well-known/jwks.json"
    })
    .to_string();
    let app = meerkat_meta::console_json_router_with_aggregator_and_access(
        state,
        meerkat_meta::MetaConsoleAggregator::new(std::sync::Arc::new(
            meerkat_meta::InMemoryConsoleLogStore::default(),
        )),
        None,
    );
    let mut scopes = Vec::new();
    for principal in [
        Some("alice@example.com"),
        Some("alice@example.com"),
        Some("bob@example.com"),
        None,
    ] {
        let mut request = axum::http::Request::builder().uri("/console/experience");
        if let Some(principal) = principal {
            let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
            header.kid = Some("kid-current".into());
            let token = jsonwebtoken::encode(
                &header,
                &json!({
                    "iss":"https://trusted.meta.localhost", "aud":"meerkat-console",
                    "sub":principal, "email":principal, "provider":"google_oauth",
                    "exp":chrono::Utc::now().timestamp()+300
                }),
                &jsonwebtoken::EncodingKey::from_secret(b"phase8-trusted-current-secret"),
            )
            .expect("token");
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = app
            .clone()
            .oneshot(request.body(axum::body::Body::empty()).expect("request"))
            .await
            .expect("response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 128 * 1024)
            .await
            .expect("body");
        let body: Value = serde_json::from_slice(&bytes).expect("JSON");
        if principal.is_none() {
            assert!(!status.is_success());
            assert!(
                body.get("storage_scope").is_none(),
                "unauthorized responses have no persistence authority"
            );
        } else {
            assert_eq!(status, axum::http::StatusCode::OK, "{body}");
            scopes.push(
                body["storage_scope"]
                    .as_str()
                    .expect("authorized scope")
                    .to_owned(),
            );
        }
    }
    assert_eq!(scopes[0], scopes[1]);
    assert_ne!(scopes[0], scopes[2]);
    assert!(
        scopes
            .iter()
            .all(|scope| scope.starts_with("console-storage:v1:") && !scope.contains('@'))
    );
}

#[tokio::test]
async fn console_voice_readiness_is_target_scoped_and_false_without_a_host() {
    let contract: Value = serde_json::from_str(include_str!("fixtures/console_voice_v1.json"))
        .expect("shared voice contract");
    let mut state = decision_state();
    state.console.require_app_auth = true;
    let (_, readiness) = voice_rpc_with_params(
        state.clone(),
        "meta/console/voice/readiness",
        true,
        json!({"identity":"agent-a"}),
        None,
    )
    .await;
    assert_eq!(readiness["result"], contract["readiness_unavailable"]);
    let (_, malformed) = voice_rpc_with_params(
        state,
        "meta/console/voice/readiness",
        true,
        json!({"identity":"agent-a", "request_id":"unexpected"}),
        None,
    )
    .await;
    assert_eq!(malformed["error"]["code"], -32602);
}

#[tokio::test]
async fn console_voice_channel_activation_rechecks_agent_permissions_before_dispatch() {
    let mut state = decision_state();
    state.console.require_app_auth = true;
    let access = meerkat_meta::AccessController::new(meerkat_meta::AccessControlConfig {
        enabled: true,
        admins: vec!["someone-else@example.com".to_string()],
        ..Default::default()
    })
    .expect("access controller");
    for method in [
        "meta/console/voice/readiness",
        "meta/console/voice/replacement",
        "meta/console/voice/activity",
        "meta/console/voice/captions",
        "meta/live/playback_owner/register",
        "live/webrtc/answer",
        "meta/console/voice/answer_received",
    ] {
        let (_, response) = voice_rpc_with_params(
            state.clone(),
            method,
            true,
            json!({"identity":"agent-a", "request_id":"request-a", "channel_id":"channel-a"}),
            Some(access.clone()),
        )
        .await;
        assert_eq!(
            response["error"]["data"]["kind"], "access_denied",
            "{method}: {response}"
        );
    }
    let (_, closed) = voice_rpc_with_params(
        state,
        "meta/console/voice/close",
        true,
        json!({"identity":"agent-a", "request_id":"request-a"}),
        Some(access),
    )
    .await;
    assert_eq!(closed["result"]["phase"], "closed");
}

#[tokio::test]
async fn console_voice_http_never_promotes_anonymous_console_access_to_live_authority() {
    for method in [
        "meta/console/voice/open",
        "meta/console/voice/readiness",
        "meta/console/voice/close",
    ] {
        let (status, response) = voice_rpc(decision_state(), method, false).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(response["error"]["data"]["kind"], "access_denied");
    }
}

#[tokio::test]
async fn console_voice_does_not_expose_measured_output_transport() {
    let mut state = decision_state();
    state.console.require_app_auth = true;
    for method in [
        "meta/console/voice/outputs/poll",
        "meta/console/voice/outputs/ack",
    ] {
        let (_, response) = voice_rpc(state.clone(), method, true).await;
        assert_eq!(response["error"]["code"], -32601);
    }
}

#[tokio::test]
async fn console_voice_http_requires_real_console_auth_and_a_composed_voice_host() {
    let mut state = decision_state();
    state.console.require_app_auth = true;
    let (status, _) = voice_rpc(state.clone(), "meta/console/voice/open", false).await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
    let (status, response) = voice_rpc(state, "meta/console/voice/open", true).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(response["error"]["data"]["kind"], "voice_unavailable");
}

#[tokio::test]
async fn console_voice_http_read_only_blocks_open_but_not_owned_cancel_fencing() {
    let mut state = decision_state();
    state.console.require_app_auth = true;
    state.console.read_only = true;
    let (_, open) = voice_rpc(state.clone(), "meta/console/voice/open", true).await;
    assert_eq!(open["error"]["data"]["kind"], "read_only");
    // Owned caption reads mutate nothing, like context status: a read-only
    // console reaches the request scope check instead of a read-only refusal.
    let (_, captions) = voice_rpc_with_params(
        state.clone(),
        "meta/console/voice/captions",
        true,
        json!({"identity":"agent-a", "request_id":"request-a", "channel_id":"channel-a"}),
        None,
    )
    .await;
    assert_eq!(
        captions["error"]["data"]["kind"], "voice_request_conflict",
        "{captions}"
    );
    let (_, close) = voice_rpc(state, "meta/console/voice/close", true).await;
    assert_eq!(close["result"], json!({"phase":"closed"}));
}

/// The ids a console send returns (`interaction_id`, `input_frame_id`) are
/// correlation handles, not capabilities: no console route or RPC method in
/// the contract accepts one as a request field, so a principal holding
/// another's ids can neither read nor act through them. A method that ever
/// resolves one must check that the caller owns it, and update this pin.
#[test]
fn console_contract_requests_take_no_interaction_or_frame_ids() {
    let artifact: Value = serde_json::from_str(include_str!(
        "../../../docs/rct/console-rest-sse-contract-v0.5.0.json"
    ))
    .expect("contract artifact json should parse");
    const REQUEST_FIELD_LISTS: &[&str] = &[
        "required_fields",
        "optional_fields",
        "query_optional_fields",
        "required_path_params",
    ];
    const ID_FIELDS: &[&str] = &["interaction_id", "input_frame_id", "frame_id"];
    fn walk(value: &Value, path: &mut Vec<String>, offenders: &mut Vec<String>, lists: &mut usize) {
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    if REQUEST_FIELD_LISTS.contains(&key.as_str())
                        && let Some(fields) = child.as_array()
                    {
                        *lists += 1;
                        for field in fields.iter().filter_map(Value::as_str) {
                            if ID_FIELDS.contains(&field) {
                                offenders.push(format!("{}/{key}: {field}", path.join("/")));
                            }
                        }
                    }
                    path.push(key.clone());
                    walk(child, path, offenders, lists);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, path, offenders, lists);
                }
            }
            _ => {}
        }
    }
    let mut offenders = Vec::new();
    let mut lists = 0;
    walk(
        &artifact["surfaces"],
        &mut Vec::new(),
        &mut offenders,
        &mut lists,
    );
    assert!(
        lists > 20,
        "the walk must see the contract's request lists ({lists})"
    );
    assert!(
        offenders.is_empty(),
        "console requests that take an interaction or frame id: {offenders:?}"
    );
}

#[test]
fn phase0_contract_004_console_rest_sse_contract_version_is_pinned_and_enforced() {
    let artifact: Value = serde_json::from_str(include_str!(
        "../../../docs/rct/console-rest-sse-contract-v0.5.0.json"
    ))
    .expect("contract artifact json should parse");

    assert_eq!(artifact["contract_version"], json!("0.5.0"));
    assert_eq!(artifact["version_pin"], json!("v0.5.0"));

    let state = decision_state();

    let experience_method = artifact["surfaces"]["rest"]["experience"]["method"]
        .as_str()
        .expect("experience method must be present");
    let experience_path = artifact["surfaces"]["rest"]["experience"]["path"]
        .as_str()
        .expect("experience path must be present");
    let experience_response = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: experience_method.to_string(),
            path: experience_path.to_string(),
            auth: None,
        },
    );

    assert_eq!(experience_response.status, 200);
    assert_eq!(
        experience_response.body["contract_version"],
        artifact["contract_version"]
    );
    assert_eq!(
        experience_response.body["base_panel"]["route"],
        json!(experience_path)
    );

    for field in artifact["surfaces"]["rest"]["experience"]["response"]["required_top_level_fields"]
        .as_array()
        .expect("experience required field list must be present")
    {
        let field_name = field
            .as_str()
            .expect("experience required field must be string");
        assert!(
            experience_response.body.get(field_name).is_some(),
            "experience response missing required field: {field_name}"
        );
    }

    let modules_method = artifact["surfaces"]["rest"]["modules"]["method"]
        .as_str()
        .expect("modules method must be present");
    let modules_path = artifact["surfaces"]["rest"]["modules"]["path"]
        .as_str()
        .expect("modules path must be present");
    let modules_response = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: modules_method.to_string(),
            path: modules_path.to_string(),
            auth: None,
        },
    );

    assert_eq!(modules_response.status, 200);
    for field in artifact["surfaces"]["rest"]["modules"]["response"]["required_top_level_fields"]
        .as_array()
        .expect("modules required field list must be present")
    {
        let field_name = field
            .as_str()
            .expect("modules required field must be string");
        assert!(
            modules_response.body.get(field_name).is_some(),
            "modules response missing required field: {field_name}"
        );
    }
    assert!(modules_response.body["modules"].is_array());

    let send_error_codes = artifact["surfaces"]["rpc"]["methods"]["meta/console/send"]["errors"][0]
        ["codes"]
        .as_array()
        .expect("send error codes must be present");
    assert!(
        !send_error_codes.contains(&json!(
            meerkat_meta::CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE
        )),
        "send must not advertise timeline replay errors"
    );
    let query_timeline_error_codes =
        artifact["surfaces"]["rpc"]["methods"]["meta/console/query_timeline"]["errors"][0]["codes"]
            .as_array()
            .expect("query_timeline error codes must be present");
    assert!(
        query_timeline_error_codes.contains(&json!(
            meerkat_meta::CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE
        )),
        "query_timeline must advertise timeline replay errors"
    );

    let timeline_path = artifact["surfaces"]["sse"]["timeline"]["path"]
        .as_str()
        .expect("timeline path must be present");
    let keep_alive_event = artifact["surfaces"]["sse"]["activity_feed_keep_alive_event"]
        .as_str()
        .expect("keep-alive event must be present");

    assert_eq!(
        experience_response.body["chat_inspector"]["observe_route"],
        json!(timeline_path)
    );
    assert_eq!(
        experience_response.body["activity_feed"]["source_route"],
        json!(timeline_path)
    );
    assert_eq!(
        experience_response.body["activity_feed"]["keep_alive"]["event"],
        json!(keep_alive_event)
    );
}

/// A console send refused before authentication (e.g. an off-network browser
/// with no console token) answers 401 with a typed body on both send doors.
/// The console app renders `data.kind == "unauthenticated"` as a definite
/// rejection ("not authorized from this network"), never as a pending
/// acceptance: the request never reached dispatch, so nothing was reserved.
#[tokio::test]
async fn console_send_without_auth_returns_typed_unauthenticated_body_on_both_doors() {
    use tower::ServiceExt;
    let mut state = decision_state();
    state.console.require_app_auth = true;
    let app = meerkat_meta::console_json_router_with_aggregator_and_access(
        state,
        meerkat_meta::MetaConsoleAggregator::new(std::sync::Arc::new(
            meerkat_meta::InMemoryConsoleLogStore::default(),
        )),
        None,
    );
    let send_params = json!({
        "identity": "domain:calendar",
        "content": "queued from off the home network",
        "origin": "console:panel-1",
        "origin_kind": "operator",
        "idempotency_key": "idem-unauthenticated-send",
        "handling_mode": "queue",
    });

    let json_request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            json!({
                "jsonrpc": "2.0", "id": "send-1", "method": "meta/console/send",
                "params": send_params.clone(),
            })
            .to_string(),
        ))
        .expect("json request");

    let boundary = "meta-unauthenticated-boundary";
    let payload = json!({
        "jsonrpc": "2.0", "id": "send-2", "method": "meta/console/send",
        "params": send_params,
    })
    .to_string();
    let multipart_body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"payload\"\r\n\r\n{payload}\r\n--{boundary}--\r\n"
    );
    let multipart_request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc/multipart")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(axum::body::Body::from(multipart_body))
        .expect("multipart request");

    for (door, request) in [("json", json_request), ("multipart", multipart_request)] {
        let response = app.clone().oneshot(request).await.expect("response");
        assert_eq!(
            response.status(),
            axum::http::StatusCode::UNAUTHORIZED,
            "{door}: an unauthenticated send is refused before dispatch"
        );
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        let body: Value = serde_json::from_slice(&body).expect("typed json body");
        assert_eq!(body["jsonrpc"], "2.0", "{door}: {body}");
        assert_eq!(body["error"]["code"], -32600, "{door}: {body}");
        assert_eq!(
            body["error"]["data"]["kind"], "unauthenticated",
            "{door}: the refusal names its typed reason: {body}"
        );
        assert_eq!(body["error"]["data"]["http_status"], 401, "{door}: {body}");
        assert!(
            body.get("result").is_none(),
            "{door}: no acceptance receipt: {body}"
        );
    }
}
