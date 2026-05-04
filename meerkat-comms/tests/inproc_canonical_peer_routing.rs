#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, LazyLock};

use meerkat_comms::{
    AdmissionOutcome, CommsConfig, CommsRuntime, DropReason, Envelope, Inbox, InboxItem,
    InprocRegistry, Keypair, MessageKind, PeerMeta, PubKey, Router, SendError, Signature,
    TrustedPeer, TrustedPeers,
};
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;

static INPROC_REGISTRY_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

fn message(body: &str) -> MessageKind {
    MessageKind::Message {
        body: body.to_string(),
        blocks: None,
        handling_mode: None,
    }
}

fn zero_pubkey() -> PubKey {
    PubKey::new([0u8; 32])
}

fn raw_zero_trusted_peer(name: &str) -> TrustedPeer {
    TrustedPeer {
        name: name.to_string(),
        pubkey: zero_pubkey(),
        addr: format!("inproc://{name}"),
        meta: PeerMeta::default(),
    }
}

fn register_zero_pubkey_inproc_target(registry: &InprocRegistry, name: &str) -> Inbox {
    let (inbox, sender) = Inbox::new();
    registry.register_with_meta_in_namespace("", name, zero_pubkey(), sender, PeerMeta::default());
    inbox
}

#[tokio::test]
async fn router_new_raw_zero_pubkey_trust_is_not_sendable() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target_name = format!("raw-zero-router-new-{suffix}");
    let mut target_inbox = register_zero_pubkey_inproc_target(registry, &target_name);
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        Keypair::generate(),
        TrustedPeers {
            peers: vec![raw_zero_trusted_peer(&target_name)],
        },
        CommsConfig::default(),
        router_inbox_sender,
        true,
    );

    let dest = zero_pubkey().to_peer_id();
    let result = router.send(dest, message("raw zero should not send")).await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "raw Router::new zero-pubkey trust must not be sendable: {result:?}"
    );
    assert!(
        target_inbox.try_drain().is_empty(),
        "zero-pubkey registry target must not receive from raw Router::new trust"
    );

    registry.clear();
}

#[tokio::test]
async fn router_with_shared_peers_raw_zero_pubkey_trust_is_not_sendable() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target_name = format!("raw-zero-router-shared-{suffix}");
    let mut target_inbox = register_zero_pubkey_inproc_target(registry, &target_name);
    let trusted_peers = Arc::new(parking_lot::RwLock::new(TrustedPeers {
        peers: vec![raw_zero_trusted_peer(&target_name)],
    }));
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::with_shared_peers(
        Keypair::generate(),
        trusted_peers,
        CommsConfig::default(),
        router_inbox_sender,
        true,
    );

    let dest = zero_pubkey().to_peer_id();
    let result = router.send(dest, message("raw zero should not send")).await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "raw Router::with_shared_peers zero-pubkey trust must not be sendable: {result:?}"
    );
    assert!(
        target_inbox.try_drain().is_empty(),
        "zero-pubkey registry target must not receive from raw shared trust"
    );

    registry.clear();
}

#[tokio::test]
async fn router_send_filters_late_shared_raw_zero_pubkey_trust_mutation() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target_name = format!("raw-zero-router-late-shared-{suffix}");
    let mut target_inbox = register_zero_pubkey_inproc_target(registry, &target_name);
    let trusted_peers = Arc::new(parking_lot::RwLock::new(TrustedPeers::new()));
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::with_shared_peers(
        Keypair::generate(),
        trusted_peers.clone(),
        CommsConfig::default(),
        router_inbox_sender,
        true,
    );
    trusted_peers
        .write()
        .peers
        .push(raw_zero_trusted_peer(&target_name));

    let dest = zero_pubkey().to_peer_id();
    let result = router
        .send(dest, message("late raw zero should not send"))
        .await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "late shared zero-pubkey trust mutation must not become sendable: {result:?}"
    );
    assert!(
        target_inbox.try_drain().is_empty(),
        "zero-pubkey registry target must not receive from late shared trust mutation"
    );

    registry.clear();
}

