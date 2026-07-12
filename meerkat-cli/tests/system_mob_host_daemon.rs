//! Multi-host mobs phase 2 — §W4.5 e2e-system row: REAL `rkat mob host`
//! process lifecycle.
//!
//! Everything the deterministic two-hosts-in-one-process walk cannot pin:
//! the shipped binary boots the daemon composition, the host binding
//! descriptor lands 0600 on disk and is re-minted in place after a bind, a
//! SECOND process (this test) binds over real loopback TCP, a consumed
//! (stale) descriptor is rejected, and a daemon restart over the same realm
//! recovers the binding from the R8 store (intruder still `AlreadyBound`,
//! recorded supervisor converges through the bind-replay arm).
//!
//! Lane: `e2e-system` (local `make e2e-system` / nightly — NOT GitHub CI).
//! Registered as the `cli-mob-host-daemon` suite in
//! `tests/integration/src/e2e_lanes.rs`.

#![cfg(feature = "integration-real-tests")]
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../meerkat-mob/tests/support/probe.rs"]
mod probe;

use std::path::PathBuf;
use std::time::Duration;

use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_mob::runtime::bridge_protocol::{
    BridgeRejectionCause, BridgeReply, WireHostBindingDescriptor,
};
use probe::{raw_bind_host_command, spawn_peer_comms_endpoint};
use tempfile::TempDir;
use tokio::process::{Child, Command};

const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
const DESCRIPTOR_TIMEOUT: Duration = Duration::from_secs(60);

