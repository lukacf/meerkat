#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, LazyLock, Mutex};

use meerkat_comms::{
    AdmissionOutcome, CommsConfig, CommsRuntime, DropReason, Envelope, Inbox, InprocRegistry,
    Keypair, MessageKind, PeerMeta, PubKey, Router, SendError, Signature, TrustedPeer,
    TrustedPeers,
};
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::CommsTrustMutation;

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

fn trusted_descriptor_for(
    name: &str,
    pubkey: PubKey,
) -> meerkat_core::comms::TrustedPeerDescriptor {
    meerkat_core::comms::TrustedPeerDescriptor {
        peer_id: pubkey.to_peer_id(),
        name: meerkat_core::comms::PeerName::new(name.to_string()).expect("valid peer name"),
        address: meerkat_core::comms::PeerAddress::new(
            meerkat_core::comms::PeerTransport::Inproc,
            name,
        ),
        pubkey: *pubkey.as_bytes(),
    }
}

type TestMachineAuthority =
    Arc<Mutex<meerkat_runtime::meerkat_machine::dsl::MeerkatMachineAuthority>>;

struct TestPeerCommsAuthority {
    authority: TestMachineAuthority,
    dsl: Arc<meerkat_runtime::HandleDslAuthority>,
}

impl TestPeerCommsAuthority {
    fn install(runtime: &CommsRuntime, session_id: &str) -> Self {
        let authority = Arc::new(Mutex::new(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineAuthority::new(),
        ));
        let local_endpoint = local_endpoint_for(runtime);
        {
            let mut guard = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard
                .apply_signal(
                    meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
                )
                .expect("Initialize signal");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *guard,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                    session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(session_id),
                },
            )
            .expect("RegisterSession input");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *guard,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::PublishLocalEndpoint {
                    endpoint: local_endpoint,
                },
            )
            .expect("PublishLocalEndpoint input");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *guard,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::Prepare {
                    session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(session_id),
                    run_id: meerkat_runtime::meerkat_machine::dsl::RunId::from(format!(
                        "{session_id}-ingress-run"
                    )),
                },
            )
            .expect("Prepare input");
        }

        let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::from_shared(
            Arc::clone(&authority),
        ));
        meerkat_runtime::RuntimePeerCommsHandle::install_generated_on(Arc::clone(&dsl), runtime)
            .expect("install generated peer-comms handle");
        Self { authority, dsl }
    }

    fn reinstall(&self, runtime: &CommsRuntime) {
        meerkat_runtime::RuntimePeerCommsHandle::install_generated_on(
            Arc::clone(&self.dsl),
            runtime,
        )
        .expect("install generated peer-comms handle");
    }

    fn add_authority(
        &self,
        peer: &meerkat_core::comms::TrustedPeerDescriptor,
    ) -> meerkat_core::comms::CommsTrustMutationAuthority {
        self.try_add_authority(peer)
            .expect("generated peer projection add authority")
    }

    fn try_add_authority(
        &self,
        peer: &meerkat_core::comms::TrustedPeerDescriptor,
    ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
        let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(peer);
        let transition = {
            let mut guard = self
                .authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *guard,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
                    endpoint: endpoint.clone(),
                },
            )
            .expect("AddDirectPeerEndpoint input")
        };
        let mut obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                meerkat_runtime::protocol_comms_trust_reconcile::PeerProjectionFreshnessAuthority::from_authority(
                    Arc::clone(&self.authority),
                ),
            );
        let obligation = obligations.pop().expect("generated reconcile obligation");
        meerkat_runtime::protocol_comms_trust_reconcile::authority_for_endpoint(
            &obligation,
            &endpoint,
        )
    }
}

fn local_endpoint_for(
    runtime: &CommsRuntime,
) -> meerkat_runtime::meerkat_machine::dsl::PeerEndpoint {
    meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::new(
        "local",
        runtime.public_key().to_peer_id().to_string(),
        "inproc://local",
        *runtime.public_key().as_bytes(),
    )
}

