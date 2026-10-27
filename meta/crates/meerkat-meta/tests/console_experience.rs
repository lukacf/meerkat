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
use std::collections::BTreeMap;

use meerkat_meta::{
    AuthPolicy, AuthProvider, BigQueryNaming, ConsoleAccessRequest, ConsoleLiveSnapshot,
    ConsolePolicy, ConsoleRestJsonRequest, RuntimeDecisionInputs, RuntimeOpsPolicy,
    TrustedOidcRuntimeConfig, build_runtime_decision_state, handle_console_rest_json_route,
    handle_console_rest_json_route_with_snapshot,
};
use serde_json::json;

fn release_json() -> String {
    include_str!("../assets/release-targets.json").to_string()
}

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
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

fn decision_state(require_app_auth: bool) -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "phase8_dataset".to_string(),
            table: "phase8_table".to_string(),
        },
        trusted_meta_toml: trusted_toml(),
        auth: AuthPolicy {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: vec![
                "alice@example.com".to_string(),
                "svc:deploy-bot".to_string(),
            ],
        },
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: release_json(),
    })
    .expect("decision state builds")
}

fn decision_state_with_console_policy(
    console: ConsolePolicy,
) -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "phase8_dataset".to_string(),
            table: "phase8_table".to_string(),
        },
        trusted_meta_toml: trusted_toml(),
        auth: AuthPolicy {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: vec!["alice@example.com".to_string()],
        },
        trusted_oidc: trusted_oidc(),
        console,
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: release_json(),
    })
    .expect("decision state builds")
}

