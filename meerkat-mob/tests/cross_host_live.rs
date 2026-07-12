//! Multi-host mobs phase 6b — remote live channels, controlling lane
//! (§16 / design-control-lane rows T-C1..T-C14 as adjudicated in
//! ADJ-P6B-11..16).
//!
//! Deterministic two-hosts-in-one-process battery over the phase-6 fixture
//! conventions (`cross_host_events.rs` precedent). The media plane is DIRECT
//! (DL1): the bridge carries open/close/status/control and NEVER frames —
//! T-C2's WS connect IS the input plane; there is no bridge `SendInput`.
//!
//! Merge-gated rows (need the live-pipeline lane's member arms + scripted
//! factory, ADJ-P6B seams S3/S3b/S4): T-C1..T-C5, T-C9..T-C11, T-C8b's
//! end-to-end half. Controlling-side-only rows (green on this lane alone):
//! T-C6, T-C7, T-C14.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/live_plane.rs"]
mod live_support;
mod support;

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::SinkExt as _;
use meerkat_mob::runtime::bridge_protocol::{
    BridgeLiveControlOutcome, BridgeLiveControlVerb, BridgeRejectionCause, BridgeReply,
    LiveCloseStatus, LiveOpenResult, LiveOpenTransport, WireLiveAdapterStatus,
    WireLiveTransportBootstrap,
};
use meerkat_mob::{AgentIdentity, MobControlPrincipal, MobError};
use support::{
    FIXTURE_BRIDGE_TIMEOUT, HostFixtureOptions, REAL_COMMS_TEST_LOCK, add_realtime_worker_profile,
    bind_then_materialize_with_spec, create_controlling_mob,
    create_controlling_mob_with_definition, create_controlling_mob_with_member_live_host,
    member_descriptor_from_ack, member_incarnation_from_ack, raw_close_member_live_channel_command,
    raw_member_live_status_command, raw_open_member_live_channel_command,
    realtime_portable_member_spec, scripted_member_client_completing, spawn_host_daemon_fixture,
    spawn_peer_comms_endpoint, wait_until,
};

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

/// A live-serving member host: bind the live listener FIRST so its real
/// address can ride both the bind fact and the serving plane; `advertise:
/// None` ⇒ connectable loopback base, `Some(base)` ⇒ distinguishable
/// non-listener base (the T-C1 verbatim proof).
async fn live_host_fixture(
    name: &str,
    advertise: Option<String>,
    factory: Arc<live_support::ScriptedRealtimeSessionFactory>,
) -> (support::HostDaemonFixture, live_support::FixtureLivePlane) {
    let (listener, local_addr) = live_support::bind_live_listener().await;
    let advertise_value = advertise
        .clone()
        .unwrap_or_else(|| format!("ws://{local_addr}"));
    let mut opts = HostFixtureOptions::named(name).with_member_build();
    opts.live_endpoint = Some(advertise_value);
    // The realtime rows materialize OpenAI-provider members (gpt-realtime-2
    // is the one realtime catalog row); keep Anthropic for plain workers.
    opts.resolvable_providers = vec![
        meerkat_core::Provider::Anthropic,
        meerkat_core::Provider::OpenAI,
    ];
    opts.member_llm_client = Some(scripted_member_client_completing("live-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn live member-host fixture");
    let plane = live_support::install_live_plane(&fixture, listener, advertise, factory).await;
    (fixture, plane)
}

fn websocket_bootstrap(open: &LiveOpenResult) -> (String, String) {
    match &open.transport {
        WireLiveTransportBootstrap::Websocket { url, token } => (url.clone(), token.clone()),
        other => panic!("expected websocket bootstrap, got {other:?}"),
    }
}

fn expect_rejected_cause<T: std::fmt::Debug>(
    result: Result<T, MobError>,
    expected: &BridgeRejectionCause,
    context: &str,
) {
    match result {
        Err(MobError::BridgeCommandRejected { cause, .. }) => {
            assert_eq!(&cause, expected, "{context}: cause mismatch");
        }
        other => panic!("{context}: expected BridgeCommandRejected({expected:?}), got {other:?}"),
    }
}

/// Every OBJECT KEY in `value`, recursively (the §16.9 live-field-free scan
/// checks keys, never values — base64 tokens can contain any substring).
fn collect_json_keys(value: &serde_json::Value, keys: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, nested) in map {
                keys.push(key.clone());
                collect_json_keys(nested, keys);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json_keys(item, keys);
            }
        }
        _ => {}
    }
}

// ==========================================================================
// T-C1 — proxied open returns the member's bootstrap VERBATIM: URL base =
// the ADVERTISED URL (never local_addr, never loopback-rewritten); token
// present; fields untouched (DEC-P6B-C7 / ADJ-P6B-14)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn proxied_open_returns_member_bootstrap_verbatim() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let advertised = "ws://live.advertised.test:19777".to_string();
    let (fixture, plane) = live_host_fixture(
        "xhl-t1-host-b",
        Some(advertised.clone()),
        live_support::scripted_realtime_factory(),
    )
    .await;
    let controlling =
        create_controlling_mob_with_definition("xhl-t1", add_realtime_worker_profile).await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("rt-worker", "c1", &report.host_id)
        .await
        .expect("realtime member materializes on host B");

    let open = controlling
        .handle
        .member_live_open(MobControlPrincipal::Owner, identity("c1"), None, None)
        .await
        .expect("proxied live open returns the owning host's bootstrap");

    assert!(!open.channel_id.is_empty(), "owning-side channel id minted");
    let (url, token) = websocket_bootstrap(&open);
    assert!(
        url.starts_with(&advertised),
        "bootstrap URL rides the ADVERTISED base verbatim (S5), got {url}"
    );
    assert!(
        !url.contains(&plane.local_addr.to_string()),
        "the controlling host never rewrites the URL to the member's local_addr: {url}"
    );
    assert!(
        url.contains(&format!("channel={}", open.channel_id)),
        "the G38 channel pin rides the bootstrap URL: {url}"
    );
    assert!(!token.is_empty(), "single-use WS bearer token present");

    fixture.shutdown().await;
}