#[tokio::test]
async fn router_add_trusted_peer_raw_zero_pubkey_trust_is_not_sendable() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target_name = format!("raw-zero-router-add-{suffix}");
    let mut target_inbox = register_zero_pubkey_inproc_target(registry, &target_name);
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        Keypair::generate(),
        TrustedPeers::new(),
        CommsConfig::default(),
        router_inbox_sender,
        true,
    );
    assert!(
        router
            .add_trusted_peer(raw_zero_trusted_peer(&target_name))
            .is_err(),
        "raw Router::add_trusted_peer zero-pubkey trust must be rejected"
    );

    let dest = zero_pubkey().to_peer_id();
    let result = router.send(dest, message("raw zero should not send")).await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "raw Router::add_trusted_peer zero-pubkey trust must not be sendable: {result:?}"
    );
    assert!(
        target_inbox.try_drain().is_empty(),
        "zero-pubkey registry target must not receive from raw add_trusted_peer trust"
    );

    registry.clear();
}

#[tokio::test]
async fn router_auth_disabled_inproc_fallback_rejects_zero_pubkey_registry_identity() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target_name = format!("auth-disabled-zero-fallback-{suffix}");
    let mut target_inbox = register_zero_pubkey_inproc_target(registry, &target_name);
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        Keypair::generate(),
        TrustedPeers::new(),
        CommsConfig::default(),
        router_inbox_sender,
        false,
    );

    let dest = zero_pubkey().to_peer_id();
    let result = router
        .send(
            dest,
            message("auth-disabled fallback must not trust zero pubkeys"),
        )
        .await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "auth-disabled inproc fallback must not synthesize a zero-pubkey sendable peer: {result:?}"
    );
    assert!(
        target_inbox.try_drain().is_empty(),
        "zero-pubkey registry target must not receive through auth-disabled fallback"
    );

    registry.clear();
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn runtime_auth_disabled_directory_skips_zero_pubkey_inproc_identity() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let runtime_name = format!("auth-disabled-directory-sender-{suffix}");
    let zero_name = format!("auth-disabled-directory-zero-{suffix}");
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let runtime = CommsRuntime::new(meerkat_comms::ResolvedCommsConfig {
        enabled: true,
        name: runtime_name,
        inproc_namespace: None,
        identity_dir: tmp.path().join("identity"),
        trusted_peers_path: tmp.path().join("trusted_peers.json"),
        listen_uds: None,
        listen_tcp: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        comms_config: CommsConfig::default(),
        auth: meerkat_core::CommsAuthMode::Open,
        require_peer_auth: false,
        allow_external_unauthenticated: false,
    })
    .await
    .expect("auth-disabled runtime");

    let (_zero_inbox, zero_sender) = Inbox::new();
    registry.register_with_meta_in_namespace(
        "",
        &zero_name,
        zero_pubkey(),
        zero_sender,
        PeerMeta::default(),
    );

    let peers = CoreCommsRuntime::peers(&runtime).await;
    assert!(
        peers
            .iter()
            .all(|peer| peer.peer_id != zero_pubkey().to_peer_id()),
        "auth-disabled peer directory must not advertise zero-pubkey inproc identities: {peers:?}"
    );

    registry.clear();
}

#[tokio::test]
async fn ingress_rejects_late_shared_zero_pubkey_trust_mutation() {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let receiver =
        CommsRuntime::inproc_only(&format!("zero-ingress-receiver-{suffix}")).expect("runtime");
    let trusted_peers = receiver.router_arc().shared_trusted_peers();
    trusted_peers
        .write()
        .peers
        .push(raw_zero_trusted_peer(&format!(
            "zero-ingress-sender-{suffix}"
        )));

    let envelope = Envelope {
        id: uuid::Uuid::new_v4(),
        from: zero_pubkey(),
        to: receiver.public_key(),
        kind: message("zero pubkey ingress must not admit"),
        sig: Signature::new([0u8; 64]),
    };

    let outcome = receiver
        .router_arc()
        .inbox_sender()
        .send_connection_ingress(envelope, true, &trusted_peers);
    assert_eq!(
        outcome,
        AdmissionOutcome::Dropped {
            reason: DropReason::UntrustedSender
        },
        "late shared zero-pubkey trust mutation must not admit ingress"
    );

    let drained = CoreCommsRuntime::drain_messages(&receiver).await;
    assert!(
        drained.is_empty(),
        "zero-pubkey ingress must not reach runtime messages: {drained:?}"
    );
}

