//! M4 de-weld acceptance: the judgment plane (taint firewall controls,
//! Steward assembly, console Memory panel) runs against ANY provider that
//! advertises the capability traits — proven here with a minimal in-memory
//! fake that is not the bundled SQLite store — and a provider that
//! advertises none of them (the markdown store) stays recall-only by its
//! flags, with engine requests refusing loudly instead of silently not
//! existing.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use meerkat_meta::memory::records::{
    InjectionLogEntry, MemoryAuthor, MemoryKind, MemoryProvenance, MemoryRecord, MemoryScope,
    RecordStatus, TrustTier, UsageStats,
};
use meerkat_meta::memory::steward::StewardConfig;
use meerkat_meta::memory::{
    CommitReceipt, DreamAuditVerdict, DreamRunAudit, EvidenceRefResolver, LlmWriteGate,
    MemoryEventSink, MemoryPanelStore, PanelRecordsPage, PendingHarvest, PendingPromotion,
    PendingProposal, PersistedDreamRun, QuarantineReviewError, QuarantineReviewOutcome,
    QuarantineReviewRequest, ScopeOverview, SelectedRecordFetch, StageToken, StagedMemoryStore,
    StagedMutationBatch, StewardStore, TaintableStore, TombstoneMeta, TombstoneSource,
};
use meerkat_meta::memory_wiring::{MemoryEnginesConfig, MemoryStackSeams, attach_memory_engines};
use meerkat_meta::{
    AgentMemoryConfig, AgentMemoryError, AgentMemoryProvider, AgentMemoryRecallRequest,
    AgentMemoryRecord, UnifiedRuntime,
};
use meerkat_mob::MobDefinition;
use serde_json::{Value, json};
use tower::ServiceExt;

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const REALM: &str = "fake-realm";

/// Per-call mob id: 0.8.23's fail-closed in-proc registration means
/// concurrently running tests must not share a supervisor route.
fn deweld_mob_toml() -> String {
    format!(
        r#"
[mob]
id = "deweld-memory-mob-{}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn test_definition() -> MobDefinition {
    MobDefinition::from_toml(&deweld_mob_toml()).expect("parse test mob definition")
}

// ---------------------------------------------------------------------------
// A minimal non-SQLite judgment-capable provider.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeState {
    records: Mutex<Vec<MemoryRecord>>,
    gate: Mutex<Option<Arc<dyn LlmWriteGate>>>,
    gate_installs: AtomicUsize,
    sink: Mutex<Option<Arc<dyn MemoryEventSink>>>,
    resolver: Mutex<Option<Arc<dyn EvidenceRefResolver>>>,
}

/// Capability switches so tests can shape partially-capable providers.
#[derive(Clone, Copy)]
struct FakeCaps {
    steward: bool,
    panel: bool,
    fetch: bool,
    tombstones: bool,
}

impl Default for FakeCaps {
    fn default() -> Self {
        Self {
            steward: true,
            panel: true,
            fetch: true,
            tombstones: true,
        }
    }
}

#[derive(Clone, Default)]
struct FakeJudgmentStore {
    state: Arc<FakeState>,
    caps: FakeCaps,
}

impl FakeJudgmentStore {
    fn seeded() -> Self {
        let fake = Self::default();
        fake.state.records.lock().unwrap().push(MemoryRecord {
            id: "mem-fake-1".to_string(),
            scope: MemoryScope::Identity {
                realm: REALM.to_string(),
                identity: "identity:helper".to_string(),
            },
            kind: MemoryKind::Fact,
            title: "Fake panel fact".to_string(),
            description: "seeded through the fake provider".to_string(),
            body: "visible through the de-welded panel".to_string(),
            tags: vec![],
            provenance: MemoryProvenance {
                evidence: vec![],
                author: MemoryAuthor::Operator,
                profile: None,
                verification: None,
            },
            trust: TrustTier::Operator,
            status: RecordStatus::Active,
            supersedes: None,
            derived_from: vec![],
            working_set_rank: None,
            created_at_ms: 1,
            updated_at_ms: 1,
            usage: UsageStats::default(),
            ever_quarantined: false,
        });
        fake
    }

