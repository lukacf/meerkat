//! A Meta realm kept in a host store, over Core's reference in-memory host
//! store ([`InMemoryHostStore`]) and its manual clock.
//!
//! Every process of a test opens the same store through a [`TestHost`] of its
//! own. A restart drops every handle of the old process; its lease stays
//! recorded on the store's clock, so the next open meets `Held` until the
//! test lets the lease lapse. A failover also starts the runtime on an empty
//! cache directory, so nothing local carries over.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use meerkat_core::host_store::{
    CommitBatch, FenceGrant, HeadPage, HostStoreError, Row, RowIndex, ScopeKind, ScopeName,
    ScopeRef, ScopeVersion, VersionedHead,
};
use meerkat_core::types::{ContentInput, SessionId};
use meerkat_runtime::LogicalRuntimeId;
use meerkat_runtime::RuntimeStore as _;
use meerkat_store_conformance::InMemoryHostStore;
use sha2::{Digest as _, Sha256};

use super::*;
use crate::host_store_documents::composition_key;
use crate::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentCustomizer, AgentIdentity,
    CustomizerError, DurableAgentSpec, FirstBootManifest, ManagedPeerEdge, RosterContext,
    RosterError, RosterProvider, TopologyContext, TopologyError, TopologyProvider,
};
use crate::mob_handle_runtime::{
    MobBootstrapOptions, MobBootstrapSpec, MobRuntime, MobRuntimeError,
};
use crate::runtime::{MemberIdleRetireOverrideRecord, PersistentMetadataStore};
use crate::storage_migrate::MigrateMode;
use crate::storage_migrate_native::{
    HostStoreImportClass, HostStoreImportRefusal, HostStoreImportReport, HostStoreImportRequest,
    import_continuity_export_into_host_store,
};
use crate::unified_runtime::{
    EventLogConfig, EventLogError, EventQuery, PersistedEvent, UnifiedRuntime,
    UnifiedRuntimeBuilder, UnifiedRuntimeBuilderError,
};

/// The storage provider's name, which the realm's locations carry.
const PROVIDER: &str = "test-host";
const ALICE: &str = "personal:alice";
const BOB: &str = "personal:bob";
const CAROL: &str = "personal:carol";
/// A turn whose request carries this waits for its model call until the test
/// releases it.
const HELD_TURN: &str = "HOLD-THIS-TURN";
/// A turn whose request carries this answers its first model call with a
/// call to a tool no catalog has, so its run calls the model again with the
/// tool's refusal.
const TOOL_TURN: &str = "CALL-A-TOOL-FIRST";

fn lock<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Wait until `check` answers, for at most half a minute.
async fn eventually<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Wait until the asynchronous `check` holds, for at most half a minute.
async fn eventually_holds<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !check().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ---------------------------------------------------------------------------
// The host store as one process sees it
// ---------------------------------------------------------------------------

/// One process's handle on the realm's host store: the reference store, which
/// every process of a test shares, and the faults a test arms on this process
/// alone.
struct TestHost {
    store: Arc<InMemoryHostStore>,
    /// Batches that landed through this process.
    landed: AtomicUsize,
    /// The next commits answer `Unavailable` without applying.
    failing: AtomicUsize,
    /// Once this many session scopes were created through this process, every
    /// commit answers `Unavailable`: the store went away.
    sessions_until_outage: AtomicUsize,
    sessions_created: AtomicUsize,
    /// While set the process is paused: it neither renews nor acquires.
    paused: AtomicBool,
    /// The appended rows of each batch that landed through this process and
    /// touched the mob announcement store's scope, in order.
    announcement_rows: StdMutex<Vec<Vec<u8>>>,
    /// The next batches that touch the mob announcement store's scope are
    /// refused, and nothing of them lands.
    refused_announcement_batches: AtomicUsize,
}

/// Whether `rows` carry `text`.
fn carries(rows: &[u8], text: &str) -> bool {
    rows.windows(text.len())
        .any(|window| window == text.as_bytes())
}

/// Whether `batch` touches the mob announcement store's scope.
fn touches_announcements(batch: &CommitBatch) -> bool {
    batch
        .updates()
        .iter()
        .any(|update| update.scope.name.as_str().starts_with("mxa:"))
}

impl TestHost {
    fn over(store: Arc<InMemoryHostStore>) -> Self {
        Self {
            store,
            landed: AtomicUsize::new(0),
            failing: AtomicUsize::new(0),
            sessions_until_outage: AtomicUsize::new(usize::MAX),
            sessions_created: AtomicUsize::new(0),
            paused: AtomicBool::new(false),
            announcement_rows: StdMutex::new(Vec::new()),
            refused_announcement_batches: AtomicUsize::new(0),
        }
    }

    fn landed(&self) -> usize {
        self.landed.load(Ordering::SeqCst)
    }

    /// The first landed announcement store batch whose rows carry `text`, by
    /// its place among them.
    fn first_announcement_batch_carrying(&self, text: &str) -> Option<usize> {
        lock(&self.announcement_rows)
            .iter()
            .position(|rows| carries(rows, text))
    }

    /// Whether the landed announcement store batch at `place` carries `text`
    /// in its rows.
    fn announcement_batch_carries(&self, place: usize, text: &str) -> bool {
        lock(&self.announcement_rows)
            .get(place)
            .is_some_and(|rows| carries(rows, text))
    }

    fn refuse_next_announcement_batches(&self, count: usize) {
        self.refused_announcement_batches
            .store(count, Ordering::SeqCst);
    }

    fn fail_next_commits(&self, count: usize) {
        self.failing.store(count, Ordering::SeqCst);
    }

    fn outage_after_sessions(&self, sessions: usize) {
        self.sessions_until_outage.store(sessions, Ordering::SeqCst);
    }

    fn pause(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    fn running(&self) -> Result<(), HostStoreError> {
        if self.paused.load(Ordering::SeqCst) {
            return Err(HostStoreError::Unavailable(
                "the process is paused".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl HostStore for TestHost {
    async fn acquire_fence(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        self.running()?;
        self.store.acquire_fence(mode).await
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        self.running()?;
        self.store.renew_fence(token).await
    }

    fn fence_timing(&self) -> FenceTiming {
        self.store.fence_timing()
    }

    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError> {
        self.store.load_head(scope).await
    }

    async fn list_heads(
        &self,
        kind: ScopeKind,
        after: Option<&ScopeName>,
        limit: usize,
    ) -> Result<HeadPage, HostStoreError> {
        self.store.list_heads(kind, after, limit).await
    }

    async fn read_rows(
        &self,
        scope: &ScopeRef,
        from: RowIndex,
        limit: usize,
    ) -> Result<Vec<Row>, HostStoreError> {
        self.store.read_rows(scope, from, limit).await
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError> {
        if self
            .failing
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(HostStoreError::Unavailable(
                "the store failed this batch".to_string(),
            ));
        }
        if self.sessions_created.load(Ordering::SeqCst)
            >= self.sessions_until_outage.load(Ordering::SeqCst)
        {
            return Err(HostStoreError::Unavailable(
                "the store went away".to_string(),
            ));
        }
        let announcements = touches_announcements(batch);
        if announcements
            && self
                .refused_announcement_batches
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok()
        {
            return Err(HostStoreError::Invalid(
                "the store refused this batch".to_string(),
            ));
        }
        self.store.commit(batch).await?;
        if announcements {
            lock(&self.announcement_rows).push(
                batch
                    .updates()
                    .iter()
                    .flat_map(|update| update.append.iter())
                    .flat_map(|row| row.as_bytes().iter().copied())
                    .collect(),
            );
        }
        let created = batch
            .updates()
            .iter()
            .filter(|update| update.scope.kind == ScopeKind::Session && update.previous.is_none())
            .count();
        self.sessions_created.fetch_add(created, Ordering::SeqCst);
        self.landed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A realm config whose failed batch fails at once, so an injected failure
/// is not retried away within the commit retry budget.
fn failing_fast() -> SpiRealmConfig {
    SpiRealmConfig {
        commit_retry_budget: Duration::from_millis(1),
        ..SpiRealmConfig::default()
    }
}

// ---------------------------------------------------------------------------
// The realm a test's processes share
// ---------------------------------------------------------------------------

/// A realm kept in a host store as a test's processes share it: the host
/// store, and the host's own stores beside it.
struct Fixture {
    store: Arc<InMemoryHostStore>,
    timing: FenceTiming,
    schedule: Arc<dyn meerkat::ScheduleStore>,
    workgraph: Arc<dyn meerkat::WorkGraphStore>,
    blobs: Arc<dyn BinaryBlobStore>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_timing(FenceTiming::default())
    }

    fn with_timing(timing: FenceTiming) -> Self {
        Self {
            store: Arc::new(InMemoryHostStore::with_fence_timing(timing)),
            timing,
            schedule: Arc::new(meerkat::MemoryScheduleStore::new()),
            workgraph: Arc::new(meerkat::MemoryWorkGraphStore::new()),
            blobs: Arc::new(crate::blob_store::ObjectStoreBlobStore::memory()),
        }
    }

    /// A new process's handle on the store.
    fn process(&self) -> Arc<TestHost> {
        Arc::new(TestHost::over(Arc::clone(&self.store)))
    }

    /// The host's own stores, as the host supplies them to every open. They
    /// are in memory, and declared so.
    fn stores(&self) -> HostStoreMetaStores {
        HostStoreMetaStores {
            schedule_store: Arc::clone(&self.schedule),
            workgraph_store: Arc::clone(&self.workgraph),
            job_store: None,
            blob_store: Arc::clone(&self.blobs),
            console_log_store: None,
            agent_memory_provider: None,
            event_log_store: None,
            durability: ["schedule", "workgraph", "blobs"]
                .into_iter()
                .map(|domain| {
                    DurabilityDeclaration::durable(domain, DurabilityResolution::DeclaredEphemeral)
                })
                .collect(),
        }
    }

    async fn open(
        &self,
        host: &Arc<TestHost>,
    ) -> Result<HostStoreMetaStorageProvider, HostStoreOpenError> {
        self.open_with(host, SpiRealmConfig::default()).await
    }

    async fn open_with(
        &self,
        host: &Arc<TestHost>,
        config: SpiRealmConfig,
    ) -> Result<HostStoreMetaStorageProvider, HostStoreOpenError> {
        HostStoreMetaStorageProvider::open(
            PROVIDER,
            Arc::clone(host) as Arc<dyn HostStore>,
            config,
            self.stores(),
        )
        .await
    }

    /// [`Self::open`], with the host supplying `log` as the realm's event log
    /// store, in memory and declared so.
    async fn open_with_event_log(
        &self,
        host: &Arc<TestHost>,
        log: RecordedLog,
    ) -> Result<HostStoreMetaStorageProvider, HostStoreOpenError> {
        let mut stores = self.stores();
        stores.event_log_store = Some(Box::new(log));
        stores.durability.push(DurabilityDeclaration::durable(
            "event_log",
            DurabilityResolution::DeclaredEphemeral,
        ));
        HostStoreMetaStorageProvider::open(
            PROVIDER,
            Arc::clone(host) as Arc<dyn HostStore>,
            SpiRealmConfig::default(),
            stores,
        )
        .await
    }

    /// The previous holder is gone, and its lease lapsed on the store's
    /// clock.
    fn lapse(&self) {
        self.store.lapse_fence_leases();
    }

    /// Every scope the store lists, with its version.
    async fn heads(&self) -> Vec<(String, ScopeVersion)> {
        let mut heads = Vec::new();
        for kind in [
            ScopeKind::Session,
            ScopeKind::Identity,
            ScopeKind::Mob,
            ScopeKind::Record,
        ] {
            let mut after: Option<ScopeName> = None;
            loop {
                let page = self
                    .store
                    .list_heads(kind, after.as_ref(), 256)
                    .await
                    .expect("the store lists its heads");
                heads.extend(
                    page.heads
                        .into_iter()
                        .map(|head| (format!("{kind:?} {}", head.name), head.head.version)),
                );
                match page.next {
                    Some(next) => after = Some(next),
                    None => break,
                }
            }
        }
        heads
    }

    fn import_request(&self, export: &Path, mode: MigrateMode) -> HostStoreImportRequest {
        self.import_request_over(&self.process(), export, mode)
    }

    fn import_request_over(
        &self,
        host: &Arc<TestHost>,
        export: &Path,
        mode: MigrateMode,
    ) -> HostStoreImportRequest {
        HostStoreImportRequest {
            host: Arc::clone(host) as Arc<dyn HostStore>,
            realm_config: failing_fast(),
            blob_store: Arc::clone(&self.blobs),
            continuity_export: export.to_path_buf(),
            mode,
        }
    }
}

/// Open the realm again as a new process. The stopped process's lease is
/// still recorded, so the open first meets `Held` for at most the lease; it
/// opens once the lease lapsed.
async fn reopen(fixture: &Fixture) -> Arc<HostStoreMetaStorageProvider> {
    match fixture.open(&fixture.process()).await {
        Err(HostStoreOpenError::Held { remaining, .. }) => assert!(
            remaining <= fixture.timing.lease(),
            "a restart waits at most the lease, not {remaining:?}"
        ),
        other => panic!("a restart before the lease lapsed meets Held, not {other:?}"),
    }
    fixture.lapse();
    Arc::new(
        fixture
            .open(&fixture.process())
            .await
            .expect("the realm opens once the lease lapsed"),
    )
}

/// An event log store that keeps every event the runtime ingests into it.
#[derive(Clone, Default)]
struct RecordedLog(Arc<StdMutex<Vec<PersistedEvent>>>);

impl RecordedLog {
    fn count(&self) -> usize {
        lock(&self.0).len()
    }
}

impl EventLogStore for RecordedLog {
    fn append_batch(
        &self,
        events: Vec<PersistedEvent>,
    ) -> Pin<Box<dyn Future<Output = Result<(), EventLogError>> + Send + '_>> {
        lock(&self.0).extend(events);
        Box::pin(async { Ok(()) })
    }

    fn query(
        &self,
        _query: EventQuery,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PersistedEvent>, EventLogError>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

// ---------------------------------------------------------------------------
// The model, the roster and the host's providers
// ---------------------------------------------------------------------------

/// The model every member calls: it records each request and answers "ok",
/// a request that carries [`HELD_TURN`] waits until the test releases it,
/// and the first call of a [`TOOL_TURN`] calls a tool instead.
#[derive(Clone)]
struct Model {
    requests: Arc<StdMutex<Vec<String>>>,
    gate: Arc<tokio::sync::Semaphore>,
    /// The calls that ended: answered to the end, or dropped by the run.
    settled: Arc<AtomicUsize>,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            requests: Arc::default(),
            gate: Arc::new(tokio::sync::Semaphore::new(0)),
            settled: Arc::default(),
        }
    }
}

/// Counts a model call as settled when its stream ends or is dropped.
struct CallSettled(Arc<AtomicUsize>);

impl Drop for CallSettled {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl Model {
    fn count(&self) -> usize {
        lock(&self.requests).len()
    }

    fn settled(&self) -> usize {
        self.settled.load(Ordering::SeqCst)
    }

    fn requests_since(&self, start: usize) -> Vec<String> {
        lock(&self.requests).iter().skip(start).cloned().collect()
    }

    fn requests_carrying(&self, text: &str) -> usize {
        lock(&self.requests)
            .iter()
            .filter(|request| request.contains(text))
            .count()
    }

    /// Let every held turn, and every later one, answer.
    fn release(&self) {
        self.gate.close();
    }

    async fn wait_for(&self, at_least: usize, what: &str) {
        eventually(what, || (self.count() >= at_least).then_some(())).await;
    }
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for Model {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let rendered = serde_json::to_string(request).unwrap_or_default();
        let held = rendered.contains(HELD_TURN);
        // The call after the tool call carries the tool's result: it answers.
        let call_tool = rendered.contains(TOOL_TURN)
            && !request
                .messages
                .iter()
                .any(|message| matches!(message, meerkat_core::Message::ToolResults { .. }));
        lock(&self.requests).push(rendered);
        let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
            request,
            meerkat_core::Provider::OpenAI,
            if call_tool {
                meerkat_core::StopReason::ToolUse
            } else {
                meerkat_core::StopReason::EndTurn
            },
        );
        let gate = Arc::clone(&self.gate);
        let settled = CallSettled(Arc::clone(&self.settled));
        Box::pin(async_stream::stream! {
            let _settled = settled;
            if held {
                // Closed by the test: the turn then answers.
                let _ = gate.acquire().await;
            }
            if call_tool {
                yield Ok(meerkat_client::LlmEvent::ToolCallComplete {
                    id: "call-no-such-tool".to_string(),
                    name: "no_such_tool".to_string(),
                    args: serde_json::json!({}),
                    meta: None,
                });
            } else {
                yield Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                });
            }
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).expect("a valid identity")
}

fn spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: id(name),
        profile: meerkat_mob::ProfileName::from("personal"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

/// A roster that lists these identities.
struct Roster(Vec<DurableAgentSpec>);

impl Roster {
    fn of(names: &[&str]) -> Self {
        Self(names.iter().copied().map(spec).collect())
    }
}

#[async_trait::async_trait]
impl RosterProvider for Roster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(self.0.clone())
    }
}

/// A host customizer that changes nothing: with one, the build publishes
/// each identity's customizer tools and holds a restored member none covers.
struct PlainCustomizer;

#[async_trait::async_trait]
impl AgentCustomizer for PlainCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }
}

/// Every pair of the roster's identities is wired.
struct CompleteGraph;

#[async_trait::async_trait]
impl TopologyProvider for CompleteGraph {
    async fn compute_edges(
        &self,
        _target_identities: &[AgentIdentity],
        context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        let identities: Vec<&AgentIdentity> =
            context.roster.iter().map(|spec| &spec.identity).collect();
        let mut edges = Vec::new();
        for (index, a) in identities.iter().enumerate() {
            for b in &identities[index + 1..] {
                edges.push(
                    ManagedPeerEdge::new((*a).clone(), (*b).clone())
                        .map_err(|error| TopologyError::InvalidEdge(format!("{error:?}")))?,
                );
            }
        }
        Ok(edges)
    }
}

// ---------------------------------------------------------------------------
// Runtimes over the realm
// ---------------------------------------------------------------------------

fn unique_mob_id(test: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!("host-store-{test}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn definition(mob_id: &str) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.personal]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.personal.tools]\ncomms = true\n"
    ))
    .expect("the mob definition parses")
}

/// A build over the realm `provider` opened, on the per-host cache `cache`,
/// whose roster lists `names`.
fn builder(
    provider: &Arc<HostStoreMetaStorageProvider>,
    cache: &Path,
    mob_id: &str,
    names: &[&str],
    model: &Model,
) -> UnifiedRuntimeBuilder {
    UnifiedRuntimeBuilder::default()
        .definition(definition(mob_id))
        .storage_provider(provider.clone())
        .scratch_dir(cache)
        .roster_provider(Arc::new(Roster::of(names)))
        .identity_runtime_instance_id(mob_id)
        .default_llm_client(Arc::new(model.clone()))
}

async fn build(builder: UnifiedRuntimeBuilder) -> UnifiedRuntime {
    match Box::pin(builder.build()).await {
        Ok(runtime) => runtime,
        Err(error) => panic!("the build over the host store realm: {error}"),
    }
}

/// The configuration refusal of a build that must refuse.
async fn refused_configuration(builder: UnifiedRuntimeBuilder) -> String {
    match Box::pin(builder.build()).await {
        Ok(runtime) => {
            stop(runtime).await;
            panic!("the build must refuse its configuration");
        }
        Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(message)) => message,
        Err(other) => panic!("a configuration refusal, not: {other}"),
    }
}

/// Shut `runtime` down gracefully, as a host stops its process, within the
/// shutdown horizon the runtime publishes.
async fn stop(runtime: UnifiedRuntime) {
    let horizon = runtime.shutdown_horizon();
    let _report = tokio::time::timeout(horizon, runtime.shutdown())
        .await
        .expect("the runtime shuts down within its published horizon");
}

/// Send `text` to `name`, and wait until the realm committed the model's
/// answer to it. Answers the requests the turn made. A test sends a text to
/// a session once.
async fn send(
    runtime: &UnifiedRuntime,
    provider: &HostStoreMetaStorageProvider,
    model: &Model,
    name: &str,
    text: &str,
) -> Vec<String> {
    let before = model.count();
    runtime
        .identity_runtime()
        .expect("an identity-first runtime")
        .send(&id(name), &ContentInput::Text(text.to_string()))
        .await
        .unwrap_or_else(|error| panic!("{name} takes the turn: {error}"));
    let session = session_of(runtime, name).await;
    eventually_holds(&format!("the realm to commit {name}'s answer"), || {
        answer_committed(provider, &session, text)
    })
    .await;
    model.requests_since(before)
}

/// Whether the realm committed an answer to `text` in `session`: the
/// session's committed document holds `text`, and an assistant message
/// after it.
async fn answer_committed(
    provider: &HostStoreMetaStorageProvider,
    session: &SessionId,
    text: &str,
) -> bool {
    let Ok(Some(committed)) = provider
        .realm
        .runtime_store()
        .load_committed_whole_blob_snapshot(&LogicalRuntimeId::for_session(session))
        .await
    else {
        return false;
    };
    let Ok(document) = meerkat_core::Session::from_persisted_bytes(committed.bytes()) else {
        return false;
    };
    let messages = document.messages();
    messages
        .iter()
        .rposition(|message| {
            serde_json::to_string(message).is_ok_and(|encoded| encoded.contains(text))
        })
        .is_some_and(|asked| {
            messages[asked + 1..]
                .iter()
                .any(|message| matches!(message, meerkat_core::Message::BlockAssistant(_)))
        })
}

/// The session Core binds `name` to.
async fn session_of(runtime: &UnifiedRuntime, name: &str) -> SessionId {
    runtime
        .identity_runtime()
        .expect("an identity-first runtime")
        .status(&id(name))
        .await
        .unwrap_or_else(|error| panic!("the status of {name}: {error}"))
        .session_id
        .unwrap_or_else(|| panic!("{name} is bound to a session"))
}

fn token(name: &str) -> String {
    format!("TOKEN-{}-7Q", name.replace(':', "-"))
}

/// Every path under `dir`.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path.clone());
            }
            found.push(path);
        }
    }
    found
}