#[tokio::test]
async fn router_rejects_duplicate_trusted_canonical_peer_identity() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let target_name = format!("duplicate-canonical-target-{suffix}");
    let stale_name = format!("duplicate-canonical-stale-{suffix}");
    let (mut target_inbox, target_sender) = Inbox::new();

    registry.register_with_meta_in_namespace(
        "",
        &target_name,
        target_pubkey,
        target_sender,
        PeerMeta::default(),
    );

    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        Keypair::generate(),
        TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: stale_name.clone(),
                    pubkey: target_pubkey,
                    addr: format!("inproc://{stale_name}"),
                    meta: PeerMeta::default(),
                },
                TrustedPeer {
                    name: target_name.clone(),
                    pubkey: target_pubkey,
                    addr: format!("inproc://{target_name}"),
                    meta: PeerMeta::default(),
                },
            ],
        },
        CommsConfig::default(),
        router_inbox_sender,
        true,
    );

    let dest = target_pubkey.to_peer_id();
    let result = router
        .send(dest, message("duplicate canonical trust must not route"))
        .await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "duplicate trusted canonical identity must fail closed: {result:?}"
    );
    assert!(
        target_inbox.try_drain().is_empty(),
        "duplicate trusted canonical identity must not deliver by whichever entry wins"
    );

    registry.clear();
}

#[tokio::test]
async fn router_auth_disabled_fallback_rejects_duplicate_trusted_canonical_peer_identity() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let target_name = format!("auth-disabled-duplicate-target-{suffix}");
    let stale_name = format!("auth-disabled-duplicate-stale-{suffix}");
    let (mut target_inbox, target_sender) = Inbox::new();

    registry.register_with_meta_in_namespace(
        "",
        &target_name,
        target_pubkey,
        target_sender,
        PeerMeta::default(),
    );

    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        Keypair::generate(),
        TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: stale_name.clone(),
                    pubkey: target_pubkey,
                    addr: format!("inproc://{stale_name}"),
                    meta: PeerMeta::default(),
                },
                TrustedPeer {
                    name: target_name.clone(),
                    pubkey: target_pubkey,
                    addr: format!("inproc://{target_name}"),
                    meta: PeerMeta::default(),
                },
            ],
        },
        CommsConfig::default(),
        router_inbox_sender,
        false,
    );

    let dest = target_pubkey.to_peer_id();
    let result = router
        .send(
            dest,
            message("auth-disabled duplicate canonical trust must not fallback"),
        )
        .await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "auth-disabled duplicate trusted canonical identity must fail closed before fallback: {result:?}"
    );
    assert!(
        target_inbox.try_drain().is_empty(),
        "auth-disabled fallback must not deliver to an ambiguous trusted canonical identity"
    );

    registry.clear();
}

#[tokio::test]
async fn router_auth_disabled_fallback_rejects_late_shared_duplicate_trust() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let target_name = format!("late-auth-disabled-duplicate-target-{suffix}");
    let stale_name = format!("late-auth-disabled-duplicate-stale-{suffix}");
    let (mut target_inbox, target_sender) = Inbox::new();

    registry.register_with_meta_in_namespace(
        "",
        &target_name,
        target_pubkey,
        target_sender,
        PeerMeta::default(),
    );

    let trusted_peers = Arc::new(parking_lot::RwLock::new(TrustedPeers::new()));
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::with_shared_peers(
        Keypair::generate(),
        trusted_peers.clone(),
        CommsConfig::default(),
        router_inbox_sender,
        false,
    );
    trusted_peers.write().peers.extend([
        TrustedPeer {
            name: stale_name.clone(),
            pubkey: target_pubkey,
            addr: format!("inproc://{stale_name}"),
            meta: PeerMeta::default(),
        },
        TrustedPeer {
            name: target_name.clone(),
            pubkey: target_pubkey,
            addr: format!("inproc://{target_name}"),
            meta: PeerMeta::default(),
        },
    ]);

    let dest = target_pubkey.to_peer_id();
    let result = router
        .send(
            dest,
            message("late shared duplicate canonical trust must not fallback"),
        )
        .await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "late shared duplicate trusted canonical identity must fail closed before fallback: {result:?}"
    );
    assert!(
        target_inbox.try_drain().is_empty(),
        "auth-disabled fallback must not deliver after late shared duplicate trust mutation"
    );

    registry.clear();
}

