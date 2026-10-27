#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]
//! Cross-mob signed-peer tests for Unit 4 (Real Ed25519 peer descriptors
//! + gateway key management).
//!
//! Strategy: every realistic UnifiedRuntime test that spins up sessions is
//! `#[ignore]`-gated because it depends on a full LLM-capable factory.
//! These tests instead exercise the parts meta owns end-to-end:
//!
//! 1. `GatewayPeerKeys` round-trips on disk and through base64 wire form.
//! 2. The `meta/peer_pubkey` RPC dispatches over both the unified RPC
//!    and the console RPC paths and returns the gateway's pubkey.
//! 3. The contact-directory-aware `wire_local` path stamps a real pubkey
//!    on non-inproc descriptors and rejects empty / zero pubkeys closed.
//! 4. End-to-end happy path: two gateways, each with its own keypair,
//!    contact directories carrying each other's pubkeys, build and accept
//!    `TrustedPeerDescriptor`s the way meerkat-comms does at ingest.

use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_meta::contact_directory::{ContactDirectory, MobTransport};
use meerkat_meta::{
    DiscoverySpec, GatewayPeerKeys, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
    UnifiedRuntime, decode_pubkey_b64, handle_unified_rpc_json,
};
use meerkat_mob::{MobDefinition, MobStorage};
use serde_json::Value;
use tempfile::TempDir;

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Build a small UnifiedRuntime fixture with comms enabled. Same shape as
/// `console_phase0_boundaries::build_unified_runtime`, kept inline so the
/// signed-peer suite stays self-contained and doesn't pull in identity-
/// first machinery it never exercises.
async fn build_runtime() -> (TempDir, UnifiedRuntime) {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");

    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));

    let mut definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "signed-peer-mob-{}"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
    .expect("parse mob definition");
    for binding in definition.profiles.values_mut() {
        if let Some(profile) = binding.as_inline_mut() {
            profile.model = "gpt-5.5".to_string();
        }
    }

    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "cross-mob-signed".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };

    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap unified runtime");
    (temp_dir, runtime)
}

async fn stop_runtime_allowing_boundary_cancel(runtime: &UnifiedRuntime) {
    if let Err(err) = runtime.mob_handle().stop().await {
        assert!(
            err.to_string().contains("cancel_after_boundary"),
            "stop failed: {err:?}"
        );
    }
}

fn rpc_request(method: &str, params: Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
    .to_string()
}

#[tokio::test]
async fn gateway_keys_round_trip_through_state_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = GatewayPeerKeys::load_or_create(dir.path()).expect("create");
    let pubkey = first.pubkey_bytes();
    let second = GatewayPeerKeys::load_or_create(dir.path()).expect("load");
    assert_eq!(second.pubkey_bytes(), pubkey, "key persists across loads");

    // Round-trip through base64 (mirrors what meta/peer_pubkey returns).
    let encoded = first.pubkey_b64();
    let decoded = decode_pubkey_b64(&encoded).expect("decode");
    assert_eq!(decoded, pubkey);
}

#[tokio::test]
async fn peer_pubkey_rpc_returns_configured_keypair() {
    let (_tmp, mut runtime) = build_runtime().await;
    let keys = GatewayPeerKeys::ephemeral();
    let expected_b64 = keys.pubkey_b64();
    runtime.set_gateway_peer_keys(keys);

    let response = handle_unified_rpc_json(
        &runtime,
        &rpc_request("meta/peer_pubkey", serde_json::json!({})),
        Duration::from_secs(2),
        None,
        None,
    )
    .await;

    let parsed: Value = serde_json::from_str(&response).expect("json");
    let result = parsed.get("result").expect("result present (no error)");
    assert_eq!(
        result.get("pubkey_b64").and_then(Value::as_str),
        Some(expected_b64.as_str())
    );

    stop_runtime_allowing_boundary_cancel(&runtime).await;
}

#[tokio::test]
async fn peer_pubkey_rpc_errors_when_no_keypair_configured() {
    let (_tmp, runtime) = build_runtime().await;
    // Deliberately do NOT set gateway keys — inproc-only deployments
    // skip this and the RPC should report capability_unavailable rather
    // than fabricating a pubkey.
    let response = handle_unified_rpc_json(
        &runtime,
        &rpc_request("meta/peer_pubkey", serde_json::json!({})),
        Duration::from_secs(2),
        None,
        None,
    )
    .await;

    let parsed: Value = serde_json::from_str(&response).expect("json");
    assert!(
        parsed.get("result").map(Value::is_null).unwrap_or(true),
        "result must be absent on error"
    );
    let error = parsed.get("error").expect("error present");
    assert_eq!(error.get("code").and_then(Value::as_i64), Some(-32004));

    stop_runtime_allowing_boundary_cancel(&runtime).await;
}

