//! Cross-mob control round trip - the end-to-end proof that two
//! `UnifiedRuntime`s can wire members across a real TCP/UDS control channel
//! and then exchange agent-plane comms envelopes in BOTH directions.
//! (This is the file promised by `tests/cross_mob_tcp.rs`.)
//!
//! Topology per test: two UnifiedRuntimes in one process, each with its own
//! mob and its own cross-mob control listener bound on an ephemeral port
//! (`control_listen("tcp://127.0.0.1:0")`). Members run their comms
//! runtimes in TCP mode (`comms.mode = tcp`, `127.0.0.1:0`), so every
//! member has a real dialable envelope listener with a kernel-assigned
//! port. Runtime A's contact directory points at B's *bound* control
//! address, so `wire_cross_mob` flows A -> control channel -> B and
//! installs signed `TrustedPeerDescriptor`s on both sides:
//!
//! * on A: bob's member facts (peer id, comms name, transport pubkey,
//!   advertised tcp address) discovered via `LookupMember`;
//! * on B: alice's member facts carried by the `Wire` request - the
//!   reverse leg. Before the reverse-address fix this carried
//!   `inproc://...` and the gateway pubkey, which was both unreachable
//!   from another process and rejected by the peer-id/pubkey consistency
//!   check; the descriptor could never deliver.
//!
//! Delivery proof:
//! 1. control plane A->B: `send_cross_mob` (Inject) returns bob's bridge
//!    session id.
//! 2. agent plane A->B: alice's comms runtime sends a `PeerMessage` to the
//!    bob entry in HER trust directory; the router dials bob's member
//!    listener over real TCP and requires an ack signed by bob's keypair.
//! 3. agent plane B->A (the reply leg): bob sends a `PeerMessage` to the
//!    alice entry the REMOTE Wire request installed. A successful, acked
//!    send is cryptographic proof that the reverse descriptor carries a
//!    dialable address and alice's real member pubkey.
//!
//! # Two-process variant
//!
//! Deliberately not implemented here. The subprocess harness
//! (`tests/identity_first_subprocess_reboot.rs`) drives the `rpc_gateway`
//! binary over stdin JSON-RPC, and that binary has no control-listen
//! surface at all today; `--control-listen` exists on `meta_gateway`,
//! which surfaces its bound control address only in tracing, not in the
//! init response. A two-process round trip needs BOTH processes to hand
//! their bound control address and gateway pubkey back to the test before
//! either contact directory can be written, so it requires a gateway
//! protocol change (an init-response field, plus the flag on rpc_gateway),
//! not just a test. That is a small follow-up, out of scope for this
//! lane. Everything transport-level that a second process would exercise
//! (real TCP sockets for control AND envelope planes, signed descriptor
//! verification at ingress, ack signing by the addressed keypair) is
//! already crossed by the in-process pair below - nothing here shares
//! memory except the test harness itself.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::sync::Arc;

use meerkat_client::TestClient;
use meerkat_core::comms::{CommsCommand, PeerRoute, PeerSendability};
use meerkat_core::types::HandlingMode;
use meerkat_meta::contact_directory::{ContactDirectory, ContactEntry, MobTransport};
use meerkat_meta::runtime::cross_mob_control::{
    ControlGrant, ControlGrantTable, ControlMemberScope, ControlVerb,
};
use meerkat_meta::runtime::cross_mob_remote::{RemoteMobError, RemoteMobProxy};
use meerkat_meta::runtime::keyed_inject::{KeyedInjectCallOutcome, NativeStatus};
use meerkat_meta::{GatewayPeerKeys, UnifiedRuntime, UnifiedRuntimeBuilder};
use meerkat_mob::{MobDefinition, SpawnMemberSpec};

const MOB_TOML_A: &str = r#"
[mob]
id = "mob-a"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#;

const MOB_TOML_B: &str = r#"
[mob]
id = "mob-b"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#;