#[tokio::test]
async fn router_remove_trusted_peer_removes_all_duplicate_canonical_matches() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let target_name = format!("remove-duplicate-target-{suffix}");
    let shadow_name = format!("remove-duplicate-shadow-{suffix}");
    let (mut target_inbox, target_sender) = Inbox::new();

    registry.register_with_meta_in_namespace(
        "",
        &target_name,
        target_pubkey,
        target_sender,
        PeerMeta::default(),
    );

    let trusted_peers = Arc::new(parking_lot::RwLock::new(TrustedPeers::new()));
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::with_shared_peers(
        Keypair::generate(),
        trusted_peers.clone(),
        CommsConfig::default(),
        router_inbox_sender,
        true,
    );
    trusted_peers.write().peers.extend([
        TrustedPeer {
            name: shadow_name.clone(),
            pubkey: target_pubkey,
            addr: format!("inproc://{shadow_name}"),
            meta: PeerMeta::default(),
        },
        TrustedPeer {
            name: target_name.clone(),
            pubkey: target_pubkey,
            addr: format!("inproc://{target_name}"),
            meta: PeerMeta::default(),
        },
    ]);

    let dest = target_pubkey.to_peer_id();
    assert!(
        router.remove_trusted_peer(&dest),
        "remove should report that duplicate canonical trust entries were removed"
    );
    assert!(
        trusted_peers
            .read()
            .peers
            .iter()
            .all(|peer| peer.pubkey != target_pubkey),
        "remove must clear every raw entry for the canonical peer id"
    );

    let result = router
        .send(dest, message("removed duplicate trust must not route"))
        .await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "removing duplicate trust must not leave one duplicate sendable: {result:?}"
    );
    assert!(
        target_inbox.try_drain().is_empty(),
        "removed duplicate canonical identity must not deliver through the remaining entry"
    );

    registry.clear();
}