    fn gate_installed(&self) -> bool {
        self.state.gate.lock().unwrap().is_some()
    }

    fn gate_installs(&self) -> usize {
        self.state.gate_installs.load(Ordering::SeqCst)
    }

    fn sink_installed(&self) -> bool {
        self.state.sink.lock().unwrap().is_some()
    }

    fn resolver_installed(&self) -> bool {
        self.state.resolver.lock().unwrap().is_some()
    }
}

#[async_trait]
impl AgentMemoryProvider for FakeJudgmentStore {
    async fn recall(
        &self,
        _request: AgentMemoryRecallRequest,
    ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
        Ok(Vec::new())
    }

    fn as_taintable(&self) -> Option<Arc<dyn TaintableStore>> {
        Some(Arc::new(self.clone()))
    }

    fn as_steward_store(&self) -> Option<Arc<dyn StewardStore>> {
        self.caps
            .steward
            .then(|| Arc::new(self.clone()) as Arc<dyn StewardStore>)
    }

    fn as_memory_panel_store(&self) -> Option<Arc<dyn MemoryPanelStore>> {
        self.caps
            .panel
            .then(|| Arc::new(self.clone()) as Arc<dyn MemoryPanelStore>)
    }

    fn as_selected_record_fetch(&self) -> Option<Arc<dyn SelectedRecordFetch>> {
        self.caps
            .fetch
            .then(|| Arc::new(self.clone()) as Arc<dyn SelectedRecordFetch>)
    }

    fn as_tombstone_source(&self) -> Option<Arc<dyn TombstoneSource>> {
        self.caps
            .tombstones
            .then(|| Arc::new(self.clone()) as Arc<dyn TombstoneSource>)
    }
}

impl TaintableStore for FakeJudgmentStore {
    fn set_llm_write_gate(&self, gate: Arc<dyn LlmWriteGate>) {
        self.state.gate_installs.fetch_add(1, Ordering::SeqCst);
        *self.state.gate.lock().unwrap() = Some(gate);
    }

    fn set_llm_write_gate_if_absent(&self, gate: Arc<dyn LlmWriteGate>) -> bool {
        let mut guard = self.state.gate.lock().unwrap();
        if guard.is_some() {
            return false;
        }
        self.state.gate_installs.fetch_add(1, Ordering::SeqCst);
        *guard = Some(gate);
        true
    }

    fn set_evidence_resolver(&self, resolver: Arc<dyn EvidenceRefResolver>) {
        *self.state.resolver.lock().unwrap() = Some(resolver);
    }

    fn set_event_sink(&self, sink: Arc<dyn MemoryEventSink>) {
        *self.state.sink.lock().unwrap() = Some(sink);
    }

    fn set_event_sink_if_absent(&self, sink: Arc<dyn MemoryEventSink>) -> bool {
        let mut guard = self.state.sink.lock().unwrap();
        if guard.is_some() {
            return false;
        }
        *guard = Some(sink);
        true
    }
}

#[async_trait]
impl StagedMemoryStore for FakeJudgmentStore {
    async fn stage(&self, batch: StagedMutationBatch) -> Result<StageToken, AgentMemoryError> {
        Ok(StageToken {
            realm: batch.realm,
            token: "fake-stage".to_string(),
        })
    }

    async fn commit(&self, token: StageToken) -> Result<CommitReceipt, AgentMemoryError> {
        Ok(CommitReceipt {
            token: token.token,
            applied_ops: 0,
            memory_ids: Vec::new(),
        })
    }
}

