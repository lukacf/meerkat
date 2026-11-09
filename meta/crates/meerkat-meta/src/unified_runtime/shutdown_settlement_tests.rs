//! The terminal shutdown's budgets: the root mob's teardown waits for its
//! members' runtime unregisters within its budget, and the durable child
//! root stands down in a window of its own once the root has returned.
//! Within the root's budget the shutdown also waits for the root mob's actor
//! to exit, so a successor in this process composes on the same state
//! directory once the runtime has dropped.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use meerkat_core::types::SessionId;
use meerkat_mob::{
    AgentIdentity, MobDefinition, MobId, MobRuntimeMode, ProfileName, SpawnMemberSpec,
};
use serde_json::json;
use tokio::sync::oneshot;

use crate::child_mob_storage::{
    ChildMobRootClaim, ChildMobStandDownOutcome, ChildMobStorage, child_mob_root,
};
use crate::storage_layout::MetaStorageLayout;
use crate::{DiscoverySpec, MetaConfig, MobBootstrapSpec, MobTerminalShutdownOutcome};

use super::UnifiedRuntime;

/// How long T1's held unregister runs: past the 5 s the root teardown was
/// given before, inside the budget it has now.
const SLOW_UNREGISTER: Duration = Duration::from_secs(6);

/// Bound for a held saga's release task; it only turns a hang into a
/// failure.
const RELEASE_BOUND: Duration = Duration::from_mins(1);

/// How long the row that holds the root mob's actor exit waits for shutdown
/// before it lets the exit go: far longer than the steps shutdown runs after
/// its mob's answer, so a shutdown that does not wait for the actor has
/// returned by then.
const ACTOR_EXIT_HOLD: Duration = Duration::from_secs(5);

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", NEXT_MOB.fetch_add(1, Ordering::Relaxed))
}

fn root_definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.coordinator]\nmodel = \"gpt-5.5\"\n\
         runtime_mode = \"turn_driven\"\n[profiles.coordinator.tools]\ncomms = true\n"
    ))
    .expect("root definition")
}

fn child_definition(mob_id: &MobId) -> MobDefinition {
    serde_json::from_value(json!({
        "id": mob_id.to_string(),
        "profiles": { "worker": {
            "model": "gpt-5.5",
            "runtime_mode": "turn_driven",
            "tools": { "comms": true }
        } }
    }))
    .expect("child definition")
}

/// A runtime over a persistent root mob in `dir`, with durable child mobs when
/// `durable_children`, and the runtime machine its members register with.
async fn persistent_runtime(
    dir: &Path,
    durable_children: bool,
) -> (UnifiedRuntime, Arc<meerkat_runtime::MeerkatMachine>) {
    let (storage, provenance) =
        crate::mob_composition_manifest::persistent_mob_storage(dir.join("mob.sqlite3"))
            .expect("root mob storage");
    let mut spec = MobBootstrapSpec::persistent(
        root_definition(&unique("terminal-root")),
        storage,
        provenance,
        dir.to_path_buf(),
        8,
    )
    .await
    .expect("persistent spec");
    if durable_children {
        spec = spec.with_child_mob_storage(ChildMobStorage::Durable(
            MetaStorageLayout::with_injected_roots(dir.to_path_buf(), None),
        ));
    }
    let adapter = spec
        .runtime_adapter
        .clone()
        .expect("the persistent composition carries its runtime adapter");
    spec.options.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: unique("terminal-budget"),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .timeout(Duration::from_secs(10))
            .build(),
    )
    .await
    .expect("build the runtime");
    (runtime, adapter)
}

/// Seat the root's runtime-backed coordinator; returns it and its session.
async fn coordinator(runtime: &UnifiedRuntime) -> (AgentIdentity, SessionId) {
    let identity = AgentIdentity::from("coordinator");
    Box::pin(runtime.mob_handle().spawn_spec(SpawnMemberSpec::new(
        ProfileName::from("coordinator"),
        identity.clone(),
    )))
    .await
    .expect("spawn the coordinator");
    let session = runtime
        .mob_handle()
        .resolve_bridge_session_id(&identity)
        .await
        .expect("coordinator session");
    (identity, session)
}