/// Every member binds its own signed comms listener on an ephemeral
/// loopback port, so cross-process envelope delivery is real TCP.
fn tcp_member_comms_config() -> meerkat::Config {
    let mut config = meerkat::Config::default();
    config.comms.mode = meerkat_core::CommsRuntimeMode::Tcp;
    config.comms.address = Some("127.0.0.1:0".to_string());
    config
}

fn grants_for(caller: &GatewayPeerKeys, member: &str) -> ControlGrantTable {
    let mut grants = ControlGrantTable::new();
    grants.insert(
        caller.pubkey_bytes(),
        ControlGrant::new(
            "peer-gateway",
            ControlVerb::member_plane(),
            ControlMemberScope::members([member]),
        ),
    );
    grants
}

/// Every test here bootstraps the same mob-a/mob-b pair and shuts both down
/// cleanly. 0.8.23's fail-closed in-proc registration refuses a live route
/// takeover, so the tests serialize (topology_control precedent): terminal
/// shutdown releases the supervisor routes before the next test bootstraps.
static CONTROL_TEST_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

async fn build_runtime(
    mob_toml: &str,
    control_listen: &str,
    keys: GatewayPeerKeys,
    grants: ControlGrantTable,
) -> UnifiedRuntime {
    let definition = MobDefinition::from_toml(mob_toml).expect("parse mob definition");
    let mut runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition)
            .default_llm_client(Arc::new(TestClient::default()))
            .meerkat_config(tcp_member_comms_config())
            .control_listen(control_listen)
            .control_grants(grants)
            .build(),
    )
    .await
    .expect("build runtime");
    runtime.set_gateway_peer_keys(keys);
    runtime
}

async fn spawn_member(runtime: &UnifiedRuntime, member: &str) {
    runtime
        .mob_handle()
        .ensure_member(SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ids::AgentIdentity::from(member),
        ))
        .await
        .expect("ensure member");
}

/// The live comms runtime backing a member's bridge session.
async fn member_comms(
    runtime: &UnifiedRuntime,
    member: &str,
) -> Arc<dyn meerkat_core::agent::CommsRuntime> {
    let handle = runtime.mob_handle();
    let session_id = handle
        .resolve_bridge_session_id(&meerkat_mob::ids::AgentIdentity::from(member))
        .await
        .expect("member has a bridge session");
    runtime
        .mob_runtime()
        .session_service()
        .expect("session service")
        .comms_runtime(&session_id)
        .await
        .expect("member comms runtime")
}

fn comms_name(mob_id: &str, member: &str) -> String {
    meerkat_core::MemberCommsName::new(mob_id, "worker", member)
        .expect("comms name")
        .to_string()
}

/// Find `peer_name` in the runtime-visible peer directory of a member's
/// comms runtime, requiring PeerMessage sendability.
async fn sendable_peer(
    comms: &Arc<dyn meerkat_core::agent::CommsRuntime>,
    peer_name: &str,
) -> Option<meerkat_core::comms::PeerDirectoryEntry> {
    comms.peers().await.into_iter().find(|peer| {
        peer.name.as_str() == peer_name
            && peer.sendable_kinds.contains(&PeerSendability::PeerMessage)
    })
}

fn peer_message(to: &meerkat_core::comms::PeerDirectoryEntry, body: &str) -> CommsCommand {
    CommsCommand::PeerMessage {
        to: PeerRoute::new(to.peer_id),
        body: body.to_string(),
        blocks: None,
        content_taint: None,
        handling_mode: HandlingMode::Queue,
        objective_id: None,
        in_reply_to: None,
    }
}

