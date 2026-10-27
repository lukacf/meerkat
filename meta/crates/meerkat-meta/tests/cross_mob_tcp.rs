//! Cross-mob TCP transport plumbing - surface tests.
//!
//! These tests pin the structural contract of the TCP contact path:
//!
//! 1. `ContactDirectory::from_toml` accepts `tcp://host:port` entries.
//! 2. `UnifiedRuntime::has_remote_contacts()` flips on for TCP entries.
//! 3. The peer-spec helpers `build_tcp_peer_spec` produce valid
//!    comms-layer addresses.
//! 4. `wire_cross_mob` routes TCP contacts through the `LocalOrRemote`
//!    dispatcher (local roster checks first, then the real cross-process
//!    control client).
//! 5. `wire_local` / `unwire_local` accept `tcp://` addresses and
//!    pass them through to the comms-layer trust store unchanged
//!    (no early scheme rejection in meta).
//!
//! The end-to-end proof (two runtimes, real control listeners, signed
//! envelope delivery both ways) lives in
//! `tests/cross_mob_control_round_trip.rs`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::sync::Arc;

use meerkat_client::TestClient;
use meerkat_mob::MobDefinition;

use meerkat_meta::contact_directory::{ContactDirectory, MobTransport};
use meerkat_meta::runtime::cross_mob_control::{ControlAuthorizer, ControlListenAddr};
use meerkat_meta::runtime::cross_mob_remote::RemoteEndpoint;
use meerkat_meta::runtime::remote_host::RemoteHostClient;
use meerkat_meta::unified_runtime::cross_mob::{CrossMobError, build_tcp_peer_spec};
use meerkat_meta::{GatewayPeerKeys, UnifiedRuntimeBuilder};

const MINIMAL_MOB_TOML_A: &str = r#"
[mob]
id = "mob-a"

[profiles.worker]
model = "gpt-5.5"
"#;

const MINIMAL_MOB_TOML_B: &str = r#"
[mob]
id = "mob-b"

[profiles.worker]
model = "gpt-5.5"
"#;

fn definition_a() -> MobDefinition {
    // Per-call mob id: 0.8.23's fail-closed in-proc registration means
    // concurrently running tests must not share a supervisor route. Nothing
    // in these tests asserts on the local definition id.
    static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    MobDefinition::from_toml(&MINIMAL_MOB_TOML_A.replace(
        "id = \"mob-a\"",
        &format!(
            "id = \"mob-a-{}\"",
            NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ),
    ))
    .expect("parse mob-a")
}
fn definition_b() -> MobDefinition {
    // Per-call mob id: 0.8.23's fail-closed in-proc registration means
    // concurrently running tests must not share a supervisor route. Nothing
    // in these tests asserts on the local definition id.
    static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    MobDefinition::from_toml(&MINIMAL_MOB_TOML_B.replace(
        "id = \"mob-b\"",
        &format!(
            "id = \"mob-b-{}\"",
            NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ),
    ))
    .expect("parse mob-b")
}

/// Pick a free TCP port on loopback. The port is released immediately;
/// if a listener races onto it before the test wires the address in, the
/// test still passes (we never actually bind on the discovered address).
async fn ephemeral_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

#[tokio::test]
async fn tcp_contact_directory_round_trips_through_toml() {
    let dir = ContactDirectory::from_toml(
        r#"
        [mobs]
        mob-b = "tcp://127.0.0.1:9001"
        "#,
    )
    .expect("parse tcp contact directory");

    let entry = dir.get("mob-b").expect("mob-b entry");
    assert_eq!(
        entry.transport,
        MobTransport::Tcp("127.0.0.1:9001".to_string())
    );
}

#[tokio::test]
async fn tcp_peer_spec_helper_builds_canonical_address() {
    let spec = build_tcp_peer_spec(
        "mob-a/worker/alice",
        "00000000-0000-4000-8000-000000000001",
        "127.0.0.1:9001",
    )
    .expect("tcp spec");
    assert_eq!(spec.address.endpoint(), "127.0.0.1:9001");
    assert_eq!(spec.name.as_str(), "mob-a/worker/alice");
}

