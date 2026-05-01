use std::sync::LazyLock;

use meerkat_comms::{
    CommsConfig, Inbox, InboxItem, InprocRegistry, Keypair, MessageKind, PeerMeta, Router,
    TrustedPeer, TrustedPeers,
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