/// The names a state directory keeps authority under: a host store realm's
/// cache holds none of them.
const STATE_DIR_AUTHORITY: [&str; 10] = [
    crate::storage_layout::RUNTIME_DB_FILE_NAME,
    crate::storage_layout::EVENT_LOG_DB_FILE_NAME,
    crate::storage_layout::CANONICAL_SESSIONS_DB_FILE_NAME,
    crate::storage_layout::CANONICAL_CONTINUITY_DB_FILE_NAME,
    crate::storage_layout::CANONICAL_METADATA_DB_FILE_NAME,
    crate::storage_layout::CANONICAL_CONSOLE_DB_FILE_NAME,
    crate::storage_layout::STATE_DIR_LOCK_FILE_NAME,
    crate::identity_first::FIRST_BOOT_MANIFEST_FILE,
    crate::identity_first::HELD_COMPLETION_OWED_DIR,
    "realm_manifest.json",
];

/// Core's undirected member edges, by Core member id.
async fn wired_pairs(runtime: &UnifiedRuntime) -> BTreeSet<(String, String)> {
    let mut pairs = BTreeSet::new();
    for member in runtime.mob_handle().list_members().await {
        for peer in &member.wired_to {
            let (a, b) = (member.agent_identity.to_string(), peer.to_string());
            pairs.insert(if a < b { (a, b) } else { (b, a) });
        }
    }
    pairs
}

/// Every pair of `names`, by Core member id.
fn desired_pairs(names: &[&str]) -> BTreeSet<(String, String)> {
    let members: Vec<String> = names
        .iter()
        .map(|name| crate::member_comms_id::roster_member_id_for_identity(name).to_string())
        .collect();
    let mut pairs = BTreeSet::new();
    for (index, a) in members.iter().enumerate() {
        for b in &members[index + 1..] {
            pairs.insert(if a < b {
                (a.clone(), b.clone())
            } else {
                (b.clone(), a.clone())
            });
        }
    }
    pairs
}

/// How many member edges the mob's log records as wired, and as unwired.
async fn wiring_events(runtime: &UnifiedRuntime) -> (usize, usize) {
    let handle = runtime.mob_handle();
    let (mut wired, mut unwired) = (0, 0);
    let mut cursor = 0;
    loop {
        let events = handle
            .poll_events(cursor, 1_000)
            .await
            .expect("the mob's events");
        let Some(last) = events.last() else {
            break;
        };
        cursor = last.cursor;
        for event in &events {
            match &event.kind {
                meerkat_mob::MobEventKind::MembersWired { .. } => wired += 1,
                meerkat_mob::MobEventKind::MembersWiredBatch { edges, .. } => wired += edges.len(),
                meerkat_mob::MobEventKind::MembersUnwired { .. } => unwired += 1,
                _ => {}
            }
        }
    }
    (wired, unwired)
}

// ---------------------------------------------------------------------------
// Continuity exports
// ---------------------------------------------------------------------------

fn lowercase_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// Write a continuity export of `entries` (identity, session, document) at
/// `path`, as the export scripts write one. The entries come in identity
/// order.
fn write_export(path: &Path, entries: &[(String, SessionId, Vec<u8>)]) {
    use base64::Engine as _;
    let mut lines = Vec::new();
    for (identity, session, document) in entries {
        let line = format!(
            "{{\"identity\":\"{identity}\",\"state\":\"ready\",\"record\":{{\"identity\":\
             \"{identity}\",\"agent_runtime_id\":\"rt-{runtime}\",\"session_id\":\"{session}\",\
             \"generation\":1,\"checkpoint_version\":1}},\"document\":{{\"length\":{length},\
             \"sha256\":\"{sha256}\",\"base64\":\"{base64}\"}}}}\n",
            runtime = identity.replace(':', "-"),
            length = document.len(),
            sha256 = lowercase_hex(&Sha256::digest(document)),
            base64 = base64::engine::general_purpose::STANDARD.encode(document),
        );
        lines.extend_from_slice(line.as_bytes());
    }
    let mut file = format!(
        "{{\"format\":\"meerkat-meta.continuity-export\",\"version\":1,\"writer\":\"test\",\
         \"entries\":{},\"entries_length\":{},\"entries_sha256\":\"{}\"}}\n",
        entries.len(),
        lines.len(),
        lowercase_hex(&Sha256::digest(&lines))
    )
    .into_bytes();
    file.extend_from_slice(&lines);
    std::fs::write(path, file).expect("write the export");
}

/// A released 0.9.3 host's continuity export (`memory` or `durable`),
/// unpacked into `dir`: four ready entries, one of them a 3.9 MB document.
fn released_export(dir: &Path, capture: &str) -> PathBuf {
    let source = std::fs::File::open(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/released_0_9_3_h_remote")
            .join(format!("{capture}-export.jsonl.gz")),
    )
    .expect("the released export fixture");
    let export = dir.join(format!("{capture}-export.jsonl"));
    let mut out = std::fs::File::create(&export).expect("the unpacked export");
    std::io::copy(&mut flate2::read::GzDecoder::new(source), &mut out).expect("gunzip");
    export
}

/// The session and the document of the first ready entry of `export`.
fn first_ready_entry(export: &Path) -> (SessionId, Vec<u8>) {
    use base64::Engine as _;
    let text = std::fs::read_to_string(export).expect("read the export");
    let entry = text
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("an export entry"))
        .find(|entry| entry["state"] == "ready")
        .expect("a ready entry");
    let session = SessionId::parse(entry["record"]["session_id"].as_str().expect("its session"))
        .expect("a session id");
    let document = base64::engine::general_purpose::STANDARD
        .decode(entry["document"]["base64"].as_str().expect("its document"))
        .expect("the document's base64");
    (session, document)
}

/// Export what a first realm's runtime gave `names`, as a 0.9.3 host's
/// continuity export carries it: each identity's committed session document,
/// after a turn that left a token in its transcript.
async fn export_of_a_running_realm(
    dir: &Path,
    mob_id: &str,
    names: &[&str],
) -> (PathBuf, BTreeMap<String, SessionId>) {
    let source = Fixture::new();
    let model = Model::default();
    let cache = tempfile::tempdir().expect("the source realm's cache");
    let mut sessions = BTreeMap::new();
    {
        let provider = Arc::new(
            source
                .open(&source.process())
                .await
                .expect("open the source realm"),
        );
        let runtime = build(builder(&provider, cache.path(), mob_id, names, &model)).await;
        for name in names {
            send(
                &runtime,
                &provider,
                &model,
                name,
                &format!("Remember this token: {}", token(name)),
            )
            .await;
            sessions.insert((*name).to_string(), session_of(&runtime, name).await);
        }
        stop(runtime).await;
    }
    source.lapse();
    let provider = source
        .open(&source.process())
        .await
        .expect("reopen the source realm");
    let mut entries = Vec::new();
    for (name, session) in &sessions {
        let committed = provider
            .realm
            .runtime_store()
            .load_committed_whole_blob_snapshot(&LogicalRuntimeId::for_session(session))
            .await
            .expect("read the committed document")
            .unwrap_or_else(|| panic!("{name}'s session is committed"));
        entries.push((name.clone(), session.clone(), committed.bytes().to_vec()));
    }
    let export = dir.join("source-export.jsonl");
    write_export(&export, &entries);
    (export, sessions)
}

#[derive(serde::Serialize)]
struct CompactionCommitFingerprint<'a> {
    selection: &'a meerkat_core::TranscriptRewriteSelection,
    original_span_digest: &'a str,
    replacement_digest: &'a str,
    messages_before: usize,
    messages_after: usize,
    actor: &'a Option<String>,
}

fn compaction_commit_fingerprint(commit: &meerkat_core::TranscriptRewriteCommit) -> String {
    let canonical = serde_json::to_vec(&CompactionCommitFingerprint {
        selection: &commit.selection,
        original_span_digest: &commit.original_span_digest,
        replacement_digest: &commit.replacement_digest,
        messages_before: commit.messages_before,
        messages_after: commit.messages_after,
        actor: &commit.actor,
    })
    .expect("encode the commit fingerprint");
    format!("sha256:{}", lowercase_hex(&Sha256::digest(canonical)))
}

