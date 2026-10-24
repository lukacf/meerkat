//! A durable worker member's run resumed after its process was killed at a
//! committed round, over the built-in SQLite realm.
//!
//! The first incarnation is a child process (this test binary again) that
//! runs the long run harness's mob: its durable `worker` member takes one
//! tool round, whose round commit applies the run's provisional tail to the
//! physical head, and reaches the kill point at its next model call, where
//! the parent kills it with SIGKILL. A second incarnation reopens the mob
//! over the same files; it materializes the member and resumes the open run,
//! which must complete and commit its boundary.

#![cfg(all(
    feature = "runtime-adapter",
    any(target_os = "linux", target_os = "macos")
))]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

mod long_run_support;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use long_run_support::{
    HarnessGate, Incarnation, KillSwitch, OpenOptions, ROOT_ENV, STEP, Store, fresh_root,
    read_killed, read_plan, run_until_killed, write_plan,
};
use meerkat_runtime::{InputTerminalOutcome, LogicalRuntimeId};
use meerkat_store_conformance::{BeforeResume, KillPoint, LongRunPlan, ResumeGate, RoundSet};

/// The child incarnation's libtest path.
const CHILD_TEST: &str = "member_resumed_run_child_incarnation";

/// One tool round commits, then the kill point is the run's next model call.
const PLAN: LongRunPlan = LongRunPlan {
    rounds: 5,
    calls_per_round: 2,
    kill_round: 1,
    kill_point: KillPoint::ModelCall,
    compactions: RoundSet::EMPTY,
    image_rounds: RoundSet::EMPTY,
    before_resume: BeforeResume::Nothing,
};

/// The child incarnation: the harness's mob runs the plan until its kill
/// point, records what the parent reads, and waits to be killed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn member_resumed_run_child_incarnation() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let plan = read_plan(&root);
    let kill = KillSwitch::new(&root);
    run_until_killed(&Store::Sqlite { root }, plan, kill, None, true).await;
}

/// Mob: a restarted mob materializes the durable worker and resumes its
/// killed run, which completes and commits its boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_run_resumed_after_a_kill_at_a_committed_round_completes() {
    let base = tempfile::tempdir().expect("tempdir");
    let root = fresh_root(base.path(), &std::sync::atomic::AtomicU32::new(0));
    write_plan(&root, &PLAN);
    let log = std::fs::File::create(root.join("child.log")).expect("child log");
    let mut child = std::process::Command::new(std::env::current_exe().expect("current_exe"))
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(ROOT_ENV, &root)
        .stdout(log.try_clone().expect("child log"))
        .stderr(log)
        .spawn()
        .expect("spawn the child incarnation");
    let started = Instant::now();
    let reached = loop {
        if root.join("killed").exists() {
            break Ok(());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Err(format!("the child exited ({status}) first")),
            Ok(None) => {}
            Err(error) => break Err(format!("poll the child: {error}")),
        }
        if started.elapsed() > STEP {
            break Err("the child never reached its kill point".to_string());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    // SIGKILL: the incarnation ends without any shutdown or cancellation.
    let _ = child.kill();
    let _ = child.wait();
    if let Err(error) = reached {
        panic!(
            "{error}: {}",
            std::fs::read_to_string(root.join("child.log")).unwrap_or_default()
        );
    }
    let killed = read_killed(&root).expect("the killed incarnation's state");

    let store = Store::Sqlite { root: root.clone() };
    let incarnation = Incarnation::open(
        &store,
        OpenOptions {
            plan: PLAN,
            restart: true,
            kill: None,
            race: None,
            gate: Some(HarnessGate::new(ResumeGate::Continue, true)),
            log_to_file: false,
        },
    )
    .await;
    assert!(
        incarnation
            .wait_terminal(&killed.session, &killed.input, STEP)
            .await,
        "the killed member input reaches a terminal after the restart"
    );
    let outcome = incarnation
        .input_terminal(&killed.session, &killed.input)
        .await;
    assert!(
        matches!(outcome, Some(InputTerminalOutcome::Consumed)),
        "the resumed member run {} completes: {outcome:?}",
        killed.run.run_id
    );
    let tail = incarnation
        .runtime_store
        .load_head_canonical_provisional_tail(&LogicalRuntimeId::for_session(&killed.session))
        .await
        .expect("read the provisional tail");
    assert!(
        tail.is_none(),
        "the resumed member run commits its boundary: {:?}",
        tail.map(|tail| tail.run_id().clone())
    );
    incarnation.shut_down().await;
}
