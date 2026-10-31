//! Core over a store through the long run, compacted long run and durable
//! worker conformance chapters of `meerkat-store-conformance`, over the
//! built-in SQLite realm (the first incarnation is a child process killed
//! with SIGKILL) and over the reference host store (the runtime the first
//! incarnation runs on is shut down in this process, which Core treats as a
//! crash of that incarnation), plus Core's own row for a killed process's
//! identity lease.
//!
//! Each chapter runs every row over a fresh store, so a chapter takes
//! minutes, not seconds.
//!
//! The deploy rows end the first incarnation the way a host deploys instead
//! of killing it: SIGTERM, then the mob's terminal Shutdown inside the host's
//! budget. A graceful requeue stop keeps a durable session's in-flight run at
//! its last committed round, exactly as a crash does, so the restart resumes
//! it; a run whose requeue bound passed is settled as expired instead.

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
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use long_run_support::{
    ChildRace, DeployConfig, DeployRestart, FollowUp, HarnessGate, HostRace, Incarnation,
    KillSwitch, KilledState, OpenOptions, ROOT_ENV, RaceHook, Restarted, SHUTDOWN_BUDGET, STEP,
    Store, WHOLE_BLOB, WORKER, await_supervisor_route_release, discard_provisional_tail,
    fail_model_call, fresh_root, kill_orphaned_process, other_writer, read_killed,
    read_killed_member, read_plan, restart, restart_deploy, restart_workers,
    run_resumed_until_killed, run_until_killed, settled_run_report, wait_converged, write_plan,
};
use meerkat_core::types::SessionId;
use meerkat_store_conformance::{
    BeforeResume, ConformanceFailure, DurableWorkerHarness, KillPoint, KilledRun, KilledWorker,
    LongRunHarness, LongRunPlan, RestartReport, RestartedWorker, ResumeGate, ResumePointKind,
    ResumedRun, RoundSet, UnrepeatableTool, call_id, chapters, resumes,
};

/// The child incarnation's test: this binary runs it again, with
/// [`ROOT_ENV`] naming the row. As an ordinary test it does nothing.
const CHILD_TEST: &str = "sqlite_child_incarnation";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sqlite_child_incarnation() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let plan = read_plan(&root);
    let kill = KillSwitch::new(&root);
    if root.join(RESUMED_KILL).exists() {
        run_resumed_until_killed(&Store::Sqlite { root }, plan, kill).await;
        return;
    }
    let race: Arc<dyn RaceHook> = Arc::new(ChildRace { root: root.clone() });
    run_until_killed(&Store::Sqlite { root }, plan, kill, Some(race), true).await;
}

/// A row's marker: its next incarnation reopens the mob and is killed again
/// at the plan's kill point, instead of creating the mob and its input.
const RESUMED_KILL: &str = "resumed-kill";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// The built-in SQLite realm, a child process killed with SIGKILL.
    Sqlite,
    /// The reference host store, every Core object dropped.
    Host,
}

/// The Core harness of both chapters over one backend.
struct Harness {
    backend: Backend,
    base: tempfile::TempDir,
    rows: AtomicU32,
    /// The row between its kill and its restart.
    current: Mutex<Option<(Store, KilledState)>>,
}

/// A child incarnation, killed with SIGKILL and reaped however the row
/// that spawned it ends: at its kill point, on a failure, or by a panic.
struct ChildIncarnation(std::process::Child);

impl Drop for ChildIncarnation {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn harness_failure(step: &str, detail: impl std::fmt::Display) -> ConformanceFailure {
    ConformanceFailure::new("core_long_run_harness", step, detail.to_string())
}

impl Harness {
    fn new(backend: Backend) -> Self {
        Self {
            backend,
            base: tempfile::Builder::new()
                .prefix("meerkat-long-run-")
                .tempdir()
                .expect("fixture storage in the system temp dir"),
            rows: AtomicU32::new(0),
            current: Mutex::new(None),
        }
    }

    /// Run a fresh row's first incarnation until its kill point, kill it,
    /// and read what it left.
    async fn kill_row(&self, plan: &LongRunPlan) -> Result<KilledState, ConformanceFailure> {
        let root = fresh_root(self.base.path(), &self.rows);
        let store = match self.backend {
            Backend::Sqlite => {
                let store = Store::Sqlite { root: root.clone() };
                Self::kill_child(&store, plan).await?;
                store
            }
            Backend::Host => {
                let store = Store::fresh_host(root.clone());
                Self::kill_in_process(&store, plan).await?;
                store
            }
        };
        let killed = read_killed(&root).map_err(|error| harness_failure("read_killed", error))?;
        *self.current.lock().unwrap() = Some((store, killed.clone()));
        Ok(killed)
    }