// ==========================================================================
// T-C2 — the returned bootstrap is CONNECTABLE: direct WS to the member
// listener drives text input against the scripted factory (the WS IS the
// input plane — no bridge SendInput exists to call, DL10)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn direct_ws_connect_drives_text_input_end_to_end() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (fixture, plane) = live_host_fixture(
        "xhl-t2-host-b",
        None, // connectable loopback advertise
        live_support::scripted_realtime_factory(),
    )
    .await;
    let controlling =
        create_controlling_mob_with_definition("xhl-t2", add_realtime_worker_profile).await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("rt-worker", "c2", &report.host_id)
        .await
        .expect("realtime member materializes on host B");

    let open = controlling
        .handle
        .member_live_open(MobControlPrincipal::Owner, identity("c2"), None, None)
        .await
        .expect("proxied live open");
    let (url, _token) = websocket_bootstrap(&open);

    // Direct connect: the console's client dials the member host itself.
    let (mut ws, _response) = tokio_tungstenite::connect_async(url.as_str())
        .await
        .expect("direct WS connect to the member-host live listener");
    let chunk = meerkat_contracts::wire::LiveInputChunkWire::Text {
        text: "live text over the direct plane".to_string(),
    };
    let encoded = serde_json::to_string(&chunk).expect("encode live input chunk");
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        encoded.into(),
    ))
    .await
    .expect("send text input over the direct WS");

    // The scripted adapter observes the input on the OWNING host — the WS
    // IS the input plane (no bridge SendInput exists), and observations
    // project owning-side (§16.2, no live event proxy exists).
    let factory = Arc::clone(&plane.factory);
    wait_until("scripted adapter observed the WS text input", || {
        let factory = Arc::clone(&factory);
        async move {
            factory.adapters().iter().any(|adapter| {
                adapter.commands().iter().any(|command| {
                    matches!(
                        command,
                        meerkat_core::live_adapter::LiveAdapterCommand::SendInput {
                            chunk: meerkat_core::live_adapter::LiveInputChunk::Text { text },
                        } if text.contains("live text over the direct plane")
                    )
                })
            })
        }
    })
    .await;

    fixture.shutdown().await;
}

// ==========================================================================
// T-C3 — close returns the typed status; a closed channel's status is a
// typed LiveChannelNotFound (the reconciliation terminal)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn close_clears_channel_and_status_reports_not_found() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (fixture, _plane) = live_host_fixture(
        "xhl-t3-host-b",
        None,
        live_support::scripted_realtime_factory(),
    )
    .await;
    let controlling =
        create_controlling_mob_with_definition("xhl-t3", add_realtime_worker_profile).await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("rt-worker", "c3", &report.host_id)
        .await
        .expect("realtime member materializes on host B");

    let open = controlling
        .handle
        .member_live_open(MobControlPrincipal::Owner, identity("c3"), None, None)
        .await
        .expect("proxied live open");
    let closed = controlling
        .handle
        .member_live_close(
            MobControlPrincipal::Owner,
            identity("c3"),
            open.channel_id.clone(),
        )
        .await
        .expect("proxied live close");
    assert!(
        matches!(closed, LiveCloseStatus::Closed),
        "close returns the typed status, got {closed:?}"
    );

    // Local parity (RPC behavior freeze): a KNOWN-but-closed channel reports
    // its machine-adjudicated `Closed` status — the same truth local
    // `live/status` serves. `LiveChannelNotFound` stays reserved for UNKNOWN
    // ids (T-C4) and the `status(None)` no-active-channel probe.
    let closed_status = controlling
        .handle
        .member_live_status(
            MobControlPrincipal::Owner,
            identity("c3"),
            Some(open.channel_id.clone()),
        )
        .await
        .expect("status of a closed channel is a point read, not an error");
    assert_eq!(
        closed_status.channel_id, open.channel_id,
        "status echoes the probed channel"
    );
    assert_eq!(
        closed_status.status,
        WireLiveAdapterStatus::Closed,
        "closed channel reports its terminal status"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-C4 — the dedicated point read: channel_id:None discovers the active
// channel (ADJ-P6B-2); a wrong explicit id is typed LiveChannelNotFound;
// member_status stays live-field-free (§16.9 / ADJ-P6B-15)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn status_point_read_discovers_active_channel() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (fixture, _plane) = live_host_fixture(
        "xhl-t4-host-b",
        None,
        live_support::scripted_realtime_factory(),
    )
    .await;
    let controlling =
        create_controlling_mob_with_definition("xhl-t4", add_realtime_worker_profile).await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("rt-worker", "c4", &report.host_id)
        .await
        .expect("realtime member materializes on host B");

    let open = controlling
        .handle
        .member_live_open(MobControlPrincipal::Owner, identity("c4"), None, None)
        .await
        .expect("proxied live open");

    // Active-channel discovery: the console names NO channel id.
    let discovered = controlling
        .handle
        .member_live_status(MobControlPrincipal::Owner, identity("c4"), None)
        .await
        .expect("channel-less status resolves the member's active channel");
    assert_eq!(
        discovered.channel_id, open.channel_id,
        "status(None) discovers the active channel id"
    );

    // A wrong explicit id is an honest typed miss, never a coerced report.
    expect_rejected_cause(
        controlling
            .handle
            .member_live_status(
                MobControlPrincipal::Owner,
                identity("c4"),
                Some("no-such-channel".to_string()),
            )
            .await,
        &BridgeRejectionCause::LiveChannelNotFound,
        "status of an unknown channel id",
    );

    // §16.9: member_status assembly stays bridge-free and live-field-free —
    // the point read above is the ONLY remote channel-state path.
    let snapshot = controlling
        .handle
        .member_status(&identity("c4"))
        .await
        .expect("member status projection");
    let wire = snapshot
        .to_member_status_result(meerkat_contracts::wire::WireMemberRef::encode(
            controlling.mob_id.as_ref(),
            "c4",
        ))
        .expect("wire member status projection");
    let value = serde_json::to_value(&wire).expect("serialize member status");
    let mut keys = Vec::new();
    collect_json_keys(&value, &mut keys);
    assert!(
        keys.iter().all(|key| !key.contains("live")),
        "member_status is live-field-free (§16.9); offending keys in {keys:?}"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-C5 — control verbs: commit_input / interrupt / truncate / refresh each
// round-trip to a typed BridgeLiveControlOutcome and mutate the scripted
// adapter (DL10's turn-level vocabulary; no frame verbs)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn control_verbs_round_trip_and_mutate_the_scripted_adapter() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (fixture, plane) = live_host_fixture(
        "xhl-t5-host-b",
        None,
        live_support::scripted_realtime_factory(),
    )
    .await;
    let controlling =
        create_controlling_mob_with_definition("xhl-t5", add_realtime_worker_profile).await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("rt-worker", "c5", &report.host_id)
        .await
        .expect("realtime member materializes on host B");

    let open = controlling
        .handle
        .member_live_open(MobControlPrincipal::Owner, identity("c5"), None, None)
        .await
        .expect("proxied live open");

    let commit = controlling
        .handle
        .member_live_control(
            MobControlPrincipal::Owner,
            identity("c5"),
            open.channel_id.clone(),
            BridgeLiveControlVerb::CommitInput,
        )
        .await
        .expect("commit_input round-trips");
    assert!(
        matches!(commit, BridgeLiveControlOutcome::CommitInput { .. }),
        "commit_input outcome carries its own typed variant: {commit:?}"
    );

    let interrupt = controlling
        .handle
        .member_live_control(
            MobControlPrincipal::Owner,
            identity("c5"),
            open.channel_id.clone(),
            BridgeLiveControlVerb::Interrupt,
        )
        .await
        .expect("interrupt round-trips (media-plane barge-in, Live-scoped)");
    assert!(
        matches!(interrupt, BridgeLiveControlOutcome::Interrupt { .. }),
        "interrupt outcome carries its own typed variant: {interrupt:?}"
    );

    let truncate = controlling
        .handle
        .member_live_control(
            MobControlPrincipal::Owner,
            identity("c5"),
            open.channel_id.clone(),
            BridgeLiveControlVerb::Truncate {
                item_id: "item-1".to_string(),
                content_index: 0,
                audio_played_ms: 250,
            },
        )
        .await
        .expect("truncate round-trips");
    assert!(
        matches!(truncate, BridgeLiveControlOutcome::Truncate { .. }),
        "truncate outcome carries its own typed variant: {truncate:?}"
    );

    let refresh = controlling
        .handle
        .member_live_control(
            MobControlPrincipal::Owner,
            identity("c5"),
            open.channel_id.clone(),
            BridgeLiveControlVerb::Refresh,
        )
        .await
        .expect("refresh round-trips");
    assert!(
        matches!(refresh, BridgeLiveControlOutcome::Refresh { .. }),
        "refresh outcome carries its own typed variant: {refresh:?}"
    );

    // The verbs MUTATED the owning adapter (scripted-adapter observation):
    // each turn-level verb landed as its own `LiveAdapterCommand`.
    let factory = Arc::clone(&plane.factory);
    wait_until("scripted adapter observed the control verbs", || {
        let factory = Arc::clone(&factory);
        async move {
            use meerkat_core::live_adapter::LiveAdapterCommand;
            let commands: Vec<LiveAdapterCommand> = factory
                .adapters()
                .iter()
                .flat_map(|adapter| adapter.commands())
                .collect();
            commands
                .iter()
                .any(|command| matches!(command, LiveAdapterCommand::CommitInput { .. }))
                && commands
                    .iter()
                    .any(|command| matches!(command, LiveAdapterCommand::Interrupt))
                && commands.iter().any(|command| {
                    matches!(
                        command,
                        LiveAdapterCommand::TruncateAssistantOutput {
                            item_id,
                            content_index: 0,
                            audio_played_ms: 250,
                        } if item_id == "item-1"
                    )
                })
        }
    })
    .await;

    fixture.shutdown().await;
}