#[tokio::test]
async fn shared_console_exposes_registered_native_panels_and_removes_stale_scopes()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::{body::Body, http::Request};
    use meerkat_meta::console_aggregator::{
        AllowAllConsoleVisibilityPolicy, ConsoleRuntimeRegistration, MetaConsoleAggregator,
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn experience(app: axum::Router) -> serde_json::Value {
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/console/experience")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice(&body).expect("JSON")
    }

    let aggregator = MetaConsoleAggregator::in_memory();
    let app = meerkat_meta::http_console::console_json_router_with_aggregator(
        decision_state(false),
        aggregator.clone(),
    );
    let empty = experience(app.clone()).await;
    assert_eq!(empty["memory"]["available"], false);
    assert_eq!(empty["workgraph"]["available"], false);
    assert_eq!(empty["panel_runtimes"], json!([]));

    let memory_dir = tempfile::tempdir()?;
    let runtime = Arc::new(
        meerkat_meta::UnifiedRuntime::builder()
            .definition(meerkat_mob::MobDefinition::from_toml(
                "[mob]\nid = 'repository-team'\n[profiles.worker]\nmodel = 'gpt-6-astra'\n",
            )?)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build()
            .await?,
    );
    runtime.set_memory_panel_store(Arc::new(
        meerkat_meta::memory::SqliteAgentMemoryStore::open(memory_dir.path())?,
    ));
    aggregator.register_runtime(ConsoleRuntimeRegistration {
        runtime_key: "repository".into(),
        identity_namespace: "repository".into(),
        runtime: runtime.clone(),
        visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
    });
    let registered = experience(app.clone()).await;
    assert_eq!(registered["memory"]["available"], true);
    assert_eq!(registered["memory"]["can_read"], true);
    assert_eq!(registered["memory"]["can_review_quarantine"], true);
    assert_eq!(registered["workgraph"]["available"], true);
    assert_eq!(registered["workgraph"]["can_view"], true);
    assert_eq!(registered["workgraph"]["can_manage"], false);
    assert_eq!(
        registered["panel_runtimes"],
        json!([{
            "key":"repository", "label":"repository-team", "namespace":"repository",
            "memory":true, "workgraph":true,
        }])
    );

    let protected = meerkat_meta::http_console::console_json_router_with_aggregator(
        decision_state(true),
        aggregator.clone(),
    );
    let denied = protected
        .oneshot(
            Request::builder()
                .uri("/console/experience")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(denied.status(), 401);

    aggregator.unregister_runtime("repository");
    let removed = experience(app).await;
    assert_eq!(removed["memory"]["available"], false);
    assert_eq!(removed["workgraph"]["available"], false);
    assert_eq!(removed["panel_runtimes"], json!([]));
    runtime.mob_handle().stop().await?;
    Ok(())
}

#[test]
fn console_voice_is_unavailable_without_an_authenticated_voice_host() {
    for require_app_auth in [false, true] {
        let state = decision_state(require_app_auth);
        let response = handle_console_rest_json_route_with_snapshot(
            &state,
            &ConsoleRestJsonRequest {
                method: "GET".to_string(),
                path: "/console/experience".to_string(),
                auth: require_app_auth.then(|| ConsoleAccessRequest {
                    provider: AuthProvider::GoogleOAuth,
                    email: "alice@example.com".to_string(),
                }),
            },
            Some(&ConsoleLiveSnapshot::new(
                Some("voice-test".to_string()),
                Some(true),
                vec![],
                vec![],
                Vec::new(),
                true,
            )),
        );
        assert_eq!(response.status, 200);
        assert_eq!(
            response.body["runtime_capabilities"]["can_send_messages"],
            json!(true)
        );
        assert_eq!(response.body["voice"], json!({ "available": false }));
    }
}

#[test]
fn phase8_console_001_capability_driven_rendering_contract() {
    let state = decision_state(true);
    let authorized_auth = ConsoleAccessRequest {
        provider: AuthProvider::GoogleOAuth,
        email: "alice@example.com".to_string(),
    };
    let allowed = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: Some(authorized_auth.clone()),
        },
    );
    let modules_response = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/modules".to_string(),
            auth: Some(authorized_auth),
        },
    );

    assert_eq!(allowed.status, 200);
    assert_eq!(modules_response.status, 200);
    assert_eq!(allowed.body["contract_version"], json!("0.5.0"));
    assert_eq!(
        allowed.body["base_panel"]["panel_id"],
        json!("console.home")
    );
    assert_eq!(
        allowed.body["base_panel"]["route"],
        json!("/console/experience")
    );
    assert_eq!(
        allowed.body["module_panels"],
        json!([
            {
                "panel_id":"module.router",
                "module_id":"router",
                "title":"router module",
                "route":"/console/modules/router",
                "capabilities":{"can_render":true,"can_subscribe_activity":true}
            },
            {
                "panel_id":"module.delivery",
                "module_id":"delivery",
                "title":"delivery module",
                "route":"/console/modules/delivery",
                "capabilities":{"can_render":true,"can_subscribe_activity":true}
            }
        ])
    );
    assert_eq!(
        allowed.body["agent_sidebar"]["panel_id"],
        json!("console.agent_sidebar")
    );
    assert_eq!(allowed.body["agent_sidebar"]["schema_version"], json!("1"));
    assert_eq!(
        allowed.body["agent_sidebar"]["refresh"],
        json!({
            "mode": "poll",
            "interval_ms": 5000
        })
    );
    assert_eq!(
        allowed.body["agent_sidebar"]["source_method"],
        json!("meta/status")
    );
    assert_eq!(
        allowed.body["agent_sidebar"]["selection_contract"]["supported_scopes"],
        json!(["mob", "agent"])
    );
    assert_eq!(
        allowed.body["agent_sidebar"]["selection_contract"]["selected_member_id_field"],
        json!("member_id")
    );
    assert_eq!(
        allowed.body["agent_sidebar"]["list_item_contract"]["member_id_field"],
        json!("member_id")
    );
    assert_eq!(
        allowed.body["agent_sidebar"]["live_snapshot"]["agents"],
        json!([
            {
                "agent_id":"router",
                "member_id":"router",
                "label":"router",
                "kind":"module_agent",
            },
            {
                "agent_id":"delivery",
                "member_id":"delivery",
                "label":"delivery",
                "kind":"module_agent",
            }
        ])
    );
    assert_eq!(
        allowed.body["identity_status"]["schema_version"],
        json!("1")
    );
    assert_eq!(
        allowed.body["identity_status"]["refresh"],
        json!({
            "mode": "poll",
            "interval_ms": 5000
        })
    );
    assert_eq!(
        allowed.body["identity_status"]["rows"],
        json!([
            {
                "identity":"router",
                "state":"unknown",
                "addressability":"addressable",
                "labels":{},
            },
            {
                "identity":"delivery",
                "state":"unknown",
                "addressability":"addressable",
                "labels":{},
            }
        ])
    );
    assert_eq!(
        allowed.body["chat_inspector"]["panel_id"],
        json!("console.chat_inspector")
    );
    assert_eq!(allowed.body["chat_inspector"]["schema_version"], json!("1"));
    assert_eq!(
        allowed.body["chat_inspector"]["send_method"],
        json!("meta/console/send")
    );
    assert_eq!(
        allowed.body["chat_inspector"]["observe_route"],
        json!("/console/timeline/stream")
    );
    assert_eq!(
        allowed.body["topology"]["panel_id"],
        json!("console.topology")
    );
    assert_eq!(allowed.body["topology"]["schema_version"], json!("1"));
    assert_eq!(
        allowed.body["topology"]["source_method"],
        json!("meta/status")
    );
    assert!(allowed.body["topology"]["live_snapshot"].is_object());
    assert_eq!(
        allowed.body["topology"]["live_snapshot"]["nodes"],
        modules_response.body["modules"]
    );
    assert_eq!(
        allowed.body["topology"]["live_snapshot"]["node_count"],
        json!(
            modules_response.body["modules"]
                .as_array()
                .expect("modules array")
                .len()
        )
    );
    assert_eq!(
        allowed.body["health_overview"]["panel_id"],
        json!("console.health_overview")
    );
    assert_eq!(
        allowed.body["health_overview"]["schema_version"],
        json!("1")
    );
    assert_eq!(
        allowed.body["health_overview"]["source_method"],
        json!("meta/status")
    );
    assert!(allowed.body["health_overview"]["live_snapshot"].is_object());
    assert_eq!(
        allowed.body["health_overview"]["live_snapshot"]["loaded_modules"],
        modules_response.body["modules"]
    );
    assert_eq!(
        allowed.body["health_overview"]["live_snapshot"]["loaded_module_count"],
        json!(
            modules_response.body["modules"]
                .as_array()
                .expect("modules array")
                .len()
        )
    );
}