/// `session` as a released 0.8.10 document holds its one compaction rewrite.
fn encode_as_released_0810_compaction_fixture(
    session: &meerkat_core::Session,
) -> serde_json::Value {
    let history = session
        .validated_transcript_history_state()
        .expect("the history state validates")
        .expect("the rewrite graph exists");
    assert_eq!(history.digest_format(), 3, "the current transcript format");
    assert_eq!(history.commit_count(), 1, "one rewrite");
    let commit = history.last_commit().expect("the rewrite commit");
    let (start, end) = commit.selection.bounds();
    let mut released_commit = serde_json::to_value(commit).expect("encode the commit");
    released_commit
        .as_object_mut()
        .expect("a commit object")
        .remove("rewrite_generation");
    released_commit["selection"] = serde_json::json!({
        "type": "compaction_message_range",
        "range": { "start": start, "end": end }
    });
    let released_graph = serde_json::json!({
        "head": history.head(),
        "commits": [released_commit],
        "revisions": [
            history
                .materialize_revision(&commit.parent_revision)
                .expect("the parent revision"),
            history
                .materialize_revision(&commit.revision)
                .expect("the revision"),
        ],
        "digest_format": 2,
    });
    let mut encoded = serde_json::to_value(session).expect("encode the session");
    encoded["version"] = serde_json::json!(2);
    encoded["metadata"][meerkat_core::SESSION_TRANSCRIPT_HISTORY_STATE_KEY] = released_graph;
    encoded["metadata"]
        .as_object_mut()
        .expect("the session metadata")
        .remove(meerkat_core::SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
    encoded
}

/// A session whose document carries one pending compaction projection
/// intent, and that intent.
fn session_with_compaction_intent() -> (
    meerkat_core::Session,
    meerkat_core::CompactionProjectionIntent,
) {
    let mut session = meerkat_core::Session::new();
    session.push(meerkat_core::types::Message::User(
        meerkat_core::types::UserMessage::text("verbose context one"),
    ));
    session.push(meerkat_core::types::Message::User(
        meerkat_core::types::UserMessage::text("verbose context two"),
    ));
    let parent = session
        .transcript_revision()
        .expect("the transcript revision");
    session
        .commit_transcript_rewrite(
            meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
            vec![meerkat_core::types::Message::User(
                meerkat_core::types::UserMessage::compaction_summary("compacted context"),
            )],
            meerkat_core::TranscriptRewriteReason::new("compaction"),
            Some("host-store-import".to_string()),
            Some(parent),
        )
        .expect("the compaction rewrite");
    let encoded = serde_json::to_vec(&encode_as_released_0810_compaction_fixture(&session))
        .expect("encode the released document");
    let (mut session, _receipt) = meerkat_core::import_released_0810_session(&encoded)
        .expect("the released 0.8.10 document imports")
        .into_parts();
    let commit = session
        .validated_transcript_history_state()
        .expect("the history state validates")
        .expect("the rewrite graph exists")
        .last_commit()
        .expect("the rewrite commit")
        .clone();
    let intent = meerkat_core::CompactionProjectionIntent {
        projection: serde_json::from_value(serde_json::json!({
            "session_id": session.id(),
            "parent_revision": &commit.parent_revision,
            "revision": &commit.revision,
            "commit_fingerprint": compaction_commit_fingerprint(&commit),
        }))
        .expect("the projection key"),
        summary_tokens: 5,
        messages_before: 2,
        messages_after: 1,
        summary_source: None,
        estimated_history_tokens_after: None,
    };
    session
        .add_compaction_projection_intent(intent.clone())
        .expect("the pending intent");
    (session, intent)
}

async fn import(request: &HostStoreImportRequest) -> HostStoreImportReport {
    import_continuity_export_into_host_store(request).await
}

// ---------------------------------------------------------------------------
// The realm, its fence and Meta's documents
// ---------------------------------------------------------------------------

/// A second process opening a realm another one holds meets the holder's
/// fence, typed, and writes nothing.
#[tokio::test]
async fn a_second_process_meets_held_and_writes_nothing() {
    let fixture = Fixture::new();
    let first = fixture
        .open(&fixture.process())
        .await
        .expect("the first process opens the realm");
    first
        .realm
        .ensure_realm_record(&crate::storage_provider::default_meerkat_level_realm())
        .await
        .expect("the first process writes the realm record");
    let before = fixture.heads().await;

    let second = fixture.process();
    match fixture.open(&second).await {
        Err(HostStoreOpenError::Held { holder, remaining }) => {
            assert_eq!(holder, first.realm().fence(), "the first process holds it");
            assert!(
                !remaining.is_zero() && remaining <= fixture.timing.lease(),
                "{remaining:?}"
            );
        }
        other => panic!("the second process meets Held, not {other:?}"),
    }
    assert_eq!(second.landed(), 0, "the second process committed nothing");
    assert_eq!(fixture.heads().await, before, "the realm is unchanged");
    assert!(first.readiness().is_ready(), "the holder still serves");
}

/// A takeover of a paused process waits out what is left of its lease, and
/// the paused process's next write re-acquires, meets the new holder and
/// poisons its realm: it writes nothing more, and its self-fence stays
/// closed.
#[tokio::test]
async fn a_takeover_of_a_paused_process_fences_it_out_at_its_next_write() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3))
        .expect("a fence timing");
    let fixture = Fixture::with_timing(timing);
    let paused_host = fixture.process();
    let paused = fixture
        .open(&paused_host)
        .await
        .expect("the first process opens the realm");
    paused
        .realm
        .documents()
        .write("probe", Some(b"before".to_vec()))
        .await
        .expect("a write under the fence");

    // The process stops: no renew, no acquire. Its own window closes.
    paused_host.pause(true);
    eventually("the paused process's window to close", || {
        (!paused.readiness().lease_live).then_some(())
    })
    .await;

    // Another process takes over: it waits out what remains of the paused
    // holder's lease before it serves.
    fixture.store.advance_clock(Duration::from_secs(2));
    let taker = fixture
        .open_with(
            &fixture.process(),
            SpiRealmConfig {
                acquire: AcquireMode::Takeover,
                ..SpiRealmConfig::default()
            },
        )
        .await
        .expect("the takeover opens the realm");
    assert_eq!(taker.realm().superseded_lease(), Duration::from_secs(1));

    // The paused process resumes; its next write meets the new holder.
    paused_host.pause(false);
    let refused = paused
        .realm
        .documents()
        .write("probe", Some(b"after".to_vec()))
        .await
        .expect_err("the fenced-out process writes nothing");
    assert!(
        matches!(refused.spi(), Some(SpiError::RealmPoisoned(_))),
        "{refused}"
    );
    let readiness = paused.readiness();
    assert!(
        readiness.realm_poisoned.is_some() && !readiness.is_ready() && !readiness.lease_live,
        "{readiness:?}"
    );

    // The new holder serves, and holds what was written before the takeover.
    assert!(taker.readiness().is_ready());
    let probe = taker
        .realm
        .documents()
        .read("probe")
        .await
        .expect("read the probe");
    assert_eq!(probe.as_deref().map(Vec::as_slice), Some(&b"before"[..]));
    taker
        .realm
        .documents()
        .write("probe", Some(b"taken".to_vec()))
        .await
        .expect("the new holder writes");
}

/// Held-completion marks are documents of the realm: a restart reads them,
/// and so does a process on another host.
#[tokio::test]
async fn held_completion_marks_survive_a_restart_and_a_failover() {
    use crate::identity_first::held_bootstrap::{
        clear_host_store_mark, mark_host_store, read_host_store_marks,
    };
    let fixture = Fixture::new();
    let (alice, bob) = (id(ALICE), id(BOB));
    {
        let provider = fixture.open(&fixture.process()).await.expect("open");
        mark_host_store(provider.realm.documents(), &[alice.clone(), bob.clone()])
            .await
            .expect("mark both identities");
    }

    fixture.lapse();
    {
        let provider = fixture.open(&fixture.process()).await.expect("restart");
        let marked = read_host_store_marks(provider.realm.documents())
            .await
            .expect("read the marks");
        assert_eq!(marked, BTreeSet::from([alice.clone(), bob.clone()]));
        clear_host_store_mark(provider.realm.documents(), &alice)
            .await
            .expect("clear one mark");
    }

    fixture.lapse();
    let provider = fixture.open(&fixture.process()).await.expect("failover");
    let marked = read_host_store_marks(provider.realm.documents())
        .await
        .expect("read the marks");
    assert_eq!(marked, BTreeSet::from([bob]));
}

/// The structural-events cursor and the idle-retirement opt-ins are
/// documents of the realm: a process on another host reads both, so it
/// skips no event and keeps every opt-in.
#[tokio::test]
async fn the_events_cursor_and_idle_opt_ins_survive_a_failover() {
    let fixture = Fixture::new();
    let opt_in = MemberIdleRetireOverrideRecord {
        mob_id: "mob-a".to_string(),
        member_id: "worker-1".to_string(),
        session_id: SessionId::new(),
        policy: crate::mob_handle_runtime::DelegateIdleRetireOverride::Seconds(600),
        recorded_at: chrono::Utc::now(),
    };
    {
        let provider = fixture.open(&fixture.process()).await.expect("open");
        let metadata = SpiMetadataStore::new(provider.realm.documents().clone());
        metadata
            .set_subscription_cursor("mob-a", 41)
            .await
            .expect("set the cursor");
        metadata.cursor_writes_settled("mob-a").await;
        metadata
            .set_member_idle_retire_override(&opt_in)
            .await
            .expect("record the opt-in");
    }

    fixture.lapse();
    let provider = fixture.open(&fixture.process()).await.expect("failover");
    let metadata = SpiMetadataStore::new(provider.realm.documents().clone());
    assert_eq!(
        metadata
            .get_subscription_cursor("mob-a")
            .await
            .expect("read the cursor"),
        Some(41)
    );
    assert_eq!(
        metadata
            .load_member_idle_retire_overrides()
            .await
            .expect("load the opt-ins"),
        vec![opt_in.clone()]
    );
    metadata
        .clear_member_idle_retire_override(&opt_in)
        .await
        .expect("clear the opt-in");
    assert!(
        metadata
            .load_member_idle_retire_overrides()
            .await
            .expect("load the opt-ins")
            .is_empty()
    );
}

/// A burst of cursor checkpoints lands in a few batches, not one per event:
/// each write takes the newest cursor, one at a time. The newest cursor is
/// the one a restart reads, so no event after it is skipped.
#[tokio::test]
async fn a_burst_of_cursor_checkpoints_lands_in_a_few_batches_and_skips_nothing() {
    let fixture = Fixture::new();
    let host = fixture.process();
    {
        let provider = fixture.open(&host).await.expect("open");
        let metadata = SpiMetadataStore::new(provider.realm.documents().clone());
        let before = host.landed();
        for cursor in 1..=1_000_u64 {
            metadata
                .set_subscription_cursor("mob-a", cursor)
                .await
                .expect("set the cursor");
            // The subscription yields between its events.
            if cursor % 100 == 0 {
                tokio::task::yield_now().await;
            }
        }
        metadata.cursor_writes_settled("mob-a").await;
        let commits = host.landed() - before;
        // Bounded by the writer's turns (the subscription yielded ten
        // times), not by the 1,000 events.
        assert!(
            (1..=50).contains(&commits),
            "1,000 checkpoints landed in {commits} batches"
        );
    }

    fixture.lapse();
    let provider = fixture.open(&fixture.process()).await.expect("restart");
    let metadata = SpiMetadataStore::new(provider.realm.documents().clone());
    assert_eq!(
        metadata
            .get_subscription_cursor("mob-a")
            .await
            .expect("read the cursor"),
        Some(1_000)
    );
}

/// The host's fence timing shows in the storage census, and a restart waits
/// at most its lease.
#[tokio::test]
async fn a_host_fence_timing_shows_in_the_census_and_bounds_the_restart_wait() {
    let timing = FenceTiming::try_new(Duration::from_secs(5), Duration::from_secs(15))
        .expect("a fence timing");
    let fixture = Fixture::with_timing(timing);
    {
        let provider = fixture.open(&fixture.process()).await.expect("open");
        let census = provider
            .realm
            .census_slots(&[], Path::new("/var/cache/meta"));
        let realm = census
            .iter()
            .find(|slot| slot.durability.domain() == "host_store_realm")
            .expect("the census lists the realm");
        let detail = realm.detail.as_deref().unwrap_or_default();
        assert!(
            detail.contains("renew every 5s, lease 15s, self-fence window 10s"),
            "{detail}"
        );
    }

    let remaining = match fixture.open(&fixture.process()).await {
        Err(HostStoreOpenError::Held { remaining, .. }) => remaining,
        other => panic!("a restart meets Held, not {other:?}"),
    };
    assert!(remaining <= Duration::from_secs(15), "{remaining:?}");
    fixture.store.advance_clock(remaining);
    fixture
        .open(&fixture.process())
        .await
        .expect("the realm opens once the lease ran out");
}

/// A failed batch of one Meta document poisons that document's scope: it
/// refuses every later call, while the other documents serve and the
/// readiness probe reports it. A restart heals it, and the failed batch
/// never landed.
#[tokio::test]
async fn a_failed_document_batch_refuses_that_document_until_the_realm_reopens() {
    let fixture = Fixture::new();
    let host = fixture.process();
    {
        let provider = fixture
            .open_with(&host, failing_fast())
            .await
            .expect("open");
        let documents = provider.realm.documents();
        documents
            .write("cursor/mob-a", Some(b"first".to_vec()))
            .await
            .expect("the first write");
        host.fail_next_commits(1);
        let failed = documents
            .write("cursor/mob-a", Some(b"second".to_vec()))
            .await
            .expect_err("the store fails the batch");
        assert!(
            matches!(
                failed.spi(),
                Some(SpiError::Host(HostStoreError::Unavailable(_)))
            ),
            "{failed}"
        );
        let refused = documents
            .read("cursor/mob-a")
            .await
            .expect_err("the poisoned document refuses");
        assert!(
            matches!(refused.spi(), Some(SpiError::ScopePoisoned { .. })),
            "{refused}"
        );
        documents
            .write("cursor/mob-b", Some(b"other".to_vec()))
            .await
            .expect("another document still serves");
        let readiness = provider.readiness();
        assert!(
            readiness.scope_poisoned && readiness.realm_poisoned.is_none() && !readiness.is_ready(),
            "{readiness:?}"
        );
    }

    fixture.lapse();
    let provider = fixture.open(&fixture.process()).await.expect("restart");
    let documents = provider.realm.documents();
    let healed = documents
        .read("cursor/mob-a")
        .await
        .expect("the document reads again");
    assert_eq!(healed.as_deref().map(Vec::as_slice), Some(&b"first"[..]));
    documents
        .write("cursor/mob-a", Some(b"third".to_vec()))
        .await
        .expect("the document is written again");
    assert!(provider.readiness().is_ready());
}

/// A realm a newer Meta wrote refuses to open, typed.
#[tokio::test]
async fn a_realm_a_newer_meta_wrote_refuses_to_open() {
    let fixture = Fixture::new();
    {
        let provider = fixture.open(&fixture.process()).await.expect("open");
        let newer = serde_json::to_vec(&serde_json::json!({
            "format": "meerkat-meta.host-store-realm",
            "version": 2,
            "meerkat_level_realm": "meta",
            "created_by_meta": "0.10.0",
            "import": null,
        }))
        .expect("encode the record");
        provider
            .realm
            .documents()
            .write(REALM_RECORD_KEY, Some(newer))
            .await
            .expect("write the newer record");
    }

    fixture.lapse();
    match fixture.open(&fixture.process()).await {
        Err(HostStoreOpenError::RealmFormat { found, supported }) => {
            assert_eq!(found, 2);
            assert_eq!(supported, REALM_RECORD_VERSION);
        }
        other => panic!("a newer realm refuses typed, not {other:?}"),
    }
}

