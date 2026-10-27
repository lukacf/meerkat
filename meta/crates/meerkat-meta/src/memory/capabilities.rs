//! Judgment-plane capability traits (M4 de-weld).
//!
//! The full judgment plane — the §10.1 taint firewall controls, the Steward's
//! dream read/write surface, and the console Memory panel's read API — used
//! to be inherent methods on the bundled [`SqliteAgentMemoryStore`]; every
//! consumer (StewardEngine, `memory_wiring`, the console panel, the builder)
//! was welded to that concrete type. These traits promote that surface so the
//! plane runs against ANY provider that advertises the capability, with the
//! bundled store as just one implementor — the same shape the Distiller
//! (`AgentMemoryProvider` + `TombstoneSource`) and the RecallCoordinator
//! (`SelectedRecordFetch`) already have.
//!
//! Discovery is by capability accessor on [`AgentMemoryProvider`]
//! (`as_taintable`, `as_steward_store`, `as_memory_panel_store`,
//! `as_selected_record_fetch`, `as_tombstone_source`), replacing the deleted
//! `as_sqlite_store()` downcast — the provider trait no longer names its own
//! implementation.
//!
//! Trait cut, deliberately layered:
//! - [`TaintableStore`] stands alone: firewall wiring happens before any
//!   engine exists and is required even when no engine is enabled.
//! - [`StewardStore`] extends [`StagedMemoryStore`] (the dream stages and
//!   commits batches) and [`TombstoneSource`] (the orient phase renders
//!   recent tombstones).
//! - [`MemoryPanelStore`] extends [`StewardStore`]: seven of the panel's
//!   fifteen reads are steward reads, and everything the panel renders
//!   (dream runs, promotions, proposals, harvests) is judgment-plane output
//!   — a provider that can serve the panel can serve the steward. The
//!   supertrait keeps the shared methods defined exactly once instead of
//!   duplicating names across two traits on the same implementor.
//!
//! All row types here are plain portable data (no SQLite types leak).
//!
//! [`SqliteAgentMemoryStore`]: crate::memory::sqlite_store::SqliteAgentMemoryStore
//! [`AgentMemoryProvider`]: crate::identity_first::agent_memory::AgentMemoryProvider
//! [`SelectedRecordFetch`]: crate::memory::factory_handle::SelectedRecordFetch

use std::sync::Arc;

use async_trait::async_trait;

use crate::identity_first::agent_memory::AgentMemoryError;
use crate::memory::distiller::TombstoneSource;
use crate::memory::events::MemoryEventSink;
use crate::memory::records::{
    InjectionLogEntry, MemoryAuthor, MemoryId, MemoryRecord, MemoryScope, NewMemoryRecord,
    ProposalId,
};
use crate::memory::review::{
    QuarantineReviewError, QuarantineReviewOutcome, QuarantineReviewRequest,
};
use crate::memory::staged::{StageToken, StagedMemoryStore};
use crate::memory::taint::LlmWriteGate;

// ---------------------------------------------------------------------------
// Evidence resolution (re-homed from sqlite_store.rs)
// ---------------------------------------------------------------------------

/// §10.2 P3: whether an [`EvidenceRef`] resolves against the persistent
/// session store (session exists; a cited range lies within the persisted
/// transcript). The semantic endorsement half of an `agent_verified` retier
/// is the dream's judgment (recorded in the op rationale); this is the
/// mechanical half.
///
/// [`EvidenceRef`]: crate::memory::records::EvidenceRef
pub trait EvidenceRefResolver: Send + Sync {
    fn resolves(&self, evidence: &crate::memory::records::EvidenceRef) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Firewall control surface (§10.1)
// ---------------------------------------------------------------------------

/// The §10.1 taint-firewall control surface: install the LLM write gate, the
/// evidence-ref resolver, and the timeline event sink on a store. The
/// `*_if_absent` variants are load-bearing for the classic builder path,
/// which must never clobber a gate or sink an embedder installed before
/// handing the store over.
pub trait TaintableStore: Send + Sync {
    /// Install the §10.1 LLM write gate. Wiring installs it at startup,
    /// before any member can dispatch a write.
    fn set_llm_write_gate(&self, gate: Arc<dyn LlmWriteGate>);

    /// Install the §10.1 gate only when none is present. Returns whether
    /// this call installed the gate.
    fn set_llm_write_gate_if_absent(&self, gate: Arc<dyn LlmWriteGate>) -> bool;