/// A peer-message send is only a delivery proof when the receipt carries a
/// verified signed ack from the addressed member's keypair (the TCP path);
/// anything else would mean the envelope took a route this test is not
/// trying to prove.
fn assert_acked(receipt: meerkat_core::comms::SendReceipt, leg: &str) {
    match receipt {
        meerkat_core::comms::SendReceipt::PeerMessageSent { delivery, .. } => {
            assert_eq!(
                delivery,
                meerkat_core::comms::PeerDeliveryOutcome::Acked,
                "{leg}: delivery must be a verified signed ack over the socket transport"
            );
        }
        other => panic!("{leg}: expected PeerMessageSent, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn configured_listener_without_grants_refuses_every_caller() {
    let _serial = CONTROL_TEST_LOCK.lock().await;
    let definition = MobDefinition::from_toml(MOB_TOML_B).expect("parse mob definition");
    let server_keys = GatewayPeerKeys::ephemeral();
    let mut runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition)
            .default_llm_client(Arc::new(TestClient::default()))
            .meerkat_config(tcp_member_comms_config())
            .control_listen("tcp://127.0.0.1:0")
            .build(),
    )
    .await
    .expect("build deny-all listener");
    runtime.set_gateway_peer_keys(server_keys.clone());

    let address = runtime
        .control_listener_advertised_address()
        .expect("listener bound");
    let entry = ContactEntry {
        mob_id: "mob-b".to_string(),
        transport: MobTransport::Tcp(
            address
                .strip_prefix("tcp://")
                .expect("tcp listener address")
                .to_string(),
        ),
        pubkey: Some(server_keys.pubkey_bytes()),
        require_signed_control: Some(true),
    };
    let caller = Arc::new(GatewayPeerKeys::ephemeral());
    let proxy = RemoteMobProxy::from_entry_with_caller(&entry, Some(caller))
        .expect("valid contact")
        .expect("tcp proxy");
    let error = proxy
        .lookup_member("bob")
        .await
        .expect_err("an omitted grant table must deny every caller");
    assert!(
        matches!(
            error,
            RemoteMobError::Rejected { ref code, .. }
                if code == "caller_not_granted"
        ),
        "unexpected deny-all result: {error:?}"
    );

    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tcp_control_round_trip_wires_and_delivers_both_ways() {
    let _serial = CONTROL_TEST_LOCK.lock().await;
    let keys_a = GatewayPeerKeys::ephemeral();
    let keys_b = GatewayPeerKeys::ephemeral();
    // B first: its bound control address feeds A's contact directory.
    let rt_b = build_runtime(
        MOB_TOML_B,
        "tcp://127.0.0.1:0",
        keys_b.clone(),
        grants_for(&keys_a, "bob"),
    )
    .await;
    let control_b = rt_b
        .control_listener_advertised_address()
        .expect("B bound a control listener");
    assert!(
        control_b.starts_with("tcp://127.0.0.1:") && !control_b.ends_with(":0"),
        "bound control address must carry the real port: {control_b}"
    );

    let mut rt_a = build_runtime(
        MOB_TOML_A,
        "tcp://127.0.0.1:0",
        keys_a,
        grants_for(&keys_b, "alice"),
    )
    .await;
    let pubkey_b = rt_b.gateway_peer_keys().expect("keys").pubkey_b64();
    let dir_a = ContactDirectory::from_toml(&format!(
        r#"
        [mobs]
        mob-b = {{ transport = "{control_b}", pubkey = "{pubkey_b}" }}
        "#,
    ))
    .expect("contact directory for mob-a");
    rt_a.set_contact_directory(dir_a);

    spawn_member(&rt_a, "alice").await;
    spawn_member(&rt_b, "bob").await;

    // Wire A -> B across the real control channel.
    Box::pin(rt_a.wire_cross_mob("alice", "bob", "mob-b"))
        .await
        .expect("wire alice <-> bob across processes");

    let alice_comms = member_comms(&rt_a, "alice").await;
    let bob_comms = member_comms(&rt_b, "bob").await;
    let bob_name = comms_name("mob-b", "bob");
    let alice_name = comms_name("mob-a", "alice");

    let bob_entry = sendable_peer(&alice_comms, &bob_name)
        .await
        .expect("alice's directory lists bob as a sendable peer");
    let alice_entry = sendable_peer(&bob_comms, &alice_name)
        .await
        .expect("bob's directory lists alice as a sendable peer (reverse wire leg)");
    // The reverse-address fix in one assertion: the descriptor the Wire
    // request installed on B must carry a dialable socket address for
    // alice, not inproc://.
    assert_eq!(
        alice_entry.address.transport().as_scheme(),
        "tcp",
        "reverse descriptor must carry alice's tcp comms address, got {}",
        alice_entry.address
    );
    assert_eq!(
        bob_entry.address.transport().as_scheme(),
        "tcp",
        "forward descriptor must carry bob's tcp comms address, got {}",
        bob_entry.address
    );

    // Control plane A->B: app-level injection into bob's session.
    let injected_session =
        Box::pin(rt_a.send_cross_mob("alice", "bob", "mob-b", "hello bob (control plane)"))
            .await
            .expect("send_cross_mob injects into bob's session");
    let bob_session = rt_b
        .mob_handle()
        .resolve_bridge_session_id(&meerkat_mob::ids::AgentIdentity::from("bob"))
        .await
        .expect("bob session");
    assert_eq!(injected_session, bob_session.to_string());

    // Agent plane A->B: alice's router dials bob's member listener over
    // real TCP; success requires an ack signed by bob's keypair. This is
    // the forward-descriptor proof (LookupMember-supplied member pubkey +
    // advertised address), exercised from the direction that was broken.
    let receipt = alice_comms
        .send(peer_message(&bob_entry, "hello bob (agent plane)"))
        .await
        .expect("alice delivers a signed envelope to bob over TCP");
    assert_acked(receipt, "alice -> bob");

    // Agent plane B->A: THE reply leg. This send only exists if the Wire
    // request carried alice's dialable address and real member pubkey.
    let receipt = bob_comms
        .send(peer_message(&alice_entry, "hello alice (reply leg)"))
        .await
        .expect("bob delivers a signed envelope back to alice over TCP");
    assert_acked(receipt, "bob -> alice (reply leg)");

    // Unwire across the control channel and verify both directories drop
    // the peering.
    Box::pin(rt_a.unwire_cross_mob("alice", "bob", "mob-b"))
        .await
        .expect("unwire alice <-> bob");
    assert!(
        sendable_peer(&alice_comms, &bob_name).await.is_none(),
        "alice must no longer list bob after unwire"
    );
    assert!(
        sendable_peer(&bob_comms, &alice_name).await.is_none(),
        "bob must no longer list alice after remote unwire"
    );

    rt_a.shutdown().await;
    rt_b.shutdown().await;
}

/// UDS variant of the control channel: same wire flow with the peer's
/// control listener on a unix socket. Member envelope transport stays TCP
/// (each member has its own listener), so this pins exactly the control
/// transport difference.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn uds_control_round_trip_wires_both_sides() {
    let _serial = CONTROL_TEST_LOCK.lock().await;
    let keys_a = GatewayPeerKeys::ephemeral();
    let keys_b = GatewayPeerKeys::ephemeral();
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("mob-b-control.sock");
    let rt_b = build_runtime(
        MOB_TOML_B,
        &format!("uds://{}", socket.display()),
        keys_b.clone(),
        grants_for(&keys_a, "bob"),
    )
    .await;
    let control_b = rt_b
        .control_listener_advertised_address()
        .expect("B bound a control listener");
    assert_eq!(control_b, format!("uds://{}", socket.display()));

    let mut rt_a = build_runtime(
        MOB_TOML_A,
        "tcp://127.0.0.1:0",
        keys_a,
        grants_for(&keys_b, "alice"),
    )
    .await;
    let pubkey_b = rt_b.gateway_peer_keys().expect("keys").pubkey_b64();
    let dir_a = ContactDirectory::from_toml(&format!(
        r#"
        [mobs]
        mob-b = {{ transport = "{control_b}", pubkey = "{pubkey_b}" }}
        "#,
    ))
    .expect("contact directory for mob-a");
    rt_a.set_contact_directory(dir_a);

    spawn_member(&rt_a, "alice").await;
    spawn_member(&rt_b, "bob").await;

    Box::pin(rt_a.wire_cross_mob("alice", "bob", "mob-b"))
        .await
        .expect("wire alice <-> bob over the UDS control channel");

    let alice_comms = member_comms(&rt_a, "alice").await;
    let bob_comms = member_comms(&rt_b, "bob").await;
    let bob_entry = sendable_peer(&alice_comms, &comms_name("mob-b", "bob"))
        .await
        .expect("alice lists bob");
    let alice_entry = sendable_peer(&bob_comms, &comms_name("mob-a", "alice"))
        .await
        .expect("bob lists alice (reverse leg over UDS control)");

    // One delivery each way proves the descriptors are live, not just rows.
    let receipt = alice_comms
        .send(peer_message(&bob_entry, "hello bob (uds-wired)"))
        .await
        .expect("alice -> bob");
    assert_acked(receipt, "alice -> bob (uds-wired)");
    let receipt = bob_comms
        .send(peer_message(&alice_entry, "hello alice (uds-wired reply)"))
        .await
        .expect("bob -> alice");
    assert_acked(receipt, "bob -> alice (uds-wired reply)");

    rt_a.shutdown().await;
    rt_b.shutdown().await;
}