/// A provider serves one build: the realm handle it opened is the only one
/// of the process.
#[tokio::test]
async fn a_provider_serves_one_build() {
    let fixture = Fixture::new();
    let provider = fixture.open(&fixture.process()).await.expect("open");
    let cache = tempfile::tempdir().expect("cache");
    let ctx = MetaRealmOpenContext {
        layout: crate::storage_layout::MetaStorageLayout::declared_ephemeral(
            cache.path().to_path_buf(),
        ),
        state_dir: cache.path().to_path_buf(),
        declared_ephemeral_domains: Vec::new(),
        meerkat_realm_declaration: None,
    };
    assert!(
        provider.open_realm(&ctx).await.is_ok(),
        "the first build's store set"
    );
    match provider.open_realm(&ctx).await {
        Err(MetaStorageProviderError::Open { slot, message }) => {
            assert_eq!(slot, "host store realm");
            assert!(message.contains("already serves a build"), "{message}");
        }
        Err(other) => panic!("a second build is refused as one, not {other}"),
        Ok(_) => panic!("a second build over one provider must be refused"),
    }
}

/// The runtime store a build composes over the realm self-fences on the
/// realm: its model calls and dispatches stop with the realm's fence.
#[tokio::test]
async fn the_realm_runtime_store_self_fences_on_the_realm() {
    let fixture = Fixture::new();
    let provider = fixture.open(&fixture.process()).await.expect("open");
    let realm_id = provider
        .realm
        .meerkat_level_realm(None)
        .await
        .expect("the realm's meerkat-level realm");
    let composed = provider.realm.provider_meerkat_stores(realm_id);
    let fence = composed
        .runtime_store
        .self_fence()
        .expect("the realm's runtime store self-fences");
    assert!(
        std::ptr::addr_eq(Arc::as_ptr(&fence), Arc::as_ptr(provider.realm())),
        "the runtime store fences on the realm the provider opened"
    );
}

// ---------------------------------------------------------------------------
// Runtimes over the realm
// ---------------------------------------------------------------------------

/// A first build over a new realm writes the realm record, creates the root
/// mob storage in the realm with its composition record, and lists the
/// realm, its limits and the cache in the census; the cache holds no
/// authority. Each spawn landed in the realm: a new process reads every
/// identity's binding from it.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_build_keeps_the_realm_in_the_host_store_and_nothing_in_the_cache() {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("first-build");
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    let mut sessions = Vec::new();
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let runtime = build(builder(
            &provider,
            cache.path(),
            &mob_id,
            &[ALICE, BOB],
            &model,
        ))
        .await;
        for name in [ALICE, BOB] {
            sessions.push((name, session_of(&runtime, name).await));
        }

        let record = read_realm_record(provider.realm.documents())
            .await
            .expect("the realm record reads")
            .expect("the first build wrote the realm record");
        assert_eq!(
            record.meerkat_level_realm,
            crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID
        );
        assert_eq!(record.import, None);
        let stored: Vec<String> = provider
            .realm
            .mob_host()
            .stored_mobs()
            .into_iter()
            .map(|mob| mob.storage_key)
            .collect();
        assert_eq!(stored, vec![ROOT_MOB_STORAGE_KEY.to_string()]);
        let manifest = provider
            .realm
            .documents()
            .read(&composition_key(ROOT_MOB_STORAGE_KEY))
            .await
            .expect("the composition record reads")
            .expect("the create recorded the composition");
        let manifest: serde_json::Value =
            serde_json::from_slice(&manifest).expect("a composition manifest");
        assert_eq!(manifest["definition"]["id"], mob_id.as_str());

        let census = runtime
            .resolved_storage()
            .expect("the build records its storage census");
        let domains: BTreeSet<String> = census
            .slots
            .iter()
            .map(|slot| slot.durability.domain().to_string())
            .collect();
        for domain in [
            "host_store_realm",
            "sessions",
            "mob_storage",
            "continuity",
            "metadata",
            "jobs",
            "console",
            "child_mobs",
            "councils",
            "task_tools",
            "local_cache",
        ] {
            assert!(
                domains.contains(domain),
                "the census lists {domain}: {domains:?}"
            );
        }

        let authority: Vec<PathBuf> = files_under(cache.path())
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        STATE_DIR_AUTHORITY.contains(&name)
                            || name.ends_with(crate::mob_composition_manifest::MANIFEST_FILE_SUFFIX)
                    })
            })
            .collect();
        assert!(
            authority.is_empty(),
            "the cache holds no authority: {authority:?}"
        );
        stop(runtime).await;
    }

    let provider = reopen(&fixture).await;
    let stored = provider.realm.mob_host().stored_mobs();
    assert!(
        stored
            .iter()
            .any(|mob| mob.storage_key == ROOT_MOB_STORAGE_KEY
                && mob.mob_id == Some(meerkat_mob::MobId::from(mob_id.as_str()))),
        "{stored:?}"
    );
    let (storage, _provenance) =
        crate::mob_composition_manifest::host_store_mob_storage(&provider.realm)
            .await
            .expect("the realm's root mob storage");
    let history = storage
        .member_binding_history()
        .await
        .expect("the realm's member bindings");
    for (name, session) in &sessions {
        let member = crate::member_comms_id::roster_member_id_for_identity(name);
        let binding = history
            .last_binding(&member)
            .unwrap_or_else(|| panic!("the realm binds {name}"));
        assert_eq!(binding.session_id(), Some(session), "{name}'s session");
        assert_eq!(
            binding.continuity_intent(),
            &meerkat_mob::SpawnContinuityIntent::DurableIdentity {
                continuity_key: (*name).to_string(),
            },
            "{name} is a durable identity"
        );
    }
}

/// Two starts over one realm: the second resumes each identity on the
/// session the first committed, with its transcript. With `failover` the
/// second starts on another host, whose cache is empty. Both starts share
/// this process's comms identity directory (the per-host default), so the
/// failover proves the realm and the cache halves only; the members' key
/// pairs moving with the realm are
/// `a_failover_with_the_comms_identity_directory_on_a_moved_volume_recovers_every_member`.
async fn a_restart_resumes_each_identity(failover: bool) {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id(if failover { "failover" } else { "restart" });
    let model = Model::default();
    let first_cache = tempfile::tempdir().expect("cache");
    let mut sessions = Vec::new();
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let runtime = build(builder(
            &provider,
            first_cache.path(),
            &mob_id,
            &[ALICE, BOB],
            &model,
        ))
        .await;
        for name in [ALICE, BOB] {
            send(
                &runtime,
                &provider,
                &model,
                name,
                &format!("Remember this token: {}", token(name)),
            )
            .await;
            sessions.push((name, session_of(&runtime, name).await));
        }
        stop(runtime).await;
    }

    let cache = if failover {
        drop(first_cache);
        tempfile::tempdir().expect("another host's empty cache")
    } else {
        first_cache
    };
    let provider = reopen(&fixture).await;
    let runtime = build(builder(
        &provider,
        cache.path(),
        &mob_id,
        &[ALICE, BOB],
        &model,
    ))
    .await;
    for (name, session) in &sessions {
        assert_eq!(
            &session_of(&runtime, name).await,
            session,
            "{name} resumes on its session"
        );
        let requests = send(
            &runtime,
            &provider,
            &model,
            name,
            "Which token did I give you?",
        )
        .await;
        assert!(
            requests
                .iter()
                .any(|request| request.contains(&token(name))),
            "{name}'s turn replays its committed transcript"
        );
    }
    stop(runtime).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restart_on_the_same_host_resumes_each_identity_on_its_session() {
    a_restart_resumes_each_identity(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failover_onto_an_empty_cache_resumes_each_identity_on_its_session() {
    a_restart_resumes_each_identity(true).await;
}

/// The storage census entry of the members' comms identities.
fn comms_identity_slot(runtime: &UnifiedRuntime) -> crate::storage_health::StorageSlotSummary {
    runtime
        .resolved_storage()
        .expect("the build records its storage census")
        .slots
        .into_iter()
        .find(|slot| slot.durability.domain() == "comms_identity")
        .expect("the census lists the members' comms identities")
}

/// With the members' comms identity directory on a volume that moves with
/// the realm (`session_identity_config_root`), a start on another host, on
/// an empty cache of its own, recovers every member on its session: their
/// key pairs are on the volume, and the per-host default directory holds
/// none of them, so the start does not depend on the host's home.
#[tokio::test(flavor = "multi_thread")]
async fn a_failover_with_the_comms_identity_directory_on_a_moved_volume_recovers_every_member() {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("moved-keys");
    let model = Model::default();
    let volume = tempfile::tempdir().expect("a volume that moves with the realm");
    let keys = volume.path().join(".rkat").join("session_comms_identity");
    let mut sessions = Vec::new();
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let cache = tempfile::tempdir().expect("the first host's cache");
        let runtime = build(
            builder(&provider, cache.path(), &mob_id, &[ALICE, BOB], &model)
                .session_identity_config_root(volume.path()),
        )
        .await;
        for name in [ALICE, BOB] {
            send(
                &runtime,
                &provider,
                &model,
                name,
                &format!("Remember this token: {}", token(name)),
            )
            .await;
            sessions.push((name, session_of(&runtime, name).await));
        }
        stop(runtime).await;
    }
    let ambient = meerkat::canonical_session_comms_identity_root(None).ok();
    for (name, session) in &sessions {
        assert!(
            keys.join(session.to_string())
                .join("identity.key")
                .is_file(),
            "{name}'s key pair is on the volume"
        );
        if let Some(ambient) = ambient.as_ref() {
            assert!(
                !ambient.join(session.to_string()).exists(),
                "no ambient directory holds {name}'s key pair"
            );
        }
    }

    // Another host: an empty cache of its own, and the volume moved with the
    // realm.
    let provider = reopen(&fixture).await;
    let cache = tempfile::tempdir().expect("another host's empty cache");
    let runtime = build(
        builder(&provider, cache.path(), &mob_id, &[ALICE, BOB], &model)
            .session_identity_config_root(volume.path()),
    )
    .await;
    let slot = comms_identity_slot(&runtime);
    assert!(!slot.degraded, "{slot:?}");
    assert_eq!(slot.backend, keys.display().to_string());
    let readiness = provider.readiness();
    assert!(readiness.is_ready(), "{readiness:?}");
    for member in runtime.mob_handle().list_members().await {
        assert_eq!(
            member.status,
            meerkat_mob::MobMemberStatus::Active,
            "{} is healthy: {:?}",
            member.agent_identity,
            member.error
        );
    }
    for (name, session) in &sessions {
        assert_eq!(
            &session_of(&runtime, name).await,
            session,
            "{name} resumes on its session"
        );
        let requests = send(
            &runtime,
            &provider,
            &model,
            name,
            "Which token did I give you?",
        )
        .await;
        assert!(
            requests
                .iter()
                .any(|request| request.contains(&token(name))),
            "{name}'s turn replays its committed transcript"
        );
    }
    stop(runtime).await;
}

/// A member restored without its key pair stays broken, as since 0.9.4, and
/// the storage census names the directory that lost it, its resolved path
/// and the setting that places it: here the comms identity directory is
/// emptied between two starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_restored_without_its_key_pair_is_broken_and_the_census_names_the_directory() {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("lost-keys");
    let model = Model::default();
    let volume = tempfile::tempdir().expect("the comms identity volume");
    let keys = volume.path().join(".rkat").join("session_comms_identity");
    let cache = tempfile::tempdir().expect("cache");
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let runtime = build(
            builder(&provider, cache.path(), &mob_id, &[ALICE], &model)
                .session_identity_config_root(volume.path()),
        )
        .await;
        stop(runtime).await;
    }
    // The directory is emptied: alice's key pair is gone.
    for entry in std::fs::read_dir(&keys).expect("the comms identity directory") {
        std::fs::remove_dir_all(entry.expect("a key pair's directory").path())
            .expect("remove the key pair");
    }

    let provider = reopen(&fixture).await;
    let runtime = build(
        builder(&provider, cache.path(), &mob_id, &[ALICE], &model)
            .session_identity_config_root(volume.path()),
    )
    .await;
    let alice = crate::member_comms_id::roster_member_id_for_identity(ALICE);
    let member = runtime
        .mob_handle()
        .list_members()
        .await
        .into_iter()
        .find(|member| member.agent_identity == alice)
        .expect("the realm restores alice");
    assert_eq!(
        member.status,
        meerkat_mob::MobMemberStatus::Broken,
        "{:?}",
        member.error
    );
    let slot = comms_identity_slot(&runtime);
    assert!(slot.degraded, "{slot:?}");
    let detail = slot.detail.unwrap_or_default();
    for named in [
        keys.display().to_string(),
        "holds no key pair".to_string(),
        alice.to_string(),
        "UnifiedRuntimeBuilder::session_identity_config_root".to_string(),
    ] {
        assert!(
            detail.contains(&named),
            "the census names {named}: {detail}"
        );
    }
    let readiness = provider.readiness();
    assert!(
        !readiness.is_ready()
            && readiness.comms_identity_degraded
            && readiness.broken_members == vec![ALICE.to_string()],
        "the readiness probe reads red and names the member: {readiness:?}"
    );
    stop(runtime).await;
}

/// A definition edited between two starts is refused, and the refusal names
/// the realm's composition record.
#[tokio::test(flavor = "multi_thread")]
async fn a_definition_edited_between_restarts_is_refused_naming_the_realm_document() {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("edited");
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let runtime = build(builder(&provider, cache.path(), &mob_id, &[ALICE], &model)).await;
        stop(runtime).await;
    }

    let provider = reopen(&fixture).await;
    let edited = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.personal]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.personal.tools]\ncomms = true\n\n[profiles.reviewer]\n\
         model = \"gpt-5.5\"\nruntime_mode = \"turn_driven\"\n"
    ))
    .expect("the edited definition parses");
    let refused = match Box::pin(
        builder(&provider, cache.path(), &mob_id, &[ALICE], &model)
            .definition(edited)
            .build(),
    )
    .await
    {
        Ok(runtime) => {
            stop(runtime).await;
            panic!("an edited definition must be refused");
        }
        Err(error) => error.to_string(),
    };
    assert!(
        refused.contains(&format!(
            "host-store:{PROVIDER}:composition/{ROOT_MOB_STORAGE_KEY}"
        )),
        "{refused}"
    );
    assert!(refused.contains("diverges"), "{refused}");
}

/// A realm whose mob was created but whose composition record never landed
/// (a crash between the two) is resumed by the next start, which records
/// the composition it was given.
#[tokio::test(flavor = "multi_thread")]
async fn a_composition_record_lost_after_the_create_is_recorded_by_the_next_start() {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("unrecorded");
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let runtime = build(builder(&provider, cache.path(), &mob_id, &[ALICE], &model)).await;
        stop(runtime).await;
    }

    let provider = reopen(&fixture).await;
    let key = composition_key(ROOT_MOB_STORAGE_KEY);
    provider
        .realm
        .documents()
        .write(&key, None)
        .await
        .expect("remove the composition record");
    let runtime = build(builder(&provider, cache.path(), &mob_id, &[ALICE], &model)).await;
    let manifest = provider
        .realm
        .documents()
        .read(&key)
        .await
        .expect("the composition record reads")
        .expect("the start recorded the composition");
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest).expect("a composition manifest");
    assert_eq!(manifest["definition"]["id"], mob_id.as_str());
    stop(runtime).await;
}

