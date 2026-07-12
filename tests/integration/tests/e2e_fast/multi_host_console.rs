//! Multi-host mobs phase 7 — deterministic e2e-fast console row (T-A8,
//! §17.11): the new `mob/*` console verbs served end-to-end through the
//! REAL RPC handler → `MobMcpState` wrapper → `MobHandle` path against a
//! two-hosts-in-one-process fixture. Walk: bind an EPHEMERAL
//! (`durable_sessions=false`) member-build host → spawn B2 with placement →
//! `mob/hosts` labels the bound host honestly (durable_sessions=false,
//! eager verified reachability, member count 1) → `mob/member_history` serves
//! the remote page with typed placement + `host_claimed` provenance →
//! `mob/route_installs` reports the drained (empty ⇒ complete) obligation
//! set → `mob/member_live_status(channel_id: None)` — the reply-loss
//! discovery read — degrades TYPED against the live-incapable host (DL5:
//! absence of the advertised endpoint IS the incapability; no bridge
//! traffic, never a hang or a silent success).
//!
//! Deterministic: TestClient members, loopback port-0 listeners, no live
//! providers.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::support;

use meerkat_mob_mcp::MobMcpState;
use meerkat_rpc::handlers::mob as mob_handlers;
use meerkat_rpc::protocol::RpcId;
use support::{HostFixtureOptions, REAL_COMMS_TEST_LOCK, spawn_host_daemon_fixture};

fn raw_params(value: serde_json::Value) -> Box<serde_json::value::RawValue> {
    serde_json::value::RawValue::from_string(value.to_string()).expect("serialize rpc params")
}

fn success_json(response: meerkat_rpc::protocol::RpcResponse, context: &str) -> serde_json::Value {
    assert!(
        response.error.is_none(),
        "{context}: expected success, got {:?}",
        response.error
    );
    serde_json::from_str(
        response
            .result
            .unwrap_or_else(|| panic!("{context}: missing result"))
            .get(),
    )
    .unwrap_or_else(|error| panic!("{context}: result decodes: {error}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn console_rpc_verbs_serve_multi_host_facts() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;

    // Ephemeral member substrate ⇒ the host declares
    // `durable_sessions=false` at bind — the §17.11 labelling case consoles
    // read to render cursor gaps honestly. No live endpoint is advertised
    // (DL5: absence is the fact).
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("e2e-console-host").ephemeral())
            .await
            .expect("spawn ephemeral member-build host fixture");
    let controlling = support::create_controlling_mob("e2e-console").await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("B2 materializes on the console host");

    // The console seam: the RPC handlers drive the SAME MobMcpState
    // wrappers every surface consumes (ADJ-P7-3), over the controlling
    // handle (owner console, A16).
    let state = MobMcpState::new_in_memory();
    state
        .mob_insert_handle(controlling.mob_id.clone(), controlling.handle.clone())
        .await;
    let mob_id = controlling.mob_id.to_string();

    // ── mob/hosts: bound row with honest capability + reachability facts ─
    let response = mob_handlers::handle_hosts(
        Some(RpcId::Num(1)),
        Some(&raw_params(serde_json::json!({ "mob_id": mob_id }))),
        &state,
    )
    .await;
    let hosts = success_json(response, "mob/hosts");
    let rows = hosts["hosts"].as_array().expect("hosts rows");
    assert_eq!(rows.len(), 1, "exactly the bound fixture host");
    let row = &rows[0];
    assert_eq!(row["host_id"], serde_json::json!(report.host_id));
    assert_eq!(row["bind_phase"], serde_json::json!("bound"));
    assert_eq!(row["authority_epoch"], serde_json::json!(report.epoch));
    assert_eq!(
        row["capabilities"]["durable_sessions"],
        serde_json::json!(false),
        "the ephemeral host is labelled durable_sessions=false"
    );
    assert!(
        row.get("capabilities")
            .and_then(|caps| caps.get("live_endpoint"))
            .is_none(),
        "no advertised live endpoint ⇒ the fact is ABSENT (DL5)"
    );
    assert_eq!(
        row["materialized_member_count"],
        serde_json::json!(1),
        "B2's placement counts against the host"
    );
    // Bind eagerly performs a verified HostStatus observation, so the
    // observer-local reachability projection is already authoritative here.
    assert_eq!(row["control_reachability"], serde_json::json!("reachable"));
    assert!(
        row["last_seen_ms"].as_u64().is_some(),
        "verified host observation must expose a numeric observer-local age"
    );
    assert_eq!(
        row["freshness_reason"],
        serde_json::json!("host_status_ack")
    );

    // ── mob/member_history: remote page, typed placement + provenance ──
    let response = mob_handlers::handle_member_history(
        Some(RpcId::Num(2)),
        Some(&raw_params(serde_json::json!({
            "mob_id": mob_id,
            "agent_identity": "b2",
        }))),
        &state,
    )
    .await;
    let history = success_json(response, "mob/member_history");
    assert_eq!(
        history["placement"],
        serde_json::json!(report.host_id),
        "the machine's member_placement fact rides the envelope"
    );
    assert_eq!(
        history["provenance"],
        serde_json::json!("host_claimed"),
        "a bridge-served page is host-attested"
    );
    let page = &history["page"];
    assert!(
        page["message_count"].is_u64(),
        "the shared wire page body carries message_count: {page}"
    );
    assert!(page["complete"].is_boolean());
    assert!(history["generation"].is_u64());

    // ── mob/route_installs: no cross-host wiring ⇒ drained/complete ────
    let response = mob_handlers::handle_route_installs(
        Some(RpcId::Num(3)),
        Some(&raw_params(serde_json::json!({ "mob_id": mob_id }))),
        &state,
    )
    .await;
    let installs = success_json(response, "mob/route_installs");
    assert_eq!(installs["outstanding"], serde_json::json!([]));
    assert_eq!(installs["complete"], serde_json::json!(true));

    // ── mob/member_live_status(channel_id: None): the discovery read
    // degrades TYPED against a live-incapable host — synchronous reject on
    // the recorded bind fact, no bridge traffic, never quiet. The cause
    // carries no multi-host wire code, so the rendering is the ordinary
    // invalid-params fallback (byte-identical posture).
    let response = mob_handlers::handle_member_live_status(
        Some(RpcId::Num(4)),
        Some(&raw_params(serde_json::json!({
            "mob_id": mob_id,
            "agent_identity": "b2",
        }))),
        &state,
    )
    .await;
    assert!(response.result.is_none());
    let error = response
        .error
        .expect("live status against a live-incapable host is a typed error");
    assert!(
        error.message.contains("live"),
        "the degradation names the live incapability: {}",
        error.message
    );

    fixture.shutdown().await;
}
