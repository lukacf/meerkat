use std::sync::{Arc, LazyLock};

use meerkat_comms::{
    CommsConfig, Inbox, InboxItem, InprocRegistry, Keypair, MessageKind, PeerMeta, Router,
    PubKey, SendError, TrustedPeer, TrustedPeers,
};

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