#[async_trait]
impl TombstoneSource for FakeJudgmentStore {
    async fn recent_tombstones(
        &self,
        _scope: &MemoryScope,
        _since_ms: u64,
        _limit: usize,
    ) -> Result<Vec<TombstoneMeta>, AgentMemoryError> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl StewardStore for FakeJudgmentStore {
    fn scope_floors(&self) -> (usize, usize) {
        (4_000, 32 * 1024 * 1024)
    }

    async fn scope_overview(&self, _realm: &str) -> Result<Vec<ScopeOverview>, AgentMemoryError> {
        Ok(Vec::new())
    }

    async fn pending_proposals(
        &self,
        _realm: &str,
        _limit: usize,
    ) -> Result<Vec<PendingProposal>, AgentMemoryError> {
        Ok(Vec::new())
    }

    async fn set_proposal_status(
        &self,
        _realm: &str,
        _proposal_id: &str,
        _status: &str,
    ) -> Result<(), AgentMemoryError> {
        Ok(())
    }

    async fn quarantined_records(
        &self,
        _realm: &str,
        _limit: usize,
    ) -> Result<Vec<MemoryRecord>, AgentMemoryError> {
        Ok(Vec::new())
    }

    async fn review_quarantined(
        &self,
        _request: QuarantineReviewRequest,
    ) -> Result<QuarantineReviewOutcome, QuarantineReviewError> {
        Err(QuarantineReviewError::Store(AgentMemoryError::Unsupported(
            "the fake judgment store holds no quarantine queue".to_string(),
        )))
    }

    async fn records_by_ids(
        &self,
        _realm: &str,
        ids: &[String],
    ) -> Result<Vec<MemoryRecord>, AgentMemoryError> {
        let records = self.state.records.lock().unwrap();
        Ok(records
            .iter()
            .filter(|record| ids.contains(&record.id))
            .cloned()
            .collect())
    }

    async fn recent_records(
        &self,
        _realm: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>, AgentMemoryError> {
        let records = self.state.records.lock().unwrap();
        Ok(records.iter().take(limit).cloned().collect())
    }

    async fn injection_log(
        &self,
        _realm: &str,
        _limit: usize,
    ) -> Result<Vec<InjectionLogEntry>, AgentMemoryError> {
        Ok(Vec::new())
    }

    async fn record_pending_harvest(
        &self,
        _realm: &str,
        _identity: &str,
        _session_key: Option<&str>,
        _cause: &str,
    ) -> Result<(), AgentMemoryError> {
        Ok(())
    }

    async fn pending_harvests(
        &self,
        _realm: &str,
        _limit: usize,
    ) -> Result<Vec<PendingHarvest>, AgentMemoryError> {
        Ok(Vec::new())
    }

    async fn mark_harvest_complete(
        &self,
        _realm: &str,
        _identity: &str,
        _retired_at_ms: u64,
    ) -> Result<(), AgentMemoryError> {
        Ok(())
    }

    async fn record_pending_promotion(
        &self,
        _realm: &str,
        _promotion: PendingPromotion,
    ) -> Result<(), AgentMemoryError> {
        Ok(())
    }

    async fn pending_promotion_by_id(
        &self,
        _realm: &str,
        _pending_id: &str,
    ) -> Result<Option<PendingPromotion>, AgentMemoryError> {
        Ok(None)
    }

    async fn pending_promotions(
        &self,
        _realm: &str,
    ) -> Result<Vec<PendingPromotion>, AgentMemoryError> {
        Ok(Vec::new())
    }

    async fn resolve_pending_promotion(
        &self,
        _realm: &str,
        _pending_id: &str,
        _status: &str,
    ) -> Result<bool, AgentMemoryError> {
        Ok(false)
    }

    async fn rekey_pending_promotion(
        &self,
        _realm: &str,
        _old_pending_id: &str,
        _new_pending_id: &str,
    ) -> Result<bool, AgentMemoryError> {
        Ok(false)
    }

    async fn discard_stage(&self, _token: StageToken) -> Result<(), AgentMemoryError> {
        Ok(())
    }

    async fn save_dream_run(
        &self,
        _realm: &str,
        _run: PersistedDreamRun,
    ) -> Result<(), AgentMemoryError> {
        Ok(())
    }

    async fn save_dream_audit_verdicts(
        &self,
        _realm: &str,
        _run_id: &str,
        _verdicts: Vec<(String, String, String)>,
    ) -> Result<(), AgentMemoryError> {
        Ok(())
    }
}

#[async_trait]
impl MemoryPanelStore for FakeJudgmentStore {
    async fn panel_realms(&self) -> Result<Vec<String>, AgentMemoryError> {
        Ok(vec![REALM.to_string()])
    }

    async fn record_by_id(
        &self,
        _realm: &str,
        memory_id: &str,
    ) -> Result<Option<MemoryRecord>, AgentMemoryError> {
        let records = self.state.records.lock().unwrap();
        Ok(records
            .iter()
            .find(|record| record.id == memory_id)
            .cloned())
    }

    async fn records_page(
        &self,
        _realm: &str,
        _scope_kind: Option<&str>,
        _scope_key: Option<&str>,
        _status_kind: Option<&str>,
        limit: usize,
        _cursor: Option<(u64, String)>,
    ) -> Result<PanelRecordsPage, AgentMemoryError> {
        let records = self.state.records.lock().unwrap();
        Ok(PanelRecordsPage {
            records: records.iter().take(limit).cloned().collect(),
            next_cursor: None,
        })
    }

    async fn supersede_chain(
        &self,
        _realm: &str,
        _memory_id: &str,
        _max_len: usize,
    ) -> Result<Vec<MemoryRecord>, AgentMemoryError> {
        Ok(Vec::new())
    }

    async fn injection_log_for_record(
        &self,
        _realm: &str,
        _record_id: &str,
        _limit: usize,
    ) -> Result<Vec<InjectionLogEntry>, AgentMemoryError> {
        Ok(Vec::new())
    }

    async fn dream_runs(
        &self,
        _realm: &str,
        _limit: usize,
    ) -> Result<Vec<PersistedDreamRun>, AgentMemoryError> {
        Ok(Vec::new())
    }

    async fn open_dream_audit_verdicts(
        &self,
        _realm: &str,
        _limit: usize,
    ) -> Result<Vec<DreamAuditVerdict>, AgentMemoryError> {
        Ok(Vec::new())
    }

    async fn dream_history(
        &self,
        _realm: &str,
        _max_runs: usize,
    ) -> Result<Vec<DreamRunAudit>, AgentMemoryError> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl SelectedRecordFetch for FakeJudgmentStore {
    async fn fetch_records(
        &self,
        _scopes: &[MemoryScope],
        ids: &[String],
    ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
        let records = self.state.records.lock().unwrap();
        Ok(records
            .iter()
            .filter(|record| ids.contains(&record.id))
            .map(|record| AgentMemoryRecord {
                memory_id: record.id.clone(),
                title: record.title.clone(),
                body: record.body.clone(),
                tags: record.tags.clone(),
                created_at_ms: record.created_at_ms,
                updated_at_ms: record.updated_at_ms,
            })
            .collect())
    }
}

/// Event sink seam filler; assembly requires one.
struct NullSink;

impl MemoryEventSink for NullSink {
    fn emit(&self, _event: meerkat_meta::memory::MemoryTimelineEvent) {}
}

/// Minimal LLM stub so classic-path runtimes build without a live provider.
#[derive(Clone, Default)]
struct StubClient;

impl LlmClient for StubClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            });
            yield Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            });
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