/// Create a durable child mob owned by `owner`, as agent `mob_create` does,
/// and seat its turn-driven workers.
async fn create_child(runtime: &UnifiedRuntime, owner: SessionId) {
    let child_id = MobId::from(unique("terminal-child").as_str());
    let state = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("agent mob state");
    state
        .mob_create_definition_with_owner_bridge_session(
            child_definition(&child_id),
            owner,
            true,
            false,
        )
        .await
        .expect("create the child mob");
    for worker in ["worker-a", "worker-b"] {
        state
            .mob_spawn(
                &child_id,
                ProfileName::from("worker"),
                AgentIdentity::from(worker),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("seat a child worker");
    }
}

/// Hold the next runtime unregister saga until `hold` after it enters the
/// hold, then release it.
fn hold_next_unregister(
    adapter: &meerkat_runtime::MeerkatMachine,
    hold: Duration,
) -> tokio::task::JoinHandle<()> {
    let (entered, release) = adapter.test_hold_next_unregister_saga();
    tokio::spawn(async move {
        entered.await.expect("the held saga reports its entry");
        tokio::time::sleep(hold).await;
        let _ = release.send(());
    })
}

/// A root mob's terminal teardown waits for a member's runtime unregister
/// that runs longer than the 5 s the teardown used to get: the member is
/// reported unregistered and its registration is gone when shutdown
/// returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_root_teardown_slower_than_five_seconds_still_unregisters_every_member() {
    let dir = tempfile::tempdir().expect("state dir");
    let (runtime, adapter) = persistent_runtime(dir.path(), false).await;
    let (member, session) = coordinator(&runtime).await;
    // Armed right before shutdown: the mob stop before the terminal teardown
    // unregisters nothing, so the held saga is the member's own.
    let released = hold_next_unregister(&adapter, SLOW_UNREGISTER);

    let report = runtime.shutdown().await;

    let MobTerminalShutdownOutcome::Completed(terminal) = &report.mob_terminal_shutdown else {
        panic!("the terminal teardown completes: {report:?}");
    };
    assert!(
        matches!(
            terminal.members.get(&member),
            Some(meerkat_mob::MemberShutdownOutcome::Unregistered)
        ),
        "the member's unregister settles within the terminal budget: {terminal:?}"
    );
    assert!(terminal.is_clean(), "{terminal:?}");
    assert!(
        adapter
            .current_session_registration_witness(&session)
            .await
            .is_none(),
        "the member's registration is gone when shutdown returns"
    );
    tokio::time::timeout(RELEASE_BOUND, released)
        .await
        .expect("the held saga is released")
        .expect("the release task");
}

/// A root teardown that spends its whole budget leaves the durable child
/// root a stand-down window of its own: the live child answers its
/// admission close, its workers unregister, and the root is released for a
/// successor in the same process.
///
/// The root's held member is still pending when shutdown returns, and the
/// report still attests cleanup: `cleanup_completed()` does not gate on the
/// root's terminal report. A change to that gate must update this row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_root_that_spends_its_whole_budget_leaves_the_child_root_its_own_window() {
    let dir = tempfile::tempdir().expect("state dir");
    let (runtime, adapter) = persistent_runtime(dir.path(), true).await;
    let (_member, owner) = coordinator(&runtime).await;
    create_child(&runtime, owner).await;
    // The root's member unregister outlives the root budget below.
    let released = hold_next_unregister(&adapter, Duration::from_secs(2));

    let report = runtime
        .shutdown_with_root_budget(Duration::from_secs(1))
        .await;

    assert_eq!(
        report.child_mob_stand_down,
        ChildMobStandDownOutcome::Released { child_mobs: 1 },
        "the child root stands down in its own window: {report:?}"
    );
    assert!(report.cleanup_completed(), "{report:?}");
    ChildMobRootClaim::acquire(&child_mob_root(dir.path()))
        .expect("the released root is claimable in the same process");
    tokio::time::timeout(RELEASE_BOUND, released)
        .await
        .expect("the held saga is released")
        .expect("the release task");
}

/// The same slow root with a durable child root that holds no child mobs, the
/// shape a production shutdown hit: the child root is released.
///
/// GREEN-only row. Before the child stand-down had its own window this shape
/// raced the root actor's exit (a close sent while the actor was still
/// dropping its command channel went unanswered under the expired deadline),
/// so it is not a RED; the one-child row above pins the window.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_root_that_spends_its_whole_budget_releases_an_empty_child_root() {
    let dir = tempfile::tempdir().expect("state dir");
    let (runtime, adapter) = persistent_runtime(dir.path(), true).await;
    let _ = coordinator(&runtime).await;
    let released = hold_next_unregister(&adapter, Duration::from_secs(2));

    let report = runtime
        .shutdown_with_root_budget(Duration::from_secs(1))
        .await;

    assert_eq!(
        report.child_mob_stand_down,
        ChildMobStandDownOutcome::Released { child_mobs: 0 },
        "{report:?}"
    );
    assert!(report.cleanup_completed(), "{report:?}");
    ChildMobRootClaim::acquire(&child_mob_root(dir.path()))
        .expect("the released root is claimable in the same process");
    tokio::time::timeout(RELEASE_BOUND, released)
        .await
        .expect("the held saga is released")
        .expect("the release task");
}

