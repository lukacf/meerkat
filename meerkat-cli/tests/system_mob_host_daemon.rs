//! Multi-host mobs phase 2 — §W4.5 e2e-system row: REAL `rkat mob host`
//! process lifecycle.
//!
//! Everything the deterministic two-hosts-in-one-process walk cannot pin:
//! the shipped binary boots the daemon composition, the host binding
//! descriptor lands 0600 on disk and is re-minted in place after a bind, a
//! SECOND process (this test) binds over real loopback TCP, a consumed
//! (stale) descriptor is rejected, and a daemon restart over the same SQLite
//! realm revives a schedule-tooled member before firing its overdue exact-
//! session schedule. The recovered binding still rejects an intruder and the
//! recorded supervisor converges through the bind-replay arm.
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
use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use meerkat::{
    CreateScheduleRequest, MisfirePolicy, MissingTargetPolicy, Occurrence, OccurrencePhase,
    OverlapPolicy, ScheduleId, ScheduleService, ScheduleStore, ScheduledSessionAction,
    SessionTargetBinding, SqliteScheduleStore, TargetBinding, TriggerSpec,
};
use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_core::types::{ContentInput, SessionId};
use meerkat_mob::runtime::bridge_protocol::{
    BridgeRejectionCause, BridgeReply, MaterializeLaunchMode, WireHostBindingDescriptor,
};
use probe::{
    portable_member_spec_for_raw_probe, raw_bind_host_command, raw_host_status_command,
    raw_materialize_member_command, spawn_peer_comms_endpoint,
};
use tempfile::TempDir;
use tokio::process::{Child, Command};

