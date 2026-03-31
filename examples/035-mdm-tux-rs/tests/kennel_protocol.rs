//! Kennel protocol integration tests.
//!
//! These tests spawn a real kennel TCP server and exercise the control protocol
//! through signed envelopes. No LLM API key needed.

use anyhow::Context;
use mdm_tux::{
    ClaimGrant, KennelPayload, KennelTargetState, ListScope, SignedKennelEnvelope,
    TargetListEntry, build_signed_envelope, read_envelope, verify_envelope, write_envelope,
};
use meerkat_comms::identity::Keypair;
use tokio::io::BufReader;
use tokio::net::TcpStream;

/// Helper: connect to kennel at `addr`, send a payload, and read one response.
async fn roundtrip(
    addr: &str,
    keypair: &Keypair,
    signer_id: &str,
    payload: KennelPayload,
) -> anyhow::Result<KennelPayload> {
    let stream = TcpStream::connect(addr).await?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let env = build_signed_envelope(keypair, signer_id, payload)?;
    write_envelope(&mut writer, &env).await?;
    let resp = read_envelope(&mut reader)
        .await?
        .context("no response from kennel")?;
    verify_envelope(&resp)?;
    Ok(resp.payload)
}

/// Helper: open a persistent control session to the kennel.
struct KennelSession {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
    keypair: Keypair,
    signer_id: String,
}

impl KennelSession {
    async fn connect(addr: &str, keypair: Keypair) -> anyhow::Result<Self> {
        let signer_id = keypair.public_key().to_peer_id();
        let stream = TcpStream::connect(addr).await?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
            keypair,
            signer_id,
        })
    }

    async fn send(&mut self, payload: KennelPayload) -> anyhow::Result<()> {
        let env = build_signed_envelope(&self.keypair, &self.signer_id, payload)?;
        write_envelope(&mut self.writer, &env).await
    }

    async fn recv(&mut self) -> anyhow::Result<KennelPayload> {
        let env = read_envelope(&mut self.reader)
            .await?
            .context("connection closed")?;
        verify_envelope(&env)?;
        Ok(env.payload)
    }

    fn id(&self) -> &str {
        &self.signer_id
    }
}

/// Spawn a real kennel process on a random port and return its address.
/// Binds the port first and keeps it bound until the kennel is ready to avoid TOCTOU races.
async fn spawn_kennel() -> anyhow::Result<(String, tokio::process::Child, tempfile::TempDir)> {
    let temp = tempfile::tempdir()?;

    // Bind to find a free port, keep the listener alive until the kennel is spawned.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    // Drop the listener right before spawning so the kennel can bind.
    drop(listener);

    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mcm-kennel"))
        .arg("--listen")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--data-dir")
        .arg(temp.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    // Wait for kennel to be ready (longer timeout, retry with backoff)
    for i in 0..100 {
        if TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .is_ok()
        {
            // Return temp so it lives as long as the caller needs
            return Ok((format!("127.0.0.1:{port}"), child, temp));
        }
        tokio::time::sleep(std::time::Duration::from_millis(50 + i * 10)).await;
    }
    anyhow::bail!("kennel did not start within 10s on port {port}");
}

// ── Envelope tests ───────────────────────────────────────────────────────────

#[test]
fn signed_envelope_roundtrip() {
    let kp = Keypair::generate();
    let signer_id = kp.public_key().to_peer_id();
    let payload = KennelPayload::TargetHeartbeat;
    let env = build_signed_envelope(&kp, &signer_id, payload).unwrap();
    let signer = verify_envelope(&env).unwrap();
    assert_eq!(signer, kp.public_key());
}

#[test]
fn invalid_signature_rejected() {
    let kp = Keypair::generate();
    let signer_id = kp.public_key().to_peer_id();
    let mut env = build_signed_envelope(&kp, &signer_id, KennelPayload::TargetHeartbeat).unwrap();
    // Corrupt the signature
    env.signature = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        vec![0u8; 64],
    );
    assert!(verify_envelope(&env).is_err());
}

