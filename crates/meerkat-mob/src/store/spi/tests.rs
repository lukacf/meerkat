//! Rows for the mob host store adapter, unrun until the candidate's
//! validation.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::host_store::{
    AcquireMode, CommitBatch, CommitId, FenceGrant, FenceTiming, FenceToken, HeadPage, HostStore,
    HostStoreError, Row, RowIndex, ScopeHead, ScopeKind, ScopeName, ScopeRef, ScopeVersion,
    VersionedHead,
};
use meerkat_runtime::store::spi::{SpiRealm, SpiRealmConfig};

use super::*;

/// One scope of the in-test host store.
#[derive(Debug, Clone)]
struct HostScope {
    version: ScopeVersion,
    commit_id: CommitId,
    head: Vec<u8>,
    first: u64,
    next: u64,
    rows: BTreeMap<u64, Row>,
}

/// What one committed batch looked like.
#[derive(Debug, Clone)]
struct BatchRecord {
    scopes: Vec<ScopeRef>,
    payload: usize,
}

#[derive(Debug, Default)]
struct HostState {
    token: u64,
    scopes: BTreeMap<ScopeRef, HostScope>,
    landed: HashSet<String>,
    batches: Vec<BatchRecord>,
    fail_next_commit: Option<HostStoreError>,
}

/// A host store meeting every obligation at commit: versions are checked
/// (Conflict), lower tokens refused (FencedOut), commit ids idempotent.
#[derive(Debug, Default)]
struct MemoryHost {
    state: StdMutex<HostState>,
    /// The fence timing it declares; the default when unset.
    timing: Option<FenceTiming>,
    /// When set, every commit waits here until notified.
    commit_gate: StdMutex<Option<Arc<tokio::sync::Notify>>>,
    /// Commits that started, counted before they wait at the gate.
    commits_started: std::sync::atomic::AtomicU64,
}

impl MemoryHost {
    fn state(&self) -> std::sync::MutexGuard<'_, HostState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn fail_next_commit(&self, error: HostStoreError) {
        self.state().fail_next_commit = Some(error);
    }

    fn batches(&self) -> Vec<BatchRecord> {
        self.state().batches.clone()
    }

    #[cfg_attr(not(feature = "runtime-adapter"), allow(dead_code))]
    fn with_timing(timing: FenceTiming) -> Self {
        Self {
            timing: Some(timing),
            ..Self::default()
        }
    }

    fn set_commit_gate(&self, gate: Option<Arc<tokio::sync::Notify>>) {
        *self
            .commit_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = gate;
    }
}

#[async_trait]
impl HostStore for MemoryHost {
    async fn acquire_fence(&self, _mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        let mut state = self.state();
        state.token += 1;
        Ok(FenceGrant {
            token: FenceToken::new(state.token),
            superseded_lease: Duration::ZERO,
        })
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        let state = self.state();
        if token.get() < state.token {
            return Err(HostStoreError::FencedOut {
                current: FenceToken::new(state.token),
            });
        }
        Ok(())
    }

    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError> {
        Ok(self.state().scopes.get(scope).map(|stored| VersionedHead {
            version: stored.version,
            commit_id: stored.commit_id.clone(),
            head: stored.head.clone(),
        }))
    }

    async fn list_heads(
        &self,
        kind: ScopeKind,
        after: Option<&ScopeName>,
        limit: usize,
    ) -> Result<HeadPage, HostStoreError> {
        let state = self.state();
        let mut heads: Vec<ScopeHead> = state
            .scopes
            .iter()
            .filter(|(scope, _)| {
                scope.kind == kind && after.is_none_or(|after| &scope.name > after)
            })
            .map(|(scope, stored)| ScopeHead {
                name: scope.name.clone(),
                head: VersionedHead {
                    version: stored.version,
                    commit_id: stored.commit_id.clone(),
                    head: stored.head.clone(),
                },
            })
            .collect();
        heads.sort_by(|left, right| {
            left.name
                .as_str()
                .as_bytes()
                .cmp(right.name.as_str().as_bytes())
        });
        let next = (heads.len() > limit).then(|| heads[limit - 1].name.clone());
        heads.truncate(limit);
        Ok(HeadPage { heads, next })
    }