#[tokio::test]
async fn runtime_peer_directory_skips_duplicate_trusted_canonical_peer_identity() {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let runtime = CommsRuntime::inproc_only(&format!("duplicate-directory-runtime-{suffix}"))
        .expect("runtime");
    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let peer_id = target_pubkey.to_peer_id();

    runtime.trusted_peers_shared().write().peers.extend([
        TrustedPeer {
            name: format!("duplicate-directory-primary-{suffix}"),
            pubkey: target_pubkey,
            addr: format!("inproc://duplicate-directory-primary-{suffix}"),
            meta: PeerMeta::default(),
        },
        TrustedPeer {
            name: format!("duplicate-directory-shadow-{suffix}"),
            pubkey: target_pubkey,
            addr: format!("inproc://duplicate-directory-shadow-{suffix}"),
            meta: PeerMeta::default(),
        },
    ]);

    let peers = CoreCommsRuntime::peers(&runtime).await;

    assert!(
        peers.iter().all(|peer| peer.peer_id != peer_id),
        "duplicate trusted canonical identities must not be advertised: {peers:?}"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn runtime_auth_disabled_directory_suppresses_inproc_for_duplicate_trust() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let runtime_name = format!("auth-disabled-duplicate-directory-runtime-{suffix}");
    let target_name = format!("auth-disabled-duplicate-directory-target-{suffix}");
    let stale_name = format!("auth-disabled-duplicate-directory-stale-{suffix}");
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let runtime = CommsRuntime::new(meerkat_comms::ResolvedCommsConfig {
        enabled: true,
        name: runtime_name,
        inproc_namespace: None,
        identity_dir: tmp.path().join("identity"),
        trusted_peers_path: tmp.path().join("trusted_peers.json"),
        listen_uds: None,
        listen_tcp: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        comms_config: CommsConfig::default(),
        auth: meerkat_core::CommsAuthMode::Open,
        require_peer_auth: false,
        allow_external_unauthenticated: false,
    })
    .await
    .expect("auth-disabled runtime");
    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let peer_id = target_pubkey.to_peer_id();
    let (_target_inbox, target_sender) = Inbox::new();

    runtime.trusted_peers_shared().write().peers.extend([
        TrustedPeer {
            name: stale_name.clone(),
            pubkey: target_pubkey,
            addr: format!("inproc://{stale_name}"),
            meta: PeerMeta::default(),
        },
        TrustedPeer {
            name: target_name.clone(),
            pubkey: target_pubkey,
            addr: format!("inproc://{target_name}"),
            meta: PeerMeta::default(),
        },
    ]);
    registry.register_with_meta_in_namespace(
        "",
        &target_name,
        target_pubkey,
        target_sender,
        PeerMeta::default(),
    );

    let peers = CoreCommsRuntime::peers(&runtime).await;

    assert!(
        peers.iter().all(|peer| peer.peer_id != peer_id),
        "auth-disabled peer directory must not re-advertise ambiguous trusted identities as inproc: {peers:?}"
    );

    registry.clear();
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn runtime_auth_disabled_directory_suppresses_inproc_for_duplicate_live_canonical_identity() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let runtime_name = format!("auth-disabled-live-duplicate-directory-runtime-{suffix}");
    let local_name = format!("auth-disabled-live-duplicate-local-{suffix}");
    let remote_name = format!("auth-disabled-live-duplicate-remote-{suffix}");
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let runtime = CommsRuntime::new(meerkat_comms::ResolvedCommsConfig {
        enabled: true,
        name: runtime_name,
        inproc_namespace: Some("realm-local".to_string()),
        identity_dir: tmp.path().join("identity"),
        trusted_peers_path: tmp.path().join("trusted_peers.json"),
        listen_uds: None,
        listen_tcp: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        comms_config: CommsConfig::default(),
        auth: meerkat_core::CommsAuthMode::Open,
        require_peer_auth: false,
        allow_external_unauthenticated: false,
    })
    .await
    .expect("auth-disabled runtime");
    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let peer_id = target_pubkey.to_peer_id();
    let (_local_inbox, local_sender) = Inbox::new();
    let (_remote_inbox, remote_sender) = Inbox::new();

    registry.register_with_meta_in_namespace(
        "realm-local",
        &local_name,
        target_pubkey,
        local_sender,
        PeerMeta::default(),
    );
    registry.register_with_meta_in_namespace(
        "realm-remote",
        &remote_name,
        target_pubkey,
        remote_sender,
        PeerMeta::default(),
    );

    let peers = CoreCommsRuntime::peers(&runtime).await;

    assert!(
        peers.iter().all(|peer| peer.peer_id != peer_id),
        "auth-disabled peer directory must not advertise duplicate live canonical inproc identities: {peers:?}"
    );

    registry.clear();
}

#[tokio::test]
async fn router_inproc_same_namespace_delivers_to_resolved_peer_identity_not_display_name() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let shadow_keypair = Keypair::generate();
    let shadow_pubkey = shadow_keypair.public_key();

    let (mut target_inbox, target_sender) = Inbox::new();
    let (mut shadow_inbox, shadow_sender) = Inbox::new();

    registry.register_with_meta_in_namespace(
        "",
        "canonical-target",
        target_pubkey,
        target_sender,
        PeerMeta::default(),
    );
    registry.register_with_meta_in_namespace(
        "",
        "shared-display-name",
        shadow_pubkey,
        shadow_sender,
        PeerMeta::default(),
    );

    let sender_keypair = Keypair::generate();
    let sender_pubkey = sender_keypair.public_key();
    let trusted_peers = TrustedPeers {
        peers: vec![TrustedPeer {
            name: "shared-display-name".to_string(),
            pubkey: target_pubkey,
            addr: "inproc://shared-display-name".to_string(),
            meta: PeerMeta::default(),
        }],
    };
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        sender_keypair,
        trusted_peers,
        CommsConfig::default(),
        router_inbox_sender,
        true,
    );

    router
        .send(
            meerkat_comms::router::peer_id_from_pubkey(&target_pubkey),
            message("canonical route"),
        )
        .await
        .expect("send should route to the trusted peer identity");

    let target_items = target_inbox.try_drain();
    let shadow_items = shadow_inbox.try_drain();

    assert_eq!(
        shadow_items.len(),
        0,
        "display-name shadow must not receive"
    );
    assert_eq!(target_items.len(), 1, "canonical target should receive");

    let InboxItem::External { envelope } = &target_items[0] else {
        panic!("expected external envelope");
    };
    assert_eq!(envelope.from, sender_pubkey);
    assert_eq!(envelope.to, target_pubkey);
    assert!(envelope.verify(), "signed envelope should verify");

    registry.clear();
}

