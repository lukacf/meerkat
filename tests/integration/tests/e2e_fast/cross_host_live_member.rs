//! Phase 6b — deterministic MEMBER-side live rows (work item L9;
//! T-L21..T-L24, T-L26) on the phase-6 two-hosts harness: a member host
//! fixture serves bridge-delivered live verbs end-to-end through the ONE
//! extracted pipeline — token minted by the OWNING session's machine,
//! bootstrap URL based on the ADVERTISED URL, direct WS connect driving
//! text input against the scripted factory, token misuse failing closed,
//! and the reply-loss reconciliation primitives (status discovers, close
//! clears, reopen succeeds; NO auto-resend).
//!
//! T-L25 (e2e-system marker, §16.6 row 7): member-host restart with a
//! console WS attached — the socket dies with the process (transport
//! registry is process memory), post-rebind `MemberLiveChannelStatus`
//! reports no active channel, and a fresh open succeeds. That row needs a
//! REAL process restart and lives with the e2e-system daemon lifecycle
//! lane (`meerkat-cli/tests/system_mob_host_daemon.rs` family), not here.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::live_support;
use super::support;

use futures::{SinkExt as _, StreamExt as _};
use meerkat_contracts::LiveInputChunkWire;
use meerkat_core::live_adapter::{LiveAdapterCommand, LiveInputChunk};
use meerkat_mob::runtime::bridge_protocol::{BridgeRejectionCause, BridgeReply};
use support::{
    FIXTURE_BRIDGE_TIMEOUT, HostFixtureOptions, REAL_COMMS_TEST_LOCK,
    bind_then_materialize_with_spec, member_descriptor_from_ack, member_incarnation_from_ack,
    raw_close_member_live_channel_command, raw_member_live_status_command,
    raw_open_member_live_channel_command, raw_poll_member_events_command,
    realtime_portable_member_spec, spawn_host_daemon_fixture, spawn_peer_comms_endpoint,
    wait_until,
};

fn websocket_bootstrap(reply: &BridgeReply) -> (String, String, String) {
    match reply {
        BridgeReply::MemberLiveChannelOpened(response) => match &response.open.transport {
            meerkat_contracts::WireLiveTransportBootstrap::Websocket { url, token } => {
                (response.open.channel_id.clone(), url.clone(), token.clone())
            }
            other => panic!("expected websocket bootstrap, got {other:?}"),
        },
        other => panic!("expected MemberLiveChannelOpened, got {other:?}"),
    }
}

/// Fail-closed pin for token misuse: the transport admits the HTTP
/// upgrade unconditionally and adjudicates the token post-upgrade, so a
/// rejected connect shows up as (a) an outright handshake error, or (b) a
/// connected socket whose FIRST server frame is the typed error frame or
/// an immediate close — never a served input plane.
async fn assert_ws_rejected_post_upgrade(url: &str, context: &str) {
    let Ok((mut socket, _response)) = tokio_tungstenite::connect_async(url).await else {
        return;
    };
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .unwrap_or_else(|_| panic!("{context}: server neither rejected nor closed the socket"));
    match first {
        None => {}
        Some(Err(_)) => {}
        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => {}
        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
            assert!(
                text.contains("error"),
                "{context}: expected the typed error frame, got {text}"
            );
        }
        Some(Ok(other)) => panic!("{context}: expected rejection, got frame {other:?}"),
    }
}

