//! SessionDocumentMachine — canonical session-document registry authority.
//!
//! This machine owns per-session "session document" lifecycle facts that are
//! consumed by every session path (`meerkat-core` session/recovery and
//! `meerkat-session` ephemeral service, including the runtime-less WASM
//! path). It is a true per-session REGISTRY keyed by `SessionId`, not a
//! stateless classifier: the canonical phase truth lives in the machine's
//! own `Map` state, and transitions compute from and mutate that map.
//!
//! For now it models only the FIRST-TURN region (ported verbatim from the
//! retired `SessionDeferredTurnAuthorityMachine`). The machine is named and
//! scoped for the broader session-document domain so later folds
//! (system-context, realtime-transcript, durable-config) can join the same
//! canonical machine.
#![allow(clippy::too_many_arguments)]

use meerkat_machine_dsl::machine;

use super::OptionValueExt;

/// Bridging key type for session identity. Maps to `meerkat_core::SessionId`.
///
/// The DSL needs `Ord + Hash + Clone` for `Map` keys; this newtype satisfies
/// that while staying a thin wrapper over the session id string.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct SessionId(pub String);

/// Opaque identity of one sealed recovery candidate. Binds the exact evidence
/// (session, runtime authority stamp, store-head digest, CAS token, observed
/// run identity) so a classification of one head can never authorize mutating
/// a later head. Derived by the shell as a digest over those facts; the
/// machine treats it as opaque.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct RecoveryCandidateId(pub String);

impl<T: Into<String>> From<T> for SessionId {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Per-session first-turn lifecycle phase.
///
/// `Inactive` is the default (and the value for any session id absent from the
/// `session_first_turn_phase` map), `Pending` means the deferred first turn is
/// staged but not yet started, and `Consumed` is the absorbing terminal phase
/// once the first turn has started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionFirstTurnPhase {
    #[default]
    Inactive,
    Pending,
    Consumed,
}

/// Disposition for an initial-prompt staging decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionInitialPromptStageDecision {
    #[default]
    Clear,
    Store,
}

/// Disposition for a runtime system-context append-staging decision.
///
/// Ported verbatim from the retired `SessionSystemContextAuthorityMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SystemContextAppendDecision {
    #[default]
    Staged,
    Duplicate,
    RejectEmpty,
    RejectConflict,
}

/// Machine-owned admission verdict for a PERSIST-TIME system-context append
/// continuity check.
///
/// The session store's atomic append-only save guard must decide whether an
/// incoming persisted system prompt is an admissible runtime-context-append
/// continuation of the previously persisted one. This is the SAME machine that
/// owns the staging-path append disposition ([`SystemContextAppendDecision`]);
/// the persist-time decision is its own append-admission verdict over the
/// structural prefix observations plus the typed `is_runtime_context_append`
/// provenance marker (NOT a `[Runtime System Context]` content prefix). The
/// session-store shell extracts those pure observations, drives
/// `ResolveSystemContextPersistAppendAdmission`, and mirrors the verdict:
/// `Admit` -> the divergence is an admissible append, `Reject` -> it is not.
/// Fails closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SystemContextPersistAppendAdmission {
    #[default]
    Reject,
    Admit,
}

/// Typed provenance class for a runtime system-context append.
///
/// This is the canonical replacement for the retired `runtime:steer:` string
/// prefix folklore: the producer of a runtime-steer append constructs it with
/// [`SystemContextSource::RuntimeSteer`]; everything else is
/// [`SystemContextSource::Normal`]. The machine guards the typed field — no
/// generated or shell code reclassifies a source string into this fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SystemContextSource {
    #[default]
    Normal,
    RuntimeSteer,
}

// ---------------------------------------------------------------------------
// Realtime-transcript region typed vocabulary (folded from the retired
// SessionRealtimeTranscriptAuthorityMachine).
//
// These are the SAME typed observation/decision enums the retired machine
// carried. The bulky per-item registry (`SessionRealtimeTranscriptState`,
// the content-segment maps, the causal ordering, message assembly) stays a
// NON-generated shell helper in meerkat-core: the DSL has no string-content
// op, no topological-order op, and no materialize-loop construct, so the
// shell computes those mechanical facts and feeds them as typed RAW
// observations. The machine decides the action vector / materialize verdict
// from those observations — never the other way around.
// ---------------------------------------------------------------------------

/// Provider-neutral role for a realtime transcript item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptRoleKind {
    #[default]
    User,
    Assistant,
}

/// Output lane carried by an assistant realtime transcript item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptLaneKind {
    #[default]
    Display,
    Spoken,
}

/// Terminal-boundary stop-reason class observed for a realtime assistant turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptStopReasonKind {
    Cancelled,
    ToolUse,
    #[default]
    Other,
}

/// Per-item materialization verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptMaterializeDecision {
    #[default]
    Wait,
    MarkSkipped,
    MaterializeUser,
    MaterializeAssistant,
}

/// Machine-owned disposition for a caller-stable non-text user-input
/// identity. The shell supplies only raw registry/materialization facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeUserContentIdentityDisposition {
    #[default]
    RejectInvalidIdentity,
    RejectUnmaterializedPredecessor,
    RejectConflict,
    AlreadyCommitted,
    CommitNew,
}

/// Machine-owned one-slot admission decision for a durable pending image-blob
/// anchor. The shell supplies only occupancy/exact-match observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeUserContentBlobStageDisposition {
    #[default]
    RejectOccupied,
    StageNew,
    ReuseExact,
}

/// Machine-owned recovery decision for the durable pending image-blob slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeUserContentBlobRecoveryDisposition {
    #[default]
    NoPending,
    RetryExact,
    CommitVerifiedBeforeCurrent,
    ClearInvalidBeforeCurrent,
}

/// Machine-owned legality decision for clearing the pending slot after a
/// reducer commit (or an exact already-committed replay).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeUserContentBlobFinalizeDisposition {
    #[default]
    RejectMismatch,
    NoPending,
    ClearCommitted,
}

// ---------------------------------------------------------------------------
// Durable-config region typed vocabulary (folded from the retired
// SessionDurableConfigAuthorityMachine).
//
// The metadata-persist / build-state-persist / build-state-restore admission
// verdicts branch only on a handful of decision-relevant facts (schema
// version, model presence, mob-tool authority context kind), so those are the
// only typed observations the inputs carry. The bulky `SessionMetadata` /
// `SessionBuildState` records stay in the meerkat-core shell; a config field
// the verdict never reads is not an authority input and is not modeled here.
// ---------------------------------------------------------------------------

/// Typed provenance class for a system-prompt mutation request.
///
/// This is carried on the mutation request so every provenance is a typed
/// fact at the seam (no `source` string folklore). The mutation guard does not
/// branch on the provenance — the verdict is decided from prompt presence —
/// but keeping the class typed pins the producer's intent at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionSystemPromptSource {
    #[default]
    DirectMutation,
    ExplicitBuild,
    DefaultBuild,
    WasmDefaultBuild,
    RuntimeContextAppend,
    RuntimeSteerCleanup,
}

// ---------------------------------------------------------------------------
// Pending-continuation region typed vocabulary (folded from the retired
// non-canonical PendingContinuationAdmissionMachine).
//
// The pending-boundary is a session-document-tail-derived fact: given the
// typed `session_tail` class (the pure mechanical encoding of `messages.last()`
// produced by the `observe_session_tail` encoder, which stays a pure encoder)
// and the count of staged tool results, the machine decides whether a
// continuation has an effective pending boundary to run. This is the SAME
// `has_effective_pending_boundary` decision the retired machine carried; it now
// lives as a SessionDocumentMachine transition so both meerkat-core
// (`run_pending`) and meerkat-session (turn admission) drive the canonical
// machine and MIRROR the emitted disposition.
// ---------------------------------------------------------------------------

/// Provider-neutral class of the last message in a session's transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ObservedSessionTailKind {
    #[default]
    Empty,
    System,
    SystemNotice,
    User,
    BlockAssistant,
    ToolResults,
}

/// Disposition for a pending-continuation admission decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PendingContinuationDisposition {
    RunPending,
    #[default]
    NoPendingBoundary,
}

/// Public terminal witness emitted alongside a `NoPendingBoundary` disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PendingContinuationPublicTerminal {
    #[default]
    NoPendingBoundary,
}

// ---------------------------------------------------------------------------
// Resume-override-admission region (folded from the handwritten
// session_recovery.rs `resolve_effective_turn_config` override-admission and
// `resolve_resume_llm_binding` shell helpers under LUC-524 Dogma Invariant 1).
//
// The shell computes only typed presence/override observations against the
// surface recovery overrides and the durable session defaults (including the
// RAW first-turn phase — NOT the already-reduced overrides-allowed verdict). It
// carries NO pre-decided admission verdict. The machine decides the
// accept/reject verdict AND the effective LLM-binding selection below; the
// first-turn-overrides legality is re-derived here from the raw phase via the
// same `phase_allows_initial_turn_overrides` helper the first-turn region uses.
// ---------------------------------------------------------------------------

/// Typed reason a resume-override admission was rejected. The shell maps each
/// variant to its existing typed recovery-error message; the verdict is the
/// machine's decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ResumeOverrideRejection {
    #[default]
    ProviderRequiresModel,
    BuildOnlyAfterFirstTurn,
}

/// Effective provider selection for a resumed turn's LLM binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ResumeProviderSelection {
    /// Recompute the provider from the (new) model — clear the stored provider.
    #[default]
    RecomputeFromModel,
    /// Use the explicit provider override.
    UseOverride,
    /// Retain the stored provider.
    UseStored,
}

/// Effective self-hosted-binding selection for a resumed turn's LLM binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ResumeSelfHostedSelection {
    /// Clear the persisted self-hosted server binding (model changed).
    #[default]
    Clear,
    /// Use the explicit self-hosted server route supplied for this recovery.
    UseOverride,
    /// Retain the persisted self-hosted server binding.
    Retain,
}

/// Machine-decided live-vs-durable session-document authority verdict. The
/// session-store shell extracts four pure boolean observations of session-
/// document divergence and mirrors this verdict: `LiveAuthoritative` keeps the
/// live (runtime) session document; `DurableAuthoritative` supersedes it with
/// the stored durable document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LiveSessionAuthorityKind {
    /// The live (runtime) session document remains authoritative.
    #[default]
    LiveAuthoritative,
    /// The durable (stored) session document supersedes the live one.
    DurableAuthoritative,
}

/// Typed reason the durable session document superseded the live one. The
/// machine — not the shell — encodes the precedence (archived > uncommitted
/// transcript > runtime system-context divergence > stored transcript-revision
/// divergence) and mints this typed reason, replacing the prior `&'static str`
/// folklore. The shell mirrors the reason and branches on
/// `RuntimeSystemContextDiverged` for its runtime-context-only sync path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LiveSessionAuthorityReason {
    /// The stored session document is archived.
    #[default]
    StoredArchived,
    /// The live transcript carries uncommitted (ahead-of-durable) messages.
    LiveUncommittedTranscript,
    /// The runtime system-context state diverged from durable truth.
    RuntimeSystemContextDiverged,
    /// The stored transcript revision diverged from the live revision.
    StoredTranscriptRevisionDiverged,
}

/// Disposition for a runtime-authoritative projection save whose durable
/// session-store row ran AHEAD of the runtime authority. The intra-turn
/// best-effort checkpointer writes the durable row while the machine boundary
/// commit writes the runtime-store snapshot; the two commit points are
/// non-atomic, so a host kill (or an in-process lifecycle-commit failure that
/// evicted the uncommitted live turn) leaves the row carrying turn content
/// the machine never committed. The runtime authority is singular: the row is
/// an explicitly rebuildable projection and must converge back to committed
/// truth rather than poisoning every subsequent save. The machine — not a
/// shell comparison — owns the disposition; the shell extracts the pure
/// continuation observation and mirrors the verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeProjectionConflictDisposition {
    /// The row does not faithfully continue the authority transcript — a
    /// genuine content fork, or evidence that cannot be verified. The save
    /// fails closed exactly as before.
    #[default]
    RejectDivergent,
    /// The committed authority's checkpoint chain is AT OR PAST the row's
    /// stamped revision: the row is an intra-turn projection that its own
    /// run superseded (an aborted or raced intermediate state whose final
    /// outcome already committed). Converging the row onto committed truth
    /// discards nothing the execution did not itself supersede — a LOST tail
    /// can never reach this arm, because an uncommitted tail ahead of
    /// authority blocks every later boundary commit (the save preflight
    /// fails closed), so authority can only pass the row when no lost tail
    /// exists.
    ConvergeSupersededProjection,
    /// The row is a VERIFIED STRICT DESCENDANT of the authority transcript:
    /// its tail is durable turn content whose boundary commit never landed.
    /// The bytes are retained for a machine-owned recovery commit to promote
    /// or repair.
    ///
    /// This disposition NEVER authorizes shrinking the row. The former
    /// `RebuildToAuthority` did, on the premise that an ahead row could only
    /// be never-durable in-process residue. That premise is false: the
    /// StoreCheckpointer writes intra-turn rows to the canonical store outside
    /// the boundary transaction, so the tail can be durable — and can be a
    /// COMPLETED turn (observed: a row two messages ahead whose last message
    /// carries stop_reason=EndTurn and a concrete run_id). Discarding it is
    /// data loss, and in an agentic harness the tail also records tool calls
    /// that already executed.
    RetainForRecovery,
}

/// Coarse class of a durable row's checkpoint provenance.
///
/// The read-source decision needs to know whether the row was written by a
/// COMMITTED boundary or by the best-effort intra-turn checkpointer; it does
/// not need the full provenance vocabulary, and folding the rest into
/// `Committed` keeps the model state small.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum CheckpointProvenanceClass {
    /// No verifiable stamp on the row.
    #[default]
    Unstamped,
    /// Written by a committed boundary (run boundary, rewrite, creation,
    /// fork, or a recovery commit).
    Committed,
    /// Written by the best-effort intra-turn checkpointer, outside the
    /// boundary transaction.
    IntraTurn,
}

/// What a cold or live reader should serve for a session.
///
/// Replaces the single `read_from_store_head: bool`, which could not express
/// "the durable row is real but not yet committed authority" and therefore
/// forced that case into one of the two serving answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeSnapshotReadDisposition {
    /// Serve the committed runtime snapshot.
    UseRuntimeSnapshot,
    /// Serve the durable store head; it is a committed descendant.
    UseCommittedStoreHead,
    /// The durable head is a verified descendant written by the intra-turn
    /// checkpointer and the session is cold. It must NOT be served as ordinary
    /// authority, and it must NOT be discarded: a machine-owned recovery
    /// commit promotes or repairs it first.
    RecoveryRequired,
    /// Evidence is forked or unverifiable. Retain intact; refuse to serve.
    /// Fail-closed default: an uninitialized verdict must never serve a
    /// document, matching every sibling classifier in this region.
    #[default]
    Quarantine,
}

/// What execution an uncommitted durable tail records, as observed
/// mechanically by the shell.
///
/// The distinction matters because it selects between three different safe
/// answers, and collapsing it to a boolean maps one of them to the wrong
/// meaning:
/// - `NoExecutionContent`: the tail holds no assistant turn output at all
///   (for example a queued user message the checkpointer projected). The
///   input lifecycle still owns that work and will redeliver it, so there is
///   nothing for recovery to commit, close, or protect. Committed authority
///   is served and the row is RETAINED by the conflict machinery — holding
///   the session here would be an availability loss with no integrity gain.
/// - `BoundExecution`: the tail's assistant content carries run identity, so
///   a recovery commit can be bound to an exact run.
/// - `UnboundExecution`: the tail records assistant output that carries NO
///   run identity. This is real execution the input lifecycle will NOT
///   redeliver, and no run exists to anchor a recovery boundary to. Neither
///   serving past it (which invites a later projection rebuild to discard
///   it) nor committing it is safe: quarantine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DurableTailExecutionEvidence {
    NoExecutionContent,
    BoundExecution,
    #[default]
    UnboundExecution,
}

/// How many distinct run identities the durable tail carries.
///
/// A recoverable lost boundary is exactly one run; zero means the tail's
/// messages carry no run identity to bind a recovery commit to, and more than
/// one means multiple boundary commits were lost — both unclassifiable for the
/// first cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RunIdCardinality {
    #[default]
    NoRunId,
    SingleRunId,
    MultipleRunIds,
}

/// The terminal stop shape of the durable tail's last assistant message.
///
/// Coarse by design: the classifier needs "the provider ended the turn"
/// (EndTurn), "the turn stopped to run tools" (ToolUse), "no terminal was
/// recorded" (Absent — interrupted mid-stream), or "some other recorded stop"
/// (Other — refusals, token limits; unclassifiable for the first cut).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DurableTailStopReason {
    #[default]
    Absent,
    EndTurn,
    ToolUse,
    Other,
}

/// What kind of recovery the durable tail admits.
///
/// Classification only — authorization is MeerkatMachine's, and no class ever
/// authorizes discarding the tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DurableTailRecoveryClass {
    /// The tail is one complete turn whose boundary commit never landed:
    /// single run, EndTurn terminal, no dangling tool calls, no orphan
    /// results, nothing after the terminal.
    CompletedCandidate,
    /// The tail is one interrupted turn that recovery can close: single run,
    /// structurally coherent, but the turn never reached EndTurn (stopped at
    /// tool use or mid-stream).
    InterruptedRepairableCandidate,
    /// The tail is one complete turn written by a PRE-RUN-IDENTITY legacy
    /// writer: digest-proven strict continuation, ZERO run identity anywhere
    /// in the tail, pre-witness-v3 stamp evidence on the head row, and the
    /// clean completed shape (EndTurn terminal, no dangling calls, no orphan
    /// results, nothing after the terminal). No run id can ever appear on
    /// such a tail — the bookkeeping did not exist when it was written — so
    /// holding it for a run identity is a permanent availability loss, not
    /// caution. Adopted through a recovery boundary bound to a
    /// domain-separated deterministic legacy run identity.
    LegacyCompletedCandidate,
    /// Anything else. Held intact; never served, never discarded.
    #[default]
    Ambiguous,
}