const KEYED_MEMBER: &str = "personal:alice";

const MOB_TOML_IDENTITY: &str = r#"
[mob]
id = "mob-k"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
"#;

struct KeyedMemberRoster;

#[async_trait::async_trait]
impl meerkat_meta::identity_first::contracts::RosterProvider for KeyedMemberRoster {
    async fn roster(
        &self,
        _context: &meerkat_meta::identity_first::RosterContext,
    ) -> Result<
        Vec<meerkat_meta::identity_first::DurableAgentSpec>,
        meerkat_meta::identity_first::RosterError,
    > {
        Ok(vec![meerkat_meta::identity_first::DurableAgentSpec {
            identity: meerkat_meta::identity_first::AgentIdentity::parse(KEYED_MEMBER)
                .expect("parse identity"),
            profile: meerkat_mob::ProfileName::from("personal"),
            addressability: meerkat_meta::identity_first::AgentAddressability::Addressable,
            display_name: None,
            labels: std::collections::BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        }])
    }
}

/// The caller's grant for the keyed inject verbs on one member.
fn keyed_grants_for(caller: &GatewayPeerKeys, member: &str) -> ControlGrantTable {
    let mut grants = ControlGrantTable::new();
    grants.insert(
        caller.pubkey_bytes(),
        ControlGrant::new(
            "peer-gateway",
            [ControlVerb::InjectKeyed, ControlVerb::InjectStatus],
            ControlMemberScope::members([member]),
        ),
    );
    grants
}