#[tokio::test]
async fn router_inproc_same_namespace_rejects_duplicate_live_canonical_peer_identity() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let (mut local_inbox, local_sender) = Inbox::new();
    let (mut remote_inbox, remote_sender) = Inbox::new();

    registry.register_with_meta_in_namespace(
        "realm-local",
        "local-target",
        target_pubkey,
        local_sender,
        PeerMeta::default(),
    );
    registry.register_with_meta_in_namespace(
        "realm-remote",
        "remote-target",
        target_pubkey,
        remote_sender,
        PeerMeta::default(),
    );

    let sender_keypair = Keypair::generate();
    let trusted_peers = TrustedPeers {
        peers: vec![TrustedPeer {
            name: "local-target".to_string(),
            pubkey: target_pubkey,
            addr: "inproc://local-target".to_string(),
            meta: PeerMeta::default(),
        }],
    };
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        sender_keypair,
        trusted_peers,
        CommsConfig::default(),
        router_inbox_sender,
        true,
    )
    .with_inproc_namespace(Some("realm-local".to_string()));

    let dest = meerkat_comms::router::peer_id_from_pubkey(&target_pubkey);
    let result = router
        .send(dest, message("same-namespace duplicate canonical route"))
        .await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "same-namespace inproc delivery must fail closed when the canonical identity is live in another namespace: {result:?}"
    );
    assert!(
        local_inbox.try_drain().is_empty(),
        "must not deliver to the namespace-local registration before checking canonical ambiguity"
    );
    assert!(
        remote_inbox.try_drain().is_empty(),
        "must not choose the duplicate registration in another namespace"
    );

    registry.clear();
}

#[tokio::test]
async fn router_inproc_cross_namespace_rejects_ambiguous_peer_identity() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let (mut alpha_inbox, alpha_sender) = Inbox::new();
    let (mut beta_inbox, beta_sender) = Inbox::new();

    registry.register_with_meta_in_namespace(
        "realm-alpha",
        "alpha-target",
        target_pubkey,
        alpha_sender,
        PeerMeta::default(),
    );
    registry.register_with_meta_in_namespace(
        "realm-beta",
        "beta-target",
        target_pubkey,
        beta_sender,
        PeerMeta::default(),
    );

    let sender_keypair = Keypair::generate();
    let trusted_peers = TrustedPeers {
        peers: vec![TrustedPeer {
            name: "alpha-target".to_string(),
            pubkey: target_pubkey,
            addr: "inproc://alpha-target".to_string(),
            meta: PeerMeta::default(),
        }],
    };
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        sender_keypair,
        trusted_peers,
        CommsConfig::default(),
        router_inbox_sender,
        true,
    )
    .with_inproc_namespace(Some("realm-sender".to_string()));

    let dest = meerkat_comms::router::peer_id_from_pubkey(&target_pubkey);
    let result = router
        .send(dest, message("ambiguous cross-namespace route"))
        .await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "ambiguous cross-namespace pubkey registrations must fail closed: {result:?}"
    );
    assert!(
        alpha_inbox.try_drain().is_empty(),
        "must not fall back to display-name scoped delivery"
    );
    assert!(
        beta_inbox.try_drain().is_empty(),
        "must not pick an arbitrary namespace for a reused identity"
    );

    registry.clear();
}