async fn rpc(app: &axum::Router, method: &str, params: Value) -> Value {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": "test",
        "method": method,
        "params": params,
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload.to_string()))
                .expect("rpc request"),
        )
        .await
        .expect("rpc response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("rpc body");
    serde_json::from_slice(&body).expect("rpc json")
}

// ---------------------------------------------------------------------------
// attach_memory_engines runs against the non-SQLite fake.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn attach_memory_engines_assembles_over_non_sqlite_provider() {
    let fake = FakeJudgmentStore::default();
    let stack = attach_memory_engines(
        Arc::new(fake.clone()),
        &AgentMemoryConfig::default(),
        &MemoryEnginesConfig::default(),
        MemoryStackSeams {
            event_sink: Some(Arc::new(NullSink)),
            ..Default::default()
        },
    )
    .expect("firewall-only stack must assemble over a capability-advertising fake");

    assert!(
        fake.gate_installed(),
        "assembly must install the taint write gate through TaintableStore"
    );
    assert!(
        fake.sink_installed(),
        "assembly must install the event sink through TaintableStore"
    );
    assert!(
        stack.panel.is_some(),
        "the stack must surface the fake's MemoryPanelStore capability"
    );
    assert!(
        stack.steward_store.is_some(),
        "the stack must surface the fake's StewardStore capability"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn attach_with_steward_engine_constructs_against_fake() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let session_store =
        meerkat_store::SqliteSessionStore::open(state_dir.path().join("sessions.db"))
            .expect("session store");

    let fake = FakeJudgmentStore::default();
    let stack = attach_memory_engines(
        Arc::new(fake.clone()),
        &AgentMemoryConfig::default(),
        &MemoryEnginesConfig {
            steward: StewardConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        },
        MemoryStackSeams {
            persistent_state: Some(state_dir.path().to_path_buf()),
            transcript_store: Some(Arc::new(session_store)),
            event_sink: Some(Arc::new(NullSink)),
            ..Default::default()
        },
    )
    .expect("steward stack must assemble over the fake StewardStore");

    assert!(
        stack.steward.is_some(),
        "the Steward engine must construct against the fake store"
    );
    assert!(
        fake.resolver_installed(),
        "steward assembly must install the evidence resolver through TaintableStore"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn attach_names_the_missing_steward_capability() {
    let fake = FakeJudgmentStore {
        caps: FakeCaps {
            steward: false,
            ..FakeCaps::default()
        },
        ..FakeJudgmentStore::default()
    };
    let err = attach_memory_engines(
        Arc::new(fake),
        &AgentMemoryConfig::default(),
        &MemoryEnginesConfig {
            steward: StewardConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        },
        MemoryStackSeams {
            event_sink: Some(Arc::new(NullSink)),
            ..Default::default()
        },
    );
    let err = match err {
        Ok(_) => panic!("steward without the capability must refuse"),
        Err(err) => err,
    };
    assert!(err.contains("StewardStore"), "{err}");
}

// ---------------------------------------------------------------------------
// The classic builder path wires firewall + panel from capabilities alone.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn fake_provider_through_builder_wires_panel_and_firewall() {
    let fake = FakeJudgmentStore::seeded();
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .agent_memory(Arc::new(fake.clone()), AgentMemoryConfig::default())
            .default_llm_client(Arc::new(StubClient))
            .build(),
    )
    .await
    .expect("classic runtime over the fake provider must build");

    assert!(
        runtime.memory_panel_store().is_some(),
        "a panel-capable provider must auto-wire the Memory panel store"
    );
    assert!(
        fake.gate_installed(),
        "the classic path must install the posture write gate through TaintableStore"
    );
    assert_eq!(
        fake.gate_installs(),
        1,
        "the classic path uses the if-absent installer exactly once"
    );

    // The panel RPC is serviceable against the fake and returns its rows.
    let app = runtime.build_reference_app_router(decision_state());
    let records = rpc(&app, "meta/memory/panel/records", json!({})).await;
    assert_eq!(records["error"], Value::Null, "{records:#?}");
    let rows = records["result"]["records"].as_array().expect("records");
    assert!(
        rows.iter()
            .any(|row| row["title"] == json!("Fake panel fact")),
        "the fake's seeded record must be readable through the panel RPC: {rows:#?}"
    );
    assert_eq!(records["result"]["realms"], json!([REALM]));

    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn classic_path_never_clobbers_an_embedder_installed_gate() {
    struct EmbedderGate;
    impl LlmWriteGate for EmbedderGate {
        fn quarantine_reason(
            &self,
            _author: &MemoryAuthor,
            _kind: meerkat_meta::memory::staged::StagedBatchKind,
            _evidence: &[meerkat_meta::memory::records::EvidenceRef],
        ) -> Option<String> {
            Some("embedder gate".to_string())
        }
    }

    let fake = FakeJudgmentStore::default();
    fake.set_llm_write_gate(Arc::new(EmbedderGate));
    assert_eq!(fake.gate_installs(), 1);

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .agent_memory(Arc::new(fake.clone()), AgentMemoryConfig::default())
            .default_llm_client(Arc::new(StubClient))
            .build(),
    )
    .await
    .expect("classic runtime over the pre-gated fake must build");

    assert_eq!(
        fake.gate_installs(),
        1,
        "set_llm_write_gate_if_absent must not clobber the embedder's gate"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// Custom provider: recall-only by its capability flags.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct RecallOnlyProvider;

#[async_trait]
impl AgentMemoryProvider for RecallOnlyProvider {
    async fn recall(
        &self,
        _request: AgentMemoryRecallRequest,
    ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
        Ok(Vec::new())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_provider_stays_recall_only_by_flags() {
    let provider = RecallOnlyProvider;

    // The flags ARE the recall-only fact: no judgment capability advertised.
    assert!(provider.as_taintable().is_none());
    assert!(provider.as_steward_store().is_none());
    assert!(provider.as_memory_panel_store().is_none());
    assert!(provider.as_selected_record_fetch().is_none());
    assert!(provider.as_tombstone_source().is_none());

    // Stack assembly against it refuses loudly, naming the capability.
    let err = attach_memory_engines(
        Arc::new(provider.clone()),
        &AgentMemoryConfig::default(),
        &MemoryEnginesConfig::default(),
        MemoryStackSeams {
            event_sink: Some(Arc::new(NullSink)),
            ..Default::default()
        },
    );
    let err = match err {
        Ok(_) => panic!("a recall-only provider cannot carry the judgment plane"),
        Err(err) => err,
    };
    assert!(err.contains("TaintableStore"), "{err}");

    // The classic builder path leaves it recall-only: no panel registered.
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .agent_memory(Arc::new(provider), AgentMemoryConfig::default())
            .default_llm_client(Arc::new(StubClient))
            .build(),
    )
    .await
    .expect("classic runtime over the custom recall-only provider must build");
    assert!(
        runtime.memory_panel_store().is_none(),
        "a provider without MemoryPanelStore must not register a panel"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

fn decision_state() -> meerkat_meta::RuntimeDecisionState {
    meerkat_meta::build_runtime_decision_state(meerkat_meta::RuntimeDecisionInputs {
        bigquery: meerkat_meta::BigQueryNaming {
            dataset: "deweld_dataset".to_string(),
            table: "deweld_table".to_string(),
        },
        trusted_meta_toml: r#"
[[modules]]
id = "router"
command = "router-bin"
args = []
restart_policy = "always"
"#
        .to_string(),
        auth: meerkat_meta::AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: vec!["root@example.test".to_string()],
        },
        trusted_oidc: meerkat_meta::TrustedOidcRuntimeConfig {
            discovery_json:
                r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                    .to_string(),
            jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
        },
        console: meerkat_meta::ConsolePolicy {
            require_app_auth: false,
            ..meerkat_meta::ConsolePolicy::default()
        },
        ops: meerkat_meta::RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}