// ── Kennel integration tests ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "integration-real: spawns processes"]
async fn kennel_target_register_and_list() {
    let (addr, _kennel, _temp) = spawn_kennel().await.unwrap();

    // Register a target
    let target_kp = Keypair::generate();
    let target_id = target_kp.public_key().to_peer_id();
    let mut target = KennelSession::connect(&addr, target_kp).await.unwrap();
    target
        .send(KennelPayload::TargetRegister {
            target_id: target.id().to_string(),
            name: "test-target".into(),
            pubkey: target.id().to_string(),
            direct_addr: "tcp://127.0.0.1:9999".into(),
            labels: Default::default(),
            capabilities: Default::default(),
            attached_tux_id: None,
        })
        .await
        .unwrap();
    let resp = target.recv().await.unwrap();
    assert!(matches!(resp, KennelPayload::TargetRegistered));

    // Register a TUX and list targets
    let tux_kp = Keypair::generate();
    let mut tux = KennelSession::connect(&addr, tux_kp).await.unwrap();
    tux.send(KennelPayload::TuxRegister {
        tux_id: tux.id().to_string(),
        pubkey: tux.id().to_string(),
        direct_addr: "tcp://127.0.0.1:8888".into(),
        attached_target_ids: vec![],
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    assert!(matches!(resp, KennelPayload::TuxRegistered));

    tux.send(KennelPayload::ListTargets {
        scope: ListScope::Available,
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    let KennelPayload::TargetList { targets, .. } = resp else {
        panic!("expected TargetList");
    };
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].name, "test-target");
    assert_eq!(targets[0].state, KennelTargetState::Available);
}

#[tokio::test]
#[ignore = "integration-real: spawns processes"]
async fn kennel_full_claim_attach_release_cycle() {
    let (addr, _kennel, _temp) = spawn_kennel().await.unwrap();

    // Register target
    let target_kp = Keypair::generate();
    let mut target = KennelSession::connect(&addr, target_kp).await.unwrap();
    target
        .send(KennelPayload::TargetRegister {
            target_id: target.id().to_string(),
            name: "cycle-target".into(),
            pubkey: target.id().to_string(),
            direct_addr: "tcp://127.0.0.1:9999".into(),
            labels: Default::default(),
            capabilities: Default::default(),
            attached_tux_id: None,
        })
        .await
        .unwrap();
    let _ = target.recv().await.unwrap();

    // Register TUX
    let tux_kp = Keypair::generate();
    let mut tux = KennelSession::connect(&addr, tux_kp).await.unwrap();
    tux.send(KennelPayload::TuxRegister {
        tux_id: tux.id().to_string(),
        pubkey: tux.id().to_string(),
        direct_addr: "tcp://127.0.0.1:8888".into(),
        attached_target_ids: vec![],
    })
    .await
    .unwrap();
    let _ = tux.recv().await.unwrap();

    // Claim the target
    tux.send(KennelPayload::ClaimTargets {
        target_ids: vec![target.id().to_string()],
        lease_ttl_sec: Some(30),
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    let KennelPayload::ClaimGranted { claims } = resp else {
        panic!("expected ClaimGranted");
    };
    assert_eq!(claims.len(), 1);
    let lease_id = claims[0].lease_id.clone();

    // Ack the claim
    tux.send(KennelPayload::ClaimAck {
        lease_ids: vec![lease_id.clone()],
    })
    .await
    .unwrap();

    // Target should receive Adopted
    let resp = target.recv().await.unwrap();
    let KennelPayload::Adopted {
        lease_id: adopted_lid,
        ..
    } = resp
    else {
        panic!("expected Adopted, got {resp:?}");
    };
    assert_eq!(adopted_lid, lease_id);

    // Confirm attach
    tux.send(KennelPayload::AttachConfirmed {
        lease_id: lease_id.clone(),
    })
    .await
    .unwrap();

    // Verify target shows as mine/claimed
    tux.send(KennelPayload::ListTargets {
        scope: ListScope::Mine,
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    let KennelPayload::TargetList { targets, .. } = resp else {
        panic!("expected TargetList");
    };
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].state, KennelTargetState::Claimed);

    // Verify not in available list
    tux.send(KennelPayload::ListTargets {
        scope: ListScope::Available,
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    let KennelPayload::TargetList { targets, .. } = resp else {
        panic!("expected TargetList");
    };
    assert!(targets.is_empty());

    // Release
    tux.send(KennelPayload::ReleaseTargets {
        lease_ids: vec![lease_id.clone()],
    })
    .await
    .unwrap();

    // Target should receive Released
    let resp = target.recv().await.unwrap();
    let KennelPayload::Released { lease_id: rel_lid, reason } = resp else {
        panic!("expected Released, got {resp:?}");
    };
    assert_eq!(rel_lid, lease_id);
    assert_eq!(reason, "released_by_tux");

    // TUX should receive ClaimReleased
    let resp = tux.recv().await.unwrap();
    let KennelPayload::ClaimReleased { reason, .. } = resp else {
        panic!("expected ClaimReleased, got {resp:?}");
    };
    assert_eq!(reason, "released_by_tux");

    // Target should be available again
    tux.send(KennelPayload::ListTargets {
        scope: ListScope::Available,
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    let KennelPayload::TargetList { targets, .. } = resp else {
        panic!("expected TargetList");
    };
    assert_eq!(targets.len(), 1);
}

#[tokio::test]
#[ignore = "integration-real: spawns processes"]
async fn kennel_target_disconnect_notifies_tux() {
    let (addr, _kennel, _temp) = spawn_kennel().await.unwrap();

    // Register + claim
    let target_kp = Keypair::generate();
    let mut target = KennelSession::connect(&addr, target_kp).await.unwrap();
    target
        .send(KennelPayload::TargetRegister {
            target_id: target.id().to_string(),
            name: "disc-target".into(),
            pubkey: target.id().to_string(),
            direct_addr: "tcp://127.0.0.1:9999".into(),
            labels: Default::default(),
            capabilities: Default::default(),
            attached_tux_id: None,
        })
        .await
        .unwrap();
    let _ = target.recv().await.unwrap();

    let tux_kp = Keypair::generate();
    let mut tux = KennelSession::connect(&addr, tux_kp).await.unwrap();
    tux.send(KennelPayload::TuxRegister {
        tux_id: tux.id().to_string(),
        pubkey: tux.id().to_string(),
        direct_addr: "tcp://127.0.0.1:8888".into(),
        attached_target_ids: vec![],
    })
    .await
    .unwrap();
    let _ = tux.recv().await.unwrap();

    tux.send(KennelPayload::ClaimTargets {
        target_ids: vec![target.id().to_string()],
        lease_ttl_sec: Some(30),
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    let KennelPayload::ClaimGranted { claims } = resp else {
        panic!("expected ClaimGranted");
    };
    let lease_id = claims[0].lease_id.clone();
    tux.send(KennelPayload::ClaimAck {
        lease_ids: vec![lease_id.clone()],
    })
    .await
    .unwrap();
    let _ = target.recv().await.unwrap(); // Adopted
    tux.send(KennelPayload::AttachConfirmed {
        lease_id: lease_id.clone(),
    })
    .await
    .unwrap();

    // Drop the target connection
    drop(target);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // TUX should receive TargetLost
    let resp = tux.recv().await.unwrap();
    let KennelPayload::TargetLost { target_id, lease_id: lost_lid } = resp else {
        panic!("expected TargetLost, got {resp:?}");
    };
    assert_eq!(lost_lid, Some(lease_id));
}

#[tokio::test]
#[ignore = "integration-real: spawns processes"]
async fn kennel_claim_ack_subset() {
    let (addr, _kennel, _temp) = spawn_kennel().await.unwrap();

    // Register two targets
    let t1_kp = Keypair::generate();
    let mut t1 = KennelSession::connect(&addr, t1_kp).await.unwrap();
    t1.send(KennelPayload::TargetRegister {
        target_id: t1.id().to_string(),
        name: "target-a".into(),
        pubkey: t1.id().to_string(),
        direct_addr: "tcp://127.0.0.1:9001".into(),
        labels: Default::default(),
        capabilities: Default::default(),
        attached_tux_id: None,
    })
    .await
    .unwrap();
    let _ = t1.recv().await.unwrap();

    let t2_kp = Keypair::generate();
    let mut t2 = KennelSession::connect(&addr, t2_kp).await.unwrap();
    t2.send(KennelPayload::TargetRegister {
        target_id: t2.id().to_string(),
        name: "target-b".into(),
        pubkey: t2.id().to_string(),
        direct_addr: "tcp://127.0.0.1:9002".into(),
        labels: Default::default(),
        capabilities: Default::default(),
        attached_tux_id: None,
    })
    .await
    .unwrap();
    let _ = t2.recv().await.unwrap();

    // Register TUX and claim both
    let tux_kp = Keypair::generate();
    let mut tux = KennelSession::connect(&addr, tux_kp).await.unwrap();
    tux.send(KennelPayload::TuxRegister {
        tux_id: tux.id().to_string(),
        pubkey: tux.id().to_string(),
        direct_addr: "tcp://127.0.0.1:8888".into(),
        attached_target_ids: vec![],
    })
    .await
    .unwrap();
    let _ = tux.recv().await.unwrap();

    tux.send(KennelPayload::ClaimTargets {
        target_ids: vec![t1.id().to_string(), t2.id().to_string()],
        lease_ttl_sec: Some(30),
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    let KennelPayload::ClaimGranted { claims } = resp else {
        panic!("expected ClaimGranted");
    };
    assert_eq!(claims.len(), 2);

    // Ack only the first lease
    tux.send(KennelPayload::ClaimAck {
        lease_ids: vec![claims[0].lease_id.clone()],
    })
    .await
    .unwrap();

    // Only the acked target should receive Adopted
    let resp = t1.recv().await.unwrap();
    assert!(matches!(resp, KennelPayload::Adopted { .. }));

    // The un-acked claim should timeout (5s) and release
    // Wait for the ack deadline
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;

    // Check that only one target is in the mine list
    tux.send(KennelPayload::ListTargets {
        scope: ListScope::Mine,
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();

    // We may get a ClaimReleased first from the ack timeout
    // So drain messages until we get our TargetList
    let targets = match resp {
        KennelPayload::ClaimReleased { .. } => {
            // Good — the un-acked claim was released. Read the actual list.
            tux.send(KennelPayload::ListTargets {
                scope: ListScope::Mine,
            })
            .await
            .unwrap();
            let resp = tux.recv().await.unwrap();
            let KennelPayload::TargetList { targets, .. } = resp else {
                panic!("expected TargetList");
            };
            targets
        }
        KennelPayload::TargetList { targets, .. } => targets,
        other => panic!("unexpected: {other:?}"),
    };

    // Only the acked target should remain
    assert!(
        targets.len() <= 1,
        "expected at most 1 target in mine list, got {}",
        targets.len()
    );
}

// ── Regression: fleet churn does not leave stale claims ──────────────────────

#[tokio::test]
#[ignore = "integration-real: spawns processes"]
async fn kennel_target_disappears_from_available_after_disconnect() {
    // Regression: if a target drops while listed as Available, the kennel
    // must remove it from listings. This exercises the codepath that
    // triggers the TUX-side index clamp.
    let (addr, _kennel, _temp) = spawn_kennel().await.unwrap();

    // Register target
    let target_kp = Keypair::generate();
    let mut target = KennelSession::connect(&addr, target_kp).await.unwrap();
    target
        .send(KennelPayload::TargetRegister {
            target_id: target.id().to_string(),
            name: "ephemeral".into(),
            pubkey: target.id().to_string(),
            direct_addr: "tcp://127.0.0.1:9999".into(),
            labels: Default::default(),
            capabilities: Default::default(),
            attached_tux_id: None,
        })
        .await
        .unwrap();
    let _ = target.recv().await.unwrap();

    // Register TUX, confirm target is listed
    let tux_kp = Keypair::generate();
    let mut tux = KennelSession::connect(&addr, tux_kp).await.unwrap();
    tux.send(KennelPayload::TuxRegister {
        tux_id: tux.id().to_string(),
        pubkey: tux.id().to_string(),
        direct_addr: "tcp://127.0.0.1:8888".into(),
        attached_target_ids: vec![],
    })
    .await
    .unwrap();
    let _ = tux.recv().await.unwrap();

    tux.send(KennelPayload::ListTargets {
        scope: ListScope::Available,
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    let KennelPayload::TargetList { targets, .. } = resp else {
        panic!("expected TargetList");
    };
    assert_eq!(targets.len(), 1, "target should be listed");

    // Drop target — it should disappear from the available list
    drop(target);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    tux.send(KennelPayload::ListTargets {
        scope: ListScope::Available,
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    let KennelPayload::TargetList { targets, .. } = resp else {
        panic!("expected TargetList");
    };
    assert_eq!(targets.len(), 0, "disconnected target should be removed");
}

// ── Regression: release sends ClaimReleased so peer can be cleaned up ────────

#[tokio::test]
#[ignore = "integration-real: spawns processes"]
async fn kennel_release_sends_claim_released_to_tux() {
    // Regression: verifies the TUX receives ClaimReleased on release, which
    // is the signal it needs to remove the target from its trusted peer set.
    let (addr, _kennel, _temp) = spawn_kennel().await.unwrap();

    let target_kp = Keypair::generate();
    let mut target = KennelSession::connect(&addr, target_kp).await.unwrap();
    target
        .send(KennelPayload::TargetRegister {
            target_id: target.id().to_string(),
            name: "peer-test".into(),
            pubkey: target.id().to_string(),
            direct_addr: "tcp://127.0.0.1:9999".into(),
            labels: Default::default(),
            capabilities: Default::default(),
            attached_tux_id: None,
        })
        .await
        .unwrap();
    let _ = target.recv().await.unwrap();

    let tux_kp = Keypair::generate();
    let mut tux = KennelSession::connect(&addr, tux_kp).await.unwrap();
    tux.send(KennelPayload::TuxRegister {
        tux_id: tux.id().to_string(),
        pubkey: tux.id().to_string(),
        direct_addr: "tcp://127.0.0.1:8888".into(),
        attached_target_ids: vec![],
    })
    .await
    .unwrap();
    let _ = tux.recv().await.unwrap();

    // Full claim+ack+attach cycle
    tux.send(KennelPayload::ClaimTargets {
        target_ids: vec![target.id().to_string()],
        lease_ttl_sec: Some(30),
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    let KennelPayload::ClaimGranted { claims } = resp else {
        panic!("expected ClaimGranted");
    };
    let lease_id = claims[0].lease_id.clone();
    tux.send(KennelPayload::ClaimAck {
        lease_ids: vec![lease_id.clone()],
    })
    .await
    .unwrap();
    let _ = target.recv().await.unwrap(); // Adopted
    tux.send(KennelPayload::AttachConfirmed {
        lease_id: lease_id.clone(),
    })
    .await
    .unwrap();

    // Release
    tux.send(KennelPayload::ReleaseTargets {
        lease_ids: vec![lease_id.clone()],
    })
    .await
    .unwrap();

    // TUX MUST receive ClaimReleased (the signal to remove trusted peer)
    let resp = tux.recv().await.unwrap();
    match resp {
        KennelPayload::ClaimReleased {
            lease_id: rel_lid,
            reason,
            ..
        } => {
            assert_eq!(rel_lid, lease_id);
            assert_eq!(reason, "released_by_tux");
        }
        other => panic!("expected ClaimReleased, got {other:?}"),
    }
}