/// Which stamp-schema era the durable head row's VERIFIED checkpoint stamp
/// advertises, as observed mechanically by the shell.
///
/// Corroborating legacy-writer evidence for identity-less durable tails.
/// Witness-v3 stamps (schema 3) ship with the same writer era as
/// run-identity-era recovery bookkeeping: a 0.8.9+ mint over graph-bearing
/// authority always advertises schema 3, and every in-run assistant append
/// since v0.7.12 persists its run identity inside the same message bytes as
/// the content (whole-document writes; a crash cannot strip identity from
/// content it was appended with). A sub-v3 stamp is therefore necessary —
/// never sufficient alone — evidence of a pre-modern writer; the machine
/// combines it with intra-turn provenance, `NoRunId` cardinality, and the
/// clean completed shape before admitting a legacy adoption.
///
/// Fail-closed default: an absent or unverifiable stamp reads as modern
/// (`WitnessV3OrNewer`), which never widens any legacy arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DurableHeadStampEra {
    #[default]
    WitnessV3OrNewer,
    PreWitnessV3,
}

/// How a durable store row relates to the committed runtime authority
/// transcript.
///
/// The shell extracts this MECHANICALLY (digest-verified prefix comparison);
/// the machine assigns meaning. Replaces the pair of shell booleans
/// (`row_continues_authority`, `row_is_runtime_checkpoint`) whose conjunction
/// silently encoded a policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DurableHeadRelation {
    /// No durable row, or the row is byte-identical to the authority.
    #[default]
    AbsentOrExact,
    /// The runtime authority leads the durable row. Ordinary forward progress.
    RuntimeSnapshotAhead,
    /// The row contains the authority as a digest-verified exact prefix and
    /// holds additional durable content beyond it.
    VerifiedStrictDescendant,
    /// The row and the authority genuinely fork.
    Diverged,
    /// The relation could not be established from available evidence.
    Unverifiable,
}

// ---------------------------------------------------------------------------
// Lifecycle-terminal region (LUC-524 R004 fold). Archive lifecycle truth was
// MODE-SPLIT: runtime-backed archived-ness was owned by MeerkatMachine
// `Retire` while store-only archived-ness was owned by the session document's
// `session_lifecycle_terminal` metadata key, with a warn-continue divergence
// window (machine retired -> projection save failed -> durable doc stayed
// Active -> standalone reopen resurrected the session). THIS machine now owns
// the `lifecycle_terminal` fact for ALL profiles: both the runtime-backed and
// the store-only archive paths drive `ArchiveSessionDocument`, and the shell
// realizes the machine's action vector fail-closed (durable document commit
// FIRST, runtime retire SECOND — a failure anywhere fails the archive
// operation, so `RuntimeState::Retired` implies the durable document is
// Archived and the divergence window is unrepresentable).
// ---------------------------------------------------------------------------

/// Canonical lifecycle-terminal class for a session document. `Active` is the
/// default (and the recovered value for a document with no terminal fact);
/// `Archived` is the absorbing terminal class. Maps to
/// `meerkat_core::SessionLifecycleTerminal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionDocumentLifecycle {
    #[default]
    Active,
    Archived,
}

/// Machine-decided disposition for a session-document archive request.
///
/// Idempotence decision (documented contract): archiving an already-Archived
/// document resolves to the explicit `AlreadyArchived` verdict — a total
/// verdict, never a guard no-match — with an empty action vector (no document
/// re-write, no runtime retire). The public surface contract maps
/// `AlreadyArchived` to its existing `NotFound` error, so re-archive remains
/// observably idempotent at the API while the machine owns the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionArchiveDisposition {
    #[default]
    Archive,
    AlreadyArchived,
}

/// Typed observation of the runtime half of a session archive.
///
/// The session-document snapshot and the runtime snapshot are independent
/// durable facts. In particular, a durable session document does not prove
/// that the runtime needs retirement, while a runtime session snapshot can
/// require retirement even when no lifecycle row has been written yet.
/// `QuiescentTerminal` covers the generated `Retired` and `Destroyed`
/// terminals, neither of which requires another Retire command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionArchiveRuntimeObservation {
    #[default]
    Absent,
    RetirementRequired,
    QuiescentTerminal,
}

/// Machine-owned disposition for realizing a committed runtime checkpoint as
/// a compatibility session-document projection.
///
/// `Archived` is absorbing: a delayed runtime-loop teardown checkpoint may
/// still carry valid committed runtime bytes, but it must not overwrite the
/// terminal document or invoke its downstream projection writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeCheckpointProjectionDisposition {
    #[default]
    IgnoreArchived,
    Project,
}

/// Mechanical transcript relation between the two legacy copies of one
/// pre-typed session document, observed by the shell during one-time
/// recovery migration (committed runtime snapshot vs session-store
/// projection). The default is the fail-closed conflict shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LegacyCheckpointTranscriptRelation {
    #[default]
    Divergent,
    Identical,
    ProjectionExtendsSnapshot,
    SnapshotExtendsProjection,
    /// Fewer than two legacy copies exist, so no transcript relation is
    /// defined; single-copy transitions never read this field.
    NotComparable,
}

/// Machine-owned disposition for the one-time recovery migration of a
/// pre-typed (legacy-unverified) session document into typed checkpoint
/// authority.
///
/// `RefuseDivergent` is the fail-closed default: copies whose transcripts
/// are not related by prefix extension — or whose only mechanical proof
/// would require trusting extra transcript content held solely by an
/// unverified legacy copy — carry no proof of which conversation is
/// authoritative, so migration is refused and the conflict surfaces as
/// typed evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LegacyCheckpointMigrationDisposition {
    #[default]
    RefuseDivergent,
    MigrateCanonicalSnapshot,
    AdoptProjectionExtension,
    MigrateStoreProjection,
    /// The runtime snapshot is already typed but the session-store row is
    /// still legacy (a crash between the migration's two durable writes, or
    /// a pre-existing partial adoption). The shell rebuilds the projection
    /// from the verified runtime authority; nothing is re-stamped.
    RebuildProjectionFromTypedSnapshot,
    /// The session-store row is already typed (sanctioned downstream
    /// adoption stamped the continuity row — for example MobKit
    /// lazy-at-restore or the bulk operator sweep) while the runtime
    /// snapshot is still the pre-adoption legacy copy, and the typed
    /// transcript contains the legacy transcript (identical or prefix
    /// extension). The typed store row IS the authority; the shell
    /// overwrites the stale legacy snapshot with the typed authority
    /// bytes. Nothing is re-stamped.
    ConvergeSnapshotOntoTypedProjection,
}

// ---------------------------------------------------------------------------
// Transcript-edit region (folded from the meerkat-session persistent.rs
// `persist_transcript_fork` / `persist_transcript_rewrite` commit paths under
// LUC-524). The persist paths commit a fork or rewrite DIRECTLY via
// `save_normalized_session` / `commit_session_transcript_rewrite_snapshot`
// with no machine authorization gate. This region authorizes the commit: the
// shell carries the typed `TranscriptEditKind` directive (fork vs rewrite) and
// drives the transition BEFORE persisting; `save_normalized_session` /
// `commit_session_transcript_rewrite_snapshot` become the effect HANDLER, not
// the decision-maker.
// ---------------------------------------------------------------------------

/// Typed class of an authorized transcript-edit commit.
///
/// `Fork` covers `fork_session` / `fork_session_replace` (the
/// `persist_transcript_fork` path); `Rewrite` covers
/// `rewrite_session_transcript` / `restore_session_transcript_revision` (the
/// `persist_transcript_rewrite` path). The producer constructs the typed
/// directive at the seam — no shell code reclassifies an edit kind from a
/// string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TranscriptEditKind {
    #[default]
    Fork,
    Rewrite,
}