/// T-L21 + T-L23: a bridge-delivered open mints a bootstrap whose base is
/// the ADVERTISED URL — never the listener's `local_addr` — with the
/// machine-minted token and the pipeline-owned `ProviderManaged` default;
/// and the mob-owned peer-ingress skip HOLDS (the member host's ingress
/// hook is fail-closed `MobOwnedOnlyIngress`, so a successful open IS the
/// proof no session-owned drain reconfigure was attempted).
#[tokio::test(flavor = "multi_thread")]
async fn member_open_mints_bootstrap_from_advertised_url_not_local_addr() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (listener, local_addr) = live_support::bind_live_listener().await;
    // Distinguishable base: NOT the bound listener address.
    let advertise = "ws://advertised.live-member.test.invalid:19099".to_string();
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        live_endpoint: Some(advertise.clone()),
        ..HostFixtureOptions::named("xlm-advertise-host").with_member_build()
    })
    .await
    .expect("spawn member host fixture");
    let factory = live_support::scripted_realtime_factory();
    let _plane = live_support::install_live_plane(
        &fixture,
        listener,
        Some(advertise.clone()),
        std::sync::Arc::clone(&factory),
    )
    .await;

    let probe = spawn_peer_comms_endpoint("xlm-advertise-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("xlm-adv-{}", uuid::Uuid::new_v4().simple());
    let ack = bind_then_materialize_with_spec(
        &probe,
        &fixture,
        &mob_id,
        realtime_portable_member_spec(&mob_id, "live-worker", "worker"),
    )
    .await;
    let member = member_descriptor_from_ack("live-worker", &ack);
    let expected_member =
        member_incarnation_from_ack(&fixture, &mob_id, "live-worker", &ack, 1, 1, 1);
    probe.trust(member.clone()).await;

    let open =
        raw_open_member_live_channel_command(&probe, &mob_id, 1, expected_member, None, None);
    let reply = probe
        .send_bridge_command_raw(&member, &open, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("open reply");
    let (channel_id, url, token) = websocket_bootstrap(&reply);
    assert!(
        url.starts_with(&advertise),
        "T-L21: bootstrap base must be the ADVERTISED URL, got {url}"
    );
    assert!(
        !url.contains(&local_addr.to_string()),
        "T-L21: bootstrap must never leak the listener local_addr, got {url}"
    );
    assert!(url.contains(&format!("channel={channel_id}")));
    assert!(!token.is_empty(), "machine-minted token must be present");

    // DEC-P6B-L15: absent turning_mode arrives at the factory as the
    // pipeline-owned ProviderManaged default.
    let opens = factory.opens();
    assert_eq!(opens.len(), 1, "exactly one scripted open");
    assert_eq!(
        opens[0].turning_mode,
        meerkat_contracts::RealtimeTurningMode::ProviderManaged,
    );

    fixture.shutdown().await;
}

/// T-L22 + T-L24 + T-L26: end-to-end against a CONNECTABLE advertise —
/// direct WS connect drives text input into the scripted adapter (the WS
/// IS the input plane); token misuse (wrong channel, garbage token,
/// double-consume) fails closed at the owning machine; the reply-loss
/// reconciliation primitives hold member-side (status(None) discovers,
/// close-by-id clears + re-close is typed not-found, reopen succeeds, NO
/// auto-resend inflated the open count); live observations ride ONLY the
/// ordinary member-events pump (parity-by-absence — no live event verb
/// exists to serve).
#[tokio::test(flavor = "multi_thread")]
async fn member_live_ws_input_token_misuse_and_reconciliation_primitives() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (listener, local_addr) = live_support::bind_live_listener().await;
    let advertise = format!("ws://{local_addr}");
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        live_endpoint: Some(advertise.clone()),
        ..HostFixtureOptions::named("xlm-ws-host").with_member_build()
    })
    .await
    .expect("spawn member host fixture");
    let factory = live_support::scripted_realtime_factory();
    let _plane =
        live_support::install_live_plane(&fixture, listener, None, std::sync::Arc::clone(&factory))
            .await;

    let probe = spawn_peer_comms_endpoint("xlm-ws-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("xlm-ws-{}", uuid::Uuid::new_v4().simple());
    let ack = bind_then_materialize_with_spec(
        &probe,
        &fixture,
        &mob_id,
        realtime_portable_member_spec(&mob_id, "ws-worker", "worker"),
    )
    .await;
    let member = member_descriptor_from_ack("ws-worker", &ack);
    let expected_member =
        member_incarnation_from_ack(&fixture, &mob_id, "ws-worker", &ack, 1, 1, 1);
    probe.trust(member.clone()).await;

    // Open #1 (explicit).
    let open = raw_open_member_live_channel_command(
        &probe,
        &mob_id,
        1,
        expected_member.clone(),
        None,
        None,
    );
    let reply = probe
        .send_bridge_command_raw(&member, &open, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("open reply");
    let (channel_id, url, _token) = websocket_bootstrap(&reply);

    // T-L22 (misuse first — the token is single-use, so misuse rows must
    // run BEFORE the consuming connect): wrong-channel pin and a garbage
    // token both fail closed at the owning machine's admission. The
    // transport admits the HTTP upgrade unconditionally and adjudicates
    // the token POST-upgrade (typed error frame + POLICY close, or a
    // silent drop) — fail-closed is observed on the socket, never as a
    // connect error.
    let wrong_channel_url = url.replace(
        &format!("channel={channel_id}"),
        "channel=chan-not-this-one",
    );
    assert_ws_rejected_post_upgrade(
        &wrong_channel_url,
        "a token replayed against a different channel must fail closed",
    )
    .await;
    let garbage_url = format!(
        "{advertise}{}?token=not-a-minted-token&channel={channel_id}&format=pcm_24k_mono",
        meerkat_live::LIVE_WS_PATH
    );
    assert_ws_rejected_post_upgrade(&garbage_url, "an unminted token must fail closed").await;

    // The wrong-channel probe BURNS the single-use token (the machine's
    // TokenChannelMismatch transition consumes on mismatch — the
    // oracle-probing defense, dsl ResolveLiveWebsocketTokenAdmission*):
    // the once-legitimate bootstrap URL is now fail-closed too. Channel
    // #1 is thereby EXACTLY the reply-loss orphan shape — an active
    // owning-side binding whose token nobody can consume — which the
    // reconciliation primitives below discover and clear.
    assert_ws_rejected_post_upgrade(
        &url,
        "a token burned by a mismatch probe must fail closed on its own channel",
    )
    .await;

    // T-L26 parity-by-absence: live-derived facts ride the ORDINARY
    // member-events pump; there is no live event verb to serve. A Tail
    // poll round-trips as a plain MemberEventsPage.
    let poll = raw_poll_member_events_command(
        &probe,
        &mob_id,
        1,
        expected_member.clone(),
        meerkat_mob::runtime::bridge_protocol::BridgeEventCursor::Tail,
        Some(16),
        Some(0),
    );
    let reply = probe
        .send_bridge_command_raw(&member, &poll, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("tail poll reply");
    assert!(
        matches!(reply, BridgeReply::MemberEventsPage(_)),
        "live observation parity is the ordinary pump: {reply:?}"
    );

    // T-L24 — the reply-loss reconciliation primitives, member side:
    // status(None) DISCOVERS the active channel id (ADJ-P6B-2)...
    let status = raw_member_live_status_command(&probe, &mob_id, 1, expected_member.clone(), None);
    let reply = probe
        .send_bridge_command_raw(&member, &status, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("status(None) reply");
    match reply {
        BridgeReply::MemberLiveChannelStatusReport {
            channel_id: reported,
            ..
        } => assert_eq!(
            reported, channel_id,
            "status(None) must discover the orphan id"
        ),
        other => panic!("expected MemberLiveChannelStatusReport, got {other:?}"),
    }

    // ...close-by-id CLEARS it...
    let close = raw_close_member_live_channel_command(
        &probe,
        &mob_id,
        1,
        expected_member.clone(),
        &channel_id,
    );
    let reply = probe
        .send_bridge_command_raw(&member, &close, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("close reply");
    assert!(
        matches!(reply, BridgeReply::MemberLiveChannelClosed { .. }),
        "close-by-id must clear the channel: {reply:?}"
    );

    // ...a re-close of the cleared channel is the typed not-found
    // (idempotent-safe for the caller-driven loop)...
    let reclose = raw_close_member_live_channel_command(
        &probe,
        &mob_id,
        1,
        expected_member.clone(),
        &channel_id,
    );
    let reply = probe
        .send_bridge_command_raw(&member, &reclose, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("re-close reply");
    match reply {
        BridgeReply::Rejected { cause, .. } => {
            assert_eq!(cause, BridgeRejectionCause::LiveChannelNotFound);
        }
        other => panic!("expected typed LiveChannelNotFound, got {other:?}"),
    }

    // ...and a FRESH open succeeds (open #2, explicit).
    let reopen =
        raw_open_member_live_channel_command(&probe, &mob_id, 1, expected_member, None, None);
    let reply = probe
        .send_bridge_command_raw(&member, &reopen, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("reopen reply");
    let (_channel2, url2, _token2) = websocket_bootstrap(&reply);

    // The legitimate connect consumes open #2's single-use token.
    let (mut socket, _response) = tokio_tungstenite::connect_async(&url2)
        .await
        .expect("direct WS connect to the member listener");

    // Double-consume: the same bootstrap URL must not admit twice.
    assert_ws_rejected_post_upgrade(
        &url2,
        "a consumed single-use token must fail closed on replay",
    )
    .await;

    // The WS IS the input plane: a text chunk lands on the scripted
    // adapter as the SendInput command.
    let chunk = serde_json::to_string(&LiveInputChunkWire::Text {
        text: "hello member".to_string(),
    })
    .expect("serialize input chunk");
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(chunk.into()))
        .await
        .expect("send WS text input");
    let adapter = factory.adapters().pop().expect("scripted adapter minted");
    wait_until("scripted adapter observes the WS text input", || {
        let adapter = std::sync::Arc::clone(&adapter);
        async move {
            adapter.commands().iter().any(|command| {
                matches!(
                    command,
                    LiveAdapterCommand::SendInput {
                        chunk: LiveInputChunk::Text { text },
                    } if text == "hello member"
                )
            })
        }
    })
    .await;

    // The T-C9/T-L24 pin: EXACTLY the explicit opens — no live command
    // joined any resend classifier.
    assert_eq!(
        factory.open_count(),
        2,
        "member-side open count must equal the explicit opens only (no auto-resend)"
    );

    drop(socket);
    fixture.shutdown().await;
}
