//! What a mob's start and the lift of a stopped mob read from a host store
//! for members with long operation histories. The runtime store is the host
//! store adapter over a host that records every call (the reference
//! in-memory host store underneath), the mob's storage lives in the same
//! realm, and every member's runtime settled the same number of background
//! operations (registered, completed, completion injected) before the
//! restart, so no member holds work. Under the default restore policy every
//! member stays Dormant, and no call reads a member's session scope or a
//! chunk: the start reads every head and the mob's own scopes, and the counts
//! do not change with the history.
//!
//! The session service is the mock one, whose durable session documents live
//! in the mock, so a session-service read never reaches the host here: the
//! row also requires that the restarted service's reads of each member's
//! session (`read`, metadata, persisted session, resume authority, resume
//! verdict) do not move across the start and the lift.
//!
//! Unrun until the candidate's validation.

use super::*;
use crate::machines::mob_machine as dsl;
use crate::store::{
    MOB_FAMILY, MOB_IDENTITY_FAMILY, MOB_RUN_FAMILY, MOB_STATUS_FAMILY, SpiMobStoreHost,
};
use meerkat_core::completion_feed::CompletionFeed;
use meerkat_core::host_store::{
    AcquireMode, CommitBatch, FenceGrant, FenceTiming, FenceToken, HeadPage, HostStore,
    HostStoreError, Row, RowIndex, ScopeHead, ScopeKind, ScopeName, ScopeRef, VersionedHead,
};
use meerkat_core::ops_lifecycle::{
    CompletionCursorConsumer, OperationId, OperationKind, OperationResult, OperationSpec,
    OpsLifecycleRegistry,
};
use meerkat_core::{EpochCursorState, RuntimeEpochId};
use meerkat_runtime::store::RuntimeStore;
use meerkat_runtime::store::spi::{RUNTIME_FAMILY, SpiRealm, SpiRealmConfig, SpiRuntimeStore};
use meerkat_runtime::{
    LogicalRuntimeId, MeerkatMachine, OpsLifecycleConfig, PersistedOpsSnapshot,
    RuntimeOpsLifecycleRegistry,
};
use meerkat_store_conformance::InMemoryHostStore;

const ROW_STEP: Duration = Duration::from_secs(60);

/// The members of the mob.
const MEMBERS: usize = 2;

/// Completed operations each member's registry keeps in its snapshot.
const SETTLED_RETAINED: usize = 24;

/// The result text of each settled operation, in bytes.
const SETTLED_RESULT_BYTES: usize = 512;

/// Settled operations between two persisted snapshots of a history. The
/// runtime persists one at every terminal transition; each snapshot
/// supersedes the one before it, so fewer snapshots grow the rows the same
/// way and cost the row less time.
const SETTLED_PER_SNAPSHOT: usize = 50;

/// Heads a host store listing answers per call (the realm's page, 256
/// before 0.9.7).
const HEADS_PER_LIST: usize = 2048;

/// The family tag of Core's chunk scopes (a chunk head names its owner).
const CHUNK_TAG: &str = "ch";

/// The scope kinds an open lists.
const LISTED_KINDS: [ScopeKind; 4] = [
    ScopeKind::Session,
    ScopeKind::Identity,
    ScopeKind::Mob,
    ScopeKind::Record,
];

fn worker(index: usize) -> AgentIdentity {
    AgentIdentity::from(format!("boot-reads-{index:02}"))
}

fn turn_driven(identity: &AgentIdentity) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", identity.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

fn phase_of(
    handle: &MobHandle,
    identity: &AgentIdentity,
) -> Option<dsl::MemberMaterializationPhase> {
    handle.member_materialization_for_test(identity)
}

/// One host call: the operation, and the scope it named (a listing names
/// its kind, a commit none).
#[derive(Debug, Clone, PartialEq, Eq)]
enum HostCall {
    AcquireFence,
    RenewFence,
    LoadHead(String),
    ListHeads(ScopeKind),
    ReadRows(String),
    Commit,
}

/// The reference in-memory host store, recording every call it answers.
#[derive(Debug, Default)]
struct CountingHost {
    inner: InMemoryHostStore,
    calls: std::sync::Mutex<Vec<HostCall>>,
}

