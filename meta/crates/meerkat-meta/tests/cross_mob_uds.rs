//! Cross-mob UDS transport plumbing — Phase 1 surface tests.
//!
//! Mirrors `cross_mob_tcp.rs` but for Unix domain sockets. See that file
//! for the contract narrative.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::sync::Arc;

use meerkat_client::TestClient;
use meerkat_mob::MobDefinition;

use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::contact_directory::{ContactDirectory, MobTransport};
use meerkat_meta::unified_runtime::cross_mob::{CrossMobError, build_uds_peer_spec};

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

#[tokio::test]
async fn uds_contact_directory_round_trips_through_toml() {
    let dir = ContactDirectory::from_toml(
        r#"
        [mobs]
        mob-b = "uds:///var/run/meerkat/mob-b.sock"
        "#,
    )
    .expect("parse uds contact directory");

    let entry = dir.get("mob-b").expect("mob-b entry");
    assert_eq!(
        entry.transport,
        MobTransport::Uds("/var/run/meerkat/mob-b.sock".to_string())
    );
}

#[tokio::test]
async fn uds_peer_spec_helper_emits_triple_slash_address() {
    // `uds:///path` — the comms-layer parser splits on `://` and treats
    // the remainder as an absolute filesystem path. The helper accepts
    // either `/path` or `path` and normalizes.
    let spec = build_uds_peer_spec(
        "mob-a/worker/alice",
        "00000000-0000-4000-8000-000000000001",
        "/var/run/meerkat/mob-a.sock",
    )
    .expect("uds spec");
    assert_eq!(spec.address.endpoint(), "/var/run/meerkat/mob-a.sock");
}

#[tokio::test]
async fn unified_runtime_with_uds_contact_reports_remote_contacts() {
    let tmp_a = tempfile::tempdir().expect("temp dir a");
    let tmp_b = tempfile::tempdir().expect("temp dir b");
    let path_a = tmp_a.path().join("mob-a.sock");
    let path_b = tmp_b.path().join("mob-b.sock");

    let dir_a = ContactDirectory::from_toml(&format!(
        r#"
        [mobs]
        mob-b = "uds://{}"
        "#,
        path_b.display(),
    ))
    .expect("dir for mob-a");
    let dir_b = ContactDirectory::from_toml(&format!(
        r#"
        [mobs]
        mob-a = "uds://{}"
        "#,
        path_a.display(),
    ))
    .expect("dir for mob-b");

    let rt_a = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition_a())
            .default_llm_client(Arc::new(TestClient::default()))
            .contact_directory(dir_a)
            .build(),
    )
    .await
    .expect("build mob-a");
    let rt_b = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition_b())
            .default_llm_client(Arc::new(TestClient::default()))
            .contact_directory(dir_b)
            .build(),
    )
    .await
    .expect("build mob-b");

    assert!(rt_a.has_contact_directory());
    assert!(rt_a.has_remote_contacts());
    assert!(!rt_a.has_inproc_contacts());
    assert!(rt_b.has_remote_contacts());

    drop(rt_a);
    drop(rt_b);
}

#[tokio::test]
async fn wire_cross_mob_over_uds_surfaces_remote_seam() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let sock = tmp.path().join("mob-b.sock");

    let dir = ContactDirectory::from_toml(&format!(
        r#"
        [mobs]
        mob-b = "uds://{}"
        "#,
        sock.display(),
    ))
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

    // See cross_mob_tcp.rs for the equivalent rationale: wire_cross_mob
    // resolves the local member's roster info before reaching the
    // cross-process control channel, so an empty mob fails with
    // MemberNotFound first.
    let err = Box::pin(rt.wire_cross_mob("alice", "bob", "mob-b"))
        .await
        .expect_err("empty mob has no 'alice' member");
    assert!(
        matches!(
            err,
            CrossMobError::MemberNotFound { ref member_id, .. } if member_id == "alice"
        ),
        "got {err:?}",
    );

    drop(rt);
}