fn rkat_binary_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path.canonicalize().unwrap_or(path));
        }
    }
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        for profile in ["debug", "release"] {
            let candidate = target_dir.join(profile).join("rkat");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?;
    for target in ["target-codex", "target"] {
        for profile in ["debug", "release"] {
            let candidate = workspace_root.join(target).join(profile).join("rkat");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

struct DaemonHome {
    temp: TempDir,
}

impl DaemonHome {
    fn new() -> Self {
        Self {
            temp: TempDir::new().expect("daemon home tempdir"),
        }
    }

    fn descriptor_path(&self) -> PathBuf {
        self.temp.path().join("host-binding.json")
    }

    fn identity_dir(&self) -> PathBuf {
        self.temp.path().join("host-identity")
    }

    fn spawn_daemon(&self, rkat: &std::path::Path) -> Child {
        Command::new(rkat)
            .current_dir(self.temp.path())
            .env("HOME", self.temp.path())
            .env("XDG_CONFIG_HOME", self.temp.path().join("config"))
            .args([
                "--state-root",
                self.temp.path().join("realms").to_str().expect("utf8"),
                "--realm",
                "mob-host-e2e",
                "mob",
                "host",
                "--listen-tcp",
                "127.0.0.1:0",
                "--identity-dir",
                self.identity_dir().to_str().expect("utf8"),
                "--descriptor-out",
                self.descriptor_path().to_str().expect("utf8"),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn rkat mob host")
    }

    /// Poll the descriptor slot until it parses and (when `differs_from` is
    /// set) carries a DIFFERENT bootstrap token — the daemon rewrites the
    /// file in place on re-mint, so a partially-written or stale read is
    /// retried, never failed.
    async fn wait_for_descriptor(
        &self,
        differs_from: Option<&str>,
        daemon: &mut Child,
    ) -> WireHostBindingDescriptor {
        let deadline = tokio::time::Instant::now() + DESCRIPTOR_TIMEOUT;
        loop {
            if let Ok(bytes) = std::fs::read(self.descriptor_path())
                && let Ok(descriptor) = serde_json::from_slice::<WireHostBindingDescriptor>(&bytes)
                && differs_from.is_none_or(|old| descriptor.bootstrap_token.as_str() != old)
            {
                return descriptor;
            }
            if let Ok(Some(status)) = daemon.try_wait() {
                let mut stderr = String::new();
                if let Some(mut pipe) = daemon.stderr.take() {
                    use tokio::io::AsyncReadExt as _;
                    let _ = pipe.read_to_string(&mut stderr).await;
                }
                panic!("daemon exited before publishing a descriptor: {status}\n{stderr}");
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for the host binding descriptor"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn assert_descriptor_mode_0600(&self) {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(self.descriptor_path())
            .expect("descriptor metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "the host binding descriptor must land 0600 on disk"
        );
    }
}

fn host_trust_descriptor(descriptor: &WireHostBindingDescriptor) -> TrustedPeerDescriptor {
    let resolved = descriptor
        .identity
        .resolve()
        .expect("host descriptor identity resolves");
    TrustedPeerDescriptor::unsigned_with_pubkey(
        "mob-host-daemon",
        resolved.peer_id.to_string(),
        resolved.pubkey,
        descriptor.address.clone(),
    )
    .expect("host trusted-peer descriptor")
}

async fn shutdown(mut daemon: Child) {
    let _ = daemon.kill().await;
    let _ = daemon.wait().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane:e2e-system"]
async fn integration_real_mob_host_daemon_lifecycle() {
    let rkat = rkat_binary_path().expect("rkat binary not found");
    let home = DaemonHome::new();

    // ---- Lifetime 1: boot, descriptor on disk, bind over real TCP ----
    let mut daemon = home.spawn_daemon(&rkat);
    let d0 = home.wait_for_descriptor(None, &mut daemon).await;
    home.assert_descriptor_mode_0600();

    let supervisor = spawn_peer_comms_endpoint("system-supervisor", true, None).await;
    supervisor.trust(host_trust_descriptor(&d0)).await;
    let mob_id = "mob-system-daemon";
    let reply = supervisor
        .send_bridge_command_raw(
            &host_trust_descriptor(&d0),
            &raw_bind_host_command(&supervisor, mob_id, &d0, 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("bind reply over real TCP");
    assert!(
        matches!(reply, BridgeReply::BindHost(_)),
        "the shipped daemon must serve the bind ceremony, got {reply:?}"
    );

    // Token consumed + descriptor re-minted IN PLACE on disk.
    let d1 = home
        .wait_for_descriptor(Some(d0.bootstrap_token.as_str()), &mut daemon)
        .await;
    home.assert_descriptor_mode_0600();
    assert_eq!(d1.address, d0.address, "re-mint keeps the bind address");

    // Stale descriptor (consumed token) rejected for a second mob.
    let stale = supervisor
        .send_bridge_command_raw(
            &host_trust_descriptor(&d1),
            &raw_bind_host_command(&supervisor, "mob-system-second", &d0, 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("stale bind reply");
    assert!(
        matches!(
            stale,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::InvalidBootstrapToken,
                ..
            }
        ),
        "a consumed descriptor must be rejected typed, got {stale:?}"
    );

    // ---- Restart: same realm, same identity dir ⇒ R8 recovery ----
    shutdown(daemon).await;
    let mut daemon = home.spawn_daemon(&rkat);
    let d2 = home
        .wait_for_descriptor(Some(d1.bootstrap_token.as_str()), &mut daemon)
        .await;

    // The recovered binding still refuses a different supervisor.
    let intruder = spawn_peer_comms_endpoint("system-intruder", true, None).await;
    intruder.trust(host_trust_descriptor(&d2)).await;
    let takeover = intruder
        .send_bridge_command_raw(
            &host_trust_descriptor(&d2),
            &raw_bind_host_command(&intruder, mob_id, &d2, 2),
            REPLY_TIMEOUT,
        )
        .await
        .expect("takeover reply");
    assert!(
        matches!(
            takeover,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::AlreadyBound,
                ..
            }
        ),
        "the restarted daemon must recover the binding from the R8 store, got {takeover:?}"
    );

    // The recorded supervisor converges through the bind-replay arm at the
    // recorded tuple (fresh descriptor, restarted process, same epoch).
    supervisor.trust(host_trust_descriptor(&d2)).await;
    let replay = supervisor
        .send_bridge_command_raw(
            &host_trust_descriptor(&d2),
            &raw_bind_host_command(&supervisor, mob_id, &d2, 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("replay reply");
    assert!(
        matches!(replay, BridgeReply::BindHost(_)),
        "the recorded supervisor must converge after daemon restart, got {replay:?}"
    );

    // The doctored replay above was a fresh-token bind for the SAME mob —
    // it must not have burned d2; but the successful replay DOES leave the
    // token consumed only when the bind was fresh. Either way the daemon
    // stays serviceable: a new mob binds with the CURRENT descriptor.
    let d3: WireHostBindingDescriptor = serde_json::from_slice(
        &std::fs::read(home.descriptor_path()).expect("read current descriptor"),
    )
    .expect("current descriptor parses");
    let second = supervisor
        .send_bridge_command_raw(
            &host_trust_descriptor(&d3),
            &raw_bind_host_command(&supervisor, "mob-system-second", &d3, 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("second mob bind reply");
    assert!(
        matches!(second, BridgeReply::BindHost(_)),
        "the daemon stays bindable across restarts (A14), got {second:?}"
    );

    shutdown(daemon).await;
}