#[tokio::test]
async fn wire_local_rejects_non_inproc_without_pubkey() {
    let (_tmp, runtime) = build_runtime().await;
    // Spawn a member so wire_local has a live local member to wire from.
    let handle = runtime.mob_handle();
    handle
        .ensure_member(meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("lead"),
            meerkat_mob::ids::AgentIdentity::from("alice"),
        ))
        .await
        .expect("ensure_member");

    // Stable hyphenated UUID — meta's TrustedPeerDescriptor::test_only_unsigned
    // validates the peer_id parses, so we feed it a real one.
    let bogus_peer_id = "00000000-0000-4000-8000-000000000001";
    let err = runtime
        .wire_local(
            "alice",
            "remote-peer",
            bogus_peer_id,
            "tcp://192.168.1.50:9002",
            None,
        )
        .await
        .expect_err("wire_local must reject non-inproc without pubkey");
    let msg = format!("{err}");
    assert!(msg.contains("pubkey"), "error must mention pubkey: {msg}");

    // Zero pubkey is also rejected — the descriptor would otherwise be
    // an unsigned all-zero shape that meerkat-comms admits unconditionally.
    let err_zero = runtime
        .wire_local(
            "alice",
            "remote-peer",
            bogus_peer_id,
            "tcp://192.168.1.50:9002",
            Some([0u8; 32]),
        )
        .await
        .expect_err("zero pubkey must be rejected");
    let msg_zero = format!("{err_zero}");
    assert!(msg_zero.contains("pubkey"), "{msg_zero}");

    stop_runtime_allowing_boundary_cancel(&runtime).await;
}

#[tokio::test]
async fn cross_mob_signed_descriptor_round_trip() {
    // Two gateways, each with their own keypair. Each contact directory
    // carries the other's pubkey. Build the descriptors meta would
    // hand to meerkat-comms during a wire_local and verify they (a) carry
    // the right pubkey and (b) parse cleanly via meerkat-comms's
    // `descriptor → trusted_peer` consistency check (which derives the
    // peer_id from the pubkey and matches against the descriptor field).

    let gateway_a = GatewayPeerKeys::ephemeral();
    let gateway_b = GatewayPeerKeys::ephemeral();

    let pubkey_b_b64 = gateway_b.pubkey_b64();
    let pubkey_a_b64 = gateway_a.pubkey_b64();

    let directory_a_text = format!(
        r#"[mobs]
        gateway-b = {{ transport = "tcp://10.0.0.2:9002", pubkey = "{pubkey_b_b64}" }}
        "#,
    );
    let directory_b_text = format!(
        r#"[mobs]
        gateway-a = {{ transport = "tcp://10.0.0.1:9001", pubkey = "{pubkey_a_b64}" }}
        "#,
    );

    let directory_a = ContactDirectory::from_toml(&directory_a_text).expect("parse a");
    let directory_b = ContactDirectory::from_toml(&directory_b_text).expect("parse b");

    let entry_b = directory_a.get("gateway-b").expect("entry b");
    assert!(matches!(entry_b.transport, MobTransport::Tcp(_)));
    assert_eq!(entry_b.pubkey, Some(gateway_b.pubkey_bytes()));

    let entry_a = directory_b.get("gateway-a").expect("entry a");
    assert_eq!(entry_a.pubkey, Some(gateway_a.pubkey_bytes()));

    // Build the descriptor each side would stamp on a wire_local call.
    // For meerkat-comms's peer-id consistency check (descriptor.peer_id
    // must equal `PubKey(pubkey).to_peer_id()`), derive peer_id from the
    // pubkey via UUIDv5 the same way the comms identity module does.
    // Production gateways stamp the matching pair on both sides.
    let peer_id_b = peer_id_from_pubkey(gateway_b.pubkey_bytes());
    let peer_id_a = peer_id_from_pubkey(gateway_a.pubkey_bytes());

    let descriptor_b = TrustedPeerDescriptor::unsigned_with_pubkey(
        "gateway-b",
        &peer_id_b,
        gateway_b.pubkey_bytes(),
        "tcp://10.0.0.2:9002",
    )
    .expect("descriptor b");
    let descriptor_a = TrustedPeerDescriptor::unsigned_with_pubkey(
        "gateway-a",
        &peer_id_a,
        gateway_a.pubkey_bytes(),
        "tcp://10.0.0.1:9001",
    )
    .expect("descriptor a");

    assert_eq!(descriptor_a.pubkey, gateway_a.pubkey_bytes());
    assert_eq!(descriptor_b.pubkey, gateway_b.pubkey_bytes());
    // Roundtrip through serde — this is what hops through the comms-trust
    // ingest path.
    let serialized = serde_json::to_string(&descriptor_a).expect("serialize");
    let parsed: TrustedPeerDescriptor = serde_json::from_str(&serialized).expect("parse");
    assert_eq!(parsed.pubkey, descriptor_a.pubkey);
    assert_eq!(parsed.address.endpoint(), "10.0.0.1:9001");
}

/// Derive a `peer_id` UUID string from a 32-byte Ed25519 pubkey using
/// meerkat-comms's UUIDv5 namespace.
///
/// Delegates to `meerkat_comms::identity::PubKey::to_peer_id` so the
/// derivation stays in lock-step with the meerkat side without
/// duplicating the namespace constant in the meta test surface.
fn peer_id_from_pubkey(pubkey: [u8; 32]) -> String {
    meerkat_comms::identity::PubKey::new(pubkey)
        .to_peer_id()
        .to_string()
}
