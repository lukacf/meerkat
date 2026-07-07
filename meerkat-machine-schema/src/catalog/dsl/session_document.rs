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
pub enum RuntimeProjectionRollbackDisposition {
    /// The row does not faithfully continue the authority transcript — a
    /// genuine content fork. The save fails closed exactly as before.
    #[default]
    RejectDivergent,
    /// The row is a faithful continuation of the authority transcript (its
    /// tail is turn content whose boundary commit never landed). The
    /// projection write is authorized to rebuild the row onto the authority
    /// transcript, discarding the unacknowledged tail.
    RebuildToAuthority,
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
            // Snapshot-restore consistency authorization, ported verbatim.
            RestoreSystemContextSnapshot {
                active_keys_have_known_pending_or_seen: bool,
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
                all_identity_fields_valid: bool,
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
            ResolveRuntimeProjectionRollback {
                session_id: SessionId,
                row_continues_authority: bool,
                row_is_runtime_checkpoint: bool,
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
            ArchiveSessionDocument {
                session_id: SessionId,
                runtime_backed: bool,
                durable_snapshot_present: bool,
                runtime_session_registered: bool,
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

            // Runtime-projection-rollback disposition. The shell mirrors
            // `disposition`: RebuildToAuthority authorizes the CAS projection
            // write that converges the ahead-of-authority row back onto the
            // committed transcript; RejectDivergent keeps the fail-closed
            // rejection for genuine content forks. Total over the observation,
            // so it is emitted on both branches.
            RuntimeProjectionRollbackResolved {
                disposition: Enum<RuntimeProjectionRollbackDisposition>,
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
        // the archive is runtime-backed AND the session is actually known to
        // the runtime side (a durable snapshot exists or the runtime has the
        // session registered). A runtime-backed archive of a session the
        // runtime has never seen must not spuriously register-then-retire it.
        helper archive_should_retire_runtime(
            runtime_backed: bool,
            durable_snapshot_present: bool,
            runtime_session_registered: bool
        ) -> bool {
            runtime_backed && (durable_snapshot_present || runtime_session_registered)
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
        disposition RuntimeProjectionRollbackResolved => local seam NoOwnerRealization,
        disposition SessionToolResultsApplied => local seam NoOwnerRealization,
        disposition TranscriptRewriteCommitted => local seam NoOwnerRealization,
        disposition SessionLifecycleTerminalRecovered => local seam NoOwnerRealization,
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
        // authorization, ported verbatim from AuthorizeRestoreSystemContextState.
        // ---------------------------------------------------------------
        transition RestoreSystemContextSnapshot {
            on input RestoreSystemContextSnapshot {
                active_keys_have_known_pending_or_seen,
                seen_keys_match_known_appends
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && active_keys_have_known_pending_or_seen
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
                all_identity_fields_valid,
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
                && all_identity_fields_valid
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

        // Accept (provider recomputed from a model-only change): clears stored
        // provider + self-hosted binding; provider_overridden is true.
        transition AuthorizeSessionResumeOverridesAcceptRecomputeProvider {
            on input AuthorizeSessionResumeOverrides {
                provider_override_present,
                model_override_present,
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
            }
            update {}
            to Ready
            emit SessionResumeOverridesAuthorized {
                provider_selection: ResumeProviderSelection::RecomputeFromModel,
                self_hosted_selection: ResumeSelfHostedSelection::Clear,
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
            }
            update {}
            to Ready
            emit SessionResumeOverridesAuthorized {
                provider_selection: ResumeProviderSelection::UseOverride,
                self_hosted_selection: ResumeSelfHostedSelection::Clear,
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
            }
            update {}
            to Ready
            emit SessionResumeOverridesAuthorized {
                provider_selection: ResumeProviderSelection::UseStored,
                self_hosted_selection: ResumeSelfHostedSelection::Retain,
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
        // Runtime-projection-rollback region. Both transitions read the pure
        // continuation observation and resolve the disposition of a
        // runtime-authoritative projection save whose durable row ran ahead
        // of the authority transcript. Total over the observation: a row
        // that faithfully continues the authority (its tail is turn content
        // whose boundary commit never landed) is rebuilt onto committed
        // truth; anything else keeps the fail-closed rejection. The shell
        // mirrors the disposition.
        // ===============================================================

        transition ResolveRuntimeProjectionRollbackRebuild {
            on input ResolveRuntimeProjectionRollback {
                session_id,
                row_continues_authority,
                row_is_runtime_checkpoint
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && row_continues_authority == true
                && row_is_runtime_checkpoint == true
            }
            update {}
            to Ready
            emit RuntimeProjectionRollbackResolved {
                disposition: RuntimeProjectionRollbackDisposition::RebuildToAuthority
            }
        }

        transition ResolveRuntimeProjectionRollbackReject {
            on input ResolveRuntimeProjectionRollback {
                session_id,
                row_continues_authority,
                row_is_runtime_checkpoint
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (row_continues_authority == false
                    || row_is_runtime_checkpoint == false)
            }
            update {}
            to Ready
            emit RuntimeProjectionRollbackResolved {
                disposition: RuntimeProjectionRollbackDisposition::RejectDivergent
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

        // Archive from Active: the only transition that moves the document
        // lifecycle to Archived. The action vector instructs the shell to
        // commit the durable document iff a durable snapshot exists, and to
        // retire the runtime iff the archive is runtime-backed and the
        // runtime actually knows the session.
        transition ArchiveSessionDocumentActive {
            on input ArchiveSessionDocument {
                session_id,
                runtime_backed,
                durable_snapshot_present,
                runtime_session_registered
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
                write_document: durable_snapshot_present,
                retire_runtime: archive_should_retire_runtime(
                    runtime_backed,
                    durable_snapshot_present,
                    runtime_session_registered)
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
                durable_snapshot_present,
                runtime_session_registered
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_lifecycle_terminal.get_cloned(session_id).get("value")
                    == SessionDocumentLifecycle::Archived
            }
            guard "runtime_quiescent" { runtime_session_registered == false }
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
                durable_snapshot_present,
                runtime_session_registered
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_lifecycle_terminal.get_cloned(session_id).get("value")
                    == SessionDocumentLifecycle::Archived
            }
            guard "runtime_residue" { runtime_session_registered == true }
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