/// A member the realm restores for an identity the roster no longer lists is
/// held while the build reads the realm's member bindings, and the roster
/// pass retires it before it runs anything. The hold list the build reads
/// from the realm before the mob is built names both restored members.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_member_the_roster_dropped_is_retired_before_it_runs() {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("dropped");
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let runtime = build(
            builder(&provider, cache.path(), &mob_id, &[ALICE, BOB], &model)
                .agent_customizer(Arc::new(PlainCustomizer)),
        )
        .await;
        stop(runtime).await;
    }

    let provider = reopen(&fixture).await;
    let (alice, bob) = (
        crate::member_comms_id::roster_member_id_for_identity(ALICE),
        crate::member_comms_id::roster_member_id_for_identity(BOB),
    );
    // The build holds the restored members whose identities Core manages by
    // command, as it reads them from the realm before the mob is built.
    {
        let spec = host_store_spec(
            &fixture,
            &provider,
            cache.path(),
            definition(&mob_id),
            Arc::new(model.clone()),
        )
        .await;
        let mut held = spec
            .stored_command_driven_identity_members()
            .await
            .expect("the realm's member bindings");
        held.sort();
        let mut restored = vec![alice.clone(), bob.clone()];
        restored.sort();
        assert_eq!(held, restored, "the build holds both restored members");
    }
    let before = model.count();
    let runtime = build(
        builder(&provider, cache.path(), &mob_id, &[ALICE], &model)
            .agent_customizer(Arc::new(PlainCustomizer)),
    )
    .await;
    let members: Vec<meerkat_mob::AgentIdentity> = runtime
        .mob_handle()
        .list_members()
        .await
        .into_iter()
        .map(|member| member.agent_identity)
        .collect();
    assert!(members.contains(&alice), "{members:?}");
    assert!(
        !members.contains(&bob),
        "the roster pass retired bob: {members:?}"
    );
    let history = runtime
        .mob_handle()
        .member_binding_history()
        .await
        .expect("the realm's member bindings");
    assert!(
        history
            .last_binding(&bob)
            .is_some_and(|binding| !binding.is_live()),
        "bob's binding is retired"
    );
    assert_eq!(model.count(), before, "nothing ran");
    stop(runtime).await;
}

/// The roster's profile and a durable worker profile, which a member's mob
/// tools spawn from at runtime.
fn worker_definition(mob_id: &str) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.personal]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.personal.tools]\ncomms = true\n\n[profiles.worker]\n\
         model = \"gpt-5.5\"\nruntime_mode = \"turn_driven\"\ndurable = true\n\n\
         [profiles.worker.tools]\ncomms = true\n"
    ))
    .expect("the worker definition parses")
}

/// A durable worker spawned at runtime on a host store realm, as a member's
/// mob tools spawn it, is the roster's neither to hold nor to retire: the
/// hold list the build reads from the realm names only the roster's
/// identity, and after the restart the worker is still an active member.
#[tokio::test(flavor = "multi_thread")]
async fn a_durable_worker_spawned_at_runtime_is_neither_held_nor_retired_at_a_restart() {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("spawned-worker");
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    let worker = meerkat_mob::AgentIdentity::from("worker-1");
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let runtime = build(
            builder(&provider, cache.path(), &mob_id, &[ALICE], &model)
                .definition(worker_definition(&mob_id)),
        )
        .await;
        runtime
            .mob_handle()
            .spawn_spec(meerkat_mob::SpawnMemberSpec::new("worker", worker.clone()))
            .await
            .expect("spawn the durable worker");
        stop(runtime).await;
    }

    let provider = reopen(&fixture).await;
    {
        let spec = host_store_spec(
            &fixture,
            &provider,
            cache.path(),
            worker_definition(&mob_id),
            Arc::new(model.clone()),
        )
        .await;
        let held = spec
            .stored_command_driven_identity_members()
            .await
            .expect("the realm's member bindings");
        assert_eq!(
            held,
            vec![crate::member_comms_id::roster_member_id_for_identity(ALICE)],
            "the hold list names the roster's identity, never the worker"
        );
    }
    let runtime = build(
        builder(&provider, cache.path(), &mob_id, &[ALICE], &model)
            .definition(worker_definition(&mob_id)),
    )
    .await;
    let members = runtime.mob_handle().list_members().await;
    let entry = members
        .iter()
        .find(|member| member.agent_identity == worker)
        .unwrap_or_else(|| panic!("the roster pass kept the worker: {members:?}"));
    assert_eq!(
        entry.status,
        meerkat_mob::MobMemberStatus::Active,
        "{:?}",
        entry.error
    );
    stop(runtime).await;
}

/// A build on another host's empty cache runs over the metadata documents
/// the realm keeps: its metadata store reads the structural-events cursor
/// the first process checkpointed there, and the build restores the
/// idle-retirement opt-in recorded there.
#[tokio::test(flavor = "multi_thread")]
async fn a_failover_build_reads_the_events_cursor_and_opt_ins_the_realm_keeps() {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("metadata-failover");
    let model = Model::default();
    let member_id = crate::member_comms_id::roster_member_id_for_identity(ALICE).to_string();
    let policy = crate::mob_handle_runtime::DelegateIdleRetireOverride::Seconds(600);
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let cache = tempfile::tempdir().expect("cache");
        let runtime = build(builder(&provider, cache.path(), &mob_id, &[ALICE], &model)).await;
        send(
            &runtime,
            &provider,
            &model,
            ALICE,
            "a turn before the failover",
        )
        .await;
        let opt_in = MemberIdleRetireOverrideRecord {
            mob_id: mob_id.clone(),
            member_id: member_id.clone(),
            session_id: session_of(&runtime, ALICE).await,
            policy,
            recorded_at: chrono::Utc::now(),
        };
        runtime
            .persistent_metadata()
            .set_member_idle_retire_override(&opt_in)
            .await
            .expect("record the opt-in");
        // The structural-events subscription checkpoints into the realm.
        let realm_metadata = SpiMetadataStore::new(provider.realm.documents().clone());
        eventually_holds("the realm to hold the events cursor", || async {
            realm_metadata
                .get_subscription_cursor(&mob_id)
                .await
                .is_ok_and(|cursor| cursor.is_some())
        })
        .await;
        stop(runtime).await;
    }

    let provider = reopen(&fixture).await;
    let landed = SpiMetadataStore::new(provider.realm.documents().clone())
        .get_subscription_cursor(&mob_id)
        .await
        .expect("read the realm's events cursor")
        .expect("the realm holds the events cursor");
    let empty_cache = tempfile::tempdir().expect("another host's empty cache");
    let runtime = build(builder(
        &provider,
        empty_cache.path(),
        &mob_id,
        &[ALICE],
        &model,
    ))
    .await;
    let resumed = runtime
        .persistent_metadata()
        .get_subscription_cursor(&mob_id)
        .await
        .expect("the build's metadata store reads")
        .expect("the build reads the realm's events cursor");
    assert!(
        resumed >= landed,
        "the subscription resumes at {resumed}, at or after the realm's {landed}"
    );
    let restored = runtime
        .mob_runtime()
        .implicit_delegate_retirement_overrides()
        .expect("the runtime keeps idle-retirement opt-ins")
        .get(&mob_id, &member_id)
        .await;
    assert_eq!(
        restored,
        Some(policy),
        "the build restored the realm's opt-in"
    );
    stop(runtime).await;
}

/// A build on another host's empty cache reads the held-completion marks
/// the realm keeps: an identity a held pass left owing its host a fresh held
/// completion is refused a wake outside a held pass, typed, while an
/// unmarked identity takes its turn.
#[tokio::test(flavor = "multi_thread")]
async fn a_failover_build_reads_the_held_completion_marks_the_realm_keeps() {
    use crate::identity_first::held_bootstrap::mark_host_store;
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("held-marks-failover");
    let model = Model::default();
    {
        // An earlier process's held pass left carol unpublished.
        let provider = fixture.open(&fixture.process()).await.expect("open");
        mark_host_store(provider.realm.documents(), &[id(CAROL)])
            .await
            .expect("mark carol");
    }

    fixture.lapse();
    let provider = Arc::new(fixture.open(&fixture.process()).await.expect("failover"));
    let empty_cache = tempfile::tempdir().expect("another host's empty cache");
    let runtime = build(
        builder(
            &provider,
            empty_cache.path(),
            &mob_id,
            &[ALICE, CAROL],
            &model,
        )
        .identity_bootstrap_mode(crate::identity_first::IdentityBootstrapMode::LazyMaterialize),
    )
    .await;
    let before = model.count();
    match runtime
        .identity_runtime()
        .expect("an identity-first runtime")
        .send(
            &id(CAROL),
            &ContentInput::Text("a wake outside a held pass".to_string()),
        )
        .await
    {
        Err(crate::identity_first::IdentityRuntimeError::HeldCompletionOwed { identity }) => {
            assert_eq!(identity, id(CAROL));
        }
        Err(other) => panic!("carol owes a held completion, not: {other}"),
        Ok(_) => panic!("carol owes a held completion: her wake must be refused"),
    }
    assert_eq!(model.count(), before, "carol's turn reached nothing");
    send(
        &runtime,
        &provider,
        &model,
        ALICE,
        "an unmarked identity's turn",
    )
    .await;
    stop(runtime).await;
}

/// A host store realm refuses every setting that would keep part of its
/// authority elsewhere, typed, before anything runs: the state directory
/// seams, a mob storage, the continuity pair, mob announcements, the
/// persistent memory stack without a provider memory, and the topology
/// control plane.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_store_realm_refuses_settings_that_keep_its_authority_elsewhere() {
    let scratch = tempfile::tempdir().expect("scratch");
    let state = scratch.path().join("state");
    let model = Model::default();
    // Each case opens its own realm: a refused build may already have taken
    // its provider's one store set.
    let fresh = |test: &'static str| {
        let cache = scratch.path().join(test);
        let model = model.clone();
        async move {
            let fixture = Fixture::new();
            let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
            builder(&provider, &cache, &unique_mob_id(test), &[ALICE], &model)
        }
    };

    let refused = refused_configuration(fresh("state-dir").await.persistent_state(&state)).await;
    assert!(refused.contains("persistent_state()"), "{refused}");

    let refused = refused_configuration(fresh("mob-storage").await.mob_storage(
        meerkat_mob::MobStorage::in_memory(),
        crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
    ))
    .await;
    assert!(refused.contains("mob_storage()"), "{refused}");

    let refused = refused_configuration(
        fresh("continuity-pair")
            .await
            .continuity_store(Arc::new(
                LocalContinuityStore::in_memory().expect("a continuity store"),
            ))
            .lease_provider(Arc::new(crate::identity_first::LocalLeaseProvider::new())),
    )
    .await;
    assert!(
        refused.contains("continuity_store()") && refused.contains("lease_provider()"),
        "{refused}"
    );

    let from_state_dir = fresh("continuity-state-dir")
        .await
        .continuity_from_state_dir(&state)
        .await
        .expect("open the state directory's continuity substrate");
    let refused = refused_configuration(from_state_dir).await;
    assert!(refused.contains("continuity_from_state_dir()"), "{refused}");

    let refused = refused_configuration(fresh("memory-stack").await.persistent_agent_memory_stack(
        crate::identity_first::AgentMemoryConfig::default(),
        crate::memory_wiring::MemoryEnginesConfig::default(),
    ))
    .await;
    assert!(
        refused.contains("persistent_agent_memory_stack() needs a provider memory"),
        "{refused}"
    );

    let editable = crate::topology_control::TopologyControlPolicy {
        mode: crate::topology_control::TopologyControlMode::Editable,
        ..crate::topology_control::TopologyControlPolicy::default()
    };
    let refused = refused_configuration(
        fresh("topology-control")
            .await
            .topology_control(editable)
            .expect("a valid policy"),
    )
    .await;
    assert!(
        refused.contains("topology_control() with mode Editable"),
        "{refused}"
    );
}

/// A build over a host store realm composes mob announcements, which keep
/// their tables in the realm: the owner attaches, the storage census lists
/// the announcement store on the realm, and the per-host cache holds no
/// announcement database.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_store_realm_composes_mob_announcements_in_the_realm() {
    let fixture = Fixture::new();
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
    let runtime = build(
        builder(
            &provider,
            cache.path(),
            &unique_mob_id("announcements"),
            &[ALICE],
            &model,
        )
        .mob_announcements(crate::mob_announcements::AnnouncementConfig::default()),
    )
    .await;
    assert!(
        runtime.mob_announcements().is_some(),
        "the announcement owner is attached"
    );
    let slot = runtime
        .resolved_storage()
        .expect("the build records its storage census")
        .slots
        .into_iter()
        .find(|slot| slot.durability.domain() == "mob_announcements")
        .expect("the census lists the announcement store");
    assert!(
        slot.backend.contains(PROVIDER) && !slot.degraded,
        "{slot:?}"
    );
    assert_eq!(
        slot.durability
            .declaration()
            .map(|declaration| declaration.resolution),
        Some(DurabilityResolution::Persistent),
        "{slot:?}"
    );
    stop(runtime).await;
    let databases: Vec<PathBuf> = files_under(cache.path())
        .into_iter()
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with(crate::storage_layout::MOB_ANNOUNCEMENTS_DB_FILE_NAME)
            })
        })
        .collect();
    assert!(
        databases.is_empty(),
        "the cache holds no announcement database: {databases:?}"
    );
}

// ---------------------------------------------------------------------------
// Mob announcements on the realm
// ---------------------------------------------------------------------------

/// The announcement owner the build attached to `runtime`.
fn announcements_of(runtime: &UnifiedRuntime) -> crate::mob_announcements::AnnouncementService {
    runtime
        .mob_announcements()
        .expect("the announcement owner is attached")
}

