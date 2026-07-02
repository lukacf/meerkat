#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, LazyLock, Mutex};

use meerkat_comms::{
    AdmissionOutcome, CommsConfig, CommsRuntime, DropReason, Envelope, Inbox, InprocRegistry,
    Keypair, MessageKind, PeerMeta, PubKey, Router, SendError, Signature,
};
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::CommsTrustMutation;

static INPROC_REGISTRY_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

fn message(body: &str) -> MessageKind {
    MessageKind::Message {
        content_taint: None,
        body: body.to_string(),
        blocks: None,
        handling_mode: None,
    }
}

fn zero_pubkey() -> PubKey {
    PubKey::new([0u8; 32])
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
async fn zero_pubkey_identity_is_never_sendable() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target_name = format!("raw-zero-router-{suffix}");
    let _target_inbox = register_zero_pubkey_inproc_target(registry, &target_name);
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        Keypair::generate(),
        CommsConfig::default(),
        router_inbox_sender,
        true,
    );

    let dest = zero_pubkey().to_peer_id();
    let result = router.send(dest, message("raw zero should not send")).await;

    assert!(
        matches!(result, Err(SendError::PeerNotFound(peer_id)) if peer_id == dest),
        "zero-pubkey identity must never be sendable: {result:?}"
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
    let _target_inbox = register_zero_pubkey_inproc_target(registry, &target_name);
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        Keypair::generate(),
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
        runtime.trusted_peers_shared().len(),
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
        runtime.trusted_peers_shared().len(),
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
async fn untrusted_sender_has_no_canonical_inproc_route() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    let registry = InprocRegistry::global();
    registry.clear();

    let target_runtime = CommsRuntime::inproc_only("canonical-target").expect("target runtime");
    let peer_authority = TestPeerCommsAuthority::install(&target_runtime, "canonical-target");
    let target_pubkey = target_runtime.public_key();
    let shadow_keypair = Keypair::generate();
    let shadow_pubkey = shadow_keypair.public_key();

    let (_shadow_inbox, shadow_sender) = Inbox::new();

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
    let (_, router_inbox_sender) = Inbox::new();
    let router = Router::new(
        sender_keypair,
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
        "only generated trust mutations create send authority: {result:?}"
    );

    let target_items = CoreCommsRuntime::drain_inbox_interactions(&target_runtime).await;
    assert!(
        target_items.is_empty(),
        "an untrusted sender must not deliver to the canonical target"
    );

    registry.clear();
}