/// The persistent composition of mob `mob_id` over `dir`, its child mobs
/// process-bound. The held-exit row depends on it: with a claimed durable
/// child root, the child stand-down closes the root mob's admission with a
/// command its actor answers only once it has dropped itself, and waits for
/// that answer or the root's task end, so even a shutdown that does not wait
/// for the actor would not return while the actor's exit is held.
async fn process_bound_spec(
    dir: &Path,
    mob_id: &str,
) -> Result<MobBootstrapSpec, crate::storage_health::StorageResolutionError> {
    let (storage, provenance) =
        crate::mob_composition_manifest::persistent_mob_storage(dir.join("mob.sqlite3"))
            .expect("root mob storage");
    let spec = MobBootstrapSpec::persistent(
        root_definition(mob_id),
        storage,
        provenance,
        dir.to_path_buf(),
        8,
    )
    .await?;
    Ok(spec.with_child_mob_storage(ChildMobStorage::ProcessBound))
}

/// The row's side of [`ActorExitHold`]: unarmed, the hold lets the exit go
/// at once.
#[derive(Default)]
struct ExitGate {
    armed: Mutex<Option<(oneshot::Sender<()>, mpsc::Receiver<()>)>>,
}

impl ExitGate {
    /// Arm the hold. The receiver resolves once the actor's exit reaches it;
    /// sending on (or dropping) the sender lets the exit go on.
    fn arm(&self) -> (oneshot::Receiver<()>, mpsc::Sender<()>) {
        let (reached_tx, reached) = oneshot::channel();
        let (release, release_rx) = mpsc::channel();
        *self.armed.lock().unwrap_or_else(PoisonError::into_inner) = Some((reached_tx, release_rx));
        (reached, release)
    }
}

/// Holds the root mob actor's exit while its gate is armed. A mob-wide
/// external-tools provider owns it, and only the actor holds that provider:
/// the actor drops it in its exit, after its terminal Shutdown has answered.
/// The wait runs in `block_in_place`, which hands the worker's other tasks to
/// another thread meanwhile.
struct ActorExitHold(Arc<ExitGate>);

impl ActorExitHold {
    /// The provider's answer at a spawn: no external tools.
    fn no_tools(&self) -> Option<Arc<dyn meerkat_core::AgentToolDispatcher>> {
        None
    }
}

impl Drop for ActorExitHold {
    fn drop(&mut self) {
        let armed = self
            .0
            .armed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some((reached, release)) = armed {
            let _ = reached.send(());
            tokio::task::block_in_place(|| {
                let _ = release.recv();
            });
        }
    }
}

/// A host that shuts a runtime down, drops it and composes a successor on
/// the same state directory in this process at once is not refused as in
/// use. The root mob's actor answers its terminal Shutdown first and drops
/// what it owns after, Core's execution custody of `runtime.sqlite` among
/// it: shutdown returns only once that actor has exited, so the runtime's
/// own drop leaves nothing holding the custody. Held in its exit, the actor
/// keeps shutdown from returning; let go, the successor composes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_waits_for_the_root_actor_so_a_successor_composes_at_once() {
    let dir = tempfile::tempdir().expect("state dir");
    let mob_id = unique("terminal-successor");
    let gate = Arc::new(ExitGate::default());
    let hold = ActorExitHold(Arc::clone(&gate));
    let mut spec = process_bound_spec(dir.path(), &mob_id)
        .await
        .expect("compose the runtime")
        .with_default_external_tools_provider(Arc::new(move || hold.no_tools()));
    spec.options.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: unique("terminal-successor"),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .timeout(Duration::from_secs(10))
            .build(),
    )
    .await
    .expect("build the runtime");

    let (reached, release) = gate.arm();
    let (returned_tx, mut returned) = oneshot::channel();
    let shutdown = tokio::spawn(async move {
        let report = runtime.shutdown().await;
        let _ = returned_tx.send(());
        (runtime, report)
    });
    tokio::time::timeout(RELEASE_BOUND, reached)
        .await
        .expect("the root mob's actor exits after its Shutdown")
        .expect("the held exit reports reaching the hold");
    assert!(
        tokio::time::timeout(ACTOR_EXIT_HOLD, &mut returned)
            .await
            .is_err(),
        "shutdown returned while the root mob's actor was still exiting"
    );
    let _ = release.send(());
    let (runtime, report) = shutdown.await.expect("join the shutdown");
    assert!(
        matches!(
            report.mob_terminal_shutdown,
            MobTerminalShutdownOutcome::Completed(_)
        ),
        "{report:?}"
    );
    drop(runtime);

    if let Err(error) = process_bound_spec(dir.path(), &mob_id).await {
        panic!("the successor on the same state directory is refused: {error}");
    }
}