async fn apply_generated_trust(
    runtime: &CommsRuntime,
    peer_authority: &TestPeerCommsAuthority,
    peer: meerkat_core::comms::TrustedPeerDescriptor,
) {
    peer_authority.reinstall(runtime);
    let authority = peer_authority.add_authority(&peer);
    CoreCommsRuntime::apply_trust_mutation(
        runtime,
        CommsTrustMutation::AddTrustedPeer { authority, peer },
    )
    .await
    .expect("apply generated trust");
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
        TrustedPeers::from_peers(vec![raw_zero_trusted_peer(&target_name)]),
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
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        Keypair::generate(),
        TrustedPeers::from_peers(vec![raw_zero_trusted_peer(&target_name)]),
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
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        Keypair::generate(),
        TrustedPeers::from_peers(vec![raw_zero_trusted_peer(&target_name)]),
        CommsConfig::default(),
        router_inbox_sender,
        true,
    );

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
async fn shared_trust_upsert_raw_zero_pubkey_trust_is_not_sendable() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target_name = format!("raw-zero-router-add-{suffix}");
    let mut target_inbox = register_zero_pubkey_inproc_target(registry, &target_name);
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        Keypair::generate(),
        TrustedPeers::from_peers(vec![raw_zero_trusted_peer(&target_name)]),
        CommsConfig::default(),
        router_inbox_sender,
        true,
    );

    let dest = zero_pubkey().to_peer_id();
    let result = router.send(dest, message("raw zero should not send")).await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "raw trusted-peer upsert zero-pubkey trust must not be sendable: {result:?}"
    );
    assert!(
        target_inbox.try_drain().is_empty(),
        "zero-pubkey registry target must not receive from raw trusted-peer upsert"
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
        name: runtime_name.clone(),
        inproc_namespace: None,
        identity_dir: tmp.path().join("identity"),
        trusted_peers_path: tmp.path().join("trusted_peers.json"),
        listen_uds: None,
        listen_tcp: None,
        advertise_address: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        comms_config: CommsConfig::default(),
        auth: meerkat_core::CommsAuthMode::Open,
        require_peer_auth: false,
        allow_external_unauthenticated: false,
        pairing_password: None,
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
    let _peer_authority =
        TestPeerCommsAuthority::install(&receiver, &format!("zero-ingress-receiver-{suffix}"));
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
        .send_connection_ingress(envelope, true);
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
        TrustedPeers::from_peers(vec![
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
        ]),
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
        TrustedPeers::from_peers(vec![
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
        ]),
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

    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        Keypair::generate(),
        TrustedPeers::from_peers(vec![
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
        ]),
        CommsConfig::default(),
        router_inbox_sender,
        false,
    );

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
async fn router_without_trust_after_duplicate_removal_does_not_route() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let target_name = format!("remove-duplicate-target-{suffix}");
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
        TrustedPeers::new(),
        CommsConfig::default(),
        router_inbox_sender,
        true,
    );

    let dest = target_pubkey.to_peer_id();
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
    let primary_authority = TestPeerCommsAuthority::install(
        &runtime,
        &format!("duplicate-directory-primary-authority-{suffix}"),
    );
    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let peer_id = target_pubkey.to_peer_id();

    apply_generated_trust(
        &runtime,
        &primary_authority,
        trusted_descriptor_for(
            &format!("duplicate-directory-primary-{suffix}"),
            target_pubkey,
        ),
    )
    .await;
    let duplicate_error = primary_authority
        .try_add_authority(&trusted_descriptor_for(
            &format!("duplicate-directory-shadow-{suffix}"),
            target_pubkey,
        ))
        .expect_err("generated peer projection must reject ambiguous duplicate descriptors");
    assert!(
        duplicate_error.contains("ambiguous endpoint descriptors"),
        "unexpected duplicate descriptor error: {duplicate_error}"
    );
    assert_eq!(
        runtime.trusted_peers_shared().peers().len(),
        1,
        "generated trust projection must not add duplicates after machine rejection"
    );

    let peers = CoreCommsRuntime::peers(&runtime).await;

    assert_eq!(
        peers.iter().filter(|peer| peer.peer_id == peer_id).count(),
        1,
        "generated trust projection must expose at most one canonical peer identity: {peers:?}"
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
        name: runtime_name.clone(),
        inproc_namespace: None,
        identity_dir: tmp.path().join("identity"),
        trusted_peers_path: tmp.path().join("trusted_peers.json"),
        listen_uds: None,
        listen_tcp: None,
        advertise_address: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        comms_config: CommsConfig::default(),
        auth: meerkat_core::CommsAuthMode::Open,
        require_peer_auth: false,
        allow_external_unauthenticated: false,
        pairing_password: None,
    })
    .await
    .expect("auth-disabled runtime");
    let stale_authority =
        TestPeerCommsAuthority::install(&runtime, &format!("{runtime_name}-stale-authority"));
    let target_keypair = Keypair::generate();
    let target_pubkey = target_keypair.public_key();
    let peer_id = target_pubkey.to_peer_id();
    let (_target_inbox, target_sender) = Inbox::new();

    apply_generated_trust(
        &runtime,
        &stale_authority,
        trusted_descriptor_for(&stale_name, target_pubkey),
    )
    .await;
    let duplicate_error = stale_authority
        .try_add_authority(&trusted_descriptor_for(&target_name, target_pubkey))
        .expect_err("generated peer projection must reject ambiguous duplicate descriptors");
    assert!(
        duplicate_error.contains("ambiguous endpoint descriptors"),
        "unexpected duplicate descriptor error: {duplicate_error}"
    );
    assert_eq!(
        runtime.trusted_peers_shared().peers().len(),
        1,
        "generated trust projection must not add duplicates after machine rejection"
    );
    registry.register_with_meta_in_namespace(
        "",
        &target_name,
        target_pubkey,
        target_sender,
        PeerMeta::default(),
    );

    let peers = CoreCommsRuntime::peers(&runtime).await;

    assert_eq!(
        peers.iter().filter(|peer| peer.peer_id == peer_id).count(),
        1,
        "generated trust projection must not re-advertise duplicate trusted identities: {peers:?}"
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
        advertise_address: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        comms_config: CommsConfig::default(),
        auth: meerkat_core::CommsAuthMode::Open,
        require_peer_auth: false,
        allow_external_unauthenticated: false,
        pairing_password: None,
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
async fn router_new_raw_trust_does_not_create_canonical_inproc_route() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let target_runtime = CommsRuntime::inproc_only("canonical-target").expect("target runtime");
    let peer_authority = TestPeerCommsAuthority::install(&target_runtime, "canonical-target");
    let target_pubkey = target_runtime.public_key();
    let shadow_keypair = Keypair::generate();
    let shadow_pubkey = shadow_keypair.public_key();

    let (mut shadow_inbox, shadow_sender) = Inbox::new();

    registry.register_with_meta_in_namespace(
        "",
        "shared-display-name",
        shadow_pubkey,
        shadow_sender,
        PeerMeta::default(),
    );

    let sender_keypair = Keypair::generate();
    let sender_pubkey = sender_keypair.public_key();
    apply_generated_trust(
        &target_runtime,
        &peer_authority,
        trusted_descriptor_for("sender", sender_pubkey),
    )
    .await;
    let trusted_peers = TrustedPeers::from_peers(vec![TrustedPeer {
        name: "shared-display-name".to_string(),
        pubkey: target_pubkey,
        addr: "inproc://canonical-target".to_string(),
        meta: PeerMeta::default(),
    }]);
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        sender_keypair,
        trusted_peers,
        CommsConfig::default(),
        router_inbox_sender,
        true,
    );

    let target_peer_id = meerkat_comms::router::peer_id_from_pubkey(&target_pubkey);
    let result = router
        .send(target_peer_id, message("raw trust should not route"))
        .await;
    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == target_peer_id),
        "raw Router::new trust must not become send authority: {result:?}"
    );

    let target_items = CoreCommsRuntime::drain_inbox_interactions(&target_runtime).await;
    let shadow_items = shadow_inbox.try_drain();

    assert_eq!(
        shadow_items.len(),
        0,
        "display-name shadow must not receive"
    );
    assert!(
        target_items.is_empty(),
        "raw Router::new trust must not deliver to canonical target"
    );

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
    let trusted_peers = TrustedPeers::from_peers(vec![TrustedPeer {
        name: "local-target".to_string(),
        pubkey: target_pubkey,
        addr: "inproc://local-target".to_string(),
        meta: PeerMeta::default(),
    }]);
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
    let trusted_peers = TrustedPeers::from_peers(vec![TrustedPeer {
        name: "alpha-target".to_string(),
        pubkey: target_pubkey,
        addr: "inproc://alpha-target".to_string(),
        meta: PeerMeta::default(),
    }]);
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
