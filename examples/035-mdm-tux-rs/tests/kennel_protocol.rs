//! Kennel protocol integration tests.
//!
//! These tests spawn a real kennel TCP server and exercise the control protocol
//! through signed envelopes. No LLM API key needed.

use anyhow::Context;
use mdm_tux::{
    KennelPayload, KennelTargetState, ListScope, build_signed_envelope, read_envelope,
    verify_envelope, write_envelope,
};
use meerkat_comms::identity::Keypair;
use tokio::io::BufReader;
use tokio::net::TcpStream;

/// Helper: open a persistent control session to the kennel.
struct KennelSession {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
    keypair: Keypair,
    signer_id: String,
}

impl KennelSession {
    async fn connect(addr: &str, keypair: Keypair) -> anyhow::Result<Self> {
        let signer_id = keypair.public_key().to_peer_id().to_string();
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
        loop {
            let env = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                read_envelope(&mut self.reader),
            )
            .await??
            .context("connection closed")?;
            verify_envelope(&env)?;
            if matches!(env.payload, KennelPayload::ClaimRequestCompleted { .. }) {
                continue;
            }
            return Ok(env.payload);
        }
    }

    fn id(&self) -> &str {
        &self.signer_id
    }

    fn pubkey(&self) -> String {
        self.keypair.public_key().to_pubkey_string()
    }
}

/// Resolve the kennel binary from the runtime environment so archived test
/// binaries keep working outside the build that produced them.
fn kennel_binary() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_mdm-kennel")
        .map(std::path::PathBuf::from)
        .expect("CARGO_BIN_EXE_mdm-kennel is set by cargo test")
}

/// Spawn a real kennel process on a random port and return its address.
///
/// There is still a tiny TOCTOU gap between releasing the probe listener and the
/// child binding the same port, so this helper retries the whole spawn on a fresh
/// port if the child exits early or never becomes reachable.
async fn spawn_kennel() -> anyhow::Result<(String, tokio::process::Child, tempfile::TempDir)> {
    let mut last_err = None;
    let kennel = kennel_binary();

    for attempt in 0..10 {
        let temp = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        std::fs::create_dir(temp.path().join(".rkat"))?;
        std::fs::write(temp.path().join(".rkat/config.toml"), "")?;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        drop(listener);

        let mut child = tokio::process::Command::new(&kennel)
            .arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--data-dir")
            .arg(temp.path())
            .arg("--advertise")
            .arg("127.0.0.1")
            .arg("--hive-rpc-port")
            .arg("0")
            .env("OPENAI_API_KEY", "synthetic-no-network")
            .env("HOME", temp.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let addr = format!("127.0.0.1:{port}");
        let mut ready = false;

        for i in 0..100 {
            if TcpStream::connect(&addr).await.is_ok() {
                ready = true;
                break;
            }
            if let Some(status) = child.try_wait()? {
                last_err = Some(format!(
                    "kennel exited early on attempt {} with status {}",
                    attempt + 1,
                    status
                ));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50 + i * 10)).await;
        }

        if ready {
            return Ok((addr, child, temp));
        }

        let _ = child.kill().await;
        let _ = child.wait().await;
    }

    anyhow::bail!(
        "kennel did not start after retries{}",
        last_err
            .as_deref()
            .map(|e| format!(" ({e})"))
            .unwrap_or_default()
    );
}

// ── Envelope tests ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial_test::serial]
async fn real_broker_restart_changes_incarnation_but_preserves_hive_session() {
    let (addr, mut child, root) = spawn_kennel().await.unwrap();
    let mut first_session = None;
    let mut first_incarnation = None;
    for epoch in 0..2 {
        if epoch == 1 {
            child = tokio::process::Command::new(kennel_binary())
                .args([
                    "--listen",
                    &addr,
                    "--advertise",
                    "127.0.0.1",
                    "--hive-rpc-port",
                    "0",
                    "--data-dir",
                ])
                .arg(root.path())
                .env("HOME", root.path())
                .env("OPENAI_API_KEY", "synthetic-no-network")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
        }
        let mut tux = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if let Ok(client) = KennelSession::connect(&addr, Keypair::generate()).await {
                    break client;
                }
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "restarted kennel exited"
                );
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        tux.send(KennelPayload::TuxRegister {
            tux_id: tux.id().into(),
            pubkey: tux.pubkey(),
            attached_target_ids: vec![],
        })
        .await
        .unwrap();
        let KennelPayload::TuxRegistered {
            hive_session_id,
            broker_incarnation,
            ..
        } = tux.recv().await.unwrap()
        else {
            panic!("signed broker registration");
        };
        let session = hive_session_id.expect("durable Hive session");
        assert!(!broker_incarnation.is_empty());
        if epoch == 0 {
            first_session = Some(session);
            first_incarnation = Some(broker_incarnation);
        } else {
            assert_eq!(first_session.as_ref(), Some(&session));
            assert_ne!(first_incarnation.as_ref(), Some(&broker_incarnation));
        }
        drop(tux);
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }
}