/// A proxy to `runtime`'s control listener, pinning `server_keys` and
/// authenticating as `caller`.
fn keyed_proxy(
    runtime: &UnifiedRuntime,
    mob_id: &str,
    server_keys: &GatewayPeerKeys,
    caller: Arc<GatewayPeerKeys>,
) -> RemoteMobProxy {
    let address = runtime
        .control_listener_advertised_address()
        .expect("listener bound");
    let entry = ContactEntry {
        mob_id: mob_id.to_string(),
        transport: MobTransport::Tcp(
            address
                .strip_prefix("tcp://")
                .expect("tcp listener address")
                .to_string(),
        ),
        pubkey: Some(server_keys.pubkey_bytes()),
        require_signed_control: Some(true),
    };
    RemoteMobProxy::from_entry_with_caller(&entry, Some(caller))
        .expect("valid contact")
        .expect("tcp proxy")
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after the epoch")
            .as_millis(),
    )
    .expect("milliseconds fit")
}

/// The builder composes the keyed inject plane of the control listener it
/// binds, through the composition every gateway shares: a runtime that is not
/// identity-first answers `not_served` with its typed cause, rather than
/// `keyed_inject_unavailable` forever.
#[tokio::test(flavor = "multi_thread")]
async fn a_builder_runtime_that_is_not_identity_first_answers_keyed_inject_not_served() {
    let _serial = CONTROL_TEST_LOCK.lock().await;
    let server_keys = GatewayPeerKeys::ephemeral();
    let caller = Arc::new(GatewayPeerKeys::ephemeral());
    let runtime = build_runtime(
        MOB_TOML_B,
        "tcp://127.0.0.1:0",
        server_keys.clone(),
        keyed_grants_for(&caller, KEYED_MEMBER),
    )
    .await;
    let proxy = keyed_proxy(&runtime, "mob-b", &server_keys, caller);
    match proxy
        .inject_status(KEYED_MEMBER, "batch-1", false, now_ms())
        .await
        .expect("a signed answer")
    {
        KeyedInjectCallOutcome::Refused { code, cause, .. } => {
            assert_eq!(code, "not_served");
            assert_eq!(cause.as_deref(), Some("not_identity_first"));
        }
        other => panic!("expected not_served, got {other:?}"),
    }
    runtime.shutdown().await;
}