impl CountingHost {
    fn record(&self, call: HostCall) {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call);
    }

    /// Every call since the last take, renews left out: they run on the
    /// realm's own timer, not on the path a row measures.
    fn take_calls(&self) -> Vec<HostCall> {
        std::mem::take(
            &mut *self
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_iter()
        .filter(|call| *call != HostCall::RenewFence)
        .collect()
    }

    /// How many visible scopes of `kind` the store holds, counted without
    /// recording a call.
    async fn scope_count(&self, kind: ScopeKind) -> usize {
        self.heads(kind).await.len()
    }

    /// Every visible head of `kind`, read without recording a call.
    async fn heads(&self, kind: ScopeKind) -> Vec<ScopeHead> {
        let mut heads = Vec::new();
        let mut after: Option<ScopeName> = None;
        loop {
            let page = self
                .inner
                .list_heads(kind, after.as_ref(), HEADS_PER_LIST)
                .await
                .expect("the reference store lists its heads");
            heads.extend(page.heads);
            match page.next {
                Some(next) => after = Some(next),
                None => return heads,
            }
        }
    }

    /// The host names of the session scopes of `runtimes` and of every
    /// chunk scope they own, read without recording a call.
    async fn scopes_of_runtimes(&self, runtimes: &BTreeSet<String>) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for head in self.heads(ScopeKind::Session).await {
            let decoded: serde_json::Value =
                serde_json::from_slice(&head.head.head).expect("a session scope head is JSON");
            if decoded["fam"] == RUNTIME_FAMILY.tag
                && decoded["key"]
                    .as_str()
                    .is_some_and(|key| runtimes.contains(key))
            {
                names.insert(head.name.as_str().to_string());
            }
        }
        let owners = names.clone();
        for head in self.heads(ScopeKind::Record).await {
            if tag_of(head.name.as_str()) != CHUNK_TAG {
                continue;
            }
            let decoded: serde_json::Value =
                serde_json::from_slice(&head.head.head).expect("a chunk head is JSON");
            if decoded["owner"]
                .as_str()
                .is_some_and(|owner| owners.contains(owner))
            {
                names.insert(head.name.as_str().to_string());
            }
        }
        names
    }
}

#[async_trait]
impl HostStore for CountingHost {
    async fn acquire_fence(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        self.record(HostCall::AcquireFence);
        self.inner.acquire_fence(mode).await
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        self.record(HostCall::RenewFence);
        self.inner.renew_fence(token).await
    }

    fn fence_timing(&self) -> FenceTiming {
        self.inner.fence_timing()
    }

    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError> {
        self.record(HostCall::LoadHead(scope.name.as_str().to_string()));
        self.inner.load_head(scope).await
    }

    async fn list_heads(
        &self,
        kind: ScopeKind,
        after: Option<&ScopeName>,
        limit: usize,
    ) -> Result<HeadPage, HostStoreError> {
        self.record(HostCall::ListHeads(kind));
        self.inner.list_heads(kind, after, limit).await
    }

