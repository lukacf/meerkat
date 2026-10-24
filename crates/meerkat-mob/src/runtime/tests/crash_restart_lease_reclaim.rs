//! A mob restarted after its process was killed takes its identities'
//! leases at once.
//!
//! The first host is a child process (this test binary again) over SQLite
//! mob and session stores. It adopts a member's existing session as the
//! member's identity, waits until the identity converged under its lease, and
//! is then killed: no Shutdown runs, and the lease stays live for its whole
//! lifetime. The test resumes the mob over the same files in this process, as
//! the next boot does. The identity converges under the new incarnation's
//! lease long before the killed host's lease would expire.

use super::unregister_self_heal::{AnswerClient, definition, mob_storage, open_service};
use super::*;
use crate::runtime::tests::shutdown_lease_release::{adoption, wait_converged_within};

const PHASE_ENV: &str = "MEERKAT_LEASE_RECLAIM_PHASE";
const ROOT_ENV: &str = "MEERKAT_LEASE_RECLAIM_ROOT";
const CHILD_TEST: &str =
    "runtime::tests::actor_isolation::crash_restart_lease_reclaim::lease_reclaim_host_child";

/// Bound for one step (the child's convergence, the resume); it only turns a
/// hang into a failure.
const STEP: Duration = Duration::from_secs(150);

/// The bound within which the restart takes the killed host's lease: well
/// inside the 30 s lifetime a restart that cannot prove the exit waits out.
const RECLAIM_BOUND: Duration = Duration::from_secs(2);

/// The first host. It runs only when the parent re-executes this binary;
/// as an ordinary test it does nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lease_reclaim_host_child() {
    let (Some(root), Some(_)) = (std::env::var_os(ROOT_ENV), std::env::var_os(PHASE_ENV)) else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let mob_id = std::fs::read_to_string(root.join("mob-id")).expect("mob id");
    let (service, _store) = open_service(&root).await;
    let handle = MobBuilder::new(definition(mob_id.trim()), mob_storage(&root))
        .with_session_service(service)
        .with_default_llm_client(Arc::new(AnswerClient))
        .create()
        .await
        .expect("create the persistent mob");
    let member = AgentIdentity::from("worker-1");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", member.clone()))
        .await
        .expect("spawn the member");
    let session_id = handle
        .resolve_bridge_session_id(&member)
        .await
        .expect("member session");
    let adopted = handle
        .adopt_member_identity_declaration(adoption(handle.mob_id(), &member, &session_id))
        .await
        .expect("adopt the member's session");
    assert!(
        matches!(
            adopted.adoption,
            crate::identity::IdentityAdoptionOutcome::Adopted {
                desired_revision: 1
            }
        ),
        "{member}: {:?}",
        adopted.adoption
    );
    if let Err(latest) = wait_converged_within(&handle, &member, 0, STEP).await {
        panic!("{member}: the adopted identity converges; latest status: {latest}");
    }
    std::fs::write(root.join("converged"), "converged\n").expect("write the marker");
    std::future::pending::<()>().await;
}

/// The killed host's identity lease is taken over at once by the restart.
// The host is this test binary re-executed: a test-only process launch.
#[allow(clippy::disallowed_methods)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_restarted_after_its_process_was_killed_takes_its_identity_leases_at_once() {
    let root = tempfile::Builder::new()
        .prefix("meerkat-lease-reclaim-")
        .tempdir()
        .expect("fixture storage in the system temp dir");
    let root_path = root.path().canonicalize().expect("absolute fixture root");
    for directory in [
        "config", "runtime", "project", "context", "hosting", "delivery",
    ] {
        std::fs::create_dir_all(root_path.join(directory)).expect("fixture root");
    }
    let mob_id = format!("lease-reclaim-{}", uuid::Uuid::new_v4().simple());
    std::fs::write(root_path.join("mob-id"), &mob_id).expect("mob id");

    let log_path = root_path.join("child.log");
    let log = std::fs::File::create(&log_path).expect("child log");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(PHASE_ENV, "host")
        .env(ROOT_ENV, &root_path)
        .stdout(log.try_clone().expect("child log"))
        .stderr(log)
        .spawn()
        .expect("spawn the host");
    let marker = root_path.join("converged");
    let started = Instant::now();
    while !marker.exists() {
        let exited = child.try_wait().expect("poll the host");
        assert!(
            exited.is_none() && started.elapsed() < STEP,
            "the host never converged its identity ({exited:?}): {}",
            std::fs::read_to_string(&log_path).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let member = AgentIdentity::from("worker-1");
    let mob = MobId::from(mob_id.clone());
    let killed = match mob_storage(&root_path)
        .identity
        .observe_identity_lease(&mob, &member)
        .await
        .expect("read the host's lease")
    {
        crate::identity::IdentityStoredObservation::Valid(record) => {
            record.active.expect("the converged host holds the lease")
        }
        other => panic!("{member}: the host's lease is readable: {other:?}"),
    };
    child.kill().expect("kill the host");
    child.wait().expect("reap the host");

    let (service, _store) = open_service(&root_path).await;
    let resume = MobBuilder::for_resume(mob_storage(&root_path))
        .with_session_service(service)
        .with_default_llm_client(Arc::new(AnswerClient))
        .notify_orchestrator_on_resume(false)
        .resume();
    let resumed = tokio::time::timeout(STEP, resume)
        .await
        .expect("the resume answers")
        .expect("the resume after the kill succeeds");
    if let Err(latest) = wait_converged_within(&resumed, &member, killed.epoch, RECLAIM_BOUND).await
    {
        panic!(
            "{member}: the restart takes the killed host's lease (epoch {}, expires at {} ms) at once; latest status: {latest}",
            killed.epoch, killed.expires_at_ms
        );
    }
    resumed.shutdown().await.expect("shut the resumed mob down");
}
