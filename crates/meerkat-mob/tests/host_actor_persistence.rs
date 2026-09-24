//! Multi-host mobs phase 2 — R8 host-side persistence + FLAG-3 controlling-
//! side durability (design §W4.1 "phase-2-specific rows": persist/recover,
//! corrupted-row startup abort, persist-failure fail-closed; plus the §9
//! "controlling host restart" row via `MobHostAuthorityRecord` +
//! `RecoverHostBinding`).
//!
//! TDD-first: written against the Lane W0/W1/W2 shapes; compiles once those
//! lanes land.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::runtime::bridge_protocol::{BridgeRejectionCause, BridgeReply};
use meerkat_mob::store::{MobHostBindPhaseRecord, MobRuntimeMetadataStore as _};
use support::{
    ControllingMobPaths, HostFixtureOptions, REAL_COMMS_TEST_LOCK, descriptor_to_bind_request,
    persistent_service, raw_bind_host_command, spawn_host_daemon_fixture,
    spawn_peer_comms_endpoint,
};

const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

fn sqlite_runtime_store(dir: &std::path::Path) -> Arc<dyn meerkat_runtime::RuntimeStore> {
    Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(dir.join("runtime.sqlite3"))
            .expect("open sqlite runtime store"),
    )
}

// ===========================================================================
// R8: bind → durable CAS row → daemon restart → recover_from_state
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn bind_persists_record_and_daemon_restart_recovers_binding() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let state_dir = tempfile::tempdir().expect("host state dir");
    let store = sqlite_runtime_store(state_dir.path());
    let identity_secret = meerkat_comms::Keypair::generate().secret_bytes();

    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        runtime_store: Some(Arc::clone(&store)),
        identity_secret: Some(identity_secret),
        ..HostFixtureOptions::named("persist-host")
    })
    .await
    .expect("spawn host daemon fixture");

    let supervisor = spawn_peer_comms_endpoint("persist-supervisor", true, None).await;
    supervisor.trust(fixture.host_peer_descriptor()).await;
    let mob_id = "mob-persist";
    let d0 = fixture.current_descriptor();
    let reply = supervisor
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_bind_host_command(&supervisor, mob_id, &d0, 7),
            REPLY_TIMEOUT,
        )
        .await
        .expect("bind reply");
    assert!(matches!(reply, BridgeReply::BindHost(_)), "got {reply:?}");

    // Durable row exists BEFORE any restart: keyed by mob id, typed record
    // decodes, token never persisted (D3).
    let rows = store
        .list_mob_host_bindings()
        .await
        .expect("list binding rows");
    assert_eq!(rows.len(), 1);
    let (row_mob_id, row_bytes) = &rows[0];
    assert_eq!(row_mob_id, mob_id);
    let record: meerkat_mob::runtime::host_actor::MobHostBindingRecord =
        serde_json::from_slice(row_bytes).expect("typed MobHostBindingRecord decodes");
    assert_eq!(record.epoch, 7);
    assert_eq!(
        record.supervisor_peer_id,
        supervisor.runtime.public_key().to_peer_id().to_string()
    );
    assert!(
        !String::from_utf8_lossy(row_bytes).contains(d0.bootstrap_token.as_str()),
        "the bootstrap token must never appear in the durable record"
    );

    // Daemon restart: same store, same identity. Recovery folds the rows into
    // MobHostBindingAuthorityState and enters through the generated
    // recover_from_state seam (DEC-P2-6).
    fixture.shutdown().await;
    let restarted = spawn_host_daemon_fixture(HostFixtureOptions {
        runtime_store: Some(Arc::clone(&store)),
        identity_secret: Some(identity_secret),
        ..HostFixtureOptions::named("persist-host")
    })
    .await
    .expect("respawn host daemon fixture from durable rows");

    // The binding survived: a DIFFERENT supervisor is still typed-rejected.
    let intruder = spawn_peer_comms_endpoint("persist-intruder", true, None).await;
    intruder.trust(restarted.host_peer_descriptor()).await;
    let d1 = restarted.current_descriptor();
    let takeover = intruder
        .send_bridge_command_raw(
            &restarted.host_peer_descriptor(),
            &raw_bind_host_command(&intruder, mob_id, &d1, 9),
            REPLY_TIMEOUT,
        )
        .await
        .expect("takeover reply");
    assert!(
        matches!(
            takeover,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::AlreadyBound,
                ..
            }
        ),
        "the recovered binding must reject a second supervisor, got {takeover:?}"
    );

    // The recorded supervisor re-engages with the restarted daemon's FRESH
    // descriptor (restart re-mints the token — DEC-P2-4): the same
    // (mob, supervisor, epoch) tuple converges through the bind-replay arm.
    supervisor.trust(restarted.host_peer_descriptor()).await;
    let replay = supervisor
        .send_bridge_command_raw(
            &restarted.host_peer_descriptor(),
            &raw_bind_host_command(&supervisor, mob_id, &d1, 7),
            REPLY_TIMEOUT,
        )
        .await
        .expect("replay reply");
    assert!(
        matches!(replay, BridgeReply::BindHost(_)),
        "the recorded supervisor must converge after daemon restart, got {replay:?}"
    );

    restarted.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn corrupted_record_aborts_daemon_startup_typed() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let state_dir = tempfile::tempdir().expect("host state dir");
    let store = sqlite_runtime_store(state_dir.path());
    assert!(
        store
            .put_mob_host_binding_if_absent("mob-corrupt", b"{not-a-record")
            .await
            .expect("seed corrupted row"),
        "corrupted seed row must land"
    );

    // Fail closed: no half-recovered daemon. The typed startup abort is the
    // RecoveredStateInvariantRejected posture (design W2.2 recover-or-create).
    let startup = spawn_host_daemon_fixture(HostFixtureOptions {
        runtime_store: Some(store),
        ..HostFixtureOptions::named("corrupt-host")
    })
    .await;
    let error = match startup {
        Err(error) => error,
        Ok(_) => panic!("a corrupted R8 row must abort daemon startup"),
    };
    assert!(
        !format!("{error:?}").is_empty(),
        "startup abort must be a typed error"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn persist_failure_never_advances_in_memory_state_or_consumes_the_token() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    // Always-failing typed R8 persistence (the W2.4 seam is the injection
    // point — the actor's `persistence` field, design W2.2).
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        failing_persistence: true,
        ..HostFixtureOptions::named("persist-fail-host")
    })
    .await
    .expect("a daemon over an empty store starts fresh");

    let supervisor = spawn_peer_comms_endpoint("persist-fail-supervisor", true, None).await;
    supervisor.trust(fixture.host_peer_descriptor()).await;
    let d0 = fixture.current_descriptor();
    let bind = raw_bind_host_command(&supervisor, "mob-persist-fail", &d0, 1);

    // Persist failure ⇒ prepared authority dropped + typed failure reply —
    // in-memory state never advances past durable truth (W2.3 step (b)).
    let first = supervisor
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &bind, REPLY_TIMEOUT)
        .await
        .expect("first reply");
    assert!(
        matches!(
            first,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::Internal,
                ..
            }
        ),
        "persist failure must surface as a typed Internal rejection, got {first:?}"
    );

    // The IDENTICAL retry reproduces the identical outcome: were the binding
    // committed in memory the retry would be AlreadyBound; were the token
    // consumed it would be InvalidBootstrapToken. Neither may happen.
    let second = supervisor
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &bind, REPLY_TIMEOUT)
        .await
        .expect("second reply");
    assert!(
        matches!(
            second,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::Internal,
                ..
            }
        ),
        "neither binding state nor the token may advance past durable truth, got {second:?}"
    );
    assert_eq!(
        fixture.current_descriptor().bootstrap_token,
        d0.bootstrap_token,
        "a failed persist must not consume/re-mint the one-time token"
    );
    assert!(
        fixture
            .store
            .list_mob_host_bindings()
            .await
            .expect("list binding rows")
            .is_empty(),
        "no durable row may land when the typed persist fails"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// FLAG-3: controlling restart recovers host facts (plan §9 restart row)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn controlling_restart_recovers_host_facts_from_durable_records() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        live_endpoint: Some("wss://restart-host.test.invalid/live".to_string()),
        ..HostFixtureOptions::named("restart-recovery-host")
    })
    .await
    .expect("spawn host daemon fixture");

    let temp = tempfile::tempdir().expect("controlling temp dir");
    let paths = ControllingMobPaths::new(temp.path());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());

    // ---------------- Lifetime 1: create + bind ----------------
    let mob_id = meerkat_mob::MobId::from(format!(
        "restart-recovery-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let (host_id_string, bound_epoch) = {
        let service = persistent_service(&paths, Arc::clone(&runtime_store));
        let storage =
            meerkat_mob::MobStorage::persistent(&paths.mob_db_path).expect("persistent storage");
        let mob_service: Arc<dyn meerkat_mob::MobSessionService> = service.clone();
        let runtime_adapter = mob_service
            .runtime_adapter()
            .expect("persistent service exposes a runtime adapter");
        let handle = meerkat_mob::MobBuilder::new(
            support::controlling_mob_definition(mob_id.clone()),
            storage,
        )
        .with_session_service(mob_service)
        .with_runtime_adapter(runtime_adapter)
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .create()
        .await
        .expect("create persistent controlling mob");

        let report = handle
            .bind_host(descriptor_to_bind_request(&fixture.current_descriptor()))
            .await
            .expect("bind host in lifetime 1");
        // Stop without archiving (the cold-restart shape,
        // cold_restart_mob_resume.rs): `shutdown()` stops mob tasks — and
        // releases the supervisor-bridge listener port the resumed lifetime
        // must rebind — without touching durable state.
        handle
            .shutdown()
            .await
            .expect("shutdown controlling mob before restart");
        (report.host_id, report.epoch)
    };

    // ---------------- Lifetime 2: resume + recovered facts ----------------
    let service = persistent_service(&paths, runtime_store);
    let storage =
        meerkat_mob::MobStorage::persistent(&paths.mob_db_path).expect("reopen persistent storage");
    let mob_service: Arc<dyn meerkat_mob::MobSessionService> = service.clone();
    let runtime_adapter = mob_service
        .runtime_adapter()
        .expect("persistent service exposes a runtime adapter");
    let resumed = meerkat_mob::MobBuilder::for_resume(storage)
        .with_session_service(mob_service)
        .with_runtime_adapter(runtime_adapter)
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("resume controlling mob after cold restart");

    // FLAG-3(a): the durable MobHostAuthorityRecord written on CommitHostBind
    // in lifetime 1 carries the full recovered fact set (identity, endpoint,
    // signing key, epoch, capabilities, live endpoint). Asserted through a
    // second WAL handle on the same mob db — MobMachine host maps have no
    // public read on MobHandle; the record↔machine equality is pinned
    // in-crate (runtime/tests.rs
    // test_bind_host_commits_machine_facts_and_persists_record).
    let metadata = meerkat_mob::store::SqliteMobStores::open(&paths.mob_db_path)
        .expect("open mob db metadata handle")
        .runtime_metadata_store();
    let records = metadata
        .list_mob_host_authorities(&mob_id)
        .await
        .expect("host authority records after restart");
    assert_eq!(records.len(), 1, "one durable record per bound host");
    assert_eq!(records[0].host_id, host_id_string);
    assert_eq!(records[0].authority_epoch, bound_epoch);
    assert_eq!(records[0].bind_phase, MobHostBindPhaseRecord::Bound);
    let expected_pubkey = fixture
        .current_descriptor()
        .identity
        .resolve()
        .expect("host descriptor identity resolves")
        .pubkey;
    assert_eq!(
        records[0].signing_key, expected_pubkey,
        "the recovered record carries the host signing key"
    );
    assert!(
        !records[0].endpoint.is_empty(),
        "the recovered record carries the host endpoint"
    );
    assert!(records[0].capabilities.durable_sessions);
    assert_eq!(
        records[0].live_endpoint.as_deref(),
        Some("wss://restart-host.test.invalid/live"),
        "the recovered record carries the live endpoint fact"
    );

    // The recovered facts are LIVE machine authority, not a dead projection:
    // builder recovery applied RecoverHostBinding from the durable record
    // (without it the host answers AlreadyBound while the controlling side
    // has forgotten it exists — the operator dead-end the flag adjudication
    // names), and revoke_host operates on the recovered facts. The revoke
    // path only fires when the machine holds the host facts: the RevokeHost
    // transition must fire AND the deletion witness must mint from its
    // HostRevoked effect before the row can be deleted.
    resumed
        .revoke_host(&host_id_string)
        .await
        .expect("revoke recovered host binding");
    assert!(
        metadata
            .list_mob_host_authorities(&mob_id)
            .await
            .expect("host authority records after revoke")
            .is_empty(),
        "revoke of a RECOVERED binding deletes the durable record under the deletion witness"
    );

    fixture.shutdown().await;
}