/// One `meta/mob_announcements/*` call to `service` as the host of the mob
/// `mob_id`, keeping a refusal.
async fn try_announcement_call(
    service: &crate::mob_announcements::AnnouncementService,
    mob_id: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, crate::rpc::JsonRpcError> {
    let own = crate::mob_announcements::MobId::new(mob_id).expect("a mob id");
    crate::mob_announcements::rpc::handle_announcement_method(Some(service), &own, method, &params)
        .await
}

/// [`try_announcement_call`], which must answer.
async fn announcement_call(
    service: &crate::mob_announcements::AnnouncementService,
    mob_id: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    try_announcement_call(service, mob_id, method, params)
        .await
        .unwrap_or_else(|error| panic!("{method}: {error:?}"))
}

/// A source "inbox" of the mob `mob_id` on "mail.received", read through a
/// connector setup, whose audience is Alice, run by `runner` when one is
/// given: the declaration's form, and `sources/put`'s with an operation id.
fn declared_inbox(mob_id: &str, runner: Option<serde_json::Value>) -> serde_json::Value {
    let mut source = serde_json::json!({
        "source_id": "inbox",
        "identity": {
            "provider": "google.gmail",
            "principal": inbox_principal(),
            "scope": { "kind": "mailbox_label", "id": "INBOX" },
        },
        "connection": { "kind": "connect_setup", "id": "setup-1" },
        "observation": { "baseline": "delta", "deletions": "explicit" },
        "topics": ["mail.received"],
        "audience": [
            { "kind": "member", "member": { "mob_id": mob_id, "identity": ALICE } }
        ],
        "routes": [{ "kind": "direct" }],
    });
    if let Some(runner) = runner {
        source["runner"] = runner;
    }
    source
}

/// The verified account "inbox" is read with.
fn inbox_principal() -> serde_json::Value {
    serde_json::json!({
        "kind": "connector_verified_account",
        "issuer": "https://accounts.example",
        "strategy_id": "oauth-code",
        "subject": "subject-1",
    })
}

/// A declared subscription `id` of Alice to "mail.received".
fn declared_subscription(mob_id: &str, id: &str) -> serde_json::Value {
    serde_json::json!({
        "subscription_id": id,
        "member": { "mob_id": mob_id, "identity": ALICE },
        "topics": ["mail.received"],
        "start": { "kind": "now" },
    })
}

/// Mob announcements as a gateway's options configure them: `declarations`
/// inline, or from a sidecar file the host writes under `sidecars`.
fn announcement_options(
    declarations: serde_json::Value,
    sidecars: Option<&Path>,
) -> crate::mob_announcements::AnnouncementConfig {
    let options = match sidecars {
        Some(dir) => {
            let path = dir.join(format!("declarations-{}.json", declarations["revision"]));
            std::fs::write(&path, declarations.to_string()).expect("the sidecar is written");
            serde_json::json!({ "enabled": true, "declarations_path": path })
        }
        None => serde_json::json!({ "enabled": true, "declarations": declarations }),
    };
    crate::mob_announcements::parse_options(&options, "mob_announcements")
        .expect("the options parse")
        .expect("mob announcements are enabled")
}

/// What `declarations/get` reports once the declarations' revision is
/// `revision` and every record they manage is applied.
async fn applied_declarations(
    service: &crate::mob_announcements::AnnouncementService,
    mob_id: &str,
    revision: u64,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_mins(1);
    loop {
        let view = try_announcement_call(
            service,
            mob_id,
            "meta/mob_announcements/declarations/get",
            serde_json::json!({}),
        )
        .await;
        if let Ok(view) = &view
            && view["revision"] == serde_json::json!(revision)
            && view["records"].as_array().is_some_and(|records| {
                !records.is_empty()
                    && records
                        .iter()
                        .all(|record| record["status"] == serde_json::json!("applied"))
            })
        {
            return view.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "revision {revision} was never applied: {view:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// How many sources and subscriptions the announcement store holds.
async fn announcement_records(
    service: &crate::mob_announcements::AnnouncementService,
    mob_id: &str,
) -> (usize, usize) {
    let sources = announcement_call(
        service,
        mob_id,
        "meta/mob_announcements/sources/list",
        serde_json::json!({}),
    )
    .await;
    let subscriptions = announcement_call(
        service,
        mob_id,
        "meta/mob_announcements/subscriptions/list",
        serde_json::json!({}),
    )
    .await;
    (
        sources["sources"].as_array().map_or(0, Vec::len),
        subscriptions["subscriptions"]
            .as_array()
            .map_or(0, Vec::len),
    )
}

/// Declarations reconcile on a host store realm as on a state directory,
/// whether they come inline or from a sidecar file. The first start
/// applies them; a start on another host, onto an empty cache, finds the
/// applied revision and its records and changes nothing; a higher revision
/// applies once; and a lower one is refused as a rollback, the applied one
/// standing.
async fn declarations_apply_once_across_failovers(sidecar: bool) {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id(if sidecar {
        "declared-sidecar"
    } else {
        "declared-inline"
    });
    let model = Model::default();
    let sidecars = tempfile::tempdir().expect("the host's sidecar directory");
    let declarations = |revision: u64, subscriptions: &[&str]| {
        serde_json::json!({
            "revision": revision,
            "sources": [declared_inbox(&mob_id, None)],
            "subscriptions": subscriptions
                .iter()
                .map(|id| declared_subscription(&mob_id, id))
                .collect::<Vec<_>>(),
        })
    };
    let options = |revision: u64, subscriptions: &[&str]| {
        announcement_options(
            declarations(revision, subscriptions),
            sidecar.then(|| sidecars.path()),
        )
    };
    let start = |provider: Arc<HostStoreMetaStorageProvider>,
                 config: crate::mob_announcements::AnnouncementConfig| {
        let model = model.clone();
        let mob_id = mob_id.clone();
        async move {
            let cache = tempfile::tempdir().expect("an empty cache on this host");
            let runtime = build(
                builder(&provider, cache.path(), &mob_id, &[ALICE], &model)
                    .mob_announcements(config),
            )
            .await;
            (runtime, cache)
        }
    };

    // The first start applies revision 1.
    let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
    let (runtime, _cache) = start(provider, options(1, &["declared-a"])).await;
    let service = announcements_of(&runtime);
    let first = applied_declarations(&service, &mob_id, 1).await;
    assert_eq!(announcement_records(&service, &mob_id).await, (1, 1));
    stop(runtime).await;

    // A start on another host, onto an empty cache, changes nothing.
    let (runtime, _cache) = start(reopen(&fixture).await, options(1, &["declared-a"])).await;
    let service = announcements_of(&runtime);
    assert_eq!(
        applied_declarations(&service, &mob_id, 1).await,
        first,
        "the same revision and records"
    );
    assert_eq!(
        announcement_records(&service, &mob_id).await,
        (1, 1),
        "no source or subscription twice"
    );
    stop(runtime).await;

    // A higher revision applies once.
    let (runtime, _cache) = start(
        reopen(&fixture).await,
        options(2, &["declared-a", "declared-b"]),
    )
    .await;
    let service = announcements_of(&runtime);
    let second = applied_declarations(&service, &mob_id, 2).await;
    assert_eq!(announcement_records(&service, &mob_id).await, (1, 2));
    stop(runtime).await;
    let (runtime, _cache) = start(
        reopen(&fixture).await,
        options(2, &["declared-a", "declared-b"]),
    )
    .await;
    let service = announcements_of(&runtime);
    assert_eq!(
        applied_declarations(&service, &mob_id, 2).await,
        second,
        "applied once"
    );
    assert_eq!(announcement_records(&service, &mob_id).await, (1, 2));

    // A lower revision is refused as a rollback.
    let refused = try_announcement_call(
        &service,
        &mob_id,
        "meta/mob_announcements/declarations/apply",
        serde_json::json!({
            "revision": 1,
            "declarations": declarations(1, &["declared-a"]),
        }),
    )
    .await
    .expect_err("a lower revision is refused");
    assert_eq!(
        refused.data,
        Some(serde_json::json!({ "kind": "declaration_rollback", "applied": 2 }))
    );
    assert_eq!(applied_declarations(&service, &mob_id, 2).await, second);
    stop(runtime).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_declarations_apply_once_across_failovers_of_a_host_store_realm() {
    declarations_apply_once_across_failovers(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sidecar_declarations_apply_once_across_failovers_of_a_host_store_realm() {
    declarations_apply_once_across_failovers(true).await;
}

/// What a build's announcements keep that a start on another host must
/// find again: the subscription, the member's deliveries with their
/// handling, and the source's cursor.
async fn announcement_state(
    service: &crate::mob_announcements::AnnouncementService,
    mob_id: &str,
) -> (serde_json::Value, serde_json::Value, serde_json::Value) {
    let subscription = announcement_call(
        service,
        mob_id,
        "meta/mob_announcements/subscriptions/get",
        serde_json::json!({ "subscription_id": "sub-1" }),
    )
    .await["subscription"]
        .clone();
    let deliveries = announcement_call(
        service,
        mob_id,
        "meta/mob_announcements/deliveries/list",
        serde_json::json!({ "member": { "mob_id": mob_id, "identity": ALICE } }),
    )
    .await;
    let cursor = announcement_call(
        service,
        mob_id,
        "meta/mob_announcements/sources/get",
        serde_json::json!({ "source_id": "inbox" }),
    )
    .await["source"]["cursor"]
        .clone();
    (subscription, deliveries, cursor)
}

/// Mob announcements on a host store realm, end to end: Alice subscribes,
/// an announcement reaches her as one batch her turn handles, and a source
/// takes its first step. A start on another host, onto an empty cache,
/// finds the subscription, the delivery with its handling and the source's
/// cursor as they were, and delivers nothing again.
#[tokio::test(flavor = "multi_thread")]
async fn mob_announcements_survive_a_failover_and_deliver_once() {
    const BODY: &str = "ANNOUNCED-ONCE-4Q: the release train leaves at noon";
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("announcements-e2e");
    let model = Model::default();
    let config = crate::mob_announcements::AnnouncementConfig {
        policy: crate::mob_announcements::SealingPolicy {
            max_count: 1,
            max_bytes: 1_048_576,
            max_delay_ms: 50,
            min_wake_interval_ms: 0,
        },
        ..crate::mob_announcements::AnnouncementConfig::default()
    };

    let cache = tempfile::tempdir().expect("cache");
    let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
    let runtime = build(
        builder(&provider, cache.path(), &mob_id, &[ALICE], &model)
            .mob_announcements(config.clone()),
    )
    .await;
    let service = announcements_of(&runtime);
    announcement_call(
        &service,
        &mob_id,
        "meta/mob_announcements/subscriptions/put",
        serde_json::json!({
            "operation_id": "subscribe-1",
            "subscription_id": "sub-1",
            "member": { "mob_id": mob_id, "identity": ALICE },
            "topics": ["release"],
        }),
    )
    .await;
    let announced = announcement_call(
        &service,
        &mob_id,
        "meta/mob_announcements/announce",
        serde_json::json!({ "source_key": "k-1", "topics": ["release"], "body": BODY }),
    )
    .await;
    assert_eq!(announced["captured"], 1, "{announced}");
    let deadline = tokio::time::Instant::now() + Duration::from_mins(1);
    loop {
        let listed = announcement_call(
            &service,
            &mob_id,
            "meta/mob_announcements/deliveries/list",
            serde_json::json!({ "member": { "mob_id": mob_id, "identity": ALICE } }),
        )
        .await;
        if listed["deliveries"][0]["handling"]["state"] == serde_json::json!("handled") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Alice's turn never handled her batch: {listed}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut put = declared_inbox(&mob_id, None);
    put["operation_id"] = serde_json::json!("put-inbox");
    announcement_call(&service, &mob_id, "meta/mob_announcements/sources/put", put).await;
    let step = announcement_call(
        &service,
        &mob_id,
        "meta/mob_announcements/sources/advance",
        serde_json::json!({
            "operation_id": "baseline",
            "source_id": "inbox",
            "expected_revision": 1,
            "expected_version": 0,
            "principal": inbox_principal(),
            "next_cursor": "c1",
            "coverage": "complete",
        }),
    )
    .await;
    assert_eq!(step["source"]["cursor"]["version"], 1, "{step}");
    let kept = announcement_state(&service, &mob_id).await;
    let carried = model.requests_carrying(BODY);
    assert!(carried >= 1, "the batch reached Alice's turn");
    stop(runtime).await;

    let provider = reopen(&fixture).await;
    let cache = tempfile::tempdir().expect("an empty cache on another host");
    let runtime = build(
        builder(&provider, cache.path(), &mob_id, &[ALICE], &model).mob_announcements(config),
    )
    .await;
    let service = announcements_of(&runtime);
    assert_eq!(
        announcement_state(&service, &mob_id).await,
        kept,
        "the subscription, the delivery and the cursor as they were"
    );
    // Rounds of the new start find nothing owed and nothing to settle.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(announcement_state(&service, &mob_id).await, kept);
    assert_eq!(
        model.requests_carrying(BODY),
        carried,
        "nothing is delivered again"
    );
    stop(runtime).await;
}

/// On a host store realm Meta schedules no source runner: a declared source
/// with a runner leaves the host's schedule store without a row and says it
/// is not scheduled here. The host's own scheduler drives the source
/// through advance_source, and one step is one batch of the realm.
#[tokio::test(flavor = "multi_thread")]
async fn a_declared_runner_is_left_to_the_host_and_one_step_is_one_batch() {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("announcements-runner");
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    let host = fixture.process();
    let provider = Arc::new(fixture.open(&host).await.expect("open"));
    let runner = serde_json::json!({ "host_runnable": "collect-inbox", "every_seconds": 300 });
    let options = announcement_options(
        serde_json::json!({
            "revision": 1,
            "sources": [declared_inbox(&mob_id, Some(runner))],
        }),
        None,
    );
    let runtime = build(
        builder(&provider, cache.path(), &mob_id, &[ALICE], &model).mob_announcements(options),
    )
    .await;
    let service = announcements_of(&runtime);
    applied_declarations(&service, &mob_id, 1).await;
    let source = announcement_call(
        &service,
        &mob_id,
        "meta/mob_announcements/sources/get",
        serde_json::json!({ "source_id": "inbox" }),
    )
    .await["source"]
        .clone();
    assert_eq!(source["runner"]["every_seconds"], 300, "{source}");
    assert_eq!(
        source["health"]["runner_scheduled_here"],
        serde_json::json!(false),
        "{source}"
    );
    let schedules = fixture
        .schedule
        .list_schedules(meerkat::ScheduleFilter {
            include_deleted: true,
            ..meerkat::ScheduleFilter::default()
        })
        .await
        .expect("the host's schedules list");
    assert!(schedules.is_empty(), "no schedule row: {schedules:?}");

    // The host's scheduler fires the collector. The runtime's own rounds may
    // write batches around the step, so the step's batch is found by what it
    // carries: its receipt, and the source row whose cursor it moved.
    let step: crate::mob_announcements::sources::AdvanceSource =
        serde_json::from_value(serde_json::json!({
            "mob_id": mob_id,
            "source_id": "inbox",
            "expected_revision": 1,
            "expected_version": 0,
            "principal": inbox_principal(),
            "next_cursor": "cursor-after-host-scheduler-step-1",
            "coverage": "complete",
            "items": [],
            "streams": [],
        }))
        .expect("a collector step");
    let advanced = service
        .advance_source(
            &crate::mob_announcements::OperationId::new("host-scheduler-step-1")
                .expect("an operation id"),
            step,
        )
        .await
        .expect("the step lands");
    assert_eq!(advanced.source.cursor.version, 1);
    let receipt = host
        .first_announcement_batch_carrying("host-scheduler-step-1")
        .expect("the step's receipt landed");
    assert_eq!(
        host.first_announcement_batch_carrying("cursor-after-host-scheduler-step-1"),
        Some(receipt),
        "the moved cursor lands with the receipt"
    );
    assert!(
        host.announcement_batch_carries(receipt, "\"identity_key\":"),
        "the source row lands in the receipt's batch: one step, one batch"
    );
    stop(runtime).await;
}

/// A poisoned announcement store makes the realm not ready: a batch of the
/// store the host refuses poisons its scope, the readiness probe says so,
/// and the store refuses every later call until the realm is opened again.
#[tokio::test(flavor = "multi_thread")]
async fn a_poisoned_announcement_store_makes_the_realm_not_ready() {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("announcements-poisoned");
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    let host = fixture.process();
    let provider = Arc::new(fixture.open(&host).await.expect("open"));
    let runtime = build(
        builder(&provider, cache.path(), &mob_id, &[ALICE], &model)
            .mob_announcements(crate::mob_announcements::AnnouncementConfig::default()),
    )
    .await;
    let service = announcements_of(&runtime);
    assert!(provider.readiness().is_ready());

    host.refuse_next_announcement_batches(1);
    try_announcement_call(
        &service,
        &mob_id,
        "meta/mob_announcements/announce",
        serde_json::json!({ "source_key": "k-1", "topics": ["release"], "body": "refused" }),
    )
    .await
    .expect_err("the refused batch answers an error");
    let readiness = provider.readiness();
    assert!(
        readiness.scope_poisoned && !readiness.is_ready(),
        "{readiness:?}"
    );
    try_announcement_call(
        &service,
        &mob_id,
        "meta/mob_announcements/subscriptions/list",
        serde_json::json!({}),
    )
    .await
    .expect_err("a poisoned store refuses every later call");
    stop(runtime).await;
}

/// A realm that loses its fence while a run is in flight (its next renew
/// answers fenced out) is poisoned, and the readiness probe reports the
/// cause. The run's held model call settles and asks for a tool, and the run
/// stops: the model sees exactly that one call of it, and nothing lands in
/// the realm after the loss. A new input after the loss reaches nothing
/// either.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_fence_poisons_the_realm_and_nothing_runs_after_it() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(5))
        .expect("a fence timing");
    let fixture = Fixture::with_timing(timing);
    let mob_id = unique_mob_id("lost-fence");
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    let host = fixture.process();
    let provider = Arc::new(fixture.open(&host).await.expect("open"));
    let runtime = build(builder(&provider, cache.path(), &mob_id, &[ALICE], &model)).await;
    send(
        &runtime,
        &provider,
        &model,
        ALICE,
        "a turn before the fence is lost",
    )
    .await;
    assert!(provider.readiness().is_ready());

    // A run in flight: its model call waits, and then calls a tool, so the
    // run would call the model again.
    let before = model.count();
    runtime
        .identity_runtime()
        .expect("an identity-first runtime")
        .send(
            &id(ALICE),
            &ContentInput::Text(format!("{HELD_TURN} {TOOL_TURN}: look it up")),
        )
        .await
        .expect("the turn is admitted");
    model
        .wait_for(before + 1, "the run's held model call")
        .await;
    let settled = model.settled();

    // Another process takes the realm over.
    fixture
        .store
        .acquire_fence(AcquireMode::Takeover)
        .await
        .expect("another process takes over");
    let cause = eventually("the realm to lose its fence", || {
        provider.readiness().realm_poisoned
    })
    .await;
    assert!(cause.contains("fenced out"), "{cause}");
    assert!(!provider.readiness().is_ready());

    // The held call settles; then a new input arrives.
    let landed = host.landed();
    let heads = fixture.heads().await;
    model.release();
    eventually("the held model call to settle", || {
        (model.settled() > settled).then_some(())
    })
    .await;
    let sent = tokio::time::timeout(
        Duration::from_secs(30),
        runtime
            .identity_runtime()
            .expect("an identity-first runtime")
            .send(
                &id(ALICE),
                &ContentInput::Text("a turn after the fence is lost".to_string()),
            ),
    )
    .await;
    // Nothing may follow either: watch for a while.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        model.requests_carrying(TOOL_TURN),
        1,
        "the run in flight made no model call after its fence was lost"
    );
    assert_eq!(
        model.count(),
        before + 1,
        "nothing reaches the model after the fence was lost (the new send answered {sent:?})"
    );
    assert_eq!(host.landed(), landed, "nothing the process wrote landed");
    assert_eq!(fixture.heads().await, heads, "the realm is unchanged");
    let _ = tokio::time::timeout(Duration::from_mins(2), runtime.shutdown()).await;
}

/// A graceful shutdown keeps a run that is waiting on its model call, and
/// the next start continues it: its model call is made again with no new
/// input.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_in_flight_at_shutdown_continues_at_the_next_start() {
    let fixture = Fixture::new();
    let mob_id = unique_mob_id("requeue");
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let runtime = build(builder(&provider, cache.path(), &mob_id, &[ALICE], &model)).await;
        let before = model.count();
        runtime
            .identity_runtime()
            .expect("an identity-first runtime")
            .send(
                &id(ALICE),
                &ContentInput::Text(format!("{HELD_TURN}: summarize the plan")),
            )
            .await
            .expect("the turn is admitted");
        model
            .wait_for(before + 1, "the held turn's model call")
            .await;
        stop(runtime).await;
    }
    assert_eq!(model.requests_carrying(HELD_TURN), 1);

    let provider = reopen(&fixture).await;
    // From now on the model answers. The stopped process could not land a
    // run any more: the new process holds the fence.
    model.release();
    let runtime = build(builder(&provider, cache.path(), &mob_id, &[ALICE], &model)).await;
    eventually("the kept run's model call after the restart", || {
        (model.requests_carrying(HELD_TURN) >= 2).then_some(())
    })
    .await;
    stop(runtime).await;
}

// ---------------------------------------------------------------------------
// Imports of a continuity export
// ---------------------------------------------------------------------------

/// The first-boot rows an import leaves are carried out at the first roster
/// compile: an identity the roster lists resumes its imported session with
/// its transcript, and one it does not list waits. The next start, on an
/// empty cache, carries out the rest.
#[tokio::test(flavor = "multi_thread")]
async fn imported_first_boot_rows_are_carried_out_at_the_first_roster_compile() {
    let scratch = tempfile::tempdir().expect("scratch");
    let mob_id = unique_mob_id("imported-rows");
    let (export, sessions) =
        export_of_a_running_realm(scratch.path(), &mob_id, &[ALICE, BOB]).await;

    let fixture = Fixture::new();
    let report = import(&fixture.import_request(&export, MigrateMode::Apply)).await;
    assert!(report.is_clean() && report.completed, "{report:#?}");
    assert_eq!(report.first_boot_rows.len(), 2, "{report:#?}");
    fixture.lapse();

    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let runtime = build(builder(&provider, cache.path(), &mob_id, &[ALICE], &model)).await;
        assert_eq!(session_of(&runtime, ALICE).await, sessions[ALICE]);
        let requests = send(
            &runtime,
            &provider,
            &model,
            ALICE,
            "Which token did I give you?",
        )
        .await;
        assert!(
            requests
                .iter()
                .any(|request| request.contains(&token(ALICE))),
            "alice's turn replays her imported transcript"
        );
        let waiting = FirstBootManifest::read_host_store(provider.realm.documents())
            .await
            .expect("the first-boot rows read")
            .expect("bob's row waits for the roster");
        let identities: Vec<&str> = waiting
            .rows()
            .iter()
            .map(|row| row.identity().as_str())
            .collect();
        assert_eq!(identities, vec![BOB]);
        stop(runtime).await;
    }

    drop(cache);
    let provider = reopen(&fixture).await;
    let empty_cache = tempfile::tempdir().expect("an empty cache");
    let runtime = build(builder(
        &provider,
        empty_cache.path(),
        &mob_id,
        &[ALICE, BOB],
        &model,
    ))
    .await;
    assert_eq!(session_of(&runtime, BOB).await, sessions[BOB]);
    assert_eq!(session_of(&runtime, ALICE).await, sessions[ALICE]);
    assert!(
        FirstBootManifest::read_host_store(provider.realm.documents())
            .await
            .expect("the first-boot rows read")
            .is_none(),
        "every row is carried out"
    );
    stop(runtime).await;
}

/// The first start after an import, with an empty comms identity directory:
/// each imported member gets a new key pair there and is healthy on its
/// imported session, and peer messages flow between them both ways.
#[tokio::test(flavor = "multi_thread")]
async fn the_first_start_after_an_import_needs_no_comms_identity() {
    let scratch = tempfile::tempdir().expect("scratch");
    let mob_id = unique_mob_id("cutover");
    let (export, sessions) =
        export_of_a_running_realm(scratch.path(), &mob_id, &[ALICE, BOB]).await;
    let fixture = Fixture::new();
    let report = import(&fixture.import_request(&export, MigrateMode::Apply)).await;
    assert!(report.is_clean() && report.completed, "{report:#?}");
    fixture.lapse();

    let volume = tempfile::tempdir().expect("an empty comms identity volume");
    let keys = volume.path().join(".rkat").join("session_comms_identity");
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
    let runtime = build(
        builder(&provider, cache.path(), &mob_id, &[ALICE, BOB], &model)
            .topology_provider(Arc::new(CompleteGraph))
            .session_identity_config_root(volume.path()),
    )
    .await;
    let slot = comms_identity_slot(&runtime);
    assert!(!slot.degraded, "{slot:?}");
    let members = runtime.mob_handle().list_members().await;
    assert_eq!(members.len(), 2, "{members:?}");
    for member in &members {
        assert_eq!(
            member.status,
            meerkat_mob::MobMemberStatus::Active,
            "{} is healthy: {:?}",
            member.agent_identity,
            member.error
        );
    }
    for (name, session) in &sessions {
        assert_eq!(
            &session_of(&runtime, name).await,
            session,
            "{name} resumes its imported session"
        );
        assert!(
            keys.join(session.to_string())
                .join("identity.key")
                .is_file(),
            "{name} has a new key pair in the directory"
        );
    }

    // Peer messages flow between the imported members, both ways.
    let alice = crate::member_comms_id::roster_member_id_for_identity(ALICE);
    let bob = crate::member_comms_id::roster_member_id_for_identity(BOB);
    let desired = desired_pairs(&[ALICE, BOB]);
    eventually_holds("alice and bob to be wired", || async {
        wired_pairs(&runtime).await == desired
    })
    .await;
    for (from, to, note) in [
        (&alice, &bob, "PEER-NOTE-FROM-ALICE"),
        (&bob, &alice, "PEER-NOTE-FROM-BOB"),
    ] {
        let _receipt = runtime
            .mob_handle()
            .send_peer_message(
                from.clone(),
                to.clone(),
                note,
                meerkat_core::types::HandlingMode::Queue,
            )
            .await
            .unwrap_or_else(|error| panic!("{from} messages {to}: {error}"));
        eventually(&format!("{to}'s turn on the message from {from}"), || {
            (model.requests_carrying(note) > 0).then_some(())
        })
        .await;
    }
    stop(runtime).await;
}

/// A dry run of an import into a realm a runtime has used is refused, and
/// changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_import_into_a_realm_a_runtime_used_is_refused() {
    let scratch = tempfile::tempdir().expect("scratch");
    let export = released_export(scratch.path(), "memory");
    let fixture = Fixture::new();
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let runtime = build(builder(
            &provider,
            cache.path(),
            &unique_mob_id("used-realm"),
            &[ALICE],
            &model,
        ))
        .await;
        stop(runtime).await;
    }

    fixture.lapse();
    let before = fixture.heads().await;
    let report = import(&fixture.import_request(&export, MigrateMode::DryRun)).await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [HostStoreImportRefusal::RealmInUse { .. }]
        ),
        "{report:#?}"
    );
    assert!(!report.completed);
    assert_eq!(fixture.heads().await, before, "the dry run wrote nothing");
}