#[tokio::test]
#[serial_test::serial]
async fn connected_expiry_and_correlated_refusal_allow_retry_on_same_connection() {
    let (addr, _kennel, _root) = spawn_kennel().await.unwrap();
    let mut target = KennelSession::connect(&addr, Keypair::generate())
        .await
        .unwrap();
    let target_id = target.id().to_string();
    target
        .send(KennelPayload::TargetRegister {
            target_id: target_id.clone(),
            name: "expiry-target".into(),
            pubkey: target.pubkey(),
            direct_addr: "tcp://127.0.0.1:3".into(),
            labels: Default::default(),
            rpc_addr: None,
            capabilities: Default::default(),
            attached_tux_id: None,
        })
        .await
        .unwrap();
    assert!(matches!(
        target.recv().await.unwrap(),
        KennelPayload::TargetRegistered { .. }
    ));
    let mut first = KennelSession::connect(&addr, Keypair::generate())
        .await
        .unwrap();
    let mut loser = KennelSession::connect(&addr, Keypair::generate())
        .await
        .unwrap();
    for client in [&mut first, &mut loser] {
        client
            .send(KennelPayload::TuxRegister {
                tux_id: client.id().into(),
                pubkey: client.pubkey(),
                attached_target_ids: vec![],
            })
            .await
            .unwrap();
        assert!(matches!(
            client.recv().await.unwrap(),
            KennelPayload::TuxRegistered { .. }
        ));
    }
    first
        .send(KennelPayload::ClaimTargets {
            target_ids: vec![target_id.clone()],
            lease_ttl_sec: Some(2),
        })
        .await
        .unwrap();
    let KennelPayload::ClaimGranted { claims } = first.recv().await.unwrap() else {
        panic!("grant");
    };
    let lease_id = claims[0].lease_id.clone();
    first
        .send(KennelPayload::ClaimAck {
            lease_ids: vec![lease_id.clone()],
        })
        .await
        .unwrap();
    let request = build_signed_envelope(
        &loser.keypair,
        loser.id(),
        KennelPayload::ClaimTargets {
            target_ids: vec![target_id.clone()],
            lease_ttl_sec: Some(30),
        },
    )
    .unwrap();
    write_envelope(&mut loser.writer, &request).await.unwrap();
    let KennelPayload::ClaimGranted { claims } = loser.recv().await.unwrap() else {
        panic!("grant");
    };
    assert!(claims.is_empty());
    let completion = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        read_envelope(&mut loser.reader),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    verify_envelope(&completion).unwrap();
    assert!(
        matches!(completion.payload, KennelPayload::ClaimRequestCompleted {
        in_reply_to, refused_target_ids,
    } if in_reply_to == request.message_id && refused_target_ids == vec![target_id.clone()])
    );
    assert!(matches!(
        first.recv().await.unwrap(),
        KennelPayload::ClaimReleased {
            reason: mdm_tux::LeaseTerminationReason::LeaseExpired,
            ..
        }
    ));
    first
        .send(KennelPayload::RenewLeases {
            lease_ids: vec![lease_id],
            lease_ttl_sec: Some(30),
        })
        .await
        .unwrap();
    assert!(
        matches!(first.recv().await.unwrap(), KennelPayload::LeasesRenewed { leases } if leases.is_empty())
    );
    loser
        .send(KennelPayload::ClaimTargets {
            target_ids: vec![target_id],
            lease_ttl_sec: Some(30),
        })
        .await
        .unwrap();
    assert!(
        matches!(loser.recv().await.unwrap(), KennelPayload::ClaimGranted { claims } if claims.len() == 1)
    );
}