    async fn read_rows(
        &self,
        scope: &ScopeRef,
        from: RowIndex,
        limit: usize,
    ) -> Result<Vec<Row>, HostStoreError> {
        self.record(HostCall::ReadRows(scope.name.as_str().to_string()));
        self.inner.read_rows(scope, from, limit).await
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError> {
        self.record(HostCall::Commit);
        self.inner.commit(batch).await
    }
}

/// The family tag a scope name starts with.
fn tag_of(name: &str) -> &str {
    name.split_once(':').map_or(name, |(tag, _)| tag)
}

/// The scope a read names, when the call is a read of one scope.
fn read_scope(call: &HostCall) -> Option<&str> {
    match call {
        HostCall::LoadHead(name) | HostCall::ReadRows(name) => Some(name.as_str()),
        _ => None,
    }
}

/// The reads a row compares across histories: each call by its operation
/// and the family tag of its scope, counted. Commits are left out, and so
/// are the reads of the mob's own scopes: they follow the mob's own history,
/// which is the same in every row but not row for row deterministic.
fn read_summary(calls: &[HostCall]) -> BTreeMap<String, usize> {
    let mob_tags = [
        MOB_FAMILY.tag,
        MOB_IDENTITY_FAMILY.tag,
        MOB_RUN_FAMILY.tag,
        MOB_STATUS_FAMILY.tag,
    ];
    let mut summary = BTreeMap::new();
    for call in calls {
        let key = match call {
            HostCall::AcquireFence => "acquire_fence".to_string(),
            HostCall::ListHeads(kind) => format!("list_heads {kind}"),
            HostCall::LoadHead(name) | HostCall::ReadRows(name)
                if mob_tags.iter().any(|tag| *tag == tag_of(name)) =>
            {
                continue;
            }
            HostCall::LoadHead(name) => format!("load_head {}", tag_of(name)),
            HostCall::ReadRows(name) => format!("read_rows {}", tag_of(name)),
            HostCall::RenewFence | HostCall::Commit => continue,
        };
        *summary.entry(key).or_insert(0) += 1;
    }
    summary
}

/// A member runtime's ops lifecycle registry settling background operations
/// one after another: each is registered, runs, completes, and has its
/// completion injected into the member, so no snapshot holds work.
struct SettlingOps {
    registry: RuntimeOpsLifecycleRegistry,
    feed: Arc<dyn CompletionFeed>,
    epoch: RuntimeEpochId,
    owner: SessionId,
}

impl SettlingOps {
    fn new(owner: SessionId) -> Self {
        let registry = RuntimeOpsLifecycleRegistry::with_config(OpsLifecycleConfig {
            max_completed: SETTLED_RETAINED,
            max_concurrent: None,
        });
        let feed = registry.completion_feed_handle();
        Self {
            registry,
            feed,
            epoch: RuntimeEpochId::new(),
            owner,
        }
    }