/// An import stopped by a store outage after two sessions refuses every
/// runtime, typed, and refuses another export; run again with its own
/// export, it keeps the sessions that landed and creates the rest.
#[tokio::test]
async fn an_interrupted_import_refuses_runtimes_and_continues_where_it_stopped() {
    let scratch = tempfile::tempdir().expect("scratch");
    let export = released_export(scratch.path(), "memory");
    let other_export = released_export(scratch.path(), "durable");
    let fixture = Fixture::new();

    let host = fixture.process();
    host.outage_after_sessions(2);
    let stopped = import(&fixture.import_request_over(&host, &export, MigrateMode::Apply)).await;
    let digest = stopped
        .export
        .as_ref()
        .expect("the export read")
        .entries_sha256
        .clone();
    assert!(
        matches!(
            stopped.refusals.as_slice(),
            [HostStoreImportRefusal::Store { .. }]
        ),
        "{stopped:#?}"
    );
    assert!(!stopped.completed);
    let created = stopped
        .sessions
        .iter()
        .filter(|row| row.class == HostStoreImportClass::Created)
        .count();
    assert_eq!(created, 2, "{stopped:#?}");

    // A runtime refuses the half-imported realm.
    fixture.lapse();
    match fixture.open(&fixture.process()).await {
        Err(HostStoreOpenError::ImportIncomplete { entries_sha256 }) => {
            assert_eq!(entries_sha256, digest);
        }
        other => panic!("a half-imported realm refuses typed, not {other:?}"),
    }

    // Another export is refused: the realm records this one.
    fixture.lapse();
    let refused = import(&fixture.import_request(&other_export, MigrateMode::Apply)).await;
    assert!(
        matches!(
            refused.refusals.as_slice(),
            [HostStoreImportRefusal::OtherExport { recorded, .. }] if *recorded == digest
        ),
        "{refused:#?}"
    );

    // The same export continues: the landed sessions are kept.
    fixture.lapse();
    let rerun = import(&fixture.import_request(&export, MigrateMode::Apply)).await;
    assert!(rerun.is_clean() && rerun.completed, "{rerun:#?}");
    let classes: Vec<HostStoreImportClass> = rerun.sessions.iter().map(|row| row.class).collect();
    assert_eq!(
        classes,
        vec![
            HostStoreImportClass::Kept,
            HostStoreImportClass::Kept,
            HostStoreImportClass::Created,
            HostStoreImportClass::Created,
        ]
    );
    fixture.lapse();
    fixture
        .open(&fixture.process())
        .await
        .expect("the imported realm opens");
}

/// An import over a session the realm already holds with another document
/// refuses that row, and writes nothing.
#[tokio::test]
async fn an_import_over_another_document_of_a_session_writes_nothing() {
    let scratch = tempfile::tempdir().expect("scratch");
    let export = released_export(scratch.path(), "memory");
    let (session_id, document) = first_ready_entry(&export);
    let fixture = Fixture::new();
    {
        let provider = fixture.open(&fixture.process()).await.expect("open");
        let mut session =
            meerkat_core::Session::from_persisted_bytes(&document).expect("the document decodes");
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("a turn the export does not carry"),
        ));
        let other = session
            .to_persisted_bytes()
            .expect("encode the other document");
        provider
            .realm
            .runtime_store()
            .import_whole_blob_session(
                &LogicalRuntimeId::for_session(&session_id),
                meerkat_runtime::store::SerializedSessionSnapshot {
                    session_snapshot: Arc::new(other),
                },
            )
            .await
            .expect("the realm holds another document for the session");
    }

    fixture.lapse();
    let before = fixture.heads().await;
    let session_key = session_id.to_string();
    for mode in [MigrateMode::DryRun, MigrateMode::Apply] {
        let report = import(&fixture.import_request(&export, mode)).await;
        assert!(
            matches!(
                report.refusals.as_slice(),
                [HostStoreImportRefusal::RowsRefused { count: 1 }]
            ),
            "{report:#?}"
        );
        let row = report
            .sessions
            .iter()
            .find(|row| row.session_id == session_key)
            .expect("the session's row");
        assert_eq!(row.class, HostStoreImportClass::DifferentRow, "{report:#?}");
        assert!(!report.completed);
        assert_eq!(fixture.heads().await, before, "nothing was written");
        fixture.lapse();
    }
}

/// An import of a document that carries a pending compaction projection
/// intent records the intent in the runtime store's outbox with the
/// document.
#[tokio::test]
async fn an_import_outboxes_the_pending_compaction_intents_of_a_document() {
    let (session, intent) = session_with_compaction_intent();
    let scratch = tempfile::tempdir().expect("scratch");
    let export = scratch.path().join("compacted-export.jsonl");
    write_export(
        &export,
        &[(
            "personal:compacted".to_string(),
            session.id().clone(),
            session.to_persisted_bytes().expect("encode the session"),
        )],
    );
    let fixture = Fixture::new();
    let report = import(&fixture.import_request(&export, MigrateMode::Apply)).await;
    assert!(report.is_clean() && report.completed, "{report:#?}");
    let [row] = report.sessions.as_slice() else {
        panic!("one row: {report:#?}");
    };
    assert_eq!(row.class, HostStoreImportClass::Created);
    assert_eq!(row.pending_compaction_projections, 1);

    fixture.lapse();
    let provider = fixture.open(&fixture.process()).await.expect("open");
    let pending = provider
        .realm
        .runtime_store()
        .load_pending_compaction_projections(&LogicalRuntimeId::for_session(session.id()))
        .await
        .expect("the pending intents read");
    assert_eq!(pending, vec![intent]);
}

