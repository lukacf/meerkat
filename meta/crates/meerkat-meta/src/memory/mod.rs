//! Meta agent-memory record layer (docs/design/agent-memory-architecture.md).
//!
//! Deterministic structure only — record model and scopes (`records`),
//! staged mutations with the commit validator (`staged`), and the bundled
//! per-realm SQLite store (`sqlite_store`). This module sits inside the
//! §12 bright-line ratchet (`scripts/check-memory-bright-line`): it must
//! never grow retrieval-index machinery; that is hub material.

pub mod capabilities;
pub(crate) mod completion;
pub mod coordinator;
pub mod dispatch_taint;
pub mod distiller;
pub mod events;
pub mod factory_handle;
pub mod guards;
pub mod hygienist;
pub mod records;
pub mod review;
pub mod secrets;
pub mod spawn_customizer;
pub mod sqlite_store;
pub mod staged;
pub mod steward;
pub mod taint;

pub use capabilities::{
    DreamAuditVerdict, DreamRunAudit, EvidenceRefResolver, GATED_PROMOTION_EXPIRY_MS,
    MemoryPanelStore, PanelRecordsPage, PendingHarvest, PendingPromotion, PendingProposal,
    PersistedDreamRun, ScopeOverview, StewardStore, TaintableStore,
};
pub use coordinator::{
    ConsolePrincipalOperatorResolver, MobScopeResolver, OperatorResolver, RecallCoordinator,
    ScopeBudget, TurnInjection, TurnInjectionSkip, compose_identity_scope_set,
    compose_identity_scope_set_with_bindings, compose_identity_scope_set_with_operator,
    compose_scope_budgets,
};
pub use dispatch_taint::{DispatchTaintSlot, TaintObservingLlmClient};
pub use distiller::{
    CompactionDiscardSource, CompactionFollowUp, DistillCause, DistillOutcome,
    DistillerClientHandle, DistillerConfig, DistillerEngine, DistillerError, DistillerProfile,
    DistillerTriggers, FactoryDistillerHandle, HnswDiscardSource, SessionStoreTranscriptSource,
    TombstoneMeta, TombstoneSource, TranscriptSource,
};
pub use events::{MemoryEventSink, MemoryTimelineEvent};
pub use factory_handle::{
    AnnotatedRecord, FactoryModelClientHandle, ModelClientError, ModelClientHandle,
    RecordProvenance, SelectedRecordFetch,
};
pub use guards::{BackgroundBudget, BackgroundBudgetConfig, BudgetDenied, BudgetPermit};
pub use hygienist::{
    AppliedRevision, DistillationGate, FactoryHygienistHandle, HygieneCause, HygieneOutcome,
    HygienistClientHandle, HygienistConfig, HygienistEngine, HygienistError, HygienistProfile,
    HygienistTriggers, RevisionAction, RevisionOp, RevisionProposal, RevisionReject,
    SessionServiceRevisionSeam, SpanReference, SpanReferenceSource, StoreSpanReferenceSource,
    TranscriptRevisionSeam, ValidatedRevision, distiller_follow_up, parse_revision_reply,
    validate_revision,
};
pub use records::{
    AGENT_MEMORY_TARGET_OWNER, CalibrationRef, EvidenceRef, InjectionLogEntry, InjectionSurface,
    ManifestTier, MemoryAuthor, MemoryId, MemoryKind, MemoryProvenance, MemoryRecord, MemoryScope,
    NewMemoryRecord, ProposalId, RecordMeta, RecordStatus, TrustTier, UsageEvent, UsageStats,
    VerificationClaim, content_hash,
};
pub use review::{
    QuarantineDecision, QuarantineReviewError, QuarantineReviewOutcome, QuarantineReviewRefusal,
    QuarantineReviewRequest, QuarantineReviewer, ReviewAudit, ReviewDecision,
    ReviewedRecordReceipt, quarantine_release_ops, release_copy, release_successor_id,
};
pub use spawn_customizer::MemorySpawnCustomizer;
pub use sqlite_store::SqliteAgentMemoryStore;
pub use staged::{
    CommitReceipt, StageToken, StagedBatchError, StagedBatchView, StagedMemoryStore,
    StagedMutationBatch, StagedOp, StagedRecordView, validate_batch,
};
pub use steward::{
    DreamOutcome, DreamRun, FactoryStewardHandle, MemoryConflictBridge, MemoryGatingBridge,
    MobPurposeSource, PromotionGateResolver, SessionStoreEvidenceResolver, StewardClientHandle,
    StewardConfig, StewardEngine, StewardError, StewardProfile, StewardTriggers,
};
pub use taint::{
    CompactionResetSink, ContentTrustConfig, LlmWriteGate, MemberAgentEventSink,
    SessionTaintTracker, TaintLlmWriteGate, TaintObserverGuard, TaintState, ToolContentTrust,
    spawn_member_event_observer, spawn_taint_observer,
};
