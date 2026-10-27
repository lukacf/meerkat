//! Regression: declared definition wiring (`auto_wire_orchestrator`,
//! `role_wiring`) must converge regardless of bring-up order (a downstream app,
//! 2026-07-09). Upstream applies the rules only at spawn time and only from
//! the non-orchestrator side, so a lead ensured AFTER its workers ended with
//! `wired_to: []` and multi-member crews were dead on arrival. The
//! definition-derived default edge policy plus the reconcile-after-ensure
//! heal make the declaration a reconcilable desired state.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_meta::mob_composition_manifest::CompositionAuthority;
use meerkat_meta::{
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, UnifiedRuntime,
    handle_unified_rpc_json,
};
use meerkat_mob::{MobDefinition, MobStorage};
use serde_json::{Value, json};

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Per-call mob id: 0.8.23's fail-closed in-proc registration means
/// concurrently running tests must not share a supervisor route.
fn crew_mob_toml() -> String {
    format!(
        r#"
[mob]
id = "edge-wiring-crew-{}"
orchestrator = "lead"

[wiring]
auto_wire_orchestrator = true

[[wiring.role_wiring]]
a = "builder"
b = "reviewer"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true

[profiles.builder]
model = "gpt-5.5"
external_addressable = true

[profiles.builder.tools]
comms = true

[profiles.reviewer]
model = "gpt-5.5"
external_addressable = true

[profiles.reviewer.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Gateway-path runtime (MobBootstrapSpec + `UnifiedRuntime::bootstrap`) —
/// the construction both gateways use, where no embedder edge policy is ever
/// supplied and the definition-derived default must be installed.
async fn build_gateway_path_runtime() -> (tempfile::TempDir, UnifiedRuntime) {
    build_gateway_path_runtime_as(CompositionAuthority::Authoritative).await
}

/// [`build_gateway_path_runtime`], launched with `authority`.
async fn build_gateway_path_runtime_as(
    authority: CompositionAuthority,
) -> (tempfile::TempDir, UnifiedRuntime) {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let factory = AgentFactory::new(temp_dir.path()).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 8));
    let definition = MobDefinition::from_toml(&crew_mob_toml()).expect("parse crew definition");
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        })
        .with_composition_authority(authority);
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "edge-wiring-reconcile".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap crew runtime");
    (temp_dir, runtime)
}

async fn rpc(runtime: &UnifiedRuntime, method: &str, params: Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": "edge-wiring",
        "method": method,
        "params": params,
    })
    .to_string();
    let response =
        handle_unified_rpc_json(runtime, &request, Duration::from_secs(5), None, None).await;
    serde_json::from_str(&response).expect("json-rpc response")
}

async fn ensure(runtime: &UnifiedRuntime, role: &str, identity: &str) {
    let response = rpc(
        runtime,
        "meta/ensure_member",
        json!({ "role": role, "agent_identity": identity }),
    )
    .await;
    assert!(
        response["error"].is_null(),
        "ensure_member {identity} failed: {response:#?}"
    );
}

async fn wired_to(runtime: &UnifiedRuntime, identity: &str) -> BTreeSet<String> {
    let response = rpc(runtime, "meta/list_members", json!({})).await;
    let members = response["result"].as_array().expect("members array");
    members
        .iter()
        .find(|m| m["member_id"] == identity || m["agent_identity"] == identity)
        .unwrap_or_else(|| panic!("member {identity} not found: {members:#?}"))["wired_to"]
        .as_array()
        .map(|peers| {
            peers
                .iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The exact field failure: workers ensured FIRST, the lead LAST — upstream's
/// spawn-side wiring skips the orchestrator's own spawn, so pre-fix the lead
/// ended with `wired_to: []`.
#[tokio::test(flavor = "multi_thread")]
async fn lead_ensured_last_still_wires_to_declared_crew() {
    let (_dir, runtime) = build_gateway_path_runtime().await;

    ensure(&runtime, "builder", "builder-1").await;
    ensure(&runtime, "reviewer", "reviewer-1").await;
    ensure(&runtime, "lead", "lead-1").await;

    let lead = wired_to(&runtime, "lead-1").await;
    assert!(
        lead.contains("builder-1") && lead.contains("reviewer-1"),
        "auto_wire_orchestrator must converge regardless of bring-up order; lead wired_to: {lead:?}"
    );
    let builder = wired_to(&runtime, "builder-1").await;
    assert!(
        builder.contains("reviewer-1"),
        "role_wiring builder<->reviewer must hold: {builder:?}"
    );

    let shutdown = runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}

/// Seat `identity` on `role` through the mob handle, as a restore does: no
/// edge reconcile runs after it.
async fn seat(runtime: &UnifiedRuntime, role: &str, identity: &str) {
    if let Err(error) = runtime
        .mob_handle()
        .ensure_member(meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from(role),
            meerkat_mob::AgentIdentity::from(identity),
        ))
        .await
    {
        panic!("seat {identity}: {error}");
    }
}

/// A candidate launch's `meta/reconcile_edges` is a dry run. The lead seated
/// after its builder is unwired (spawn-time wiring skips the orchestrator's
/// own spawn); the candidate answers the edge the reconcile would wire,
/// flagged `"applied": false`, and wires nothing. The authoritative control
/// is `lead_ensured_last_still_wires_to_declared_crew`.
#[tokio::test(flavor = "multi_thread")]
async fn a_candidate_launch_answers_reconcile_edges_with_a_dry_run() {
    let (_dir, runtime) =
        build_gateway_path_runtime_as(CompositionAuthority::NonAuthoritative).await;
    seat(&runtime, "builder", "builder-1").await;
    seat(&runtime, "lead", "lead-1").await;
    assert!(
        wired_to(&runtime, "lead-1").await.is_empty(),
        "the spawn order leaves the lead unwired"
    );

    let response = rpc(&runtime, "meta/reconcile_edges", json!({})).await;
    assert!(
        response["error"].is_null(),
        "reconcile_edges failed: {response:#?}"
    );
    let result = &response["result"];
    assert_eq!(result["applied"], json!(false), "{result:#?}");
    assert_eq!(
        result["wired_edges"].as_array().map(Vec::len),
        Some(1),
        "the dry run names the lead and builder edge it would wire: {result:#?}"
    );
    assert!(
        wired_to(&runtime, "lead-1").await.is_empty(),
        "a candidate's reconcile wires nothing"
    );

    let shutdown = runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}

/// `meta/reconcile_edges` heals a manually unwired crew back to the
/// declaration (restart/out-of-band recovery affordance).
#[tokio::test(flavor = "multi_thread")]
async fn reconcile_edges_rewires_declared_crew() {
    let (_dir, runtime) = build_gateway_path_runtime().await;
    ensure(&runtime, "lead", "lead-1").await;
    ensure(&runtime, "builder", "builder-1").await;

    let response = rpc(&runtime, "meta/reconcile_edges", json!({})).await;
    assert!(
        response["error"].is_null(),
        "reconcile_edges failed: {response:#?}"
    );

    let lead = wired_to(&runtime, "lead-1").await;
    assert!(
        lead.contains("builder-1"),
        "reconcile_edges must wire the declared crew: {lead:?}"
    );

    let shutdown = runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}