/// A runtime whose configuration turns keyed inject serving off answers
/// `not_served` with the cause `runtime_disabled`.
#[tokio::test(flavor = "multi_thread")]
async fn a_builder_runtime_with_keyed_inject_off_answers_runtime_disabled() {
    let _serial = CONTROL_TEST_LOCK.lock().await;
    let server_keys = GatewayPeerKeys::ephemeral();
    let caller = Arc::new(GatewayPeerKeys::ephemeral());
    let definition = MobDefinition::from_toml(MOB_TOML_B).expect("parse mob definition");
    let mut runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition)
            .default_llm_client(Arc::new(TestClient::default()))
            .meerkat_config(tcp_member_comms_config())
            .control_listen("tcp://127.0.0.1:0")
            .control_grants(keyed_grants_for(&caller, KEYED_MEMBER))
            .keyed_inject_plane(meerkat_meta::mob_announcements::KeyedInjectPlane::Disabled)
            .build(),
    )
    .await
    .expect("build runtime");
    runtime.set_gateway_peer_keys(server_keys.clone());
    let proxy = keyed_proxy(&runtime, "mob-b", &server_keys, caller);
    match proxy
        .inject_keyed(
            meerkat_meta::runtime::keyed_inject::KeyedInject {
                member: KEYED_MEMBER,
                delivery_key: "batch-1",
                content_digest: "digest",
                summary: "1 announcement",
                body: "a body",
            },
            now_ms(),
        )
        .await
        .expect("a signed answer")
    {
        KeyedInjectCallOutcome::Refused { code, cause, .. } => {
            assert_eq!(code, "not_served");
            assert_eq!(cause.as_deref(), Some("runtime_disabled"));
        }
        other => panic!("expected not_served, got {other:?}"),
    }
    runtime.shutdown().await;
}

/// An identity-first runtime the builder composes with a control listener
/// serves peers' keyed injects: a granted status read answers from
/// the runtime's own continuation owner.
#[tokio::test(flavor = "multi_thread")]
async fn a_builder_composed_identity_first_runtime_serves_keyed_inject() {
    let _serial = CONTROL_TEST_LOCK.lock().await;
    let state = tempfile::tempdir().expect("state dir");
    let server_keys = GatewayPeerKeys::ephemeral();
    let caller = Arc::new(GatewayPeerKeys::ephemeral());
    let mut runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(MobDefinition::from_toml(MOB_TOML_IDENTITY).expect("definition"))
            .persistent_state(state.path())
            .continuity_from_state_dir(state.path())
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(KeyedMemberRoster))
            .identity_bootstrap_mode(
                meerkat_meta::identity_first::IdentityBootstrapMode::LazyMaterialize,
            )
            .identity_runtime_instance_id("mob-k")
            .comms(true)
            .default_llm_client(Arc::new(TestClient::default()))
            .control_listen("tcp://127.0.0.1:0")
            .control_grants(keyed_grants_for(&caller, KEYED_MEMBER))
            .build(),
    )
    .await
    .expect("build the identity-first runtime");
    runtime.set_gateway_peer_keys(server_keys.clone());
    let proxy = keyed_proxy(&runtime, "mob-k", &server_keys, caller);
    match proxy
        .inject_status(KEYED_MEMBER, "batch-1", false, now_ms())
        .await
        .expect("a signed answer")
    {
        KeyedInjectCallOutcome::Answered(answer) => {
            assert_eq!(answer.status, NativeStatus::NotCommitted);
        }
        other => panic!("expected a served status read, got {other:?}"),
    }
    runtime.shutdown().await;
}