// ==========================================================================
// T-C6 — live-incapable host: typed LiveTransportUnavailable from the
// bind-time fact with ZERO bridge traffic (§16.6 row 1; ADJ-P6B-11)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn live_incapable_host_rejects_typed_with_zero_bridge_traffic() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhl-t6-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("t6-done"));
    // NO live_endpoint: absence IS the live-incapable fact (DL5).
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn live-incapable host fixture");
    let controlling = create_controlling_mob("xhl-t6").await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("worker", "c6", &report.host_id)
        .await
        .expect("worker materializes on host B");

    expect_rejected_cause(
        controlling
            .handle
            .member_live_open(MobControlPrincipal::Owner, identity("c6"), None, None)
            .await,
        &BridgeRejectionCause::LiveTransportUnavailable,
        "open against a live-incapable host",
    );
    expect_rejected_cause(
        controlling
            .handle
            .member_live_close(
                MobControlPrincipal::Owner,
                identity("c6"),
                "chan-x".to_string(),
            )
            .await,
        &BridgeRejectionCause::LiveTransportUnavailable,
        "close against a live-incapable host",
    );
    expect_rejected_cause(
        controlling
            .handle
            .member_live_status(MobControlPrincipal::Owner, identity("c6"), None)
            .await,
        &BridgeRejectionCause::LiveTransportUnavailable,
        "status against a live-incapable host",
    );
    expect_rejected_cause(
        controlling
            .handle
            .member_live_control(
                MobControlPrincipal::Owner,
                identity("c6"),
                "chan-x".to_string(),
                BridgeLiveControlVerb::CommitInput,
            )
            .await,
        &BridgeRejectionCause::LiveTransportUnavailable,
        "control against a live-incapable host",
    );

    assert_eq!(
        fixture.live_commands_served(),
        0,
        "the capability gate produced ZERO bridge traffic (S3b pin)"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-C7 — webrtc: typed LiveTransportUnsupported at the verb family,
// REGARDLESS of placement (DEC-P6B-C4 / ADJ-P6B-12); zero dispatch
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn webrtc_rejects_typed_with_zero_dispatch_on_both_placements() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhl-t7-host-b").with_member_build();
    // Live-CAPABLE bind fact (the reject must come from the transport gate,
    // not the capability gate); no serving plane is needed pre-dispatch.
    opts.live_endpoint = Some("wss://live.example.test:8443".to_string());
    opts.member_llm_client = Some(scripted_member_client_completing("t7-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn live-capable host fixture");
    let controlling = create_controlling_mob("xhl-t7").await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("worker", "placed-c7", &report.host_id)
        .await
        .expect("worker materializes on host B");
    controlling
        .handle
        .spawn_spec(
            meerkat_mob::SpawnMemberSpec::new("worker", "local-c7")
                .with_backend(meerkat_mob::MobBackendKind::Session),
        )
        .await
        .expect("local member spawns");

    for member in ["placed-c7", "local-c7"] {
        match controlling
            .handle
            .member_live_open(
                MobControlPrincipal::Owner,
                identity(member),
                None,
                Some(LiveOpenTransport::Webrtc),
            )
            .await
        {
            Err(MobError::BridgeCommandRejected {
                cause: BridgeRejectionCause::LiveTransportUnsupported { requested },
                ..
            }) => {
                assert_eq!(requested, "webrtc", "the reject names the transport");
            }
            other => panic!(
                "webrtc open for {member} must reject LiveTransportUnsupported, got {other:?}"
            ),
        }
    }

    assert_eq!(
        fixture.live_commands_served(),
        0,
        "the placement-blind webrtc gate dispatched NOTHING (S3b pin)"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-C9 — reply-loss reconciliation (DEC-P6B-C9 / ADJ-P6B-15): a raw open
// whose reply the console never processes leaves an owning-side orphan;
// retry ⇒ AlreadyBound; status(None) discovers; close(id) clears; a fresh
// open succeeds; the member-side OPEN COUNT proves no auto-resend fired
// (materialize_resend_class is NOT extended to any live command)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn reply_loss_probe_then_close_reconciles_without_resend() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let factory = live_support::scripted_realtime_factory();
    let (listener, local_addr) = live_support::bind_live_listener().await;
    let mut opts = HostFixtureOptions::named("xhl-t9-host-b").with_member_build();
    opts.live_endpoint = Some(format!("ws://{local_addr}"));
    opts.resolvable_providers = vec![
        meerkat_core::Provider::Anthropic,
        meerkat_core::Provider::OpenAI,
    ];
    opts.member_llm_client = Some(scripted_member_client_completing("t9-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn live member-host fixture");
    let plane =
        live_support::install_live_plane(&fixture, listener, None, Arc::clone(&factory)).await;

    // Raw supervisor probe (the raw_deliver precedent): the same wire
    // commands the MobHandle verbs emit, with the reply path in the test's
    // hands so it can be DISCARDED.
    let probe = spawn_peer_comms_endpoint("xhl-t9-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("xhl-t9-{}", uuid::Uuid::new_v4().simple());
    let ack = bind_then_materialize_with_spec(
        &probe,
        &fixture,
        &mob_id,
        realtime_portable_member_spec(&mob_id, "orphan", "worker"),
    )
    .await;
    let member = member_descriptor_from_ack("orphan", &ack);
    let expected_member = member_incarnation_from_ack(&fixture, &mob_id, "orphan", &ack, 1, 1, 1);
    probe.trust(member.clone()).await;

    // 1. Orphan mint: the open reply is dropped UNCONSUMED. Controlling-side
    //    amnesia is structural (DL2: no channel map exists), so a dropped
    //    reply and a lost reply are the same console state.
    let lost = probe
        .send_bridge_command_raw(
            &member,
            &raw_open_member_live_channel_command(
                &probe,
                &mob_id,
                1,
                expected_member.clone(),
                None,
                None,
            ),
            FIXTURE_BRIDGE_TIMEOUT,
        )
        .await
        .expect("raw open reaches the member");
    assert!(
        matches!(lost, BridgeReply::MemberLiveChannelOpened(_)),
        "the owning side minted a channel before the reply was lost: {lost:?}"
    );
    drop(lost);

    // 2. A blind open RETRY is not resend-safe and not resend-useful: the
    //    orphan holds the one-active-channel slot (owning-machine fact).
    let retry = probe
        .send_bridge_command_raw(
            &member,
            &raw_open_member_live_channel_command(
                &probe,
                &mob_id,
                1,
                expected_member.clone(),
                None,
                None,
            ),
            FIXTURE_BRIDGE_TIMEOUT,
        )
        .await
        .expect("raw retry reaches the member");
    match retry {
        BridgeReply::Rejected {
            cause: BridgeRejectionCause::LiveChannelAlreadyBound,
            ..
        } => {}
        other => panic!("retry against the orphan must reject AlreadyBound, got {other:?}"),
    }

    // 3. DISCOVERY: status with NO channel id resolves the member's active
    //    channel (ADJ-P6B-2 — the payload amendment this row forced).
    let status = probe
        .send_bridge_command_raw(
            &member,
            &raw_member_live_status_command(&probe, &mob_id, 1, expected_member.clone(), None),
            FIXTURE_BRIDGE_TIMEOUT,
        )
        .await
        .expect("raw channel-less status reaches the member");
    let channel_id = match status {
        BridgeReply::MemberLiveChannelStatusReport { channel_id, .. } => channel_id,
        other => panic!("channel-less status must discover the orphan, got {other:?}"),
    };

    // 4. CLEAR: close-what-you-name (the CAS property — the console closes
    //    exactly the orphan it discovered).
    let closed = probe
        .send_bridge_command_raw(
            &member,
            &raw_close_member_live_channel_command(
                &probe,
                &mob_id,
                1,
                expected_member.clone(),
                &channel_id,
            ),
            FIXTURE_BRIDGE_TIMEOUT,
        )
        .await
        .expect("raw close reaches the member");
    assert!(
        matches!(closed, BridgeReply::MemberLiveChannelClosed { .. }),
        "close clears the orphan: {closed:?}"
    );

    // 5. A fresh open now succeeds.
    let fresh = probe
        .send_bridge_command_raw(
            &member,
            &raw_open_member_live_channel_command(&probe, &mob_id, 1, expected_member, None, None),
            FIXTURE_BRIDGE_TIMEOUT,
        )
        .await
        .expect("raw fresh open reaches the member");
    assert!(
        matches!(fresh, BridgeReply::MemberLiveChannelOpened(_)),
        "reconciliation reopens cleanly: {fresh:?}"
    );

    // 6. OPEN-COUNT PIN: exactly the two EXPLICIT opens reached the
    //    provider factory — the AlreadyBound retry minted nothing and no
    //    resend classifier fired for any live command.
    assert_eq!(
        plane.factory.open_count(),
        2,
        "member-side open count == explicit opens only (no auto-resend)"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-C10 — duplicate proxied open: owning-machine AlreadyBound round-trips
// as the typed cause (§16.6 row 3)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_open_rejects_live_channel_already_bound() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (fixture, _plane) = live_host_fixture(
        "xhl-t10-host-b",
        None,
        live_support::scripted_realtime_factory(),
    )
    .await;
    let controlling =
        create_controlling_mob_with_definition("xhl-t10", add_realtime_worker_profile).await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("rt-worker", "c10", &report.host_id)
        .await
        .expect("realtime member materializes on host B");

    controlling
        .handle
        .member_live_open(MobControlPrincipal::Owner, identity("c10"), None, None)
        .await
        .expect("first open succeeds");
    expect_rejected_cause(
        controlling
            .handle
            .member_live_open(MobControlPrincipal::Owner, identity("c10"), None, None)
            .await,
        &BridgeRejectionCause::LiveChannelAlreadyBound,
        "duplicate open",
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-C11 — member-side typed causes round-trip: the console receives the
// SAME error class the local path emits (§16.6 row 2; ADJ-P6B-1 cause
// parity is the one conversion impl)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn member_side_causes_round_trip_typed() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;

    // (a) Non-realtime model: B19 catalog gate ⇒ ModelNotRealtime{model,
    //     provider} across the bridge.
    let (fixture, _plane) = live_host_fixture(
        "xhl-t11a-host-b",
        None,
        live_support::scripted_realtime_factory(),
    )
    .await;
    let controlling =
        create_controlling_mob_with_definition("xhl-t11a", add_realtime_worker_profile).await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("worker", "plain-c11", &report.host_id)
        .await
        .expect("non-realtime worker materializes on host B");
    match controlling
        .handle
        .member_live_open(
            MobControlPrincipal::Owner,
            identity("plain-c11"),
            None,
            None,
        )
        .await
    {
        Err(MobError::BridgeCommandRejected {
            cause: BridgeRejectionCause::ModelNotRealtime { model, provider },
            ..
        }) => {
            assert!(
                model.contains("claude-haiku"),
                "cause names the member's model, got {model}"
            );
            assert!(!provider.is_empty(), "cause names the member's provider");
        }
        other => panic!("expected typed ModelNotRealtime round-trip, got {other:?}"),
    }
    fixture.shutdown().await;

    // (b) Realtime model, factory supporting NO provider: B18 ⇒
    //     LiveAdapterUnavailable{provider} across the bridge.
    let (fixture, _plane) = live_host_fixture(
        "xhl-t11b-host-b",
        None,
        live_support::unsupporting_realtime_factory(),
    )
    .await;
    let controlling =
        create_controlling_mob_with_definition("xhl-t11b", add_realtime_worker_profile).await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("rt-worker", "rt-c11", &report.host_id)
        .await
        .expect("realtime member materializes on host B");
    match controlling
        .handle
        .member_live_open(MobControlPrincipal::Owner, identity("rt-c11"), None, None)
        .await
    {
        Err(MobError::BridgeCommandRejected {
            cause: BridgeRejectionCause::LiveAdapterUnavailable { provider },
            ..
        }) => {
            assert!(!provider.is_empty(), "cause names the unsupported provider");
        }
        other => panic!("expected typed LiveAdapterUnavailable round-trip, got {other:?}"),
    }
    fixture.shutdown().await;
}

// ==========================================================================
// T-C14 — the LOCAL branch routes through the injected
// meerkat_runtime::member_live::MemberLiveHost (the ADJ-P6B-1 builder
// knob); absent knob ⇒ typed LiveTransportUnavailable; zero bridge traffic
// ==========================================================================

/// Scripted local gateway: records every call's (verb, session id); returns
/// deterministic typed successes.
#[derive(Default)]
struct ScriptedMemberLiveHost {
    calls: StdMutex<Vec<(String, String)>>,
}

impl ScriptedMemberLiveHost {
    fn record(&self, verb: &str, session: &meerkat_core::SessionId) {
        self.calls
            .lock()
            .expect("scripted gateway lock")
            .push((verb.to_string(), session.to_string()));
    }

    fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().expect("scripted gateway lock").clone()
    }

    fn dummy_open_result() -> LiveOpenResult {
        LiveOpenResult {
            channel_id: "scripted-local-channel".to_string(),
            transport: WireLiveTransportBootstrap::Websocket {
                url: "ws://127.0.0.1:1/live/ws?token=t&channel=scripted-local-channel".to_string(),
                token: "t".to_string(),
            },
            capabilities: meerkat_contracts::wire::WireLiveChannelCapabilities {
                audio_in: true,
                audio_out: true,
                text_in: true,
                text_out: true,
                image_in: false,
                video_in: false,
                transcript_supported: true,
                barge_in_supported: true,
                provider_native_resume: false,
            },
            continuity: meerkat_contracts::wire::WireLiveContinuityMode::Fresh,
        }
    }
}

#[async_trait::async_trait]
impl meerkat_runtime::member_live::MemberLiveHost for ScriptedMemberLiveHost {
    async fn open(
        &self,
        session: &meerkat_core::SessionId,
        _turning_mode: Option<meerkat_contracts::wire::RealtimeTurningMode>,
        _transport: Option<LiveOpenTransport>,
    ) -> Result<LiveOpenResult, meerkat_runtime::member_live::MemberLiveError> {
        self.record("open", session);
        Ok(Self::dummy_open_result())
    }

    async fn close(
        &self,
        session: &meerkat_core::SessionId,
        _channel_id: &str,
    ) -> Result<LiveCloseStatus, meerkat_runtime::member_live::MemberLiveError> {
        self.record("close", session);
        Ok(LiveCloseStatus::Closed)
    }

    async fn status(
        &self,
        session: &meerkat_core::SessionId,
        channel_id: Option<String>,
    ) -> Result<
        meerkat_runtime::member_live::MemberLiveStatus,
        meerkat_runtime::member_live::MemberLiveError,
    > {
        self.record("status", session);
        Ok(meerkat_runtime::member_live::MemberLiveStatus {
            channel_id: channel_id.unwrap_or_else(|| "scripted-local-channel".to_string()),
            status: meerkat_contracts::wire::WireLiveAdapterStatus::Ready,
        })
    }

    async fn control(
        &self,
        session: &meerkat_core::SessionId,
        _channel_id: &str,
        _verb: BridgeLiveControlVerb,
    ) -> Result<BridgeLiveControlOutcome, meerkat_runtime::member_live::MemberLiveError> {
        self.record("control", session);
        Ok(BridgeLiveControlOutcome::CommitInput {
            status: meerkat_contracts::wire::LiveCommitInputStatus::Committed,
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn local_branch_routes_through_injected_member_live_host() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gateway = Arc::new(ScriptedMemberLiveHost::default());
    let controlling = create_controlling_mob_with_member_live_host(
        "xhl-t14",
        Arc::clone(&gateway) as Arc<dyn meerkat_runtime::member_live::MemberLiveHost>,
    )
    .await;
    controlling
        .handle
        .spawn_spec(
            meerkat_mob::SpawnMemberSpec::new("worker", "local-live")
                .with_backend(meerkat_mob::MobBackendKind::Session),
        )
        .await
        .expect("local member spawns");
    let session = controlling.member_session_id(&identity("local-live")).await;

    let open = controlling
        .handle
        .member_live_open(
            MobControlPrincipal::Owner,
            identity("local-live"),
            None,
            None,
        )
        .await
        .expect("local open routes through the injected gateway");
    assert_eq!(open.channel_id, "scripted-local-channel");

    let status = controlling
        .handle
        .member_live_status(MobControlPrincipal::Owner, identity("local-live"), None)
        .await
        .expect("local status routes through the injected gateway");
    assert_eq!(status.channel_id, "scripted-local-channel");

    let closed = controlling
        .handle
        .member_live_close(
            MobControlPrincipal::Owner,
            identity("local-live"),
            open.channel_id.clone(),
        )
        .await
        .expect("local close routes through the injected gateway");
    assert!(matches!(closed, LiveCloseStatus::Closed));

    let outcome = controlling
        .handle
        .member_live_control(
            MobControlPrincipal::Owner,
            identity("local-live"),
            open.channel_id.clone(),
            BridgeLiveControlVerb::CommitInput,
        )
        .await
        .expect("local control routes through the injected gateway");
    assert!(matches!(
        outcome,
        BridgeLiveControlOutcome::CommitInput { .. }
    ));

    // Every call carried the MEMBER's bound session id — the actor resolved
    // it; the gateway never saw an identity or a placement.
    let calls = gateway.calls();
    assert_eq!(
        calls.len(),
        4,
        "all four verbs routed through the gateway: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .all(|(_, recorded)| recorded == &session.to_string()),
        "the gateway is session-id-addressed with the member's session: {calls:?}"
    );

    // Absent knob (the default): the local branch degrades typed — this
    // process has no live transport.
    let bare = create_controlling_mob("xhl-t14b").await;
    bare.handle
        .spawn_spec(
            meerkat_mob::SpawnMemberSpec::new("worker", "local-bare")
                .with_backend(meerkat_mob::MobBackendKind::Session),
        )
        .await
        .expect("local member spawns");
    expect_rejected_cause(
        bare.handle
            .member_live_open(
                MobControlPrincipal::Owner,
                identity("local-bare"),
                None,
                None,
            )
            .await,
        &BridgeRejectionCause::LiveTransportUnavailable,
        "local open without an injected gateway",
    );
}

/// Lifecycle-race fixture. Open and Control stop at an explicit gate after the
/// actor has admitted and detached them; Status/Close expose the owning-side
/// active-channel truth used by the lifecycle reconciliation barrier.
#[derive(Default)]
struct LifecycleBarrierMemberLiveHost {
    calls: StdMutex<Vec<&'static str>>,
    active_channel: StdMutex<Option<String>>,
    open_started: AtomicBool,
    control_started: AtomicBool,
    panic_after_open_effect: AtomicBool,
    block_next_close: AtomicBool,
    close_started: AtomicBool,
    release_open: tokio::sync::Notify,
    release_control: tokio::sync::Notify,
}

impl LifecycleBarrierMemberLiveHost {
    fn record(&self, call: &'static str) {
        self.calls
            .lock()
            .expect("lifecycle live fixture calls lock")
            .push(call);
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls
            .lock()
            .expect("lifecycle live fixture calls lock")
            .clone()
    }

    fn seed_active_channel(&self) {
        self.seed_active_channel_named("lifecycle-live-channel");
    }

    fn seed_active_channel_named(&self, channel_id: &str) {
        *self
            .active_channel
            .lock()
            .expect("lifecycle live fixture active-channel lock") = Some(channel_id.to_string());
    }

    fn active_channel(&self) -> Option<String> {
        self.active_channel
            .lock()
            .expect("lifecycle live fixture active-channel lock")
            .clone()
    }
}

#[async_trait::async_trait]
impl meerkat_runtime::member_live::MemberLiveHost for LifecycleBarrierMemberLiveHost {
    async fn open(
        &self,
        _session: &meerkat_core::SessionId,
        _turning_mode: Option<meerkat_contracts::wire::RealtimeTurningMode>,
        _transport: Option<LiveOpenTransport>,
    ) -> Result<LiveOpenResult, meerkat_runtime::member_live::MemberLiveError> {
        self.record("open_started");
        self.open_started.store(true, Ordering::SeqCst);
        self.release_open.notified().await;
        if self.active_channel().is_none() {
            self.seed_active_channel();
        }
        if self.panic_after_open_effect.load(Ordering::SeqCst) {
            self.record("open_panicked_after_effect");
            panic!("scripted ambiguous live Open panic after channel materialization");
        }
        self.record("open_completed");
        let mut open = ScriptedMemberLiveHost::dummy_open_result();
        open.channel_id = "lifecycle-live-channel".to_string();
        Ok(open)
    }

    async fn close(
        &self,
        _session: &meerkat_core::SessionId,
        channel_id: &str,
    ) -> Result<LiveCloseStatus, meerkat_runtime::member_live::MemberLiveError> {
        self.record("close");
        if self.block_next_close.swap(false, Ordering::SeqCst) {
            self.close_started.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        }
        let mut active = self
            .active_channel
            .lock()
            .expect("lifecycle live fixture active-channel lock");
        if active.as_deref() != Some(channel_id) {
            return Err(meerkat_runtime::member_live::MemberLiveError::ChannelNotFound);
        }
        *active = None;
        Ok(LiveCloseStatus::Closed)
    }

    async fn status(
        &self,
        _session: &meerkat_core::SessionId,
        channel_id: Option<String>,
    ) -> Result<
        meerkat_runtime::member_live::MemberLiveStatus,
        meerkat_runtime::member_live::MemberLiveError,
    > {
        self.record("status");
        let active = self
            .active_channel
            .lock()
            .expect("lifecycle live fixture active-channel lock")
            .clone()
            .ok_or(meerkat_runtime::member_live::MemberLiveError::ChannelNotFound)?;
        if channel_id
            .as_ref()
            .is_some_and(|requested| requested != &active)
        {
            return Err(meerkat_runtime::member_live::MemberLiveError::ChannelNotFound);
        }
        Ok(meerkat_runtime::member_live::MemberLiveStatus {
            channel_id: active,
            status: WireLiveAdapterStatus::Ready,
        })
    }

    async fn control(
        &self,
        _session: &meerkat_core::SessionId,
        channel_id: &str,
        _verb: BridgeLiveControlVerb,
    ) -> Result<BridgeLiveControlOutcome, meerkat_runtime::member_live::MemberLiveError> {
        if self.active_channel().as_deref() != Some(channel_id) {
            return Err(meerkat_runtime::member_live::MemberLiveError::ChannelNotFound);
        }
        self.record("control_started");
        self.control_started.store(true, Ordering::SeqCst);
        self.release_control.notified().await;
        self.record("control_completed");
        Ok(BridgeLiveControlOutcome::CommitInput {
            status: meerkat_contracts::wire::LiveCommitInputStatus::Committed,
        })
    }
}

async fn lifecycle_live_fixture(
    label: &str,
    gateway: Arc<LifecycleBarrierMemberLiveHost>,
) -> support::ControllingMob {
    let controlling = create_controlling_mob_with_member_live_host(
        label,
        gateway as Arc<dyn meerkat_runtime::member_live::MemberLiveHost>,
    )
    .await;
    controlling
        .handle
        .spawn_spec(
            meerkat_mob::SpawnMemberSpec::new("worker", "lifecycle-live")
                .with_backend(meerkat_mob::MobBackendKind::Session),
        )
        .await
        .expect("local lifecycle-live member spawns");
    controlling
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_drains_inflight_live_open_then_closes_the_exact_channel() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gateway = Arc::new(LifecycleBarrierMemberLiveHost::default());
    let controlling = lifecycle_live_fixture("xhl-lifecycle-open", Arc::clone(&gateway)).await;

    let open_handle = controlling.handle.clone();
    let open_task = tokio::spawn(async move {
        open_handle
            .member_live_open(
                MobControlPrincipal::Owner,
                identity("lifecycle-live"),
                None,
                None,
            )
            .await
    });
    wait_until("detached live Open reached its effect gate", || {
        let gateway = Arc::clone(&gateway);
        async move { gateway.open_started.load(Ordering::SeqCst) }
    })
    .await;

    let stop_handle = controlling.handle.clone();
    let mut stop_task = tokio::spawn(async move { stop_handle.stop().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(75), &mut stop_task)
            .await
            .is_err(),
        "Stop must wait for the already-admitted live Open"
    );

    gateway.release_open.notify_one();
    open_task
        .await
        .expect("open task joins")
        .expect("Open linearizes before Stop");
    stop_task
        .await
        .expect("stop task joins")
        .expect("Stop closes the live input plane");

    assert_eq!(gateway.active_channel(), None);
    assert_eq!(
        gateway.calls(),
        vec!["open_started", "open_completed", "status", "close"],
        "the lifecycle barrier drains Open, discovers the active id, and closes it before Stop"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_drains_inflight_live_control_before_closing_the_channel() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gateway = Arc::new(LifecycleBarrierMemberLiveHost::default());
    gateway.seed_active_channel();
    let controlling = lifecycle_live_fixture("xhl-lifecycle-control", Arc::clone(&gateway)).await;

    let control_handle = controlling.handle.clone();
    let control_task = tokio::spawn(async move {
        control_handle
            .member_live_control(
                MobControlPrincipal::Owner,
                identity("lifecycle-live"),
                "lifecycle-live-channel".to_string(),
                BridgeLiveControlVerb::CommitInput,
            )
            .await
    });
    wait_until("detached live Control reached its effect gate", || {
        let gateway = Arc::clone(&gateway);
        async move { gateway.control_started.load(Ordering::SeqCst) }
    })
    .await;

    let stop_handle = controlling.handle.clone();
    let mut stop_task = tokio::spawn(async move { stop_handle.stop().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(75), &mut stop_task)
            .await
            .is_err(),
        "Stop must wait for the already-admitted live Control"
    );

    gateway.release_control.notify_one();
    control_task
        .await
        .expect("control task joins")
        .expect("Control linearizes before Stop");
    stop_task
        .await
        .expect("stop task joins")
        .expect("Stop closes the live input plane");

    assert_eq!(gateway.active_channel(), None);
    assert_eq!(
        gateway.calls(),
        vec!["control_started", "control_completed", "status", "close"],
        "Control completes before the lifecycle barrier closes its channel"
    );
}

async fn placed_lifecycle_live_fixture(
    label: &str,
    gateway: Arc<LifecycleBarrierMemberLiveHost>,
) -> (support::HostDaemonFixture, support::ControllingMob, String) {
    let host_name = format!("{label}-host");
    let mut opts = HostFixtureOptions::named(&host_name).with_member_build();
    opts.live_endpoint = Some("ws://placed-lifecycle.test:19777".to_string());
    opts.resolvable_providers = vec![
        meerkat_core::Provider::Anthropic,
        meerkat_core::Provider::OpenAI,
    ];
    opts.member_llm_client = Some(scripted_member_client_completing("placed-live-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn placed lifecycle member host");
    fixture
        .member_runtime_adapter
        .as_ref()
        .expect("placed lifecycle fixture has a runtime adapter")
        .set_member_live_host(
            Arc::clone(&gateway) as Arc<dyn meerkat_runtime::member_live::MemberLiveHost>
        );
    let controlling =
        create_controlling_mob_with_definition(label, add_realtime_worker_profile).await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("rt-worker", "placed-lifecycle-live", &report.host_id)
        .await
        .expect("placed lifecycle-live member materializes");
    (fixture, controlling, report.host_id)
}

#[tokio::test(flavor = "multi_thread")]
async fn placed_stop_drains_inflight_open_then_closes_on_the_owning_host() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gateway = Arc::new(LifecycleBarrierMemberLiveHost::default());
    let (fixture, controlling, _host_id) =
        placed_lifecycle_live_fixture("xhl-placed-lifecycle-open", Arc::clone(&gateway)).await;

    let open_handle = controlling.handle.clone();
    let open_task = tokio::spawn(async move {
        open_handle
            .member_live_open(
                MobControlPrincipal::Owner,
                identity("placed-lifecycle-live"),
                None,
                None,
            )
            .await
    });
    wait_until("placed live Open reached its owning-host gate", || {
        let gateway = Arc::clone(&gateway);
        async move { gateway.open_started.load(Ordering::SeqCst) }
    })
    .await;

    let stop_handle = controlling.handle.clone();
    let mut stop_task = tokio::spawn(async move { stop_handle.stop().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(75), &mut stop_task)
            .await
            .is_err(),
        "Stop must wait for an admitted placed Open"
    );
    gateway.release_open.notify_one();
    open_task
        .await
        .expect("placed open task joins")
        .expect("placed Open linearizes before Stop");
    stop_task
        .await
        .expect("placed stop task joins")
        .expect("Stop closes the placed live input plane");

    assert_eq!(gateway.active_channel(), None);
    assert_eq!(
        gateway.calls(),
        vec!["open_started", "open_completed", "status", "close"],
        "the controller drains the placed Open and closes on the owning host"
    );
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dropped_local_open_caller_is_exactly_cleaned_by_actor_custody() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gateway = Arc::new(LifecycleBarrierMemberLiveHost::default());
    let controlling = lifecycle_live_fixture("xhl-live-open-drop", Arc::clone(&gateway)).await;

    let open_handle = controlling.handle.clone();
    let open_task = tokio::spawn(async move {
        open_handle
            .member_live_open(
                MobControlPrincipal::Owner,
                identity("lifecycle-live"),
                None,
                None,
            )
            .await
    });
    wait_until("dropped caller Open reached its effect gate", || {
        let gateway = Arc::clone(&gateway);
        async move { gateway.open_started.load(Ordering::SeqCst) }
    })
    .await;
    open_task.abort();
    let _ = open_task.await;
    gateway.release_open.notify_one();

    wait_until("actor custody exact-closed the unacknowledged Open", || {
        let gateway = Arc::clone(&gateway);
        async move {
            gateway.active_channel().is_none()
                && gateway.calls().ends_with(&["open_completed", "close"])
        }
    })
    .await;
    assert_eq!(
        gateway.calls(),
        vec!["open_started", "open_completed", "close"],
        "reply loss closes the exact returned id without an ambiguous status probe"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn successful_open_returns_only_after_durable_cleanup_custody_is_deleted() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gateway = Arc::new(LifecycleBarrierMemberLiveHost::default());
    let controlling =
        lifecycle_live_fixture("xhl-live-open-custody-transfer", Arc::clone(&gateway)).await;

    let open_handle = controlling.handle.clone();
    let open_task = tokio::spawn(async move {
        open_handle
            .member_live_open(
                MobControlPrincipal::Owner,
                identity("lifecycle-live"),
                None,
                None,
            )
            .await
    });
    wait_until("custody-transfer Open reached its effect gate", || {
        let gateway = Arc::clone(&gateway);
        async move { gateway.open_started.load(Ordering::SeqCst) }
    })
    .await;
    gateway.release_open.notify_one();

    let opened = open_task
        .await
        .expect("Open task joins")
        .expect("Open returns after durable custody transfer");
    assert_eq!(opened.channel_id, "lifecycle-live-channel");
    assert!(
        controlling
            .storage_metadata
            .list_member_live_cleanup_records(&controlling.mob_id)
            .await
            .expect("list durable member-live cleanup custody")
            .is_empty(),
        "public success cannot return while a recovery-visible cleanup row could reclaim it"
    );

    controlling
        .handle
        .member_live_close(
            MobControlPrincipal::Owner,
            identity("lifecycle-live"),
            opened.channel_id,
        )
        .await
        .expect("close transferred live channel");
}

#[tokio::test(flavor = "multi_thread")]
async fn ambiguous_open_panic_never_closes_an_unnamed_preexisting_channel() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gateway = Arc::new(LifecycleBarrierMemberLiveHost::default());
    gateway
        .panic_after_open_effect
        .store(true, Ordering::SeqCst);
    gateway.seed_active_channel_named("preexisting-legitimate-channel");
    let controlling = lifecycle_live_fixture("xhl-live-open-panic", Arc::clone(&gateway)).await;

    let open_handle = controlling.handle.clone();
    let open_task = tokio::spawn(async move {
        open_handle
            .member_live_open(
                MobControlPrincipal::Owner,
                identity("lifecycle-live"),
                None,
                None,
            )
            .await
    });
    wait_until("panicking Open reached its effect gate", || {
        let gateway = Arc::clone(&gateway);
        async move { gateway.open_started.load(Ordering::SeqCst) }
    })
    .await;
    gateway.release_open.notify_one();

    let result = open_task.await.expect("Open task joins");
    assert!(
        result.is_err(),
        "the original Open panic remains caller-visible"
    );
    assert_eq!(
        gateway.active_channel().as_deref(),
        Some("preexisting-legitimate-channel"),
        "an ambiguous Open without a returned id must not reclaim the member's current channel"
    );
    assert_eq!(
        gateway.calls(),
        vec!["open_started", "open_panicked_after_effect"],
        "ambiguous Open cleanup stays caller-driven and performs no unnamed status/close"
    );
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn dropped_open_cleanup_recovers_after_actor_crash_restart() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gateway = Arc::new(LifecycleBarrierMemberLiveHost::default());
    gateway.block_next_close.store(true, Ordering::SeqCst);
    let controlling = lifecycle_live_fixture("xhl-live-open-restart", Arc::clone(&gateway)).await;

    let open_handle = controlling.handle.clone();
    let open_task = tokio::spawn(async move {
        open_handle
            .member_live_open(
                MobControlPrincipal::Owner,
                identity("lifecycle-live"),
                None,
                None,
            )
            .await
    });
    wait_until("restart Open reached its effect gate", || {
        let gateway = Arc::clone(&gateway);
        async move { gateway.open_started.load(Ordering::SeqCst) }
    })
    .await;
    open_task.abort();
    let _ = open_task.await;
    gateway.release_open.notify_one();
    wait_until("first cleanup attempt is blocked", || {
        let gateway = Arc::clone(&gateway);
        async move { gateway.close_started.load(Ordering::SeqCst) }
    })
    .await;

    let _restarted = controlling.crash_restart().await;
    wait_until("recovered durable Open cleanup exact-closes", || {
        let gateway = Arc::clone(&gateway);
        async move { gateway.active_channel().is_none() }
    })
    .await;
    assert_eq!(
        gateway
            .calls()
            .iter()
            .filter(|call| **call == "close")
            .count(),
        2,
        "one aborted close before crash and one recovered exact close after restart"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cleanup_backed_host_revoke_allows_later_stop_without_reaching_revoked_host() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gateway = Arc::new(LifecycleBarrierMemberLiveHost::default());
    gateway.seed_active_channel();
    let (fixture, controlling, host_id) =
        placed_lifecycle_live_fixture("xhl-live-revoke-stop", Arc::clone(&gateway)).await;

    let report = controlling
        .handle
        .revoke_host(&host_id)
        .await
        .expect("host revoke closes/proves every owning live channel before its receipt");
    assert!(
        report
            .released_members
            .contains(&identity("placed-lifecycle-live")),
        "cleanup-backed receipt names the disposed placed identity"
    );
    assert_eq!(gateway.active_channel(), None);
    let calls_after_revoke = gateway.calls();
    assert_eq!(
        calls_after_revoke
            .iter()
            .filter(|call| **call == "close")
            .count(),
        1,
        "the pre-revoke barrier exact-closes the active channel once"
    );

    controlling
        .handle
        .stop()
        .await
        .expect("confirmed cleanup-backed revoke counts as live absence during Stop");
    assert_eq!(
        gateway.calls(),
        calls_after_revoke,
        "Stop does not attempt status/close over revoked authority"
    );
    fixture.shutdown().await;
}