    /// Install the §10.2 evidence-ref resolver. The steward wiring installs
    /// it at startup; from then on every staged retier to `agent_verified`
    /// must cite evidence that resolves against the session store.
    fn set_evidence_resolver(&self, resolver: Arc<dyn EvidenceRefResolver>);

    /// Wire the §9.3 timeline sink for quarantined-write events.
    fn set_event_sink(&self, sink: Arc<dyn MemoryEventSink>);

    /// Wire the §9.3 sink only when none is present. Returns whether this
    /// call installed the sink.
    fn set_event_sink_if_absent(&self, sink: Arc<dyn MemoryEventSink>) -> bool;
}

// ---------------------------------------------------------------------------
// Steward read/write rows (§8.5)
// ---------------------------------------------------------------------------

/// Per-scope store overview row for the dream's orient phase (§8.5) and
/// the P3b console Memory panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeOverview {
    pub scope: MemoryScope,
    pub active: u64,
    pub quarantined: u64,
    pub superseded: u64,
    pub tombstoned: u64,
    pub body_bytes: u64,
}

/// One pending (or held) mob/operator-scope proposal awaiting a dream
/// verdict (§8.5 promotion).
#[derive(Debug, Clone, PartialEq)]
pub struct PendingProposal {
    pub proposal_id: ProposalId,
    pub scope: MemoryScope,
    pub record: NewMemoryRecord,
    pub author: MemoryAuthor,
    pub status: String,
    pub created_at_ms: u64,
    /// §10.1 propose-time taint fact: `Some(reason)` when the write gate
    /// would have quarantined this author at propose time. A plain steward
    /// "accept" on a tainted proposal downgrades to an operator gate.
    pub taint: Option<String>,
}

/// One retired identity awaiting an exit-interview harvest (§8.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingHarvest {
    pub identity: String,
    pub session_key: Option<String>,
    pub cause: String,
    pub retired_at_ms: u64,
}

/// Gated promotions unresolved after this long are expired and their stage
/// tokens discarded: the backstop for a gate whose decision or timeout this
/// store never saw (gating entries live in process memory and do not
/// survive a restart). The steward's dream applies it to every pending
/// promotion; a quarantine review applies it to the promotion of the record
/// it decides, so a persisted orphan blocks review only until it expires,
/// whether or not a steward runs.
pub const GATED_PROMOTION_EXPIRY_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// One gated quarantine-promotion (§10.2): the staged batch commits only on
/// gating approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPromotion {
    pub pending_id: String,
    pub stage_token: String,
    pub record_id: MemoryId,
    pub scope_kind: String,
    pub scope_key: String,
    pub rationale: Option<String>,
    pub status: String,
    pub created_at_ms: u64,
}

/// One persisted dream run (§8.5): the durable verdict sheet — phases,
/// verdict counters, and skips as `DreamRun::detail()` JSON — written by the
/// steward at the end of every pipeline run (one row per partition run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedDreamRun {
    pub run_id: String,
    pub partition_label: String,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
    pub ops_committed: u64,
    /// `DreamRun::detail()` JSON text (phases, verdicts, skips).
    pub detail: String,
}

/// One usage-audit verdict awaiting (or holding) operator review — the
/// "memories you might want to correct" queue (§16 Q6). `resolved_at_ms`
/// NULL = open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DreamAuditVerdict {
    pub run_id: String,
    pub record_id: String,
    pub verdict: String,
    pub rationale: String,
    pub created_at_ms: u64,
    pub resolved_at_ms: Option<u64>,
    pub resolution: Option<String>,
}

// ---------------------------------------------------------------------------
// Steward capability (§8.5)
// ---------------------------------------------------------------------------

/// The Steward's dream read/write surface (§8.5): orient aggregates, the
/// proposal/quarantine/harvest queues, gated-promotion bookkeeping, and the
/// durable dream ledger. [`StagedMemoryStore`] supplies stage/commit (and
/// through it the provider recall/manifest surface); [`TombstoneSource`]
/// supplies the orient phase's recent-tombstone render.
#[async_trait]
pub trait StewardStore: StagedMemoryStore + TombstoneSource {
    /// The per-scope retention floors this store warns against (§7.3);
    /// rendered into the dream's orient overview as floor pressure.
    fn scope_floors(&self) -> (usize, usize);

    /// Per-scope counts and byte totals for a realm — the orient phase's
    /// one cheap aggregate.
    async fn scope_overview(&self, realm: &str) -> Result<Vec<ScopeOverview>, AgentMemoryError>;