#[test]
fn console_experience_projects_fetch_timeout_policy() {
    let state = decision_state_with_console_policy(ConsolePolicy {
        require_app_auth: true,
        fetch_timeout_ms: Some(120_000),
        ..ConsolePolicy::default()
    });
    let response = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: Some(ConsoleAccessRequest {
                provider: AuthProvider::GoogleOAuth,
                email: "alice@example.com".to_string(),
            }),
        },
    );

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body["console_policy"]["fetch_timeout_ms"],
        json!(120_000)
    );
}

#[test]
fn console_experience_projects_read_only_policy_and_capabilities() {
    let state = decision_state_with_console_policy(ConsolePolicy {
        require_app_auth: true,
        read_only: true,
        ..ConsolePolicy::default()
    });
    let response = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: Some(ConsoleAccessRequest {
                provider: AuthProvider::GoogleOAuth,
                email: "alice@example.com".to_string(),
            }),
        },
    );

    assert_eq!(response.status, 200);
    assert_eq!(response.body["console_policy"]["read_only"], json!(true));
    assert_eq!(
        response.body["runtime_capabilities"]["can_send_messages"],
        json!(false)
    );
}

#[test]
fn console_experience_projects_durable_identity_for_runtime_member_rows() {
    let state = decision_state(false);
    let runtime_snapshot = ConsoleLiveSnapshot::new(
        Some("ops".to_string()),
        Some(true),
        vec!["rt:review:singleton:0".to_string()],
        vec![],
        vec![meerkat_meta::runtime::ConsoleMember {
            agent_identity: "rt:review:singleton:0".to_string(),
            role: "review-worker".to_string(),
            state: "active".to_string(),
            model_capabilities: meerkat_meta::ConsoleModelCapabilities { image_input: false },
            runtime_mode: Some("identity_first".to_string()),
            session_id: Some("session-review".to_string()),
            wired_to: Vec::new(),
            labels: BTreeMap::from([
                ("agent_identity".to_string(), "review:singleton".to_string()),
                ("display_name".to_string(), "Review Worker".to_string()),
            ]),
            progress: Some(json!({
                "run_state": "run_open",
                "in_flight_work": 2,
                "last_progress_at_ms": 1_752_300_000_000_u64,
                "last_progress_event": "execution_advanced",
                "health": "wedged",
            })),
        }],
        true,
    );

    let response = handle_console_rest_json_route_with_snapshot(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
        Some(&runtime_snapshot),
    );

    assert_eq!(response.status, 200);
    let agent = &response.body["agent_sidebar"]["live_snapshot"]["agents"][0];
    assert_eq!(agent["identity"], json!("review:singleton"));
    assert_eq!(agent["member_id"], json!("rt:review:singleton:0"));
    assert_eq!(agent["agent_id"], json!("rt:review:singleton:0"));
    assert_eq!(
        response.body["identity_status"]["rows"][0]["identity"],
        json!("review:singleton")
    );
    // Machine-owned liveness (meerkat 0.7.29+, ask 14) rides both the sidebar
    // agent row and the identity_status row.
    assert_eq!(agent["progress"]["health"], json!("wedged"));
    assert_eq!(agent["progress"]["run_state"], json!("run_open"));
    assert_eq!(
        response.body["identity_status"]["rows"][0]["progress"]["health"],
        json!("wedged")
    );
    assert_eq!(
        response.body["identity_status"]["rows"][0]["progress"]["in_flight_work"],
        json!(2)
    );
}