/// A provider's topology of a few hundred edges is reconciled at the first
/// start after an import, and again at a restart onto an empty cache: every
/// edge is wired once, none is missing, and the restart wires nothing again.
#[tokio::test(flavor = "multi_thread")]
async fn a_provider_topology_is_reconciled_after_an_import_and_after_a_restart_onto_an_empty_cache()
{
    let scratch = tempfile::tempdir().expect("scratch");
    let mob_id = unique_mob_id("topology");
    let imported = [ALICE, BOB, CAROL];
    let (export, sessions) = export_of_a_running_realm(scratch.path(), &mob_id, &imported).await;
    let fixture = Fixture::new();
    let report = import(&fixture.import_request(&export, MigrateMode::Apply)).await;
    assert!(report.is_clean() && report.completed, "{report:#?}");
    fixture.lapse();

    // Twenty-four identities, three of them imported: 276 edges.
    let mut roster: Vec<String> = imported.iter().map(|name| (*name).to_string()).collect();
    roster.extend((1..=21).map(|index| format!("personal:member-{index:02}")));
    let names: Vec<&str> = roster.iter().map(String::as_str).collect();
    let desired = desired_pairs(&names);
    assert_eq!(desired.len(), 276);

    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    {
        let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
        let runtime = build(
            builder(&provider, cache.path(), &mob_id, &names, &model)
                .topology_provider(Arc::new(CompleteGraph)),
        )
        .await;
        for (name, session) in &sessions {
            assert_eq!(&session_of(&runtime, name).await, session, "{name}");
        }
        eventually_holds("every edge to be wired", || async {
            wired_pairs(&runtime).await == desired
        })
        .await;
        assert_eq!(wiring_events(&runtime).await, (276, 0), "each edge once");
        stop(runtime).await;
    }

    drop(cache);
    let provider = reopen(&fixture).await;
    let empty_cache = tempfile::tempdir().expect("an empty cache");
    let runtime = build(
        builder(&provider, empty_cache.path(), &mob_id, &names, &model)
            .topology_provider(Arc::new(CompleteGraph)),
    )
    .await;
    assert_eq!(
        wired_pairs(&runtime).await,
        desired,
        "the restart keeps every edge and adds none"
    );
    assert_eq!(
        wiring_events(&runtime).await,
        (276, 0),
        "the restart wires no edge again and unwires none"
    );
    stop(runtime).await;
}

// ---------------------------------------------------------------------------
// The event log
// ---------------------------------------------------------------------------

/// Over a realm whose host supplies no event log store, the builder's
/// `event_log()` ingests the runtime's events into its own store, with its
/// own settings (here its filter).
#[tokio::test(flavor = "multi_thread")]
async fn the_builder_event_log_ingests_over_a_realm_that_supplies_none() {
    let fixture = Fixture::new();
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    let log = RecordedLog::default();
    let filtered = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
    let filter_calls = Arc::clone(&filtered);
    let runtime = build(
        builder(
            &provider,
            cache.path(),
            &unique_mob_id("builder-event-log"),
            &[ALICE],
            &model,
        )
        .event_log(EventLogConfig {
            store: Box::new(log.clone()),
            filter: Some(Box::new(move |_event: &crate::UnifiedEvent| {
                filter_calls.fetch_add(1, Ordering::SeqCst);
                true
            })),
            batch_size: 1,
            ..EventLogConfig::default()
        }),
    )
    .await;
    send(
        &runtime,
        &provider,
        &model,
        ALICE,
        "a turn the event log records",
    )
    .await;
    eventually("the builder's event log to ingest the turn", || {
        (log.count() > 0).then_some(())
    })
    .await;
    assert!(
        filtered.load(Ordering::SeqCst) > 0,
        "the builder's filter decides what is ingested"
    );
    stop(runtime).await;
}

/// An event log store the host supplies with the realm, and no `event_log()`
/// on the builder: the runtime ingests into it, with the default settings.
#[tokio::test(flavor = "multi_thread")]
async fn the_realm_event_log_store_ingests_with_the_default_settings() {
    let fixture = Fixture::new();
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    let log = RecordedLog::default();
    let provider = Arc::new(
        fixture
            .open_with_event_log(&fixture.process(), log.clone())
            .await
            .expect("open"),
    );
    let runtime = build(builder(
        &provider,
        cache.path(),
        &unique_mob_id("realm-event-log"),
        &[ALICE],
        &model,
    ))
    .await;
    send(
        &runtime,
        &provider,
        &model,
        ALICE,
        "a turn the event log records",
    )
    .await;
    eventually("the realm's event log store to ingest the turn", || {
        (log.count() > 0).then_some(())
    })
    .await;
    stop(runtime).await;
}

/// The builder's `event_log()` over a realm whose host supplies an event log
/// store too is refused, typed and naming the event log, before the realm's
/// store set is taken: the same provider then serves a build without it.
#[tokio::test(flavor = "multi_thread")]
async fn two_event_logs_are_refused_before_the_realm_serves_the_build() {
    let fixture = Fixture::new();
    let model = Model::default();
    let cache = tempfile::tempdir().expect("cache");
    let mob_id = unique_mob_id("two-event-logs");
    let log = RecordedLog::default();
    let provider = Arc::new(
        fixture
            .open_with_event_log(&fixture.process(), log.clone())
            .await
            .expect("open"),
    );
    let refused = refused_configuration(
        builder(&provider, cache.path(), &mob_id, &[ALICE], &model).event_log(EventLogConfig {
            store: Box::new(RecordedLog::default()),
            ..EventLogConfig::default()
        }),
    )
    .await;
    assert!(
        refused.contains("event_log()") && refused.contains("event_log_store"),
        "{refused}"
    );

    let runtime = build(builder(&provider, cache.path(), &mob_id, &[ALICE], &model)).await;
    send(
        &runtime,
        &provider,
        &model,
        ALICE,
        "a turn the event log records",
    )
    .await;
    eventually("the realm's event log store to ingest the turn", || {
        (log.count() > 0).then_some(())
    })
    .await;
    stop(runtime).await;
}

// ---------------------------------------------------------------------------
// Agent-created child mobs
// ---------------------------------------------------------------------------

/// The persistent bootstrap spec a build composes over the realm `provider`
/// opened, on the per-host cache `cache`.
async fn host_store_spec(
    fixture: &Fixture,
    provider: &HostStoreMetaStorageProvider,
    cache: &Path,
    definition: meerkat_mob::MobDefinition,
    model: Arc<dyn meerkat_client::LlmClient>,
) -> MobBootstrapSpec {
    let realm = &provider.realm;
    let realm_id = realm
        .meerkat_level_realm(None)
        .await
        .expect("the realm's meerkat-level realm");
    realm
        .ensure_realm_record(&realm_id)
        .await
        .expect("the realm record");
    let (storage, provenance) = crate::mob_composition_manifest::host_store_mob_storage(realm)
        .await
        .expect("the realm's root mob storage");
    let composed = MobBootstrapSpec::persistent_inner_with_provider_stores(
        definition,
        storage,
        provenance,
        cache.to_path_buf(),
        8,
        Some(crate::blob_store::BlobStoreInjection::Binary(Arc::clone(
            &fixture.blobs,
        ))),
        true,
        Some(Arc::clone(&fixture.schedule)),
        Some(Arc::clone(&fixture.workgraph)),
        None,
        crate::mob_handle_runtime::CapabilityFlags::default(),
        None,
        None,
        Some(realm.provider_meerkat_stores(realm_id.clone())),
        realm_id,
        None,
        None,
        None,
        None,
        crate::mob_handle_runtime::PersistentAuthority::HostStore { cache_dir: None },
        None,
        None,
    )
    .await;
    match composed {
        Ok(spec) => spec.with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(model),
        }),
        Err(error) => panic!("the host store composition: {error}"),
    }
}

fn delegating_definition(mob_id: &str) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n\
         [profiles.worker.tools]\ncomms = true\n\n[profiles.delegator]\n\
         model = \"gpt-5.5\"\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.delegator.tools]\ncomms = true\nmob = true\n"
    ))
    .expect("the delegating mob definition parses")
}

/// A durable child mob storage selected over a host store realm is refused,
/// typed, and nothing local is claimed for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_durable_child_mob_storage_is_refused_on_a_host_store_realm() {
    let fixture = Fixture::new();
    let provider = fixture.open(&fixture.process()).await.expect("open");
    let cache = tempfile::tempdir().expect("cache");
    let spec = host_store_spec(
        &fixture,
        &provider,
        cache.path(),
        delegating_definition(&unique_mob_id("durable-children")),
        Arc::new(Model::default()),
    )
    .await
    .with_child_mob_storage(crate::ChildMobStorage::Durable(
        crate::storage_layout::MetaStorageLayout::with_injected_roots(
            cache.path().to_path_buf(),
            None,
        ),
    ));
    match MobRuntime::bootstrap(spec).await {
        Err(MobRuntimeError::ChildMobStorage(crate::ChildMobStorageError::HostStoreRealm)) => {}
        Err(other) => panic!("a durable child mob storage is refused typed, not: {other}"),
        Ok(runtime) => {
            let _ = runtime.handle().shutdown().await;
            panic!("a durable child mob storage must be refused on a host store realm");
        }
    }
    assert!(
        !crate::child_mob_storage::child_mob_root(cache.path()).exists(),
        "no child mob root was claimed in the cache"
    );
}

const DELEGATE_PROBE: &str = "DELEGATE-ON-A-HOST-STORE-REALM";

/// The delegator's probe turn calls `delegate` and records the result it
/// sees; every other turn, the helper's included, answers.
#[derive(Clone, Default)]
struct DelegatingModel {
    delegated: Arc<StdMutex<Option<String>>>,
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for DelegatingModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        // The probe reaches the member wrapped as its runtime delivers it.
        let probe_turn = serde_json::to_string(&request.messages)
            .is_ok_and(|messages| messages.contains(DELEGATE_PROBE));
        let delegate_result = request
            .messages
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::ToolResults { results, .. } => Some(results),
                _ => None,
            })
            .flatten()
            .find(|result| result.tool_use_id == "call-delegate")
            .map(meerkat_core::ToolResult::text_content);
        let call_delegate = probe_turn && delegate_result.is_none();
        if let Some(result) = delegate_result.filter(|_| probe_turn) {
            *lock(&self.delegated) = Some(result);
        }
        let stop = if call_delegate {
            meerkat_core::StopReason::ToolUse
        } else {
            meerkat_core::StopReason::EndTurn
        };
        let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
            request,
            meerkat_core::Provider::OpenAI,
            stop,
        );
        let lead = if call_delegate {
            meerkat_client::LlmEvent::ToolCallComplete {
                id: "call-delegate".to_string(),
                name: "delegate".to_string(),
                args: serde_json::json!({
                    "task": "summarize the household calendar",
                    "member_id": "helper",
                    "result_label": "helper_result",
                    "max_text_bytes": 4096,
                    "tooling": {
                        "mode": "profile",
                        "source": {
                            "type": "inline",
                            "model": "gpt-5.5",
                            "tools": { "comms": true }
                        }
                    }
                }),
                meta: None,
            }
        } else {
            meerkat_client::LlmEvent::TextDelta {
                delta: "done".to_string(),
                meta: None,
            }
        };
        Box::pin(futures::stream::iter(vec![Ok(lead), Ok(usage), Ok(done)]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// A member of a mob kept in a host store realm delegates from its own
/// turn: the helper's implicit child mob lives with the process, and the
/// delegate completes.
#[tokio::test(flavor = "multi_thread")]
async fn delegate_runs_on_a_host_store_realm() {
    let fixture = Fixture::new();
    let provider = fixture.open(&fixture.process()).await.expect("open");
    let cache = tempfile::tempdir().expect("cache");
    let model = DelegatingModel::default();
    let spec = host_store_spec(
        &fixture,
        &provider,
        cache.path(),
        delegating_definition(&unique_mob_id("delegate")),
        Arc::new(model.clone()),
    )
    .await;
    let runtime = match MobRuntime::bootstrap(spec).await {
        Ok(runtime) => runtime,
        Err(error) => panic!("the bootstrap over the host store realm: {error}"),
    };
    let handle = runtime.handle();
    let delegator = meerkat_mob::AgentIdentity::from("delegator");
    handle
        .ensure_member(meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("delegator"),
            delegator.clone(),
        ))
        .await
        .expect("seat the delegating member");
    let bounded = meerkat_mob::BoundedResultSpec::new("turn", 4096).expect("a result bound");
    let work = handle
        .start_work_for_identity_bounded(
            delegator,
            meerkat_mob::WorkSpec::new(
                ContentInput::Text(DELEGATE_PROBE.to_string()),
                meerkat_mob::WorkOrigin::Internal,
            ),
            meerkat_core::types::HandlingMode::Queue,
            bounded.clone(),
        )
        .await
        .expect("start the delegating turn");
    tokio::time::timeout(Duration::from_mins(1), work.wait_bounded(bounded))
        .await
        .expect("the delegating turn completes within the failure bound")
        .expect("the delegating turn succeeds");
    let delegated = lock(&model.delegated)
        .clone()
        .expect("the delegator saw its delegate result");
    let result: serde_json::Value =
        serde_json::from_str(&delegated).expect("a JSON delegate result");
    assert_eq!(result["agent_identity"], "helper", "{result}");
    assert_eq!(result["bounded_result"]["status"], "completed", "{result}");

    // The implicit child mob is process-bound: the realm holds the root mob's
    // storage only.
    let stored: Vec<String> = provider
        .realm
        .mob_host()
        .stored_mobs()
        .into_iter()
        .map(|mob| mob.storage_key)
        .collect();
    assert_eq!(stored, vec![ROOT_MOB_STORAGE_KEY.to_string()]);
    let _ = handle.shutdown().await;
}

/// The definition of a mob whose roster members may delegate.
fn delegating_roster_definition(mob_id: &str) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.personal]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.personal.tools]\ncomms = true\nmob = true\n"
    ))
    .expect("the delegating roster definition parses")
}

/// A roster member of a mob a builder keeps in a host store realm delegates
/// from its own turn, as an identity-first host's members do: the helper's
/// implicit child mob lives with the process, and the delegate completes.
#[tokio::test(flavor = "multi_thread")]
async fn a_builder_roster_member_delegates_on_a_host_store_realm() {
    let fixture = Fixture::new();
    let provider = Arc::new(fixture.open(&fixture.process()).await.expect("open"));
    let cache = tempfile::tempdir().expect("cache");
    let model = DelegatingModel::default();
    let mob_id = unique_mob_id("roster-delegate");
    let runtime = build(
        UnifiedRuntimeBuilder::default()
            .definition(delegating_roster_definition(&mob_id))
            .storage_provider(provider.clone())
            .scratch_dir(cache.path())
            .roster_provider(Arc::new(Roster::of(&[ALICE])))
            .identity_runtime_instance_id(&mob_id)
            .default_llm_client(Arc::new(model.clone())),
    )
    .await;
    runtime
        .identity_runtime()
        .expect("an identity-first runtime")
        .send(&id(ALICE), &ContentInput::Text(DELEGATE_PROBE.to_string()))
        .await
        .expect("alice takes the delegating turn");
    let delegated = eventually("alice to see her delegate result", || {
        lock(&model.delegated).clone()
    })
    .await;
    let result: serde_json::Value =
        serde_json::from_str(&delegated).expect("a JSON delegate result");
    assert_eq!(result["agent_identity"], "helper", "{result}");
    assert_eq!(result["bounded_result"]["status"], "completed", "{result}");

    // The implicit child mob is process-bound: the realm holds the root mob's
    // storage only.
    let stored: Vec<String> = provider
        .realm
        .mob_host()
        .stored_mobs()
        .into_iter()
        .map(|mob| mob.storage_key)
        .collect();
    assert_eq!(stored, vec![ROOT_MOB_STORAGE_KEY.to_string()]);
    stop(runtime).await;
}