    /// Pending/held proposals, oldest first (§8.5 promotion queue).
    async fn pending_proposals(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<PendingProposal>, AgentMemoryError>;

    /// Record a dream verdict on a proposal: `accepted`, `rejected`, or
    /// `held` (held stays in the pending queue for the next dream).
    async fn set_proposal_status(
        &self,
        realm: &str,
        proposal_id: &str,
        status: &str,
    ) -> Result<(), AgentMemoryError>;

    /// Quarantined records, newest first — the dream's review queue (§8.5).
    /// The steward is the one stage that reads these bodies wholesale; the
    /// caller renders them defanged.
    async fn quarantined_records(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>, AgentMemoryError>;

    /// The quarantine review transaction (§8.5 quarantine review, §10.1
    /// "until steward/operator review"): decide one quarantined record in
    /// the request's scope, atomically, through the staged validator.
    /// Releases follow [`crate::memory::review`]; repeating a decision
    /// returns the `Already*` outcome without writing; anything that cannot
    /// be decided is a typed refusal.
    async fn review_quarantined(
        &self,
        request: QuarantineReviewRequest,
    ) -> Result<QuarantineReviewOutcome, QuarantineReviewError>;

    /// Records by id, any status — the gather phase's bounded body fetch.
    /// Missing ids are skipped (the model may cite stale ids).
    async fn records_by_ids(
        &self,
        realm: &str,
        ids: &[String],
    ) -> Result<Vec<MemoryRecord>, AgentMemoryError>;

    /// Most recently updated records in a realm, any scope, active or
    /// quarantined — the gather phase filters (e.g. recent distillates by
    /// author) host-side.
    async fn recent_records(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>, AgentMemoryError>;

    /// Newest-first injection-ledger rows for a realm (§9.2). Read surface
    /// for the steward's usage audit and the console Memory panel.
    async fn injection_log(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<InjectionLogEntry>, AgentMemoryError>;

    /// Record a retired identity for the next dream's exit-interview
    /// harvest (§8.5). Idempotent per (identity, retired_at_ms).
    async fn record_pending_harvest(
        &self,
        realm: &str,
        identity: &str,
        session_key: Option<&str>,
        cause: &str,
    ) -> Result<(), AgentMemoryError>;

    /// Pending exit-interview harvests, oldest first.
    async fn pending_harvests(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<PendingHarvest>, AgentMemoryError>;

    /// Mark one exit-interview harvest done.
    async fn mark_harvest_complete(
        &self,
        realm: &str,
        identity: &str,
        retired_at_ms: u64,
    ) -> Result<(), AgentMemoryError>;

    /// Record a gated quarantine-promotion: gating `pending_id` → staged
    /// batch token (§10.2). Only a gating approval commits the token.
    async fn record_pending_promotion(
        &self,
        realm: &str,
        promotion: PendingPromotion,
    ) -> Result<(), AgentMemoryError>;

    /// Look up a still-pending gated promotion by its gating pending id.
    async fn pending_promotion_by_id(
        &self,
        realm: &str,
        pending_id: &str,
    ) -> Result<Option<PendingPromotion>, AgentMemoryError>;

    /// All still-pending gated promotions (dream-start reconciliation).
    async fn pending_promotions(
        &self,
        realm: &str,
    ) -> Result<Vec<PendingPromotion>, AgentMemoryError>;

    /// Resolve a still-pending gated promotion: `committed`, `denied`, or
    /// `expired`. Returns whether this call resolved it. A promotion already
    /// resolved (an expiry, or an operator's tombstone that invalidated it)
    /// keeps its state: a late resolution never overwrites or revives it.
    async fn resolve_pending_promotion(
        &self,
        realm: &str,
        pending_id: &str,
        status: &str,
    ) -> Result<bool, AgentMemoryError>;

    /// Re-key a still-pending gated promotion after a gating escalation
    /// minted a successor pending entry. Returns whether it was re-keyed; a
    /// resolved promotion is never re-keyed back into the pending set.
    async fn rekey_pending_promotion(
        &self,
        realm: &str,
        old_pending_id: &str,
        new_pending_id: &str,
    ) -> Result<bool, AgentMemoryError>;

    /// Discard a staged-but-uncommitted batch (denied/expired gated
    /// promotions; §8.5 crash semantics keep this safe — an unapplied stage
    /// row is never visible).
    async fn discard_stage(&self, token: StageToken) -> Result<(), AgentMemoryError>;

    /// Persist one completed dream run (idempotent on run_id).
    async fn save_dream_run(
        &self,
        realm: &str,
        run: PersistedDreamRun,
    ) -> Result<(), AgentMemoryError>;

    /// Record the usage-audit verdicts of one dream run. Only non-clean
    /// verdicts belong here (the review queue); load-bearing records are
    /// counted in the run detail, not queued.
    async fn save_dream_audit_verdicts(
        &self,
        realm: &str,
        run_id: &str,
        verdicts: Vec<(String, String, String)>,
    ) -> Result<(), AgentMemoryError>;
}

// ---------------------------------------------------------------------------
// Console Memory panel rows and capability (§9.3, P3b)
// ---------------------------------------------------------------------------

/// One page of panel records: strictly-descending `(updated_at_ms,
/// memory_id)` keyset pagination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelRecordsPage {
    pub records: Vec<MemoryRecord>,
    /// Pass back as `cursor` to continue; `None` when exhausted.
    pub next_cursor: Option<(u64, String)>,
}

/// One steward dream run reconstructed from its audit rows (every committed
/// op records its `Steward { run_id }` author in the audit `detail`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DreamRunAudit {
    pub run_id: String,
    pub first_op_at_ms: u64,
    pub last_op_at_ms: u64,
    pub ops: u64,
    /// op kind → count (create/supersede/tombstone/retier/set_rank).
    pub op_kinds: std::collections::BTreeMap<String, u64>,
    /// Ops that landed quarantined at the write seam.
    pub quarantined_ops: u64,
    /// Bounded sample of touched record ids, newest first.
    pub memory_ids: Vec<String>,
    /// Bounded sample of op rationales, newest first.
    pub rationales: Vec<String>,
}

/// The console Memory panel's read API (§9.3): the ten read-only
/// `meta/memory/panel/*` RPCs. Seven of the panel's reads are steward
/// reads (overview, proposals, quarantine, promotions, harvests, injection
/// ledger, scope floors) — the [`StewardStore`] supertrait carries those;
/// this trait adds the panel-only listing, lineage, and dream-ledger reads.
/// Everything the panel renders is judgment-plane output, so requiring the
/// steward surface is the honest capability bar, not an over-ask.
#[async_trait]
pub trait MemoryPanelStore: StewardStore {
    /// Realms with store state (panel realm picker).
    async fn panel_realms(&self) -> Result<Vec<String>, AgentMemoryError>;

    /// One record by id, any status.
    async fn record_by_id(
        &self,
        realm: &str,
        memory_id: &str,
    ) -> Result<Option<MemoryRecord>, AgentMemoryError>;

    /// Panel record listing: optional scope/status filters, newest-updated
    /// first, keyset cursor. Any status is visible here — the panel is an
    /// inspection surface and renders status explicitly.
    async fn records_page(
        &self,
        realm: &str,
        scope_kind: Option<&str>,
        scope_key: Option<&str>,
        status_kind: Option<&str>,
        limit: usize,
        cursor: Option<(u64, String)>,
    ) -> Result<PanelRecordsPage, AgentMemoryError>;

    /// Supersede lineage around one record, oldest first: ancestors via the
    /// `supersedes` pointer, the record itself, then committed successors
    /// via the `Superseded { by }` status link. When the tip has no
    /// committed successor, records *claiming* to supersede it (e.g. a
    /// quarantined supersede that left the prior active, §10.1) are
    /// appended without recursing — claims are visible but never extend
    /// the walk. Bounded by `max_len`, cycle-safe.
    async fn supersede_chain(
        &self,
        realm: &str,
        memory_id: &str,
        max_len: usize,
    ) -> Result<Vec<MemoryRecord>, AgentMemoryError>;

    /// Newest-first injection-ledger rows for one record (panel usage view).
    async fn injection_log_for_record(
        &self,
        realm: &str,
        record_id: &str,
        limit: usize,
    ) -> Result<Vec<InjectionLogEntry>, AgentMemoryError>;

    /// Persisted dream runs, newest first.
    async fn dream_runs(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<PersistedDreamRun>, AgentMemoryError>;

    /// Open (unresolved) audit verdicts, newest first — the operator review
    /// queue. One row per (run, record); the console dedups by record.
    async fn open_dream_audit_verdicts(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<DreamAuditVerdict>, AgentMemoryError>;

    /// Dream-run summaries reconstructed from steward audit rows, newest
    /// run first. Bounded scan; runs older than the scan window fall off
    /// the panel, which is acceptable for a history summary surface.
    async fn dream_history(
        &self,
        realm: &str,
        max_runs: usize,
    ) -> Result<Vec<DreamRunAudit>, AgentMemoryError>;
}