const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
const DESCRIPTOR_TIMEOUT: Duration = Duration::from_secs(60);
const SCHEDULE_TIMEOUT: Duration = Duration::from_secs(60);
const REALM_ID: &str = "mob-host-e2e";

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

    fn sessions_sqlite_path(&self) -> PathBuf {
        meerkat_store::realm_paths_in(&self.temp.path().join("realms"), REALM_ID)
            .sessions_sqlite_path
    }

    fn spawn_daemon(&self, rkat: &std::path::Path) -> Child {
        Command::new(rkat)
            .current_dir(self.temp.path())
            .env("HOME", self.temp.path())
            .env("XDG_CONFIG_HOME", self.temp.path().join("config"))
            .env("RKAT_TEST_CLIENT", "1")
            .args([
                "--state-root",
                self.temp.path().join("realms").to_str().expect("utf8"),
                "--realm",
                REALM_ID,
                "--realm-backend",
                "sqlite",
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

async fn seed_overdue_exact_session_schedule(
    home: &DaemonHome,
    session_id: &str,
) -> (ScheduleService, ScheduleId) {
    let store = Arc::new(
        SqliteScheduleStore::open(home.sessions_sqlite_path())
            .expect("open the stopped daemon's realm SQLite schedule store"),
    ) as Arc<dyn ScheduleStore>;
    let service = ScheduleService::new(store);
    let session_id = SessionId::parse(session_id).expect("materialized member session id parses");
    let schedule = service
        .create(CreateScheduleRequest {
            name: Some("revival-before-overdue-exact-session".to_string()),
            description: Some(
                "Wake the remote archivist after a cold restart and ask it to count backward from seven"
                    .to_string(),
            ),
            trigger: TriggerSpec::Once {
                due_at_utc: Utc::now() - ChronoDuration::seconds(2),
            },
            target: TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id,
                action: ScheduledSessionAction::Prompt {
                    prompt: ContentInput::Text(
                        "Count backward from seven, then report that the lighthouse survived the reboot."
                            .to_string(),
                    ),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }),
            misfire_policy: MisfirePolicy::CatchUpWithin {
                window_seconds: 3_600,
            },
            overlap_policy: OverlapPolicy::AllowConcurrent,
            missing_target_policy: MissingTargetPolicy::MarkMisfired,
            labels: Default::default(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        })
        .await
        .expect("seed one overdue exact-session occurrence");
    (service, schedule.schedule_id)
}

async fn wait_for_completed_occurrence(
    service: &ScheduleService,
    schedule_id: &ScheduleId,
    daemon: &mut Child,
) -> Occurrence {
    let deadline = tokio::time::Instant::now() + SCHEDULE_TIMEOUT;
    loop {
        if let Ok(Some(status)) = daemon.try_wait() {
            let mut stderr = String::new();
            if let Some(mut pipe) = daemon.stderr.take() {
                use tokio::io::AsyncReadExt as _;
                let _ = pipe.read_to_string(&mut stderr).await;
            }
            panic!("daemon exited before the overdue occurrence completed: {status}\n{stderr}");
        }

        let mut occurrences = service
            .list_occurrences(schedule_id)
            .await
            .expect("read overdue occurrence from the shared SQLite store");
        assert_eq!(
            occurrences.len(),
            1,
            "the once schedule must retain exactly one occurrence"
        );
        let occurrence = occurrences.remove(0);
        if occurrence.phase == OccurrencePhase::Completed {
            return occurrence;
        }
        assert!(
            !occurrence.is_terminal(),
            "overdue exact-session occurrence terminated before completion: {occurrence:?}"
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for overdue exact-session completion; last occurrence: {occurrence:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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

    // Materialize a real remote member whose portable profile includes the
    // schedule tool. The daemon builds it with RKAT_TEST_CLIENT=1, records the
    // exact SessionId in its durable host row, and serves it through the same
    // runtime adapter the realm schedule driver will use after restart.
    let member_identity = "lighthouse-archivist";
    let mut member_spec =
        portable_member_spec_for_raw_probe(mob_id, member_identity, "scheduled-worker");
    member_spec.profile.tools.schedule = true;
    let materialize = supervisor
        .send_bridge_command_raw(
            &host_trust_descriptor(&d1),
            &raw_materialize_member_command(
                &supervisor,
                1,
                1,
                member_spec,
                1,
                1,
                MaterializeLaunchMode::Fresh {},
            ),
            REPLY_TIMEOUT,
        )
        .await
        .expect("materialize schedule-tooled member over real TCP");
    let BridgeReply::MemberMaterialized(materialized) = materialize else {
        panic!("expected real member materialization, got {materialize:?}");
    };

    // ---- Lifetime 2: seed overdue exact-session work, then cold restart ----
    shutdown(daemon).await;
    let (schedule_service, schedule_id) =
        seed_overdue_exact_session_schedule(&home, &materialized.session_id).await;
    let mut daemon = home.spawn_daemon(&rkat);
    let d2 = home
        .wait_for_descriptor(Some(d1.bootstrap_token.as_str()), &mut daemon)
        .await;

    // The production schedule loop starts only after host-owned A20 revival
    // and observation recovery. The overdue occurrence therefore executes
    // once through the exact revived member attachment, never through a
    // generic persisted-session executor racing ahead of revival.
    let completed =
        wait_for_completed_occurrence(&schedule_service, &schedule_id, &mut daemon).await;
    assert_eq!(
        completed.attempt_count, 1,
        "the overdue occurrence must not be transferred or redriven across attachments"
    );

    supervisor.trust(host_trust_descriptor(&d2)).await;
    let status = supervisor
        .send_bridge_command_raw(
            &host_trust_descriptor(&d2),
            &raw_host_status_command(&supervisor, mob_id, 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("host status after scheduled cold-start delivery");
    let BridgeReply::HostStatus(status) = status else {
        panic!("expected HostStatus after scheduled cold-start delivery, got {status:?}");
    };
    let revived = status
        .members
        .iter()
        .find(|member| member.session_id == materialized.session_id)
        .unwrap_or_else(|| {
            panic!(
                "HostStatus omitted the exact scheduled member {}: {status:?}",
                materialized.session_id
            )
        });
    assert_eq!(revived.agent_identity, member_identity);
    assert!(
        revived.healthy,
        "the exact member must remain healthy after its overdue schedule completes: {revived:?}"
    );

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