    async fn read_rows(
        &self,
        scope: &ScopeRef,
        from: RowIndex,
        limit: usize,
    ) -> Result<Vec<Row>, HostStoreError> {
        let state = self.state();
        let Some(stored) = state.scopes.get(scope) else {
            return Ok(Vec::new());
        };
        if from.get() < stored.first {
            return Err(HostStoreError::Invalid(format!(
                "{scope}: row {from} was dropped"
            )));
        }
        Ok(stored
            .rows
            .range(from.get()..)
            .take(limit)
            .map(|(_, row)| row.clone())
            .collect())
    }

    fn fence_timing(&self) -> FenceTiming {
        self.timing.unwrap_or_default()
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError> {
        self.commits_started
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let gate = self
            .commit_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        let mut state = self.state();
        if let Some(error) = state.fail_next_commit.take() {
            return Err(error);
        }
        if state.landed.contains(batch.commit_id().as_str()) {
            return Ok(());
        }
        if batch.fence().get() < state.token {
            return Err(HostStoreError::FencedOut {
                current: FenceToken::new(state.token),
            });
        }
        for update in batch.updates() {
            let current = state.scopes.get(&update.scope).map(|stored| stored.version);
            if current != update.previous {
                return Err(HostStoreError::Conflict {
                    scope: update.scope.clone(),
                    current,
                });
            }
        }
        for update in batch.updates() {
            if update.delete {
                state.scopes.remove(&update.scope);
                continue;
            }
            let stored = state
                .scopes
                .entry(update.scope.clone())
                .or_insert_with(|| HostScope {
                    version: update.next,
                    commit_id: batch.commit_id().clone(),
                    head: Vec::new(),
                    first: 0,
                    next: 0,
                    rows: BTreeMap::new(),
                });
            if let Some(truncate) = update.truncate_after {
                stored.rows.retain(|index, _| *index < truncate.get());
                stored.next = stored.next.min(truncate.get());
            }
            if let Some(drop_before) = update.drop_before {
                stored.rows.retain(|index, _| *index >= drop_before.get());
                stored.first = stored.first.max(drop_before.get().min(stored.next));
            }
            for row in &update.append {
                let index = stored.next;
                stored.rows.insert(index, row.clone());
                stored.next += 1;
            }
            if let Some(head) = &update.head {
                stored.head = head.clone();
            }
            stored.version = update.next;
            stored.commit_id = batch.commit_id().clone();
        }
        state.landed.insert(batch.commit_id().as_str().to_string());
        state.batches.push(BatchRecord {
            scopes: batch
                .updates()
                .iter()
                .map(|update| update.scope.clone())
                .collect(),
            payload: batch.payload_bytes(),
        });
        Ok(())
    }
}

/// One process over the host: its realm, its mob host, one open storage.
struct Process {
    stores: SpiMobStores,
    _mobs: SpiMobStoreHost,
    _realm: Arc<SpiRealm>,
}

async fn start(host: &Arc<MemoryHost>, key: &str) -> Process {
    let dyn_host: Arc<dyn HostStore> = Arc::clone(host) as Arc<dyn HostStore>;
    let realm = SpiRealm::open(dyn_host, SpiRealmConfig::default())
        .await
        .expect("open the realm");
    let mobs = SpiMobStoreHost::for_realm(&realm)
        .await
        .expect("claim the mob families");
    let stores = mobs.open(key).await.expect("open the storage");
    Process {
        stores,
        _mobs: mobs,
        _realm: realm,
    }
}

/// Every entry of every scope of the storage, from the in-memory state.
async fn full_export(stores: &SpiMobStores) -> BTreeMap<String, Vec<u8>> {
    let bundle = &stores.bundle;
    let mut out = BTreeMap::new();
    for (name, bytes) in bundle.mob_image().await.expect("mob image") {
        out.insert(format!("mb:{name}"), bytes.to_vec());
    }
    for (name, bytes) in bundle.identity_image().await.expect("identity image") {
        out.insert(format!("mbi:{name}"), bytes.to_vec());
    }
    for (name, bytes) in bundle.run_image().await.expect("run image") {
        out.insert(format!("mbr:{name}"), bytes.to_vec());
    }
    for (name, bytes) in bundle.status_image().await.expect("status image") {
        out.insert(format!("mbs:{name}"), bytes.to_vec());
    }
    out
}

/// Export, restart the process over the same host, and require the exact
/// same export from the reloaded storage.
async fn restart_and_compare(host: &Arc<MemoryHost>, key: &str, process: Process) -> Process {
    let before = full_export(&process.stores).await;
    drop(process);
    let reopened = start(host, key).await;
    assert_eq!(
        full_export(&reopened.stores).await,
        before,
        "a reopen must load exactly what the process served"
    );
    reopened
}

fn event(kind: MobEventKind, timestamp: Option<DateTime<Utc>>) -> NewMobEvent {
    NewMobEvent {
        mob_id: MobId::from("mob"),
        timestamp,
        kind,
    }
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn reopen_restores_the_exact_export_after_each_mutating_call() {
    let host = Arc::new(MemoryHost::default());
    let mut process = start(&host, "mob").await;

    // Events.
    let storage = process.stores.storage();
    for _ in 0..3 {
        storage
            .events
            .append(event(MobEventKind::MobCompleted, None))
            .await
            .expect("append");
    }
    process = restart_and_compare(&host, "mob", process).await;

    // A spec.
    let storage = process.stores.storage();
    storage
        .specs
        .put_spec(&MobId::from("mob"), &MobDefinition::explicit("mob"), None)
        .await
        .expect("put spec");
    process = restart_and_compare(&host, "mob", process).await;

    // A flow run.
    let storage = process.stores.storage();
    let run = MobRun::authority_backed_for_steps(
        RunId::new(),
        MobId::from("mob"),
        FlowId::from("flow-a"),
        [StepId::from("step-1")],
        MobRunStatus::Pending,
        serde_json::json!({"a": 1}),
    )
    .expect("sample run");
    storage.runs.create_run(run).await.expect("create run");
    process = restart_and_compare(&host, "mob", process).await;

    // Runtime metadata.
    let storage = process.stores.storage();
    storage
        .runtime_metadata
        .put_member_event_cursor(
            &MobId::from("mob"),
            &MobMemberEventCursorRecord {
                agent_identity: "worker-1".to_string(),
                host_id: String::new(),
                host_binding_generation: None,
                member_session_id: String::new(),
                generation: 1,
                fence_token: None,
                next_seq: 7,
            },
        )
        .await
        .expect("put cursor");
    process = restart_and_compare(&host, "mob", process).await;

    // Prune, then clear (a finished destroy).
    let storage = process.stores.storage();
    storage
        .events
        .prune(Utc::now() + chrono::Duration::minutes(1))
        .await
        .expect("prune");
    process = restart_and_compare(&host, "mob", process).await;
    let storage = process.stores.storage();
    storage.events.clear().await.expect("clear");
    let process = restart_and_compare(&host, "mob", process).await;
    assert!(
        process
            .stores
            .storage()
            .events
            .replay_all()
            .await
            .expect("replay")
            .is_empty()
    );
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn a_write_over_two_scopes_commits_one_batch() {
    let host = Arc::new(MemoryHost::default());
    let process = start(&host, "mob").await;
    let bundle = Arc::clone(&process.stores.bundle);
    let before = host.batches().len();
    let run = MobRun::authority_backed_for_steps(
        RunId::new(),
        MobId::from("mob"),
        FlowId::from("flow-a"),
        [StepId::from("step-1")],
        MobRunStatus::Pending,
        serde_json::json!({}),
    )
    .expect("sample run");
    let run_id = run.run_id.clone();
    let touch = Touch {
        events: Some(EventsTouch::Appended),
        run: Some(run_id),
        ..Touch::default()
    };
    let inner = Arc::clone(&bundle);
    bundle
        .write(touch, async move {
            inner
                .events
                .append(event(MobEventKind::MobCompleted, None))
                .await?;
            inner.runs.create_run(run).await
        })
        .await
        .expect("write");
    let batches = host.batches();
    assert_eq!(batches.len(), before + 1, "one batch, not one per scope");
    let last = batches.last().expect("a batch");
    assert!(last.scopes.iter().any(|scope| scope.kind == ScopeKind::Mob));
    assert!(
        last.scopes
            .iter()
            .any(|scope| scope.kind == ScopeKind::Record)
    );
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn an_event_append_writes_only_the_new_event() {
    let host = Arc::new(MemoryHost::default());
    let process = start(&host, "mob").await;
    let storage = process.stores.storage();
    storage
        .events
        .append(event(MobEventKind::MobCompleted, None))
        .await
        .expect("append");
    storage
        .events
        .append(event(MobEventKind::MobCompleted, None))
        .await
        .expect("append");
    let early = host.batches().last().expect("a batch").payload;
    for _ in 0..200 {
        storage
            .events
            .append(event(MobEventKind::MobCompleted, None))
            .await
            .expect("append");
    }
    let late = host.batches().last().expect("a batch").payload;
    assert!(
        late <= early + 64,
        "the 202nd append carried {late} bytes against {early} for the 2nd"
    );
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn a_failed_batch_poisons_its_scopes_and_only_them() {
    let host = Arc::new(MemoryHost::default());
    let process = start(&host, "mob").await;
    let storage = process.stores.storage();
    let mut live = storage.events.subscribe().expect("subscribe");
    host.fail_next_commit(HostStoreError::Invalid("refused by the test".to_string()));
    let refused = storage
        .events
        .append(event(MobEventKind::MobCompleted, None))
        .await;
    assert!(matches!(refused, Err(MobStoreError::WriteFailed(_))));
    assert!(
        live.try_recv().is_err(),
        "a subscriber never sees an event its batch did not commit"
    );
    let read = storage.events.replay_all().await;
    assert!(
        matches!(&read, Err(MobStoreError::ReadFailed(detail)) if detail.contains("poisoned")),
        "the poisoned mob scope refuses reads: {read:?}"
    );
    // Another scope of the same storage still serves.
    storage
        .specs
        .get_spec(&MobId::from("mob"))
        .await
        .expect_err("specs share the poisoned mob scope");
    storage
        .runs
        .get_run(&RunId::new())
        .await
        .expect("the run scope is not poisoned");
    // Another storage still writes.
    let other = SpiMobStoreHost::for_realm(&process._realm)
        .await
        .expect("the same host")
        .open("other")
        .await
        .expect("open another storage");
    other
        .storage()
        .events
        .append(event(MobEventKind::MobCompleted, None))
        .await
        .expect("another storage appends");
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn subscribers_receive_committed_events_in_cursor_order() {
    let host = Arc::new(MemoryHost::default());
    let process = start(&host, "mob").await;
    let storage = process.stores.storage();
    let mut live = storage.events.subscribe().expect("subscribe");
    for _ in 0..3 {
        storage
            .events
            .append(event(MobEventKind::MobCompleted, None))
            .await
            .expect("append");
    }
    let mut cursors = Vec::new();
    while let Ok(received) = live.try_recv() {
        cursors.push(received.cursor);
    }
    assert_eq!(cursors, vec![1, 2, 3]);
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn the_mob_families_are_claimed_once_per_realm() {
    let host = Arc::new(MemoryHost::default());
    {
        let process = start(&host, "mob").await;
        process
            .stores
            .storage()
            .events
            .append(event(MobEventKind::MobCompleted, None))
            .await
            .expect("append");
    }
    let dyn_host: Arc<dyn HostStore> = Arc::clone(&host) as Arc<dyn HostStore>;
    let realm = SpiRealm::open(dyn_host, SpiRealmConfig::default())
        .await
        .expect("reopen the realm");
    let first = SpiMobStoreHost::for_realm(&realm).await.expect("first");
    let second = SpiMobStoreHost::for_realm(&realm).await.expect("second");
    assert!(Arc::ptr_eq(&first.inner, &second.inner));
    let listed = second.stored_mobs();
    assert!(
        listed.iter().any(|stored| stored.storage_key == "mob"),
        "a second claim must not hide stored mobs: {listed:?}"
    );
}

// Unrun until the candidate's validation.
#[test]
fn entries_of_an_unknown_format_are_refused() {
    let newer = br#"{"v":2,"d":{"mob_id":null,"finalized":false}}"#;
    assert!(matches!(
        decode_single::<MobMetaV1>(newer),
        Err(MobStoreError::ReadFailed(_))
    ));
    let current = encode_single(&MobMetaV1 {
        mob_id: None,
        finalized: true,
    })
    .expect("encode");
    assert_eq!(
        decode_single::<MobMetaV1>(&current).expect("decode"),
        MobMetaV1 {
            mob_id: None,
            finalized: true
        }
    );
}

// Unrun until the candidate's validation.
#[test]
fn keyed_entry_names_are_injective_and_verified() {
    let left = hashed_name(G_SUPERVISOR, &MobId::from("a/b")).expect("name");
    let right = hashed_name(G_SUPERVISOR, &MobId::from("a")).expect("name");
    assert_ne!(left, right);
    let mut body: BTreeMap<String, EntryBytes> = BTreeMap::new();
    body.insert(
        right,
        encode_keyed(&MobId::from("a/b"), &1_u64).expect("encode"),
    );
    assert!(
        decode_group::<MobId, u64>(&body, G_SUPERVISOR).is_err(),
        "an entry whose name does not hash from its key is refused"
    );
}

/// A running run, the completed state its terminal CAS moves to, and the
/// authority input of that move.
fn running_run() -> (
    MobRun,
    RunId,
    flow_run::State,
    flow_run::State,
    mob_dsl::MobMachineInput,
) {
    let run = MobRun::authority_backed_for_steps(
        RunId::new(),
        MobId::from("mob"),
        FlowId::from("flow-a"),
        [StepId::from("step-1")],
        MobRunStatus::Running,
        serde_json::json!({"a": 1}),
    )
    .expect("an authority-backed run");
    let run_id = run.run_id.clone();
    let expected_flow_state = run.flow_state.clone();
    let (completed, input) = run
        .flow_run_command_projection_for_test(
            crate::run::MobMachineFlowRunCommand::TerminalizeCompleted(
                crate::run::flow_run::inputs::TerminalizeCompleted {},
            ),
        )
        .expect("the completed run state");
    (run, run_id, expected_flow_state, completed, input)
}

fn flow_completed(run_id: &RunId) -> NewMobEvent {
    event(
        MobEventKind::FlowCompleted {
            run_id: run_id.clone(),
            flow_id: FlowId::from("flow-a"),
            structured_output: None,
        },
        None,
    )
}

// Unrun until the candidate's validation. The terminal run snapshot and
// its terminal event commit in ONE batch over the run and mob scopes, as
// the SQLite store commits them in one transaction.
#[tokio::test]
async fn a_terminal_run_and_its_event_commit_in_one_batch() {
    let host = Arc::new(MemoryHost::default());
    let process = start(&host, "mob").await;
    let storage = process.stores.storage();
    let (run, run_id, expected_flow_state, completed, input) = running_run();
    storage.runs.create_run(run).await.expect("create the run");
    let before = host.batches().len();
    let stored = storage
        .runs
        .cas_run_snapshot_and_append_terminal_event_with_authority(
            &run_id,
            MobRunStatus::Running,
            &expected_flow_state,
            MobRunStatus::Completed,
            &completed,
            vec![input.clone()],
            storage.events.as_ref(),
            flow_completed(&run_id),
        )
        .await
        .expect("the terminal commit");
    assert!(stored.is_some(), "the winning CAS commits the event");
    let batches = host.batches();
    assert_eq!(
        batches.len(),
        before + 1,
        "one batch for the run and its event"
    );
    let kinds: HashSet<ScopeKind> = batches[before]
        .scopes
        .iter()
        .map(|scope| scope.kind)
        .collect();
    assert!(
        kinds.contains(&ScopeKind::Mob) && kinds.contains(&ScopeKind::Record),
        "{kinds:?}"
    );
    // A lost CAS appends nothing and writes nothing.
    let lost = storage
        .runs
        .cas_run_snapshot_and_append_terminal_event_with_authority(
            &run_id,
            MobRunStatus::Running,
            &expected_flow_state,
            MobRunStatus::Completed,
            &completed,
            vec![input],
            storage.events.as_ref(),
            flow_completed(&run_id),
        )
        .await
        .expect("a lost CAS is not an error");
    assert!(lost.is_none());
    assert_eq!(host.batches().len(), before + 1);
    // Both sides survive a restart together.
    let process = restart_and_compare(&host, "mob", process).await;
    let storage = process.stores.storage();
    let run = storage
        .runs
        .get_run(&run_id)
        .await
        .expect("read the run")
        .expect("the run");
    assert_eq!(run.status, MobRunStatus::Completed);
    let events = storage.events.replay_all().await.expect("replay");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(&event.kind, MobEventKind::FlowCompleted { .. }))
            .count(),
        1
    );
}

// Unrun until the candidate's validation. A terminal event refused after its
// run's terminal snapshot won in memory commits neither side: the run and
// mob scopes are poisoned instead of the run committing alone.
#[tokio::test]
async fn a_refused_terminal_event_commits_neither_side() {
    let host = Arc::new(MemoryHost::default());
    let process = start(&host, "mob").await;
    let storage = process.stores.storage();
    let (run, run_id, expected_flow_state, completed, input) = running_run();
    storage.runs.create_run(run).await.expect("create the run");
    let before = host.batches().len();
    process.stores.bundle.events.fail_next_terminal_append();
    let refused = storage
        .runs
        .cas_run_snapshot_and_append_terminal_event_with_authority(
            &run_id,
            MobRunStatus::Running,
            &expected_flow_state,
            MobRunStatus::Completed,
            &completed,
            vec![input],
            storage.events.as_ref(),
            flow_completed(&run_id),
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(host.batches().len(), before, "nothing reached the host");
    assert!(
        storage.runs.get_run(&run_id).await.is_err(),
        "the run scope is poisoned"
    );
    assert!(
        storage.events.replay_all().await.is_err(),
        "the mob scope is poisoned"
    );
    // A restart serves what the host holds: the run still running, and no
    // terminal event.
    drop(storage);
    drop(process);
    let process = start(&host, "mob").await;
    let storage = process.stores.storage();
    let run = storage
        .runs
        .get_run(&run_id)
        .await
        .expect("read the run")
        .expect("the run");
    assert_eq!(run.status, MobRunStatus::Running);
    assert!(
        storage
            .events
            .replay_all()
            .await
            .expect("replay")
            .iter()
            .all(|event| !matches!(&event.kind, MobEventKind::FlowCompleted { .. }))
    );
}

// Unrun until the candidate's validation. A write dropped while its
// batch is in flight still lands it, publishes its event and records the
// scope's witness, so the next call neither conflicts nor refuses the scope
// as changed outside its lock.
#[tokio::test]
async fn a_write_dropped_during_its_commit_settles_and_the_scope_serves() {
    let host = Arc::new(MemoryHost::default());
    let process = start(&host, "mob").await;
    let storage = process.stores.storage();
    let mut live = storage.events.subscribe().expect("subscribe");
    let gate = Arc::new(tokio::sync::Notify::new());
    host.set_commit_gate(Some(Arc::clone(&gate)));
    let started = host
        .commits_started
        .load(std::sync::atomic::Ordering::Acquire);
    let append = {
        let events = Arc::clone(&storage.events);
        tokio::spawn(async move { events.append(event(MobEventKind::MobCompleted, None)).await })
    };
    while host
        .commits_started
        .load(std::sync::atomic::Ordering::Acquire)
        == started
    {
        tokio::task::yield_now().await;
    }
    append.abort();
    assert!(append.await.expect_err("aborted").is_cancelled());
    host.set_commit_gate(None);
    gate.notify_one();
    // The next call waits for the scope's lock until the handed-off commit
    // settled, then finds the witness it recorded.
    storage
        .events
        .append(event(MobEventKind::MobCompleted, None))
        .await
        .expect("the next append commits on top of the dropped one");
    let replayed = storage.events.replay_all().await.expect("the scope serves");
    assert_eq!(replayed.len(), 2);
    assert_eq!(
        live.try_recv().expect("the dropped write's event").cursor,
        1
    );
    assert_eq!(live.try_recv().expect("the next event").cursor, 2);
    let process = restart_and_compare(&host, "mob", process).await;
    assert_eq!(
        process
            .stores
            .storage()
            .events
            .replay_all()
            .await
            .expect("replay")
            .len(),
        2
    );
}

// Unrun until the candidate's validation. The identity write fence of
// a host store realm declares the host's longest commit as its Backoff
// retry bound, and answers Backoff while a commit holds the identity scope.
#[cfg(feature = "runtime-adapter")]
#[tokio::test]
async fn the_identity_write_fence_declares_the_hosts_longest_commit() {
    use meerkat_runtime::{
        RuntimeStoreError, RuntimeStoreWriteFence, RuntimeStoreWriteFenceOutcome,
    };

    struct Admit;
    impl RuntimeStoreWriteFence for Admit {
        fn execute_if_current(
            &self,
            operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
        ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
            operation().map(|()| RuntimeStoreWriteFenceOutcome::Applied)
        }
    }

    let timing = FenceTiming::default()
        .try_with_max_commit_latency(Duration::from_secs(10))
        .expect("a 10 s longest commit");
    let host = Arc::new(MemoryHost::with_timing(timing));
    let process = start(&host, "mob").await;
    let bundle = &process.stores.bundle;
    let fence = stores::SpiIdentityRuntimeWriteFence::over(bundle, Arc::new(Admit));
    assert_eq!(fence.backoff_retry_bound(), Duration::from_secs(10));
    let held = bundle.realm.cell(&bundle.identity_scope).lock_owned().await;
    assert!(matches!(
        fence.execute_if_current(Box::new(|| Ok::<(), RuntimeStoreError>(()))),
        Ok(RuntimeStoreWriteFenceOutcome::Backoff { .. })
    ));
    drop(held);
    assert!(matches!(
        fence.execute_if_current(Box::new(|| Ok::<(), RuntimeStoreError>(()))),
        Ok(RuntimeStoreWriteFenceOutcome::Applied)
    ));
}

/// Every field the stored event codec keeps beyond the public event
/// contract survives a reopen of the host store's mob storage: a member's
/// bridge ref, placed spawn, identity intent digest, peer endpoint and direct
/// fence; a recovered binding's session and endpoint; a retirement's endpoint
/// and topology mark; an abandoned respawn's runtime and fence. A restarted
/// mob rebuilds its roster and topology from them.
#[tokio::test]
async fn a_reopen_keeps_every_replay_only_event_field() {
    use crate::event::{MemberSessionBindingRecoveredEvent, MemberSpawnedEvent};
    use crate::ids::{AgentIdentity, AgentRuntimeId, Generation, PlacedSpawnId, ProfileName};
    use meerkat_contracts::wire::supervisor_bridge::{
        BridgeDirectMemberFence, BridgeDirectRuntimeSessionToken,
    };
    use meerkat_core::comms::{PeerId, TrustedPeerDescriptor};
    use meerkat_core::types::SessionId;

    let host = Arc::new(MemoryHost::default());
    let process = start(&host, "mob").await;
    let identity = AgentIdentity::from("worker");
    let generation = Generation::INITIAL;
    let runtime = AgentRuntimeId::initial(identity.clone());
    let fence = crate::ids::FenceToken::new(1);
    let session = SessionId::new();
    let pubkey = [7; 32];
    let endpoint = TrustedPeerDescriptor::unsigned_with_pubkey(
        "mob/worker/worker",
        PeerId::from_ed25519_pubkey(&pubkey).to_string(),
        pubkey,
        "inproc://mob/worker/worker",
    )
    .expect("member peer endpoint");
    let kinds = vec![
        MobEventKind::MemberSpawned(
            MemberSpawnedEvent::new(
                identity.clone(),
                generation,
                fence,
                runtime.clone(),
                ProfileName::from("worker"),
            )
            .with_bridge_member_ref(Some(MemberRef::from_bridge_session_id(session.clone())))
            .with_identity_intent_authority_digest(Some("sha256:intent".to_string()))
            .with_placed_spawn_id(Some(PlacedSpawnId::new()))
            .with_member_peer_endpoint(Some(endpoint.clone()))
            .with_direct_member_fence(Some(BridgeDirectMemberFence {
                mob_id: "mob".to_string(),
                agent_identity: identity.to_string(),
                generation: generation.get(),
                fence_token: 1,
                member_session_id: session.to_string(),
                runtime_session_token: BridgeDirectRuntimeSessionToken::new(),
            })),
        ),
        MobEventKind::MemberSessionBindingRecovered(
            MemberSessionBindingRecoveredEvent::new(
                identity.clone(),
                runtime.clone(),
                session.clone(),
            )
            .with_member_peer_endpoint(Some(endpoint.clone())),
        ),
        MobEventKind::MemberRetirementStarted {
            agent_identity: identity.clone(),
            agent_runtime_id: runtime.clone(),
            generation,
            role: ProfileName::from("worker"),
            releasing: Some(session.clone()),
            session_id: Some(session),
            retiring_peer_endpoint: Some(endpoint),
            preserve_machine_topology: true,
        },
        MobEventKind::RespawnTopologyAbandoned {
            agent_identity: identity,
            generation,
            agent_runtime_id: Some(runtime),
            fence_token: Some(fence),
        },
    ];
    let storage = process.stores.storage();
    for kind in &kinds {
        storage
            .events
            .append(event(kind.clone(), None))
            .await
            .expect("append the event");
    }
    drop(storage);
    drop(process);

    let reopened = start(&host, "mob").await;
    let replayed: Vec<MobEventKind> = reopened
        .stores
        .storage()
        .events
        .replay_all()
        .await
        .expect("replay the events")
        .into_iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        replayed, kinds,
        "a reopen replays every replay-only field of the stored events"
    );
}

/// An event entry a pre-release build of this storage wrote in the public
/// event form, without the stored codec's schema version, is refused with an
/// error that names that form and asks for the mob to be recreated: it lacks
/// the fields a restart needs, so it is never read without them.
#[test]
fn a_pre_release_event_entry_is_refused_as_such() {
    let event = MobEvent {
        cursor: 1,
        timestamp: Utc::now(),
        mob_id: MobId::from("mob"),
        kind: MobEventKind::MobCompleted,
    };
    let entry = encode_single(&event).expect("encode the public event form");
    let error = decode_event(&event_name(1), &entry).expect_err("the public form is refused");
    let MobStoreError::ReadFailed(message) = &error else {
        panic!("a read failure: {error:?}");
    };
    assert!(
        message.contains("written by a pre-release 0.9.5 build")
            && message.contains("recreate the mob"),
        "{message}"
    );
}