    /// The SQLite backend: a child process killed with SIGKILL. A compaction
    /// race's other writer runs here, in the parent, over the same files.
    async fn kill_child(store: &Store, plan: &LongRunPlan) -> Result<(), ConformanceFailure> {
        let root = store.root().to_path_buf();
        write_plan(&root, plan);
        let log = std::fs::File::create(root.join("child.log"))
            .map_err(|error| harness_failure("child_log", error))?;
        let mut child = ChildIncarnation(
            std::process::Command::new(
                std::env::current_exe().map_err(|error| harness_failure("current_exe", error))?,
            )
            .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
            .env(ROOT_ENV, &root)
            .stdout(
                log.try_clone()
                    .map_err(|error| harness_failure("child_log", error))?,
            )
            .stderr(log)
            .spawn()
            .map_err(|error| harness_failure("spawn_child", error))?,
        );
        // A deploy row's child gets SIGTERM once every worker is in flight,
        // as the host's pod does, and runs its graceful shutdown.
        let deploy = DeployConfig::read(&root);
        let bound = if deploy.is_some() {
            STEP + SHUTDOWN_BUDGET
        } else {
            STEP
        };
        let mut terminated = false;
        let started = Instant::now();
        let outcome = loop {
            if root.join("killed").exists() {
                break Ok(());
            }
            if deploy.is_some() && !terminated && root.join("in-flight").exists() {
                let _ = std::process::Command::new("kill")
                    .args(["-TERM", &child.0.id().to_string()])
                    .status();
                terminated = true;
            }
            if root.join("race").exists() && !root.join("race-done").exists() {
                let session = std::fs::read_to_string(root.join("session-id"))
                    .ok()
                    .and_then(|session| SessionId::parse(session.trim()).ok());
                match session {
                    Some(session) => {
                        other_writer(store, &session).await;
                        std::fs::write(root.join("race-done"), "done")
                            .expect("release the raced child");
                    }
                    None => break Err("the raced child never recorded its session".to_string()),
                }
            }
            match child.0.try_wait() {
                // A deploy row's child may end on its own right after its
                // stop.
                Ok(Some(_)) if root.join("killed").exists() => break Ok(()),
                Ok(Some(status)) => {
                    break Err(format!(
                        "the child exited ({status}) before its kill point: {}",
                        std::fs::read_to_string(root.join("child.log")).unwrap_or_default()
                    ));
                }
                Ok(None) => {}
                Err(error) => break Err(format!("poll the child: {error}")),
            }
            if started.elapsed() > bound {
                break Err(format!(
                    "the child never reached its kill point: {}",
                    std::fs::read_to_string(root.join("child.log")).unwrap_or_default()
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        // SIGKILL: the incarnation ends without any shutdown or cancellation.
        drop(child);
        outcome.map_err(|error| harness_failure("kill_child", error))
    }

    /// The host store backend: the first incarnation runs on a runtime of
    /// its own, shut down at the kill point.
    ///
    /// This approximates a process death only in part. The incarnation's
    /// session tasks, and so the turn in flight with its tool calls, run on
    /// that runtime and end with it. Core's runtime loops run on a runtime
    /// the process owns, which this shutdown leaves running: a loop whose
    /// turn the shutdown took writes nothing more for the run and leaves its
    /// session's registration to a cold reload, as a crash would, and the
    /// killed incarnation's other Core objects stay in this process, fenced
    /// out by the restart's takeover. A death would also have ended the tool
    /// process a started call left running, by the next start's recovery;
    /// this process outlives the call, so the harness stops that tool
    /// process itself at the kill (see [`kill_orphaned_process`]) and leaves
    /// the call's custody record to the restart. The SQLite backend, a child
    /// process killed with SIGKILL, is the proof of the death itself.
    async fn kill_in_process(store: &Store, plan: &LongRunPlan) -> Result<(), ConformanceFailure> {
        let root = store.root().to_path_buf();
        let kill = KillSwitch::new(&root);
        let kill_switch = Arc::clone(&kill);
        let mut fired = kill.subscribe();
        let incarnation_runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .map_err(|error| harness_failure("incarnation_runtime", error))?;
        let race: Arc<dyn RaceHook> = Arc::new(HostRace {
            store: store.clone(),
        });
        let incarnation_store = store.clone();
        let plan = *plan;
        let resumed = root.join(RESUMED_KILL).exists();
        incarnation_runtime.spawn(async move {
            if resumed {
                run_resumed_until_killed(&incarnation_store, plan, kill).await;
            } else {
                run_until_killed(&incarnation_store, plan, kill, Some(race), true).await;
            }
        });
        // A deploy row's incarnation gets the stand-in for SIGTERM once
        // every worker is in flight, and runs its graceful shutdown.
        if kill_switch.deploy().is_some() {
            let mut in_flight = kill_switch.subscribe_in_flight();
            let in_flight =
                tokio::time::timeout(STEP, in_flight.wait_for(|in_flight| *in_flight)).await;
            if !matches!(in_flight, Ok(Ok(_))) {
                incarnation_runtime.shutdown_background();
                return Err(harness_failure(
                    "kill_in_process",
                    "the deploy row's workers never reached their stop point",
                ));
            }
            kill_switch.terminate();
        }
        // Read as a bool at once: the watch borrow `wait_for` returns holds
        // the channel's read lock and is not `Send`, so it must not live
        // across the route release's await below.
        let reached = matches!(
            tokio::time::timeout(STEP + SHUTDOWN_BUDGET, fired.wait_for(|fired| *fired)).await,
            Ok(Ok(_))
        );
        incarnation_runtime.shutdown_background();
        // The started tool process the killed incarnation's call left
        // running: ended here, as the next start's recovery ends it after a
        // death. The call itself ended with the incarnation's runtime, and
        // the killed session's runtime loop ends without writing.
        kill_orphaned_process(&root);
        if !reached {
            return Err(harness_failure(
                "kill_in_process",
                "the incarnation never reached its kill point",
            ));
        }
        // A process exit frees the members' comms names and the mob's
        // supervisor route. The killed incarnation's objects outlive its
        // runtime in this process, so the harness frees the names itself,
        // writing nothing.
        kill_switch.release_comms();
        let mob_id = std::fs::read_to_string(root.join("mob-id"))
            .map_err(|error| harness_failure("mob_id", error))?;
        await_supervisor_route_release(mob_id.trim())
            .await
            .map_err(|error| harness_failure("kill_in_process", error))
    }

    /// Run a fresh deploy row's first incarnation until its graceful stop
    /// (or its kill mid-stop) and read what it left.
    async fn deploy_row(
        &self,
        plan: &LongRunPlan,
        deploy: DeployConfig,
    ) -> Result<DeployedRow, ConformanceFailure> {
        let root = fresh_root(self.base.path(), &self.rows);
        deploy.write(&root);
        let store = match self.backend {
            Backend::Sqlite => Store::Sqlite { root: root.clone() },
            Backend::Host => Store::fresh_host(root.clone()),
        };
        match self.backend {
            Backend::Sqlite => Self::kill_child(&store, plan).await?,
            Backend::Host => Self::kill_in_process(&store, plan).await?,
        }
        let workers = deploy
            .members()
            .iter()
            .map(|member| read_killed_member(&root, member).map(|killed| (*member, killed)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| harness_failure("read_killed", error))?;
        let account = std::fs::read_to_string(root.join("stopped"))
            .ok()
            .map(|text| serde_json::from_str(&text).expect("the stop's account"));
        Ok(DeployedRow {
            store,
            workers,
            account,
        })
    }

    async fn restart_row(
        &self,
        plan: &LongRunPlan,
        gate: Option<Arc<HarnessGate>>,
    ) -> Result<Restarted, ConformanceFailure> {
        let (store, killed) = self
            .current
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| harness_failure("restart", "no row was killed"))?;
        restart(&store, *plan, gate, &killed, self.backend == Backend::Host)
            .await
            .map_err(|error| harness_failure("restart", error))
    }
}

#[async_trait::async_trait]
impl LongRunHarness for Harness {
    fn fence_leases(&self) -> bool {
        self.backend == Backend::Host
    }

    async fn run_and_kill(&self, plan: &LongRunPlan) -> Result<KilledRun, ConformanceFailure> {
        Ok(self.kill_row(plan).await?.run)
    }

    async fn restart(&self, plan: &LongRunPlan) -> Result<RestartReport, ConformanceFailure> {
        // The gate holds the resume while the plan's before-resume action
        // lands, and then lets it continue.
        let gate = HarnessGate::new(
            ResumeGate::Continue,
            plan.before_resume == BeforeResume::Nothing,
        );
        Ok(self.restart_row(plan, Some(gate)).await?.report)
    }
}

#[async_trait::async_trait]
impl DurableWorkerHarness for Harness {
    fn fence_leases(&self) -> bool {
        self.backend == Backend::Host
    }

    async fn spawn_and_kill(&self, plan: &LongRunPlan) -> Result<KilledWorker, ConformanceFailure> {
        let killed = self.kill_row(plan).await?;
        Ok(KilledWorker {
            identity: WORKER.to_string(),
            session: killed.session.to_string(),
            run: killed.run,
        })
    }

    async fn restart(
        &self,
        plan: &LongRunPlan,
        gate: ResumeGate,
    ) -> Result<RestartedWorker, ConformanceFailure> {
        let gate = match gate {
            ResumeGate::Absent => None,
            verdict => Some(HarnessGate::new(verdict, true)),
        };
        let restarted = self.restart_row(plan, gate).await?;
        Ok(RestartedWorker {
            identity: WORKER.to_string(),
            session: restarted.session.to_string(),
            refused: restarted.refused,
            report: restarted.report,
        })
    }
}

fn pass(result: Result<(), ConformanceFailure>) {
    if let Err(failure) = result {
        panic!("{failure}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn long_run_over_the_sqlite_realm() {
    pass(chapters::long_run(&Harness::new(Backend::Sqlite)).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compacted_long_run_over_the_sqlite_realm() {
    pass(chapters::compacted_long_run(&Harness::new(Backend::Sqlite)).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_worker_over_the_sqlite_realm() {
    pass(chapters::durable_worker(&Harness::new(Backend::Sqlite)).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn long_run_over_the_reference_host_store() {
    pass(chapters::long_run(&Harness::new(Backend::Host)).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compacted_long_run_over_the_reference_host_store() {
    pass(chapters::compacted_long_run(&Harness::new(Backend::Host)).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_worker_over_the_reference_host_store() {
    pass(chapters::durable_worker(&Harness::new(Backend::Host)).await);
}

/// Bound within which a restart takes a killed process's identity lease:
/// well inside the 30 s lifetime a restart that cannot prove the exit waits
/// out.
const RECLAIM_BOUND: Duration = Duration::from_secs(2);

/// An identity-first durable worker killed with SIGKILL mid run: the restart
/// in a fresh process takes the killed process's identity reconcile lease at
/// once (the earlier incarnation's process ended), and the worker's run
/// resumes there instead of starting over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_killed_identity_first_worker_reclaims_its_lease_and_resumes_at_once() {
    let harness = Harness::new(Backend::Sqlite);
    let plan = LongRunPlan {
        rounds: 4,
        calls_per_round: 1,
        kill_round: 1,
        kill_point: KillPoint::DuringTools,
        compactions: RoundSet::EMPTY,
        image_rounds: RoundSet::EMPTY,
        before_resume: BeforeResume::Nothing,
    };
    let root = fresh_root(harness.base.path(), &harness.rows);
    std::fs::write(root.join("adopt"), "adopt").expect("ask for an identity-first worker");
    let store = Store::Sqlite { root: root.clone() };
    if let Err(failure) = Harness::kill_child(&store, &plan).await {
        panic!("{failure}");
    }
    let killed = read_killed(&root).expect("what the killed child left");
    let killed_epoch: u64 = std::fs::read_to_string(root.join("lease-epoch"))
        .expect("the killed child's lease epoch")
        .trim()
        .parse()
        .expect("lease epoch");

    let gate = HarnessGate::new(ResumeGate::Continue, true);
    let incarnation = Incarnation::open(
        &store,
        OpenOptions {
            plan,
            restart: true,
            kill: None,
            race: None,
            gate: Some(gate),
            log_to_file: false,
        },
    )
    .await;
    if let Err(latest) = wait_converged(&incarnation.mob, WORKER, killed_epoch, RECLAIM_BOUND).await
    {
        panic!("the restart takes the killed process's lease at once: {latest}");
    }
    assert!(
        incarnation
            .wait_terminal(&killed.session, &killed.input, STEP)
            .await,
        "the worker's input finishes"
    );
    let report = long_run_support::report(&incarnation, &killed.session, &killed, false)
        .await
        .expect("read the restart");
    assert!(resumes(&plan));
    let resumed = report.resumed.expect("the worker's run resumes");
    assert_eq!(resumed.run_id, killed.run.run_id, "the same run resumes");
    assert_eq!(report.receipts, 1, "the input has one receipt");
    assert!(report.inputs_settled);
    incarnation.shut_down().await;
}

/// The discarded-tail rows' plan: the process is killed during the tools of
/// a round after two committed rounds.
const DISCARDED_TAIL_PLAN: LongRunPlan = LongRunPlan {
    rounds: 4,
    calls_per_round: 1,
    kill_round: 2,
    kill_point: KillPoint::DuringTools,
    compactions: RoundSet::EMPTY,
    image_rounds: RoundSet::EMPTY,
    before_resume: BeforeResume::Nothing,
};

/// A run's inputs, once its provisional tail is discarded, run again
/// exactly once: the state a crash leaves after a failed resumed run's tail
/// is discarded and before the runtime requeues its inputs. The process is
/// killed after the run committed rounds, the tail is discarded with no
/// runtime running, and the restart finds nothing to resume: the input runs
/// as a new run, its prompt is in the transcript once, it has one receipt,
/// and no provisional tail of the killed run remains. Only a whole-blob
/// store discards a tail; on a head-canonical one the row below holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_discarded_tail_replays_its_input_exactly_once() {
    let harness = Harness::new(Backend::Sqlite);
    let plan = DISCARDED_TAIL_PLAN;
    let root = fresh_root(harness.base.path(), &harness.rows);
    std::fs::write(root.join(WHOLE_BLOB), "whole-blob").expect("ask for a whole-blob realm");
    let store = Store::Sqlite { root: root.clone() };
    if let Err(failure) = Harness::kill_child(&store, &plan).await {
        panic!("{failure}");
    }
    let killed = read_killed(&root).expect("what the killed child left");
    assert!(
        discard_provisional_tail(&store, &killed.session)
            .await
            .expect("discard the killed run's tail"),
        "the killed run left a provisional tail"
    );
    let restarted = restart(
        &store,
        plan,
        Some(HarnessGate::new(ResumeGate::Continue, true)),
        &killed,
        false,
    )
    .await
    .expect("the restart runs the input again");
    let report = &restarted.report;
    assert!(
        report.resumed.is_none(),
        "nothing resumes: {:?}",
        report.resumed
    );
    assert_eq!(report.runs.len(), 1, "{:?}", report.runs);
    assert_ne!(
        report.runs[0], killed.run.run_id,
        "the input runs as a new run"
    );
    assert_eq!(
        restarted.main_prompts, 1,
        "the prompt is in the transcript once"
    );
    assert_eq!(report.receipts, 1, "the input has one receipt");
    assert!(report.inputs_settled);
    assert_ne!(
        restarted.provisional_tail_run.as_ref(),
        Some(&killed.run.run_id),
        "no provisional tail of the killed run remains"
    );
}

/// The head-canonical side of the row above: each round commit applies the
/// run's tail to the physical head, so the store refuses to discard it, and
/// the restart's durable-tail recovery takes the killed run over from its
/// last committed round instead: the input runs exactly once, its prompt is
/// in the transcript once, it has one receipt, and no tail remains.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_applied_tail_is_not_discarded_and_its_input_runs_once() {
    let harness = Harness::new(Backend::Sqlite);
    let plan = DISCARDED_TAIL_PLAN;
    let root = fresh_root(harness.base.path(), &harness.rows);
    let store = Store::Sqlite { root: root.clone() };
    if let Err(failure) = Harness::kill_child(&store, &plan).await {
        panic!("{failure}");
    }
    let killed = read_killed(&root).expect("what the killed child left");
    let refusal = discard_provisional_tail(&store, &killed.session)
        .await
        .expect_err("the store refuses to discard the applied tail");
    assert!(
        refusal.contains("already-applied"),
        "the refusal names the applied tail: {refusal}"
    );
    let restarted = restart(
        &store,
        plan,
        Some(HarnessGate::new(ResumeGate::Continue, true)),
        &killed,
        false,
    )
    .await
    .expect("the restart runs the input");
    let report = &restarted.report;
    assert_eq!(
        report.resumed.as_ref().map(|resumed| &resumed.run_id),
        Some(&killed.run.run_id),
        "the killed run resumes over its kept tail"
    );
    assert_eq!(report.runs.len(), 1, "{:?}", report.runs);
    assert_eq!(
        restarted.main_prompts, 1,
        "the prompt is in the transcript once"
    );
    assert_eq!(report.receipts, 1, "the input has one receipt");
    assert!(report.inputs_settled);
    assert_eq!(
        restarted.provisional_tail_run, None,
        "no provisional tail remains"
    );
}

/// A deploy row's first incarnation, ended the way a host deploys.
struct DeployedRow {
    store: Store,
    workers: Vec<(&'static str, KilledState)>,
    /// The stop's account; `None` when the process was killed mid-stop.
    account: Option<serde_json::Value>,
}

/// The deploy rows' plan: five rounds of two calls, stopped in `kill_round`.
const fn deploy_plan(kill_round: u64, kill_point: KillPoint) -> LongRunPlan {
    LongRunPlan {
        rounds: 5,
        calls_per_round: 2,
        kill_round,
        kill_point,
        compactions: RoundSet::EMPTY,
        image_rounds: RoundSet::EMPTY,
        before_resume: BeforeResume::Nothing,
    }
}

/// The calls of `round`, in emission order; none for the last round.
fn calls_of(plan: &LongRunPlan, round: u64) -> Vec<String> {
    if round + 1 >= plan.rounds {
        return Vec::new();
    }
    (0..plan.calls_per_round)
        .map(|index| call_id(round, index))
        .collect()
}

/// A graceful deploy stop: the Shutdown returned inside the host's budget,
/// and every worker's waiter was told its run was kept for the next start.
fn check_graceful_stop(row: &DeployedRow) -> Result<(), String> {
    let account = row.account.as_ref().ok_or("the stop left no account")?;
    if account["shutdown"] != "ok" {
        return Err(format!("the Shutdown did not finish: {account}"));
    }
    if account["elapsed_ms"]
        .as_u64()
        .map(Duration::from_millis)
        .is_none_or(|elapsed| elapsed >= SHUTDOWN_BUDGET)
    {
        return Err(format!("the Shutdown overran the host's budget: {account}"));
    }
    for (member, _) in &row.workers {
        if account["waiters"][*member] != "requeued_across_stop" {
            return Err(format!(
                "{member}'s waiter was not told its run was kept: {account}"
            ));
        }
    }
    Ok(())
}

/// A worker whose run the deploy kept resumed it at its last committed
/// round, exactly as after a crash: the same run, the model asked only for
/// the rounds after that commit, each later call dispatched once (a started
/// built-in process answered as interrupted instead), every round and every
/// result once in the transcript, one receipt, its prompt once, and a new
/// input run to its end after it.
fn check_resumed(
    plan: &LongRunPlan,
    killed: &KilledState,
    restarted: &DeployRestart,
) -> Result<(), String> {
    let member = restarted.member;
    let report = &restarted.report;
    let kill_round = plan.kill_round;
    let tools_pending = plan.kill_point != KillPoint::ModelCall;
    let expected = if tools_pending {
        ResumedRun {
            run_id: killed.run.run_id.clone(),
            round: kill_round,
            resume_point: ResumePointKind::ToolsPending,
        }
    } else {
        ResumedRun {
            run_id: killed.run.run_id.clone(),
            round: kill_round - 1,
            resume_point: ResumePointKind::AwaitingModel,
        }
    };
    if report.resumed.as_ref() != Some(&expected) {
        return Err(format!(
            "{member} resumed {:?}, expected {expected:?}",
            report.resumed
        ));
    }
    if report.runs != [killed.run.run_id.clone()] {
        return Err(format!(
            "{member} ran {:?}; only its kept run {} runs",
            report.runs, killed.run.run_id
        ));
    }
    let first_model_round = if tools_pending {
        kill_round + 1
    } else {
        kill_round
    };
    let expected_rounds: Vec<u64> = (first_model_round..plan.rounds).collect();
    if report.model_call_rounds != expected_rounds {
        return Err(format!(
            "{member} called the model for rounds {:?}, expected {expected_rounds:?}",
            report.model_call_rounds
        ));
    }
    let interrupted = match plan.kill_point {
        KillPoint::DuringUnrepeatableTool(_) => vec![call_id(kill_round, 0)],
        _ => Vec::new(),
    };
    if report.interrupted != interrupted {
        return Err(format!(
            "{member} answered {:?} as interrupted, expected {interrupted:?}",
            report.interrupted
        ));
    }
    let mut expected_calls: Vec<(u64, String)> = (kill_round..plan.rounds)
        .flat_map(|round| {
            calls_of(plan, round)
                .into_iter()
                .map(move |call| (round, call))
        })
        .filter(|(_, call)| !interrupted.contains(call))
        .collect();
    let mut dispatched: Vec<(u64, String)> = report
        .dispatched
        .iter()
        .map(|key| (key.round, key.call_id.clone()))
        .collect();
    expected_calls.sort();
    dispatched.sort();
    if dispatched != expected_calls {
        return Err(format!(
            "{member} dispatched {dispatched:?}, expected each of {expected_calls:?} once"
        ));
    }
    // A call the stopped incarnation dispatched in its stop round comes back
    // under the same key, digest included.
    for key in &killed.run.dispatched {
        if key.round == kill_round
            && !interrupted.contains(&key.call_id)
            && !report.dispatched.contains(key)
        {
            return Err(format!(
                "{member}'s call {key:?} was not dispatched again under the same key"
            ));
        }
    }
    let every_round: Vec<u64> = (0..plan.rounds).collect();
    if report.transcript_rounds != every_round {
        return Err(format!(
            "{member}'s transcript holds rounds {:?}, expected {every_round:?}",
            report.transcript_rounds
        ));
    }
    let mut results = report.transcript_results.clone();
    results.sort();
    let mut every_call: Vec<String> = (0..plan.rounds)
        .flat_map(|round| calls_of(plan, round))
        .collect();
    every_call.sort();
    if results != every_call {
        return Err(format!(
            "{member}'s transcript holds results {results:?}, expected each of {every_call:?} once"
        ));
    }
    if report.receipts != 1
        || !report.inputs_settled
        || restarted.terminal != Some(meerkat_runtime::InputTerminalOutcome::Consumed)
        || restarted.prompts != 1
    {
        return Err(format!(
            "{member}'s input finished {:?} with {} receipts and its prompt {} times; \
             expected consumed, one receipt and one prompt",
            restarted.terminal, report.receipts, restarted.prompts
        ));
    }
    if !restarted.follow_up_consumed {
        return Err(format!(
            "{member} did not run a new input after the restart"
        ));
    }
    Ok(())
}

/// A worker whose kept run's requeue bound passed before the restart: the
/// run is settled as expired instead of resumed, nothing of it runs again,
/// its session's stream ends it with one cancelled `RunFailed`, and a new
/// input still runs.
fn check_expired(restarted: &DeployRestart) -> Result<(), String> {
    let member = restarted.member;
    let report = &restarted.report;
    let expired = meerkat_runtime::InputTerminalOutcome::Abandoned {
        reason: meerkat_runtime::InputAbandonReason::RequeueExpired,
    };
    if restarted.terminal.as_ref() != Some(&expired) {
        return Err(format!(
            "{member}'s input ended {:?}, expected {expired:?}",
            restarted.terminal
        ));
    }
    if report.resumed.is_some()
        || !report.runs.is_empty()
        || !report.model_call_rounds.is_empty()
        || !report.dispatched.is_empty()
        || report.receipts != 0
    {
        return Err(format!(
            "{member}'s expired run ran again: resumed {:?}, runs {:?}, model rounds {:?}, \
             dispatched {:?}, receipts {}",
            report.resumed,
            report.runs,
            report.model_call_rounds,
            report.dispatched,
            report.receipts
        ));
    }
    let expired = settled_run_report(meerkat_core::lifecycle::SettledRunCause::RequeueExpired);
    if restarted.killed_run_failures != [expired.clone()] {
        return Err(format!(
            "{member}'s expired run ended on its stream with {:?}, expected exactly {expired:?}",
            restarted.killed_run_failures
        ));
    }
    if !restarted.follow_up_consumed {
        return Err(format!(
            "{member} did not run a new input after the restart"
        ));
    }
    Ok(())
}

/// A deploy stop of one durable worker whose run is in flight, with no
/// queued-input choice: in round 2's tools, at round 2's model call, and in
/// round 2's started shell call. The Shutdown keeps the run at its last
/// committed round, the worker's waiter is told so, and the restart resumes
/// it there and finishes it, with no input run twice.
async fn a_deploy_stop_resumes_the_run(backend: Backend) {
    let harness = Harness::new(backend);
    for (step, plan) in [
        ("during_tools", deploy_plan(2, KillPoint::DuringTools)),
        ("at_a_model_call", deploy_plan(2, KillPoint::ModelCall)),
        (
            "in_a_started_process",
            deploy_plan(
                2,
                KillPoint::DuringUnrepeatableTool(UnrepeatableTool::BuiltInProcess),
            ),
        ),
    ] {
        let deploy = DeployConfig {
            workers: 1,
            max_age: None,
            kill_mid_suspend: false,
        };
        let row = harness
            .deploy_row(&plan, deploy)
            .await
            .unwrap_or_else(|failure| panic!("{step}: {failure}"));
        if let Err(error) = check_graceful_stop(&row) {
            panic!("{step}: {error}");
        }
        let restarted = restart_deploy(&row.store, plan, &row.workers, backend == Backend::Host)
            .await
            .unwrap_or_else(|error| panic!("{step}: {error}"));
        for (restart, (_, killed)) in restarted.iter().zip(&row.workers) {
            if let Err(error) = check_resumed(&plan, killed, restart) {
                panic!("{step}: {error}");
            }
        }
    }
}

/// A deploy stop whose requeue bound (two seconds) passes before the restart:
/// the waiter is told the run was kept, and the next start settles the run
/// as expired instead of resuming it, repairing its tail.
async fn a_deploy_stop_past_its_bound_settles_the_run_expired(backend: Backend) {
    let harness = Harness::new(backend);
    let plan = deploy_plan(1, KillPoint::DuringTools);
    let max_age = Duration::from_secs(2);
    let deploy = DeployConfig {
        workers: 1,
        max_age: Some(max_age),
        kill_mid_suspend: false,
    };
    let row = harness
        .deploy_row(&plan, deploy)
        .await
        .unwrap_or_else(|failure| panic!("{failure}"));
    if let Err(error) = check_graceful_stop(&row) {
        panic!("{error}");
    }
    // The bound, measured from the input's admission before the stop,
    // passes before the next start.
    tokio::time::sleep(max_age + Duration::from_secs(1)).await;
    let restarted = restart_workers(
        &row.store,
        plan,
        &row.workers,
        backend == Backend::Host,
        None,
        FollowUp::Scripted,
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    for restart in &restarted {
        // The settled run's tail is repaired, never discarded.
        if let Err(error) =
            check_expired(restart).and_then(|()| check_tail_repaired(&plan, restart))
        {
            panic!("{error}");
        }
    }
}

/// The host's deploy: SIGTERM reaches a process whose two durable workers
/// both have a run in flight, the mob's terminal Shutdown (no
/// queued-input choice) returns inside the host's 75-second budget, the
/// process ends, and the restart resumes every run at its last committed
/// round.
async fn a_host_deploy_resumes_every_in_flight_run(backend: Backend) {
    let harness = Harness::new(backend);
    let plan = deploy_plan(2, KillPoint::DuringTools);
    let deploy = DeployConfig {
        workers: 2,
        max_age: None,
        kill_mid_suspend: false,
    };
    let row = harness
        .deploy_row(&plan, deploy)
        .await
        .unwrap_or_else(|failure| panic!("{failure}"));
    if let Err(error) = check_graceful_stop(&row) {
        panic!("{error}");
    }
    let restarted = restart_deploy(&row.store, plan, &row.workers, backend == Backend::Host)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(restarted.len(), 2, "both workers restart");
    for (restart, (_, killed)) in restarted.iter().zip(&row.workers) {
        if let Err(error) = check_resumed(&plan, killed, restart) {
            panic!("{error}");
        }
    }
}

/// The host's deploy overrunning its budget: the same, but the process is
/// killed while the Shutdown drops the last worker's in-flight call, so the
/// stop records nothing. Whatever part of the stop landed, the restart is
/// crash-equivalent: every run resumes at its last committed round, no stop
/// or resume mark is left stuck, and no input runs twice.
async fn a_host_deploy_killed_mid_suspend_still_resumes_every_run(backend: Backend) {
    let harness = Harness::new(backend);
    let plan = deploy_plan(2, KillPoint::DuringTools);
    let deploy = DeployConfig {
        workers: 2,
        max_age: None,
        kill_mid_suspend: true,
    };
    let row = harness
        .deploy_row(&plan, deploy)
        .await
        .unwrap_or_else(|failure| panic!("{failure}"));
    assert!(
        row.account.is_none(),
        "the process was killed before the Shutdown returned: {:?}",
        row.account
    );
    let restarted = restart_deploy(&row.store, plan, &row.workers, backend == Backend::Host)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(restarted.len(), 2, "both workers restart");
    for (restart, (_, killed)) in restarted.iter().zip(&row.workers) {
        if let Err(error) = check_resumed(&plan, killed, restart) {
            panic!("{error}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deploy_stop_resumes_the_run_over_the_sqlite_realm() {
    a_deploy_stop_resumes_the_run(Backend::Sqlite).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deploy_stop_resumes_the_run_over_the_reference_host_store() {
    a_deploy_stop_resumes_the_run(Backend::Host).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deploy_stop_past_its_bound_settles_the_run_expired_over_the_sqlite_realm() {
    a_deploy_stop_past_its_bound_settles_the_run_expired(Backend::Sqlite).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deploy_stop_past_its_bound_settles_the_run_expired_over_the_reference_host_store() {
    a_deploy_stop_past_its_bound_settles_the_run_expired(Backend::Host).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_deploy_resumes_every_in_flight_run_over_the_sqlite_realm() {
    a_host_deploy_resumes_every_in_flight_run(Backend::Sqlite).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_deploy_resumes_every_in_flight_run_over_the_reference_host_store() {
    a_host_deploy_resumes_every_in_flight_run(Backend::Host).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_deploy_killed_mid_suspend_still_resumes_every_run_over_the_sqlite_realm() {
    a_host_deploy_killed_mid_suspend_still_resumes_every_run(Backend::Sqlite).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_deploy_killed_mid_suspend_still_resumes_every_run_over_the_reference_host_store() {
    a_host_deploy_killed_mid_suspend_still_resumes_every_run(Backend::Host).await;
}

/// A run whose kept or killed tail was settled instead of resumed has its
/// tail repaired, never discarded: rounds 0 to the stop round are in the
/// committed transcript (that round's message with its calls answered as
/// interrupted), the interrupted-run notice follows, and the session's next
/// run, a full scripted one, takes its round commits and finishes.
fn check_tail_repaired(plan: &LongRunPlan, restarted: &DeployRestart) -> Result<(), String> {
    let member = restarted.member;
    let report = &restarted.report;
    let stop_round = plan.kill_round;
    let rounds: Vec<u64> = (0..=stop_round).collect();
    if report.transcript_rounds != rounds {
        return Err(format!(
            "{member}'s repaired transcript holds rounds {:?}, expected {rounds:?}",
            report.transcript_rounds
        ));
    }
    let mut interrupted = report.interrupted.clone();
    interrupted.sort();
    let mut stop_calls = calls_of(plan, stop_round);
    stop_calls.sort();
    if interrupted != stop_calls {
        return Err(format!(
            "{member}'s repair answered {interrupted:?} as interrupted, expected {stop_calls:?}"
        ));
    }
    let mut results = report.transcript_results.clone();
    results.sort();
    let mut kept_calls: Vec<String> = (0..=stop_round)
        .flat_map(|round| calls_of(plan, round))
        .collect();
    kept_calls.sort();
    if results != kept_calls {
        return Err(format!(
            "{member}'s repaired transcript holds results {results:?}, expected each of \
             {kept_calls:?} once"
        ));
    }
    if restarted.notices == 0 {
        return Err(format!(
            "{member}'s repaired transcript has no interrupted-run notice"
        ));
    }
    if !restarted.follow_up_consumed {
        return Err(format!(
            "{member}'s next run did not take its round commits and finish"
        ));
    }
    Ok(())
}

/// A durable worker killed in round 2's tools whose resume the gate
/// refuses (superseded) settles its input as refused, ends the run on its
/// session's stream with one cancelled `RunFailed`, and has its tail
/// repaired, never discarded, and its session's next run takes its round
/// commits.
async fn a_refused_resume_repairs_the_tail(backend: Backend) {
    let harness = Harness::new(backend);
    let plan = deploy_plan(2, KillPoint::DuringTools);
    let killed = harness
        .kill_row(&plan)
        .await
        .unwrap_or_else(|failure| panic!("{failure}"));
    let (store, _) = harness
        .current
        .lock()
        .unwrap()
        .take()
        .expect("the killed row");
    let restarted = restart_workers(
        &store,
        plan,
        &[(WORKER, killed)],
        backend == Backend::Host,
        Some(HarnessGate::new(ResumeGate::Superseded, true)),
        FollowUp::Scripted,
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    let restarted = &restarted[0];
    assert_eq!(
        (restarted.terminal.clone(), restarted.report.resumed.clone()),
        (
            Some(meerkat_runtime::InputTerminalOutcome::Abandoned {
                reason: meerkat_runtime::InputAbandonReason::ResumeRefused,
            }),
            None,
        ),
        "the refused run's input ends refused and nothing resumes"
    );
    assert_eq!(
        restarted.killed_run_failures,
        [settled_run_report(
            meerkat_core::lifecycle::SettledRunCause::ResumeRefused
        )],
        "the refused run ends once on its session's stream, as refused"
    );
    if let Err(error) = check_tail_repaired(&plan, restarted) {
        panic!("{error}");
    }
}

/// A durable worker killed in round 2's tools whose runtime is stopped
/// at the gate, before its marked run resumes: the stop settles the run's
/// input as cancelled, and its tail is repaired, never discarded, by the
/// settlement or by the next registration (a further restart reads it).
async fn a_stop_before_the_resume_repairs_the_tail(backend: Backend) {
    let harness = Harness::new(backend);
    let mut plan = deploy_plan(2, KillPoint::DuringTools);
    plan.before_resume = BeforeResume::Stop;
    let killed = harness
        .kill_row(&plan)
        .await
        .unwrap_or_else(|failure| panic!("{failure}"));
    let (store, _) = harness
        .current
        .lock()
        .unwrap()
        .take()
        .expect("the killed row");
    let stopped = restart(
        &store,
        plan,
        Some(HarnessGate::new(ResumeGate::Continue, false)),
        &killed,
        backend == Backend::Host,
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        stopped.report.resumed.is_none(),
        "the stopped run does not resume: {:?}",
        stopped.report.resumed
    );
    let restarted = restart_workers(
        &store,
        plan,
        &[(WORKER, killed)],
        backend == Backend::Host,
        None,
        FollowUp::Scripted,
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    let restarted = &restarted[0];
    assert_eq!(
        (restarted.terminal.clone(), restarted.report.resumed.clone()),
        (
            Some(meerkat_runtime::InputTerminalOutcome::Abandoned {
                reason: meerkat_runtime::InputAbandonReason::Cancelled,
            }),
            None,
        ),
        "the stop cancels the marked run's input, and nothing resumes after it"
    );
    if let Err(error) = check_tail_repaired(&plan, restarted) {
        panic!("{error}");
    }
}

/// The stop marks of a kept run clear when it resumes. A deploy stop
/// with a 30-second requeue bound keeps a run in round 1; a restart well
/// inside the bound resumes it; that incarnation is killed in round 3, and
/// the next restart comes after the bound has passed since the input's
/// admission. The run resumes again, at round 3: it is not settled as
/// expired.
async fn a_resumed_runs_stop_marks_do_not_expire_it_later(backend: Backend) {
    let harness = Harness::new(backend);
    let bound = Duration::from_secs(30);
    let first = deploy_plan(1, KillPoint::DuringTools);
    let deploy = DeployConfig {
        workers: 1,
        max_age: Some(bound),
        kill_mid_suspend: false,
    };
    let row = harness
        .deploy_row(&first, deploy)
        .await
        .unwrap_or_else(|failure| panic!("{failure}"));
    if let Err(error) = check_graceful_stop(&row) {
        panic!("{error}");
    }
    let stopped_at = Instant::now();

    // The next incarnation reopens the mob, resumes the kept run and is
    // killed in round 3.
    let root = row.store.root().to_path_buf();
    for marker in ["deploy.json", "in-flight", "stopped", "killed"] {
        let _ = std::fs::remove_file(root.join(marker));
    }
    std::fs::write(root.join(RESUMED_KILL), "resume").expect("mark the resumed kill");
    let second = deploy_plan(3, KillPoint::DuringTools);
    match backend {
        Backend::Sqlite => Harness::kill_child(&row.store, &second).await,
        Backend::Host => Harness::kill_in_process(&row.store, &second).await,
    }
    .unwrap_or_else(|failure| panic!("{failure}"));
    let _ = std::fs::remove_file(root.join(RESUMED_KILL));

    // The bound passes, measured from the input's admission before the
    // first stop.
    let waited = stopped_at.elapsed();
    if waited < bound + Duration::from_secs(1) {
        tokio::time::sleep((bound + Duration::from_secs(1)).saturating_sub(waited)).await;
    }
    let restarted = restart_deploy(&row.store, second, &row.workers, backend == Backend::Host)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let restarted = &restarted[0];
    let (_, killed) = &row.workers[0];
    assert_eq!(
        (
            restarted.report.resumed.clone(),
            restarted.terminal.clone(),
            restarted.report.receipts,
            restarted.follow_up_consumed,
        ),
        (
            Some(ResumedRun {
                run_id: killed.run.run_id.clone(),
                round: 3,
                resume_point: ResumePointKind::ToolsPending,
            }),
            Some(meerkat_runtime::InputTerminalOutcome::Consumed),
            1,
            true,
        ),
        "the resumed run is resumed again after the bound, not settled as expired"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_resume_repairs_the_tail_over_the_sqlite_realm() {
    a_refused_resume_repairs_the_tail(Backend::Sqlite).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_resume_repairs_the_tail_over_the_reference_host_store() {
    a_refused_resume_repairs_the_tail(Backend::Host).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_before_the_resume_repairs_the_tail_over_the_sqlite_realm() {
    a_stop_before_the_resume_repairs_the_tail(Backend::Sqlite).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_before_the_resume_repairs_the_tail_over_the_reference_host_store() {
    a_stop_before_the_resume_repairs_the_tail(Backend::Host).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resumed_runs_stop_marks_do_not_expire_it_later_over_the_sqlite_realm() {
    a_resumed_runs_stop_marks_do_not_expire_it_later(Backend::Sqlite).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resumed_runs_stop_marks_do_not_expire_it_later_over_the_reference_host_store() {
    a_resumed_runs_stop_marks_do_not_expire_it_later(Backend::Host).await;
}

/// A WholeBlob run that fails after its restart consumes its input. The run
/// compacts before round 3, so round 3's first commit re-bases it: from then
/// on its input is in the session's committed history. The process is killed
/// in round 3's tools, the restart resumes the run there, and the run's next
/// model call fails with a provider error. The input ends consumed with the
/// run's one receipt: the run's rounds stay in the committed transcript (no
/// discard), no other run takes the input (no requeue), and the user message
/// is in the committed transcript once. The session then serves its next
/// input. Only over the reference host store: the built-in SQLite realm is
/// HeadCanonical, and a HeadCanonical run never re-bases.
///
/// The provider error ends the run as a hard-failure terminal, so the session
/// consumes the input in its machine-terminal branch, before it checks the
/// run's re-base: this row pins the outcome, and it holds whether or not the
/// resumed run kept its re-base. A failure that ends without a terminal (a
/// resumed dispatch the session refuses before it runs, for one) takes the
/// re-based run's own route instead. No step of this harness causes one, so
/// meerkat-session's unit rows pin that route and the re-base a seeded run
/// keeps.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resumed_rebased_runs_failure_consumes_its_input_over_the_reference_host_store() {
    let harness = Harness::new(Backend::Host);
    let plan = LongRunPlan {
        rounds: 6,
        calls_per_round: 2,
        kill_round: 3,
        kill_point: KillPoint::DuringTools,
        compactions: RoundSet::EMPTY.with(3),
        image_rounds: RoundSet::EMPTY,
        before_resume: BeforeResume::Nothing,
    };
    let failing_round = plan.kill_round + 1;
    let killed = harness
        .kill_row(&plan)
        .await
        .unwrap_or_else(|failure| panic!("{failure}"));
    let (store, _) = harness
        .current
        .lock()
        .unwrap()
        .take()
        .expect("the killed row");
    fail_model_call(store.root(), failing_round);
    let restarted = restart_workers(
        &store,
        plan,
        &[(WORKER, killed.clone())],
        true,
        None,
        FollowUp::Queued,
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    let restarted = &restarted[0];
    let report = &restarted.report;
    assert_eq!(
        (
            report.resumed.clone(),
            report.runs.clone(),
            report.model_call_rounds.clone(),
        ),
        (
            Some(ResumedRun {
                run_id: killed.run.run_id.clone(),
                round: plan.kill_round,
                resume_point: ResumePointKind::ToolsPending,
            }),
            vec![killed.run.run_id.clone()],
            vec![failing_round],
        ),
        "the killed run resumes at its re-based round, fails at its next model call, and no \
         other run takes its input"
    );
    assert_eq!(
        (
            restarted.terminal.clone(),
            report.receipts,
            report.inputs_settled,
        ),
        (
            Some(meerkat_runtime::InputTerminalOutcome::Consumed),
            1,
            true
        ),
        "the failed run consumes its input, with its one receipt"
    );
    assert_eq!(
        restarted.prompts, 1,
        "the user message is in the committed transcript once"
    );
    // Nothing comes before the run, so the compaction before round 3 folds
    // the run's own completed rounds, 0 and 1, into its one summary and keeps
    // the run's prompt and its newest round, 2, verbatim. The committed
    // transcript then holds that summary, rounds 2 and 3 and each of their
    // calls' results, and nothing of the run was repaired. A run that never
    // compacted, and so never re-based, fails here.
    let rounds = &report.transcript_rounds;
    assert_eq!(
        (rounds.clone(), restarted.summaries),
        (vec![plan.kill_round - 1, plan.kill_round], 1),
        "the committed transcript holds the compaction's one summary, the round it kept and the \
         resumed round"
    );
    let mut results = report.transcript_results.clone();
    results.sort();
    let mut expected: Vec<String> = rounds
        .iter()
        .flat_map(|round| calls_of(&plan, *round))
        .collect();
    expected.sort();
    assert_eq!(
        results, expected,
        "the committed transcript holds each call's result once"
    );
    assert_eq!(
        (report.interrupted.clone(), restarted.notices),
        (Vec::<String>::new(), 0),
        "nothing of the run is repaired"
    );
    assert!(
        restarted.follow_up_consumed,
        "the session runs its next input after the failure"
    );
}

/// A WholeBlob run killed before it ever compacted resumes from its
/// provisional tail and makes its first compaction after the restart. The
/// actor of a continued run is seeded from the tail's document, so that
/// session must carry the exact row lineage of the tail's bytes, as a
/// session decoded from a committed body does: then the compaction's
/// transcript rewrite commits and persists. A seed without row lineage
/// refuses every rewrite ("live transcript has no exact row-lineage
/// authority"), and the run finishes uncompacted. Only over the reference
/// host store: the built-in SQLite realm is HeadCanonical, whose continued run
/// is materialized from the store's verified rows.
///
/// The run is killed in round 2's tools and compacts before round 4. Nothing
/// comes before the run, so the compaction folds the run's own completed
/// rounds 0 to 2 into its one summary and keeps the run's prompt and its
/// newest round, 3, verbatim.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_resumed_from_its_tail_compacts_and_the_compaction_persists_over_the_reference_host_store()
 {
    let harness = Harness::new(Backend::Host);
    let plan = LongRunPlan {
        rounds: 6,
        calls_per_round: 2,
        kill_round: 2,
        kill_point: KillPoint::DuringTools,
        compactions: RoundSet::EMPTY.with(4),
        image_rounds: RoundSet::EMPTY,
        before_resume: BeforeResume::Nothing,
    };
    let killed = harness
        .kill_row(&plan)
        .await
        .unwrap_or_else(|failure| panic!("{failure}"));
    let (store, _) = harness
        .current
        .lock()
        .unwrap()
        .take()
        .expect("the killed row");
    let restarted = restart_workers(
        &store,
        plan,
        &[(WORKER, killed.clone())],
        true,
        None,
        FollowUp::Queued,
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    let restarted = &restarted[0];
    let report = &restarted.report;
    assert_eq!(
        (
            report.resumed.clone(),
            report.runs.clone(),
            report.model_call_rounds.clone(),
        ),
        (
            Some(ResumedRun {
                run_id: killed.run.run_id.clone(),
                round: plan.kill_round,
                resume_point: ResumePointKind::ToolsPending,
            }),
            vec![killed.run.run_id.clone()],
            (plan.kill_round + 1..plan.rounds).collect::<Vec<_>>(),
        ),
        "the killed run resumes from its tail with round 2's tools pending, and no other run \
         takes its input"
    );
    // The compaction ran after the restart and persisted: the committed
    // transcript holds its one summary, the round it kept and the rounds
    // after it.
    let rounds = &report.transcript_rounds;
    assert_eq!(
        (rounds.clone(), restarted.summaries),
        (vec![3, 4, 5], 1),
        "the resumed run's compaction folds rounds 0 to 2 into its one summary and persists"
    );
    let mut results = report.transcript_results.clone();
    results.sort();
    let mut expected: Vec<String> = rounds
        .iter()
        .flat_map(|round| calls_of(&plan, *round))
        .collect();
    expected.sort();
    assert_eq!(
        results, expected,
        "the committed transcript holds each kept call's result once"
    );
    assert_eq!(
        (report.interrupted.clone(), restarted.notices),
        (Vec::<String>::new(), 0),
        "nothing of the run is repaired"
    );
    assert_eq!(
        (
            restarted.terminal.clone(),
            report.receipts,
            report.inputs_settled,
            restarted.prompts,
        ),
        (
            Some(meerkat_runtime::InputTerminalOutcome::Consumed),
            1,
            true,
            1
        ),
        "the resumed run consumes its input with its one receipt, and its prompt is in the \
         committed transcript once"
    );
    assert!(
        restarted.follow_up_consumed,
        "the session runs its next input after the resumed run"
    );
}