#[test]
fn phase8_console_live_snapshot_prefers_runtime_state_over_config_modules() {
    let state = decision_state(false);
    let runtime_snapshot = ConsoleLiveSnapshot::new(
        None,
        Some(false),
        vec!["router".to_string()],
        vec![],
        Vec::new(),
        false,
    );
    let response = handle_console_rest_json_route_with_snapshot(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
        Some(&runtime_snapshot),
    );

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body["module_panels"]
            .as_array()
            .expect("module_panels array")
            .len(),
        2
    );
    assert_eq!(
        response.body["topology"]["live_snapshot"]["nodes"],
        json!(["router"])
    );
    assert_eq!(
        response.body["health_overview"]["live_snapshot"]["loaded_modules"],
        json!(["router"])
    );
    assert_eq!(
        response.body["health_overview"]["live_snapshot"]["loaded_module_count"],
        json!(1)
    );
    assert_eq!(
        response.body["health_overview"]["live_snapshot"]["running"],
        json!(false)
    );
}

/// A console with no live runtime has no answer for whether a mob runs: the
/// experience reports `running` as null and names it unobserved. The configured
/// modules say what could run, not that anything does.
#[test]
fn a_console_without_a_live_runtime_reports_running_unobserved() {
    let response = handle_console_rest_json_route(
        &decision_state(false),
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
    );
    assert_eq!(response.status, 200, "{}", response.body);
    let live = &response.body["health_overview"]["live_snapshot"];
    assert_eq!(
        live["loaded_modules"],
        json!(["router", "delivery"]),
        "{live}"
    );
    assert_eq!(live["running"], serde_json::Value::Null, "{live}");
    assert_eq!(
        live["unobserved"],
        json!({"reason": "not_observed", "fields": ["running"]}),
        "{live}"
    );
}

#[test]
fn phase0_contract_010_console_experience_preserves_watch_and_degraded_fields() {
    let state = decision_state(false);
    let runtime_snapshot = ConsoleLiveSnapshot::new(
        Some("runtime-1".to_string()),
        Some(true),
        vec!["router".to_string()],
        vec![meerkat_meta::ConsoleAgentLiveSnapshot {
            agent_id: "identity:luka".to_string(),
            member_id: "member-luka".to_string(),
            label: "Luka".to_string(),
            kind: "identity".to_string(),
            identity: Some("identity:luka".to_string()),
            role: Some("lead".to_string()),
            state: Some("running".to_string()),
            session_id: Some("sess-1".to_string()),
            response_phase: Some(Some("waiting".to_string())),
            model_capabilities: meerkat_meta::ConsoleModelCapabilities { image_input: false },
            watched: Some(true),
            alert_level: Some("critical".to_string()),
            degraded: Some(true),
            degraded_reason: Some("lease_expired".to_string()),
        }],
        Vec::new(),
        false,
    );

    let response = handle_console_rest_json_route_with_snapshot(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
        Some(&runtime_snapshot),
    );

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body["agent_sidebar"]["live_snapshot"]["agents"],
        json!([
            {
                "agent_id":"identity:luka",
                "member_id":"member-luka",
                "label":"Luka",
                "kind":"identity",
                "identity":"identity:luka",
                "role":"lead",
                "state":"running",
                "session_id":"sess-1",
                "response_phase":"waiting",
                "model_capabilities": {
                    "image_input": false
                },
                "watched": true,
                "alertLevel":"critical",
                "degraded": true,
                "degradedReason":"lease_expired",
            }
        ])
    );
}