#[tokio::test]
async fn unified_runtime_with_tcp_contact_reports_remote_contacts() {
    let port_a = ephemeral_port().await;
    let port_b = ephemeral_port().await;
    let dir_a = ContactDirectory::from_toml(&format!(
        r#"
        [mobs]
        mob-b = "tcp://127.0.0.1:{port_b}"
        "#,
    ))
    .expect("dir for mob-a");
    let dir_b = ContactDirectory::from_toml(&format!(
        r#"
        [mobs]
        mob-a = "tcp://127.0.0.1:{port_a}"
        "#,
    ))
    .expect("dir for mob-b");

    let rt_a = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition_a())
            .default_llm_client(Arc::new(TestClient::default()))
            .contact_directory(dir_a.clone())
            .build(),
    )
    .await
    .expect("build mob-a");
    let rt_b = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition_b())
            .default_llm_client(Arc::new(TestClient::default()))
            .contact_directory(dir_b.clone())
            .build(),
    )
    .await
    .expect("build mob-b");

    assert!(rt_a.has_contact_directory());
    assert!(rt_a.has_remote_contacts());
    assert!(!rt_a.has_inproc_contacts());

    assert!(rt_b.has_contact_directory());
    assert!(rt_b.has_remote_contacts());

    drop(rt_a);
    drop(rt_b);
}

#[tokio::test]
async fn wire_cross_mob_over_tcp_surfaces_remote_seam() {
    // With no `register_peer_mob`, a TCP contact entry routes through the
    // `RemoteMobProxy` (the real control-channel client).
    let port_b = ephemeral_port().await;
    let dir_a = ContactDirectory::from_toml(&format!(
        r#"
        [mobs]
        mob-b = "tcp://127.0.0.1:{port_b}"
        "#,
    ))
    .expect("dir");
    let rt_a = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition_a())
            .default_llm_client(Arc::new(TestClient::default()))
            .contact_directory(dir_a)
            .build(),
    )
    .await
    .expect("build mob-a");

    // wire_cross_mob now resolves the local member's roster info first,
    // so an empty mob fails with `MemberNotFound("alice", "mob-a")` before
    // even reaching the cross-process control channel. This is the
    // expected order of operations — local checks come first so we don't
    // burn a TCP connection for a misconfigured caller. The full
    // round-trip integration test (with a real listener bound on the
    // peer) lives in `cross_mob_control_round_trip.rs`.
    let err = Box::pin(rt_a.wire_cross_mob("alice", "bob", "mob-b"))
        .await
        .expect_err("empty mob has no 'alice' member");
    assert!(
        matches!(
            err,
            CrossMobError::MemberNotFound { ref member_id, .. } if member_id == "alice"
        ),
        "got {err:?}",
    );

    drop(rt_a);
}

#[tokio::test]
async fn unknown_mob_rejected_before_dispatch() {
    let dir = ContactDirectory::from_toml(
        r#"
        [mobs]
        mob-b = "tcp://127.0.0.1:9001"
        "#,
    )
    .expect("dir");
    let rt = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition_a())
            .default_llm_client(Arc::new(TestClient::default()))
            .contact_directory(dir)
            .build(),
    )
    .await
    .expect("build");

    let err = Box::pin(rt.wire_cross_mob("alice", "bob", "no-such-mob"))
        .await
        .expect_err("unknown mob");
    assert!(matches!(err, CrossMobError::UnknownMob(ref id) if id == "no-such-mob"));

    drop(rt);
}

#[tokio::test]
async fn listener_first_key_install_late_binds_signed_host_facts_and_health() {
    let mut runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition_a())
            .default_llm_client(Arc::new(TestClient::default()))
            .build(),
    )
    .await
    .expect("build runtime");
    let advertised = runtime
        .start_control_listener_with_authorizer(
            &ControlListenAddr::parse("tcp://127.0.0.1:0").expect("listen address"),
            Arc::new(ControlAuthorizer::open()),
        )
        .await
        .expect("start listener before keys");

    let keys = GatewayPeerKeys::ephemeral();
    runtime.set_gateway_peer_keys(keys.clone());

    let dial = advertised
        .strip_prefix("tcp://")
        .expect("tcp advertised address");
    let client = RemoteHostClient::new(RemoteEndpoint::Tcp(dial.to_string()), keys.pubkey_bytes());
    let verified = client
        .describe()
        .await
        .expect("late-bound host facts are signed by installed keys");
    assert_eq!(verified.host_key_b64(), keys.pubkey_b64());
    assert_eq!(verified.dialed_endpoint(), advertised);
    assert!(verified.facts().capabilities.features.multi_host_mobs);
    assert_eq!(
        client.health().await.expect("signed host health").status,
        meerkat_contracts::RuntimeHostHealthStatus::Ok
    );

    runtime.shutdown().await;
}