    fn settle_one(&self) {
        let id = OperationId::new();
        self.registry
            .register_operation(OperationSpec {
                id: id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: self.owner.clone(),
                display_name: "settled operation".into(),
                source_label: "host-store-boot-reads".into(),
                operation_source: None,
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("register the operation");
        self.registry
            .provisioning_succeeded(&id)
            .expect("start the operation");
        self.registry
            .complete_operation(
                &id,
                OperationResult {
                    id: id.clone(),
                    content: "r".repeat(SETTLED_RESULT_BYTES),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("complete the operation");
        let completion = self.feed.watermark();
        self.registry
            .advance_completion_cursor(CompletionCursorConsumer::RuntimeInjected, completion, None)
            .expect("inject the completion");
    }

    fn snapshot(&self) -> PersistedOpsSnapshot {
        self.registry
            .capture_persistence_snapshot(self.epoch.clone(), &EpochCursorState::new())
            .expect("the ops lifecycle snapshot")
    }
}

/// Write the history of `settled` settled operations into `runtime`: the
/// empty snapshot a registration persists, then a snapshot every
/// [`SETTLED_PER_SNAPSHOT`] operations and one after the last.
async fn write_settled_history(
    store: &SpiRuntimeStore,
    runtime: &LogicalRuntimeId,
    owner: &SessionId,
    settled: usize,
) {
    let ops = SettlingOps::new(owner.clone());
    store
        .persist_ops_lifecycle(runtime, &ops.snapshot())
        .await
        .expect("the registration's snapshot persists");
    let mut done = 0;
    while done < settled {
        let batch = SETTLED_PER_SNAPSHOT.min(settled - done);
        for _ in 0..batch {
            ops.settle_one();
        }
        done += batch;
        store
            .persist_ops_lifecycle(runtime, &ops.snapshot())
            .await
            .expect("the snapshot persists");
    }
}

/// A new process's realm over `host`. It takes over: a predecessor process
/// in this test may still renew its lease, and the takeover fences it out.
async fn open_realm(host: &Arc<CountingHost>) -> Arc<SpiRealm> {
    SpiRealm::open(
        Arc::clone(host) as Arc<dyn HostStore>,
        SpiRealmConfig {
            acquire: AcquireMode::Takeover,
            ..SpiRealmConfig::default()
        },
    )
    .await
    .expect("open the realm")
}

/// A persistent runtime over `store`, as one process opens it.
fn runtime_over(store: &Arc<SpiRuntimeStore>) -> Arc<MeerkatMachine> {
    Arc::new(
        MeerkatMachine::persistent(
            Arc::clone(store) as Arc<dyn RuntimeStore>,
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .expect("construct runtime authority"),
    )
}

/// What the session service read of one member's session, as the mock
/// session service counts it: its reads (`SessionService::read`), its
/// metadata reads, and its durable reads (the persisted session, the resume
/// authority, the resume verdict). Over the mock these never reach the host,
/// so the row counts them on the service itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MemberSessionReads {
    reads: u64,
    metadata_reads: u64,
    durable: DurableSessionReadCounts,
}

/// The session service's reads of each member's session so far.
fn member_session_reads(
    service: &MockSessionService,
    sessions: &[(AgentIdentity, SessionId)],
) -> Vec<(AgentIdentity, MemberSessionReads)> {
    sessions
        .iter()
        .map(|(identity, session)| {
            (
                identity.clone(),
                MemberSessionReads {
                    reads: service.session_reads_of(session).calls,
                    metadata_reads: service.persisted_session_metadata_reads_for(session),
                    durable: service.durable_reads_of(session),
                },
            )
        })
        .collect()
}

/// What one history's restart read from the host: at the start (realm open,
/// runtime store open, the mob's storage and its resume to ready), and at
/// the lift of the mob stopped after it.
struct RestartReads {
    start: Vec<HostCall>,
    lift: Vec<HostCall>,
    /// The visible scopes of each listed kind when the restart opened.
    scopes: BTreeMap<String, usize>,
    /// The host names of the members' session scopes and their chunks.
    member_scopes: BTreeSet<String>,
    /// The restarted session service's reads of each member's session,
    /// before the start and after the lift.
    service_reads_before: Vec<(AgentIdentity, MemberSessionReads)>,
    service_reads_after: Vec<(AgentIdentity, MemberSessionReads)>,
}

/// Create a mob in a host store realm, spawn its members, crash, write
/// `settled` settled operations into every member's runtime from another
/// process, then restart: open the realm, resume the mob, stop it and lift
/// it, recording what each step read.
async fn restart_after_history(label: &str, settled: usize) -> RestartReads {
    let host = Arc::new(CountingHost::default());
    let definition = with_unique_mob_id(sample_definition(), label);
    let storage_key = definition.id.to_string();

    // The first process: the mob and its members, then a crash.
    let realm = open_realm(&host).await;
    let store = Arc::new(
        SpiRuntimeStore::open(Arc::clone(&realm))
            .await
            .expect("open the runtime store"),
    );
    let mobs = SpiMobStoreHost::for_realm(&realm)
        .await
        .expect("claim the mob families");
    let storage = mobs
        .open(&storage_key)
        .await
        .expect("open the mob's storage")
        .storage();
    let service = Arc::new(MockSessionService::new());
    service.set_runtime_adapter(runtime_over(&store));
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create the mob");
    let mut sessions = Vec::with_capacity(MEMBERS);
    for index in 0..MEMBERS {
        let identity = worker(index);
        handle
            .spawn_spec(turn_driven(&identity))
            .await
            .expect("spawn a worker");
        let session = handle
            .resolve_bridge_session_id(&identity)
            .await
            .expect("a session-backed worker");
        sessions.push((identity, session));
    }
    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    crash_stop_and_release_routes(handle).await;
    drop(service);
    drop((mobs, store, realm));

    // A second process writes each member's history: settled operations
    // only, so no member holds work.
    host.inner.lapse_fence_leases();
    let realm = open_realm(&host).await;
    let store = SpiRuntimeStore::open(Arc::clone(&realm))
        .await
        .expect("open the runtime store");
    for (_, session) in &sessions {
        write_settled_history(
            &store,
            &LogicalRuntimeId::for_session(session),
            session,
            settled,
        )
        .await;
    }
    drop((store, realm));

    // The restart, recorded from its acquire on.
    host.inner.lapse_fence_leases();
    let mut scopes = BTreeMap::new();
    for kind in LISTED_KINDS {
        scopes.insert(kind.as_str().to_string(), host.scope_count(kind).await);
    }
    let runtimes: BTreeSet<String> = sessions
        .iter()
        .map(|(_, session)| LogicalRuntimeId::for_session(session).0)
        .collect();
    let member_scopes = host.scopes_of_runtimes(&runtimes).await;
    assert!(
        member_scopes.len() >= MEMBERS,
        "every member's runtime has a session scope on the host: {member_scopes:?}"
    );
    host.take_calls();
    let realm = open_realm(&host).await;
    let store = Arc::new(
        SpiRuntimeStore::open(Arc::clone(&realm))
            .await
            .expect("open the runtime store"),
    );
    let mobs = SpiMobStoreHost::for_realm(&realm)
        .await
        .expect("claim the mob families");
    let storage = mobs
        .open(&storage_key)
        .await
        .expect("open the mob's storage")
        .storage();
    restarted.set_runtime_adapter(runtime_over(&store));
    let service_reads_before = member_session_reads(&restarted, &sessions);
    let builder = MobBuilder::for_resume(storage)
        .with_session_service(restarted.clone())
        .notify_orchestrator_on_resume(false);
    let handle = tokio::time::timeout(ROW_STEP, Box::pin(builder.resume()))
        .await
        .expect("the resume answers")
        .expect("the resume succeeds");
    let start = host.take_calls();
    for (identity, _) in &sessions {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity} holds no work after {settled} settled operations and stays Dormant"
        );
    }

    // The lift of the stopped mob.
    handle.stop().await.expect("stop the mob");
    host.take_calls();
    tokio::time::timeout(ROW_STEP, handle.resume())
        .await
        .expect("the lift answers")
        .expect("the lift succeeds");
    let lift = host.take_calls();
    for (identity, _) in &sessions {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "the lift leaves {identity} Dormant"
        );
    }
    let service_reads_after = member_session_reads(&restarted, &sessions);
    crash_stop_and_release_routes(handle).await;
    drop((mobs, store, realm));
    RestartReads {
        start,
        lift,
        scopes,
        member_scopes,
        service_reads_before,
        service_reads_after,
    }
}

// Unrun until the candidate's validation. A start and a lift read no
// Dormant member's rows, whatever its history.
//
// The start makes one acquire and `list_heads` per kind at 2048 heads per
// call, reads the mob's own scopes, and makes no `load_head` or `read_rows`
// that names a member's session scope or a chunk it owns; the lift of the
// stopped mob reads none either. Every other count is the same for 0, 1,000
// and 10,000 settled operations per member.
//
// The lift half holds only where the lift binds no dormant member's
// operation owner and builds nothing for a dormant seat: before that, its
// post-commit binding pass registered every dormant member's runtime, which
// loads the member's session rows from the host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_and_a_lift_read_no_dormant_members_rows_whatever_their_history() {
    let mut summaries = Vec::new();
    for settled in [0, 1_000, 10_000] {
        let reads = restart_after_history(&format!("boot-reads-{settled}"), settled).await;
        assert_eq!(
            reads
                .start
                .iter()
                .filter(|call| **call == HostCall::AcquireFence)
                .count(),
            1,
            "the start acquires once: {:?}",
            reads.start
        );
        for kind in LISTED_KINDS {
            let scopes = reads.scopes[kind.as_str()];
            assert_eq!(
                reads
                    .start
                    .iter()
                    .filter(|call| **call == HostCall::ListHeads(kind))
                    .count(),
                scopes.div_ceil(HEADS_PER_LIST).max(1),
                "the start lists the {scopes} {kind} heads {HEADS_PER_LIST} per call"
            );
        }
        for (step, calls) in [("start", &reads.start), ("lift", &reads.lift)] {
            let member_reads: Vec<&HostCall> = calls
                .iter()
                .filter(|call| {
                    read_scope(call).is_some_and(|name| reads.member_scopes.contains(name))
                })
                .collect();
            assert!(
                member_reads.is_empty(),
                "the {step} after {settled} settled operations read a Dormant member's session \
                 or a chunk it owns: {member_reads:?}"
            );
        }
        assert!(
            reads
                .start
                .iter()
                .filter_map(read_scope)
                .any(|name| tag_of(name) == MOB_FAMILY.tag),
            "the start reads the mob's own storage from the host: {:?}",
            reads.start
        );
        assert_eq!(
            reads.service_reads_after, reads.service_reads_before,
            "the start and the lift after {settled} settled operations read no Dormant \
             member's session through the session service"
        );
        summaries.push((
            settled,
            read_summary(&reads.start),
            read_summary(&reads.lift),
        ));
    }
    for pair in summaries.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        assert_eq!(
            before.1, after.1,
            "the start reads the same after {} and after {} settled operations",
            before.0, after.0
        );
        assert_eq!(
            before.2, after.2,
            "the lift reads the same after {} and after {} settled operations",
            before.0, after.0
        );
    }
}