#[test]
fn console_experience_projects_configurable_sidebar_and_agent_grouping() {
    let mut state = decision_state(false);
    state.console.ui = meerkat_meta::load_console_ui_config_from_toml(
        r#"
title = "Ops"

[brand]
label = "Ops Console"
logo_url = "/assets/ops.svg"
logo_alt = "Ops"

[appearance]
default_theme = "dark"
default_variant = "graphite"

[environment]
label = "prod"

[layout]
initial_preset = "two_columns"
initial_control = "roster"
sidebar_collapsed = true

[rail]
visible = true
collapsed = false
active_preset_id = "critical"
empty_text = "No signals."

[[rail.filter_presets]]
id = "critical"
label = "Critical"
alert_levels = ["critical"]

[sidebar]
visible_controls = ["topology", "roster", "logs"]

[[sidebar.buttons]]
id = "ops-board"
label = "Ops Board"
href = "https://example.test/ops"
target = "_blank"

[agent_list]
group_by = ["labels.console_group", "labels.group", "role"]
subgroup_by = ["labels.org"]
section_order = ["Personal", "Initiatives", "Internal"]
default_pinned_agent_ids = ["review:singleton"]

[[agent_list.badges]]
id = "org"
label = "Org"
field = "labels.org"

[[agent_list.sections]]
name = "Initiatives"
empty_title = "No initiatives"
empty_text = "Create one in Linear."

[actions]
inspect_label = "Profile"
send_label = "Send to agent"
show_reset = false
"#,
    )
    .expect("console config parses");

    let response = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
    );

    assert_eq!(response.status, 200);
    assert_eq!(response.body["base_panel"]["title"], json!("Ops"));
    assert_eq!(
        response.body["console_config"]["brand"],
        json!({
            "label": "Ops Console",
            "logo_url": "/assets/ops.svg",
            "logo_alt": "Ops",
        })
    );
    assert_eq!(
        response.body["console_config"]["layout"]["initial_preset"],
        json!("two_columns")
    );
    assert_eq!(
        response.body["console_config"]["rail"]["filter_presets"][0]["alertLevels"],
        json!(["critical"])
    );
    assert_eq!(
        response.body["console_config"]["sidebar"]["visible_controls"],
        json!(["topology", "roster", "logs"])
    );
    assert_eq!(
        response.body["console_config"]["sidebar"]["buttons"][0],
        json!({
            "id": "ops-board",
            "label": "Ops Board",
            "href": "https://example.test/ops",
            "target": "_blank",
        })
    );
    assert_eq!(
        response.body["console_config"]["agent_list"]["subgroup_by"],
        json!(["labels.org"])
    );
    assert_eq!(
        response.body["console_config"]["agent_list"]["default_pinned_agent_ids"],
        json!(["review:singleton"])
    );
    assert_eq!(
        response.body["console_config"]["agent_list"]["badges"][0]["field"],
        json!("labels.org")
    );
    assert_eq!(
        response.body["console_config"]["actions"]["inspect_label"],
        json!("Profile")
    );
}