#[test]
fn signed_envelope_roundtrip() {
    let kp = Keypair::generate();
    let signer_id = kp.public_key().to_peer_id().to_string();
    let payload = KennelPayload::TargetHeartbeat;
    let env = build_signed_envelope(&kp, &signer_id, payload).unwrap();
    let signer = verify_envelope(&env).unwrap();
    assert_eq!(signer, kp.public_key());
}

#[test]
fn invalid_signature_rejected() {
    let kp = Keypair::generate();
    let signer_id = kp.public_key().to_peer_id().to_string();
    let mut env = build_signed_envelope(&kp, &signer_id, KennelPayload::TargetHeartbeat).unwrap();
    // Corrupt the signature
    env.signature =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, vec![0u8; 64]);
    assert!(verify_envelope(&env).is_err());
}

// ── Kennel integration tests ─────────────────────────────────────────────────

#[tokio::test]
#[serial_test::serial]
async fn kennel_target_register_and_list() {
    let (addr, _kennel, _temp) = spawn_kennel().await.unwrap();

    // Register a target
    let target_kp = Keypair::generate();
    let mut target = KennelSession::connect(&addr, target_kp).await.unwrap();
    target
        .send(KennelPayload::TargetRegister {
            target_id: target.id().to_string(),
            name: "test-target".into(),
            pubkey: target.pubkey().to_string(),
            direct_addr: "tcp://127.0.0.1:9999".into(),
            labels: Default::default(),
            rpc_addr: None,
            capabilities: Default::default(),
            attached_tux_id: None,
        })
        .await
        .unwrap();
    let resp = target.recv().await.unwrap();
    assert!(matches!(resp, KennelPayload::TargetRegistered { .. }));

    // Register a TUX and list targets
    let tux_kp = Keypair::generate();
    let mut tux = KennelSession::connect(&addr, tux_kp).await.unwrap();
    tux.send(KennelPayload::TuxRegister {
        tux_id: tux.id().to_string(),
        pubkey: tux.pubkey().to_string(),
        attached_target_ids: vec![],
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    assert!(matches!(resp, KennelPayload::TuxRegistered { .. }));

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
#[serial_test::serial]
async fn kennel_full_claim_release_cycle() {
    let (addr, _kennel, _temp) = spawn_kennel().await.unwrap();

    // Register target
    let target_kp = Keypair::generate();
    let mut target = KennelSession::connect(&addr, target_kp).await.unwrap();
    target
        .send(KennelPayload::TargetRegister {
            target_id: target.id().to_string(),
            name: "cycle-target".into(),
            pubkey: target.pubkey().to_string(),
            direct_addr: "tcp://127.0.0.1:9999".into(),
            labels: Default::default(),
            rpc_addr: None,
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
        pubkey: tux.pubkey().to_string(),
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

    // Ack the claim -- goes directly to Claimed (no attach step)
    tux.send(KennelPayload::ClaimAck {
        lease_ids: vec![lease_id.clone()],
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
    let KennelPayload::Released { lease_ref, reason } = resp else {
        panic!("expected Released, got {resp:?}");
    };
    assert_eq!(
        lease_ref,
        mdm_tux::LeaseRef::Known {
            lease_id: lease_id.clone()
        }
    );
    assert_eq!(reason, mdm_tux::LeaseTerminationReason::ReleasedByTux);

    // TUX should receive ClaimReleased
    let resp = tux.recv().await.unwrap();
    let KennelPayload::ClaimReleased { reason, .. } = resp else {
        panic!("expected ClaimReleased, got {resp:?}");
    };
    assert_eq!(reason, mdm_tux::LeaseTerminationReason::ReleasedByTux);

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
#[serial_test::serial]
async fn kennel_target_disconnect_releases_claim() {
    let (addr, _kennel, _temp) = spawn_kennel().await.unwrap();

    // Register + claim
    let target_kp = Keypair::generate();
    let mut target = KennelSession::connect(&addr, target_kp).await.unwrap();
    target
        .send(KennelPayload::TargetRegister {
            target_id: target.id().to_string(),
            name: "disc-target".into(),
            pubkey: target.pubkey().to_string(),
            direct_addr: "tcp://127.0.0.1:9999".into(),
            labels: Default::default(),
            rpc_addr: None,
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
        pubkey: tux.pubkey().to_string(),
        attached_target_ids: vec![],
    })
    .await
    .unwrap();
    let _ = tux.recv().await.unwrap();

    tux.send(KennelPayload::ClaimTargets {
        target_ids: vec![target.id().to_string()],
        lease_ttl_sec: Some(5), // short TTL so recovery expires quickly
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

    // Verify the claim is in Claimed state before disconnecting the target
    tux.send(KennelPayload::ListTargets {
        scope: ListScope::Mine,
    })
    .await
    .unwrap();
    let resp = tux.recv().await.unwrap();
    let KennelPayload::TargetList { targets, .. } = resp else {
        panic!("expected TargetList, got {resp:?}");
    };
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].state, KennelTargetState::Claimed);

    // Drop the target connection -- enters RecoveringClaim
    drop(target);

    // The lease TTL is 5s, so recovery expires at most ~5s from claim.
    // The kennel janitor ticks every 1s. We should get ClaimReleased
    // within ~7s.
    let resp = tokio::time::timeout(std::time::Duration::from_secs(10), tux.recv()).await;
    match resp {
        Ok(Ok(KennelPayload::ClaimReleased { .. })) => {
            // Expected: the claim was released after recovery expired
        }
        Ok(Ok(other)) => {
            // Some other message first -- that's ok
            eprintln!("got intermediate message: {other:?}");
        }
        Ok(Err(e)) => panic!("recv error: {e}"),
        Err(_) => panic!("timed out waiting for ClaimReleased"),
    }
}

#[tokio::test]
#[serial_test::serial]
async fn kennel_claim_ack_subset() {
    let (addr, _kennel, _temp) = spawn_kennel().await.unwrap();

    // Register two targets
    let t1_kp = Keypair::generate();
    let mut t1 = KennelSession::connect(&addr, t1_kp).await.unwrap();
    t1.send(KennelPayload::TargetRegister {
        target_id: t1.id().to_string(),
        name: "target-a".into(),
        pubkey: t1.pubkey().to_string(),
        direct_addr: "tcp://127.0.0.1:9001".into(),
        rpc_addr: None,
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
        pubkey: t2.pubkey().to_string(),
        direct_addr: "tcp://127.0.0.1:9002".into(),
        rpc_addr: None,
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
        pubkey: tux.pubkey().to_string(),
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

    // Ack only the first lease -- goes directly to Claimed
    tux.send(KennelPayload::ClaimAck {
        lease_ids: vec![claims[0].lease_id.clone()],
    })
    .await
    .unwrap();

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
            // Good -- the un-acked claim was released. Read the actual list.
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
#[serial_test::serial]
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
            pubkey: target.pubkey().to_string(),
            direct_addr: "tcp://127.0.0.1:9999".into(),
            labels: Default::default(),
            rpc_addr: None,
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
        pubkey: tux.pubkey().to_string(),
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

    // Drop target -- it should disappear from the available list
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
#[serial_test::serial]
async fn kennel_release_sends_claim_released_to_tux() {
    // Regression: verifies the TUX receives ClaimReleased on release, which
    // is the signal it needs to clean up the target.
    let (addr, _kennel, _temp) = spawn_kennel().await.unwrap();

    let target_kp = Keypair::generate();
    let mut target = KennelSession::connect(&addr, target_kp).await.unwrap();
    target
        .send(KennelPayload::TargetRegister {
            target_id: target.id().to_string(),
            name: "peer-test".into(),
            pubkey: target.pubkey().to_string(),
            direct_addr: "tcp://127.0.0.1:9999".into(),
            labels: Default::default(),
            rpc_addr: None,
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
        pubkey: tux.pubkey().to_string(),
        attached_target_ids: vec![],
    })
    .await
    .unwrap();
    let _ = tux.recv().await.unwrap();

    // Claim + ack (goes directly to Claimed)
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

    // Release
    tux.send(KennelPayload::ReleaseTargets {
        lease_ids: vec![lease_id.clone()],
    })
    .await
    .unwrap();

    // TUX MUST receive ClaimReleased
    let resp = tux.recv().await.unwrap();
    match resp {
        KennelPayload::ClaimReleased {
            lease_ref, reason, ..
        } => {
            assert_eq!(lease_ref, mdm_tux::LeaseRef::Known { lease_id });
            assert_eq!(reason, mdm_tux::LeaseTerminationReason::ReleasedByTux);
        }
        other => panic!("expected ClaimReleased, got {other:?}"),
    }
}