machine! {
    machine SessionDocumentMachine {
        version: 1,
        rust: "self" / "catalog::dsl::session_document",

        state {
            lifecycle_phase: SessionDocumentPhase,
            session_first_turn_phase: Map<SessionId, Enum<SessionFirstTurnPhase>>,
            session_pending_initial_prompt_present: Map<SessionId, bool>,
            session_pending_tool_results_count: Map<SessionId, u64>,
            session_lifecycle_terminal: Map<SessionId, Enum<SessionDocumentLifecycle>>,
        }

        init(Ready) {
            session_first_turn_phase = EmptyMap,
            session_pending_initial_prompt_present = EmptyMap,
            session_pending_tool_results_count = EmptyMap,
            session_lifecycle_terminal = EmptyMap,
        }

        terminal []

        phase SessionDocumentPhase {
            Ready,
        }

        input SessionDocumentInput {
            MarkSessionInitialTurnPending { session_id: SessionId },
            StartSessionInitialTurn { session_id: SessionId },
            StageSessionInitialPrompt { session_id: SessionId, prompt_has_content: bool },
            StageSessionToolResults { session_id: SessionId, result_count: u64 },
            ConsumeSessionDeferredInputs { session_id: SessionId },
            RestoreSessionConsumedInputs {
                session_id: SessionId,
                restore_first_turn_pending: bool,
                pending_initial_prompt_present: bool,
                pending_tool_result_message_count: u64,
            },
            RecoverSessionFirstTurnPhase {
                session_id: SessionId,
                phase: Enum<SessionFirstTurnPhase>,
                pending_initial_prompt_present: bool,
                pending_tool_result_message_count: u64,
            },
            ResolveSessionFirstTurnOverridesAllowed { session_id: SessionId },

            // -----------------------------------------------------------
            // System-context region (folded from the retired
            // SessionSystemContextAuthorityMachine).
            //
            // The bulky append payloads (text/source strings, pending/applied
            // vectors, the seen map) stay in the shell's
            // `SessionSystemContextState`. The machine owns the per-append
            // SEMANTIC decisions: append disposition (RejectEmpty / Conflict /
            // Duplicate / Staged) and the runtime-steer apply/discard
            // disposition, which guards the TYPED `SystemContextSource` field
            // instead of a `runtime:steer:` string prefix.
            // -----------------------------------------------------------
            ResolveSystemContextAppend {
                trimmed_text_byte_count: u64,
                idempotency_key_present: bool,
                existing_key_matches: bool,
                existing_key_conflicts: bool,
                active_turn_scoped: bool,
            },
            // Per-pending-append decision for `mark_pending_applied`: a
            // runtime-steer append is dropped (and its seen entry removed); a
            // normal append is promoted to applied and its seen entry marked
            // applied. The machine guards the typed `source_kind`.
            ResolveSystemContextPendingApplyItem {
                source_kind: Enum<SystemContextSource>,
            },
            // Per-item decision for transient runtime-steer cleanup: discard
            // iff the typed `source_kind` is `RuntimeSteer`.
            ResolveSystemContextSteerCleanupItem {
                source_kind: Enum<SystemContextSource>,
            },
            // Snapshot-restore consistency authorization. Active-turn
            // membership is independent of optional idempotency keys, so the
            // shell reports one structural consistency fact covering both the
            // exact pending-position witness and its keyed rollback projection.
            RestoreSystemContextSnapshot {
                active_turn_membership_is_consistent: bool,
                seen_keys_match_known_appends: bool,
            },
            // Persist-time system-context append-admission continuity check.
            // The session-store atomic append-only save guard extracts the pure
            // structural observations (whether a previous system prompt exists,
            // whether the incoming content is byte-identical, whether it extends
            // the previous content as a prefix, whether the appended remainder
            // begins with the canonical separator) plus the typed
            // `is_runtime_context_append` provenance marker, and feeds them
            // here. THIS machine — not a handwritten shell bool reducer — owns
            // the verdict "is this incoming persisted prompt an admissible
            // runtime-context-append continuation of the persisted one". The
            // shell mirrors `Admit`/`Reject` and decides nothing.
            ResolveSystemContextPersistAppendAdmission {
                has_previous: bool,
                content_identical: bool,
                content_extends_previous: bool,
                appended_starts_with_separator: bool,
                incoming_is_runtime_context_append: bool,
            },

            // -----------------------------------------------------------
            // Realtime-transcript region (folded from the retired
            // SessionRealtimeTranscriptAuthorityMachine).
            //
            // Each input carries only typed RAW observations the shell
            // computes mechanically against its bulky
            // `SessionRealtimeTranscriptState` (set membership, segment
            // concat emptiness, per-item flags). NONE carries a pre-decided
            // action. The machine resolves the action vector / materialize
            // verdict below.
            // -----------------------------------------------------------
            ResolveRealtimeItemObserved {
                role: Enum<RealtimeTranscriptRoleKind>,
                response_discarded: bool,
            },
            ResolveRealtimeItemSkipped,
            ResolveRealtimeUserTranscriptFinal {
                text_present: bool,
                segment_empty: bool,
                segment_matches: bool,
            },
            ResolveRealtimeUserContentFinal {
                content_present: bool,
                segment_empty: bool,
                segment_matches: bool,
            },
            ResolveRealtimeUserContentIdentity {
                identity_fields_valid: bool,
                key_tombstoned: bool,
                predecessor_materialized: bool,
                existing_identity_present: bool,
                existing_payload_matches: bool,
                target_item_id_available: bool,
                reducer_commit_proof_required: bool,
                reducer_commit_proof_present: bool,
            },
            ResolveRealtimeUserContentBlobStage {
                pending_present: bool,
                pending_matches_request: bool,
            },
            ResolveRealtimeUserContentBlobRecovery {
                pending_present: bool,
                request_matches_pending: bool,
                pending_blob_valid: bool,
            },
            ResolveRealtimeUserContentBlobFinalize {
                pending_present: bool,
                pending_matches_committed: bool,
            },
            ResolveRealtimeAssistantDelta {
                response_id_valid: bool,
                response_discarded: bool,
                delta_id_present: bool,
                delta_id_seen: bool,
                item_has_text: bool,
                current_lane: Enum<RealtimeTranscriptLaneKind>,
                requested_lane: Enum<RealtimeTranscriptLaneKind>,
                response_completed: bool,
                text_after_write_present: bool,
            },
            ResolveRealtimeAssistantTextReplacement {
                response_id_valid: bool,
                response_discarded: bool,
                item_materialized: bool,
                item_has_text: bool,
                current_lane: Enum<RealtimeTranscriptLaneKind>,
                requested_lane: Enum<RealtimeTranscriptLaneKind>,
                response_completed: bool,
                text_after_replace_present: bool,
            },
            ResolveRealtimeAssistantTurnCompleted {
                response_id_valid: bool,
                response_discarded: bool,
                stop_reason: Enum<RealtimeTranscriptStopReasonKind>,
            },
            ResolveRealtimeAssistantTurnInterrupted {
                response_id_valid: bool,
            },
            ResolveRealtimeMaterializeCandidate {
                item_materialized: bool,
                predecessor_materialized: bool,
                item_skipped: bool,
                item_ready: bool,
                item_text_present: bool,
                role: Enum<RealtimeTranscriptRoleKind>,
                response_id_present: bool,
                completion_present: bool,
                completion_usage_consumed: bool,
            },
            RestoreRealtimeTranscriptState {
                item_count: u64,
                first_seen_count: u64,
                first_seen_unique_count: u64,
                every_item_has_order_entry: bool,
                every_order_entry_has_item: bool,
                all_materialized_predecessor_references_exist: bool,
                no_self_predecessor_references: bool,
                causal_graph_acyclic: bool,
                all_materialized_items_have_materialized_ancestry: bool,
                all_identity_fields_valid: bool,
                all_user_content_identity_keys_match: bool,
                all_user_content_identity_fields_valid: bool,
                all_user_content_identity_item_ids_unique: bool,
                all_user_content_identities_reference_materialized_user_items: bool,
                all_user_content_tombstones_valid: bool,
                user_content_identities_and_tombstones_disjoint: bool,
                pending_user_content_blob_fields_valid: bool,
                pending_user_content_blob_uncommitted: bool,
                all_delta_ids_valid: bool,
                all_completion_response_ids_valid: bool,
                all_discarded_response_ids_valid: bool,
                all_materialized_items_were_ready_or_skipped: bool,
                all_assistant_items_have_response_unless_skipped: bool,
                all_ready_assistant_items_have_completion_or_are_skipped: bool,
                all_materialized_assistant_completions_consumed: bool,
                all_completed_assistant_text_items_are_ready_or_materialized_or_skipped: bool,
                all_discarded_assistant_items_are_skipped_or_materialized: bool,
            },

            // -----------------------------------------------------------
            // Durable-config region (folded from the retired
            // SessionDurableConfigAuthorityMachine).
            //
            // Each input carries ONLY the typed facts the machine's
            // authorization decision actually reads — it is NOT a mirror of
            // the shell's bulky `SessionMetadata` / `SessionBuildState` record.
            // The shell persists the full record; this machine owns the
            // admit/reject verdict over the decision-relevant facts alone
            // (modeling the full record as quantified inputs would be dead
            // weight here — the machine records none of it via `update {}` —
            // and an intractable TLC cross-product). NONE carries a
            // pre-decided verdict. Rejection surfaces as the input matching no
            // transition.
            // -----------------------------------------------------------
            AuthorizeSessionMetadataPersist {
                schema_version: u64,
                model_present: bool,
            },
            AuthorizeSessionBuildStatePersist {
                mob_tool_authority_context_present: bool,
                mob_tool_authority_context_generated: bool,
            },
            RestoreSessionBuildState,
            AuthorizeSystemPromptMutation {
                source: Enum<SessionSystemPromptSource>,
                prompt_present: bool,
                prompt_byte_count: u64,
                replacing_existing: bool,
            },

            // -----------------------------------------------------------
            // Pending-continuation region (folded from the retired
            // PendingContinuationAdmissionMachine). The input carries only the
            // typed RAW observations the shell computes mechanically (the
            // session-tail class from the pure `observe_session_tail` encoder
            // and the staged tool-result count). It carries NO pre-decided
            // disposition. The machine resolves RunPending / NoPendingBoundary
            // below from those observations via `has_effective_pending_boundary`.
            // -----------------------------------------------------------
            ResolvePendingContinuation {
                session_tail: Enum<ObservedSessionTailKind>,
                staged_tool_result_count: u64,
            },

            // -----------------------------------------------------------
            // Resume-override-admission region. The input carries only typed
            // presence/override observations and the RAW first-turn phase the
            // shell read from durable session state. It carries NO pre-decided
            // verdict. The machine resolves accept (with the effective
            // LLM-binding selection) or a typed rejection below.
            // -----------------------------------------------------------
            AuthorizeSessionResumeOverrides {
                provider_override_present: bool,
                model_override_present: bool,
                self_hosted_server_override_present: bool,
                has_build_only_overrides: bool,
                first_turn_phase: Enum<SessionFirstTurnPhase>,
            },

            // -----------------------------------------------------------
            // Live-vs-durable session-document authority reconciliation. The
            // session-store shell extracts FOUR pure boolean observations of
            // session-document divergence (stored transcript revision diverged,
            // live transcript carries uncommitted messages, runtime
            // system-context diverged, stored document archived). It carries NO
            // pre-decided verdict and NO string reason. THIS machine — not a
            // handwritten boolean reducer — owns the LiveAuthoritative-vs-
            // DurableAuthoritative verdict AND the precedence (archived >
            // uncommitted transcript > runtime system-context > revision) AND
            // the typed reason. The shell mirrors the verdict + typed reason and
            // decides nothing.
            // -----------------------------------------------------------
            ClassifyLiveSessionAuthority {
                stored_transcript_diverged: bool,
                live_has_uncommitted_transcript: bool,
                runtime_system_context_diverged: bool,
                stored_is_archived: bool,
            },

            // -----------------------------------------------------------
            // Recovery-source-projection region (KEYSTONE, folded from the
            // meerkat-session persistent.rs shell predicate
            // `runtime_backed_store_projection_can_recover_authority`). When
            // the runtime snapshot is absent, the session-store projection may
            // stand in as the authoritative read source iff it carries
            // canonical session metadata, build state, or a generated runtime
            // projection quarantine fact. The shell extracts the typed
            // observations and drives this input; THIS
            // machine — not a handwritten shell `||` reducer — owns the
            // recoverable verdict. The shell mirrors `recoverable` onto its load
            // fallback (recoverable -> the projection is authoritative; not
            // recoverable -> `None`). Fails closed.
            // -----------------------------------------------------------
            RecoverSessionFromStore {
                session_id: SessionId,
                has_metadata: bool,
                has_build_state: bool,
                runtime_projection_quarantined: bool,
            },

            // -----------------------------------------------------------
            // Runtime-projection-rollback region. When a runtime-authoritative
            // projection save finds the durable session-store row AHEAD of the
            // authority transcript (intra-turn checkpointer row vs a machine
            // boundary commit that never landed — host kill or in-process
            // lifecycle-commit eviction), the shell extracts two pure
            // observations — the row judged as a faithful continuation of the
            // authority by the same run-boundary proof the save guard uses,
            // and the row's typed intra-turn checkpoint provenance fact — and
            // drives this input; THIS machine — not a handwritten shell
            // comparison — owns whether the projection write may rebuild the
            // row onto committed truth. A row without the checkpointer's own
            // provenance stamp is out-of-band divergence and keeps failing
            // closed. The shell mirrors the disposition and decides nothing.
            // -----------------------------------------------------------
            ResolveRuntimeProjectionConflict {
                session_id: SessionId,
                relation: Enum<DurableHeadRelation>,
                row_provenance: Enum<CheckpointProvenanceClass>,
                authority_supersedes_row: bool,
            },

            // -----------------------------------------------------------
            // Runtime-checkpoint projection region. A runtime-loop teardown
            // can finish a committed checkpoint after archive has made the
            // session-document lifecycle terminal. THIS machine owns whether
            // the compatibility projection is still enabled; the shell only
            // realizes Project or the absorbing IgnoreArchived no-op.
            // -----------------------------------------------------------
            ResolveRuntimeCheckpointProjection { session_id: SessionId },

            // -----------------------------------------------------------
            // Legacy-checkpoint recovery-migration region. Every document
            // written before typed checkpoint stamps decodes as
            // legacy-unverified and fails closed at each authority seam,
            // with no released on-ramp into typed authority. The shell
            // observes the mechanical carriers only — which copies exist,
            // whether each is legacy, and the transcript prefix relation
            // between them — and THIS machine owns whether the one-time
            // recovery migration runs, which copy it adopts, or whether
            // the divergence is refused as typed operator evidence.
            // -----------------------------------------------------------
            ResolveLegacyCheckpointMigration {
                session_id: SessionId,
                runtime_snapshot_present: bool,
                runtime_snapshot_legacy: bool,
                store_row_present: bool,
                store_row_legacy: bool,
                transcript_relation: Enum<LegacyCheckpointTranscriptRelation>,
            },

            // -----------------------------------------------------------
            // Runtime-snapshot read-source region. On load, the committed
            // runtime session snapshot normally leads the durable store head
            // (it commits at the run boundary). A torn shutdown can freeze it
            // as a STALE STRICT PREFIX of the store head (a completed turn's
            // boundary save landed before the snapshot recommitted); loading
            // the stale copy makes every subsequent save trip the append-only
            // guard forever. The shell extracts three pure observations —
            // the store head provably EXTENDS the snapshot (the snapshot's
            // transcript digest equals the digest of the head's same-length
            // prefix, the same continuity proof the save guard uses), the
            // head row's typed intra-turn checkpoint provenance fact, and
            // whether the session is LIVE in-process — and drives this
            // input; THIS machine owns which copy is the authoritative read
            // source. A checkpointer-stamped head is uncommitted intra-turn
            // residue (its boundary commit never landed): the snapshot stays
            // authoritative and the rollback region converges the row at
            // save time. A live session keeps the snapshot too — its lag is
            // transient and the live runtime recommits past it; only a COLD
            // load (no live session: the torn-shutdown resume) defers to the
            // extending head. A shorter, equal, or diverged head also keeps
            // the snapshot. The shell mirrors the verdict and decides
            // nothing.
            // -----------------------------------------------------------
            ResolveRuntimeSnapshotReadSource {
                session_id: SessionId,
                relation: Enum<DurableHeadRelation>,
                store_provenance: Enum<CheckpointProvenanceClass>,
                session_is_live: bool,
                // What execution, if any, does the uncommitted tail record?
                tail_execution: Enum<DurableTailExecutionEvidence>,
                // Stamp-schema era of the head row's VERIFIED stamp; the
                // fail-closed default (modern) is fed whenever no verified
                // stamp exists to observe.
                head_stamp_era: Enum<DurableHeadStampEra>,
            },

            // -----------------------------------------------------------
            // Durable-tail classification. The shell mechanically encodes
            // the tail's structure (run-id cardinality, terminal stop shape,
            // dangling/orphan tool counts); THIS machine assigns meaning.
            // candidate_id binds the exact evidence (session, authority
            // stamp, store-head digest, CAS token, observed run identity) so
            // a classification of one head can never authorize mutating a
            // later head. Tool-use IDs deliberately stay OUT of the machine:
            // the sealed list rides in the candidate payload.
            // -----------------------------------------------------------
            ClassifyDurableTail {
                session_id: SessionId,
                candidate_id: RecoveryCandidateId,
                relation: Enum<DurableHeadRelation>,
                run_id_cardinality: Enum<RunIdCardinality>,
                terminal_stop_reason: Enum<DurableTailStopReason>,
                dangling_tool_use_count: u64,
                orphan_tool_result_count: u64,
                messages_after_terminal: bool,
                // Stamp-schema era of the head row's verified stamp — the
                // corroborating legacy-writer evidence for the identity-less
                // adoption arm. Fail-closed default (modern) when no
                // verified stamp exists to observe.
                head_stamp_era: Enum<DurableHeadStampEra>,
            },

            // -----------------------------------------------------------
            // Apply-pending-tool-results region (folded from the
            // meerkat-session ephemeral.rs `agent.apply_pending_tool_results`
            // call site). Staging already consults generated authority for the
            // accepted COUNT (StageSessionToolResults); the APPLY of those
            // staged results into the live transcript was still a direct
            // mutation outside the turn machine. This input authorizes the
            // apply: the shell carries the consumed result count and drives the
            // transition; `agent.apply_pending_tool_results` becomes the effect
            // HANDLER driven by the emitted `applied_count`, not the decision
            // point.
            // -----------------------------------------------------------
            ApplyPendingToolResults {
                session_id: SessionId,
                result_count: u64,
            },

            // -----------------------------------------------------------
            // Transcript-edit region. The shell carries the typed
            // `TranscriptEditKind` directive (fork vs rewrite) and drives this
            // input BEFORE persisting; THIS machine authorizes the commit and
            // emits `TranscriptRewriteCommitted`. The persist paths
            // (`save_normalized_session` for fork,
            // `commit_session_transcript_rewrite_snapshot` for rewrite) become
            // the effect HANDLER driven by the verdict, not the decision-maker.
            // -----------------------------------------------------------
            TranscriptEdit {
                session_id: SessionId,
                fork_or_rewrite_directive: Enum<TranscriptEditKind>,
            },

            // -----------------------------------------------------------
            // Lifecycle-terminal region (LUC-524 R004 fold). The recover
            // input adopts the canonical current archived-ness observation
            // (runtime mode: the Retire realization; store-only mode: the
            // durable document's typed lifecycle-terminal fact) into the
            // machine-owned registry; the archive input then decides the
            // disposition and the realization action vector from the
            // machine-owned terminal state plus three pure mode
            // observations. Neither carries a pre-decided verdict.
            // -----------------------------------------------------------
            RecoverSessionLifecycleTerminal {
                session_id: SessionId,
                terminal: Enum<SessionDocumentLifecycle>,
            },
            ReviveArchivedSessionDocument {
                session_id: SessionId,
            },
            ArchiveSessionDocument {
                session_id: SessionId,
                runtime_backed: bool,
                durable_document_present: bool,
                runtime_observation: Enum<SessionArchiveRuntimeObservation>,
            },
        }

        effect SessionDocumentEffect {
            SessionFirstTurnPhaseResolved {
                phase: Enum<SessionFirstTurnPhase>,
                was_pending: bool,
            },
            SessionFirstTurnOverridesResolved { allowed: bool },
            SessionInitialPromptStageResolved { decision: Enum<SessionInitialPromptStageDecision> },
            SessionToolResultsStageResolved { accepted_count: u64 },
            SessionConsumedInputsRestoreResolved {
                restore_first_turn_pending: bool,
                restore_initial_prompt: bool,
                restore_tool_results: bool,
            },
            SessionFirstTurnPhaseRecovered,

            // System-context region effects.
            SystemContextAppendResolved {
                decision: Enum<SystemContextAppendDecision>,
                active_turn_scoped: bool,
            },
            // `promote_to_applied`/`mark_seen_applied` are emitted for normal
            // appends; `remove_seen` for runtime-steer appends. The shell
            // mirrors these onto its bulky pending/applied/seen collections.
            SystemContextPendingApplyItemResolved {
                promote_to_applied: bool,
                mark_seen_applied: bool,
                remove_seen: bool,
            },
            // `discard` is emitted true for runtime-steer items.
            SystemContextSteerCleanupItemResolved {
                discard: bool,
            },
            SystemContextSnapshotRestoreAuthorized,
            // Persist-time append-admission verdict. The session-store shell
            // mirrors `Admit`/`Reject` onto its atomic append-only save guard.
            SystemContextPersistAppendAdmissionResolved {
                admission: Enum<SystemContextPersistAppendAdmission>,
            },

            // Realtime-transcript region effects. The action vector is the
            // machine's decision; the shell mirrors each flag onto its bulky
            // `SessionRealtimeTranscriptState` and decides nothing.
            RealtimeTranscriptEventResolved {
                observe_item: bool,
                observe_skipped: bool,
                write_user_segment: bool,
                append_assistant_segment: bool,
                replace_assistant_segment: bool,
                promote_lane: bool,
                mark_item_ready: bool,
                record_delta_id: bool,
                remove_completion: bool,
                record_completion: bool,
                discard_response: bool,
                discard_response_by_lane: bool,
                mark_response_ready: bool,
                materialize_ready_items: bool,
            },
            RealtimeMaterializeCandidateResolved {
                decision: Enum<RealtimeTranscriptMaterializeDecision>,
                consume_usage: bool,
            },
            RealtimeUserContentIdentityResolved {
                disposition: Enum<RealtimeUserContentIdentityDisposition>,
            },
            RealtimeUserContentBlobStageResolved {
                disposition: Enum<RealtimeUserContentBlobStageDisposition>,
            },
            RealtimeUserContentBlobRecoveryResolved {
                disposition: Enum<RealtimeUserContentBlobRecoveryDisposition>,
            },
            RealtimeUserContentBlobFinalizeResolved {
                disposition: Enum<RealtimeUserContentBlobFinalizeDisposition>,
            },
            RealtimeTranscriptSnapshotRestoreAuthorized,

            // Durable-config region effects. Each is a fieldless authorization
            // marker: the admission verdict is the machine's decision (a
            // rejected request matches no transition and surfaces as `Err`).
            // The original typed `SessionMetadata` / `SessionBuildState` /
            // prompt value is carried through unchanged by the meerkat-core
            // shell wrapper — there is nothing for the machine to echo, so no
            // fact is dead-carried back across the seam.
            SessionMetadataPersistAuthorized,
            SessionBuildStatePersistAuthorized,
            SessionBuildStateRestoreAuthorized,
            SystemPromptMutationAuthorized,

            // Pending-continuation region effects. The disposition is the
            // machine's decision; the shell mirrors it onto its run-pending /
            // start-turn-disposition path and decides nothing. The public
            // terminal witness is emitted alongside `NoPendingBoundary` so the
            // shell can surface the typed terminal without re-deriving it.
            PendingContinuationResolved { disposition: Enum<PendingContinuationDisposition> },
            PendingContinuationPublicTerminalResolved { terminal: Enum<PendingContinuationPublicTerminal> },

            // Resume-override-admission region effects. On accept the machine
            // emits the verdict alongside the effective LLM-binding selection
            // (provider source, self-hosted source, provider_overridden flag);
            // the shell mirrors the selection and decides nothing. On reject the
            // machine emits the typed rejection reason; the shell maps it to its
            // typed recovery error.
            SessionResumeOverridesAuthorized {
                provider_selection: Enum<ResumeProviderSelection>,
                self_hosted_selection: Enum<ResumeSelfHostedSelection>,
                provider_overridden: bool,
            },
            SessionResumeOverridesRejected { reason: Enum<ResumeOverrideRejection> },

            // Live-vs-durable session-document authority verdict. The shell
            // mirrors `authority`: LiveAuthoritative keeps the live document;
            // DurableAuthoritative supersedes it with the stored document and
            // carries the typed precedence `reason`. On LiveAuthoritative the
            // emitted `reason` is a placeholder the shell must ignore.
            LiveSessionAuthorityClassified {
                authority: Enum<LiveSessionAuthorityKind>,
                reason: Enum<LiveSessionAuthorityReason>,
            },

            // Recovery-source-projection verdict (KEYSTONE). The shell mirrors
            // `recoverable`: true -> the store projection is an authoritative
            // read source; false -> it is not (fall through to quarantine /
            // `None`). This is a total verdict over the two typed presence
            // observations, so it is emitted on both branches (never a
            // no-match) — a store-only session that legitimately carries no
            // metadata or build state resolves to `recoverable: false`
            // explicitly rather than silently failing to load.
            SessionStoreRecoverySourceResolved { recoverable: bool },

            // Runtime-projection-conflict disposition. The shell mirrors
            // `disposition`: RetainForRecovery marks a verified strict
            // descendant as recovery-owned durable content (never shrunk,
            // never served as committed authority before a machine-owned
            // recovery commit); RejectDivergent keeps the fail-closed
            // rejection for genuine content forks. Total over the observation,
            // so it is emitted on both branches.
            RuntimeProjectionConflictResolved {
                disposition: Enum<RuntimeProjectionConflictDisposition>,
            },

            // Runtime-checkpoint compatibility-projection disposition.
            // Archived is an absorbing no-op: no downstream SessionStore
            // projection writer may be invoked for retained runtime bytes.
            RuntimeCheckpointProjectionResolved {
                disposition: Enum<RuntimeCheckpointProjectionDisposition>,
            },

            // Legacy-checkpoint recovery-migration disposition. The shell
            // mirrors this verdict exactly: it stamps the named copy via
            // recovery migration and re-projects the other, or surfaces the
            // fail-closed refusal; it never chooses a copy itself.
            LegacyCheckpointMigrationResolved {
                disposition: Enum<LegacyCheckpointMigrationDisposition>,
            },

            // Runtime-snapshot read-source verdict. The shell mirrors
            // `read_from_store_head`: true loads the durable store head (the
            // snapshot is a stale strict prefix), false keeps the runtime
            // snapshot authoritative. Total over the observation, so it is
            // emitted on both branches.
            RuntimeSnapshotReadSourceResolved {
                disposition: Enum<RuntimeSnapshotReadDisposition>,
            },

            // Durable-tail classification verdict. Total over the
            // observation, so it is emitted on every branch.
            DurableTailClassified {
                candidate_id: RecoveryCandidateId,
                class: Enum<DurableTailRecoveryClass>,
            },

            // Apply-pending-tool-results verdict. The shell mirrors
            // `applied_count` onto its `agent.apply_pending_tool_results` call:
            // it applies exactly the machine-authorized count. The verdict is
            // vacuous-accept (it mirrors the consumed count), matching the
            // staging-side `SessionToolResultsStageResolved` shape.
            SessionToolResultsApplied {
                session_id: SessionId,
                applied_count: u64,
            },

            // Transcript-edit commit verdict. The shell mirrors `success` onto
            // its persist path: it commits the fork/rewrite only after the
            // machine authorizes it. The typed `kind` echoes the authorized
            // directive so the shell routes to the correct persist handler
            // without re-deriving it.
            TranscriptRewriteCommitted {
                kind: Enum<TranscriptEditKind>,
                success: bool,
            },

            // Lifecycle-terminal region effects. The recover marker witnesses
            // the registry adoption; the archive verdict carries the
            // disposition plus the realization action vector. The shell
            // realizes the vector fail-closed and IN ORDER — durable document
            // commit first, runtime retire second — and decides nothing: a
            // realization failure surfaces as the archive operation's error
            // with durable truth still convergent (`Retired` implies the
            // document is Archived; the converse failure window is retryable).
            SessionLifecycleTerminalRecovered,
            SessionRevivalResolved,
            SessionArchiveResolved {
                disposition: Enum<SessionArchiveDisposition>,
                write_document: bool,
                retire_runtime: bool,
            },
        }

        helper phase_allows_initial_turn_overrides(phase: Enum<SessionFirstTurnPhase>) -> bool {
            phase == SessionFirstTurnPhase::Pending
        }

        helper should_store_initial_prompt(
            phase: Enum<SessionFirstTurnPhase>,
            prompt_has_content: bool
        ) -> bool {
            phase == SessionFirstTurnPhase::Pending && prompt_has_content
        }

        // System-context append classification helpers (ported verbatim from
        // the retired SessionSystemContextAuthorityMachine).
        helper append_is_empty(trimmed_text_byte_count: u64) -> bool {
            trimmed_text_byte_count == 0
        }

        helper append_is_conflict(
            idempotency_key_present: bool,
            existing_key_conflicts: bool
        ) -> bool {
            idempotency_key_present && existing_key_conflicts
        }

        helper append_is_duplicate(
            idempotency_key_present: bool,
            existing_key_matches: bool,
            existing_key_conflicts: bool
        ) -> bool {
            idempotency_key_present && existing_key_matches && existing_key_conflicts == false
        }

        helper append_is_new(
            idempotency_key_present: bool,
            existing_key_matches: bool,
            existing_key_conflicts: bool
        ) -> bool {
            idempotency_key_present == false
                || (existing_key_matches == false && existing_key_conflicts == false)
        }

        // Persist-time append-admission verdict. Admissible iff the incoming
        // persisted prompt is either a byte-identical no-op refresh of an
        // existing prompt, OR a prefix-preserving append (separator-delimited)
        // carrying the typed runtime-context-append provenance, OR — when there
        // is no previous prompt — itself a typed runtime-context-append. Every
        // other shape is rejected. Mirrors the retired
        // `system_context_is_append` shell reducer exactly.
        helper persist_append_is_admissible(
            has_previous: bool,
            content_identical: bool,
            content_extends_previous: bool,
            appended_starts_with_separator: bool,
            incoming_is_runtime_context_append: bool
        ) -> bool {
            (has_previous && content_identical)
                || (has_previous
                    && content_extends_previous
                    && appended_starts_with_separator
                    && incoming_is_runtime_context_append)
                || (has_previous == false && incoming_is_runtime_context_append)
        }

        // Realtime-transcript region classification helpers (ported verbatim
        // from the retired SessionRealtimeTranscriptAuthorityMachine).
        helper realtime_delta_is_duplicate(delta_id_present: bool, delta_id_seen: bool) -> bool {
            delta_id_present && delta_id_seen
        }

        helper realtime_lane_accepts(
            item_has_text: bool,
            current_lane: Enum<RealtimeTranscriptLaneKind>,
            requested_lane: Enum<RealtimeTranscriptLaneKind>
        ) -> bool {
            current_lane == requested_lane || item_has_text == false
        }

        helper realtime_should_mark_ready_after_write(
            response_completed: bool,
            text_after_write_present: bool
        ) -> bool {
            response_completed && text_after_write_present
        }

        helper realtime_stop_reason_discards(
            stop_reason: Enum<RealtimeTranscriptStopReasonKind>
        ) -> bool {
            stop_reason == RealtimeTranscriptStopReasonKind::Cancelled
        }

        helper realtime_stop_reason_removes_completion(
            stop_reason: Enum<RealtimeTranscriptStopReasonKind>
        ) -> bool {
            stop_reason == RealtimeTranscriptStopReasonKind::ToolUse
        }

        helper realtime_stop_reason_records_completion(
            stop_reason: Enum<RealtimeTranscriptStopReasonKind>
        ) -> bool {
            stop_reason == RealtimeTranscriptStopReasonKind::Other
        }

        // Pending-continuation classification helpers (ported verbatim from the
        // retired PendingContinuationAdmissionMachine). A `User` or `ToolResults`
        // tail is itself a runnable continuation boundary; staged tool results
        // are a boundary even when the tail is something else.
        helper tail_has_pending_boundary(session_tail: Enum<ObservedSessionTailKind>) -> bool {
            session_tail == ObservedSessionTailKind::User
                || session_tail == ObservedSessionTailKind::ToolResults
        }

        helper has_effective_pending_boundary(
            session_tail: Enum<ObservedSessionTailKind>,
            staged_tool_result_count: u64
        ) -> bool {
            tail_has_pending_boundary(session_tail) || staged_tool_result_count > 0
        }

        // Resume-override-admission classification helpers (folded from the
        // handwritten session_recovery.rs shell). Each reject condition is the
        // verbatim port of the corresponding shell `if` guard; the LLM-binding
        // selection helpers port `resolve_resume_llm_binding`.
        helper resume_reject_provider_requires_model(
            provider_override_present: bool,
            model_override_present: bool
        ) -> bool {
            provider_override_present && model_override_present == false
        }

        helper resume_reject_build_only_after_first_turn(
            has_build_only_overrides: bool,
            first_turn_phase: Enum<SessionFirstTurnPhase>
        ) -> bool {
            has_build_only_overrides
                && phase_allows_initial_turn_overrides(first_turn_phase) == false
        }

        // A resume request is admissible iff neither reject condition fires.
        // Used as the guard prefix for every accept branch. The illegal
        // "clear + set" provider_params/auth_binding fourth state is no longer
        // representable at the shell seam (it carries a single
        // `Option<TurnMetadataOverride<T>>`), so the machine no longer observes
        // or rejects it.
        helper resume_overrides_admissible(
            provider_override_present: bool,
            model_override_present: bool,
            has_build_only_overrides: bool,
            first_turn_phase: Enum<SessionFirstTurnPhase>
        ) -> bool {
            resume_reject_provider_requires_model(
                provider_override_present,
                model_override_present
            ) == false
            && resume_reject_build_only_after_first_turn(
                has_build_only_overrides,
                first_turn_phase
            ) == false
        }

        // Provider selection (port of resolve_resume_llm_binding): a model
        // change without an explicit provider override recomputes the provider
        // from the new model; an explicit provider override is used directly;
        // otherwise the stored provider is retained.
        helper resume_provider_recompute_from_model(
            model_override_present: bool,
            provider_override_present: bool
        ) -> bool {
            model_override_present && provider_override_present == false
        }

        // Recovery-source-projection predicate (port of the retired shell
        // `runtime_backed_store_projection_can_recover_authority`): a persisted
        // store projection is a valid authoritative read source iff it carries
        // canonical session metadata, build state, OR a generated runtime
        // projection quarantine fact.
        helper store_projection_can_recover_authority(
            has_metadata: bool,
            has_build_state: bool,
            runtime_projection_quarantined: bool
        ) -> bool {
            has_metadata || has_build_state || runtime_projection_quarantined
        }

        // Lifecycle-terminal realization helper: the runtime is retired iff
        // the archive is runtime-backed AND the typed runtime observation says
        // retirement work remains. The durable session-document snapshot is
        // intentionally not consulted here: it authorizes the document write,
        // but cannot prove runtime presence or override a quiescent terminal.
        helper archive_should_retire_runtime(
            runtime_backed: bool,
            runtime_observation: Enum<SessionArchiveRuntimeObservation>
        ) -> bool {
            runtime_backed
                && runtime_observation
                    == SessionArchiveRuntimeObservation::RetirementRequired
        }

        disposition SessionFirstTurnPhaseResolved => local seam NoOwnerRealization,
        disposition SessionFirstTurnOverridesResolved => local seam NoOwnerRealization,
        disposition SessionInitialPromptStageResolved => local seam NoOwnerRealization,
        disposition SessionToolResultsStageResolved => local seam NoOwnerRealization,
        disposition SessionConsumedInputsRestoreResolved => local seam NoOwnerRealization,
        disposition SessionFirstTurnPhaseRecovered => local seam NoOwnerRealization,
        disposition SystemContextAppendResolved => local seam NoOwnerRealization,
        disposition SystemContextPendingApplyItemResolved => local seam NoOwnerRealization,
        disposition SystemContextSteerCleanupItemResolved => local seam NoOwnerRealization,
        disposition SystemContextSnapshotRestoreAuthorized => local seam NoOwnerRealization,
        disposition SystemContextPersistAppendAdmissionResolved => local seam NoOwnerRealization,
        disposition RealtimeTranscriptEventResolved => local seam NoOwnerRealization,
        disposition RealtimeMaterializeCandidateResolved => local seam NoOwnerRealization,
        disposition RealtimeUserContentIdentityResolved => local seam NoOwnerRealization,
        disposition RealtimeUserContentBlobStageResolved => local seam NoOwnerRealization,
        disposition RealtimeUserContentBlobRecoveryResolved => local seam NoOwnerRealization,
        disposition RealtimeUserContentBlobFinalizeResolved => local seam NoOwnerRealization,
        disposition RealtimeTranscriptSnapshotRestoreAuthorized => local seam NoOwnerRealization,
        disposition SessionMetadataPersistAuthorized => local seam NoOwnerRealization,
        disposition SessionBuildStatePersistAuthorized => local seam NoOwnerRealization,
        disposition SessionBuildStateRestoreAuthorized => local seam NoOwnerRealization,
        disposition SystemPromptMutationAuthorized => local seam NoOwnerRealization,
        disposition PendingContinuationResolved => local seam NoOwnerRealization,
        disposition PendingContinuationPublicTerminalResolved => local seam NoOwnerRealization,
        disposition SessionResumeOverridesAuthorized => local seam NoOwnerRealization,
        disposition SessionResumeOverridesRejected => local seam NoOwnerRealization,
        disposition LiveSessionAuthorityClassified => local seam NoOwnerRealization,
        disposition SessionStoreRecoverySourceResolved => local seam NoOwnerRealization,
        disposition RuntimeProjectionConflictResolved => local seam NoOwnerRealization,
        disposition RuntimeCheckpointProjectionResolved => local seam NoOwnerRealization,
        disposition LegacyCheckpointMigrationResolved => local seam NoOwnerRealization,
        disposition RuntimeSnapshotReadSourceResolved => local seam NoOwnerRealization,
        disposition DurableTailClassified => local seam NoOwnerRealization,
        disposition SessionToolResultsApplied => local seam NoOwnerRealization,
        disposition TranscriptRewriteCommitted => local seam NoOwnerRealization,
        disposition SessionLifecycleTerminalRecovered => local seam NoOwnerRealization,
        disposition SessionRevivalResolved => local seam NoOwnerRealization,
        disposition SessionArchiveResolved => local seam NoOwnerRealization,

        // ---------------------------------------------------------------
        // MarkSessionInitialTurnPending
        //
        // Old legality (MarkInitialTurnPending): Inactive|Pending -> Pending
        // emit was_pending=false; Consumed stays Consumed emit was_pending=false.
        // ---------------------------------------------------------------
        transition MarkSessionInitialTurnPendingInactiveOrPending {
            on input MarkSessionInitialTurnPending { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (self.session_first_turn_phase.get_cloned(session_id).get("value")
                        == SessionFirstTurnPhase::Inactive
                    || self.session_first_turn_phase.get_cloned(session_id).get("value")
                        == SessionFirstTurnPhase::Pending)
            }
            update {
                self.session_first_turn_phase.insert(session_id, SessionFirstTurnPhase::Pending);
            }
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Pending,
                was_pending: false
            }
        }

        transition MarkSessionInitialTurnPendingConsumed {
            on input MarkSessionInitialTurnPending { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Consumed
            }
            update {}
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Consumed,
                was_pending: false
            }
        }

        // ---------------------------------------------------------------
        // StartSessionInitialTurn
        //
        // Old legality (StartInitialTurn): Pending -> Consumed emit
        // was_pending=true (load-bearing for rollback); Inactive stays
        // Inactive emit was_pending=false; Consumed stays Consumed emit
        // was_pending=false. `Consumed` is the absorbing phase.
        // ---------------------------------------------------------------
        transition StartSessionInitialTurnPending {
            on input StartSessionInitialTurn { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Pending
            }
            update {
                self.session_first_turn_phase.insert(session_id, SessionFirstTurnPhase::Consumed);
            }
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Consumed,
                was_pending: true
            }
        }

        transition StartSessionInitialTurnInactive {
            on input StartSessionInitialTurn { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Inactive
            }
            update {}
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Inactive,
                was_pending: false
            }
        }

        transition StartSessionInitialTurnConsumed {
            on input StartSessionInitialTurn { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Consumed
            }
            update {}
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Consumed,
                was_pending: false
            }
        }

        // ---------------------------------------------------------------
        // ResolveSessionFirstTurnOverridesAllowed
        //
        // Old legality (AllowsInitialTurnOverrides): allowed == (phase == Pending).
        // ---------------------------------------------------------------
        transition ResolveSessionFirstTurnOverridesAllowed {
            on input ResolveSessionFirstTurnOverridesAllowed { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && phase_allows_initial_turn_overrides(
                    self.session_first_turn_phase.get_cloned(session_id).get("value")
                )
            }
            update {}
            to Ready
            emit SessionFirstTurnOverridesResolved { allowed: true }
        }

        transition ResolveSessionFirstTurnOverridesDenied {
            on input ResolveSessionFirstTurnOverridesAllowed { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && phase_allows_initial_turn_overrides(
                    self.session_first_turn_phase.get_cloned(session_id).get("value")
                ) == false
            }
            update {}
            to Ready
            emit SessionFirstTurnOverridesResolved { allowed: false }
        }

        // ---------------------------------------------------------------
        // StageSessionInitialPrompt
        //
        // Old legality (ResolveInitialPromptStage): Store iff phase == Pending
        // && prompt_has_content; else Clear. The machine also tracks
        // presence in its own state for recovery legality.
        // ---------------------------------------------------------------
        transition StageSessionInitialPromptStore {
            on input StageSessionInitialPrompt { session_id, prompt_has_content }
            guard {
                self.lifecycle_phase == Phase::Ready
                && should_store_initial_prompt(
                    self.session_first_turn_phase.get_cloned(session_id).get("value"),
                    prompt_has_content
                )
            }
            update {
                self.session_pending_initial_prompt_present.insert(session_id, true);
            }
            to Ready
            emit SessionInitialPromptStageResolved {
                decision: SessionInitialPromptStageDecision::Store
            }
        }

        transition StageSessionInitialPromptClear {
            on input StageSessionInitialPrompt { session_id, prompt_has_content }
            guard {
                self.lifecycle_phase == Phase::Ready
                && should_store_initial_prompt(
                    self.session_first_turn_phase.get_cloned(session_id).get("value"),
                    prompt_has_content
                ) == false
            }
            update {
                self.session_pending_initial_prompt_present.insert(session_id, false);
            }
            to Ready
            emit SessionInitialPromptStageResolved {
                decision: SessionInitialPromptStageDecision::Clear
            }
        }

        // ---------------------------------------------------------------
        // StageSessionToolResults
        //
        // Old legality (ResolveToolResultsStage): vacuously accepts in every
        // phase, emitting accepted_count == result_count. The machine tracks
        // the count in its own state for recovery legality.
        // ---------------------------------------------------------------
        transition StageSessionToolResults {
            on input StageSessionToolResults { session_id, result_count }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (self.session_first_turn_phase.get_cloned(session_id).get("value")
                        == SessionFirstTurnPhase::Inactive
                    || self.session_first_turn_phase.get_cloned(session_id).get("value")
                        == SessionFirstTurnPhase::Pending
                    || self.session_first_turn_phase.get_cloned(session_id).get("value")
                        == SessionFirstTurnPhase::Consumed)
            }
            update {
                self.session_pending_tool_results_count.insert(session_id, result_count);
            }
            to Ready
            emit SessionToolResultsStageResolved { accepted_count: result_count }
        }

        // ---------------------------------------------------------------
        // ConsumeSessionDeferredInputs
        //
        // Old legality: consuming a started turn is `StartInitialTurn`
        // (Pending -> Consumed emit was_pending=true) followed by clearing
        // the bulky payload mirrors. The shell takes the actual payloads;
        // the machine clears its presence/count mirrors and absorbs the
        // first-turn phase.
        // ---------------------------------------------------------------
        transition ConsumeSessionDeferredInputsPending {
            on input ConsumeSessionDeferredInputs { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Pending
            }
            update {
                self.session_first_turn_phase.insert(session_id, SessionFirstTurnPhase::Consumed);
                self.session_pending_initial_prompt_present.insert(session_id, false);
                self.session_pending_tool_results_count.insert(session_id, 0);
            }
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Consumed,
                was_pending: true
            }
        }

        transition ConsumeSessionDeferredInputsInactive {
            on input ConsumeSessionDeferredInputs { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Inactive
            }
            update {
                self.session_pending_initial_prompt_present.insert(session_id, false);
                self.session_pending_tool_results_count.insert(session_id, 0);
            }
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Inactive,
                was_pending: false
            }
        }

        transition ConsumeSessionDeferredInputsConsumed {
            on input ConsumeSessionDeferredInputs { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Consumed
            }
            update {
                self.session_pending_initial_prompt_present.insert(session_id, false);
                self.session_pending_tool_results_count.insert(session_id, 0);
            }
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Consumed,
                was_pending: false
            }
        }

        // ---------------------------------------------------------------
        // RestoreSessionConsumedInputs
        //
        // Old legality (ResolveConsumedInputsRestore): vacuously authorizes,
        // emitting restore_initial_prompt == pending_initial_prompt_present
        // and restore_tool_results == (pending_tool_result_message_count > 0).
        // On a Consumed session that is being rolled back to Pending, restore
        // the machine-owned phase + presence/count mirrors.
        // ---------------------------------------------------------------
        transition RestoreSessionConsumedInputs {
            on input RestoreSessionConsumedInputs {
                session_id,
                restore_first_turn_pending,
                pending_initial_prompt_present,
                pending_tool_result_message_count
            }
            guard { self.lifecycle_phase == Phase::Ready && restore_first_turn_pending }
            update {
                self.session_first_turn_phase.insert(session_id, SessionFirstTurnPhase::Pending);
                self.session_pending_initial_prompt_present
                    .insert(session_id, pending_initial_prompt_present);
                self.session_pending_tool_results_count
                    .insert(session_id, pending_tool_result_message_count);
            }
            to Ready
            emit SessionConsumedInputsRestoreResolved {
                restore_first_turn_pending: restore_first_turn_pending,
                restore_initial_prompt: pending_initial_prompt_present,
                restore_tool_results: pending_tool_result_message_count > 0
            }
        }

        transition RestoreSessionConsumedInputsNoPhaseRollback {
            on input RestoreSessionConsumedInputs {
                session_id,
                restore_first_turn_pending,
                pending_initial_prompt_present,
                pending_tool_result_message_count
            }
            guard { self.lifecycle_phase == Phase::Ready && restore_first_turn_pending == false }
            update {
                self.session_pending_initial_prompt_present
                    .insert(session_id, pending_initial_prompt_present);
                self.session_pending_tool_results_count
                    .insert(session_id, pending_tool_result_message_count);
            }
            to Ready
            emit SessionConsumedInputsRestoreResolved {
                restore_first_turn_pending: restore_first_turn_pending,
                restore_initial_prompt: pending_initial_prompt_present,
                restore_tool_results: pending_tool_result_message_count > 0
            }
        }

        // ---------------------------------------------------------------
        // RecoverSessionFirstTurnPhase
        //
        // Old legality (RestoreDeferredTurnState): authorize a durable
        // snapshot restore for any first-turn phase, then adopt the restored
        // phase + presence/count into the machine-owned registry.
        // ---------------------------------------------------------------
        transition RecoverSessionFirstTurnPhase {
            on input RecoverSessionFirstTurnPhase {
                session_id,
                phase,
                pending_initial_prompt_present,
                pending_tool_result_message_count
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (phase == SessionFirstTurnPhase::Inactive
                    || phase == SessionFirstTurnPhase::Pending
                    || phase == SessionFirstTurnPhase::Consumed)
            }
            update {
                self.session_first_turn_phase.insert(session_id, phase);
                self.session_pending_initial_prompt_present
                    .insert(session_id, pending_initial_prompt_present);
                self.session_pending_tool_results_count
                    .insert(session_id, pending_tool_result_message_count);
            }
            to Ready
            emit SessionFirstTurnPhaseRecovered
        }

        // ===============================================================
        // System-context region (folded from the retired
        // SessionSystemContextAuthorityMachine).
        // ===============================================================

        // ---------------------------------------------------------------
        // ResolveSystemContextAppend — four-way append disposition.
        //
        // Ported verbatim from the retired ResolveAppend transitions. The
        // observations (key present / matches / conflicts) are mechanical
        // string-equality facts the shell computes against its bulky `seen`
        // map; the SEMANTIC disposition is decided here from those typed
        // observations via the append classification helpers.
        // ---------------------------------------------------------------
        transition ResolveSystemContextAppendEmpty {
            on input ResolveSystemContextAppend {
                trimmed_text_byte_count,
                idempotency_key_present,
                existing_key_matches,
                existing_key_conflicts,
                active_turn_scoped
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && append_is_empty(trimmed_text_byte_count)
            }
            update {}
            to Ready
            emit SystemContextAppendResolved {
                decision: SystemContextAppendDecision::RejectEmpty,
                active_turn_scoped: active_turn_scoped
            }
        }

        transition ResolveSystemContextAppendConflict {
            on input ResolveSystemContextAppend {
                trimmed_text_byte_count,
                idempotency_key_present,
                existing_key_matches,
                existing_key_conflicts,
                active_turn_scoped
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && append_is_empty(trimmed_text_byte_count) == false
                && append_is_conflict(idempotency_key_present, existing_key_conflicts)
            }
            update {}
            to Ready
            emit SystemContextAppendResolved {
                decision: SystemContextAppendDecision::RejectConflict,
                active_turn_scoped: active_turn_scoped
            }
        }

        transition ResolveSystemContextAppendDuplicate {
            on input ResolveSystemContextAppend {
                trimmed_text_byte_count,
                idempotency_key_present,
                existing_key_matches,
                existing_key_conflicts,
                active_turn_scoped
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && append_is_empty(trimmed_text_byte_count) == false
                && append_is_duplicate(
                    idempotency_key_present,
                    existing_key_matches,
                    existing_key_conflicts)
            }
            update {}
            to Ready
            emit SystemContextAppendResolved {
                decision: SystemContextAppendDecision::Duplicate,
                active_turn_scoped: active_turn_scoped
            }
        }

        transition ResolveSystemContextAppendNew {
            on input ResolveSystemContextAppend {
                trimmed_text_byte_count,
                idempotency_key_present,
                existing_key_matches,
                existing_key_conflicts,
                active_turn_scoped
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && append_is_empty(trimmed_text_byte_count) == false
                && append_is_new(
                    idempotency_key_present,
                    existing_key_matches,
                    existing_key_conflicts)
            }
            update {}
            to Ready
            emit SystemContextAppendResolved {
                decision: SystemContextAppendDecision::Staged,
                active_turn_scoped: active_turn_scoped
            }
        }

        // ---------------------------------------------------------------
        // ResolveSystemContextPersistAppendAdmission — persist-time
        // append-admission continuity verdict for the session-store atomic
        // append-only save guard. The shell extracts the structural prefix
        // observations plus the typed runtime-context-append provenance; this
        // machine owns the Admit/Reject verdict via persist_append_is_admissible.
        // ---------------------------------------------------------------
        transition ResolveSystemContextPersistAppendAdmissionAdmit {
            on input ResolveSystemContextPersistAppendAdmission {
                has_previous,
                content_identical,
                content_extends_previous,
                appended_starts_with_separator,
                incoming_is_runtime_context_append
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persist_append_is_admissible(
                    has_previous,
                    content_identical,
                    content_extends_previous,
                    appended_starts_with_separator,
                    incoming_is_runtime_context_append)
            }
            update {}
            to Ready
            emit SystemContextPersistAppendAdmissionResolved {
                admission: SystemContextPersistAppendAdmission::Admit
            }
        }

        transition ResolveSystemContextPersistAppendAdmissionReject {
            on input ResolveSystemContextPersistAppendAdmission {
                has_previous,
                content_identical,
                content_extends_previous,
                appended_starts_with_separator,
                incoming_is_runtime_context_append
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persist_append_is_admissible(
                    has_previous,
                    content_identical,
                    content_extends_previous,
                    appended_starts_with_separator,
                    incoming_is_runtime_context_append) == false
            }
            update {}
            to Ready
            emit SystemContextPersistAppendAdmissionResolved {
                admission: SystemContextPersistAppendAdmission::Reject
            }
        }

        // ---------------------------------------------------------------
        // ResolveSystemContextPendingApplyItem — per-pending-append apply
        // decision. Guards the TYPED source_kind: a runtime-steer append is
        // dropped from the applied set and its seen entry removed; a normal
        // append is promoted to applied and its seen entry marked applied.
        //
        // This replaces the retired `is_runtime_steer_append` string-prefix
        // classification (`source.starts_with("runtime:steer:")`).
        // ---------------------------------------------------------------
        transition ResolveSystemContextPendingApplyItemRuntimeSteer {
            on input ResolveSystemContextPendingApplyItem { source_kind }
            guard {
                self.lifecycle_phase == Phase::Ready
                && source_kind == SystemContextSource::RuntimeSteer
            }
            update {}
            to Ready
            emit SystemContextPendingApplyItemResolved {
                promote_to_applied: false,
                mark_seen_applied: false,
                remove_seen: true
            }
        }

        transition ResolveSystemContextPendingApplyItemNormal {
            on input ResolveSystemContextPendingApplyItem { source_kind }
            guard {
                self.lifecycle_phase == Phase::Ready
                && source_kind == SystemContextSource::Normal
            }
            update {}
            to Ready
            emit SystemContextPendingApplyItemResolved {
                promote_to_applied: true,
                mark_seen_applied: true,
                remove_seen: false
            }
        }

        // ---------------------------------------------------------------
        // ResolveSystemContextSteerCleanupItem — per-item transient-steer
        // discard decision, guarding the typed source_kind.
        // ---------------------------------------------------------------
        transition ResolveSystemContextSteerCleanupItemRuntimeSteer {
            on input ResolveSystemContextSteerCleanupItem { source_kind }
            guard {
                self.lifecycle_phase == Phase::Ready
                && source_kind == SystemContextSource::RuntimeSteer
            }
            update {}
            to Ready
            emit SystemContextSteerCleanupItemResolved { discard: true }
        }

        transition ResolveSystemContextSteerCleanupItemNormal {
            on input ResolveSystemContextSteerCleanupItem { source_kind }
            guard {
                self.lifecycle_phase == Phase::Ready
                && source_kind == SystemContextSource::Normal
            }
            update {}
            to Ready
            emit SystemContextSteerCleanupItemResolved { discard: false }
        }

        // ---------------------------------------------------------------
        // RestoreSystemContextSnapshot — snapshot-restore consistency
        // authorization for key-independent active-turn membership and seen
        // idempotency projections.
        // ---------------------------------------------------------------
        transition RestoreSystemContextSnapshot {
            on input RestoreSystemContextSnapshot {
                active_turn_membership_is_consistent,
                seen_keys_match_known_appends
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && active_turn_membership_is_consistent
                && seen_keys_match_known_appends
            }
            update {}
            to Ready
            emit SystemContextSnapshotRestoreAuthorized
        }

        // ===============================================================
        // Realtime-transcript region (folded from the retired
        // SessionRealtimeTranscriptAuthorityMachine). Each transition is a
        // verbatim port: it reads only the typed RAW observations carried on
        // the input and resolves the action vector / materialize verdict.
        // The shell mirrors the emitted decision onto its bulky
        // `SessionRealtimeTranscriptState` and decides nothing.
        // ===============================================================

        transition ResolveRealtimeItemObservedDiscardedAssistant {
            on input ResolveRealtimeItemObserved { role, response_discarded }
            guard {
                self.lifecycle_phase == Phase::Ready
                && role == RealtimeTranscriptRoleKind::Assistant
                && response_discarded
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: true,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeItemObservedPresent {
            on input ResolveRealtimeItemObserved { role, response_discarded }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (role != RealtimeTranscriptRoleKind::Assistant || response_discarded == false)
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeItemSkipped {
            on input ResolveRealtimeItemSkipped
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: true,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeUserTranscriptFinalEmpty {
            on input ResolveRealtimeUserTranscriptFinal { text_present, segment_empty, segment_matches }
            guard {
                self.lifecycle_phase == Phase::Ready
                && text_present == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: true,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeUserTranscriptFinalStore {
            on input ResolveRealtimeUserTranscriptFinal { text_present, segment_empty, segment_matches }
            guard {
                self.lifecycle_phase == Phase::Ready
                && text_present
                && segment_empty
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: true,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: true,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeUserTranscriptFinalReplayOrConflict {
            on input ResolveRealtimeUserTranscriptFinal { text_present, segment_empty, segment_matches }
            guard {
                self.lifecycle_phase == Phase::Ready
                && text_present
                && segment_empty == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: true,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        // Caller-stable identity admission is a separate generated decision
        // from content-segment materialization. The shell computes only raw
        // validation/registry/predecessor observations and mirrors this
        // disposition onto its durable committed-only identity registry.
        transition ResolveRealtimeUserContentIdentityInvalid {
            on input ResolveRealtimeUserContentIdentity { identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (identity_fields_valid == false
                    || (key_tombstoned == false
                        && predecessor_materialized
                        && existing_identity_present == false
                        && target_item_id_available
                        && reducer_commit_proof_required
                        && reducer_commit_proof_present == false))
            }
            update {}
            to Ready
            emit RealtimeUserContentIdentityResolved {
                disposition: RealtimeUserContentIdentityDisposition::RejectInvalidIdentity
            }
        }

        transition ResolveRealtimeUserContentIdentityUnmaterializedPredecessor {
            on input ResolveRealtimeUserContentIdentity { identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present }
            guard {
                self.lifecycle_phase == Phase::Ready
                && identity_fields_valid
                && key_tombstoned == false
                && predecessor_materialized == false
            }
            update {}
            to Ready
            emit RealtimeUserContentIdentityResolved {
                disposition: RealtimeUserContentIdentityDisposition::RejectUnmaterializedPredecessor
            }
        }

        transition ResolveRealtimeUserContentIdentityConflict {
            on input ResolveRealtimeUserContentIdentity { identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present }
            guard {
                self.lifecycle_phase == Phase::Ready
                && identity_fields_valid
                && (key_tombstoned
                    || (predecessor_materialized
                        && ((existing_identity_present && existing_payload_matches == false)
                            || (existing_identity_present == false && target_item_id_available == false))))
            }
            update {}
            to Ready
            emit RealtimeUserContentIdentityResolved {
                disposition: RealtimeUserContentIdentityDisposition::RejectConflict
            }
        }

        transition ResolveRealtimeUserContentIdentityReplay {
            on input ResolveRealtimeUserContentIdentity { identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present }
            guard {
                self.lifecycle_phase == Phase::Ready
                && identity_fields_valid
                && key_tombstoned == false
                && predecessor_materialized
                && existing_identity_present
                && existing_payload_matches
            }
            update {}
            to Ready
            emit RealtimeUserContentIdentityResolved {
                disposition: RealtimeUserContentIdentityDisposition::AlreadyCommitted
            }
        }

        transition ResolveRealtimeUserContentIdentityCommitNew {
            on input ResolveRealtimeUserContentIdentity { identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present }
            guard {
                self.lifecycle_phase == Phase::Ready
                && identity_fields_valid
                && key_tombstoned == false
                && predecessor_materialized
                && existing_identity_present == false
                && target_item_id_available
                && (reducer_commit_proof_required == false || reducer_commit_proof_present)
            }
            update {}
            to Ready
            emit RealtimeUserContentIdentityResolved {
                disposition: RealtimeUserContentIdentityDisposition::CommitNew
            }
        }

        // The durable blob staging slot is bounded to one anchor per session.
        // The shell exposes only occupancy/equality observations; this machine
        // owns whether a caller may create, reuse, reject, recover, or clear it.
        transition ResolveRealtimeUserContentBlobStageNew {
            on input ResolveRealtimeUserContentBlobStage { pending_present, pending_matches_request }
            guard {
                self.lifecycle_phase == Phase::Ready
                && pending_present == false
            }
            update {}
            to Ready
            emit RealtimeUserContentBlobStageResolved {
                disposition: RealtimeUserContentBlobStageDisposition::StageNew
            }
        }

        transition ResolveRealtimeUserContentBlobStageReuseExact {
            on input ResolveRealtimeUserContentBlobStage { pending_present, pending_matches_request }
            guard {
                self.lifecycle_phase == Phase::Ready
                && pending_present
                && pending_matches_request
            }
            update {}
            to Ready
            emit RealtimeUserContentBlobStageResolved {
                disposition: RealtimeUserContentBlobStageDisposition::ReuseExact
            }
        }

        transition ResolveRealtimeUserContentBlobStageRejectOccupied {
            on input ResolveRealtimeUserContentBlobStage { pending_present, pending_matches_request }
            guard {
                self.lifecycle_phase == Phase::Ready
                && pending_present
                && pending_matches_request == false
            }
            update {}
            to Ready
            emit RealtimeUserContentBlobStageResolved {
                disposition: RealtimeUserContentBlobStageDisposition::RejectOccupied
            }
        }

        transition ResolveRealtimeUserContentBlobRecoveryNone {
            on input ResolveRealtimeUserContentBlobRecovery { pending_present, request_matches_pending, pending_blob_valid }
            guard {
                self.lifecycle_phase == Phase::Ready
                && pending_present == false
            }
            update {}
            to Ready
            emit RealtimeUserContentBlobRecoveryResolved {
                disposition: RealtimeUserContentBlobRecoveryDisposition::NoPending
            }
        }

        transition ResolveRealtimeUserContentBlobRecoveryExact {
            on input ResolveRealtimeUserContentBlobRecovery { pending_present, request_matches_pending, pending_blob_valid }
            guard {
                self.lifecycle_phase == Phase::Ready
                && pending_present
                && request_matches_pending
            }
            update {}
            to Ready
            emit RealtimeUserContentBlobRecoveryResolved {
                disposition: RealtimeUserContentBlobRecoveryDisposition::RetryExact
            }
        }

        transition ResolveRealtimeUserContentBlobRecoveryCommitVerified {
            on input ResolveRealtimeUserContentBlobRecovery { pending_present, request_matches_pending, pending_blob_valid }
            guard {
                self.lifecycle_phase == Phase::Ready
                && pending_present
                && request_matches_pending == false
                && pending_blob_valid
            }
            update {}
            to Ready
            emit RealtimeUserContentBlobRecoveryResolved {
                disposition: RealtimeUserContentBlobRecoveryDisposition::CommitVerifiedBeforeCurrent
            }
        }

        transition ResolveRealtimeUserContentBlobRecoveryClearInvalid {
            on input ResolveRealtimeUserContentBlobRecovery { pending_present, request_matches_pending, pending_blob_valid }
            guard {
                self.lifecycle_phase == Phase::Ready
                && pending_present
                && request_matches_pending == false
                && pending_blob_valid == false
            }
            update {}
            to Ready
            emit RealtimeUserContentBlobRecoveryResolved {
                disposition: RealtimeUserContentBlobRecoveryDisposition::ClearInvalidBeforeCurrent
            }
        }

        transition ResolveRealtimeUserContentBlobFinalizeNone {
            on input ResolveRealtimeUserContentBlobFinalize { pending_present, pending_matches_committed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && pending_present == false
            }
            update {}
            to Ready
            emit RealtimeUserContentBlobFinalizeResolved {
                disposition: RealtimeUserContentBlobFinalizeDisposition::NoPending
            }
        }

        transition ResolveRealtimeUserContentBlobFinalizeClearCommitted {
            on input ResolveRealtimeUserContentBlobFinalize { pending_present, pending_matches_committed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && pending_present
                && pending_matches_committed
            }
            update {}
            to Ready
            emit RealtimeUserContentBlobFinalizeResolved {
                disposition: RealtimeUserContentBlobFinalizeDisposition::ClearCommitted
            }
        }

        transition ResolveRealtimeUserContentBlobFinalizeRejectMismatch {
            on input ResolveRealtimeUserContentBlobFinalize { pending_present, pending_matches_committed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && pending_present
                && pending_matches_committed == false
            }
            update {}
            to Ready
            emit RealtimeUserContentBlobFinalizeResolved {
                disposition: RealtimeUserContentBlobFinalizeDisposition::RejectMismatch
            }
        }

        // Multimodal user content follows the same generated action-vector
        // contract as a finalized transcript segment, but it is a distinct
        // typed input so the shell never launders image presence through a
        // synthetic text placeholder. `write_user_segment` means "write the
        // typed user segment" for both inputs; the shell mirrors it into the
        // text or block map selected by the event variant.
        transition ResolveRealtimeUserContentFinalEmpty {
            on input ResolveRealtimeUserContentFinal { content_present, segment_empty, segment_matches }
            guard {
                self.lifecycle_phase == Phase::Ready
                && content_present == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: true,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeUserContentFinalStore {
            on input ResolveRealtimeUserContentFinal { content_present, segment_empty, segment_matches }
            guard {
                self.lifecycle_phase == Phase::Ready
                && content_present
                && segment_empty
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: true,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: true,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeUserContentFinalReplayOrConflict {
            on input ResolveRealtimeUserContentFinal { content_present, segment_empty, segment_matches }
            guard {
                self.lifecycle_phase == Phase::Ready
                && content_present
                && segment_empty == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: true,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantDeltaInvalidOrDuplicate {
            on input ResolveRealtimeAssistantDelta {
                response_id_valid,
                response_discarded,
                delta_id_present,
                delta_id_seen,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_write_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (response_id_valid == false
                    || realtime_delta_is_duplicate(delta_id_present, delta_id_seen))
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: false
            }
        }

        transition ResolveRealtimeAssistantDeltaDiscarded {
            on input ResolveRealtimeAssistantDelta {
                response_id_valid,
                response_discarded,
                delta_id_present,
                delta_id_seen,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_write_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: true,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantDeltaLaneConflict {
            on input ResolveRealtimeAssistantDelta {
                response_id_valid,
                response_discarded,
                delta_id_present,
                delta_id_seen,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_write_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && realtime_delta_is_duplicate(delta_id_present, delta_id_seen) == false
                && realtime_lane_accepts(item_has_text, current_lane, requested_lane) == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: delta_id_present,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantDeltaAccepted {
            on input ResolveRealtimeAssistantDelta {
                response_id_valid,
                response_discarded,
                delta_id_present,
                delta_id_seen,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_write_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && realtime_delta_is_duplicate(delta_id_present, delta_id_seen) == false
                && realtime_lane_accepts(item_has_text, current_lane, requested_lane)
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: true,
                replace_assistant_segment: false,
                promote_lane: true,
                mark_item_ready: realtime_should_mark_ready_after_write(response_completed, text_after_write_present),
                record_delta_id: delta_id_present,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantReplacementInvalid {
            on input ResolveRealtimeAssistantTextReplacement {
                response_id_valid,
                response_discarded,
                item_materialized,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_replace_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: false
            }
        }

        transition ResolveRealtimeAssistantReplacementDiscarded {
            on input ResolveRealtimeAssistantTextReplacement {
                response_id_valid,
                response_discarded,
                item_materialized,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_replace_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: true,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantReplacementLocked {
            on input ResolveRealtimeAssistantTextReplacement {
                response_id_valid,
                response_discarded,
                item_materialized,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_replace_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && item_materialized
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantReplacementLaneConflict {
            on input ResolveRealtimeAssistantTextReplacement {
                response_id_valid,
                response_discarded,
                item_materialized,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_replace_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && item_materialized == false
                && realtime_lane_accepts(item_has_text, current_lane, requested_lane) == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantReplacementAccepted {
            on input ResolveRealtimeAssistantTextReplacement {
                response_id_valid,
                response_discarded,
                item_materialized,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_replace_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && item_materialized == false
                && realtime_lane_accepts(item_has_text, current_lane, requested_lane)
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: true,
                promote_lane: true,
                mark_item_ready: realtime_should_mark_ready_after_write(response_completed, text_after_replace_present),
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantTurnCompletedInvalid {
            on input ResolveRealtimeAssistantTurnCompleted { response_id_valid, response_discarded, stop_reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: false
            }
        }

        transition ResolveRealtimeAssistantTurnCompletedDiscard {
            on input ResolveRealtimeAssistantTurnCompleted { response_id_valid, response_discarded, stop_reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && (response_discarded || realtime_stop_reason_discards(stop_reason))
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: true,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantTurnCompletedToolUse {
            on input ResolveRealtimeAssistantTurnCompleted { response_id_valid, response_discarded, stop_reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && realtime_stop_reason_removes_completion(stop_reason)
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: true,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantTurnCompletedRecord {
            on input ResolveRealtimeAssistantTurnCompleted { response_id_valid, response_discarded, stop_reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && realtime_stop_reason_records_completion(stop_reason)
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: true,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: true,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantTurnInterruptedInvalid {
            on input ResolveRealtimeAssistantTurnInterrupted { response_id_valid }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: false
            }
        }

        transition ResolveRealtimeAssistantTurnInterruptedValid {
            on input ResolveRealtimeAssistantTurnInterrupted { response_id_valid }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: true,
                discard_response: false,
                discard_response_by_lane: true,
                mark_response_ready: true,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeMaterializeAlreadyDone {
            on input ResolveRealtimeMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized
            }
            update {}
            to Ready
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::Wait,
                consume_usage: false
            }
        }

        transition ResolveRealtimeMaterializeWaitForPredecessor {
            on input ResolveRealtimeMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized == false
                && predecessor_materialized == false
            }
            update {}
            to Ready
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::Wait,
                consume_usage: false
            }
        }

        transition ResolveRealtimeMaterializeSkipped {
            on input ResolveRealtimeMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized == false
                && predecessor_materialized
                && item_skipped
            }
            update {}
            to Ready
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::MarkSkipped,
                consume_usage: false
            }
        }

        transition ResolveRealtimeMaterializeWaitForReadyText {
            on input ResolveRealtimeMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized == false
                && predecessor_materialized
                && item_skipped == false
                && (item_ready == false || item_text_present == false)
            }
            update {}
            to Ready
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::Wait,
                consume_usage: false
            }
        }

        transition ResolveRealtimeMaterializeUser {
            on input ResolveRealtimeMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized == false
                && predecessor_materialized
                && item_skipped == false
                && item_ready
                && item_text_present
                && role == RealtimeTranscriptRoleKind::User
            }
            update {}
            to Ready
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::MaterializeUser,
                consume_usage: false
            }
        }

        transition ResolveRealtimeMaterializeAssistant {
            on input ResolveRealtimeMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized == false
                && predecessor_materialized
                && item_skipped == false
                && item_ready
                && item_text_present
                && role == RealtimeTranscriptRoleKind::Assistant
                && response_id_present
                && completion_present
            }
            update {}
            to Ready
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::MaterializeAssistant,
                consume_usage: completion_usage_consumed == false
            }
        }

        transition ResolveRealtimeMaterializeAssistantMissingCompletion {
            on input ResolveRealtimeMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized == false
                && predecessor_materialized
                && item_skipped == false
                && item_ready
                && item_text_present
                && role == RealtimeTranscriptRoleKind::Assistant
                && (response_id_present == false || completion_present == false)
            }
            update {}
            to Ready
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::Wait,
                consume_usage: false
            }
        }

        transition AuthorizeRestoreRealtimeTranscriptState {
            on input RestoreRealtimeTranscriptState {
                item_count,
                first_seen_count,
                first_seen_unique_count,
                every_item_has_order_entry,
                every_order_entry_has_item,
                all_materialized_predecessor_references_exist,
                no_self_predecessor_references,
                causal_graph_acyclic,
                all_materialized_items_have_materialized_ancestry,
                all_identity_fields_valid,
                all_user_content_identity_keys_match,
                all_user_content_identity_fields_valid,
                all_user_content_identity_item_ids_unique,
                all_user_content_identities_reference_materialized_user_items,
                all_user_content_tombstones_valid,
                user_content_identities_and_tombstones_disjoint,
                pending_user_content_blob_fields_valid,
                pending_user_content_blob_uncommitted,
                all_delta_ids_valid,
                all_completion_response_ids_valid,
                all_discarded_response_ids_valid,
                all_materialized_items_were_ready_or_skipped,
                all_assistant_items_have_response_unless_skipped,
                all_ready_assistant_items_have_completion_or_are_skipped,
                all_materialized_assistant_completions_consumed,
                all_completed_assistant_text_items_are_ready_or_materialized_or_skipped,
                all_discarded_assistant_items_are_skipped_or_materialized
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_count == first_seen_count
                && first_seen_count == first_seen_unique_count
                && every_item_has_order_entry
                && every_order_entry_has_item
                && all_materialized_predecessor_references_exist
                && no_self_predecessor_references
                && causal_graph_acyclic
                && all_materialized_items_have_materialized_ancestry
                && all_identity_fields_valid
                && all_user_content_identity_keys_match
                && all_user_content_identity_fields_valid
                && all_user_content_identity_item_ids_unique
                && all_user_content_identities_reference_materialized_user_items
                && all_user_content_tombstones_valid
                && user_content_identities_and_tombstones_disjoint
                && pending_user_content_blob_fields_valid
                && pending_user_content_blob_uncommitted
                && all_delta_ids_valid
                && all_completion_response_ids_valid
                && all_discarded_response_ids_valid
                && all_materialized_items_were_ready_or_skipped
                && all_assistant_items_have_response_unless_skipped
                && all_ready_assistant_items_have_completion_or_are_skipped
                && all_materialized_assistant_completions_consumed
                && all_completed_assistant_text_items_are_ready_or_materialized_or_skipped
                && all_discarded_assistant_items_are_skipped_or_materialized
            }
            update {}
            to Ready
            emit RealtimeTranscriptSnapshotRestoreAuthorized
        }

        // ===============================================================
        // Durable-config region (folded from the retired
        // SessionDurableConfigAuthorityMachine). Each transition reads only
        // the typed RAW observations carried on the input and resolves the
        // admission verdict. A request that fails the guard matches no
        // transition and surfaces to the shell as `Err` — exactly the
        // reject path the retired machine returned. The shell wrapper mirrors
        // the verdict (admit -> return the original typed value; reject ->
        // propagate the error) and decides nothing.
        // ===============================================================

        // ---------------------------------------------------------------
        // AuthorizeSessionMetadataPersist — admit a session-metadata persist
        // iff the record is well-formed enough to drive a session: a nonzero
        // schema version and a configured model. Ported verbatim from the
        // retired guard `schema_version > 0 && model_present`.
        // ---------------------------------------------------------------
        transition AuthorizeSessionMetadataPersist {
            on input AuthorizeSessionMetadataPersist {
                schema_version,
                model_present,
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && schema_version > 0
                && model_present == true
            }
            update {}
            to Ready
            emit SessionMetadataPersistAuthorized
        }

        // ---------------------------------------------------------------
        // AuthorizeSessionBuildStatePersist — admit a build-state persist iff
        // its mob-tool authority context is absent or is the generated
        // authority kind. Ported verbatim from the retired guard
        // `mob_tool_authority_context_present == false
        //  || mob_tool_authority_context_generated == true`.
        // ---------------------------------------------------------------
        transition AuthorizeSessionBuildStatePersist {
            on input AuthorizeSessionBuildStatePersist {
                mob_tool_authority_context_present,
                mob_tool_authority_context_generated,
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (
                    mob_tool_authority_context_present == false
                    || mob_tool_authority_context_generated == true
                )
            }
            update {}
            to Ready
            emit SessionBuildStatePersistAuthorized
        }

        // ---------------------------------------------------------------
        // RestoreSessionBuildState — the recovery half of the build-state
        // fact. Ported verbatim from the retired guard, which authorized any
        // persisted build-state snapshot (`Ready`-only guard).
        // ---------------------------------------------------------------
        transition RestoreSessionBuildState {
            on input RestoreSessionBuildState
            guard { self.lifecycle_phase == Phase::Ready }
            update {}
            to Ready
            emit SessionBuildStateRestoreAuthorized
        }

        // ---------------------------------------------------------------
        // AuthorizeSystemPromptMutation — admit a system-prompt mutation iff
        // the prompt has content or is an explicit clear (zero bytes). Ported
        // verbatim from the retired guard
        // `prompt_present == true || prompt_byte_count == 0`.
        // ---------------------------------------------------------------
        transition AuthorizeSystemPromptMutation {
            on input AuthorizeSystemPromptMutation {
                source,
                prompt_present,
                prompt_byte_count,
                replacing_existing,
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (prompt_present == true || prompt_byte_count == 0)
            }
            update {}
            to Ready
            emit SystemPromptMutationAuthorized
        }

        // ===============================================================
        // Pending-continuation region (folded from the retired
        // PendingContinuationAdmissionMachine). Both transitions read only the
        // typed RAW observations carried on the input and resolve the
        // disposition via `has_effective_pending_boundary`. The shell mirrors
        // the emitted disposition (and the public terminal witness) onto its
        // run-pending / start-turn-disposition path and decides nothing.
        // ===============================================================

        transition ResolvePendingContinuationWithBoundary {
            on input ResolvePendingContinuation {
                session_tail,
                staged_tool_result_count
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && has_effective_pending_boundary(session_tail, staged_tool_result_count)
            }
            update {}
            to Ready
            emit PendingContinuationResolved {
                disposition: PendingContinuationDisposition::RunPending
            }
        }

        transition ResolvePendingContinuationWithoutBoundary {
            on input ResolvePendingContinuation {
                session_tail,
                staged_tool_result_count
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && has_effective_pending_boundary(session_tail, staged_tool_result_count) == false
            }
            update {}
            to Ready
            emit PendingContinuationResolved {
                disposition: PendingContinuationDisposition::NoPendingBoundary
            }
            emit PendingContinuationPublicTerminalResolved {
                terminal: PendingContinuationPublicTerminal::NoPendingBoundary
            }
        }

        // ===============================================================
        // Resume-override-admission region. Reject transitions are guarded in
        // the shell's first-match-wins precedence order: each lower-priority
        // reject only fires when every higher-priority reject condition is
        // false. The three accept transitions split on the provider selection.
        // ===============================================================

        // Reject (priority 1): provider override without a model override.
        transition AuthorizeSessionResumeOverridesRejectProviderRequiresModel {
            on input AuthorizeSessionResumeOverrides {
                provider_override_present,
                model_override_present,
                self_hosted_server_override_present,
                has_build_only_overrides,
                first_turn_phase
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && resume_reject_provider_requires_model(
                    provider_override_present,
                    model_override_present
                )
            }
            update {}
            to Ready
            emit SessionResumeOverridesRejected {
                reason: ResumeOverrideRejection::ProviderRequiresModel
            }
        }

        // Reject (priority 2): build-only overrides after the first turn started.
        transition AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn {
            on input AuthorizeSessionResumeOverrides {
                provider_override_present,
                model_override_present,
                self_hosted_server_override_present,
                has_build_only_overrides,
                first_turn_phase
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && resume_reject_provider_requires_model(
                    provider_override_present,
                    model_override_present
                ) == false
                && resume_reject_build_only_after_first_turn(
                    has_build_only_overrides,
                    first_turn_phase
                )
            }
            update {}
            to Ready
            emit SessionResumeOverridesRejected {
                reason: ResumeOverrideRejection::BuildOnlyAfterFirstTurn
            }
        }

        // Accept (provider recomputed from a model-only change) without an
        // exact server override: clears stored provider + self-hosted binding;
        // provider_overridden is true.
        transition AuthorizeSessionResumeOverridesAcceptRecomputeProvider {
            on input AuthorizeSessionResumeOverrides {
                provider_override_present,
                model_override_present,
                self_hosted_server_override_present,
                has_build_only_overrides,
                first_turn_phase
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && resume_overrides_admissible(
                    provider_override_present,
                    model_override_present,
                    has_build_only_overrides,
                    first_turn_phase
                )
                && resume_provider_recompute_from_model(
                    model_override_present,
                    provider_override_present
                )
                && self_hosted_server_override_present == false
            }
            update {}
            to Ready
            emit SessionResumeOverridesAuthorized {
                provider_selection: ResumeProviderSelection::RecomputeFromModel,
                self_hosted_selection: ResumeSelfHostedSelection::Clear,
                provider_overridden: true
            }
        }

        // Accept a model-only provider recompute while preserving the caller's
        // exact self-hosted server route for downstream registry validation.
        transition AuthorizeSessionResumeOverridesAcceptRecomputeProviderWithSelfHostedOverride {
            on input AuthorizeSessionResumeOverrides {
                provider_override_present,
                model_override_present,
                self_hosted_server_override_present,
                has_build_only_overrides,
                first_turn_phase
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && resume_overrides_admissible(
                    provider_override_present,
                    model_override_present,
                    has_build_only_overrides,
                    first_turn_phase
                )
                && resume_provider_recompute_from_model(
                    model_override_present,
                    provider_override_present
                )
                && self_hosted_server_override_present
            }
            update {}
            to Ready
            emit SessionResumeOverridesAuthorized {
                provider_selection: ResumeProviderSelection::RecomputeFromModel,
                self_hosted_selection: ResumeSelfHostedSelection::UseOverride,
                provider_overridden: true
            }
        }

        // Accept (explicit provider override): use the override; self-hosted is
        // cleared because a provider override always rides a model override;
        // provider_overridden is true.
        transition AuthorizeSessionResumeOverridesAcceptUseOverride {
            on input AuthorizeSessionResumeOverrides {
                provider_override_present,
                model_override_present,
                self_hosted_server_override_present,
                has_build_only_overrides,
                first_turn_phase
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && resume_overrides_admissible(
                    provider_override_present,
                    model_override_present,
                    has_build_only_overrides,
                    first_turn_phase
                )
                && resume_provider_recompute_from_model(
                    model_override_present,
                    provider_override_present
                ) == false
                && provider_override_present
                && self_hosted_server_override_present == false
            }
            update {}
            to Ready
            emit SessionResumeOverridesAuthorized {
                provider_selection: ResumeProviderSelection::UseOverride,
                self_hosted_selection: ResumeSelfHostedSelection::Clear,
                provider_overridden: true
            }
        }

        // Accept an explicit provider/model pair together with an exact
        // self-hosted server route for downstream registry validation.
        transition AuthorizeSessionResumeOverridesAcceptUseOverrideWithSelfHostedOverride {
            on input AuthorizeSessionResumeOverrides {
                provider_override_present,
                model_override_present,
                self_hosted_server_override_present,
                has_build_only_overrides,
                first_turn_phase
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && resume_overrides_admissible(
                    provider_override_present,
                    model_override_present,
                    has_build_only_overrides,
                    first_turn_phase
                )
                && resume_provider_recompute_from_model(
                    model_override_present,
                    provider_override_present
                ) == false
                && provider_override_present
                && self_hosted_server_override_present
            }
            update {}
            to Ready
            emit SessionResumeOverridesAuthorized {
                provider_selection: ResumeProviderSelection::UseOverride,
                self_hosted_selection: ResumeSelfHostedSelection::UseOverride,
                provider_overridden: true
            }
        }

        // Accept (retain stored provider): no provider override and not a
        // model-only recompute. self-hosted retained iff the model is unchanged;
        // provider_overridden iff the model changed.
        transition AuthorizeSessionResumeOverridesAcceptRetainStored {
            on input AuthorizeSessionResumeOverrides {
                provider_override_present,
                model_override_present,
                self_hosted_server_override_present,
                has_build_only_overrides,
                first_turn_phase
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && resume_overrides_admissible(
                    provider_override_present,
                    model_override_present,
                    has_build_only_overrides,
                    first_turn_phase
                )
                && resume_provider_recompute_from_model(
                    model_override_present,
                    provider_override_present
                ) == false
                && provider_override_present == false
                && self_hosted_server_override_present == false
            }
            update {}
            to Ready
            emit SessionResumeOverridesAuthorized {
                provider_selection: ResumeProviderSelection::UseStored,
                self_hosted_selection: ResumeSelfHostedSelection::Retain,
                provider_overridden: false
            }
        }

        // Accept a route-only recovery override (or any retained-provider
        // recovery carrying one) and select the exact caller route.
        transition AuthorizeSessionResumeOverridesAcceptRetainStoredWithSelfHostedOverride {
            on input AuthorizeSessionResumeOverrides {
                provider_override_present,
                model_override_present,
                self_hosted_server_override_present,
                has_build_only_overrides,
                first_turn_phase
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && resume_overrides_admissible(
                    provider_override_present,
                    model_override_present,
                    has_build_only_overrides,
                    first_turn_phase
                )
                && resume_provider_recompute_from_model(
                    model_override_present,
                    provider_override_present
                ) == false
                && provider_override_present == false
                && self_hosted_server_override_present
            }
            update {}
            to Ready
            emit SessionResumeOverridesAuthorized {
                provider_selection: ResumeProviderSelection::UseStored,
                self_hosted_selection: ResumeSelfHostedSelection::UseOverride,
                provider_overridden: false
            }
        }

        // ---------------------------------------------------------------
        // ClassifyLiveSessionAuthority — live-vs-durable session-document
        // authority reconciliation. The session-store shell extracts four pure
        // boolean divergence observations; THIS machine owns the verdict, the
        // precedence (archived > uncommitted transcript > runtime system-context
        // > stored transcript-revision), and the typed reason.
        //
        //   all four false                       -> LiveAuthoritative
        //   stored_is_archived                   -> Durable / StoredArchived
        //   live_has_uncommitted_transcript      -> Durable / LiveUncommittedTranscript
        //   runtime_system_context_diverged      -> Durable / RuntimeSystemContextDiverged
        //   else (stored_transcript_diverged)    -> Durable / StoredTranscriptRevisionDiverged
        //
        // The four Durable guards are mutually exclusive and, with the Live
        // guard, total over the boolean cube. Stateless self-loop in Ready.
        // ---------------------------------------------------------------
        transition ClassifyLiveSessionAuthorityLive {
            on input ClassifyLiveSessionAuthority {
                stored_transcript_diverged,
                live_has_uncommitted_transcript,
                runtime_system_context_diverged,
                stored_is_archived
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && stored_transcript_diverged == false
                && live_has_uncommitted_transcript == false
                && runtime_system_context_diverged == false
                && stored_is_archived == false
            }
            update {}
            to Ready
            emit LiveSessionAuthorityClassified {
                authority: LiveSessionAuthorityKind::LiveAuthoritative,
                reason: LiveSessionAuthorityReason::StoredArchived
            }
        }

        transition ClassifyLiveSessionAuthorityDurableArchived {
            on input ClassifyLiveSessionAuthority {
                stored_transcript_diverged,
                live_has_uncommitted_transcript,
                runtime_system_context_diverged,
                stored_is_archived
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && stored_is_archived == true
            }
            update {}
            to Ready
            emit LiveSessionAuthorityClassified {
                authority: LiveSessionAuthorityKind::DurableAuthoritative,
                reason: LiveSessionAuthorityReason::StoredArchived
            }
        }

        transition ClassifyLiveSessionAuthorityDurableUncommitted {
            on input ClassifyLiveSessionAuthority {
                stored_transcript_diverged,
                live_has_uncommitted_transcript,
                runtime_system_context_diverged,
                stored_is_archived
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && stored_is_archived == false
                && live_has_uncommitted_transcript == true
            }
            update {}
            to Ready
            emit LiveSessionAuthorityClassified {
                authority: LiveSessionAuthorityKind::DurableAuthoritative,
                reason: LiveSessionAuthorityReason::LiveUncommittedTranscript
            }
        }

        transition ClassifyLiveSessionAuthorityDurableSystemContext {
            on input ClassifyLiveSessionAuthority {
                stored_transcript_diverged,
                live_has_uncommitted_transcript,
                runtime_system_context_diverged,
                stored_is_archived
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && stored_is_archived == false
                && live_has_uncommitted_transcript == false
                && runtime_system_context_diverged == true
            }
            update {}
            to Ready
            emit LiveSessionAuthorityClassified {
                authority: LiveSessionAuthorityKind::DurableAuthoritative,
                reason: LiveSessionAuthorityReason::RuntimeSystemContextDiverged
            }
        }

        transition ClassifyLiveSessionAuthorityDurableRevision {
            on input ClassifyLiveSessionAuthority {
                stored_transcript_diverged,
                live_has_uncommitted_transcript,
                runtime_system_context_diverged,
                stored_is_archived
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && stored_is_archived == false
                && live_has_uncommitted_transcript == false
                && runtime_system_context_diverged == false
                && stored_transcript_diverged == true
            }
            update {}
            to Ready
            emit LiveSessionAuthorityClassified {
                authority: LiveSessionAuthorityKind::DurableAuthoritative,
                reason: LiveSessionAuthorityReason::StoredTranscriptRevisionDiverged
            }
        }

        // ===============================================================
        // Recovery-source-projection region (KEYSTONE). Both transitions read
        // typed store/runtime observations and resolve the recoverable
        // verdict via `store_projection_can_recover_authority`. The verdict is
        // total over the boolean cube, so it is emitted on both branches; the
        // shell mirrors `recoverable` onto its load fallback and decides
        // nothing. Fails closed.
        // ===============================================================

        transition RecoverSessionFromStoreAuthorized {
            on input RecoverSessionFromStore {
                session_id,
                has_metadata,
                has_build_state,
                runtime_projection_quarantined
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && store_projection_can_recover_authority(
                    has_metadata,
                    has_build_state,
                    runtime_projection_quarantined
                )
            }
            update {}
            to Ready
            emit SessionStoreRecoverySourceResolved { recoverable: true }
        }

        transition RecoverSessionFromStoreUnrecoverable {
            on input RecoverSessionFromStore {
                session_id,
                has_metadata,
                has_build_state,
                runtime_projection_quarantined
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && store_projection_can_recover_authority(
                    has_metadata,
                    has_build_state,
                    runtime_projection_quarantined
                ) == false
            }
            update {}
            to Ready
            emit SessionStoreRecoverySourceResolved { recoverable: false }
        }

        // ===============================================================
        // Runtime-snapshot read-source region. Both transitions read the
        // pure observations (the store head provably extends the runtime
        // snapshot; the head row carries the intra-turn checkpointer's
        // provenance stamp; the session is live in-process) and resolve the
        // authoritative load source. Total over the observation cube,
        // emitted on both branches; the shell mirrors `read_from_store_head`
        // and decides nothing. A stale-strict-prefix snapshot loading
        // unreconciled on a COLD resume is the permanent-save-rejection
        // wedge (append-only guard vs frozen snapshot); a
        // checkpointer-stamped ahead row is uncommitted intra-turn residue
        // and must NOT be served (the rollback region converges it at save
        // time); a live session's snapshot lag is transient and the live
        // runtime recommits past it.
        // ===============================================================

        // A COMMITTED descendant is ordinary authority — independently of
        // local actor liveness. A live actor observing a committed strict
        // descendant is a superseded writer (another boundary, or an archive
        // commit whose retirement failed, advanced past its snapshot); its
        // stale snapshot must not mask committed truth, and its own next
        // boundary preflight fails closed against the newer base. The live
        // exception below is valid only for UNCOMMITTED intra-turn rows.
        transition ResolveRuntimeSnapshotReadSourceCommittedHead {
            on input ResolveRuntimeSnapshotReadSource {
                session_id,
                relation,
                store_provenance,
                session_is_live,
                tail_execution,
                head_stamp_era
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && relation == DurableHeadRelation::VerifiedStrictDescendant
                && store_provenance == CheckpointProvenanceClass::Committed
            }
            update {}
            to Ready
            emit RuntimeSnapshotReadSourceResolved {
                disposition: RuntimeSnapshotReadDisposition::UseCommittedStoreHead
            }
        }

        // An INTRA-TURN descendant on a cold session is real durable content
        // whose boundary commit never landed. It is neither servable as
        // committed authority nor discardable: recovery owns it.
        transition ResolveRuntimeSnapshotReadSourceRecoveryRequired {
            on input ResolveRuntimeSnapshotReadSource {
                session_id,
                relation,
                store_provenance,
                session_is_live,
                tail_execution,
                head_stamp_era
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && relation == DurableHeadRelation::VerifiedStrictDescendant
                && store_provenance != CheckpointProvenanceClass::Committed
                && session_is_live == false
                && tail_execution == DurableTailExecutionEvidence::BoundExecution
            }
            update {}
            to Ready
            emit RuntimeSnapshotReadSourceResolved {
                disposition: RuntimeSnapshotReadDisposition::RecoveryRequired
            }
        }

        // A COLD intra-turn descendant whose assistant tail carries NO run
        // identity, on a head row whose VERIFIED stamp predates the
        // witness-v3 era, is the legacy lost-boundary shape: run-identity
        // bookkeeping did not exist when the tail was written, so no run id
        // can ever appear and no reconciliation verb can promote it — the
        // quarantine below would be a permanent availability loss on bytes
        // already digest-proven to be exact continuations. Recovery owns it:
        // the classifier applies the full legacy gate (clean EndTurn shape,
        // NoRunId cardinality, legacy stamp era) and anything less stays
        // held. Intra-turn provenance is required explicitly — only the
        // in-run checkpointer mints it, which is what makes the missing run
        // identity evidence of writer era rather than of writer path.
        transition ResolveRuntimeSnapshotReadSourceLegacyRecoveryRequired {
            on input ResolveRuntimeSnapshotReadSource {
                session_id,
                relation,
                store_provenance,
                session_is_live,
                tail_execution,
                head_stamp_era
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && relation == DurableHeadRelation::VerifiedStrictDescendant
                && store_provenance == CheckpointProvenanceClass::IntraTurn
                && session_is_live == false
                && tail_execution == DurableTailExecutionEvidence::UnboundExecution
                && head_stamp_era == DurableHeadStampEra::PreWitnessV3
            }
            update {}
            to Ready
            emit RuntimeSnapshotReadSourceResolved {
                disposition: RuntimeSnapshotReadDisposition::RecoveryRequired
            }
        }

        // A COMMITTED (or unstamped) fork, or unverifiable evidence, is
        // contradictory: retain intact, refuse to serve.
        transition ResolveRuntimeSnapshotReadSourceQuarantine {
            on input ResolveRuntimeSnapshotReadSource {
                session_id,
                relation,
                store_provenance,
                session_is_live,
                tail_execution,
                head_stamp_era
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && ((relation == DurableHeadRelation::Diverged
                        && store_provenance != CheckpointProvenanceClass::IntraTurn)
                    || relation == DurableHeadRelation::Unverifiable
                    // A COLD uncommitted descendant whose assistant content
                    // carries no run identity records execution that no
                    // recovery boundary can be anchored to and that the input
                    // lifecycle will not redeliver. Serving past it would
                    // invite a later projection rebuild to discard it. The
                    // one carve-out is the legacy shape (intra-turn row whose
                    // verified stamp predates witness-v3): recovery owns
                    // that, in the arm above.
                    || (relation == DurableHeadRelation::VerifiedStrictDescendant
                        && store_provenance != CheckpointProvenanceClass::Committed
                        && session_is_live == false
                        && tail_execution
                            == DurableTailExecutionEvidence::UnboundExecution
                        && (store_provenance != CheckpointProvenanceClass::IntraTurn
                            || head_stamp_era != DurableHeadStampEra::PreWitnessV3)))
            }
            update {}
            to Ready
            emit RuntimeSnapshotReadSourceResolved {
                disposition: RuntimeSnapshotReadDisposition::Quarantine
            }
        }

        // Everything else serves the committed runtime snapshot: an exact or
        // behind row, a live session whose ahead-row is UNCOMMITTED (the live
        // runtime's own intra-turn residue; its snapshot lag is transient and
        // it recommits past the row), or an INTRA-TURN sibling
        // that diverges from committed authority — the checkpointer's
        // projection of a turn the boundary resolved differently (e.g. an
        // evicted/cancelled live turn). The committed child is the verified
        // authority; the sibling row is RETAINED (saves over it stay
        // fail-closed and loud) but must not outrank committed truth or
        // quarantine the session.
        transition ResolveRuntimeSnapshotReadSourceSnapshot {
            on input ResolveRuntimeSnapshotReadSource {
                session_id,
                relation,
                store_provenance,
                session_is_live,
                tail_execution,
                head_stamp_era
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && relation != DurableHeadRelation::Unverifiable
                && (relation != DurableHeadRelation::Diverged
                    || store_provenance == CheckpointProvenanceClass::IntraTurn)
                && (relation != DurableHeadRelation::VerifiedStrictDescendant
                    || (store_provenance != CheckpointProvenanceClass::Committed
                        && (session_is_live == true
                            || tail_execution
                                == DurableTailExecutionEvidence::NoExecutionContent)))
            }
            update {}
            to Ready
            emit RuntimeSnapshotReadSourceResolved {
                disposition: RuntimeSnapshotReadDisposition::UseRuntimeSnapshot
            }
        }

        // ===============================================================
        // Durable-tail classification region. Total and disjoint over the
        // observation:
        //   Completed:  verified descendant, single run, EndTurn, no
        //               dangling calls, no orphan results, nothing after
        //               the terminal.
        //   Repairable: verified descendant, single run, coherent (no
        //               orphans, nothing after the terminal), NO dangling
        //               calls, stopped at ToolUse or with no recorded
        //               terminal. A dangling tool_use proves intent, not
        //               execution: its external side effect may have fired
        //               before the crash, so no tail carrying one may be
        //               auto-closed and resumed.
        //   Legacy:     verified descendant, ZERO run identity, EndTurn,
        //               no dangling calls, no orphan results, nothing
        //               after the terminal, AND pre-witness-v3 stamp
        //               evidence on the head row. A pre-run-identity
        //               writer wrote this tail; no run id can ever appear,
        //               so it is adopted (never held for an identity that
        //               cannot exist) under a domain-separated legacy run
        //               identity.
        //   Ambiguous:  everything else (explicit negation of the above —
        //               non-descendant relations, zero-without-legacy-
        //               evidence or multiple runs, orphan results, content
        //               after the terminal, Other stop shapes, ANY
        //               dangling tool_use call, unclean legacy shapes).
        // No class authorizes discarding; Ambiguous is held intact. For a
        // dangling call that means held for reconciliation: readable, never
        // executed against, until an idempotency/reconciliation witness or
        // an operator verb clears it.
        // ===============================================================

        transition ClassifyDurableTailCompleted {
            on input ClassifyDurableTail {
                session_id,
                candidate_id,
                relation,
                run_id_cardinality,
                terminal_stop_reason,
                dangling_tool_use_count,
                orphan_tool_result_count,
                messages_after_terminal,
                head_stamp_era
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && relation == DurableHeadRelation::VerifiedStrictDescendant
                && run_id_cardinality == RunIdCardinality::SingleRunId
                && terminal_stop_reason == DurableTailStopReason::EndTurn
                && dangling_tool_use_count == 0
                && orphan_tool_result_count == 0
                && messages_after_terminal == false
            }
            update {}
            to Ready
            emit DurableTailClassified {
                candidate_id: candidate_id,
                class: DurableTailRecoveryClass::CompletedCandidate
            }
        }

        // Legacy adoption: a digest-proven byte-exact continuation written by
        // a pre-run-identity writer. Four independent evidence axes must
        // agree: zero run identity anywhere in the tail (every in-run
        // assistant append since v0.7.12 persists its run id inside the same
        // message bytes as the content, so a modern in-run tail cannot lack
        // one), pre-witness-v3 stamp evidence on the head row (a modern mint
        // over graph-bearing authority always advertises schema 3), the
        // clean COMPLETED shape, and the verified strict-descendant
        // relation. Anything less — an interrupted legacy tail, a
        // tool-racing shape, modern stamp evidence — stays Ambiguous and
        // held exactly as before.
        transition ClassifyDurableTailLegacyCompleted {
            on input ClassifyDurableTail {
                session_id,
                candidate_id,
                relation,
                run_id_cardinality,
                terminal_stop_reason,
                dangling_tool_use_count,
                orphan_tool_result_count,
                messages_after_terminal,
                head_stamp_era
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && relation == DurableHeadRelation::VerifiedStrictDescendant
                && run_id_cardinality == RunIdCardinality::NoRunId
                && terminal_stop_reason == DurableTailStopReason::EndTurn
                && dangling_tool_use_count == 0
                && orphan_tool_result_count == 0
                && messages_after_terminal == false
                && head_stamp_era == DurableHeadStampEra::PreWitnessV3
            }
            update {}
            to Ready
            emit DurableTailClassified {
                candidate_id: candidate_id,
                class: DurableTailRecoveryClass::LegacyCompletedCandidate
            }
        }

        transition ClassifyDurableTailRepairable {
            on input ClassifyDurableTail {
                session_id,
                candidate_id,
                relation,
                run_id_cardinality,
                terminal_stop_reason,
                dangling_tool_use_count,
                orphan_tool_result_count,
                messages_after_terminal,
                head_stamp_era
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && relation == DurableHeadRelation::VerifiedStrictDescendant
                && run_id_cardinality == RunIdCardinality::SingleRunId
                && dangling_tool_use_count == 0
                && orphan_tool_result_count == 0
                && messages_after_terminal == false
                && (terminal_stop_reason == DurableTailStopReason::ToolUse
                    || terminal_stop_reason == DurableTailStopReason::Absent)
            }
            update {}
            to Ready
            emit DurableTailClassified {
                candidate_id: candidate_id,
                class: DurableTailRecoveryClass::InterruptedRepairableCandidate
            }
        }

        transition ClassifyDurableTailAmbiguous {
            on input ClassifyDurableTail {
                session_id,
                candidate_id,
                relation,
                run_id_cardinality,
                terminal_stop_reason,
                dangling_tool_use_count,
                orphan_tool_result_count,
                messages_after_terminal,
                head_stamp_era
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (relation != DurableHeadRelation::VerifiedStrictDescendant
                    || run_id_cardinality != RunIdCardinality::SingleRunId
                    || orphan_tool_result_count != 0
                    || messages_after_terminal == true
                    || terminal_stop_reason == DurableTailStopReason::Other
                    || dangling_tool_use_count != 0)
                // Exact complement of the legacy adoption arm: an
                // identity-less tail stays Ambiguous unless EVERY legacy
                // conjunct holds.
                && (relation != DurableHeadRelation::VerifiedStrictDescendant
                    || run_id_cardinality != RunIdCardinality::NoRunId
                    || terminal_stop_reason != DurableTailStopReason::EndTurn
                    || dangling_tool_use_count != 0
                    || orphan_tool_result_count != 0
                    || messages_after_terminal == true
                    || head_stamp_era != DurableHeadStampEra::PreWitnessV3)
            }
            update {}
            to Ready
            emit DurableTailClassified {
                candidate_id: candidate_id,
                class: DurableTailRecoveryClass::Ambiguous
            }
        }

        // ===============================================================
        // Runtime-projection-rollback region. These transitions read the
        // pure continuation observation and resolve the disposition of a
        // runtime-authoritative projection save whose durable row ran ahead
        // of the authority transcript. Total over the observation: a row
        // that faithfully continues the authority (its tail is turn content
        // whose boundary commit never landed) is retained for recovery; an
        // INTRA-TURN row the authority provably superseded is rebuilt onto
        // committed truth; anything else — including every COMMITTED row —
        // keeps the fail-closed rejection. The shell mirrors the
        // disposition.
        // ===============================================================

        transition ResolveRuntimeProjectionConflictRetain {
            on input ResolveRuntimeProjectionConflict {
                session_id,
                relation,
                row_provenance,
                authority_supersedes_row
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && relation == DurableHeadRelation::VerifiedStrictDescendant
            }
            update {}
            to Ready
            emit RuntimeProjectionConflictResolved {
                disposition: RuntimeProjectionConflictDisposition::RetainForRecovery
            }
        }

        // The committed authority already passed the row's revision AND the
        // row carries the checkpointer's own INTRA-TURN provenance stamp:
        // the row is its own run's superseded intermediate projection, and
        // the wedge invariant (a lost tail fails every later boundary
        // preflight) proves no lost content can reach this arm. A verified
        // strict descendant NEVER reaches here — authority behind the row
        // excludes supersession by construction. A COMMITTED row never
        // converges: revision ordering does not prove ancestry between two
        // committed run boundaries, so committed siblings stay divergent
        // and fail closed on the Reject arm below.
        transition ResolveRuntimeProjectionConflictConverge {
            on input ResolveRuntimeProjectionConflict {
                session_id,
                relation,
                row_provenance,
                authority_supersedes_row
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && relation != DurableHeadRelation::VerifiedStrictDescendant
                && row_provenance == CheckpointProvenanceClass::IntraTurn
                && authority_supersedes_row == true
            }
            update {}
            to Ready
            emit RuntimeProjectionConflictResolved {
                disposition: RuntimeProjectionConflictDisposition::ConvergeSupersededProjection
            }
        }

        transition ResolveRuntimeProjectionConflictReject {
            on input ResolveRuntimeProjectionConflict {
                session_id,
                relation,
                row_provenance,
                authority_supersedes_row
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && relation != DurableHeadRelation::VerifiedStrictDescendant
                && (authority_supersedes_row == false
                    || row_provenance != CheckpointProvenanceClass::IntraTurn)
            }
            update {}
            to Ready
            emit RuntimeProjectionConflictResolved {
                disposition: RuntimeProjectionConflictDisposition::RejectDivergent
            }
        }

        // ===============================================================
        // Runtime-checkpoint projection region. Total over the canonical
        // lifecycle-terminal state: Active projects, while Archived is an
        // absorbing no-op even when a delayed teardown still holds valid
        // committed runtime checkpoint bytes.
        // ===============================================================

        transition ResolveRuntimeCheckpointProjectionActive {
            on input ResolveRuntimeCheckpointProjection { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_lifecycle_terminal.get_cloned(session_id).get("value")
                    == SessionDocumentLifecycle::Active
            }
            update {}
            to Ready
            emit RuntimeCheckpointProjectionResolved {
                disposition: RuntimeCheckpointProjectionDisposition::Project
            }
        }

        transition ResolveRuntimeCheckpointProjectionArchived {
            on input ResolveRuntimeCheckpointProjection { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_lifecycle_terminal.get_cloned(session_id).get("value")
                    == SessionDocumentLifecycle::Archived
            }
            update {}
            to Ready
            emit RuntimeCheckpointProjectionResolved {
                disposition: RuntimeCheckpointProjectionDisposition::IgnoreArchived
            }
        }

        // ===============================================================
        // Legacy-checkpoint recovery-migration region. Total over the legal
        // observation shapes the shell can emit: the runtime snapshot copy
        // is legacy (with the projection absent, legacy-related by prefix,
        // already typed — the partial-adoption rebuild — or typed with the
        // legacy snapshot related by prefix — the sanctioned-adoption
        // convergence), or the runtime snapshot is absent and the
        // session-store row is the sole legacy copy. Content custody is
        // lifecycle-independent: migration stamps the exact observed
        // conversation and never alters the document's lifecycle terminal.
        // ===============================================================

        transition ResolveLegacyCheckpointMigrationSnapshotIdenticalProjection {
            on input ResolveLegacyCheckpointMigration {
                session_id,
                runtime_snapshot_present,
                runtime_snapshot_legacy,
                store_row_present,
                store_row_legacy,
                transcript_relation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && runtime_snapshot_present == true
                && runtime_snapshot_legacy == true
                && store_row_present == true
                && store_row_legacy == true
                && transcript_relation == LegacyCheckpointTranscriptRelation::Identical
            }
            update {}
            to Ready
            emit LegacyCheckpointMigrationResolved {
                disposition: LegacyCheckpointMigrationDisposition::MigrateCanonicalSnapshot
            }
        }

        transition ResolveLegacyCheckpointMigrationSnapshotAheadOfProjection {
            on input ResolveLegacyCheckpointMigration {
                session_id,
                runtime_snapshot_present,
                runtime_snapshot_legacy,
                store_row_present,
                store_row_legacy,
                transcript_relation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && runtime_snapshot_present == true
                && runtime_snapshot_legacy == true
                && store_row_present == true
                && store_row_legacy == true
                && transcript_relation == LegacyCheckpointTranscriptRelation::SnapshotExtendsProjection
            }
            update {}
            to Ready
            emit LegacyCheckpointMigrationResolved {
                disposition: LegacyCheckpointMigrationDisposition::MigrateCanonicalSnapshot
            }
        }

        transition ResolveLegacyCheckpointMigrationProjectionExtension {
            on input ResolveLegacyCheckpointMigration {
                session_id,
                runtime_snapshot_present,
                runtime_snapshot_legacy,
                store_row_present,
                store_row_legacy,
                transcript_relation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && runtime_snapshot_present == true
                && runtime_snapshot_legacy == true
                && store_row_present == true
                && store_row_legacy == true
                && transcript_relation == LegacyCheckpointTranscriptRelation::ProjectionExtendsSnapshot
            }
            update {}
            to Ready
            emit LegacyCheckpointMigrationResolved {
                disposition: LegacyCheckpointMigrationDisposition::AdoptProjectionExtension
            }
        }

        transition ResolveLegacyCheckpointMigrationDivergentCopies {
            on input ResolveLegacyCheckpointMigration {
                session_id,
                runtime_snapshot_present,
                runtime_snapshot_legacy,
                store_row_present,
                store_row_legacy,
                transcript_relation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && runtime_snapshot_present == true
                && runtime_snapshot_legacy == true
                && store_row_present == true
                && store_row_legacy == true
                && transcript_relation == LegacyCheckpointTranscriptRelation::Divergent
            }
            update {}
            to Ready
            emit LegacyCheckpointMigrationResolved {
                disposition: LegacyCheckpointMigrationDisposition::RefuseDivergent
            }
        }

        transition ResolveLegacyCheckpointMigrationSnapshotOnly {
            on input ResolveLegacyCheckpointMigration {
                session_id,
                runtime_snapshot_present,
                runtime_snapshot_legacy,
                store_row_present,
                store_row_legacy,
                transcript_relation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && runtime_snapshot_present == true
                && runtime_snapshot_legacy == true
                && store_row_present == false
            }
            update {}
            to Ready
            emit LegacyCheckpointMigrationResolved {
                disposition: LegacyCheckpointMigrationDisposition::MigrateCanonicalSnapshot
            }
        }

        transition ResolveLegacyCheckpointMigrationStoreRowOnly {
            on input ResolveLegacyCheckpointMigration {
                session_id,
                runtime_snapshot_present,
                runtime_snapshot_legacy,
                store_row_present,
                store_row_legacy,
                transcript_relation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && runtime_snapshot_present == false
                && store_row_present == true
                && store_row_legacy == true
            }
            update {}
            to Ready
            emit LegacyCheckpointMigrationResolved {
                disposition: LegacyCheckpointMigrationDisposition::MigrateStoreProjection
            }
        }

        transition ResolveLegacyCheckpointMigrationTypedSnapshotLegacyProjection {
            on input ResolveLegacyCheckpointMigration {
                session_id,
                runtime_snapshot_present,
                runtime_snapshot_legacy,
                store_row_present,
                store_row_legacy,
                transcript_relation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && runtime_snapshot_present == true
                && runtime_snapshot_legacy == false
                && store_row_present == true
                && store_row_legacy == true
            }
            update {}
            to Ready
            emit LegacyCheckpointMigrationResolved {
                disposition: LegacyCheckpointMigrationDisposition::RebuildProjectionFromTypedSnapshot
            }
        }

        transition ResolveLegacyCheckpointMigrationSnapshotIdenticalTypedProjection {
            on input ResolveLegacyCheckpointMigration {
                session_id,
                runtime_snapshot_present,
                runtime_snapshot_legacy,
                store_row_present,
                store_row_legacy,
                transcript_relation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && runtime_snapshot_present == true
                && runtime_snapshot_legacy == true
                && store_row_present == true
                && store_row_legacy == false
                && transcript_relation == LegacyCheckpointTranscriptRelation::Identical
            }
            update {}
            to Ready
            emit LegacyCheckpointMigrationResolved {
                disposition: LegacyCheckpointMigrationDisposition::ConvergeSnapshotOntoTypedProjection
            }
        }

        transition ResolveLegacyCheckpointMigrationTypedProjectionExtension {
            on input ResolveLegacyCheckpointMigration {
                session_id,
                runtime_snapshot_present,
                runtime_snapshot_legacy,
                store_row_present,
                store_row_legacy,
                transcript_relation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && runtime_snapshot_present == true
                && runtime_snapshot_legacy == true
                && store_row_present == true
                && store_row_legacy == false
                && transcript_relation == LegacyCheckpointTranscriptRelation::ProjectionExtendsSnapshot
            }
            update {}
            to Ready
            emit LegacyCheckpointMigrationResolved {
                disposition: LegacyCheckpointMigrationDisposition::ConvergeSnapshotOntoTypedProjection
            }
        }

        transition ResolveLegacyCheckpointMigrationSnapshotAheadOfTypedProjection {
            on input ResolveLegacyCheckpointMigration {
                session_id,
                runtime_snapshot_present,
                runtime_snapshot_legacy,
                store_row_present,
                store_row_legacy,
                transcript_relation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && runtime_snapshot_present == true
                && runtime_snapshot_legacy == true
                && store_row_present == true
                && store_row_legacy == false
                && transcript_relation == LegacyCheckpointTranscriptRelation::SnapshotExtendsProjection
            }
            update {}
            to Ready
            emit LegacyCheckpointMigrationResolved {
                disposition: LegacyCheckpointMigrationDisposition::RefuseDivergent
            }
        }

        transition ResolveLegacyCheckpointMigrationDivergentFromTypedProjection {
            on input ResolveLegacyCheckpointMigration {
                session_id,
                runtime_snapshot_present,
                runtime_snapshot_legacy,
                store_row_present,
                store_row_legacy,
                transcript_relation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && runtime_snapshot_present == true
                && runtime_snapshot_legacy == true
                && store_row_present == true
                && store_row_legacy == false
                && transcript_relation == LegacyCheckpointTranscriptRelation::Divergent
            }
            update {}
            to Ready
            emit LegacyCheckpointMigrationResolved {
                disposition: LegacyCheckpointMigrationDisposition::RefuseDivergent
            }
        }

        transition ResolveLegacyCheckpointMigrationTypedProjectionNotComparable {
            on input ResolveLegacyCheckpointMigration {
                session_id,
                runtime_snapshot_present,
                runtime_snapshot_legacy,
                store_row_present,
                store_row_legacy,
                transcript_relation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && runtime_snapshot_present == true
                && runtime_snapshot_legacy == true
                && store_row_present == true
                && store_row_legacy == false
                && transcript_relation == LegacyCheckpointTranscriptRelation::NotComparable
            }
            update {}
            to Ready
            emit LegacyCheckpointMigrationResolved {
                disposition: LegacyCheckpointMigrationDisposition::RefuseDivergent
            }
        }

        // ===============================================================
        // Apply-pending-tool-results region. The transition reads the consumed
        // result count and authorizes the apply, emitting `applied_count` equal
        // to `result_count` (vacuous-accept, mirroring the staging-side
        // StageSessionToolResults shape). The shell mirrors `applied_count`
        // onto its `agent.apply_pending_tool_results` call and decides nothing.
        // ===============================================================

        transition ApplyPendingToolResults {
            on input ApplyPendingToolResults {
                session_id,
                result_count
            }
            guard { self.lifecycle_phase == Phase::Ready }
            update {}
            to Ready
            emit SessionToolResultsApplied {
                session_id: session_id,
                applied_count: result_count
            }
        }

        // ===============================================================
        // Transcript-edit region. Each transition reads the typed
        // `TranscriptEditKind` directive and authorizes the commit, echoing the
        // kind so the shell routes to the correct persist handler. The shell
        // mirrors `success` onto its persist path and decides nothing.
        // ===============================================================

        transition TranscriptEditFork {
            on input TranscriptEdit {
                session_id,
                fork_or_rewrite_directive
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && fork_or_rewrite_directive == TranscriptEditKind::Fork
            }
            update {}
            to Ready
            emit TranscriptRewriteCommitted {
                kind: TranscriptEditKind::Fork,
                success: true
            }
        }

        transition TranscriptEditRewrite {
            on input TranscriptEdit {
                session_id,
                fork_or_rewrite_directive
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && fork_or_rewrite_directive == TranscriptEditKind::Rewrite
            }
            update {}
            to Ready
            emit TranscriptRewriteCommitted {
                kind: TranscriptEditKind::Rewrite,
                success: true
            }
        }

        // ===============================================================
        // Lifecycle-terminal region (LUC-524 R004 fold). The recover
        // transition adopts the canonical current archived-ness into the
        // machine-owned registry; the two archive transitions decide the
        // disposition and the realization action vector from that
        // machine-owned terminal state. The Archive guards read the
        // machine's own map — a drive against an unseeded session id fails
        // closed at the generated accessor, so the shell MUST recover-seed
        // before driving the archive input.
        // ===============================================================

        transition RecoverSessionLifecycleTerminal {
            on input RecoverSessionLifecycleTerminal { session_id, terminal }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (terminal == SessionDocumentLifecycle::Active
                    || terminal == SessionDocumentLifecycle::Archived)
            }
            update {
                self.session_lifecycle_terminal.insert(session_id, terminal);
            }
            to Ready
            emit SessionLifecycleTerminalRecovered
        }

        transition ReviveArchivedSessionDocument {
            on input ReviveArchivedSessionDocument { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_lifecycle_terminal.get_cloned(session_id).get("value")
                    == SessionDocumentLifecycle::Archived
            }
            update {
                self.session_lifecycle_terminal
                    .insert(session_id, SessionDocumentLifecycle::Active);
            }
            to Ready
            emit SessionRevivalResolved
        }

        // Archive from Active: the only transition that moves the document
        // lifecycle to Archived. The action vector instructs the shell to
        // commit the durable document iff a durable snapshot exists, and to
        // retire the runtime iff the archive is runtime-backed and the
        // runtime actually knows the session.
        transition ArchiveSessionDocumentActive {
            on input ArchiveSessionDocument {
                session_id,
                runtime_backed,
                durable_document_present,
                runtime_observation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_lifecycle_terminal.get_cloned(session_id).get("value")
                    == SessionDocumentLifecycle::Active
            }
            update {
                self.session_lifecycle_terminal
                    .insert(session_id, SessionDocumentLifecycle::Archived);
            }
            to Ready
            emit SessionArchiveResolved {
                disposition: SessionArchiveDisposition::Archive,
                write_document: durable_document_present,
                retire_runtime: archive_should_retire_runtime(
                    runtime_backed,
                    runtime_observation)
            }
        }

        // Idempotent re-archive of a QUIESCENT archived document (no
        // registered runtime): explicit AlreadyArchived verdict with an
        // empty action vector. The surface contract maps this verdict to
        // its existing NotFound error.
        transition ArchiveSessionDocumentAlreadyArchived {
            on input ArchiveSessionDocument {
                session_id,
                runtime_backed,
                durable_document_present,
                runtime_observation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_lifecycle_terminal.get_cloned(session_id).get("value")
                    == SessionDocumentLifecycle::Archived
            }
            guard "runtime_quiescent" {
                runtime_observation != SessionArchiveRuntimeObservation::RetirementRequired
            }
            update {}
            to Ready
            emit SessionArchiveResolved {
                disposition: SessionArchiveDisposition::AlreadyArchived,
                write_document: false,
                retire_runtime: false
            }
        }

        // Retry convergence (ask 21b): an archived document with a STILL
        // REGISTERED runtime is the partial state left by an archive whose
        // document commit landed but whose runtime retire failed (the
        // realization order is document-first by design). The machine owns
        // the convergence: re-archive completes the retire (no document
        // rewrite) instead of resolving AlreadyArchived — which mapped to
        // NotFound and made the partial state permanently unrecoverable
        // (never-run mob members stranded in `retiring` forever).
        transition ArchiveSessionDocumentCompleteRetire {
            on input ArchiveSessionDocument {
                session_id,
                runtime_backed,
                durable_document_present,
                runtime_observation
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_lifecycle_terminal.get_cloned(session_id).get("value")
                    == SessionDocumentLifecycle::Archived
            }
            guard "runtime_residue" {
                runtime_observation == SessionArchiveRuntimeObservation::RetirementRequired
            }
            update {}
            to Ready
            emit SessionArchiveResolved {
                disposition: SessionArchiveDisposition::Archive,
                write_document: false,
                retire_runtime: true
            }
        }

    }
}