#[test]
fn phase8_req_003_choke_104_unified_activity_feed_contract_over_events() {
    let state = decision_state(false);
    let experience = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
    );

    assert_eq!(
        experience.body["activity_feed"]["source_route"],
        json!("/console/timeline/stream")
    );
    assert_eq!(
        experience.body["activity_feed"]["request_contract"]["last_event_id_header"],
        json!("optional Last-Event-ID checkpoint from prior event_id")
    );
    assert_eq!(
        experience.body["activity_feed"]["panel_id"],
        json!("console.activity_feed")
    );
    assert_eq!(
        experience.body["activity_feed"]["event_contract"]["envelope_fields"],
        json!([
            "event_id",
            "interaction_id",
            "identity",
            "event_type",
            "timestamp_ms",
            "data"
        ])
    );
    assert_eq!(
        experience.body["activity_feed"]["event_contract"]["event_type_path"],
        json!("event_type")
    );
}

#[test]
fn phase8_console_002_auth_protected_access_remains_enforced() {
    let state = decision_state(true);

    let missing = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
    );
    let denied_provider = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: Some(ConsoleAccessRequest {
                provider: AuthProvider::GitHubOAuth,
                email: "alice@example.com".to_string(),
            }),
        },
    );
    let allowed = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: Some(ConsoleAccessRequest {
                provider: AuthProvider::ServiceIdentity,
                email: "svc:deploy-bot".to_string(),
            }),
        },
    );

    assert_eq!(missing.status, 401);
    assert_eq!(
        missing.body,
        json!({"error":"unauthorized","reason":"missing_credentials"})
    );
    assert_eq!(denied_provider.status, 401);
    assert_eq!(
        denied_provider.body,
        json!({"error":"unauthorized","reason":"provider_mismatch"})
    );
    assert_eq!(allowed.status, 200);
}

#[test]
fn console_experience_preserves_known_quiet_and_unknown_response_phases() {
    let state = decision_state(false);
    for roster in [false, true] {
        for (phase, expected) in [
            (
                Some(Some("generating".to_string())),
                Some(json!("generating")),
            ),
            (Some(None), Some(serde_json::Value::Null)),
            (None, None),
        ] {
            let agent = meerkat_meta::ConsoleAgentLiveSnapshot {
                agent_id: "worker-runtime".into(),
                member_id: "worker-runtime".into(),
                label: "Worker".into(),
                kind: "meerkat".into(),
                identity: Some("worker".into()),
                role: Some("lead".into()),
                state: Some("active".into()),
                session_id: Some("session-worker".into()),
                model_capabilities: Default::default(),
                response_phase: phase,
                watched: None,
                alert_level: None,
                degraded: None,
                degraded_reason: None,
            };
            let encoded = serde_json::to_value(&agent).unwrap();
            assert_eq!(encoded.get("response_phase"), expected.as_ref());
            let decoded: meerkat_meta::ConsoleAgentLiveSnapshot =
                serde_json::from_value(encoded).unwrap();
            assert_eq!(decoded.response_phase, agent.response_phase);
            let members = if roster {
                vec![meerkat_meta::runtime::ConsoleMember {
                    agent_identity: "worker-runtime".into(),
                    role: "lead".into(),
                    state: "active".into(),
                    model_capabilities: Default::default(),
                    runtime_mode: None,
                    session_id: Some("session-worker".into()),
                    wired_to: vec![],
                    labels: BTreeMap::from([("agent_identity".into(), "worker".into())]),
                    progress: None,
                }]
            } else {
                vec![]
            };
            let snapshot = ConsoleLiveSnapshot::new(
                Some("runtime".into()),
                Some(true),
                vec![],
                vec![agent],
                members,
                roster,
            );
            let response = handle_console_rest_json_route_with_snapshot(
                &state,
                &ConsoleRestJsonRequest {
                    method: "GET".into(),
                    path: "/console/experience".into(),
                    auth: None,
                },
                Some(&snapshot),
            );
            assert_eq!(response.status, 200);
            let sidebar = &response.body["agent_sidebar"]["live_snapshot"]["agents"][0];
            let identity = &response.body["identity_status"]["rows"][0];
            assert_eq!(
                sidebar.get("response_phase"),
                expected.as_ref(),
                "roster={roster}"
            );
            assert_eq!(
                identity.get("response_phase"),
                expected.as_ref(),
                "roster={roster}"
            );
        }
    }
}
