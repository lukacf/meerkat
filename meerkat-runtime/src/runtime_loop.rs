//! RuntimeLoop — per-session tokio task that processes queued inputs.
//!
//! When `MeerkatMachine::accept_input()` queues an input and sets
//! the wake flag, it sends a signal on the wake channel. The RuntimeLoop
//! picks it up, dequeues the input, converts it to a `RunPrimitive`,
//! and applies it via the `CoreExecutor` (which calls `SessionService::start_turn()`
//! under the hood).

use meerkat_core::interaction::InteractionId;
use meerkat_core::lifecycle::core_executor::{
    CoreApplyFailureCause, CoreApplyTerminal, CoreExecutorError,
};
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive, StagedRunInput};
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::turn_execution_authority::ContentShape as TurnContentShape;
use meerkat_core::types::TranscriptMessageIdentity;

use crate::input::Input;
#[cfg(test)]
use crate::input::input_prompt_text;
#[cfg(test)]
use crate::input::runtime_input_projection_for_machine_batch;
use crate::tokio;

/// Extract a prompt string from an `Input`.
#[cfg(test)]
pub(crate) fn input_to_prompt(input: &Input) -> String {
    input_prompt_text(input)
}

/// Canonical runtime-side constructor for [`RuntimeTurnMetadata`].
///
/// This is the ONLY construction site for `RuntimeTurnMetadata` inside
/// `meerkat-runtime/src/`. Any other literal/default construction is an
/// RMAT-governed seam leak; the `turn_metadata_single_construction_site`
/// integration test greps for that invariant at build time.
pub(crate) fn for_input(
    input: &Input,
    semantics: crate::ingress_types::RuntimeInputSemantics,
) -> meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
    let mut metadata = match input {
        Input::Prompt(prompt) => prompt.turn_metadata.clone().unwrap_or_default(),
        Input::FlowStep(flow_step) => flow_step.turn_metadata.clone().unwrap_or_default(),
        Input::ExternalEvent(event) => RuntimeTurnMetadata {
            handling_mode: Some(event.handling_mode),
            render_metadata: event.render_metadata.clone(),
            transcript_identity: TranscriptMessageIdentity {
                objective_id: event.objective_id,
                ..Default::default()
            },
            ..Default::default()
        },
        Input::Continuation(continuation) => RuntimeTurnMetadata {
            handling_mode: Some(continuation.handling_mode),
            turn_tool_overlay: continuation.turn_tool_overlay.clone(),
            ..Default::default()
        },
        Input::Peer(peer) => RuntimeTurnMetadata {
            handling_mode: peer.handling_mode,
            transcript_identity: TranscriptMessageIdentity {
                objective_id: peer.objective_id,
                ..Default::default()
            },
            ..Default::default()
        },
        _ => RuntimeTurnMetadata::default(),
    };
    // Directed terminal publication is runtime-minted capability data. Never
    // preserve IDs supplied through caller-authored turn metadata.
    metadata.directed_interaction_ids.clear();
    if let Some(handling_mode) = semantics.execution_handling_mode {
        metadata.handling_mode = Some(handling_mode);
    }
    metadata.execution_kind = Some(semantics.execution_kind);
    metadata.peer_response_terminal_apply_intent = semantics.peer_response_terminal_apply_intent;
    match input {
        Input::FlowStep(flow_step) => {
            if let Some(interaction_id) = flow_step.directed_interaction_id {
                metadata.directed_interaction_ids.push(interaction_id);
            }
        }
        Input::Peer(peer) => {
            if let Some(interaction_id) = peer.directed_interaction_id {
                metadata.directed_interaction_ids.push(interaction_id);
            }
        }
        _ => {}
    }
    if let Some(correlation_id) = input.header().correlation_id.as_ref() {
        metadata.transcript_identity.interaction_id = Some(InteractionId(correlation_id.0));
    }
    metadata
}

/// Canonical constructor for the member-side bridge turn-directive lane
/// (multi-host §18.11): the directive's handling mode and tool overlay are
/// the only caller-supplied facts; everything else keeps the FlowStep-input
/// defaults exactly as `for_input`'s FlowStep arm would derive them. Lives
/// HERE because this file is the single sanctioned construction site for
/// `RuntimeTurnMetadata` (see `turn_metadata_single_construction_site`).
pub(crate) fn for_bridge_turn_directive(
    handling_mode: meerkat_core::types::HandlingMode,
    turn_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
) -> meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
        handling_mode: Some(handling_mode),
        turn_tool_overlay,
        ..Default::default()
    }
}

/// Merge the per-input turn metadata carried by a staged batch into a single
/// typed carrier. Scalar conflicts (two inputs disagreeing on e.g. `model`)
/// are refused with a typed error so caller policy is not silently replaced by
/// shell defaults.
pub(crate) fn merge_batch_turn_metadata(
    inputs: &[(meerkat_core::lifecycle::InputId, Input)],
    semantics: &[crate::ingress_types::RuntimeInputSemantics],
) -> Result<
    Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata>,
    meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict,
> {
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
    if inputs.len() != semantics.len() {
        return Err(
            meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict {
                field: "execution_kind",
                reason: "runtime-stamped execution kind missing for one or more inputs",
            },
        );
    }

    let mut acc: Option<RuntimeTurnMetadata> = None;
    let mut transcript_identity = TranscriptIdentityConsensus::default();
    for ((_, input), semantics) in inputs.iter().zip(semantics.iter()) {
        let mut meta = for_input(input, *semantics);
        transcript_identity.observe(&meta.transcript_identity);
        // Identity consensus needs a sticky conflict state across the whole
        // batch. Feeding the lossy empty result of a pairwise conflict into a
        // later merge would let unrelated causality reseed the accumulator.
        meta.transcript_identity = TranscriptMessageIdentity::default();
        match acc.as_mut() {
            None => acc = Some(meta),
            Some(existing) => {
                existing.merge(meta)?;
            }
        }
    }
    if let Some(metadata) = acc.as_mut() {
        metadata.transcript_identity = transcript_identity.finish();
    }
    // Batch-level peer-reply capability mint: every peer *message* delivery
    // in the admitted batch contributes one typed reply capability. Minted
    // AFTER the per-input fold so the scalar overlay merge above never sees a
    // runtime-minted overlay (which would spuriously conflict across inputs).
    let deliveries = inputs
        .iter()
        .filter_map(|(_, input)| crate::input::peer_reply_capability(input))
        .collect::<Vec<_>>();
    if !deliveries.is_empty() {
        // `for_input` stamps `execution_kind` on every input, so a non-empty
        // batch always folded into `Some`; a missing accumulator here is a
        // logic error and fails closed with a typed error.
        let Some(metadata) = acc.as_mut() else {
            return Err(
                meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict {
                    field: "turn_tool_overlay",
                    reason: "peer reply capability minted for a batch with no metadata accumulator",
                },
            );
        };
        attach_peer_reply_capabilities(metadata, deliveries)?;
    }
    Ok(acc.filter(|m| !m.is_empty()))
}

#[derive(Debug, Clone, Default)]
enum IdentityFieldConsensus<T> {
    #[default]
    Unseen,
    Agreed(T),
    Conflicted,
}

impl<T: Eq> IdentityFieldConsensus<T> {
    fn observe(&mut self, candidate: Option<T>) {
        let Some(candidate) = candidate else {
            return;
        };
        match self {
            Self::Unseen => *self = Self::Agreed(candidate),
            Self::Agreed(current) if *current == candidate => {}
            Self::Agreed(_) => *self = Self::Conflicted,
            Self::Conflicted => {}
        }
    }

    fn finish(self) -> Option<T> {
        match self {
            Self::Agreed(value) => Some(value),
            Self::Unseen | Self::Conflicted => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct TranscriptIdentityConsensus {
    interaction_id: IdentityFieldConsensus<InteractionId>,
    run_id: IdentityFieldConsensus<RunId>,
    objective_id: IdentityFieldConsensus<meerkat_core::interaction::ObjectiveId>,
}

impl TranscriptIdentityConsensus {
    fn observe(&mut self, identity: &TranscriptMessageIdentity) {
        self.interaction_id.observe(identity.interaction_id);
        self.run_id.observe(identity.run_id.clone());
        self.objective_id.observe(identity.objective_id);
    }

    fn finish(self) -> TranscriptMessageIdentity {
        TranscriptMessageIdentity {
            interaction_id: self.interaction_id.finish(),
            run_id: self.run_id.finish(),
            objective_id: self.objective_id.finish(),
        }
    }
}

/// Attach the batch's typed peer-reply capabilities to the accumulated turn
/// metadata by composing a dispatch-context-only overlay into its turn tool
/// overlay. Mutates the single accumulator minted by [`for_input`] — this is
/// NOT a second `RuntimeTurnMetadata` construction site.
fn attach_peer_reply_capabilities(
    metadata: &mut meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata,
    deliveries: Vec<meerkat_core::comms::PeerReplyCapability>,
) -> Result<(), meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    use meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict;

    let context = meerkat_core::comms::PeerReplyDispatchContext { deliveries };
    let value = serde_json::to_value(&context).map_err(|error| {
        tracing::error!(
            error = %error,
            "typed peer reply dispatch context failed to serialize"
        );
        TurnMetadataMergeConflict {
            field: "turn_tool_overlay",
            reason: "peer reply dispatch context failed to serialize",
        }
    })?;
    let mut reply_overlay = meerkat_core::service::TurnToolOverlay::default();
    reply_overlay.dispatch_context.insert(
        meerkat_core::comms::COMMS_PEER_REPLY_DISPATCH_CONTEXT_KEY.to_string(),
        value,
    );
    let composed = meerkat_core::service::TurnToolOverlay::compose(
        metadata.turn_tool_overlay.take(),
        reply_overlay,
    )
    .map_err(|error| {
        tracing::error!(
            error = %error,
            "peer reply overlay composition conflicted with an existing dispatch-context entry"
        );
        TurnMetadataMergeConflict {
            field: "turn_tool_overlay",
            reason: "peer reply dispatch context conflicted with an existing overlay entry",
        }
    })?;
    metadata.turn_tool_overlay = Some(composed);
    Ok(())
}

fn completion_terminal_observation(
    terminal: Option<&CoreApplyTerminal>,
) -> crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation {
    match terminal {
        Some(CoreApplyTerminal::RunResult(_)) => {
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::RunResult
        }
        Some(CoreApplyTerminal::CallbackPending { .. }) => {
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::CallbackPending
        }
        Some(CoreApplyTerminal::MachineTerminalFailure { .. }) => {
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::MachineTerminal
        }
        Some(CoreApplyTerminal::NoPendingBoundary) | None => {
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::NoResult
        }
    }
}

fn nondirected_completion_input_ids(
    input_ids: &[InputId],
    directed_interaction_ids: &[InteractionId],
) -> Vec<InputId> {
    let directed = directed_interaction_ids
        .iter()
        .map(|interaction_id| interaction_id.0)
        .collect::<std::collections::HashSet<_>>();
    input_ids
        .iter()
        .filter(|input_id| !directed.contains(&input_id.0))
        .cloned()
        .collect()
}

async fn runtime_completion_result_class(
    driver: &crate::meerkat_machine::SharedDriver,
    run_id: Option<&RunId>,
    terminal: crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation,
    finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
) -> Result<
    crate::meerkat_machine::driver::RuntimeCompletionResultAuthority,
    crate::RuntimeDriverError,
> {
    let driver = driver.lock().await;
    crate::meerkat_machine::driver::machine_resolve_runtime_completion_result(
        &driver,
        run_id,
        terminal,
        finalization,
    )
}

// This is the handoff boundary for independently owned completion facts; keep
// the arguments explicit so no shell aggregate can manufacture authority.
#[allow(clippy::too_many_arguments)]
async fn resolve_runtime_completion_waiters(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    authority_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    input_ids: &[InputId],
    run_id: &RunId,
    terminal: Option<&CoreApplyTerminal>,
    finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
    finalization_error: Option<meerkat_core::TurnErrorMetadata>,
) {
    let Some(completions) = completions else {
        return;
    };
    let driver = std::sync::Arc::clone(driver);
    let completions = std::sync::Arc::clone(completions);
    let owned_input_ids = input_ids.to_vec();
    let owned_run_id = run_id.clone();
    let owned_terminal = terminal.cloned();
    let handoff = tokio::spawn(async move {
        let _authority_guard = authority_guard;
        let driver_guard = driver.lock_owned().await;
        let completion_guard = completions.lock_owned().await;
        let bundle = crate::meerkat_machine::driver::machine_resolve_runtime_completion_result(
            &driver_guard,
            Some(&owned_run_id),
            completion_terminal_observation(owned_terminal.as_ref()),
            finalization,
        )
        .map_err(|error| {
            crate::completion::CompletionWaitError::AuthorityUnavailable(format!(
                "runtime completion result authority missing: {error}"
            ))
        })
        .and_then(|authority| {
            crate::completion::authorize_runtime_terminal_bundle(
                &[],
                owned_terminal.as_ref(),
                authority,
                finalization_error,
                None,
            )
        });
        let _driver_guard = driver_guard;
        let mut completion_guard = completion_guard;
        match bundle {
            Ok(bundle) => {
                completion_guard
                    .resolve_authorized_runtime_terminal_bundle(owned_input_ids, bundle);
            }
            Err(error) => completion_guard.fail_inputs(owned_input_ids, error),
        }
    });
    if let Err(error) = handoff.await {
        tracing::error!(
            %error,
            "owned runtime completion waiter handoff failed"
        );
    }
}

async fn reconcile_compaction_projection_outbox(
    driver: &crate::meerkat_machine::SharedDriver,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
) -> Result<Option<Vec<u8>>, crate::RuntimeDriverError> {
    let intents = {
        let driver = driver.lock().await;
        driver.load_pending_compaction_projections().await?
    };
    reconcile_loaded_compaction_projection_outbox(driver, executor, intents).await
}

async fn reconcile_loaded_compaction_projection_outbox(
    driver: &crate::meerkat_machine::SharedDriver,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    intents: Vec<meerkat_core::CompactionProjectionIntent>,
) -> Result<Option<Vec<u8>>, crate::RuntimeDriverError> {
    // Always invoke the executor, including for an empty outbox: the durable
    // memory store uses the exact empty authority set to abort stages left by
    // a crash before RuntimeStore::atomic_apply.
    executor
        .reconcile_committed_compaction_projections(&intents)
        .await
        .map_err(|error| {
            crate::RuntimeDriverError::Internal(format!(
                "compaction projection reconciliation failed: {error}"
            ))
        })?;
    let authoritative_snapshot = {
        let driver = driver.lock().await;
        driver.load_compaction_checkpoint_snapshot().await?
    };
    let compatibility_checkpoint = if let Some(snapshot) = authoritative_snapshot.as_deref() {
        let cleaned = compatibility_checkpoint_after_compaction_finalize(snapshot, &intents)?;
        executor
            .checkpoint_committed_session_snapshot(&cleaned)
            .await
            .map_err(|error| {
                crate::RuntimeDriverError::Internal(format!(
                    "failed to checkpoint authoritative runtime snapshot after compaction reconciliation: {error}"
                ))
            })?;
        Some(cleaned)
    } else {
        None
    };

    // The SessionStore row is a compatibility projection of the committed
    // runtime snapshot. Keep the durable outbox pending until that projection
    // succeeds: marking an intent finalized also cleans it from the runtime
    // snapshot, so doing that first would let a projection failure quarantine
    // and delete the only committed transcript authority. With the outbox
    // still pending, a failed checkpoint leaves the runtime snapshot intact
    // and cold recovery can retry the idempotent projection finalization.
    for intent in &intents {
        let driver = driver.lock().await;
        driver
            .mark_compaction_projection_finalized(&intent.projection)
            .await?;
    }
    Ok(compatibility_checkpoint)
}

/// Abort every live pre-commit projection after a failed runtime boundary
/// commit.
///
/// `RuntimeStore::atomic_apply` is the final fallible persistence operation in
/// `commit_runtime_loop_run`, and its contract guarantees that an error leaves
/// none of the boundary writes visible. Startup also drains every previously
/// committed projection outbox before admitting work. Consequently no durable
/// authority can retain the live transcript, context events, or compaction
/// stages produced by this failed run: they must all be aborted.
async fn abort_rejected_run_projections_after_commit_failure(
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
) -> Result<(), crate::RuntimeDriverError> {
    executor
        .abort_rejected_run_projections()
        .await
        .map_err(|error| {
            crate::RuntimeDriverError::Internal(format!(
                "rejected runtime run projection abort failed: {error}"
            ))
        })
}

fn compatibility_checkpoint_after_compaction_finalize(
    snapshot: &[u8],
    finalized: &[meerkat_core::CompactionProjectionIntent],
) -> Result<Vec<u8>, crate::RuntimeDriverError> {
    if finalized.is_empty() {
        return Ok(snapshot.to_vec());
    }
    let mut session: meerkat_core::Session = serde_json::from_slice(snapshot).map_err(|error| {
        crate::RuntimeDriverError::Internal(format!(
            "committed runtime checkpoint is not a Session: {error}"
        ))
    })?;
    for intent in finalized {
        session
            .complete_compaction_projection_intent(&intent.projection)
            .map_err(|error| {
                crate::RuntimeDriverError::Internal(format!(
                    "failed to clear finalized compaction intent {} from compatibility checkpoint: {error}",
                    intent.projection.revision()
                ))
            })?;
    }
    serde_json::to_vec(&session).map_err(|error| {
        crate::RuntimeDriverError::Internal(format!(
            "failed to serialize committed compatibility checkpoint: {error}"
        ))
    })
}

#[derive(Debug, thiserror::Error)]
enum InteractionTerminalPublicationError {
    #[error("retryable terminal publication failure: {0}")]
    Retryable(String),
    #[error("stale terminal publication authority: {0}")]
    StaleAuthority(String),
    #[error("terminal publication corruption: {0}")]
    Corrupt(String),
}

impl InteractionTerminalPublicationError {
    fn from_driver(context: &str, error: crate::RuntimeDriverError) -> Self {
        let detail = format!("{context}: {error}");
        match error {
            crate::RuntimeDriverError::NotReady { .. }
            | crate::RuntimeDriverError::NotFound { .. }
            | crate::RuntimeDriverError::Destroyed
            | crate::RuntimeDriverError::StaleAuthority { .. } => Self::StaleAuthority(detail),
            crate::RuntimeDriverError::ValidationFailed { .. }
            | crate::RuntimeDriverError::RecoveryCorruption { .. } => Self::Corrupt(detail),
            crate::RuntimeDriverError::UnregisterFinalizationOutcomeUnknown { .. }
            | crate::RuntimeDriverError::UnregisterInProgress { .. }
            | crate::RuntimeDriverError::RuntimeStopInProgress { .. }
            | crate::RuntimeDriverError::Internal(_) => Self::Retryable(detail),
        }
    }

    fn from_generated_authority(context: &str, error: crate::RuntimeDriverError) -> Self {
        match error {
            crate::RuntimeDriverError::NotReady { .. }
            | crate::RuntimeDriverError::NotFound { .. }
            | crate::RuntimeDriverError::Destroyed
            | crate::RuntimeDriverError::StaleAuthority { .. } => {
                Self::StaleAuthority(format!("{context}: {error}"))
            }
            other => Self::Corrupt(format!("{context}: {other}")),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn finalize_publish_and_resolve_runtime_terminals(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    authority_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    teardown_slot: &RuntimeLoopTeardownSlot,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    input_ids: &[InputId],
    interaction_ids: &[InteractionId],
    run_id: &RunId,
    terminal_observation: crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation,
    terminal: Option<&CoreApplyTerminal>,
    finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
    finalization_error: Option<meerkat_core::TurnErrorMetadata>,
) -> Result<(), InteractionTerminalPublicationError> {
    let batch_key = crate::input_state::InteractionTerminalBatchKey::Run {
        run_id: run_id.clone(),
    };
    let (persisted_input_ids, persisted_interaction_ids) = if interaction_ids.is_empty() {
        (input_ids.to_vec(), Vec::new())
    } else {
        let candidate_owner_input_id = interaction_ids
            .iter()
            .min_by_key(|interaction_id| interaction_id.0)
            .map(|interaction_id| InputId::from_uuid(interaction_id.0))
            .ok_or_else(|| {
                InteractionTerminalPublicationError::Corrupt(
                    "directed terminal batch lost its candidate owner".to_string(),
                )
            })?;
        driver
            .lock()
            .await
            .interaction_terminal_batch_for_owner(&candidate_owner_input_id)
            .map_err(|error| {
                InteractionTerminalPublicationError::from_driver(
                    "failed to load committed interaction terminal batch",
                    error,
                )
            })?
    };
    let result = publish_authorized_runtime_terminal_batch(
        driver,
        completions,
        Some(authority_guard),
        executor,
        &persisted_input_ids,
        &persisted_interaction_ids,
        &batch_key,
        terminal_observation,
        terminal,
        None,
        finalization,
        finalization_error,
        None,
    )
    .await;
    if result.is_ok() && interaction_ids.is_empty() {
        teardown_slot.clear_pending_nondirected_run_terminal(run_id);
    }
    result
}

enum OwnedRuntimeLoopCommitOutcome {
    Committed,
    Rejected(String),
    CleanupCarrierCorrupt(String),
}

/// Own the persistence await and synchronous non-directed recovery-carrier
/// transition independently of the runtime-loop task.
///
/// Some stores execute `atomic_apply` on a blocking worker. Cancelling the
/// outer loop can drop its await while that worker still commits, so staging a
/// carrier only after the await is too late. This child owns M, the full commit
/// payload, and the teardown slot: it installs `PreparingPersistence` before
/// the persistence await, clears it on a typed rejection, and marks it
/// `Persisted` in the same child before releasing M.
#[allow(clippy::too_many_arguments)]
async fn commit_runtime_loop_output_owned(
    authority_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    teardown_slot: std::sync::Arc<RuntimeLoopTeardownSlot>,
    driver: crate::meerkat_machine::SharedDriver,
    run_id: RunId,
    input_ids: Vec<InputId>,
    receipt: meerkat_core::lifecycle::RunBoundaryReceiptDraft,
    session_snapshot: Option<Vec<u8>>,
    directed_interaction_ids: Vec<InteractionId>,
    terminal: Option<CoreApplyTerminal>,
    machine_terminal_error: Option<meerkat_core::TurnErrorMetadata>,
    retain_nondirected_carrier: bool,
) -> Result<
    (
        crate::tokio::sync::OwnedMutexGuard<()>,
        OwnedRuntimeLoopCommitOutcome,
    ),
    crate::RuntimeDriverError,
> {
    let task = tokio::spawn(async move {
        if let Err(error) = teardown_slot.stage_owned_commit_in_flight(&run_id) {
            return (
                authority_guard,
                OwnedRuntimeLoopCommitOutcome::Rejected(format!(
                    "failed to stage owned runtime commit cleanup carrier: {error}"
                )),
            );
        }
        if retain_nondirected_carrier {
            let carrier = PendingNondirectedRunTerminal {
                phase: PendingNondirectedRunTerminalPhase::PreparingPersistence,
                input_ids: input_ids.clone(),
                run_id: run_id.clone(),
                terminal: terminal.clone(),
                terminal_observation: completion_terminal_observation(terminal.as_ref()),
                completion_error_metadata: machine_terminal_error.clone(),
            };
            if let Err(error) = teardown_slot.stage_prepared_nondirected_run_terminal(carrier) {
                let cleanup_error = teardown_slot
                    .mark_owned_commit_rejected_abort_pending(&run_id)
                    .err();
                let outcome = match cleanup_error {
                    Some(cleanup_error) => {
                        OwnedRuntimeLoopCommitOutcome::CleanupCarrierCorrupt(format!(
                            "failed to stage non-directed commit recovery carrier: {error}; rejected-commit cleanup carrier also failed: {cleanup_error}"
                        ))
                    }
                    None => OwnedRuntimeLoopCommitOutcome::Rejected(format!(
                        "failed to stage non-directed commit recovery carrier: {error}"
                    )),
                };
                return (authority_guard, outcome);
            }
        }

        let commit_result = if let Some(error) = machine_terminal_error {
            match session_snapshot {
                Some(session_snapshot) => crate::meerkat_machine::commit_machine_terminal_run(
                    &driver,
                    run_id.clone(),
                    error,
                    crate::meerkat_machine::driver::MachineTerminalAppliedDraft {
                        receipt,
                        session_snapshot,
                    },
                )
                .await
                .map_err(|error| error.to_string()),
                None => {
                    Err("machine-terminal apply omitted the required session snapshot".to_string())
                }
            }
        } else {
            crate::meerkat_machine::commit_runtime_loop_run(
                &driver,
                run_id.clone(),
                input_ids,
                receipt,
                session_snapshot,
                directed_interaction_ids,
                terminal.as_ref(),
            )
            .await
            .map_err(|error| error.to_string())
        };

        let outcome = match commit_result {
            Ok(()) => {
                if retain_nondirected_carrier {
                    match teardown_slot.mark_nondirected_run_terminal_persisted(&run_id) {
                        Ok(()) => match teardown_slot.clear_owned_commit_in_flight(&run_id) {
                            Ok(()) => OwnedRuntimeLoopCommitOutcome::Committed,
                            Err(error) => OwnedRuntimeLoopCommitOutcome::CleanupCarrierCorrupt(
                                error.to_string(),
                            ),
                        },
                        Err(error) => {
                            // Keep InFlight intact. The durable commit may
                            // already exist, so teardown must fail closed; it
                            // may not reinterpret this as a rejected boundary.
                            OwnedRuntimeLoopCommitOutcome::CleanupCarrierCorrupt(error.to_string())
                        }
                    }
                } else {
                    match teardown_slot.clear_owned_commit_in_flight(&run_id) {
                        Ok(()) => OwnedRuntimeLoopCommitOutcome::Committed,
                        Err(error) => {
                            OwnedRuntimeLoopCommitOutcome::CleanupCarrierCorrupt(error.to_string())
                        }
                    }
                }
            }
            Err(error) => match teardown_slot.mark_owned_commit_rejected_abort_pending(&run_id) {
                Ok(()) => OwnedRuntimeLoopCommitOutcome::Rejected(error),
                Err(cleanup_error) => {
                    OwnedRuntimeLoopCommitOutcome::CleanupCarrierCorrupt(format!(
                        "runtime commit rejected with {error}; cleanup carrier also failed: {cleanup_error}"
                    ))
                }
            },
        };
        (authority_guard, outcome)
    });

    task.await.map_err(|error| {
        crate::RuntimeDriverError::Internal(format!(
            "owned runtime-loop commit task ended before publishing its outcome: {error}"
        ))
    })
}

enum OwnedRuntimeLoopTerminalization {
    Cancelled,
    Failed {
        failure: CoreApplyFailureCause,
        completion_reason: String,
    },
}

enum OwnedRuntimeLoopTerminalizationOutcome {
    Persisted,
    Rejected(String),
    CleanupCarrierCorrupt(String),
}

/// Own a failed/cancelled run's mutable driver boundary independently of the
/// runtime-loop future. Persistent drivers update their live projection before
/// awaiting the store, so dropping the caller during that await could otherwise
/// expose uncommitted terminal state or strand contributors that became
/// inactive. This child receives M before its first pollable persistence point,
/// stages every in-process recovery carrier, and does not release M until the
/// store result and carrier transition agree.
#[allow(clippy::too_many_arguments)]
async fn realize_runtime_loop_terminal_owned(
    authority_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    teardown_slot: std::sync::Arc<RuntimeLoopTeardownSlot>,
    driver: crate::meerkat_machine::SharedDriver,
    run_id: RunId,
    nondirected_completion_input_ids: Vec<InputId>,
    has_directed_inputs: bool,
    terminalization: OwnedRuntimeLoopTerminalization,
) -> Result<
    (
        crate::tokio::sync::OwnedMutexGuard<()>,
        OwnedRuntimeLoopTerminalizationOutcome,
    ),
    crate::RuntimeDriverError,
> {
    let task = tokio::spawn(async move {
        if let Err(error) = teardown_slot.stage_owned_terminalization_in_flight(&run_id) {
            return (
                authority_guard,
                OwnedRuntimeLoopTerminalizationOutcome::CleanupCarrierCorrupt(error.to_string()),
            );
        }
        let retain_nondirected_carrier = !nondirected_completion_input_ids.is_empty();
        if retain_nondirected_carrier {
            let carrier = PendingNondirectedRunTerminal {
                phase: PendingNondirectedRunTerminalPhase::PreparingPersistence,
                input_ids: nondirected_completion_input_ids,
                run_id: run_id.clone(),
                terminal: None,
                terminal_observation: crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::MachineTerminal,
                completion_error_metadata: None,
            };
            if let Err(error) = teardown_slot.stage_prepared_nondirected_run_terminal(carrier) {
                let _ = teardown_slot.clear_owned_terminalization_in_flight(&run_id);
                return (
                    authority_guard,
                    OwnedRuntimeLoopTerminalizationOutcome::CleanupCarrierCorrupt(
                        error.to_string(),
                    ),
                );
            }
        }

        let (realization, completion_reason) = match terminalization {
            OwnedRuntimeLoopTerminalization::Cancelled => (
                crate::meerkat_machine::cancel_runtime_loop_run(&driver, run_id.clone())
                    .await
                    .map_err(|error| error.to_string()),
                "runtime run cancelled".to_string(),
            ),
            OwnedRuntimeLoopTerminalization::Failed {
                failure,
                completion_reason,
            } => (
                crate::meerkat_machine::fail_runtime_loop_run(&driver, run_id.clone(), failure)
                    .await
                    .map_err(|error| error.to_string()),
                completion_reason,
            ),
        };

        let outcome = match realization {
            Ok(()) => {
                let nondirected_carrier_result = if retain_nondirected_carrier {
                    let completion_error_metadata = {
                        let driver_guard = driver.lock().await;
                        machine_terminal_completion_error(&driver_guard, completion_reason)
                    };
                    completion_error_metadata
                        .and_then(|metadata| {
                            teardown_slot
                                .set_nondirected_run_terminal_completion_error(&run_id, metadata)
                        })
                        .and_then(|()| {
                            teardown_slot.mark_nondirected_run_terminal_persisted(&run_id)
                        })
                } else {
                    Ok(())
                };
                match nondirected_carrier_result.and_then(|()| {
                    if has_directed_inputs {
                        teardown_slot.mark_owned_terminalization_directed_drain_pending(&run_id)
                    } else {
                        teardown_slot.clear_owned_terminalization_in_flight(&run_id)
                    }
                }) {
                    Ok(()) => OwnedRuntimeLoopTerminalizationOutcome::Persisted,
                    Err(error) => {
                        // The store may already contain the terminalized run.
                        // Retain every surviving carrier so teardown fails
                        // closed rather than publishing a runless replacement.
                        OwnedRuntimeLoopTerminalizationOutcome::CleanupCarrierCorrupt(
                            error.to_string(),
                        )
                    }
                }
            }
            Err(error) => match teardown_slot.clear_owned_terminalization_in_flight(&run_id) {
                Ok(()) => {
                    teardown_slot.clear_pending_nondirected_run_terminal(&run_id);
                    OwnedRuntimeLoopTerminalizationOutcome::Rejected(error)
                }
                Err(cleanup_error) => {
                    OwnedRuntimeLoopTerminalizationOutcome::CleanupCarrierCorrupt(format!(
                        "runtime terminalization rejected with {error}; cleanup carrier also failed: {cleanup_error}"
                    ))
                }
            },
        };
        (authority_guard, outcome)
    });

    task.await.map_err(|error| {
        crate::RuntimeDriverError::Internal(format!(
            "owned runtime-loop terminalization task ended before publishing its outcome: {error}"
        ))
    })
}

#[allow(clippy::too_many_arguments)]
async fn publish_authorized_runtime_terminal_batch(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    authority_guard: Option<crate::tokio::sync::OwnedMutexGuard<()>>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    input_ids: &[InputId],
    interaction_ids: &[InteractionId],
    batch_key: &crate::input_state::InteractionTerminalBatchKey,
    terminal_observation: crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation,
    terminal: Option<&CoreApplyTerminal>,
    runtime_termination_reason: Option<&str>,
    finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
    finalization_error: Option<meerkat_core::TurnErrorMetadata>,
    already_finalized_events: Option<&[meerkat_core::event::AgentEvent]>,
) -> Result<(), InteractionTerminalPublicationError> {
    // A non-directed batch has no durable event/outbox carrier. Acquire the
    // driver and completion guards in canonical order before consuming its
    // one-shot generated authority, then synchronously transfer both guards
    // and the bundle to an independently owned task. Cancelling the outer
    // runtime loop can no longer let unregister overwrite this result with a
    // later RuntimeTerminated delivery.
    if interaction_ids.is_empty() {
        let Some(completions) = completions else {
            let _authority_guard = authority_guard;
            let authority = runtime_completion_result_class(
                driver,
                batch_key.run_id(),
                terminal_observation,
                finalization,
            )
            .await
            .map_err(|error| {
                InteractionTerminalPublicationError::from_generated_authority(
                    "generated non-directed terminal authority missing",
                    error,
                )
            })?;
            let _bundle = crate::completion::authorize_runtime_terminal_bundle(
                interaction_ids,
                terminal,
                authority,
                finalization_error,
                runtime_termination_reason,
            )
            .map_err(|error| {
                InteractionTerminalPublicationError::Corrupt(format!(
                    "non-directed terminal projection failed: {error}"
                ))
            })?;
            return Ok(());
        };
        let Some(authority_guard) = authority_guard else {
            return Err(InteractionTerminalPublicationError::Corrupt(
                "non-directed terminal handoff lost its exact runtime authority guard".to_string(),
            ));
        };
        let driver = std::sync::Arc::clone(driver);
        let completions = std::sync::Arc::clone(completions);
        let owned_input_ids = input_ids.to_vec();
        let owned_run_id = batch_key.run_id().cloned();
        let owned_terminal = terminal.cloned();
        let owned_runtime_termination_reason = runtime_termination_reason.map(str::to_string);
        let handoff = tokio::spawn(async move {
            let _authority_guard = authority_guard;
            let driver_guard = driver.lock_owned().await;
            let completion_guard = completions.lock_owned().await;
            let authority =
                crate::meerkat_machine::driver::machine_resolve_runtime_completion_result(
                    &driver_guard,
                    owned_run_id.as_ref(),
                    terminal_observation,
                    finalization,
                )
                .map_err(|error| {
                    InteractionTerminalPublicationError::from_generated_authority(
                        "generated non-directed terminal authority missing",
                        error,
                    )
                })?;
            let bundle = crate::completion::authorize_runtime_terminal_bundle(
                &[],
                owned_terminal.as_ref(),
                authority,
                finalization_error,
                owned_runtime_termination_reason.as_deref(),
            )
            .map_err(|error| {
                InteractionTerminalPublicationError::Corrupt(format!(
                    "non-directed terminal projection failed: {error}"
                ))
            })?;
            let _driver_guard = driver_guard;
            let mut completion_guard = completion_guard;
            completion_guard.resolve_authorized_runtime_terminal_bundle(owned_input_ids, bundle);
            Ok::<(), InteractionTerminalPublicationError>(())
        });
        return handoff.await.map_err(|error| {
            InteractionTerminalPublicationError::Corrupt(format!(
                "owned non-directed completion handoff failed: {error}"
            ))
        })?;
    }

    let authority = runtime_completion_result_class(
        driver,
        batch_key.run_id(),
        terminal_observation,
        finalization,
    )
    .await
    .map_err(|error| {
        InteractionTerminalPublicationError::from_generated_authority(
            "generated interaction terminal authority missing",
            error,
        )
    })?;
    let bundle = crate::completion::authorize_runtime_terminal_bundle(
        interaction_ids,
        terminal,
        authority,
        finalization_error,
        runtime_termination_reason,
    )
    .map_err(|error| {
        InteractionTerminalPublicationError::Corrupt(format!(
            "interaction terminal projection failed: {error}"
        ))
    })?;
    let events = bundle.interaction_events();

    if !events.is_empty() {
        let candidate_owner_input_id = match batch_key {
            crate::input_state::InteractionTerminalBatchKey::Run { .. } => interaction_ids
                .first()
                .map(|interaction_id| InputId::from_uuid(interaction_id.0))
                .ok_or_else(|| {
                    InteractionTerminalPublicationError::Corrupt(
                        "interaction terminal event batch lost its candidate-owner input"
                            .to_string(),
                    )
                })?,
            crate::input_state::InteractionTerminalBatchKey::RuntimeTermination {
                candidate_owner_input_id,
            } => candidate_owner_input_id.clone(),
        };
        if let Some(finalized) = already_finalized_events {
            let mut expected = std::collections::BTreeMap::new();
            for event in events {
                let interaction_id = crate::input_state::interaction_terminal_event_id(event)
                    .ok_or_else(|| {
                        InteractionTerminalPublicationError::Corrupt(
                            "authorized terminal bundle emitted a non-terminal".to_string(),
                        )
                    })?;
                let digest = crate::input_state::interaction_terminal_payload_digest(event)
                    .map_err(InteractionTerminalPublicationError::Corrupt)?;
                if expected.insert(interaction_id.0, digest).is_some() {
                    return Err(InteractionTerminalPublicationError::Corrupt(
                        "authorized terminal bundle repeated an interaction id".to_string(),
                    ));
                }
            }
            let mut recovered = std::collections::BTreeMap::new();
            for event in finalized {
                let interaction_id = crate::input_state::interaction_terminal_event_id(event)
                    .ok_or_else(|| {
                        InteractionTerminalPublicationError::Corrupt(
                            "recovered outbox carried a non-terminal event".to_string(),
                        )
                    })?;
                let digest = crate::input_state::interaction_terminal_payload_digest(event)
                    .map_err(InteractionTerminalPublicationError::Corrupt)?;
                if recovered.insert(interaction_id.0, digest).is_some() {
                    return Err(InteractionTerminalPublicationError::Corrupt(
                        "recovered outbox repeated an interaction id".to_string(),
                    ));
                }
            }
            if expected != recovered {
                return Err(InteractionTerminalPublicationError::Corrupt(
                    "recovered finalized outbox did not match generated terminal authority"
                        .to_string(),
                ));
            }
        } else {
            let mut driver = driver.lock().await;
            driver
                .finalize_interaction_terminal_outboxes(
                    &candidate_owner_input_id,
                    events,
                    finalization,
                )
                .await
                .map_err(|error| {
                    InteractionTerminalPublicationError::from_driver(
                        "interaction terminal outbox finalization failed",
                        error,
                    )
                })?;
        }
        let receipts = executor
            .publish_interaction_terminals(events)
            .await
            .map_err(|error| {
                InteractionTerminalPublicationError::Retryable(format!(
                    "interaction terminal durable publication failed: {error}"
                ))
            })?;
        if let Some(completions) = completions {
            // Keep M/driver authority and the waiter registry jointly owned
            // across the receipt CAS and synchronous bundle delivery. The
            // child is spawned only after both guards are acquired, so every
            // pre-spawn cancellation leaves the outbox Finalized/recoverable,
            // while every post-spawn cancellation leaves an independent owner
            // that makes Published + waiter delivery converge.
            let driver_guard = std::sync::Arc::clone(driver).lock_owned().await;
            let completion_guard = std::sync::Arc::clone(completions).lock_owned().await;
            let owned_input_ids = input_ids.to_vec();
            let handoff = tokio::spawn(async move {
                let _authority_guard = authority_guard;
                let mut driver_guard = driver_guard;
                driver_guard
                    .mark_interaction_terminal_outboxes_published(
                        &candidate_owner_input_id,
                        &receipts,
                    )
                    .await
                    .map_err(|error| {
                        InteractionTerminalPublicationError::from_driver(
                            "interaction terminal publication receipt persistence failed",
                            error,
                        )
                    })?;
                let mut completion_guard = completion_guard;
                completion_guard
                    .resolve_authorized_runtime_terminal_bundle(owned_input_ids, bundle);
                Ok::<(), InteractionTerminalPublicationError>(())
            });
            return handoff.await.map_err(|error| {
                InteractionTerminalPublicationError::Retryable(format!(
                    "owned interaction terminal receipt handoff failed: {error}"
                ))
            })?;
        }

        let _authority_guard = authority_guard;
        let mut driver = driver.lock().await;
        driver
            .mark_interaction_terminal_outboxes_published(&candidate_owner_input_id, &receipts)
            .await
            .map_err(|error| {
                InteractionTerminalPublicationError::from_driver(
                    "interaction terminal publication receipt persistence failed",
                    error,
                )
            })?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum RuntimeProjectionRecoveryAuthority {
    RuntimeLoop,
    Teardown,
}

async fn drain_recovered_interaction_terminal_outboxes(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    authority_binding: &RuntimeLoopAuthorityBinding,
    authority_kind: RuntimeProjectionRecoveryAuthority,
) -> Result<(), InteractionTerminalPublicationError> {
    let authority_guard = match authority_kind {
        RuntimeProjectionRecoveryAuthority::RuntimeLoop => {
            authority_binding
                .lock_current_driver_authority(driver, "interaction terminal recovery drain")
                .await
        }
        RuntimeProjectionRecoveryAuthority::Teardown => {
            authority_binding
                .lock_current_teardown_driver_authority(
                    driver,
                    "interaction terminal teardown recovery drain",
                )
                .await
        }
    }
    .map_err(|error| {
        InteractionTerminalPublicationError::from_driver(
            "interaction terminal recovery lost runtime authority",
            error,
        )
    })?;
    drain_recovered_interaction_terminal_outboxes_under_authority(
        driver,
        completions,
        executor,
        &authority_guard,
    )
    .await
}

async fn drain_recovered_interaction_terminal_outboxes_under_authority(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    _authority_guard: &crate::tokio::sync::OwnedMutexGuard<()>,
) -> Result<(), InteractionTerminalPublicationError> {
    let batches = driver
        .lock()
        .await
        .interaction_terminal_recovery_batches()
        .await
        .map_err(|error| {
            InteractionTerminalPublicationError::from_driver(
                "interaction terminal outbox recovery failed",
                error,
            )
        })?;
    for batch in batches {
        let crate::meerkat_machine::driver::InteractionTerminalRecoveryBatch {
            batch_key,
            input_ids,
            interaction_ids,
            terminal,
            terminal_observation,
            runtime_termination_reason,
            completion_error_metadata,
            phase,
        } = batch;
        if let Some(run_id) = batch_key.run_id() {
            let driver = driver.lock().await;
            crate::meerkat_machine::driver::machine_recover_runtime_completion_result_correlation(
                &driver, run_id,
            )
            .map_err(|error| {
                InteractionTerminalPublicationError::from_generated_authority(
                    "generated interaction terminal recovery correlation missing",
                    error,
                )
            })?;
        }
        match phase {
            crate::meerkat_machine::driver::InteractionTerminalRecoveryPhase::Candidate => {
                let committed_snapshot = if batch_key.run_id().is_some() {
                    driver
                        .lock()
                        .await
                        .committed_session_snapshot_for_terminal_recovery()
                        .await
                        .map_err(|error| {
                            InteractionTerminalPublicationError::from_driver(
                                "interaction terminal recovery snapshot load failed",
                                error,
                            )
                        })?
                } else {
                    None
                };
                let (finalization, finalization_error) = if let Some(snapshot) =
                    committed_snapshot.as_deref()
                {
                    match executor
                            .checkpoint_committed_session_snapshot(snapshot)
                            .await
                        {
                            Ok(()) => (
                                crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
                                completion_error_metadata.clone(),
                            ),
                            Err(error) => {
                                let metadata = meerkat_core::TurnErrorMetadata::runtime_apply_failure(
                                    format!(
                                        "runtime session checkpoint failed during terminal recovery: {error}"
                                    ),
                                );
                                (
                                    crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed,
                                    Some(metadata),
                                )
                            }
                        }
                } else {
                    (
                            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
                            completion_error_metadata.clone(),
                        )
                };
                publish_authorized_runtime_terminal_batch(
                    driver,
                    completions,
                    None,
                    executor,
                    &input_ids,
                    &interaction_ids,
                    &batch_key,
                    terminal_observation,
                    terminal.as_ref(),
                    runtime_termination_reason.as_deref(),
                    finalization,
                    finalization_error.or(completion_error_metadata),
                    None,
                )
                .await?;
            }
            crate::meerkat_machine::driver::InteractionTerminalRecoveryPhase::Finalized {
                events,
                finalization,
                finalization_error,
            } => {
                publish_authorized_runtime_terminal_batch(
                    driver,
                    completions,
                    None,
                    executor,
                    &input_ids,
                    &interaction_ids,
                    &batch_key,
                    terminal_observation,
                    terminal.as_ref(),
                    runtime_termination_reason.as_deref(),
                    finalization,
                    finalization_error.or(completion_error_metadata),
                    Some(&events),
                )
                .await?;
            }
        }
    }
    Ok(())
}

async fn drain_recovered_interaction_terminal_outboxes_until_clear(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    authority_binding: &RuntimeLoopAuthorityBinding,
    turn_boundary_already_held: bool,
    authority_kind: RuntimeProjectionRecoveryAuthority,
) -> bool {
    let _turn_finalization_guard = if turn_boundary_already_held {
        None
    } else {
        match executor.turn_finalization_boundary_handle() {
            Some(boundary) => match boundary.acquire().await {
                Ok(guard) => Some(guard),
                Err(error) => {
                    tracing::error!(
                        %error,
                        "failed to acquire runtime turn-finalization boundary for terminal recovery"
                    );
                    return true;
                }
            },
            None => None,
        }
    };
    let mut delay_ms = 25_u64;
    loop {
        match drain_recovered_interaction_terminal_outboxes(
            driver,
            completions,
            executor,
            authority_binding,
            authority_kind,
        )
        .await
        {
            Ok(()) => return false,
            Err(InteractionTerminalPublicationError::Retryable(error)) => {
                tracing::error!(
                    error = %error,
                    retry_delay_ms = delay_ms,
                    "interaction terminal recovery drain remains pending"
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                delay_ms = delay_ms.saturating_mul(2).min(1_000);
            }
            Err(InteractionTerminalPublicationError::StaleAuthority(error)) => {
                tracing::warn!(
                    error = %error,
                    "interaction terminal recovery stopped after losing runtime authority"
                );
                return true;
            }
            Err(InteractionTerminalPublicationError::Corrupt(error)) => {
                tracing::error!(
                    error = %error,
                    "interaction terminal recovery failed closed on durable corruption"
                );
                return true;
            }
        }
    }
}

/// Repair every durable post-commit projection before an abnormally exited
/// runtime loop is allowed to stop or clean up its exact executor.
///
/// The runtime-loop handoff guard survives task cancellation and retains the
/// executor, but a cancelled loop may have exited after `atomic_apply` and
/// before compaction reconciliation or interaction-terminal publication. The
/// teardown owner must finish those committed boundaries first; otherwise a
/// later runless stop could overwrite waiter delivery while surface cleanup
/// discards the only live session projection.
async fn recover_committed_runtime_projections_before_teardown(
    driver: &crate::meerkat_machine::SharedDriver,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    teardown_slot: &RuntimeLoopTeardownSlot,
    recovery: &RuntimeLoopRecoveryContext,
) -> Result<(), crate::RuntimeDriverError> {
    let _turn_finalization_guard = match executor.turn_finalization_boundary_handle() {
        Some(boundary) => Some(boundary.acquire().await.map_err(|error| {
            crate::RuntimeDriverError::Internal(format!(
                "failed to acquire runtime turn-finalization boundary for teardown recovery: {error}"
            ))
        })?),
        None => None,
    };
    let mut authority_guard = Some(
        recovery
            .authority_binding
            .lock_current_teardown_driver_authority(driver, "runtime teardown projection recovery")
            .await?,
    );
    let rejected_run_projection_abort_completed = match teardown_slot.owned_commit_cleanup_state() {
        Some(OwnedCommitCleanupState::InFlight { run_id }) => {
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "runtime teardown observed owned commit {run_id} before its child published an outcome"
                ),
            });
        }
        Some(OwnedCommitCleanupState::RejectedAbortPending { run_id }) => {
            executor
                .abort_rejected_run_projections()
                .await
                .map_err(|error| {
                    crate::RuntimeDriverError::Internal(format!(
                        "failed to abort rejected runtime run projections during teardown: {error}"
                    ))
                })?;
            teardown_slot.complete_rejected_commit_abort(&run_id);
            true
        }
        None => false,
    };
    let pending_directed_terminalization_run_id = match teardown_slot.owned_terminalization_state()
    {
        Some(OwnedTerminalizationState::InFlight { run_id }) => {
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "runtime teardown observed owned terminalization {run_id} after acquiring its driver authority"
                ),
            });
        }
        Some(OwnedTerminalizationState::PersistedDirectedDrainPending { run_id }) => Some(run_id),
        None => None,
    };
    // A rejected atomic boundary has no committed current-run compaction
    // outbox, and startup drained every older outbox before admitting work.
    // The whole-run abort above therefore already settled the exact live
    // compaction stage before discarding its actor. Do not follow it with the
    // generic empty-outbox reconciliation: custom builders may require that
    // actor, and asking again after discard turns successful cleanup into an
    // unrecoverable `NotFound`/unsupported failure.
    if !rejected_run_projection_abort_completed {
        reconcile_compaction_projection_outbox(driver, executor).await?;
    }

    // A mixed failed batch has two disjoint public recipient sets: durable
    // directed Interaction terminals and a local non-directed completion
    // carrier. Match the live path by converging the durable/public side first;
    // exposing the local result while publication can still block would split
    // one run's observable terminal boundary.
    if let Some(run_id) = pending_directed_terminalization_run_id.as_ref() {
        let first_drain = drain_recovered_interaction_terminal_outboxes_under_authority(
            driver,
            recovery.completions.as_ref(),
            executor,
            authority_guard.as_ref().ok_or_else(|| {
                crate::RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "runtime teardown lost exact authority before directed drain for {run_id}"
                    ),
                }
            })?,
        )
        .await;
        if let Err(error) = first_drain {
            tracing::warn!(
                %run_id,
                %error,
                "runtime teardown released authority before retrying directed terminal drain"
            );
            drop(authority_guard.take());
            if drain_recovered_interaction_terminal_outboxes_until_clear(
                driver,
                recovery.completions.as_ref(),
                executor,
                &recovery.authority_binding,
                true,
                RuntimeProjectionRecoveryAuthority::Teardown,
            )
            .await
            {
                return Err(crate::RuntimeDriverError::Internal(
                    "runtime teardown could not drain persisted directed interaction terminals"
                        .to_string(),
                ));
            }
            authority_guard = Some(
                recovery
                    .authority_binding
                    .lock_current_teardown_driver_authority(
                        driver,
                        "runtime teardown non-directed recovery after directed retry",
                    )
                    .await?,
            );
        }
        teardown_slot.clear_owned_terminalization_directed_drain(run_id)?;
    }

    if let Some(pending) = teardown_slot.pending_nondirected_run_terminal() {
        if pending.phase != PendingNondirectedRunTerminalPhase::Persisted {
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "runtime teardown observed non-directed run {} before its owner published a durable outcome",
                    pending.run_id
                ),
            });
        }
        let completions = recovery.completions.as_ref().ok_or_else(|| {
            crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "persisted non-directed run {} lost its completion registry",
                    pending.run_id
                ),
            }
        })?;
        let batch_key = crate::input_state::InteractionTerminalBatchKey::Run {
            run_id: pending.run_id.clone(),
        };
        publish_authorized_runtime_terminal_batch(
            driver,
            Some(completions),
            Some(authority_guard.take().ok_or_else(|| {
                crate::RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "runtime teardown lost exact authority before non-directed recovery for {}",
                        pending.run_id
                    ),
                }
            })?),
            executor,
            &pending.input_ids,
            &[],
            &batch_key,
            pending.terminal_observation,
            pending.terminal.as_ref(),
            None,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
            pending.completion_error_metadata,
            None,
        )
        .await
        .map_err(|error| {
            crate::RuntimeDriverError::Internal(format!(
                "failed to recover persisted non-directed runtime terminal: {error}"
            ))
        })?;
        teardown_slot.clear_pending_nondirected_run_terminal(&pending.run_id);
    } else {
        drop(authority_guard.take());
    }

    if drain_recovered_interaction_terminal_outboxes_until_clear(
        driver,
        recovery.completions.as_ref(),
        executor,
        &recovery.authority_binding,
        true,
        RuntimeProjectionRecoveryAuthority::Teardown,
    )
    .await
    {
        return Err(crate::RuntimeDriverError::Internal(
            "runtime teardown could not drain committed interaction terminal outboxes".to_string(),
        ));
    }
    Ok(())
}

async fn resolve_machine_terminal_completion_waiters(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    authority_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    teardown_slot: &std::sync::Arc<RuntimeLoopTeardownSlot>,
    input_ids: &[InputId],
    run_id: &RunId,
    reason: String,
) {
    let Some(completions) = completions else {
        return;
    };
    let driver = std::sync::Arc::clone(driver);
    let completions = std::sync::Arc::clone(completions);
    let teardown_slot = std::sync::Arc::clone(teardown_slot);
    let owned_input_ids = input_ids.to_vec();
    let owned_run_id = run_id.clone();
    let handoff = tokio::spawn(async move {
        let _authority_guard = authority_guard;
        let driver_guard = driver.lock_owned().await;
        let completion_guard = completions.lock_owned().await;
        let bundle = machine_terminal_completion_error(&driver_guard, reason)
            .and_then(|error_metadata| {
                crate::meerkat_machine::driver::machine_resolve_runtime_completion_result(
                    &driver_guard,
                    Some(&owned_run_id),
                    crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::MachineTerminal,
                    crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
                )
                .map(|authority| (authority, error_metadata))
            })
            .map_err(|error| {
                crate::completion::CompletionWaitError::AuthorityUnavailable(format!(
                    "runtime terminal completion authority missing: {error}"
                ))
            })
            .and_then(|(authority, error_metadata)| {
                crate::completion::authorize_runtime_terminal_bundle(
                    &[],
                    None,
                    authority,
                    error_metadata,
                    None,
                )
            });
        let _driver_guard = driver_guard;
        let mut completion_guard = completion_guard;
        match bundle {
            Ok(bundle) => {
                completion_guard
                    .resolve_authorized_runtime_terminal_bundle(owned_input_ids, bundle);
            }
            Err(error) => completion_guard.fail_inputs(owned_input_ids, error),
        }
        teardown_slot.clear_pending_nondirected_run_terminal(&owned_run_id);
    });
    if let Err(error) = handoff.await {
        tracing::error!(
            %error,
            "owned machine-terminal completion waiter handoff failed"
        );
    }
}

fn fail_closed_completion_waiters(
    registry: &mut crate::completion::CompletionRegistry,
    input_ids: &[InputId],
    reason: impl Into<String>,
) {
    let reason = reason.into();
    registry.fail_inputs(
        input_ids.iter().cloned(),
        crate::completion::CompletionWaitError::AuthorityUnavailable(reason),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RuntimeLoopTeardownDisposition {
    #[default]
    PreserveRegistration,
    UnregisterRequired,
    /// This exact attachment exited before its serving gate opened.
    ///
    /// No runtime work could have committed through it, so teardown must stop
    /// and clean the failed candidate without replaying the startup projection
    /// recovery that already rejected it. Durable outboxes remain untouched
    /// for a future attachment to reconcile. The exact startup/pending owner,
    /// not this loop's asynchronous watcher, retires the attachment: only that
    /// owner knows whether the service turn-finalization boundary is held.
    UnservedAttachment,
}

impl RuntimeLoopTeardownDisposition {
    pub(crate) fn requires_unregister(self) -> bool {
        matches!(self, Self::UnregisterRequired)
    }

    fn requires_projection_recovery(self) -> bool {
        !matches!(self, Self::UnservedAttachment)
    }
}

struct RuntimeLoopTerminalHandoff {
    stop_completion: Option<crate::effect::StopEffectCompletion>,
    executor_stop_retry_reason: Option<String>,
    deferred_executor_stop_error: Option<crate::RuntimeDriverError>,
    disposition: RuntimeLoopTeardownDisposition,
}

impl Default for RuntimeLoopTerminalHandoff {
    fn default() -> Self {
        Self {
            stop_completion: None,
            executor_stop_retry_reason: Some(
                "runtime loop exited without a canonical terminal stop".into(),
            ),
            deferred_executor_stop_error: None,
            disposition: RuntimeLoopTeardownDisposition::PreserveRegistration,
        }
    }
}

struct RuntimeLoopExitHandoff {
    executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    terminal: RuntimeLoopTerminalHandoff,
}

enum RuntimeLoopTeardownState {
    Pending,
    Ready(RuntimeLoopExitHandoff),
    Cleaning,
    Cleaned(Option<RuntimeLoopCleanupReceipt>),
}

/// Shared mechanical handoff from the runtime-loop body to the machine-owned
/// cleanup coordinator.
///
/// The loop deposits its exact executor here as its final action. The
/// coordinator, not a caller future and not a detached surface task, owns the
/// required cleanup. A failed cleanup restores the executor into `Ready`, so a
/// later explicit stop/unregister retry can discharge the same obligation.
pub(crate) struct RuntimeLoopTeardownSlot {
    state: std::sync::Mutex<RuntimeLoopTeardownState>,
    last_unregister_result: std::sync::Mutex<Option<Result<(), crate::RuntimeDriverError>>>,
    disposition: std::sync::Mutex<RuntimeLoopTeardownDisposition>,
    recovery_context: Option<RuntimeLoopRecoveryContext>,
    owned_commit_cleanup_state: std::sync::Mutex<Option<OwnedCommitCleanupState>>,
    owned_terminalization_state: std::sync::Mutex<Option<OwnedTerminalizationState>>,
    pending_nondirected_run_terminal: std::sync::Mutex<Option<PendingNondirectedRunTerminal>>,
    changed: tokio::sync::Notify,
}

#[derive(Clone)]
struct RuntimeLoopRecoveryContext {
    authority_binding: RuntimeLoopAuthorityBinding,
    completions: Option<crate::meerkat_machine::SharedCompletionRegistry>,
}

/// Exact in-process recovery carrier for a persisted run outcome that has
/// completion waiters but no directed Interaction event/outbox. The carrier is
/// installed before the owning persistence await and is cleared only after
/// authorized waiter delivery. Abnormal teardown can therefore replay the
/// persisted payload with the retained executor instead of reclassifying it as
/// a runless runtime termination.
#[derive(Clone)]
struct PendingNondirectedRunTerminal {
    phase: PendingNondirectedRunTerminalPhase,
    input_ids: Vec<InputId>,
    run_id: RunId,
    terminal: Option<CoreApplyTerminal>,
    terminal_observation: crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation,
    completion_error_metadata: Option<meerkat_core::TurnErrorMetadata>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingNondirectedRunTerminalPhase {
    PreparingPersistence,
    Persisted,
}

#[derive(Clone, PartialEq, Eq)]
enum OwnedCommitCleanupState {
    InFlight { run_id: RunId },
    RejectedAbortPending { run_id: RunId },
}

#[derive(Clone, PartialEq, Eq)]
enum OwnedTerminalizationState {
    InFlight { run_id: RunId },
    PersistedDirectedDrainPending { run_id: RunId },
}

impl RuntimeLoopTeardownSlot {
    #[cfg(test)]
    pub(crate) fn pending() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            state: std::sync::Mutex::new(RuntimeLoopTeardownState::Pending),
            last_unregister_result: std::sync::Mutex::new(None),
            disposition: std::sync::Mutex::new(
                RuntimeLoopTeardownDisposition::PreserveRegistration,
            ),
            recovery_context: None,
            owned_commit_cleanup_state: std::sync::Mutex::new(None),
            owned_terminalization_state: std::sync::Mutex::new(None),
            pending_nondirected_run_terminal: std::sync::Mutex::new(None),
            changed: tokio::sync::Notify::new(),
        })
    }

    fn pending_with_recovery(
        authority_binding: RuntimeLoopAuthorityBinding,
        completions: Option<crate::meerkat_machine::SharedCompletionRegistry>,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            state: std::sync::Mutex::new(RuntimeLoopTeardownState::Pending),
            last_unregister_result: std::sync::Mutex::new(None),
            disposition: std::sync::Mutex::new(
                RuntimeLoopTeardownDisposition::PreserveRegistration,
            ),
            recovery_context: Some(RuntimeLoopRecoveryContext {
                authority_binding,
                completions,
            }),
            owned_commit_cleanup_state: std::sync::Mutex::new(None),
            owned_terminalization_state: std::sync::Mutex::new(None),
            pending_nondirected_run_terminal: std::sync::Mutex::new(None),
            changed: tokio::sync::Notify::new(),
        })
    }

    fn stage_owned_commit_in_flight(
        &self,
        run_id: &RunId,
    ) -> Result<(), crate::RuntimeDriverError> {
        let mut state = self
            .owned_commit_cleanup_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(current) = state.as_ref() {
            let current_run_id = match current {
                OwnedCommitCleanupState::InFlight { run_id }
                | OwnedCommitCleanupState::RejectedAbortPending { run_id } => run_id,
            };
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "runtime run {run_id} attempted to replace owned commit cleanup state for {current_run_id}"
                ),
            });
        }
        *state = Some(OwnedCommitCleanupState::InFlight {
            run_id: run_id.clone(),
        });
        Ok(())
    }

    fn mark_owned_commit_rejected_abort_pending(
        &self,
        run_id: &RunId,
    ) -> Result<(), crate::RuntimeDriverError> {
        let mut state = self
            .owned_commit_cleanup_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(
            state.as_ref(),
            Some(OwnedCommitCleanupState::InFlight { run_id: current }) if current == run_id
        ) {
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "rejected runtime commit {run_id} lost its in-flight cleanup carrier"
                ),
            });
        }
        *state = Some(OwnedCommitCleanupState::RejectedAbortPending {
            run_id: run_id.clone(),
        });
        Ok(())
    }

    fn clear_owned_commit_in_flight(
        &self,
        run_id: &RunId,
    ) -> Result<(), crate::RuntimeDriverError> {
        let mut state = self
            .owned_commit_cleanup_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(
            state.as_ref(),
            Some(OwnedCommitCleanupState::InFlight { run_id: current }) if current == run_id
        ) {
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "successful runtime commit {run_id} lost its in-flight cleanup carrier"
                ),
            });
        }
        *state = None;
        Ok(())
    }

    fn complete_rejected_commit_abort(&self, run_id: &RunId) {
        let mut state = self
            .owned_commit_cleanup_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(
            state.as_ref(),
            Some(OwnedCommitCleanupState::RejectedAbortPending { run_id: current }) if current == run_id
        ) {
            *state = None;
        }
        drop(state);
        self.clear_pending_nondirected_run_terminal(run_id);
    }

    fn owned_commit_cleanup_state(&self) -> Option<OwnedCommitCleanupState> {
        self.owned_commit_cleanup_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn stage_owned_terminalization_in_flight(
        &self,
        run_id: &RunId,
    ) -> Result<(), crate::RuntimeDriverError> {
        let mut state = self
            .owned_terminalization_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(current) = state.as_ref() {
            let current_run_id = match current {
                OwnedTerminalizationState::InFlight { run_id }
                | OwnedTerminalizationState::PersistedDirectedDrainPending { run_id } => run_id,
            };
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "runtime run {run_id} attempted to replace owned terminalization for {current_run_id}"
                ),
            });
        }
        *state = Some(OwnedTerminalizationState::InFlight {
            run_id: run_id.clone(),
        });
        Ok(())
    }

    fn clear_owned_terminalization_in_flight(
        &self,
        run_id: &RunId,
    ) -> Result<(), crate::RuntimeDriverError> {
        let mut state = self
            .owned_terminalization_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(
            state.as_ref(),
            Some(OwnedTerminalizationState::InFlight { run_id: current }) if current == run_id
        ) {
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "runtime run {run_id} lost its owned terminalization cleanup carrier"
                ),
            });
        }
        *state = None;
        Ok(())
    }

    fn mark_owned_terminalization_directed_drain_pending(
        &self,
        run_id: &RunId,
    ) -> Result<(), crate::RuntimeDriverError> {
        let mut state = self
            .owned_terminalization_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(
            state.as_ref(),
            Some(OwnedTerminalizationState::InFlight { run_id: current }) if current == run_id
        ) {
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "runtime run {run_id} lost its in-flight terminalization before directed drain handoff"
                ),
            });
        }
        *state = Some(OwnedTerminalizationState::PersistedDirectedDrainPending {
            run_id: run_id.clone(),
        });
        Ok(())
    }

    fn clear_owned_terminalization_directed_drain(
        &self,
        run_id: &RunId,
    ) -> Result<(), crate::RuntimeDriverError> {
        let mut state = self
            .owned_terminalization_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(
            state.as_ref(),
            Some(OwnedTerminalizationState::PersistedDirectedDrainPending { run_id: current }) if current == run_id
        ) {
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "runtime run {run_id} lost its persisted directed terminal drain carrier"
                ),
            });
        }
        *state = None;
        Ok(())
    }

    fn owned_terminalization_state(&self) -> Option<OwnedTerminalizationState> {
        self.owned_terminalization_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn stage_prepared_nondirected_run_terminal(
        &self,
        terminal: PendingNondirectedRunTerminal,
    ) -> Result<(), crate::RuntimeDriverError> {
        let mut pending = self
            .pending_nondirected_run_terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(current) = pending.as_ref() {
            return Err(crate::RuntimeDriverError::Internal(format!(
                "runtime loop attempted to replace pending non-directed terminal for run {} with {}",
                current.run_id, terminal.run_id
            )));
        }
        *pending = Some(terminal);
        Ok(())
    }

    fn mark_nondirected_run_terminal_persisted(
        &self,
        run_id: &RunId,
    ) -> Result<(), crate::RuntimeDriverError> {
        let mut pending = self
            .pending_nondirected_run_terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(terminal) = pending.as_mut() else {
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "persisted non-directed run {run_id} lost its in-process terminal carrier"
                ),
            });
        };
        if &terminal.run_id != run_id
            || terminal.phase != PendingNondirectedRunTerminalPhase::PreparingPersistence
        {
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "persisted non-directed run {run_id} found mismatched terminal carrier for {}",
                    terminal.run_id
                ),
            });
        }
        terminal.phase = PendingNondirectedRunTerminalPhase::Persisted;
        Ok(())
    }

    fn set_nondirected_run_terminal_completion_error(
        &self,
        run_id: &RunId,
        completion_error_metadata: Option<meerkat_core::TurnErrorMetadata>,
    ) -> Result<(), crate::RuntimeDriverError> {
        let mut pending = self
            .pending_nondirected_run_terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(terminal) = pending.as_mut() else {
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "non-directed terminalization for run {run_id} lost its in-process carrier"
                ),
            });
        };
        if &terminal.run_id != run_id
            || terminal.phase != PendingNondirectedRunTerminalPhase::PreparingPersistence
        {
            return Err(crate::RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "non-directed terminalization for run {run_id} found mismatched carrier for {}",
                    terminal.run_id
                ),
            });
        }
        terminal.completion_error_metadata = completion_error_metadata;
        Ok(())
    }

    fn pending_nondirected_run_terminal(&self) -> Option<PendingNondirectedRunTerminal> {
        self.pending_nondirected_run_terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn clear_pending_nondirected_run_terminal(&self, run_id: &RunId) {
        let mut pending = self
            .pending_nondirected_run_terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending
            .as_ref()
            .is_some_and(|terminal| &terminal.run_id == run_id)
        {
            *pending = None;
        }
    }

    pub(crate) fn disposition(&self) -> RuntimeLoopTeardownDisposition {
        *self
            .disposition
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn last_unregister_result(&self) -> Option<Result<(), crate::RuntimeDriverError>> {
        self.last_unregister_result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn clear_last_unregister_result(&self) {
        *self
            .last_unregister_result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn publish(&self, mut handoff: RuntimeLoopExitHandoff) {
        let mut retained_receipt = None;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*state, RuntimeLoopTeardownState::Pending) {
            *self
                .disposition
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = handoff.terminal.disposition;
            // The owned unregister worker can fail while the loop is still in
            // CoreExecutor::apply (for example, persisting BeginUnregister may
            // fail before it takes the attachment). Reconcile that retained
            // result while publishing under the same lock order used by
            // acknowledge_unregister_result, otherwise the stop caller can
            // wait forever on a receipt that arrived one instruction late.
            let retained_result = self
                .last_unregister_result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(result) = retained_result {
                retained_receipt = handoff
                    .terminal
                    .stop_completion
                    .take()
                    .map(|stop_completion| {
                        (
                            RuntimeLoopCleanupReceipt {
                                stop_completion: Some(stop_completion),
                            },
                            result,
                        )
                    });
            }
            *state = RuntimeLoopTeardownState::Ready(handoff);
            drop(state);
            self.changed.notify_waiters();
        }
        if let Some((receipt, result)) = retained_receipt {
            // Only the acknowledgement is consumed. The exact executor and
            // its default stop-hook retry obligation remain Ready for an
            // explicit retry of the failed unregister saga.
            receipt.acknowledge(result);
        }
    }

    pub(crate) async fn wait_until_published(&self) {
        loop {
            let mut changed = std::pin::pin!(self.changed.notified());
            changed.as_mut().enable();
            let pending = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_pending();
            if !pending {
                return;
            }
            changed.await;
        }
    }

    /// Run the external executor cleanup once. The exact stop acknowledgement
    /// remains retained in the slot after success until the owned unregister
    /// supervisor publishes the saga's final typed result.
    pub(crate) async fn cleanup_once(
        &self,
        driver: &crate::meerkat_machine::SharedDriver,
    ) -> Result<(), crate::RuntimeDriverError> {
        enum CleanupClaim {
            Handoff(RuntimeLoopExitHandoff),
            AlreadyCleaned,
            Wait,
        }
        loop {
            let mut changed = std::pin::pin!(self.changed.notified());
            changed.as_mut().enable();
            let claim = {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match std::mem::replace(&mut *state, RuntimeLoopTeardownState::Cleaning) {
                    RuntimeLoopTeardownState::Ready(handoff) => CleanupClaim::Handoff(handoff),
                    RuntimeLoopTeardownState::Cleaned(receipt) => {
                        *state = RuntimeLoopTeardownState::Cleaned(receipt);
                        CleanupClaim::AlreadyCleaned
                    }
                    RuntimeLoopTeardownState::Pending => {
                        *state = RuntimeLoopTeardownState::Pending;
                        CleanupClaim::Wait
                    }
                    RuntimeLoopTeardownState::Cleaning => {
                        *state = RuntimeLoopTeardownState::Cleaning;
                        CleanupClaim::Wait
                    }
                }
            };
            let handoff = match claim {
                CleanupClaim::Handoff(handoff) => handoff,
                CleanupClaim::AlreadyCleaned => return Ok(()),
                CleanupClaim::Wait => {
                    changed.await;
                    continue;
                }
            };

            let mut cleanup_guard = RuntimeLoopCleanupGuard::new(self, handoff);
            let (requires_abnormal_exit_recovery, requires_projection_recovery) = {
                let terminal = &cleanup_guard.handoff_mut()?.terminal;
                (
                    terminal.executor_stop_retry_reason.is_some()
                        || terminal.deferred_executor_stop_error.is_some()
                        || self.owned_commit_cleanup_state().is_some()
                        || self.owned_terminalization_state().is_some()
                        || self.pending_nondirected_run_terminal().is_some(),
                    terminal.disposition.requires_projection_recovery(),
                )
            };
            if requires_projection_recovery
                && requires_abnormal_exit_recovery
                && let Some(recovery_context) = self.recovery_context.as_ref()
                && let Err(error) = recover_committed_runtime_projections_before_teardown(
                    driver,
                    cleanup_guard.handoff_mut()?.executor.as_mut(),
                    self,
                    recovery_context,
                )
                .await
            {
                cleanup_guard.fail_stop_completion(error.clone())?;
                return Err(error);
            }
            if let Some(error) = cleanup_guard
                .handoff_mut()?
                .terminal
                .deferred_executor_stop_error
                .take()
            {
                cleanup_guard.fail_stop_completion(error.clone())?;
                return Err(error);
            }
            if let Some(reason) = cleanup_guard
                .handoff_mut()?
                .terminal
                .executor_stop_retry_reason
                .clone()
            {
                if let Err(error) = crate::control_plane::apply_runtime_executor_stop_hook(
                    cleanup_guard.handoff_mut()?.executor.as_mut(),
                    reason,
                )
                .await
                .map_err(|error| {
                    crate::RuntimeDriverError::Internal(format!(
                        "failed to retry stop-runtime-executor effect: {error}"
                    ))
                }) {
                    cleanup_guard.fail_stop_completion(error.clone())?;
                    return Err(error);
                }
                cleanup_guard
                    .handoff_mut()?
                    .terminal
                    .executor_stop_retry_reason = None;
            }
            let publication_handle = cleanup_guard.handoff_mut()?.executor.publication_handle();
            let completions = self
                .recovery_context
                .as_ref()
                .and_then(|recovery| recovery.completions.as_ref());
            if let Err(error) = crate::control_plane::terminalize_async_stop(
                driver,
                completions,
                publication_handle,
                None,
            )
            .await
            {
                cleanup_guard.fail_stop_completion(error.clone())?;
                return Err(error);
            }
            let cleanup_result = cleanup_guard
                .handoff_mut()?
                .executor
                .cleanup_after_runtime_stop_terminalized()
                .await
                .map_err(|error| {
                    crate::RuntimeDriverError::Internal(format!(
                        "required executor cleanup failed after durable runtime stop: {error}"
                    ))
                });
            match cleanup_result {
                Ok(()) => {
                    let mut handoff = cleanup_guard.complete()?;
                    let receipt = RuntimeLoopCleanupReceipt {
                        stop_completion: handoff.terminal.stop_completion.take(),
                    };
                    *self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        RuntimeLoopTeardownState::Cleaned(Some(receipt));
                    self.changed.notify_waiters();
                    return Ok(());
                }
                Err(error) => {
                    cleanup_guard.fail_stop_completion(error.clone())?;
                    return Err(error);
                }
            }
        }
    }

    /// Publish the exact saga result to the retained stop acknowledgement.
    /// On worker panic before cleanup completes, fail the acknowledgement but
    /// leave the executor in `Ready` for an explicit unregister retry.
    pub(crate) fn acknowledge_unregister_result(
        &self,
        result: Result<(), crate::RuntimeDriverError>,
    ) {
        let receipt = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *self
                .last_unregister_result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result.clone());
            match &mut *state {
                RuntimeLoopTeardownState::Cleaned(receipt) => receipt.take(),
                RuntimeLoopTeardownState::Ready(handoff) => {
                    let stop_completion = handoff.terminal.stop_completion.take();
                    stop_completion.map(|stop_completion| RuntimeLoopCleanupReceipt {
                        stop_completion: Some(stop_completion),
                    })
                }
                RuntimeLoopTeardownState::Pending | RuntimeLoopTeardownState::Cleaning => None,
            }
        };
        if let Some(receipt) = receipt {
            receipt.acknowledge(result);
        }
    }

    /// Publish an ordinary-stop coordinator result to the exact stop request
    /// without recording unregister terminal truth. Completed cleanup remains
    /// retryable through the entry-owned stop coordinator.
    pub(crate) fn acknowledge_runtime_stop_result(
        &self,
        result: Result<(), crate::RuntimeDriverError>,
    ) {
        let receipt = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match &mut *state {
                RuntimeLoopTeardownState::Cleaned(receipt) => receipt.take(),
                RuntimeLoopTeardownState::Ready(handoff) => handoff
                    .terminal
                    .stop_completion
                    .take()
                    .map(|stop_completion| RuntimeLoopCleanupReceipt {
                        stop_completion: Some(stop_completion),
                    }),
                RuntimeLoopTeardownState::Pending | RuntimeLoopTeardownState::Cleaning => None,
            }
        };
        if let Some(receipt) = receipt {
            receipt.acknowledge(result);
        }
    }
}

impl RuntimeLoopTeardownState {
    fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }
}

/// Exact acknowledgement retained until cleanup and unregister durability have
/// both reached a typed result.
pub(crate) struct RuntimeLoopCleanupReceipt {
    stop_completion: Option<crate::effect::StopEffectCompletion>,
}

impl RuntimeLoopCleanupReceipt {
    pub(crate) fn acknowledge(mut self, result: Result<(), crate::RuntimeDriverError>) {
        if let Some(stop_completion) = self.stop_completion.take() {
            let _ = stop_completion.send(result);
        }
    }
}

struct RuntimeLoopCleanupGuard<'a> {
    slot: &'a RuntimeLoopTeardownSlot,
    handoff: Option<RuntimeLoopExitHandoff>,
}

impl<'a> RuntimeLoopCleanupGuard<'a> {
    fn new(slot: &'a RuntimeLoopTeardownSlot, handoff: RuntimeLoopExitHandoff) -> Self {
        Self {
            slot,
            handoff: Some(handoff),
        }
    }

    fn handoff_mut(&mut self) -> Result<&mut RuntimeLoopExitHandoff, crate::RuntimeDriverError> {
        self.handoff.as_mut().ok_or_else(|| {
            crate::RuntimeDriverError::Internal(
                "runtime-loop cleanup guard lost its handoff".into(),
            )
        })
    }

    fn fail_stop_completion(
        &mut self,
        error: crate::RuntimeDriverError,
    ) -> Result<(), crate::RuntimeDriverError> {
        if let Some(stop_completion) = self.handoff_mut()?.terminal.stop_completion.take() {
            let _ = stop_completion.send(Err(error));
        }
        Ok(())
    }

    fn complete(mut self) -> Result<RuntimeLoopExitHandoff, crate::RuntimeDriverError> {
        self.handoff.take().ok_or_else(|| {
            crate::RuntimeDriverError::Internal("runtime-loop cleanup guard completed twice".into())
        })
    }
}

impl Drop for RuntimeLoopCleanupGuard<'_> {
    fn drop(&mut self) {
        let Some(handoff) = self.handoff.take() else {
            return;
        };
        *self
            .slot
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            RuntimeLoopTeardownState::Ready(handoff);
        self.slot.changed.notify_waiters();
    }
}

struct RuntimeLoopHandoffGuard {
    slot: std::sync::Arc<RuntimeLoopTeardownSlot>,
    executor: Option<Box<dyn meerkat_core::lifecycle::CoreExecutor>>,
    fallback_disposition: RuntimeLoopTeardownDisposition,
}

impl RuntimeLoopHandoffGuard {
    fn new(
        slot: std::sync::Arc<RuntimeLoopTeardownSlot>,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) -> Self {
        Self {
            slot,
            executor: Some(executor),
            // The guard is constructed before the loop task is spawned.  An
            // executor therefore remains unserved until the second startup
            // gate is observed inside the task, including when the spawned
            // future is aborted before its first poll.
            fallback_disposition: RuntimeLoopTeardownDisposition::UnservedAttachment,
        }
    }

    fn set_fallback_disposition(&mut self, disposition: RuntimeLoopTeardownDisposition) {
        self.fallback_disposition = disposition;
    }

    fn executor_mut(
        &mut self,
    ) -> Result<&mut (dyn meerkat_core::lifecycle::CoreExecutor + '_), crate::RuntimeDriverError>
    {
        match self.executor.as_mut() {
            Some(executor) => Ok(executor.as_mut()),
            None => Err(crate::RuntimeDriverError::Internal(
                "runtime-loop handoff guard lost its executor".into(),
            )),
        }
    }

    fn publish(
        &mut self,
        terminal: RuntimeLoopTerminalHandoff,
    ) -> Result<(), crate::RuntimeDriverError> {
        let executor = self.executor.take().ok_or_else(|| {
            crate::RuntimeDriverError::Internal(
                "runtime-loop handoff guard published its executor twice".into(),
            )
        })?;
        self.slot
            .publish(RuntimeLoopExitHandoff { executor, terminal });
        Ok(())
    }
}

impl Drop for RuntimeLoopHandoffGuard {
    fn drop(&mut self) {
        if let Some(executor) = self.executor.take() {
            self.slot.publish(RuntimeLoopExitHandoff {
                executor,
                terminal: RuntimeLoopTerminalHandoff {
                    disposition: self.fallback_disposition,
                    ..RuntimeLoopTerminalHandoff::default()
                },
            });
        }
    }
}

pub(crate) struct SpawnedRuntimeLoop {
    pub(crate) loop_handle: tokio::task::JoinHandle<()>,
    pub(crate) teardown_slot: std::sync::Arc<RuntimeLoopTeardownSlot>,
    pub(crate) startup: std::sync::Arc<RuntimeLoopStartupSlot>,
    pub(crate) startup_authority_transfer: Option<RuntimeLoopStartupAuthorityTransfer>,
    pub(crate) serving_release: Option<RuntimeLoopServingRelease>,
}

impl SpawnedRuntimeLoop {
    pub(crate) fn startup_slot(&self) -> std::sync::Arc<RuntimeLoopStartupSlot> {
        std::sync::Arc::clone(&self.startup)
    }

    pub(crate) fn take_startup_authority_transfer(
        &mut self,
    ) -> Result<RuntimeLoopStartupAuthorityTransfer, crate::RuntimeDriverError> {
        self.startup_authority_transfer.take().ok_or_else(|| {
            crate::RuntimeDriverError::Internal(
                "runtime-loop startup authority transfer was already taken".to_string(),
            )
        })
    }

    pub(crate) fn take_serving_release(
        &mut self,
    ) -> Result<RuntimeLoopServingRelease, crate::RuntimeDriverError> {
        self.serving_release.take().ok_or_else(|| {
            crate::RuntimeDriverError::Internal(
                "runtime-loop serving release was already taken".to_string(),
            )
        })
    }
}

/// Exact second phase of runtime-loop startup.
///
/// Startup reconciliation may inspect the executor, but the loop cannot enter
/// its work-select until the attachment owner has committed every mechanical
/// sidecar associated with that exact attachment.
pub(crate) struct RuntimeLoopServingRelease {
    sender: tokio::sync::oneshot::Sender<()>,
}

impl RuntimeLoopServingRelease {
    pub(crate) fn release(self) -> Result<(), crate::RuntimeDriverError> {
        self.sender.send(()).map_err(|()| {
            crate::RuntimeDriverError::Internal(
                "runtime loop exited before its attachment became serving".to_string(),
            )
        })
    }

    #[cfg(test)]
    pub(crate) fn closed_receiver_for_test() -> Self {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        drop(receiver);
        Self { sender }
    }
}

pub(crate) struct RuntimeLoopStartupAuthorityTransfer {
    sender: tokio::sync::oneshot::Sender<Option<crate::tokio::sync::OwnedMutexGuard<()>>>,
}

impl RuntimeLoopStartupAuthorityTransfer {
    pub(crate) fn send(
        self,
        authority_guard: Option<crate::tokio::sync::OwnedMutexGuard<()>>,
    ) -> Result<(), crate::RuntimeDriverError> {
        self.sender.send(authority_guard).map_err(|_| {
            crate::RuntimeDriverError::Internal(
                "runtime-loop exited before accepting startup authority".to_string(),
            )
        })
    }
}

/// Publishes exactly one typed startup result for the runtime-loop attachment.
///
/// The guard is constructed before the loop task is spawned, so aborting the
/// task before its first poll still fails the waiter instead of leaving
/// `ensure_session_with_executor` blocked forever.
struct RuntimeLoopStartupGuard {
    slot: std::sync::Arc<RuntimeLoopStartupSlot>,
    published: bool,
}

impl RuntimeLoopStartupGuard {
    fn new(slot: std::sync::Arc<RuntimeLoopStartupSlot>) -> Self {
        Self {
            slot,
            published: false,
        }
    }

    fn publish(&mut self, result: Result<(), String>) {
        self.slot.publish(result);
        self.published = true;
    }
}

impl Drop for RuntimeLoopStartupGuard {
    fn drop(&mut self) {
        if !self.published {
            self.slot.publish(Err(
                "runtime loop exited before startup reconciliation completed".to_string(),
            ));
        }
    }
}

/// Mechanical readiness handoff from the loop to its attachment caller.
///
/// Attachment publication alone is not readiness: the loop first has to
/// reconcile the durable compaction projection outbox through its exact
/// executor. The slot keeps that boundary cancellation-safe and race-free.
pub(crate) struct RuntimeLoopStartupSlot {
    result: std::sync::Mutex<Option<Result<(), String>>>,
    changed: tokio::sync::Notify,
}

impl RuntimeLoopStartupSlot {
    fn pending() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            result: std::sync::Mutex::new(None),
            changed: tokio::sync::Notify::new(),
        })
    }

    fn publish(&self, result: Result<(), String>) {
        let mut current = self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current.is_some() {
            return;
        }
        *current = Some(result);
        drop(current);
        self.changed.notify_waiters();
    }

    pub(crate) async fn wait(&self) -> Result<(), crate::RuntimeDriverError> {
        loop {
            let mut changed = std::pin::pin!(self.changed.notified());
            changed.as_mut().enable();
            if let Some(result) = self
                .result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
            {
                return result.map_err(crate::RuntimeDriverError::Internal);
            }
            changed.await;
        }
    }
}

async fn apply_runtime_loop_effect_and_record_handoff(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    mut effect: crate::effect::RuntimeEffect,
    handoff: &mut RuntimeLoopTerminalHandoff,
    turn_finalization_guard: Option<
        Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>,
    >,
) -> bool {
    let effect_is_stop = effect.is_stop();
    let stop_completion = effect.take_stop_completion();
    match crate::control_plane::apply_runtime_loop_executor_effect(
        driver,
        completions,
        executor,
        effect,
        turn_finalization_guard,
    )
    .await
    {
        Ok(crate::control_plane::RuntimeLoopEffectOutcome::Continue) => false,
        Ok(
            crate::control_plane::RuntimeLoopEffectOutcome::StopTerminalizedNeedsExternalCleanup,
        ) => {
            handoff.stop_completion = stop_completion;
            handoff.executor_stop_retry_reason = None;
            handoff.deferred_executor_stop_error = None;
            true
        }
        Err(failure) => {
            tracing::error!(
                error = %failure.error,
                "failed to apply runtime-loop executor effect"
            );
            record_runtime_loop_effect_failure_handoff(
                handoff,
                stop_completion,
                effect_is_stop,
                failure,
            );
            true
        }
    }
}

fn record_runtime_loop_effect_failure_handoff(
    handoff: &mut RuntimeLoopTerminalHandoff,
    stop_completion: Option<crate::effect::StopEffectCompletion>,
    effect_is_stop: bool,
    failure: crate::control_plane::RuntimeLoopEffectFailure,
) {
    handoff.stop_completion = stop_completion;
    match (failure.executor_stop_retry_reason, effect_is_stop) {
        (Some(reason), _) => {
            // Preserve the existing stop-hook retry contract: the first
            // cleanup attempt reports the original stop error, while the
            // retained exact executor remains available for an explicit retry
            // of that same stop request.
            handoff.executor_stop_retry_reason = Some(reason);
            handoff.deferred_executor_stop_error = Some(failure.error);
        }
        (None, true) => {
            // The stop hook succeeded, but machine terminalization failed.
            // Machine-managed executors now retain the exact mutation fence;
            // invoking the stop hook again would try to reacquire that same
            // non-reentrant fence. Preserve the established cleanup contract:
            // the retained handoff immediately retries terminalization without
            // repeating Stop.
            handoff.executor_stop_retry_reason = None;
            handoff.deferred_executor_stop_error = None;
        }
        (None, false) => {
            // A failed non-stop effect also exits the runtime loop, but it has
            // not yet crossed the canonical executor-stop seam. Retain that
            // obligation so post-loop cleanup first runs the stop hook (and,
            // for machine-managed executors, mints the exact post-stop
            // mutation fence) instead of invoking cleanup against an executor
            // that is still AwaitingStop.
            handoff.executor_stop_retry_reason = Some(format!(
                "runtime executor effect failed before canonical stop: {}",
                failure.error
            ));
            handoff.deferred_executor_stop_error = None;
        }
    }
}

async fn stop_runtime_loop_executor_from_dsl_effect(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    reason: String,
    handoff: &mut RuntimeLoopTerminalHandoff,
    turn_finalization_guard: Option<
        Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>,
    >,
) -> bool {
    let authority = {
        let driver = driver.lock().await;
        driver.shared_dsl_authority()
    };

    let effects = match crate::meerkat_machine::apply_dsl_transition_on_authority(
        &authority,
        crate::meerkat_machine::dsl::MeerkatMachineInput::StopRuntimeExecutor {
            reason: reason.clone(),
        },
        "RuntimeLoopStopRuntimeExecutor",
    ) {
        Ok(effects) => effects,
        Err(error) => {
            tracing::error!(
                error = %error,
                "failed to apply DSL stop-runtime-executor transition after runtime loop snapshot failure"
            );
            handoff.executor_stop_retry_reason = Some(reason);
            return true;
        }
    };

    let projected_effect = match crate::effect::runtime_effect_projection_from_dsl_effects(&effects)
    {
        Ok(effect) => effect,
        Err(error) => {
            tracing::error!(
                error = %error,
                "DSL stop-runtime-executor transition did not emit a runtime effect fact"
            );
            handoff.executor_stop_retry_reason = Some(reason);
            return true;
        }
    };

    apply_runtime_loop_effect_and_record_handoff(
        driver,
        completions,
        executor,
        projected_effect.into_effect(),
        handoff,
        turn_finalization_guard,
    )
    .await
}

fn fail_completion_waiters(
    registry: &mut crate::completion::CompletionRegistry,
    input_ids: &[InputId],
    reason: impl Into<String>,
) {
    fail_closed_completion_waiters(registry, input_ids, reason);
}

fn machine_terminal_completion_error(
    driver: &crate::meerkat_machine::driver::DriverEntry,
    detail: String,
) -> Result<Option<meerkat_core::TurnErrorMetadata>, crate::RuntimeDriverError> {
    let authority = driver.shared_dsl_authority();
    let auth = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let outcome = auth.state().terminal_outcome.ok_or_else(|| {
        crate::RuntimeDriverError::Internal(
            "missing generated terminal_outcome for runtime completion".to_string(),
        )
    })?;
    match outcome {
        crate::meerkat_machine::dsl::TurnTerminalOutcome::Cancelled => {
            match auth.state().terminal_cause_kind {
                None | Some(crate::meerkat_machine::dsl::TurnTerminalCauseKind::Unknown) => {
                    Ok(None)
                }
                Some(cause_kind) => Err(crate::RuntimeDriverError::Internal(format!(
                    "generated cancelled terminal carried failure cause {cause_kind:?}"
                ))),
            }
        }
        crate::meerkat_machine::dsl::TurnTerminalOutcome::Failed
        | crate::meerkat_machine::dsl::TurnTerminalOutcome::BudgetExhausted
        | crate::meerkat_machine::dsl::TurnTerminalOutcome::TimeBudgetExceeded
        | crate::meerkat_machine::dsl::TurnTerminalOutcome::StructuredOutputValidationFailed => {
            let cause_kind = auth.state().terminal_cause_kind.ok_or_else(|| {
                crate::RuntimeDriverError::Internal(
                    "missing generated terminal_cause_kind for failed runtime completion"
                        .to_string(),
                )
            })?;
            if cause_kind == crate::meerkat_machine::dsl::TurnTerminalCauseKind::Unknown {
                return Err(crate::RuntimeDriverError::Internal(
                    "unknown generated terminal_cause_kind for failed runtime completion"
                        .to_string(),
                ));
            }
            Ok(Some(meerkat_core::TurnErrorMetadata::terminal(
                meerkat_core::TurnTerminalCauseKind::from(cause_kind),
                meerkat_core::TurnTerminalOutcome::from(outcome),
                detail,
            )))
        }
        crate::meerkat_machine::dsl::TurnTerminalOutcome::None
        | crate::meerkat_machine::dsl::TurnTerminalOutcome::Completed => {
            Err(crate::RuntimeDriverError::Internal(format!(
                "generated terminal outcome {outcome:?} cannot resolve failed runtime completion"
            )))
        }
    }
}

fn primitive_admitted_content_shape(primitive: &RunPrimitive) -> TurnContentShape {
    match primitive {
        RunPrimitive::StagedInput(staged) => TurnContentShape::from_staged_presence(
            !staged.appends.is_empty(),
            !staged.context_appends.is_empty(),
        ),
        RunPrimitive::ImmediateAppend(_) => TurnContentShape::ImmediateAppend,
        RunPrimitive::ImmediateContextAppend(_) => TurnContentShape::ImmediateContext,
        _ => TurnContentShape::Conversation,
    }
}

fn primitive_turn_start_input(
    run_id: &RunId,
    primitive: &RunPrimitive,
) -> Option<crate::meerkat_machine::dsl::MeerkatMachineInput> {
    match primitive {
        RunPrimitive::ImmediateAppend(_) => Some(
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartImmediateAppend {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
            },
        ),
        RunPrimitive::ImmediateContextAppend(_) => Some(
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartImmediateContext {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
            },
        ),
        RunPrimitive::StagedInput(_) if primitive.is_peer_response_terminal_context_and_run() => {
            let admitted_content_shape = crate::meerkat_machine::dsl::ContentShape::from(
                primitive_admitted_content_shape(primitive),
            );
            Some(
                crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                    primitive_kind:
                        crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn,
                    admitted_content_shape,
                    vision_enabled: false,
                    image_tool_results_enabled: false,
                    max_extraction_retries: 0,
                },
            )
        }
        RunPrimitive::StagedInput(_) if primitive.is_context_only_apply_without_turn() => Some(
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartImmediateContext {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
            },
        ),
        RunPrimitive::StagedInput(staged) if staged.appends.is_empty() => None,
        RunPrimitive::StagedInput(_) => {
            let admitted_content_shape = crate::meerkat_machine::dsl::ContentShape::from(
                primitive_admitted_content_shape(primitive),
            );
            Some(
                crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                    primitive_kind:
                        crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn,
                    admitted_content_shape,
                    vision_enabled: false,
                    image_tool_results_enabled: false,
                    max_extraction_retries: 0,
                },
            )
        }
        _ => {
            let admitted_content_shape = crate::meerkat_machine::dsl::ContentShape::from(
                primitive_admitted_content_shape(primitive),
            );
            Some(
                crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                    primitive_kind:
                        crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn,
                    admitted_content_shape,
                    vision_enabled: false,
                    image_tool_results_enabled: false,
                    max_extraction_retries: 0,
                },
            )
        }
    }
}

async fn prepare_turn_state_for_primitive(
    driver: &crate::meerkat_machine::SharedDriver,
    run_id: &RunId,
    primitive: &RunPrimitive,
) -> Result<(), crate::RuntimeDriverError> {
    if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
        return Err(crate::RuntimeDriverError::Internal(reason.to_string()));
    }
    let Some(input) = primitive_turn_start_input(run_id, primitive) else {
        return Ok(());
    };
    let authority = {
        let driver = driver.lock().await;
        driver.shared_dsl_authority()
    };
    let mut auth = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Retired drain path: `machine_begin_run` treats Retired as
    // `is_retired_drain` and skips the `Prepare` DSL input, leaving
    // phase at Retired. The turn-start transitions
    // (`StartConversationRun{Initializing,Attached}` /
    // `StartImmediate{Append,Context}{Initializing,Attached}`) only
    // guard on Initializing/Attached — no Retired variant. Skip the
    // signal during drain; shell-side `set_control_projection` has
    // already advanced control so `executor.apply` proceeds next.
    if auth.state().lifecycle_phase == crate::meerkat_machine::dsl::MeerkatPhase::Retired {
        return Ok(());
    }
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut *auth, input)
        .map(|_| ())
        .map_err(|err| {
            crate::RuntimeDriverError::Internal(format!(
                "failed to start runtime turn state for run {run_id}: {err}"
            ))
        })
}

#[cfg(test)]
pub(crate) fn try_inputs_to_primitive_with_boundary(
    inputs: &[(InputId, Input)],
    boundary: RunApplyBoundary,
    semantics: &[crate::ingress_types::RuntimeInputSemantics],
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    let projections = inputs
        .iter()
        .map(|(_, input)| runtime_input_projection_for_machine_batch(input))
        .collect::<Vec<_>>();
    try_projected_inputs_to_primitive_with_boundary(inputs, &projections, boundary, semantics)
}

pub(crate) fn try_projected_inputs_to_primitive_with_boundary(
    inputs: &[(InputId, Input)],
    projections: &[crate::ingress_types::RuntimeInputProjection],
    boundary: RunApplyBoundary,
    semantics: &[crate::ingress_types::RuntimeInputSemantics],
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    // Injected-context appends chain BEFORE the input's own append: the
    // delivery invariant is that host-attached injected context lands in the
    // transcript immediately before the turn's user/peer message, in order.
    let appends = projections
        .iter()
        .flat_map(|projection| {
            projection
                .injected_context_appends
                .clone()
                .into_iter()
                .chain(projection.append.clone())
                .chain(projection.additional_appends.clone())
        })
        .collect::<Vec<_>>();
    let context_appends = projections
        .iter()
        .filter_map(|projection| projection.context_append.clone())
        .collect::<Vec<_>>();
    let contributing_input_ids = inputs
        .iter()
        .map(|(input_id, _)| input_id.clone())
        .collect::<Vec<_>>();
    // Merge turn metadata from ALL inputs in the batch, not just the first.
    // Scalar conflicts are typed errors; collection fields accumulate.
    let turn_metadata = merge_batch_turn_metadata(inputs, semantics)?;

    Ok(RunPrimitive::StagedInput(StagedRunInput {
        boundary,
        appends,
        context_appends,
        contributing_input_ids,
        turn_metadata,
    }))
}

#[cfg(test)]
pub(crate) fn inputs_to_primitive_with_boundary(
    inputs: &[(InputId, Input)],
    boundary: RunApplyBoundary,
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    let semantics = fallback_batch_semantics(inputs);
    try_inputs_to_primitive_with_boundary(inputs, boundary, &semantics)
}

#[cfg(test)]
pub(crate) fn inputs_to_primitive(
    inputs: &[(InputId, Input)],
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    let boundary = inputs
        .first()
        .map(|(_, input)| fallback_unadmitted_semantics(input).boundary)
        .unwrap_or(RunApplyBoundary::RunStart);
    inputs_to_primitive_with_boundary(inputs, boundary)
}

#[cfg(test)]
fn fallback_unadmitted_semantics(input: &Input) -> crate::ingress_types::RuntimeInputSemantics {
    crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(input, true)
        .expect("generated admission semantics")
}

#[cfg(test)]
fn fallback_batch_semantics(
    inputs: &[(InputId, Input)],
) -> Vec<crate::ingress_types::RuntimeInputSemantics> {
    inputs
        .iter()
        .map(|(_, input)| fallback_unadmitted_semantics(input))
        .collect()
}

/// Convert an `Input` + its ID to a `RunPrimitive` for `CoreExecutor::apply()`.
#[cfg(test)]
pub(crate) fn input_to_primitive(
    input: &Input,
    input_id: InputId,
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    inputs_to_primitive(&[(input_id, input.clone())])
}

#[cfg(test)]
pub(crate) fn admitted_input_to_primitive(
    input: &Input,
    input_id: InputId,
    projection: crate::ingress_types::RuntimeInputProjection,
    semantics: crate::ingress_types::RuntimeInputSemantics,
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    try_projected_inputs_to_primitive_with_boundary(
        &[(input_id, input.clone())],
        &[projection],
        semantics.boundary,
        &[semantics],
    )
}

#[derive(Clone)]
struct RuntimeLoopAuthorityBinding {
    machine: std::sync::Weak<crate::meerkat_machine::MeerkatMachine>,
    session_id: meerkat_core::types::SessionId,
    #[cfg(test)]
    detached_test_gate: Option<std::sync::Arc<crate::tokio::sync::Mutex<()>>>,
}

impl RuntimeLoopAuthorityBinding {
    fn new(
        machine: std::sync::Weak<crate::meerkat_machine::MeerkatMachine>,
        session_id: meerkat_core::types::SessionId,
    ) -> Self {
        Self {
            machine,
            session_id,
            #[cfg(test)]
            detached_test_gate: None,
        }
    }

    #[cfg(test)]
    fn detached_for_test() -> Self {
        Self {
            machine: std::sync::Weak::<crate::meerkat_machine::MeerkatMachine>::new(),
            session_id: meerkat_core::types::SessionId::new(),
            detached_test_gate: Some(std::sync::Arc::new(crate::tokio::sync::Mutex::new(()))),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn run_before_queue_authority_test_hook(&self) {
        if let Some(machine) = self.machine.upgrade() {
            machine
                .run_runtime_loop_before_queue_authority_test_hook(&self.session_id)
                .await;
        }
    }

    async fn lock_current_driver_authority(
        &self,
        driver: &crate::meerkat_machine::SharedDriver,
        context: &'static str,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, crate::traits::RuntimeDriverError> {
        #[cfg(test)]
        if let Some(gate) = &self.detached_test_gate {
            let _ = (driver, context);
            return Ok(std::sync::Arc::clone(gate).lock_owned().await);
        }

        let machine =
            self.machine
                .upgrade()
                .ok_or(crate::traits::RuntimeDriverError::NotReady {
                    state: crate::runtime_state::RuntimeState::Destroyed,
                })?;
        machine
            .lock_current_runtime_loop_driver_authority(&self.session_id, driver)
            .await
            .map_err(|err| {
                tracing::warn!(
                    session_id = %self.session_id,
                    error = %err,
                    context,
                    "runtime loop refused stale session driver authority"
                );
                err
            })
    }

    /// Exact teardown authority for an executor already retained by the
    /// runtime-loop handoff slot. Unlike live loop work, teardown recovery must
    /// also admit `Queuing` after `RuntimeExecutorExited`; exact session/driver
    /// identity under M is the authority, not an Active registration claim.
    async fn lock_current_teardown_driver_authority(
        &self,
        driver: &crate::meerkat_machine::SharedDriver,
        context: &'static str,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, crate::traits::RuntimeDriverError> {
        #[cfg(test)]
        if let Some(gate) = &self.detached_test_gate {
            let _ = (driver, context);
            return Ok(std::sync::Arc::clone(gate).lock_owned().await);
        }

        let machine =
            self.machine
                .upgrade()
                .ok_or(crate::traits::RuntimeDriverError::NotReady {
                    state: crate::runtime_state::RuntimeState::Destroyed,
                })?;
        machine
            .lock_current_session_driver_gate(&self.session_id, driver)
            .await
            .map_err(|err| {
                tracing::warn!(
                    session_id = %self.session_id,
                    error = %err,
                    context,
                    "runtime teardown refused stale session driver authority"
                );
                err
            })
    }

    async fn runtime_stop_cleanup_in_progress(
        &self,
        driver: &crate::meerkat_machine::SharedDriver,
    ) -> Result<bool, crate::traits::RuntimeDriverError> {
        #[cfg(test)]
        if self.detached_test_gate.is_some() {
            let _ = driver;
            return Ok(false);
        }

        let machine =
            self.machine
                .upgrade()
                .ok_or(crate::traits::RuntimeDriverError::NotReady {
                    state: crate::runtime_state::RuntimeState::Destroyed,
                })?;
        machine
            .current_runtime_stop_cleanup_in_progress(&self.session_id, driver)
            .await
    }

    async fn begin_durable_unregister_for_teardown(
        &self,
        driver: &crate::meerkat_machine::SharedDriver,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        let machine =
            self.machine
                .upgrade()
                .ok_or(crate::traits::RuntimeDriverError::NotReady {
                    state: crate::runtime_state::RuntimeState::Destroyed,
                })?;
        machine
            .begin_unregister_from_runtime_loop_teardown(&self.session_id, driver)
            .await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedWakeOutcome {
    Noop,
    Injected,
    StaleAuthority,
}

/// Spawn the per-session runtime loop with optional completion registry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_runtime_loop_with_completions(
    driver: crate::meerkat_machine::SharedDriver,
    executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    mut wake_rx: tokio::sync::mpsc::Receiver<()>,
    mut effect_rx: tokio::sync::mpsc::Receiver<crate::effect::RuntimeEffect>,
    completions: Option<crate::meerkat_machine::SharedCompletionRegistry>,
    completion_feed: Option<std::sync::Arc<dyn meerkat_core::completion_feed::CompletionFeed>>,
    ops_lifecycle: Option<std::sync::Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>>,
    epoch_cursor_state: Option<std::sync::Arc<meerkat_core::EpochCursorState>>,
    machine_weak: std::sync::Weak<crate::meerkat_machine::MeerkatMachine>,
    session_id: meerkat_core::types::SessionId,
    startup_turn_finalization_boundary_already_held: bool,
) -> SpawnedRuntimeLoop {
    #[cfg(test)]
    let authority_binding = if machine_weak.strong_count() == 0 {
        RuntimeLoopAuthorityBinding::detached_for_test()
    } else {
        RuntimeLoopAuthorityBinding::new(machine_weak.clone(), session_id.clone())
    };
    #[cfg(not(test))]
    let authority_binding =
        RuntimeLoopAuthorityBinding::new(machine_weak.clone(), session_id.clone());
    #[cfg(test)]
    let detached_test_startup = machine_weak.strong_count() == 0;
    let teardown_slot = RuntimeLoopTeardownSlot::pending_with_recovery(
        authority_binding.clone(),
        completions.clone(),
    );
    let startup = RuntimeLoopStartupSlot::pending();
    let (startup_authority_sender, startup_authority_receiver) = tokio::sync::oneshot::channel();
    let (serving_release_sender, serving_release_receiver) = tokio::sync::oneshot::channel();
    let startup_guard = RuntimeLoopStartupGuard::new(std::sync::Arc::clone(&startup));
    let teardown_watcher_slot = std::sync::Arc::clone(&teardown_slot);
    let teardown_machine = machine_weak;
    let teardown_session_id = session_id;
    tokio::spawn(async move {
        teardown_watcher_slot.wait_until_published().await;
        let disposition = teardown_watcher_slot.disposition();
        let Some(machine) = teardown_machine.upgrade() else {
            return;
        };
        // This watcher is deliberately separate from both the loop body and
        // the owned unregister saga. It may join an already-running direct
        // unregister or observe its retained terminal result, but never turns
        // a completed failure into an implicit retry. The saga never awaits
        // this watcher, so no self-join cycle is possible.
        if let Err(error) = machine
            .observe_runtime_loop_teardown(
                &teardown_session_id,
                std::sync::Arc::clone(&teardown_watcher_slot),
                disposition,
            )
            .await
        {
            tracing::warn!(
                session_id = %teardown_session_id,
                %error,
                "runtime-loop exit teardown did not complete; retained for retry"
            );
        }
    });
    let loop_teardown_slot = std::sync::Arc::clone(&teardown_slot);
    let loop_process_teardown_slot = std::sync::Arc::clone(&teardown_slot);
    let loop_handoff_guard = RuntimeLoopHandoffGuard::new(loop_teardown_slot, executor);
    let loop_handle = tokio::spawn(async move {
        let mut loop_handoff_guard = loop_handoff_guard;
        let mut startup_guard = startup_guard;
        macro_rules! executor_or_return {
            () => {
                match loop_handoff_guard.executor_mut() {
                    Ok(executor) => executor,
                    Err(error) => {
                        tracing::error!(%error, "runtime loop lost executor handoff authority");
                        return;
                    }
                }
            };
        }
        let mut terminal_handoff = RuntimeLoopTerminalHandoff {
            disposition: RuntimeLoopTeardownDisposition::UnservedAttachment,
            ..RuntimeLoopTerminalHandoff::default()
        };
        // Feed-based idle wake state (local to this loop).
        // Seed from generated cursor authority when available. Even an
        // all-zero runtime-owned cursor must win over the feed watermark so a
        // fresh runtime loop cannot silently skip background completions that
        // land before the task reaches its first select iteration.
        let initial_watermark = match ops_lifecycle.as_ref() {
            Some(registry) => {
                let obs = match registry.completion_cursor(
                    meerkat_core::ops_lifecycle::CompletionCursorConsumer::RuntimeObserved,
                ) {
                    Ok(value) => value,
                    Err(err) => {
                        tracing::error!(
                            session_id = %authority_binding.session_id,
                            error = %err,
                            "runtime loop refused to seed idle-wake watermark from poisoned ops cursor authority"
                        );
                        return;
                    }
                };
                let inj = match registry.completion_cursor(
                    meerkat_core::ops_lifecycle::CompletionCursorConsumer::RuntimeInjected,
                ) {
                    Ok(value) => value,
                    Err(err) => {
                        tracing::error!(
                            session_id = %authority_binding.session_id,
                            error = %err,
                            "runtime loop refused to seed idle-wake watermark from poisoned ops cursor authority"
                        );
                        return;
                    }
                };
                // `Ok(None)` from either consumer means no generated cursor
                // authority for that consumer; treat the missing value as 0 so
                // the watermark falls back to the legitimate no-authority floor.
                obs.unwrap_or(0).max(inj.unwrap_or(0))
            }
            None => 0,
        };
        let mut observed_seq: meerkat_core::completion_feed::CompletionSeq = initial_watermark;
        let mut last_injected_seq: meerkat_core::completion_feed::CompletionSeq =
            match ops_lifecycle.as_ref() {
                Some(registry) => match registry.completion_cursor(
                    meerkat_core::ops_lifecycle::CompletionCursorConsumer::RuntimeInjected,
                ) {
                    Ok(Some(value)) if value > 0 => value,
                    Ok(_) => initial_watermark,
                    Err(err) => {
                        tracing::error!(
                            session_id = %authority_binding.session_id,
                            error = %err,
                            "runtime loop refused to seed idle-wake watermark from poisoned ops cursor authority"
                        );
                        return;
                    }
                },
                None => initial_watermark,
            };

        // Committed transcript/compaction projection recovery precedes public
        // terminal publication. A cold process can carry both outboxes from
        // one atomic run boundary; resolving the Interaction first would let
        // callers observe completion before the authoritative session
        // checkpoint converged. Startup recovery owns the same B -> M order as
        // ordinary turn finalization and abnormal teardown, so no concurrent
        // turn can interleave between the recovered checkpoint and terminal
        // drain.
        let startup_turn_finalization_guard = if startup_turn_finalization_boundary_already_held {
            // A shared actor/attachment transaction retains the exact outer B
            // while awaiting this startup result. Reacquiring the same
            // non-reentrant boundary here would deadlock that transaction.
            None
        } else {
            match executor_or_return!().turn_finalization_boundary_handle() {
                Some(boundary) => match boundary.acquire().await {
                    Ok(guard) => Some(guard),
                    Err(error) => {
                        startup_guard.publish(Err(error.to_string()));
                        tracing::error!(
                            session_id = %authority_binding.session_id,
                            %error,
                            "runtime loop failed to acquire startup recovery turn boundary"
                        );
                        return;
                    }
                },
                None => None,
            }
        };
        // The attachment owner claimed M before spawning us. It publishes the
        // exact attachment while keeping that claim, then transfers the owned
        // guard through this one-shot only after B is either acquired here or
        // proved retained by the boundary-aware caller. The channel therefore
        // closes the attachment race without an M -> B lock inversion or a
        // caller-waits-startup/task-waits-M cycle.
        let startup_authority_guard = match startup_authority_receiver.await {
            Ok(Some(authority_guard)) => authority_guard,
            Ok(None) => match authority_binding
                .lock_current_driver_authority(&driver, "runtime startup projection recovery")
                .await
            {
                Ok(guard) => guard,
                Err(error) => {
                    drop(startup_turn_finalization_guard);
                    startup_guard.publish(Err(error.to_string()));
                    tracing::error!(
                        session_id = %authority_binding.session_id,
                        %error,
                        "runtime loop lost exact authority before startup projection recovery"
                    );
                    return;
                }
            },
            Err(_) => {
                drop(startup_turn_finalization_guard);
                startup_guard.publish(Err(
                    "runtime-loop attachment dropped startup authority before transfer".to_string(),
                ));
                return;
            }
        };
        let compaction_recovery =
            reconcile_compaction_projection_outbox(&driver, executor_or_return!()).await;
        if let Err(error) = compaction_recovery {
            drop(startup_authority_guard);
            drop(startup_turn_finalization_guard);
            startup_guard.publish(Err(error.to_string()));
            tracing::error!(
                session_id = %authority_binding.session_id,
                %error,
                "runtime loop failed compaction projection recovery before accepting work"
            );
            return;
        }
        if let Err(error) = drain_recovered_interaction_terminal_outboxes_under_authority(
            &driver,
            completions.as_ref(),
            executor_or_return!(),
            &startup_authority_guard,
        )
        .await
        {
            drop(startup_authority_guard);
            drop(startup_turn_finalization_guard);
            startup_guard.publish(Err(error.to_string()));
            tracing::error!(
                session_id = %authority_binding.session_id,
                %error,
                "runtime loop failed interaction-terminal recovery before accepting work"
            );
            return;
        }
        drop(startup_authority_guard);
        drop(startup_turn_finalization_guard);
        startup_guard.publish(Ok(()));

        // Startup readiness is deliberately distinct from serving readiness.
        // The machine returns a non-clone pending attachment lease after the
        // first phase; its owner publishes exact surface sidecars and releases
        // this gate only when the same attachment is committed. Dropping the
        // release half aborts the loop before any queued/completion work runs.
        if serving_release_receiver.await.is_err() {
            return;
        }
        terminal_handoff.disposition = RuntimeLoopTeardownDisposition::PreserveRegistration;
        loop_handoff_guard
            .set_fallback_disposition(RuntimeLoopTeardownDisposition::PreserveRegistration);

        loop {
            // Build a future for the idle wake. Backed by the completion feed
            // only when generated ops cursor authority is present; otherwise
            // pends forever because the feed watermark is not delivery
            // authority.
            let idle_wake = async {
                if let (Some(feed), Some(_)) = (completion_feed.as_ref(), ops_lifecycle.as_ref()) {
                    feed.wait_for_advance(observed_seq).await;
                } else {
                    std::future::pending::<()>().await;
                }
            };

            tokio::select! {
                biased;
                maybe_effect = effect_rx.recv() => {
                    match maybe_effect {
                        Some(effect) => {
                            let turn_finalization_guard = match executor_or_return!()
                                .turn_finalization_boundary_handle()
                            {
                                Some(boundary) => match boundary.acquire().await {
                                    Ok(guard) => Some(guard),
                                    Err(error) => {
                                        tracing::error!(
                                            %error,
                                            "failed to acquire turn-finalization boundary for direct executor effect"
                                        );
                                        break;
                                    }
                                },
                                None => None,
                            };
                            let authority_guard = match authority_binding
                                .lock_current_driver_authority(
                                    &driver,
                                    "runtime loop direct executor effect",
                                )
                                .await
                            {
                                Ok(guard) => guard,
                                Err(_) => break,
                            };
                            if effect.is_stop() {
                                // Stop hooks may re-enter machine control, and
                                // required cleanup is an explicit post-loop
                                // handoff. Relinquish session authority before
                                // realizing either boundary.
                                drop(authority_guard);
                            }
                            if apply_runtime_loop_effect_and_record_handoff(
                                &driver,
                                completions.as_ref(),
                                executor_or_return!(),
                                effect,
                                &mut terminal_handoff,
                                turn_finalization_guard,
                            )
                            .await
                            {
                                break;
                            }
                        }
                        None => {
                            // Closed effect receiver is NOT an applied stop —
                            // route through the canonical stop/terminalize
                            // path so the DSL executor-exit, durable
                            // runtime-state commit, and waiter terminalization
                            // all fire (same shape as the ready-effect drain
                            // ChannelClosed arm).
                            let turn_finalization_guard = match executor_or_return!()
                                .turn_finalization_boundary_handle()
                            {
                                Some(boundary) => match boundary.acquire().await {
                                    Ok(guard) => Some(guard),
                                    Err(error) => {
                                        tracing::error!(
                                            %error,
                                            "failed to acquire turn-finalization boundary for closed effect channel"
                                        );
                                        break;
                                    }
                                },
                                None => None,
                            };
                            let _ = stop_runtime_loop_executor_from_dsl_effect(
                                &driver,
                                completions.as_ref(),
                                executor_or_return!(),
                                "runtime effect channel closed".to_string(),
                                &mut terminal_handoff,
                                turn_finalization_guard,
                            )
                            .await;
                            break;
                        }
                    }
                }
                maybe_wake = wake_rx.recv() => {
                    match maybe_wake {
                        Some(()) => {
                            if process_queue(
                                &driver,
                                executor_or_return!(),
                                &mut effect_rx,
                                completions.as_ref(),
                                &authority_binding,
                                &loop_process_teardown_slot,
                                &mut terminal_handoff,
                            )
                            .await
                            {
                                break;
                            }
                            // Secondary wake path: re-check after queue drain.
                            // If a completion arrived during process_queue, inject
                            // and immediately process the continuation.
                            match maybe_inject_feed_wake(
                                &driver,
                                completion_feed.as_deref(),
                                &mut observed_seq,
                                &mut last_injected_seq,
                                epoch_cursor_state.as_deref(),
                                ops_lifecycle.as_deref(),
                                &authority_binding,
                            )
                            .await
                            {
                                FeedWakeOutcome::Injected => {
                                    if process_queue(
                                        &driver,
                                        executor_or_return!(),
                                        &mut effect_rx,
                                        completions.as_ref(),
                                        &authority_binding,
                                        &loop_process_teardown_slot,
                                        &mut terminal_handoff,
                                    )
                                    .await
                                    {
                                        break;
                                    }
                                }
                                FeedWakeOutcome::StaleAuthority => break,
                                FeedWakeOutcome::Noop => {}
                            }
                        }
                        None => {
                            // Closed wake channel is NOT an applied stop —
                            // terminalize through the canonical
                            // StopRuntimeExecutor path rather than silently
                            // exiting the loop.
                            let turn_finalization_guard = match executor_or_return!()
                                .turn_finalization_boundary_handle()
                            {
                                Some(boundary) => match boundary.acquire().await {
                                    Ok(guard) => Some(guard),
                                    Err(error) => {
                                        tracing::error!(
                                            %error,
                                            "failed to acquire turn-finalization boundary for closed wake channel"
                                        );
                                        break;
                                    }
                                },
                                None => None,
                            };
                            let _ = stop_runtime_loop_executor_from_dsl_effect(
                                &driver,
                                completions.as_ref(),
                                executor_or_return!(),
                                "runtime wake channel closed".to_string(),
                                &mut terminal_handoff,
                                turn_finalization_guard,
                            )
                            .await;
                            break;
                        }
                    }
                }
                () = idle_wake => {
                    // A completion arrived while idle. Generated ops authority
                    // classifies whether it should wake this runtime; other
                    // completions already wake through their owning channels.
                    match maybe_inject_feed_wake(
                        &driver,
                        completion_feed.as_deref(),
                        &mut observed_seq,
                        &mut last_injected_seq,
                        epoch_cursor_state.as_deref(),
                        ops_lifecycle.as_deref(),
                        &authority_binding,
                    )
                    .await
                    {
                        FeedWakeOutcome::Injected => {
                            if process_queue(
                                &driver,
                                executor_or_return!(),
                                &mut effect_rx,
                                completions.as_ref(),
                                &authority_binding,
                                &loop_process_teardown_slot,
                                &mut terminal_handoff,
                            )
                            .await
                            {
                                break;
                            }
                        }
                        FeedWakeOutcome::StaleAuthority => break,
                        FeedWakeOutcome::Noop => {}
                    }
                }
            }
        }

        // This publication is the loop task's final action. The machine-owned
        // coordinator receives the exact executor, awaits this task externally,
        // and runs the external-only cleanup hook before any separately
        // authorized unregister removal.
        if let Err(error) = loop_handoff_guard.publish(terminal_handoff) {
            tracing::error!(%error, "runtime loop failed to publish executor handoff");
        }
    });
    let spawned = SpawnedRuntimeLoop {
        loop_handle,
        teardown_slot,
        startup,
        startup_authority_transfer: Some(RuntimeLoopStartupAuthorityTransfer {
            sender: startup_authority_sender,
        }),
        serving_release: Some(RuntimeLoopServingRelease {
            sender: serving_release_sender,
        }),
    };
    #[cfg(test)]
    let mut spawned = spawned;
    #[cfg(test)]
    if detached_test_startup {
        match spawned.take_startup_authority_transfer() {
            Ok(transfer) => {
                if let Err(error) = transfer.send(None) {
                    tracing::error!(%error, "failed to transfer detached test startup authority");
                }
            }
            Err(error) => {
                tracing::error!(%error, "detached test startup authority transfer was missing");
            }
        }
        match spawned.take_serving_release() {
            Ok(release) => {
                if let Err(error) = release.release() {
                    tracing::error!(%error, "failed to release detached test serving gate");
                }
            }
            Err(error) => {
                tracing::error!(%error, "detached test serving release was missing");
            }
        }
    }
    spawned
}

/// Check for new background op completions and inject a continuation if needed.
///
/// Called after queue processing completes (session has returned to idle).
/// Cursor-advance plan for the quiescent detached-wake injection tail.
///
/// A successful injection advances BOTH the injected and observed cursors so
/// the completion is recorded as delivered. A failed injection must advance
/// NEITHER: the completion stays visible at the unchanged observed cursor so
/// the next wake re-presents it for retry instead of laundering a failed
/// injection into a watermark advance that hides the completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetachedWakeCursorPlan {
    /// Injection succeeded: advance the injected then the observed cursor.
    AdvanceInjectedAndObserved,
    /// Injection failed: leave both cursors untouched so the completion is
    /// re-presented on the next wake.
    LeaveVisibleForRetry,
}

impl DetachedWakeCursorPlan {
    /// Decide the cursor-advance plan from the injection outcome.
    fn from_injection(injection_succeeded: bool) -> Self {
        if injection_succeeded {
            Self::AdvanceInjectedAndObserved
        } else {
            Self::LeaveVisibleForRetry
        }
    }

    /// Test-only convenience query: the runtime path itself matches on the
    /// plan variants directly (see the `match` at the detached-wake site), so
    /// this boolean projection exists only for the unit tests.
    #[cfg(test)]
    fn advances_observed_cursor(self) -> bool {
        matches!(self, Self::AdvanceInjectedAndObserved)
    }
}

/// Distinguishes ordinary no-op from stale-authority shutdown so cursor
/// projection cannot advance after the runtime loop loses current ownership.
fn advance_runtime_completion_cursor(
    ops_lifecycle: Option<&dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
    consumer: meerkat_core::ops_lifecycle::CompletionCursorConsumer,
    cursor: meerkat_core::completion_feed::CompletionSeq,
    epoch_cursor_state: Option<&meerkat_core::EpochCursorState>,
) -> Result<meerkat_core::completion_feed::CompletionSeq, FeedWakeOutcome> {
    let Some(registry) = ops_lifecycle else {
        tracing::warn!(
            ?consumer,
            cursor,
            has_epoch_projection = epoch_cursor_state.is_some(),
            "runtime loop refused completion cursor advance without generated ops authority"
        );
        return Err(FeedWakeOutcome::StaleAuthority);
    };
    registry
        .advance_completion_cursor(consumer, cursor, epoch_cursor_state)
        .map_err(|err| {
            tracing::warn!(
                ?consumer,
                cursor,
                error = %err,
                "generated ops authority rejected runtime completion cursor advance"
            );
            FeedWakeOutcome::StaleAuthority
        })
}

fn batch_has_generated_wake_completion(
    batch: &meerkat_core::completion_feed::CompletionBatch,
    last_injected_seq: meerkat_core::completion_feed::CompletionSeq,
    registry: &dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry,
) -> Result<bool, FeedWakeOutcome> {
    for entry in batch
        .entries
        .iter()
        .filter(|entry| entry.seq > last_injected_seq)
    {
        match registry.classify_operation_completion_wake(&entry.operation_id, entry.kind) {
            Ok(meerkat_core::ops_lifecycle::OperationCompletionWakeClass::Wake) => {
                return Ok(true);
            }
            Ok(meerkat_core::ops_lifecycle::OperationCompletionWakeClass::Ignore) => {}
            Err(err) => {
                tracing::warn!(
                    operation_id = %entry.operation_id,
                    kind = ?entry.kind,
                    error = %err,
                    "generated completion-wake authority rejected runtime feed entry"
                );
                return Err(FeedWakeOutcome::StaleAuthority);
            }
        }
    }
    Ok(false)
}

async fn maybe_inject_feed_wake(
    driver: &crate::meerkat_machine::SharedDriver,
    feed: Option<&dyn meerkat_core::completion_feed::CompletionFeed>,
    observed_seq: &mut meerkat_core::completion_feed::CompletionSeq,
    last_injected_seq: &mut meerkat_core::completion_feed::CompletionSeq,
    epoch_cursor_state: Option<&meerkat_core::EpochCursorState>,
    ops_lifecycle: Option<&dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
    authority_binding: &RuntimeLoopAuthorityBinding,
) -> FeedWakeOutcome {
    let Some(feed) = feed else {
        return FeedWakeOutcome::Noop;
    };
    let Some(registry) = ops_lifecycle else {
        tracing::warn!(
            "runtime loop refused completion feed wake without generated ops cursor authority"
        );
        return FeedWakeOutcome::StaleAuthority;
    };
    let batch = feed.list_since(*observed_seq);

    let has_new_wake_completion =
        match batch_has_generated_wake_completion(&batch, *last_injected_seq, registry) {
            Ok(has_wake_completion) => has_wake_completion,
            Err(outcome) => return outcome,
        };

    let Ok(_authority_guard) = authority_binding
        .lock_current_driver_authority(driver, "runtime loop feed wake")
        .await
    else {
        return FeedWakeOutcome::StaleAuthority;
    };

    if !has_new_wake_completion {
        // No generated wake-worthy completions: advance to prevent hot-spin
        // on observe-only entries.
        let advanced = match advance_runtime_completion_cursor(
            Some(registry),
            meerkat_core::ops_lifecycle::CompletionCursorConsumer::RuntimeObserved,
            batch.watermark,
            epoch_cursor_state,
        ) {
            Ok(cursor) => cursor,
            Err(outcome) => return outcome,
        };
        *observed_seq = advanced;
        return FeedWakeOutcome::Noop;
    }

    // Verify quiescence before injecting.
    let d = driver.lock().await;
    if !d.is_quiescent_for_detached_wake() {
        // Non-quiescent: do NOT advance observed_seq. The completion
        // stays visible for the next wake so it's not permanently lost.
        return FeedWakeOutcome::Noop;
    }
    drop(d);

    let input = crate::input::Input::Continuation(
        crate::input::ContinuationInput::detached_background_op_completed(),
    );
    let mut d = driver.lock().await;
    let injection_succeeded = d.as_driver_mut().accept_input(input).await.is_ok();
    drop(d);

    match DetachedWakeCursorPlan::from_injection(injection_succeeded) {
        DetachedWakeCursorPlan::LeaveVisibleForRetry => {
            // Injection failed: do NOT advance either cursor. Mirror the
            // non-quiescent branch above and leave the completion visible so
            // the next wake re-presents it for retry instead of laundering the
            // failed injection into a watermark advance that hides the
            // completion.
            FeedWakeOutcome::Noop
        }
        DetachedWakeCursorPlan::AdvanceInjectedAndObserved => {
            // Injection succeeded: advance both the injected and observed
            // cursors so the completion is recorded as delivered and the loop
            // does not hot-spin on it.
            let injected = match advance_runtime_completion_cursor(
                Some(registry),
                meerkat_core::ops_lifecycle::CompletionCursorConsumer::RuntimeInjected,
                batch.watermark,
                epoch_cursor_state,
            ) {
                Ok(cursor) => cursor,
                Err(outcome) => return outcome,
            };
            *last_injected_seq = injected;
            let observed = match advance_runtime_completion_cursor(
                Some(registry),
                meerkat_core::ops_lifecycle::CompletionCursorConsumer::RuntimeObserved,
                batch.watermark,
                epoch_cursor_state,
            ) {
                Ok(cursor) => cursor,
                Err(outcome) => return outcome,
            };
            *observed_seq = observed;
            FeedWakeOutcome::Injected
        }
    }
}

/// What the runtime loop should do with the backlog after a batch failed
/// terminally and its surviving members were rolled back to their lanes.
enum FailedBatchBacklogOutcome {
    /// The failed batch was deferred behind the backlog — keep draining so
    /// other queued work runs before the failed payload is retried.
    ContinueProcessing,
    /// Nothing else is queued: leave the failed batch queued for a future
    /// wake instead of hot-looping on the same payload indefinitely.
    Park,
    /// Deferring the batch hit a genuine projection invariant break. The loop
    /// was stopped through the canonical executor-stop path (a silent return
    /// here strands the backlog behind a dropped wake); the payload is the
    /// stop path's should_stop verdict.
    Stopped(bool),
}

/// Canonical post-failure backlog handling for a terminally failed batch.
///
/// The machine's failure realization may have resolved individual batch
/// members past the queued world (max-attempts abandonment, boundary-applied
/// members) — the defer sweep is total over those via the machine's
/// `DeferInputBehindBacklogAlreadyResolved` no-op arm, so a defer error here
/// is projection corruption, never a machine-owned resolution.
///
/// Callers must drop any driver-authority guard before invoking this: a stop
/// hook may re-enter machine control, while required cleanup is handed off
/// only after the loop exits.
async fn resolve_failed_batch_backlog(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    input_ids: &[InputId],
    handoff: &mut RuntimeLoopTerminalHandoff,
    turn_finalization_guard: Option<
        Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>,
    >,
) -> FailedBatchBacklogOutcome {
    let defer_error = {
        let mut d = driver.lock().await;
        if !d.has_queued_input_outside(input_ids) {
            return FailedBatchBacklogOutcome::Park;
        }
        match d.defer_queued_inputs_behind_backlog(input_ids) {
            Ok(()) => {
                d.take_wake_requested();
                None
            }
            Err(err) => Some(err),
        }
    };
    let Some(err) = defer_error else {
        return FailedBatchBacklogOutcome::ContinueProcessing;
    };
    tracing::error!(
        error = %err,
        "failed to defer failed input batch behind backlog"
    );
    let should_stop = stop_runtime_loop_executor_from_dsl_effect(
        driver,
        completions,
        executor,
        format!("failed to defer failed input batch behind backlog: {err}"),
        handoff,
        turn_finalization_guard,
    )
    .await;
    FailedBatchBacklogOutcome::Stopped(should_stop)
}

/// Process all queued inputs until the queue is empty.
#[allow(clippy::too_many_arguments)]
async fn process_queue(
    driver: &crate::meerkat_machine::SharedDriver,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    effect_rx: &mut tokio::sync::mpsc::Receiver<crate::effect::RuntimeEffect>,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    authority_binding: &RuntimeLoopAuthorityBinding,
    teardown_slot: &std::sync::Arc<RuntimeLoopTeardownSlot>,
    handoff: &mut RuntimeLoopTerminalHandoff,
) -> bool {
    loop {
        let turn_finalization_guard = match executor.turn_finalization_boundary_handle() {
            Some(boundary) => match boundary.acquire().await {
                Ok(guard) => Some(guard),
                Err(error) => {
                    tracing::error!(
                        %error,
                        "failed to acquire runtime turn-finalization boundary"
                    );
                    return true;
                }
            },
            None => None,
        };
        let effect_authority_guard = match authority_binding
            .lock_current_driver_authority(driver, "runtime loop executor effects")
            .await
        {
            Ok(guard) => guard,
            Err(_) => return true,
        };
        match crate::control_plane::drain_ready_executor_effects(executor, effect_rx).await {
            Ok(crate::control_plane::EffectDrainOutcome::StopEffectPending(effect)) => {
                // The drain never applies stops: realize it here after the
                // authority guard is released. Required cleanup remains a
                // post-loop external handoff.
                drop(effect_authority_guard);
                return apply_runtime_loop_effect_and_record_handoff(
                    driver,
                    completions,
                    executor,
                    effect,
                    handoff,
                    turn_finalization_guard,
                )
                .await;
            }
            Ok(crate::control_plane::EffectDrainOutcome::Empty) => {}
            Ok(crate::control_plane::EffectDrainOutcome::ChannelClosed) => {
                // The effect sender was dropped: the control plane is gone.
                // Closure is NOT an applied stop — route through the canonical
                // stop/terminalize path so the DSL executor-exit, durable
                // stopped truth, and completion waiters cannot split before the
                // loop exits.
                drop(effect_authority_guard);
                return stop_runtime_loop_executor_from_dsl_effect(
                    driver,
                    completions,
                    executor,
                    "runtime effect channel closed".to_string(),
                    handoff,
                    turn_finalization_guard,
                )
                .await;
            }
            Err(error) => {
                tracing::error!(
                    error = %error,
                    "failed to drain runtime executor effect"
                );
                return true;
            }
        }
        drop(effect_authority_guard);

        // Stop ownership is installed before its worker enqueues the executor
        // effect. If that coordinator is already active, yield to the outer
        // biased select instead of admitting queued ordinary work in the gap
        // between the two operations.
        match authority_binding
            .runtime_stop_cleanup_in_progress(driver)
            .await
        {
            Ok(true) => return false,
            Ok(false) => {}
            Err(_) => return true,
        }

        #[cfg(any(test, feature = "test-support"))]
        authority_binding
            .run_before_queue_authority_test_hook()
            .await;

        let queue_authority_guard = match authority_binding
            .lock_current_driver_authority(driver, "runtime loop queue processing")
            .await
        {
            Ok(guard) => guard,
            Err(_) => return true,
        };

        // Stop coordinator insertion takes this same mutation gate. Recheck
        // after claiming it so coordinator ownership and ordinary dequeue have
        // one total order; a stop installed after the earlier fast-path check
        // can no longer lose one queued batch in the gap.
        match authority_binding
            .runtime_stop_cleanup_in_progress(driver)
            .await
        {
            Ok(true) => {
                drop(queue_authority_guard);
                return false;
            }
            Ok(false) => {}
            Err(_) => return true,
        }

        // Effect publication also commits under this exact mutation gate. A
        // cancel can become ready after the first drain but before queue
        // authority is acquired. Keep this consumed wake inside process_queue:
        // the next lap drains the effect and then resumes the same queued work.
        if !effect_rx.is_empty() {
            drop(queue_authority_guard);
            continue;
        }

        enum RuntimeLoopDequeueOutcome {
            Ready {
                input_ids: Vec<InputId>,
                run_id: RunId,
                primitive: Box<
                    Result<
                        RunPrimitive,
                        meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict,
                    >,
                >,
                batch: crate::meerkat_machine::driver::AuthorizedRuntimeLoopBatch,
            },
            ProjectionMismatch {
                input_ids: Vec<InputId>,
                reason: String,
            },
        }

        // Dequeue and prepare under the driver lock.
        let dequeued = 'dequeue: {
            let mut d = driver.lock().await;

            // Immediate attached steer can pre-bind the DSL run before waking
            // the loop. The generated classifier owns whether that binding
            // admits queue processing and whether the loop must reuse it.
            let current_run_id = d.current_run_id();
            let queue_plan = match d.runtime_loop_queue_admission(current_run_id.is_some()) {
                Ok(queue_plan) => queue_plan,
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        "failed closed while classifying runtime queue admission"
                    );
                    return false;
                }
            };
            if !queue_plan.can_process_queue() {
                return false;
            }
            let prebound_run_id = if queue_plan.uses_prebound_run() {
                current_run_id
            } else {
                None
            };

            // Ask the ingress authority for the next batch of input IDs.
            // The authority implements steer-first priority and same-boundary
            // batching for steer inputs; queue inputs follow the prompt-aware
            // batching policy.
            let Some(batch) = crate::meerkat_machine::machine_authorize_runtime_loop_batch(&d)
            else {
                return false;
            };

            // Dequeue the batch members from the physical queues. The physical
            // projection must exactly match the machine-selected batch; partial
            // shrinkage would turn projection drift into different semantic work.
            let staged_inputs = match d.dequeue_batch_exact(&batch) {
                Ok(staged_inputs) => staged_inputs,
                Err(error) => {
                    let reason =
                        format!("runtime loop batch projection conformance failed: {error}");
                    tracing::error!(
                        error = %error,
                        "runtime loop batch projection conformance failed"
                    );
                    break 'dequeue RuntimeLoopDequeueOutcome::ProjectionMismatch {
                        input_ids: batch.input_ids().to_vec(),
                        reason,
                    };
                }
            };

            let run_id = prebound_run_id.unwrap_or_else(RunId::new);

            // The checked-in Meerkat machine owns the coarse "this dequeued
            // batch has now become a run" transition, including unwind on
            // staging failure.
            let staged_ids: Vec<_> = staged_inputs.iter().map(|(id, _)| id.clone()).collect();

            let contributing_input_ids = staged_inputs
                .iter()
                .map(|(staged_input_id, _)| staged_input_id.clone())
                .collect::<Vec<_>>();
            let semantics =
                crate::meerkat_machine::machine_batch_runtime_semantics(&d, &staged_ids);
            let projections =
                crate::meerkat_machine::machine_batch_primitive_projections(&d, &staged_inputs);
            let primitive = match (semantics, projections) {
                (Some(semantics), Some(projections)) => {
                    let boundary = semantics.first().map(|semantics| semantics.boundary).ok_or(
                        meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict {
                            field: "runtime_boundary",
                            reason: "runtime-stamped boundary missing for staged inputs",
                        },
                    );
                    match boundary {
                        Ok(boundary) => try_projected_inputs_to_primitive_with_boundary(
                            &staged_inputs,
                            &projections,
                            boundary,
                            &semantics,
                        ),
                        Err(error) => Err(error),
                    }
                }
                (None, _) => Err(
                    meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict {
                        field: "execution_kind",
                        reason: "runtime-stamped execution kind missing for one or more inputs",
                    },
                ),
                // Fail closed alongside semantics: a missing primitive projection
                // (co-recorded with runtime semantics) must reject the batch, not
                // construct a run primitive from a defaulted projection.
                (Some(_), None) => Err(
                    meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict {
                        field: "primitive_projection",
                        reason: "runtime-stamped primitive projection missing for one or more inputs",
                    },
                ),
            };
            RuntimeLoopDequeueOutcome::Ready {
                input_ids: contributing_input_ids,
                run_id,
                primitive: Box::new(primitive),
                batch,
            }
        };

        match dequeued {
            RuntimeLoopDequeueOutcome::Ready {
                input_ids,
                run_id,
                primitive,
                batch,
            } => {
                if let Err(err) = crate::meerkat_machine::prepare_runtime_loop_batch_start(
                    driver,
                    run_id.clone(),
                    batch,
                )
                .await
                {
                    tracing::error!(%run_id, error = %err, "failed to prepare runtime loop batch");
                    if let Some(completions) = completions.as_ref() {
                        let mut completions = completions.lock().await;
                        fail_completion_waiters(
                            &mut completions,
                            &input_ids,
                            format!("runtime batch preparation failed: {err}"),
                        );
                    }
                    return false;
                }
                let staged_directed_interaction_ids = {
                    let driver_guard = driver.lock().await;
                    match crate::meerkat_machine::driver::machine_staged_directed_interaction_ids(
                        &driver_guard,
                    ) {
                        Ok(interaction_ids) => interaction_ids,
                        Err(error) => {
                            drop(driver_guard);
                            drop(queue_authority_guard);
                            return stop_runtime_loop_executor_from_dsl_effect(
                                driver,
                                completions,
                                executor,
                                format!(
                                    "runtime staged directed-interaction projection failed: {error}"
                                ),
                                handoff,
                                turn_finalization_guard,
                            )
                            .await;
                        }
                    }
                };
                let primitive = match *primitive {
                    Ok(primitive) => primitive,
                    Err(conflict) => {
                        tracing::error!(
                            %run_id,
                            field = conflict.field,
                            reason = conflict.reason,
                            "batch turn-metadata merge conflict"
                        );
                        let completion_reason = format!("runtime primitive rejected: {conflict}");
                        let nondirected_input_ids = if completions.is_some() {
                            nondirected_completion_input_ids(
                                &input_ids,
                                &staged_directed_interaction_ids,
                            )
                        } else {
                            Vec::new()
                        };
                        let (queue_authority_guard, terminalization_outcome) =
                            match realize_runtime_loop_terminal_owned(
                                queue_authority_guard,
                                std::sync::Arc::clone(teardown_slot),
                                std::sync::Arc::clone(driver),
                                run_id.clone(),
                                nondirected_input_ids.clone(),
                                !staged_directed_interaction_ids.is_empty(),
                                OwnedRuntimeLoopTerminalization::Failed {
                                    failure: CoreApplyFailureCause::primitive_rejected(
                                        conflict.to_string(),
                                    ),
                                    completion_reason: completion_reason.clone(),
                                },
                            )
                            .await
                            {
                                Ok(outcome) => outcome,
                                Err(error) => {
                                    tracing::error!(
                                        %run_id,
                                        %error,
                                        "owned primitive-rejection terminalization ended without an outcome"
                                    );
                                    return true;
                                }
                            };
                        match terminalization_outcome {
                            OwnedRuntimeLoopTerminalizationOutcome::Persisted => {}
                            OwnedRuntimeLoopTerminalizationOutcome::Rejected(error) => {
                                tracing::error!(
                                    %run_id,
                                    %error,
                                    "failed to persist primitive-rejection terminalization"
                                );
                                drop(queue_authority_guard);
                                return stop_runtime_loop_executor_from_dsl_effect(
                                    driver,
                                    completions,
                                    executor,
                                    format!("runtime primitive rejection snapshot failed: {error}"),
                                    handoff,
                                    turn_finalization_guard,
                                )
                                .await;
                            }
                            OwnedRuntimeLoopTerminalizationOutcome::CleanupCarrierCorrupt(
                                error,
                            ) => {
                                tracing::error!(
                                    %run_id,
                                    %error,
                                    "primitive-rejection recovery carrier became corrupt"
                                );
                                drop(queue_authority_guard);
                                return true;
                            }
                        }
                        if !staged_directed_interaction_ids.is_empty() {
                            if let Err(error) =
                                drain_recovered_interaction_terminal_outboxes_under_authority(
                                    driver,
                                    completions,
                                    executor,
                                    &queue_authority_guard,
                                )
                                .await
                            {
                                tracing::error!(
                                    %run_id,
                                    %error,
                                    "primitive-rejection directed terminal drain remains pending"
                                );
                                drop(queue_authority_guard);
                                return true;
                            }
                            if let Err(error) =
                                teardown_slot.clear_owned_terminalization_directed_drain(&run_id)
                            {
                                tracing::error!(
                                    %run_id,
                                    %error,
                                    "primitive-rejection directed drain lost its recovery carrier"
                                );
                                drop(queue_authority_guard);
                                return true;
                            }
                        }
                        resolve_machine_terminal_completion_waiters(
                            driver,
                            completions,
                            queue_authority_guard,
                            teardown_slot,
                            &nondirected_input_ids,
                            &run_id,
                            completion_reason,
                        )
                        .await;
                        // Same backlog semantics as a failed apply: other
                        // queued work must not strand behind the rolled-back
                        // batch until an unrelated external wake.
                        match resolve_failed_batch_backlog(
                            driver,
                            completions,
                            executor,
                            &input_ids,
                            handoff,
                            turn_finalization_guard,
                        )
                        .await
                        {
                            FailedBatchBacklogOutcome::ContinueProcessing => continue,
                            FailedBatchBacklogOutcome::Park => return false,
                            FailedBatchBacklogOutcome::Stopped(should_stop) => {
                                return should_stop;
                            }
                        }
                    }
                };
                if let Err(error) =
                    prepare_turn_state_for_primitive(driver, &run_id, &primitive).await
                {
                    tracing::error!(%run_id, error = %error, "failed to start runtime turn state");
                    let completion_reason =
                        format!("runtime turn-state preparation failed: {error}");
                    let nondirected_input_ids = if completions.is_some() {
                        nondirected_completion_input_ids(
                            &input_ids,
                            &staged_directed_interaction_ids,
                        )
                    } else {
                        Vec::new()
                    };
                    let (queue_authority_guard, terminalization_outcome) =
                        match realize_runtime_loop_terminal_owned(
                            queue_authority_guard,
                            std::sync::Arc::clone(teardown_slot),
                            std::sync::Arc::clone(driver),
                            run_id.clone(),
                            nondirected_input_ids.clone(),
                            !staged_directed_interaction_ids.is_empty(),
                            OwnedRuntimeLoopTerminalization::Failed {
                                failure: CoreApplyFailureCause::executor_internal(
                                    error.to_string(),
                                ),
                                completion_reason: completion_reason.clone(),
                            },
                        )
                        .await
                        {
                            Ok(outcome) => outcome,
                            Err(terminalization_error) => {
                                tracing::error!(
                                    %run_id,
                                    error = %terminalization_error,
                                    "owned turn-state failure terminalization ended without an outcome"
                                );
                                return true;
                            }
                        };
                    match terminalization_outcome {
                        OwnedRuntimeLoopTerminalizationOutcome::Persisted => {}
                        OwnedRuntimeLoopTerminalizationOutcome::Rejected(terminalization_error) => {
                            tracing::error!(
                                %run_id,
                                error = %terminalization_error,
                                "failed to persist turn-state preparation terminalization"
                            );
                            drop(queue_authority_guard);
                            return stop_runtime_loop_executor_from_dsl_effect(
                                driver,
                                completions,
                                executor,
                                format!(
                                    "runtime turn-state preparation snapshot failed: {terminalization_error}"
                                ),
                                handoff,
                                turn_finalization_guard,
                            )
                            .await;
                        }
                        OwnedRuntimeLoopTerminalizationOutcome::CleanupCarrierCorrupt(
                            terminalization_error,
                        ) => {
                            tracing::error!(
                                %run_id,
                                error = %terminalization_error,
                                "turn-state failure recovery carrier became corrupt"
                            );
                            drop(queue_authority_guard);
                            return true;
                        }
                    }
                    if !staged_directed_interaction_ids.is_empty() {
                        if let Err(terminalization_error) =
                            drain_recovered_interaction_terminal_outboxes_under_authority(
                                driver,
                                completions,
                                executor,
                                &queue_authority_guard,
                            )
                            .await
                        {
                            tracing::error!(
                                %run_id,
                                error = %terminalization_error,
                                "turn-state failure directed terminal drain remains pending"
                            );
                            drop(queue_authority_guard);
                            return true;
                        }
                        if let Err(terminalization_error) =
                            teardown_slot.clear_owned_terminalization_directed_drain(&run_id)
                        {
                            tracing::error!(
                                %run_id,
                                error = %terminalization_error,
                                "turn-state failure directed drain lost its recovery carrier"
                            );
                            drop(queue_authority_guard);
                            return true;
                        }
                    }
                    resolve_machine_terminal_completion_waiters(
                        driver,
                        completions,
                        queue_authority_guard,
                        teardown_slot,
                        &nondirected_input_ids,
                        &run_id,
                        completion_reason,
                    )
                    .await;
                    // Same backlog semantics as a failed apply: other queued
                    // work must not strand behind the rolled-back batch until
                    // an unrelated external wake.
                    match resolve_failed_batch_backlog(
                        driver,
                        completions,
                        executor,
                        &input_ids,
                        handoff,
                        turn_finalization_guard,
                    )
                    .await
                    {
                        FailedBatchBacklogOutcome::ContinueProcessing => continue,
                        FailedBatchBacklogOutcome::Park => return false,
                        FailedBatchBacklogOutcome::Stopped(should_stop) => return should_stop,
                    }
                }
                drop(queue_authority_guard);

                let directed_interaction_ids = staged_directed_interaction_ids;

                // Execute outside the driver lock (this calls start_turn, which is slow)
                let result = executor.apply(run_id.clone(), primitive).await;

                // Lock again to update driver state
                let d = driver.lock().await;
                match result {
                    Ok(output) => {
                        drop(d);
                        let terminal_authority_guard = match authority_binding
                            .lock_current_driver_authority(driver, "runtime loop terminal commit")
                            .await
                        {
                            Ok(guard) => guard,
                            Err(_) => {
                                if let Some(completions) = completions.as_ref() {
                                    let mut completions = completions.lock().await;
                                    fail_completion_waiters(
                                        &mut completions,
                                        &input_ids,
                                        "runtime session unregistered before terminal commit",
                                    );
                                }
                                return true;
                            }
                        };
                        let meerkat_core::lifecycle::core_executor::CoreApplyOutput {
                            receipt,
                            session_snapshot,
                            terminal,
                        } = output;
                        // A resume against a session that has no pending
                        // boundary is a successful terminal observation, but
                        // the executor must not remain attached afterward.
                        // Commit and resolve this batch first, then exit so the
                        // loop guard publishes the exact executor to the
                        // machine-owned unregister saga.
                        let teardown_after_commit = matches!(
                            terminal.as_ref(),
                            Some(CoreApplyTerminal::NoPendingBoundary)
                        );
                        let committed_session_snapshot = session_snapshot.clone();
                        let machine_terminal_error = match terminal.as_ref() {
                            Some(CoreApplyTerminal::MachineTerminalFailure { error }) => {
                                Some(error.clone())
                            }
                            _ => None,
                        };
                        let (terminal_authority_guard, commit_outcome) =
                            match commit_runtime_loop_output_owned(
                                terminal_authority_guard,
                                std::sync::Arc::clone(teardown_slot),
                                std::sync::Arc::clone(driver),
                                run_id.clone(),
                                input_ids.clone(),
                                receipt,
                                session_snapshot,
                                directed_interaction_ids.clone(),
                                terminal.clone(),
                                machine_terminal_error.clone(),
                                directed_interaction_ids.is_empty() && completions.is_some(),
                            )
                            .await
                            {
                                Ok(outcome) => outcome,
                                Err(error) => {
                                    tracing::error!(
                                        %run_id,
                                        %error,
                                        "owned runtime-loop commit task failed closed"
                                    );
                                    return true;
                                }
                            };
                        let commit_result = match commit_outcome {
                            OwnedRuntimeLoopCommitOutcome::Committed => Ok(()),
                            OwnedRuntimeLoopCommitOutcome::Rejected(error) => Err(error),
                            OwnedRuntimeLoopCommitOutcome::CleanupCarrierCorrupt(error) => {
                                tracing::error!(
                                    %run_id,
                                    %error,
                                    "committed non-directed run lost its exact recovery carrier"
                                );
                                // Do not publish a competing runless terminal.
                                // The retained PreparingPersistence carrier makes
                                // teardown fail closed rather than discard an
                                // ambiguously committed waiter payload.
                                drop(terminal_authority_guard);
                                return true;
                            }
                        };
                        if let Err(err) = commit_result {
                            tracing::error!(%run_id, error = %err, "failed to commit runtime loop run");
                            let projection_cleanup_error =
                                abort_rejected_run_projections_after_commit_failure(executor)
                                    .await
                                    .err();
                            if let Some(cleanup_error) = projection_cleanup_error.as_ref() {
                                tracing::error!(
                                    %run_id,
                                    error = %cleanup_error,
                                    "failed to abort rejected runtime run projections after commit failure"
                                );
                            }
                            if projection_cleanup_error.is_none() {
                                teardown_slot.complete_rejected_commit_abort(&run_id);
                            }
                            let failure_detail = match projection_cleanup_error {
                                Some(cleanup_error) => format!(
                                    "runtime loop commit failed: {err}; rejected run projection cleanup also failed: {cleanup_error}"
                                ),
                                None => format!("runtime loop commit failed: {err}"),
                            };
                            let completion_error =
                                meerkat_core::TurnErrorMetadata::runtime_apply_failure(
                                    failure_detail.clone(),
                                );
                            if directed_interaction_ids.is_empty() {
                                // Ordinary callers can observe the richer
                                // per-run finalization failure immediately.
                                // A directed turn, however, also requires one
                                // exact durable Interaction terminal.  The
                                // failed commit rolled its run outbox back, so
                                // resolving that waiter here would split it
                                // from the runless RuntimeTerminated event that
                                // the canonical stop path publishes below.
                                // Leave directed waiters pending until that
                                // single durable stop authority resolves both
                                // the event and the waiter.
                                resolve_runtime_completion_waiters(
                                    driver,
                                    completions,
                                    terminal_authority_guard,
                                    &input_ids,
                                    &run_id,
                                    terminal.as_ref(),
                                    crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed,
                                    Some(completion_error),
                                )
                                .await;
                            } else {
                                drop(terminal_authority_guard);
                            }
                            let should_stop = stop_runtime_loop_executor_from_dsl_effect(
                                driver,
                                completions,
                                executor,
                                format!(
                                    "runtime loop commit failed for run {run_id}: {failure_detail}"
                                ),
                                handoff,
                                turn_finalization_guard,
                            )
                            .await;
                            return should_stop;
                        }

                        let authoritative_checkpoint = match reconcile_compaction_projection_outbox(
                            driver, executor,
                        )
                        .await
                        {
                            Ok(checkpoint) => checkpoint,
                            Err(err) => {
                                tracing::error!(
                                    %run_id,
                                    error = %err,
                                    "failed to finalize committed compaction projection outbox"
                                );
                                let completion_error =
                                    meerkat_core::TurnErrorMetadata::runtime_apply_failure(
                                        format!(
                                            "runtime compaction projection finalization failed after commit: {err}"
                                        ),
                                    );
                                let publication_error =
                                    finalize_publish_and_resolve_runtime_terminals(
                                        driver,
                                        completions,
                                        terminal_authority_guard,
                                        teardown_slot,
                                        executor,
                                        &input_ids,
                                        &directed_interaction_ids,
                                        &run_id,
                                        completion_terminal_observation(terminal.as_ref()),
                                        terminal.as_ref(),
                                        crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed,
                                        Some(completion_error.clone()),
                                    )
                                    .await
                                    .err();
                                if let Some(publication_error) = publication_error.as_ref() {
                                    tracing::error!(
                                        %run_id,
                                        error = %publication_error,
                                        "failed to durably publish directed terminal after compaction finalization failure"
                                    );
                                }
                                if publication_error.is_some()
                                    && directed_interaction_ids.is_empty()
                                {
                                    // No durable interaction outbox can retry
                                    // this batch. Exit through the abnormal
                                    // handoff so teardown consumes the retained
                                    // non-directed carrier before any runless
                                    // stop terminal is minted.
                                    return true;
                                }
                                if publication_error.is_some()
                                    && drain_recovered_interaction_terminal_outboxes_until_clear(
                                        driver,
                                        completions,
                                        executor,
                                        authority_binding,
                                        true,
                                        RuntimeProjectionRecoveryAuthority::RuntimeLoop,
                                    )
                                    .await
                                {
                                    return true;
                                }
                                return stop_runtime_loop_executor_from_dsl_effect(
                                        driver,
                                        completions,
                                        executor,
                                        format!(
                                            "runtime compaction projection finalization failed after commit for run {run_id}: {err}"
                                        ),
                                        handoff,
                                        turn_finalization_guard,
                                    )
                                    .await;
                            }
                        };

                        let checkpoint_result = match (
                            authoritative_checkpoint,
                            committed_session_snapshot.as_deref(),
                        ) {
                            (Some(_), _) => Ok(()),
                            (None, Some(session_snapshot)) => {
                                executor
                                    .checkpoint_committed_session_snapshot(session_snapshot)
                                    .await
                            }
                            (None, None) => Ok(()),
                        };
                        if let Err(err) = checkpoint_result {
                            tracing::error!(
                                %run_id,
                                error = %err,
                                "failed to checkpoint committed runtime session snapshot"
                            );
                            let completion_error =
                                meerkat_core::TurnErrorMetadata::runtime_apply_failure(format!(
                                    "runtime session checkpoint failed after commit: {err}"
                                ));
                            let publication_error =
                                finalize_publish_and_resolve_runtime_terminals(
                                    driver,
                                    completions,
                                    terminal_authority_guard,
                                    teardown_slot,
                                    executor,
                                    &input_ids,
                                    &directed_interaction_ids,
                                    &run_id,
                                    completion_terminal_observation(terminal.as_ref()),
                                    terminal.as_ref(),
                                    crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed,
                                    Some(completion_error.clone()),
                                )
                                .await
                                .err();
                            if let Some(publication_error) = publication_error.as_ref() {
                                tracing::error!(
                                    %run_id,
                                    error = %publication_error,
                                    "failed to durably publish directed terminal after checkpoint failure"
                                );
                            }
                            if publication_error.is_some() && directed_interaction_ids.is_empty() {
                                return true;
                            }
                            if publication_error.is_some()
                                && drain_recovered_interaction_terminal_outboxes_until_clear(
                                    driver,
                                    completions,
                                    executor,
                                    authority_binding,
                                    true,
                                    RuntimeProjectionRecoveryAuthority::RuntimeLoop,
                                )
                                .await
                            {
                                return true;
                            }
                            let should_stop = stop_runtime_loop_executor_from_dsl_effect(
                                driver,
                                completions,
                                executor,
                                format!(
                                    "runtime session checkpoint failed after commit for run {run_id}: {err}"
                                ),
                                handoff,
                                turn_finalization_guard,
                            )
                            .await;
                            return should_stop;
                        }

                        if let Err(err) = finalize_publish_and_resolve_runtime_terminals(
                            driver,
                            completions,
                            terminal_authority_guard,
                            teardown_slot,
                            executor,
                            &input_ids,
                            &directed_interaction_ids,
                            &run_id,
                            completion_terminal_observation(terminal.as_ref()),
                            terminal.as_ref(),
                            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
                            machine_terminal_error,
                        )
                        .await
                        {
                            tracing::error!(
                                %run_id,
                                error = %err,
                                "failed to durably publish directed interaction terminal"
                            );
                            if directed_interaction_ids.is_empty() {
                                return true;
                            }
                            if drain_recovered_interaction_terminal_outboxes_until_clear(
                                driver,
                                completions,
                                executor,
                                authority_binding,
                                true,
                                RuntimeProjectionRecoveryAuthority::RuntimeLoop,
                            )
                            .await
                            {
                                return true;
                            }
                            continue;
                        }
                        if teardown_after_commit {
                            handoff.disposition =
                                RuntimeLoopTeardownDisposition::UnregisterRequired;
                            if let Err(error) = authority_binding
                                .begin_durable_unregister_for_teardown(driver)
                                .await
                            {
                                // The typed handoff disposition is the
                                // fail-closed fallback. Its watcher starts the
                                // unregister saga, which must persist Draining
                                // before taking the exact executor.
                                tracing::error!(
                                    session_id = %authority_binding.session_id,
                                    %error,
                                    "no-pending runtime apply could not persist BeginUnregister before loop handoff"
                                );
                            }
                            return true;
                        }
                    }
                    Err(e) => {
                        drop(d);
                        let teardown_required = e.requires_runtime_teardown();
                        if teardown_required {
                            handoff.disposition =
                                RuntimeLoopTeardownDisposition::UnregisterRequired;
                        }
                        let terminal_authority_guard = match authority_binding
                            .lock_current_driver_authority(driver, "runtime loop terminal failure")
                            .await
                        {
                            Ok(guard) => guard,
                            Err(_) => {
                                if let Some(completions) = completions.as_ref() {
                                    let mut completions = completions.lock().await;
                                    fail_completion_waiters(
                                        &mut completions,
                                        &input_ids,
                                        "runtime session unregistered before terminal failure",
                                    );
                                }
                                return true;
                            }
                        };
                        let cancelled = e.is_cancelled();
                        let executor_stopped = matches!(&e, CoreExecutorError::Stopped);
                        let legacy_machine_terminal =
                            matches!(&e, CoreExecutorError::TerminalFailure { .. });
                        let error_msg = e.to_string();
                        if executor_stopped || legacy_machine_terminal {
                            // `Stopped` and the legacy error-shaped terminal
                            // are fail-closed executor exits. Neither carries
                            // the required receipt/session witness, so neither
                            // may enter RunFailed/retry or consume an applied
                            // terminal. The sole failed-but-applied route is
                            // the successful `CoreApplyOutput` carrier above.
                            drop(terminal_authority_guard);
                            return stop_runtime_loop_executor_from_dsl_effect(
                                driver,
                                completions,
                                executor,
                                format!(
                                    "runtime executor stopped during run {run_id}: {error_msg}"
                                ),
                                handoff,
                                turn_finalization_guard,
                            )
                            .await;
                        }
                        let has_directed_inputs = !directed_interaction_ids.is_empty();
                        let nondirected_input_ids = if completions.is_none()
                            || (cancelled && has_directed_inputs)
                        {
                            Vec::new()
                        } else {
                            nondirected_completion_input_ids(&input_ids, &directed_interaction_ids)
                        };
                        let terminalization = if cancelled {
                            OwnedRuntimeLoopTerminalization::Cancelled
                        } else {
                            OwnedRuntimeLoopTerminalization::Failed {
                                failure: e.apply_failure_cause(),
                                completion_reason: format!("apply failed: {error_msg}"),
                            }
                        };
                        let (terminal_authority_guard, terminalization_outcome) =
                            match realize_runtime_loop_terminal_owned(
                                terminal_authority_guard,
                                std::sync::Arc::clone(teardown_slot),
                                std::sync::Arc::clone(driver),
                                run_id.clone(),
                                nondirected_input_ids.clone(),
                                has_directed_inputs,
                                terminalization,
                            )
                            .await
                            {
                                Ok(outcome) => outcome,
                                Err(error) => {
                                    tracing::error!(
                                        %run_id,
                                        %error,
                                        "owned runtime failure terminalization ended without an outcome"
                                    );
                                    return true;
                                }
                            };
                        match terminalization_outcome {
                            OwnedRuntimeLoopTerminalizationOutcome::Persisted => {}
                            OwnedRuntimeLoopTerminalizationOutcome::Rejected(error) => {
                                tracing::error!(
                                    %run_id,
                                    %error,
                                    "failed to persist runtime terminalization"
                                );
                                drop(terminal_authority_guard);
                                return stop_runtime_loop_executor_from_dsl_effect(
                                    driver,
                                    completions,
                                    executor,
                                    format!("runtime failure snapshot failed: {error}"),
                                    handoff,
                                    turn_finalization_guard,
                                )
                                .await;
                            }
                            OwnedRuntimeLoopTerminalizationOutcome::CleanupCarrierCorrupt(
                                error,
                            ) => {
                                tracing::error!(
                                    %run_id,
                                    %error,
                                    "runtime terminalization recovery carrier became corrupt"
                                );
                                drop(terminal_authority_guard);
                                return true;
                            }
                        }
                        let mut terminal_authority_guard = Some(terminal_authority_guard);
                        if cancelled {
                            let Some(publication_authority_guard) = terminal_authority_guard.take()
                            else {
                                tracing::error!(
                                    %run_id,
                                    "cancelled completion lost its terminal authority guard"
                                );
                                return true;
                            };
                            if let Err(error) =
                                finalize_publish_and_resolve_runtime_terminals(
                                    driver,
                                    completions,
                                    publication_authority_guard,
                                    teardown_slot,
                                    executor,
                                    &input_ids,
                                    &directed_interaction_ids,
                                    &run_id,
                                    crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::MachineTerminal,
                                    None,
                                    crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
                                    None,
                                )
                                .await
                            {
                                tracing::error!(
                                    %run_id,
                                    error = %error,
                                    "failed to durably publish directed cancellation terminal"
                                );
                                if !has_directed_inputs {
                                    return true;
                                }
                                if drain_recovered_interaction_terminal_outboxes_until_clear(
                                    driver,
                                    completions,
                                    executor,
                                    authority_binding,
                                    true,
                                    RuntimeProjectionRecoveryAuthority::RuntimeLoop,
                                )
                                .await
                                {
                                    return true;
                                }
                            }
                            if has_directed_inputs
                                && let Err(error) = teardown_slot
                                    .clear_owned_terminalization_directed_drain(&run_id)
                            {
                                tracing::error!(
                                    %run_id,
                                    %error,
                                    "cancelled directed drain lost its recovery carrier"
                                );
                                return true;
                            }
                        } else {
                            if has_directed_inputs {
                                let Some(drain_authority_guard) = terminal_authority_guard.as_ref()
                                else {
                                    tracing::error!(
                                        %run_id,
                                        "runtime failure lost authority before directed drain"
                                    );
                                    return true;
                                };
                                if let Err(error) =
                                    drain_recovered_interaction_terminal_outboxes_under_authority(
                                        driver,
                                        completions,
                                        executor,
                                        drain_authority_guard,
                                    )
                                    .await
                                {
                                    tracing::error!(
                                        %run_id,
                                        %error,
                                        "runtime failure directed terminal drain remains pending"
                                    );
                                    drop(terminal_authority_guard.take());
                                    if drain_recovered_interaction_terminal_outboxes_until_clear(
                                        driver,
                                        completions,
                                        executor,
                                        authority_binding,
                                        true,
                                        RuntimeProjectionRecoveryAuthority::RuntimeLoop,
                                    )
                                    .await
                                    {
                                        return true;
                                    }
                                }
                                if let Err(error) = teardown_slot
                                    .clear_owned_terminalization_directed_drain(&run_id)
                                {
                                    tracing::error!(
                                        %run_id,
                                        %error,
                                        "runtime failure directed drain lost its recovery carrier"
                                    );
                                    return true;
                                }
                            }
                            if !nondirected_input_ids.is_empty() {
                                if terminal_authority_guard.is_none() {
                                    terminal_authority_guard = match authority_binding
                                        .lock_current_driver_authority(
                                            driver,
                                            "runtime failure non-directed completion recovery",
                                        )
                                        .await
                                    {
                                        Ok(guard) => Some(guard),
                                        Err(_) => return true,
                                    };
                                }
                                let Some(completion_authority_guard) =
                                    terminal_authority_guard.take()
                                else {
                                    tracing::error!(
                                        %run_id,
                                        "runtime failure completion lost its terminal authority guard"
                                    );
                                    return true;
                                };
                                resolve_machine_terminal_completion_waiters(
                                    driver,
                                    completions,
                                    completion_authority_guard,
                                    teardown_slot,
                                    &nondirected_input_ids,
                                    &run_id,
                                    format!("apply failed: {error_msg}"),
                                )
                                .await;
                            }
                        }
                        if teardown_required {
                            // This is an ownership handoff, not a retryable
                            // failed batch. Returning exits the loop; its RAII
                            // guard publishes the exact executor before the
                            // watcher starts canonical unregister. Calling
                            // unregister from inside CoreExecutor::apply would
                            // self-join this loop.
                            drop(terminal_authority_guard);
                            if let Err(error) = authority_binding
                                .begin_durable_unregister_for_teardown(driver)
                                .await
                            {
                                tracing::error!(
                                    session_id = %authority_binding.session_id,
                                    %error,
                                    "teardown-required apply could not persist BeginUnregister before loop handoff"
                                );
                            }
                            return true;
                        }
                        // The stop path inside the backlog resolution can
                        // re-enter the machine — never call it under the
                        // authority guard (ask 21c deadlock pattern).
                        drop(terminal_authority_guard);
                        match resolve_failed_batch_backlog(
                            driver,
                            completions,
                            executor,
                            &input_ids,
                            handoff,
                            turn_finalization_guard,
                        )
                        .await
                        {
                            FailedBatchBacklogOutcome::ContinueProcessing => continue,
                            FailedBatchBacklogOutcome::Park => return false,
                            FailedBatchBacklogOutcome::Stopped(should_stop) => {
                                return should_stop;
                            }
                        }
                    }
                }
            }
            RuntimeLoopDequeueOutcome::ProjectionMismatch { input_ids, reason } => {
                if let Some(completions) = completions.as_ref() {
                    let mut completions = completions.lock().await;
                    fail_completion_waiters(&mut completions, &input_ids, reason.clone());
                }
                drop(queue_authority_guard);
                return stop_runtime_loop_executor_from_dsl_effect(
                    driver,
                    completions,
                    executor,
                    reason,
                    handoff,
                    turn_finalization_guard,
                )
                .await;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::input::*;
    use chrono::Utc;
    use meerkat_core::lifecycle::run_primitive::{
        ConversationAppend, ConversationAppendRole, CoreRenderable, PeerResponseTerminalApplyIntent,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::time::Duration;

    use meerkat_core::ops_lifecycle::{
        OperationKind, OperationResult, OperationSource, OperationSpec, OpsLifecycleRegistry,
    };
    use meerkat_core::types::SessionId;

    const TEST_PEER_RESPONSE_ROUTE_ID: &str = "11111111-1111-4111-8111-111111111111";
    const TEST_PEER_RESPONSE_REQUEST_ID: &str = "22222222-2222-4222-8222-222222222222";
    const TEST_PEER_RESPONSE_REQUEST_ID_2: &str = "33333333-3333-4333-8333-333333333333";

    #[test]
    fn compatibility_checkpoint_clears_exact_finalized_compaction_intent() {
        let mut session = meerkat_core::Session::new();
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("verbose context one"),
        ));
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("verbose context two"),
        ));
        let parent = session.transcript_revision().unwrap();
        session
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                vec![meerkat_core::types::Message::User(
                    meerkat_core::types::UserMessage::compaction_summary("compacted context"),
                )],
                meerkat_core::TranscriptRewriteReason::new("compaction"),
                Some("runtime-loop-checkpoint-test".to_string()),
                Some(parent),
            )
            .unwrap();
        let mut encoded = serde_json::to_value(&session).unwrap();
        encoded["metadata"][meerkat_core::SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["commits"][0]["selection"] = serde_json::json!({
            "type": "compaction_message_range",
            "range": { "start": 0, "end": 2 }
        });
        let mut session: meerkat_core::Session = serde_json::from_value(encoded).unwrap();
        let commit = session
            .transcript_history_state()
            .unwrap()
            .unwrap()
            .commits
            .last()
            .unwrap()
            .clone();
        let intent = meerkat_core::CompactionProjectionIntent {
            projection: serde_json::from_value(serde_json::json!({
                "session_id": session.id(),
                "parent_revision": &commit.parent_revision,
                "revision": &commit.revision,
                "commit_fingerprint": "sha256:bc151128bd2f8e459c3b111114ca86f1a6270b048a971fffb6d86a032d61eaa7",
            }))
            .unwrap(),
            summary_tokens: 1,
            messages_before: 2,
            messages_after: 1,
        };
        session
            .add_compaction_projection_intent(intent.clone())
            .unwrap();
        let stale_pre_finalize = serde_json::to_vec(&session).unwrap();

        let cleaned =
            compatibility_checkpoint_after_compaction_finalize(&stale_pre_finalize, &[intent])
                .unwrap();
        let recovered: meerkat_core::Session = serde_json::from_slice(&cleaned).unwrap();
        assert!(
            recovered
                .compaction_projection_intents()
                .unwrap()
                .is_empty(),
            "compatibility checkpoint bytes must not reintroduce a finalized intent on cold resume"
        );
    }

    #[test]
    fn terminal_recovery_classifies_stale_and_corrupt_authority_as_non_retryable() {
        let stale = InteractionTerminalPublicationError::from_driver(
            "phase CAS",
            crate::RuntimeDriverError::StaleAuthority {
                reason: "superseded".to_string(),
            },
        );
        assert!(matches!(
            stale,
            InteractionTerminalPublicationError::StaleAuthority(_)
        ));

        let corrupt = InteractionTerminalPublicationError::from_generated_authority(
            "generated result",
            crate::RuntimeDriverError::Internal("impossible result class".to_string()),
        );
        assert!(matches!(
            corrupt,
            InteractionTerminalPublicationError::Corrupt(_)
        ));
    }

    fn background_spec(name: &str) -> OperationSpec {
        OperationSpec {
            id: meerkat_core::ops_lifecycle::OperationId::new(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: SessionId::new(),
            display_name: name.into(),
            source_label: "runtime-loop-test".into(),
            operation_source: None,
            child_session_id: None,
            expect_peer_channel: false,
        }
    }

    fn mob_child_spec(name: &str) -> OperationSpec {
        let child_session_id = SessionId::new();
        OperationSpec {
            id: meerkat_core::ops_lifecycle::OperationId::new(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: SessionId::new(),
            display_name: name.into(),
            source_label: "runtime-loop-test".into(),
            operation_source: Some(OperationSource::session_child(child_session_id.clone())),
            child_session_id: Some(child_session_id),
            expect_peer_channel: true,
        }
    }

    fn op_result(id: &meerkat_core::ops_lifecycle::OperationId, content: &str) -> OperationResult {
        OperationResult {
            id: id.clone(),
            content: content.into(),
            is_error: false,
            duration_ms: 42,
            tokens_used: 7,
        }
    }

    fn make_shared_ephemeral_driver(runtime_id: &str) -> crate::meerkat_machine::SharedDriver {
        Arc::new(crate::tokio::sync::Mutex::new(
            crate::meerkat_machine::DriverEntry::Ephemeral(
                crate::driver::ephemeral::EphemeralRuntimeDriver::new(
                    crate::identifiers::LogicalRuntimeId::new(runtime_id),
                ),
            ),
        ))
    }

    fn stop_runtime_executor_effect(reason: &str) -> crate::effect::RuntimeEffect {
        crate::effect::runtime_effect_for_test(
            crate::meerkat_machine::dsl::RuntimeEffectKind::StopRuntimeExecutor,
            reason,
        )
    }

    struct BoundaryCancelFailingExecutor {
        cancel_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::lifecycle::CoreExecutor for BoundaryCancelFailingExecutor {
        async fn apply(
            &mut self,
            _run_id: meerkat_core::lifecycle::RunId,
            _primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
        ) -> Result<
            meerkat_core::lifecycle::core_executor::CoreApplyOutput,
            meerkat_core::lifecycle::CoreExecutorError,
        > {
            unreachable!("effect handoff regression does not apply queued work")
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            Err(
                meerkat_core::lifecycle::CoreExecutorError::control_failed_runtime(
                    "synthetic missing session actor",
                ),
            )
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct RejectedAbortTeardownProjectionProbe {
        abort_calls: Arc<AtomicUsize>,
        reconcile_calls: Arc<AtomicUsize>,
        discarded: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::lifecycle::CoreExecutor for RejectedAbortTeardownProjectionProbe {
        async fn apply(
            &mut self,
            _run_id: meerkat_core::lifecycle::RunId,
            _primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
        ) -> Result<
            meerkat_core::lifecycle::core_executor::CoreApplyOutput,
            meerkat_core::lifecycle::CoreExecutorError,
        > {
            unreachable!("teardown projection recovery does not apply queued work")
        }

        async fn reconcile_committed_compaction_projections(
            &mut self,
            _intents: &[meerkat_core::CompactionProjectionIntent],
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
            if self.discarded.load(Ordering::SeqCst) {
                return Err(meerkat_core::lifecycle::CoreExecutorError::Internal(
                    "post-discard compaction reconciliation must not run".to_string(),
                ));
            }
            Ok(())
        }

        async fn abort_rejected_run_projections(
            &mut self,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            self.abort_calls.fetch_add(1, Ordering::SeqCst);
            self.discarded.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn rejected_abort_teardown_skips_post_discard_empty_reconciliation() {
        let driver = make_shared_ephemeral_driver("rejected-abort-teardown-projections");
        let teardown_slot = RuntimeLoopTeardownSlot::pending();
        let run_id = RunId::new();
        teardown_slot
            .stage_owned_commit_in_flight(&run_id)
            .expect("stage rejected commit cleanup carrier");
        teardown_slot
            .mark_owned_commit_rejected_abort_pending(&run_id)
            .expect("publish rejected-abort cleanup phase");

        let abort_calls = Arc::new(AtomicUsize::new(0));
        let reconcile_calls = Arc::new(AtomicUsize::new(0));
        let discarded = Arc::new(AtomicBool::new(false));
        let mut executor = RejectedAbortTeardownProjectionProbe {
            abort_calls: Arc::clone(&abort_calls),
            reconcile_calls: Arc::clone(&reconcile_calls),
            discarded: Arc::clone(&discarded),
        };
        let recovery = RuntimeLoopRecoveryContext {
            authority_binding: RuntimeLoopAuthorityBinding::detached_for_test(),
            completions: None,
        };

        recover_committed_runtime_projections_before_teardown(
            &driver,
            &mut executor,
            &teardown_slot,
            &recovery,
        )
        .await
        .expect("rejected projection cleanup must converge without an actorless reconcile");

        assert!(discarded.load(Ordering::SeqCst));
        assert_eq!(abort_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            reconcile_calls.load(Ordering::SeqCst),
            0,
            "whole-run abort already settled the authoritative empty outbox"
        );
        assert!(
            teardown_slot.owned_commit_cleanup_state().is_none(),
            "successful cleanup must clear the exact rejected-commit carrier"
        );
    }

    #[tokio::test]
    async fn teardown_publication_notify_race_does_not_miss_wakeups() {
        for _ in 0..256 {
            let slot = RuntimeLoopTeardownSlot::pending();
            let stop_calls = Arc::new(AtomicUsize::new(0));
            let apply_calls = Arc::new(AtomicUsize::new(0));
            let guard = RuntimeLoopHandoffGuard::new(
                Arc::clone(&slot),
                Box::new(
                    crate::control_plane::test_support::StopFailingExecutor::new(
                        stop_calls,
                        apply_calls,
                    ),
                ),
            );
            let waiter = {
                let slot = Arc::clone(&slot);
                tokio::spawn(async move { slot.wait_until_published().await })
            };
            tokio::task::yield_now().await;
            drop(guard);
            tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .expect("teardown publication waiter must not miss notify_waiters")
                .expect("teardown publication waiter task must not panic");
        }
    }

    #[tokio::test]
    async fn startup_guard_fails_waiter_on_pre_ready_exit() {
        let startup = RuntimeLoopStartupSlot::pending();
        drop(RuntimeLoopStartupGuard::new(Arc::clone(&startup)));

        let error = tokio::time::timeout(Duration::from_secs(1), startup.wait())
            .await
            .expect("pre-ready exit must publish startup failure")
            .expect_err("pre-ready exit cannot be reported as ready");
        assert!(
            error
                .to_string()
                .contains("exited before startup reconciliation completed")
        );
    }

    #[tokio::test]
    async fn startup_guard_fails_waiter_when_task_is_aborted_before_ready() {
        let startup = RuntimeLoopStartupSlot::pending();
        let startup_guard = RuntimeLoopStartupGuard::new(Arc::clone(&startup));
        let task = tokio::spawn(async move {
            let _startup_guard = startup_guard;
            std::future::pending::<()>().await;
        });
        task.abort();
        let _ = task.await;

        let error = tokio::time::timeout(Duration::from_secs(1), startup.wait())
            .await
            .expect("aborted pre-ready task must publish startup failure")
            .expect_err("aborted pre-ready task cannot be reported as ready");
        assert!(
            error
                .to_string()
                .contains("exited before startup reconciliation completed")
        );
    }

    #[test]
    fn handoff_guard_is_unserved_even_when_loop_future_is_never_polled() {
        let slot = RuntimeLoopTeardownSlot::pending();
        let guard = RuntimeLoopHandoffGuard::new(
            Arc::clone(&slot),
            Box::new(
                crate::control_plane::test_support::StopFailingExecutor::new(
                    Arc::new(AtomicUsize::new(0)),
                    Arc::new(AtomicUsize::new(0)),
                ),
            ),
        );
        let never_polled = async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        };

        // Dropping an unpolled spawned future drops its captures without ever
        // executing the async body.  The handoff must still report that its
        // serving gate never opened.
        drop(never_polled);
        assert_eq!(
            slot.disposition(),
            RuntimeLoopTeardownDisposition::UnservedAttachment
        );
        assert!(
            !slot.disposition().requires_unregister(),
            "the exact startup/pending owner must choose boundary-aware cleanup"
        );
    }

    #[tokio::test]
    async fn unregister_error_before_loop_publication_replays_stop_receipt_and_keeps_retry() {
        let slot = RuntimeLoopTeardownSlot::pending();
        let (stop_completion, stop_result) = tokio::sync::oneshot::channel();
        let retained_error = crate::RuntimeDriverError::Internal(
            "synthetic BeginUnregister persistence failure".into(),
        );

        // The unregister supervisor can publish this result before a blocked
        // loop returns from apply and deposits its executor.
        slot.acknowledge_unregister_result(Err(retained_error.clone()));
        slot.publish(RuntimeLoopExitHandoff {
            executor: Box::new(
                crate::control_plane::test_support::StopFailingExecutor::new(
                    Arc::new(AtomicUsize::new(0)),
                    Arc::new(AtomicUsize::new(0)),
                ),
            ),
            terminal: RuntimeLoopTerminalHandoff {
                stop_completion: Some(stop_completion),
                ..RuntimeLoopTerminalHandoff::default()
            },
        });

        let observed = tokio::time::timeout(Duration::from_secs(1), stop_result)
            .await
            .expect("late-published stop receipt must not hang")
            .expect("stop receipt sender must remain owned until replay")
            .expect_err("the retained unregister failure must be replayed exactly");
        assert_eq!(observed.to_string(), retained_error.to_string());

        let state = slot
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let RuntimeLoopTeardownState::Ready(handoff) = &*state else {
            panic!("failed pre-publication unregister must retain the exact executor for retry");
        };
        assert!(handoff.terminal.stop_completion.is_none());
        assert!(
            handoff.terminal.executor_stop_retry_reason.is_some(),
            "replaying the caller acknowledgement must not erase the stop-hook obligation"
        );
    }

    #[tokio::test]
    async fn handoff_guard_drop_on_early_return_executes_required_stop_hook() {
        let slot = RuntimeLoopTeardownSlot::pending();
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let guard = RuntimeLoopHandoffGuard::new(
            Arc::clone(&slot),
            Box::new(
                crate::control_plane::test_support::ApplyFailingExecutor::new(
                    Arc::new(AtomicUsize::new(0)),
                    Arc::clone(&stop_calls),
                ),
            ),
        );
        drop(guard); // models an early cursor-authority error return

        {
            let state = slot
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let RuntimeLoopTeardownState::Ready(handoff) = &*state else {
                panic!("early return must publish the exact executor");
            };
            assert!(handoff.terminal.executor_stop_retry_reason.is_some());
        }

        slot.cleanup_once(&make_shared_ephemeral_driver("early-return-stop-hook"))
            .await
            .expect("unregister cleanup must realize the retained stop obligation");
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_boundary_cancel_retains_canonical_stop_before_cleanup() {
        let driver = make_shared_ephemeral_driver("boundary-cancel-stop-handoff");
        let cancel_calls = Arc::new(AtomicUsize::new(0));
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = BoundaryCancelFailingExecutor {
            cancel_calls: Arc::clone(&cancel_calls),
            stop_calls: Arc::clone(&stop_calls),
        };
        let mut terminal_handoff = RuntimeLoopTerminalHandoff::default();

        let should_stop = apply_runtime_loop_effect_and_record_handoff(
            &driver,
            None,
            &mut executor,
            crate::effect::runtime_effect_for_test(
                crate::meerkat_machine::dsl::RuntimeEffectKind::CancelAfterBoundary,
                "boundary cancel",
            ),
            &mut terminal_handoff,
            None,
        )
        .await;

        assert!(should_stop, "failed boundary cancel must stop the loop");
        assert_eq!(cancel_calls.load(Ordering::SeqCst), 1);
        assert_eq!(stop_calls.load(Ordering::SeqCst), 0);
        assert!(
            terminal_handoff
                .executor_stop_retry_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("failed before canonical stop")),
            "non-stop effect failure must retain the canonical stop obligation"
        );
        assert!(
            terminal_handoff.deferred_executor_stop_error.is_none(),
            "the abnormal-exit stop hook should run on the first cleanup attempt"
        );

        let slot = RuntimeLoopTeardownSlot::pending();
        slot.publish(RuntimeLoopExitHandoff {
            executor: Box::new(executor),
            terminal: terminal_handoff,
        });
        slot.cleanup_once(&driver)
            .await
            .expect("cleanup must realize the retained canonical stop first");
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_stop_terminalization_does_not_rearm_the_stop_hook() {
        let mut terminal_handoff = RuntimeLoopTerminalHandoff::default();
        record_runtime_loop_effect_failure_handoff(
            &mut terminal_handoff,
            None,
            true,
            crate::control_plane::RuntimeLoopEffectFailure {
                error: crate::RuntimeDriverError::Internal(
                    "synthetic post-stop terminalization failure".to_string(),
                ),
                executor_stop_retry_reason: None,
            },
        );

        assert!(
            terminal_handoff.executor_stop_retry_reason.is_none(),
            "a successful stop hook must not be repeated after terminalization fails"
        );
        assert!(
            terminal_handoff.deferred_executor_stop_error.is_none(),
            "the retained handoff must retry terminalization immediately"
        );
    }

    #[tokio::test]
    async fn handoff_guard_drop_during_panic_executes_required_stop_hook() {
        let slot = RuntimeLoopTeardownSlot::pending();
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
            let slot = Arc::clone(&slot);
            let stop_calls = Arc::clone(&stop_calls);
            move || {
                let _guard = RuntimeLoopHandoffGuard::new(
                    slot,
                    Box::new(
                        crate::control_plane::test_support::ApplyFailingExecutor::new(
                            Arc::new(AtomicUsize::new(0)),
                            stop_calls,
                        ),
                    ),
                );
                panic!("synthetic runtime-loop panic");
            }
        }));
        assert!(panic_result.is_err());

        {
            let state = slot
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let RuntimeLoopTeardownState::Ready(handoff) = &*state else {
                panic!("panic unwinding must publish the exact executor");
            };
            assert!(handoff.terminal.executor_stop_retry_reason.is_some());
        }

        slot.cleanup_once(&make_shared_ephemeral_driver("panic-stop-hook"))
            .await
            .expect("panic handoff cleanup must realize the retained stop obligation");
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn runtime_loop_stop_effect_failure_is_fail_closed_from_helper() {
        let driver = make_shared_ephemeral_driver("stop-helper-fail-closed");
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = crate::control_plane::test_support::StopFailingExecutor::new(
            Arc::clone(&stop_calls),
            Arc::clone(&apply_calls),
        );
        let mut terminal_handoff = RuntimeLoopTerminalHandoff::default();

        let should_stop = stop_runtime_loop_executor_from_dsl_effect(
            &driver,
            None,
            &mut executor,
            "snapshot failure should stop the runtime loop".to_string(),
            &mut terminal_handoff,
            None,
        )
        .await;

        assert!(
            should_stop,
            "stop-effect failures must fail closed by stopping the runtime loop"
        );
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "stop-effect failure must not fall through to queued work"
        );
    }

    #[tokio::test]
    async fn runtime_loop_drain_effect_failure_is_fail_closed() {
        let driver = make_shared_ephemeral_driver("drain-effect-fail-closed");
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = crate::control_plane::test_support::StopFailingExecutor::new(
            Arc::clone(&stop_calls),
            Arc::clone(&apply_calls),
        );
        let (effect_tx, mut effect_rx) = tokio::sync::mpsc::channel(1);
        effect_tx
            .send(stop_runtime_executor_effect("drain failure should stop"))
            .await
            .expect("test effect should enqueue");

        let authority_binding = RuntimeLoopAuthorityBinding::detached_for_test();
        let teardown_slot = RuntimeLoopTeardownSlot::pending();
        let mut terminal_handoff = RuntimeLoopTerminalHandoff::default();
        let should_stop = process_queue(
            &driver,
            &mut executor,
            &mut effect_rx,
            None,
            &authority_binding,
            &teardown_slot,
            &mut terminal_handoff,
        )
        .await;

        assert!(
            should_stop,
            "drained executor-effect failures must stop the runtime loop"
        );
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "failed stop effect must not be followed by ordinary queue processing"
        );
    }

    /// Regression for #287: dropping the effect sender mid-run must drive the
    /// loop through the canonical stop/terminalize path (StopRuntimeExecutor),
    /// NOT exit bare as if a stop effect had already been applied. Before the
    /// fix, channel closure returned `Ok(true)` directly and `stop_calls`
    /// stayed at 0 — the executor stop was never invoked.
    #[tokio::test]
    async fn runtime_loop_effect_channel_closure_routes_through_stop_path() {
        let driver = make_shared_ephemeral_driver("effect-channel-closed-stop");
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = crate::control_plane::test_support::StopFailingExecutor::new(
            Arc::clone(&stop_calls),
            Arc::clone(&apply_calls),
        );
        // Drop the effect sender so the drain observes a disconnected channel
        // without any stop effect ever being delivered.
        let (effect_tx, mut effect_rx) = tokio::sync::mpsc::channel(1);
        drop(effect_tx);

        let authority_binding = RuntimeLoopAuthorityBinding::detached_for_test();
        let teardown_slot = RuntimeLoopTeardownSlot::pending();
        let mut terminal_handoff = RuntimeLoopTerminalHandoff::default();
        let should_stop = process_queue(
            &driver,
            &mut executor,
            &mut effect_rx,
            None,
            &authority_binding,
            &teardown_slot,
            &mut terminal_handoff,
        )
        .await;

        assert!(
            should_stop,
            "a closed effect channel must stop the runtime loop"
        );
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            1,
            "channel closure must route through StopRuntimeExecutor, not exit bare"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "channel closure must not fall through to ordinary queue processing"
        );
    }

    #[tokio::test]
    async fn runtime_loop_direct_effect_failure_exits_loop_with_channels_open() {
        let driver = make_shared_ephemeral_driver("direct-effect-fail-closed");
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let executor = crate::control_plane::test_support::StopFailingExecutor::new(
            Arc::clone(&stop_calls),
            Arc::clone(&apply_calls),
        );
        let (wake_tx, wake_rx) = tokio::sync::mpsc::channel(1);
        let (effect_tx, effect_rx) = tokio::sync::mpsc::channel(1);
        let handle = spawn_runtime_loop_with_completions(
            driver,
            Box::new(executor),
            wake_rx,
            effect_rx,
            None,
            None,
            None,
            None,
            std::sync::Weak::<crate::meerkat_machine::MeerkatMachine>::new(),
            SessionId::new(),
            false,
        );

        effect_tx
            .send(stop_runtime_executor_effect(
                "direct effect failure should stop",
            ))
            .await
            .expect("test effect should enqueue");

        tokio::time::timeout(Duration::from_secs(1), handle.loop_handle)
            .await
            .expect("runtime loop must exit after executor-effect failure")
            .expect("runtime loop task should not panic");
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(apply_calls.load(Ordering::SeqCst), 0);
        drop((wake_tx, effect_tx));
    }

    /// Row #58 residual gate: dropping the effect sender while the loop is
    /// blocked in its main `select!` (NOT inside the ready-effect drain) must
    /// also route through the canonical StopRuntimeExecutor terminalize path —
    /// never exit the loop bare.
    #[tokio::test]
    async fn runtime_loop_direct_effect_channel_closure_routes_through_stop_path() {
        let driver = make_shared_ephemeral_driver("direct-effect-channel-closed");
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let executor = crate::control_plane::test_support::StopFailingExecutor::new(
            Arc::clone(&stop_calls),
            Arc::clone(&apply_calls),
        );
        let (wake_tx, wake_rx) = tokio::sync::mpsc::channel(1);
        let (effect_tx, effect_rx) = tokio::sync::mpsc::channel(1);
        let handle = spawn_runtime_loop_with_completions(
            driver,
            Box::new(executor),
            wake_rx,
            effect_rx,
            None,
            None,
            None,
            None,
            std::sync::Weak::<crate::meerkat_machine::MeerkatMachine>::new(),
            SessionId::new(),
            false,
        );

        drop(effect_tx);

        tokio::time::timeout(Duration::from_secs(1), handle.loop_handle)
            .await
            .expect("runtime loop must exit after effect channel closure")
            .expect("runtime loop task should not panic");
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            1,
            "effect channel closure must route through StopRuntimeExecutor, not exit bare"
        );
        assert_eq!(apply_calls.load(Ordering::SeqCst), 0);
        drop(wake_tx);
    }

    /// Row #58 residual gate (sibling): dropping the wake sender must also
    /// terminalize through the canonical StopRuntimeExecutor path.
    #[tokio::test]
    async fn runtime_loop_wake_channel_closure_routes_through_stop_path() {
        let driver = make_shared_ephemeral_driver("wake-channel-closed");
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let executor = crate::control_plane::test_support::StopFailingExecutor::new(
            Arc::clone(&stop_calls),
            Arc::clone(&apply_calls),
        );
        let (wake_tx, wake_rx) = tokio::sync::mpsc::channel(1);
        let (effect_tx, effect_rx) = tokio::sync::mpsc::channel(1);
        let handle = spawn_runtime_loop_with_completions(
            driver,
            Box::new(executor),
            wake_rx,
            effect_rx,
            None,
            None,
            None,
            None,
            std::sync::Weak::<crate::meerkat_machine::MeerkatMachine>::new(),
            SessionId::new(),
            false,
        );

        drop(wake_tx);

        tokio::time::timeout(Duration::from_secs(1), handle.loop_handle)
            .await
            .expect("runtime loop must exit after wake channel closure")
            .expect("runtime loop task should not panic");
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            1,
            "wake channel closure must route through StopRuntimeExecutor, not exit bare"
        );
        assert_eq!(apply_calls.load(Ordering::SeqCst), 0);
        drop(effect_tx);
    }

    fn make_prompt(text: &str) -> Input {
        Input::Prompt(PromptInput {
            injected_context: Vec::new(),
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            content: text.into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        })
    }

    #[test]
    fn input_to_prompt_extracts_text() {
        let input = make_prompt("hello world");
        assert_eq!(input_to_prompt(&input), "hello world");
    }

    #[test]
    fn input_to_prompt_peer() {
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    display_identity: Some("Peer P".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: None,
            content: "peer message".into(),
            payload: None,
            handling_mode: None,
        });
        assert_eq!(input_to_prompt(&input), "peer message");
    }

    #[test]
    fn input_to_prompt_peer_message_uses_body_when_projection_text_is_empty() {
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            content: "plain body payload".into(),
            payload: None,
            handling_mode: None,
        });

        assert_eq!(input_to_prompt(&input), "plain body payload");
    }

    #[test]
    fn input_to_prompt_peer_request_is_runtime_owned_and_ignores_bogus_body() {
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "11111111-1111-4111-8111-111111111111".into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Request {
                request_id: "req-123".into(),
                intent: "checksum_token".into(),
            }),
            content: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({"subject": "alpha beta gamma"})),
            handling_mode: None,
        });

        let prompt = input_to_prompt(&input);
        assert!(
            prompt.starts_with("Peer request from peer_id 11111111-1111-4111-8111-111111111111")
        );
        assert!(prompt.contains("\"peer_id\":\"11111111-1111-4111-8111-111111111111\""));
        assert!(prompt.contains("\"in_reply_to\":\"req-123\""));
        assert!(prompt.contains("\"status\":\"completed\""));
        assert!(!prompt.contains("to=\""));
        assert!(prompt.contains("Do not use send_message for this reply."));
    }

    #[test]
    fn machine_batch_projection_peer_response_terminal_is_runtime_owned_from_typed_payload() {
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("Analyst".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            })),
            handling_mode: None,
        });

        let projection = runtime_input_projection_for_machine_batch(&input);
        let context = projection
            .context_append
            .expect("terminal peer response should project in machine batch");
        let CoreRenderable::SystemNotice { blocks, .. } = context.content else {
            panic!("expected typed terminal context");
        };
        assert!(matches!(
            blocks.first(),
            Some(meerkat_core::types::SystemNoticeBlock::Comms { peer, request_id, status, .. })
                if peer.as_ref().and_then(|peer| peer.display_name.as_deref()) == Some("Analyst")
                    && request_id.as_deref() == Some("018f6f79-7a82-7c4e-a552-a3b86f9630f1")
                    && status.as_deref() == Some("completed")
        ));
    }

    #[test]
    fn machine_batch_projection_peer_response_terminal_omits_payload_key_extraction() {
        // Regression: runtime must not reach into the peer response payload
        // to extract ad-hoc fields (request_intent, token, etc.) and bake
        // them into prompt text. The canonical projection is the typed
        // convention + the Result JSON verbatim; any per-fixture semantics
        // belong to the peer application, not the runtime.
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("Analyst".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            })),
            handling_mode: None,
        });

        let projection = runtime_input_projection_for_machine_batch(&input);
        let context = projection
            .context_append
            .expect("terminal peer response should project in machine batch");
        let CoreRenderable::SystemNotice { blocks, .. } = context.content else {
            panic!("expected typed terminal context");
        };
        let rendered = blocks
            .first()
            .and_then(|block| match block {
                meerkat_core::types::SystemNoticeBlock::Comms { payload, .. } => {
                    payload.as_ref().map(ToString::to_string)
                }
                _ => None,
            })
            .unwrap_or_default();
        assert!(
            !rendered.contains("Authoritative result fields:"),
            "runtime must not emit scenario-specific authoritative-field hints: {rendered}"
        );
        assert!(
            !rendered.contains("the exact token answer is"),
            "runtime must not coach the model about specific payload values: {rendered}"
        );
        assert!(
            !rendered.contains("request_intent=checksum_token"),
            "runtime must not re-flatten payload keys into prompt text: {rendered}"
        );
    }

    #[test]
    fn input_to_primitive_creates_staged() -> Result<(), String> {
        let input = make_prompt("test prompt");
        let input_id = input.id().clone();
        let primitive = input_to_primitive(&input, input_id.clone())
            .expect("single input metadata cannot conflict");

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.boundary, RunApplyBoundary::RunStart);
        assert_eq!(staged.contributing_input_ids, vec![input_id]);
        assert_eq!(staged.appends.len(), 1);
        assert_eq!(staged.appends[0].role, ConversationAppendRole::User);
        match &staged.appends[0].content {
            CoreRenderable::Text { text } => assert_eq!(text, "test prompt"),
            other => return Err(format!("expected text content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn input_to_primitive_preserves_typed_prompt_appends_without_user_text() -> Result<(), String> {
        let typed_append = ConversationAppend {
            role: ConversationAppendRole::SystemNotice,
            content: CoreRenderable::SystemNotice {
                kind: meerkat_core::types::SystemNoticeKind::Comms,
                body: Some("Peer message".to_string()),
                blocks: vec![meerkat_core::types::SystemNoticeBlock::Comms {
                    sender_taint: None,
                    kind: meerkat_core::types::CommsNoticeKind::Message,
                    direction: meerkat_core::types::SystemNoticeDirection::Incoming,
                    peer: None,
                    request_id: None,
                    intent: None,
                    status: None,
                    summary: Some("Peer message".to_string()),
                    payload: None,
                    content: Vec::new(),
                }],
            },
        };
        let mut input = make_prompt("");
        let Input::Prompt(prompt) = &mut input else {
            return Err("expected prompt".to_string());
        };
        prompt.typed_turn_appends = vec![typed_append.clone()];
        let input_id = input.id().clone();

        let primitive = input_to_primitive(&input, input_id.clone())
            .expect("single input metadata cannot conflict");
        let RunPrimitive::StagedInput(staged) = primitive else {
            return Err("expected staged input".to_string());
        };
        assert_eq!(staged.contributing_input_ids, vec![input_id]);
        assert_eq!(staged.appends, vec![typed_append]);
        Ok(())
    }

    #[test]
    fn staged_conversation_input_starts_conversation_turn_state() -> Result<(), String> {
        let input = make_prompt("test prompt");
        let input_id = input.id().clone();
        let primitive =
            input_to_primitive(&input, input_id).expect("single input metadata cannot conflict");
        let run_id = RunId::new();

        let start_input = primitive_turn_start_input(&run_id, &primitive)
            .ok_or_else(|| "expected staged content input to start turn state".to_string())?;

        match start_input {
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                run_id: got_run_id,
                primitive_kind,
                admitted_content_shape,
                ..
            } => {
                assert_eq!(
                    got_run_id,
                    crate::meerkat_machine::dsl::RunId::from_domain(&run_id)
                );
                assert_eq!(
                    primitive_kind,
                    crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn
                );
                assert_eq!(
                    admitted_content_shape,
                    crate::meerkat_machine::dsl::ContentShape::Conversation
                );
                Ok(())
            }
            other => Err(format!("expected StartConversationRun, got {other:?}")),
        }
    }

    #[test]
    fn peer_response_terminal_forced_immediate_boundary_is_invalid_apply_intent()
    -> Result<(), String> {
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("Analyst".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            })),
            handling_mode: None,
        });
        let input_id = input.id().clone();

        let primitive = inputs_to_primitive_with_boundary(
            &[(input_id.clone(), input)],
            RunApplyBoundary::Immediate,
        )
        .expect("single input metadata cannot conflict");
        assert!(
            primitive
                .peer_response_terminal_apply_intent_violation()
                .is_some(),
            "terminal peer response with an immediate boundary must fail the typed apply invariant"
        );
        assert!(!primitive.is_context_only_apply_without_turn());
        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.boundary, RunApplyBoundary::Immediate);
        assert_eq!(staged.contributing_input_ids, vec![input_id]);
        assert_eq!(staged.appends.len(), 1);
        assert_eq!(staged.context_appends.len(), 1);
        assert_eq!(
            staged.context_appends[0].key,
            format!(
                "peer_response_terminal:{TEST_PEER_RESPONSE_ROUTE_ID}:018f6f79-7a82-7c4e-a552-a3b86f9630f1"
            )
        );
        match &staged.context_appends[0].content {
            CoreRenderable::SystemNotice { blocks, .. } => {
                assert!(matches!(
                    blocks.first(),
                    Some(meerkat_core::types::SystemNoticeBlock::Comms { payload, .. })
                        if payload.as_ref().is_some_and(|payload| payload.to_string().contains("birch seventeen"))
                ));
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn peer_response_terminal_input_boundary_matches_machine_boundary() -> Result<(), String> {
        // Row 12 unification: `input_boundary` (typed Input view) and
        // `machine_input_boundary` (ingress handling_mode view) must agree
        // on ResponseTerminal inputs. Canonical answer is RunStart, since
        // ResponseTerminal is forbidden from carrying a handling_mode
        // override and the runtime-loop batch path already stages it as
        // RunStart.
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("Analyst".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: "done".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            })),
            handling_mode: None,
        });

        let primitive = inputs_to_primitive(&[(input.id().clone(), input)])
            .expect("single input metadata cannot conflict");
        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.boundary, RunApplyBoundary::RunStart);
        // Terminal peer responses carry both the typed comms notice append
        // (the model-visible content of the mandatory requester reaction
        // turn) and the keyed runtime context append.
        assert_eq!(staged.appends.len(), 1);
        assert_eq!(staged.context_appends.len(), 1);
        Ok(())
    }

    #[test]
    fn queued_peer_response_terminal_reaction_turn_has_model_visible_content() -> Result<(), String>
    {
        // Regression lock for the live mob bug where a block-less terminal
        // peer response staged a context-only primitive: the mandatory
        // `AppendContextAndRun` reaction turn then ran with a fabricated
        // empty user prompt, which Anthropic rejects with HTTP 400
        // ("user messages must have non-empty content"). The terminal
        // LlmFailure tore down the member's live session (and its comms
        // runtime), which later surfaced as
        // "wire requires comms runtime for '<peer>'" during respawn.
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("analyst-rt".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: TEST_PEER_RESPONSE_REQUEST_ID.into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: String::new().into(),
            payload: None,
            handling_mode: None,
        });
        let primitive = inputs_to_primitive(&[(input.id().clone(), input)])
            .expect("single input metadata cannot conflict");
        assert!(primitive.is_peer_response_terminal_context_and_run());
        let staged = match &primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(
            staged.appends.len(),
            1,
            "block-less terminal response must carry its typed comms notice append"
        );
        let provider_prompt =
            meerkat_core::lifecycle::run_primitive::model_projection_content_input_from_conversation_appends(
                &staged.appends,
            );
        assert!(
            !provider_prompt.text_content().trim().is_empty(),
            "the mandatory requester reaction turn must have non-empty model-visible content"
        );
        Ok(())
    }

    #[test]
    fn queued_peer_response_terminal_starts_requester_reaction_turn() -> Result<(), String> {
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("analyst-rt".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: TEST_PEER_RESPONSE_REQUEST_ID.into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: String::new().into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            })),
            handling_mode: None,
        });
        let primitive = inputs_to_primitive(&[(input.id().clone(), input)])
            .expect("single input metadata cannot conflict");
        let run_id = RunId::new();

        let start_input = primitive_turn_start_input(&run_id, &primitive).ok_or_else(|| {
            "terminal response should start a requester reaction turn".to_string()
        })?;

        match start_input {
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                run_id: got_run_id,
                primitive_kind,
                admitted_content_shape,
                ..
            } => {
                assert_eq!(
                    got_run_id,
                    crate::meerkat_machine::dsl::RunId::from_domain(&run_id)
                );
                assert_eq!(
                    primitive_kind,
                    crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn
                );
                assert_eq!(
                    admitted_content_shape,
                    crate::meerkat_machine::dsl::ContentShape::ConversationAndContext
                );
                Ok(())
            }
            other => Err(format!("expected StartConversationRun, got {other:?}")),
        }
    }

    #[test]
    fn peer_response_terminal_apply_intent_is_policy_runtime_and_executor_consistent()
    -> Result<(), String> {
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("analyst-rt".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: TEST_PEER_RESPONSE_REQUEST_ID.into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: String::new().into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            })),
            handling_mode: None,
        });

        let policy = crate::policy_table::DefaultPolicyTable::resolve(&input, true);
        assert_eq!(policy.apply_mode, crate::ApplyMode::StageRunStart);
        assert_eq!(policy.wake_mode, crate::WakeMode::WakeIfIdle);
        assert_eq!(policy.queue_mode, crate::QueueMode::Fifo);

        let semantics =
            crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(&input, true)
                .expect("generated admission semantics");
        assert_eq!(semantics.boundary, RunApplyBoundary::RunStart);
        assert_eq!(
            semantics.execution_kind,
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn
        );
        assert_eq!(
            semantics.peer_response_terminal_apply_intent,
            Some(PeerResponseTerminalApplyIntent::AppendContextAndRun)
        );

        let primitive = try_inputs_to_primitive_with_boundary(
            &[(input.id().clone(), input)],
            semantics.boundary,
            &[semantics],
        )
        .expect("single input metadata cannot conflict");
        let metadata = primitive
            .turn_metadata()
            .ok_or_else(|| "terminal primitive should carry metadata".to_string())?;
        assert_eq!(
            metadata.peer_response_terminal_apply_intent,
            Some(PeerResponseTerminalApplyIntent::AppendContextAndRun)
        );
        assert!(primitive.is_peer_response_terminal_context_and_run());
        assert_eq!(
            primitive.peer_response_terminal_apply_intent_violation(),
            None
        );
        assert!(
            !primitive.is_context_only_apply_without_turn(),
            "executor context-only shortcut must not swallow terminal peer responses"
        );

        Ok(())
    }

    fn make_peer_message(peer_id: &str, body: &str) -> Input {
        Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: Some("analyst-rt".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::Message),
            content: body.into(),
            payload: None,
            handling_mode: None,
        })
    }

    #[test]
    fn turn_metadata_stamps_transcript_identity_from_input_correlation() {
        let interaction_uuid = uuid::Uuid::from_u128(7);
        let mut input = make_peer_message("peer-1", "hello");
        if let Input::Peer(peer) = &mut input {
            peer.header.correlation_id = Some(crate::CorrelationId::from_uuid(interaction_uuid));
        }
        let semantics =
            crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(&input, true)
                .unwrap();

        let metadata = for_input(&input, semantics);

        assert_eq!(
            metadata.transcript_identity.interaction_id,
            Some(InteractionId(interaction_uuid))
        );
        assert_eq!(metadata.transcript_identity.run_id, None);
    }

    #[test]
    fn merged_turn_metadata_clears_distinct_transcript_identities() {
        let mut first = make_peer_message("peer-1", "hello");
        let mut second = make_peer_message("peer-2", "again");
        if let Input::Peer(peer) = &mut first {
            peer.header.correlation_id =
                Some(crate::CorrelationId::from_uuid(uuid::Uuid::from_u128(1)));
        }
        if let Input::Peer(peer) = &mut second {
            peer.header.correlation_id =
                Some(crate::CorrelationId::from_uuid(uuid::Uuid::from_u128(2)));
        }
        let first_semantics =
            crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(&first, true)
                .unwrap();
        let second_semantics =
            crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(
                &second, true,
            )
            .unwrap();
        let inputs = vec![(InputId::new(), first), (InputId::new(), second)];
        let semantics = vec![first_semantics, second_semantics];

        let metadata = merge_batch_turn_metadata(&inputs, &semantics)
            .unwrap()
            .expect("metadata should still carry execution kind");

        assert!(metadata.transcript_identity.is_empty());
    }

    #[test]
    fn merged_turn_metadata_preserves_shared_objective_across_interaction_conflict() {
        let objective_id = meerkat_core::interaction::ObjectiveId(uuid::Uuid::from_u128(90));
        let mut inputs = (1_u128..=3)
            .map(|interaction| {
                let mut input = make_peer_message("peer-1", "batched");
                if let Input::Peer(peer) = &mut input {
                    peer.objective_id = Some(objective_id);
                    peer.header.correlation_id = Some(crate::CorrelationId::from_uuid(
                        uuid::Uuid::from_u128(interaction),
                    ));
                }
                input
            })
            .collect::<Vec<_>>();
        let semantics = inputs.iter().map(admission_semantics).collect::<Vec<_>>();
        let inputs = inputs
            .drain(..)
            .map(|input| (InputId::new(), input))
            .collect::<Vec<_>>();

        let metadata = merge_batch_turn_metadata(&inputs, &semantics)
            .unwrap()
            .expect("metadata should carry shared objective causality");

        assert_eq!(metadata.transcript_identity.interaction_id, None);
        assert_eq!(
            metadata.transcript_identity.objective_id,
            Some(objective_id)
        );
    }

    #[test]
    fn merged_turn_metadata_does_not_reseed_conflicted_objective() {
        let first = meerkat_core::interaction::ObjectiveId(uuid::Uuid::from_u128(91));
        let conflicting = meerkat_core::interaction::ObjectiveId(uuid::Uuid::from_u128(92));
        let mut inputs = [first, conflicting, first]
            .into_iter()
            .map(|objective_id| {
                let mut input = make_peer_message("peer-1", "batched");
                if let Input::Peer(peer) = &mut input {
                    peer.objective_id = Some(objective_id);
                }
                input
            })
            .collect::<Vec<_>>();
        let semantics = inputs.iter().map(admission_semantics).collect::<Vec<_>>();
        let inputs = inputs
            .drain(..)
            .map(|input| (InputId::new(), input))
            .collect::<Vec<_>>();

        let metadata = merge_batch_turn_metadata(&inputs, &semantics)
            .unwrap()
            .expect("execution kind should keep metadata non-empty");

        assert_eq!(metadata.transcript_identity.objective_id, None);
    }

    fn admission_semantics(input: &Input) -> crate::ingress_types::RuntimeInputSemantics {
        crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(input, true)
            .expect("generated admission semantics")
    }

    fn reply_context_from_overlay(
        overlay: &meerkat_core::service::TurnToolOverlay,
    ) -> meerkat_core::comms::PeerReplyDispatchContext {
        let value = overlay
            .dispatch_context
            .get(meerkat_core::comms::COMMS_PEER_REPLY_DISPATCH_CONTEXT_KEY)
            .expect("comms.peer_reply dispatch context entry");
        serde_json::from_value(value.clone()).expect("typed peer reply dispatch context")
    }

    /// Ask 26 keystone: a turn triggered by a peer message mints a typed
    /// reply-capability overlay on the batch turn metadata (dispatch-context
    /// only — no tool visibility restriction).
    #[test]
    fn peer_message_turn_mints_typed_reply_capability_overlay() {
        let peer_uuid = uuid::Uuid::from_u128(0x42);
        let interaction_uuid = uuid::Uuid::from_u128(7);
        let mut input = make_peer_message(&peer_uuid.to_string(), "hello");
        if let Input::Peer(peer) = &mut input {
            peer.header.correlation_id = Some(crate::CorrelationId::from_uuid(interaction_uuid));
        }
        let semantics = admission_semantics(&input);
        let inputs = vec![(InputId::new(), input)];

        let metadata = merge_batch_turn_metadata(&inputs, &[semantics])
            .expect("single input metadata cannot conflict")
            .expect("peer message batch carries metadata");

        let overlay = metadata
            .turn_tool_overlay
            .expect("peer message turn must mint a reply-capability overlay");
        assert_eq!(overlay.allowed_tools, None);
        assert_eq!(overlay.blocked_tools, None);
        let context = reply_context_from_overlay(&overlay);
        assert_eq!(context.deliveries.len(), 1);
        let capability = &context.deliveries[0];
        assert_eq!(
            capability.peer_id,
            meerkat_core::comms::PeerId::from_uuid(peer_uuid)
        );
        assert_eq!(capability.in_reply_to, InteractionId(interaction_uuid));
        assert_eq!(capability.display_name.as_deref(), Some("analyst-rt"));
        assert_eq!(
            capability.kind,
            meerkat_core::comms::PeerReplyDeliveryKind::Message
        );
    }

    /// A batch with two peer message deliveries mints both capabilities, in
    /// batch delivery order.
    #[test]
    fn peer_message_batch_mints_one_capability_per_delivery() {
        let first_peer = uuid::Uuid::from_u128(0xA1);
        let second_peer = uuid::Uuid::from_u128(0xA2);
        let mut first = make_peer_message(&first_peer.to_string(), "hello");
        let mut second = make_peer_message(&second_peer.to_string(), "again");
        if let Input::Peer(peer) = &mut first {
            peer.header.correlation_id =
                Some(crate::CorrelationId::from_uuid(uuid::Uuid::from_u128(1)));
        }
        if let Input::Peer(peer) = &mut second {
            peer.header.correlation_id =
                Some(crate::CorrelationId::from_uuid(uuid::Uuid::from_u128(2)));
        }
        let semantics = vec![admission_semantics(&first), admission_semantics(&second)];
        let inputs = vec![(InputId::new(), first), (InputId::new(), second)];

        let metadata = merge_batch_turn_metadata(&inputs, &semantics)
            .expect("peer message batch must merge")
            .expect("peer message batch carries metadata");

        let overlay = metadata
            .turn_tool_overlay
            .expect("two-delivery batch must mint a reply-capability overlay");
        let context = reply_context_from_overlay(&overlay);
        assert_eq!(
            context
                .deliveries
                .iter()
                .map(|capability| (capability.peer_id, capability.in_reply_to))
                .collect::<Vec<_>>(),
            vec![
                (
                    meerkat_core::comms::PeerId::from_uuid(first_peer),
                    InteractionId(uuid::Uuid::from_u128(1))
                ),
                (
                    meerkat_core::comms::PeerId::from_uuid(second_peer),
                    InteractionId(uuid::Uuid::from_u128(2))
                ),
            ],
            "deliveries must preserve batch delivery order"
        );
    }

    /// Non-message peer conventions and non-canonical peer ids mint no
    /// overlay at the batch level.
    #[test]
    fn non_reply_eligible_batches_mint_no_overlay() {
        let terminal =
            make_terminal_peer_response(TEST_PEER_RESPONSE_ROUTE_ID, TEST_PEER_RESPONSE_REQUEST_ID);
        let semantics = admission_semantics(&terminal);
        let metadata = merge_batch_turn_metadata(&[(InputId::new(), terminal)], &[semantics])
            .expect("terminal batch must merge")
            .expect("terminal batch carries metadata");
        assert!(
            metadata.turn_tool_overlay.is_none(),
            "terminal peer responses must not mint a reply capability"
        );

        let mut non_canonical = make_peer_message("peer-1", "hello");
        if let Input::Peer(peer) = &mut non_canonical {
            peer.header.correlation_id =
                Some(crate::CorrelationId::from_uuid(uuid::Uuid::from_u128(3)));
        }
        let semantics = admission_semantics(&non_canonical);
        let metadata = merge_batch_turn_metadata(&[(InputId::new(), non_canonical)], &[semantics])
            .expect("non-canonical peer batch must merge")
            .expect("non-canonical peer batch carries metadata");
        assert!(
            metadata.turn_tool_overlay.is_none(),
            "a non-canonical peer id must not mint a reply capability"
        );
    }

    /// The reply-capability overlay composes with a caller-supplied overlay
    /// carrying an unrelated dispatch-context key (e.g. WorkGraph attention)
    /// instead of replacing it.
    #[test]
    fn reply_capability_overlay_composes_with_existing_dispatch_context() {
        let workgraph_overlay = meerkat_core::service::TurnToolOverlay {
            allowed_tools: None,
            blocked_tools: Some(vec![meerkat_core::types::ToolName::from("blocked_by_flow")]),
            dispatch_context: std::collections::BTreeMap::from([(
                "workgraph.attention_projection".to_string(),
                serde_json::json!({"binding_id": "b"}),
            )]),
        };
        let prompt = Input::Prompt(crate::input::PromptInput::new(
            "carry the workgraph overlay",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    turn_tool_overlay: Some(workgraph_overlay),
                    ..Default::default()
                },
            ),
        ));
        let peer_uuid = uuid::Uuid::from_u128(0x51);
        let mut peer = make_peer_message(&peer_uuid.to_string(), "hello");
        if let Input::Peer(peer_input) = &mut peer {
            peer_input.header.correlation_id =
                Some(crate::CorrelationId::from_uuid(uuid::Uuid::from_u128(4)));
        }
        let semantics = vec![admission_semantics(&prompt), admission_semantics(&peer)];
        let inputs = vec![(InputId::new(), prompt), (InputId::new(), peer)];

        let metadata = merge_batch_turn_metadata(&inputs, &semantics)
            .expect("prompt + peer message batch must merge")
            .expect("batch carries metadata");

        let overlay = metadata.turn_tool_overlay.expect("composed overlay");
        assert_eq!(
            overlay.blocked_tools,
            Some(vec![meerkat_core::types::ToolName::from("blocked_by_flow")]),
            "caller overlay facts must survive the reply-capability mint"
        );
        assert_eq!(
            overlay.dispatch_context["workgraph.attention_projection"],
            serde_json::json!({"binding_id": "b"})
        );
        let context = reply_context_from_overlay(&overlay);
        assert_eq!(context.deliveries.len(), 1);
        assert_eq!(
            context.deliveries[0].peer_id,
            meerkat_core::comms::PeerId::from_uuid(peer_uuid)
        );
    }

    fn make_terminal_peer_response(peer_id: &str, request_id: &str) -> Input {
        Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: Some("analyst-rt".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: request_id.into(),
                status: ResponseTerminalStatus::Completed,
            }),
            content: String::new().into(),
            payload: Some(serde_json::json!({"ok": true})),
            handling_mode: None,
        })
    }

    /// Field class (0.7.23 crew gateway): a poison input reaching the
    /// machine's max-attempts abandonment mid-wake must not wedge the queue.
    /// Pre-fix, the whole-batch defer sweep hit `GuardRejected` on the
    /// just-abandoned member ("failed to defer failed input batch behind
    /// backlog"), the loop returned with the wake dropped, and the innocent
    /// backlog stranded until an unrelated external wake (~13 minutes in the
    /// field). Post-fix the same wake keeps draining: the poison terminalizes
    /// as machine policy and the next queued input still gets its attempt.
    #[tokio::test]
    async fn max_attempts_abandonment_does_not_wedge_the_backlog() {
        let driver = make_shared_ephemeral_driver("defer-wedge-class");
        let poison_id =
            accept_queued_input_id(&driver, make_peer_message("lead-rt", "poison steer")).await;
        let innocent_id = accept_queued_input_id(
            &driver,
            make_terminal_peer_response(TEST_PEER_RESPONSE_ROUTE_ID, TEST_PEER_RESPONSE_REQUEST_ID),
        )
        .await;

        // Two stage attempts already burned on earlier wakes: the next
        // failure is the abandonment attempt — the exact field shape.
        {
            let mut guard = driver.lock().await;
            match &mut *guard {
                crate::meerkat_machine::DriverEntry::Ephemeral(d) => {
                    d.increment_attempt_count_for_test(&poison_id).unwrap();
                    d.increment_attempt_count_for_test(&poison_id).unwrap();
                }
                _ => panic!("expected ephemeral driver"),
            }
        }

        let apply_calls = Arc::new(AtomicUsize::new(0));
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = crate::control_plane::test_support::ApplyFailingExecutor::new(
            Arc::clone(&apply_calls),
            Arc::clone(&stop_calls),
        );
        let (_effect_tx, mut effect_rx) = tokio::sync::mpsc::channel(1);
        let authority_binding = RuntimeLoopAuthorityBinding::detached_for_test();
        let teardown_slot = RuntimeLoopTeardownSlot::pending();
        let mut terminal_handoff = RuntimeLoopTerminalHandoff::default();

        let should_stop = process_queue(
            &driver,
            &mut executor,
            &mut effect_rx,
            None,
            &authority_binding,
            &teardown_slot,
            &mut terminal_handoff,
        )
        .await;
        assert!(
            !should_stop,
            "a machine-policy abandonment must not stop the runtime loop"
        );
        assert_eq!(stop_calls.load(Ordering::SeqCst), 0);

        let guard = driver.lock().await;
        match &*guard {
            crate::meerkat_machine::DriverEntry::Ephemeral(d) => {
                assert_eq!(
                    d.input_phase(&poison_id),
                    Some(crate::input_state::InputLifecycleState::Abandoned),
                    "poison input must terminalize at the generated retry cap"
                );
                assert_eq!(d.input_attempt_count(&poison_id), 3);
                assert_eq!(
                    d.input_attempt_count(&innocent_id),
                    1,
                    "the same wake must keep draining past the abandoned poison — a zero \
                     attempt count means the backlog wedged behind a dropped wake"
                );
                assert_eq!(
                    d.input_phase(&innocent_id),
                    Some(crate::input_state::InputLifecycleState::Queued),
                    "the innocent input keeps its remaining machine-policy retries"
                );
            }
            _ => panic!("expected ephemeral driver"),
        }
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            2,
            "one wake must attempt both the poison batch and the innocent batch"
        );
    }

    async fn accept_queued_input_id(
        driver: &crate::meerkat_machine::SharedDriver,
        input: Input,
    ) -> InputId {
        let mut guard = driver.lock().await;
        match guard
            .as_driver_mut()
            .accept_input(input)
            .await
            .expect("input should be accepted")
        {
            crate::accept::AcceptOutcome::Accepted { input_id, .. } => input_id,
            other => panic!("expected accepted queued input, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn terminal_peer_response_batches_do_not_mix_with_peer_messages() {
        let terminal_first = make_shared_ephemeral_driver("terminal-first");
        let terminal_id = accept_queued_input_id(
            &terminal_first,
            make_terminal_peer_response(TEST_PEER_RESPONSE_ROUTE_ID, TEST_PEER_RESPONSE_REQUEST_ID),
        )
        .await;
        let _message_id = accept_queued_input_id(
            &terminal_first,
            make_peer_message("analyst-rt", "follow-up"),
        )
        .await;

        {
            let guard = terminal_first.lock().await;
            let batch =
                crate::meerkat_machine::driver::machine_authorize_runtime_loop_batch(&guard)
                    .expect("generated terminal-first runtime-loop batch")
                    .input_ids()
                    .to_vec();
            assert_eq!(
                batch,
                vec![terminal_id],
                "terminal response batch must not absorb later normal peer messages"
            );
            assert_eq!(
                crate::meerkat_machine::machine_batch_runtime_semantics(&guard, &batch).and_then(
                    |semantics| {
                        semantics
                            .into_iter()
                            .find_map(|semantics| semantics.peer_response_terminal_apply_intent)
                    }
                ),
                Some(PeerResponseTerminalApplyIntent::AppendContextAndRun)
            );
        }

        let message_first = make_shared_ephemeral_driver("message-first");
        let message_id =
            accept_queued_input_id(&message_first, make_peer_message("analyst-rt", "first")).await;
        let _terminal_id = accept_queued_input_id(
            &message_first,
            make_terminal_peer_response(
                TEST_PEER_RESPONSE_ROUTE_ID,
                TEST_PEER_RESPONSE_REQUEST_ID_2,
            ),
        )
        .await;

        let guard = message_first.lock().await;
        let batch = crate::meerkat_machine::driver::machine_authorize_runtime_loop_batch(&guard)
            .expect("generated message-first runtime-loop batch")
            .input_ids()
            .to_vec();
        assert_eq!(
            batch,
            vec![message_id],
            "normal peer message batch must not absorb later terminal responses"
        );
        assert_eq!(
            crate::meerkat_machine::machine_batch_runtime_semantics(&guard, &batch).and_then(
                |semantics| {
                    semantics
                        .into_iter()
                        .find_map(|semantics| semantics.peer_response_terminal_apply_intent)
                }
            ),
            None
        );
    }

    #[test]
    fn peer_input_with_blocks_produces_blocks_renderable() -> Result<(), String> {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "see this image".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
        ];
        let peer_id = "018f6f79-7a82-7c4e-a552-a3b86f963001";
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            content: meerkat_core::types::ContentInput::Blocks(blocks.clone()),
            payload: None,
            handling_mode: None,
        });
        let input_id = input.id().clone();
        let primitive =
            input_to_primitive(&input, input_id).expect("single input metadata cannot conflict");

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.appends.len(), 1);
        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                let Some(meerkat_core::types::SystemNoticeBlock::Comms { content, peer, .. }) =
                    got.first()
                else {
                    return Err("expected comms block".into());
                };
                assert_eq!(
                    peer.as_ref().map(|peer| peer.id),
                    Some(meerkat_core::comms::PeerId::parse(peer_id).expect("valid peer id"))
                );
                assert_eq!(content, &blocks);
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn peer_multimodal_append_keeps_source_without_duplicating_block_text() -> Result<(), String> {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "caption text".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
        ];
        let peer_id = "018f6f79-7a82-7c4e-a552-a3b86f963002";
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            content: meerkat_core::types::ContentInput::Blocks(blocks.clone()),
            payload: None,
            handling_mode: None,
        });
        let staged = match input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict")
        {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                let Some(meerkat_core::types::SystemNoticeBlock::Comms { content, peer, .. }) =
                    got.first()
                else {
                    return Err("expected comms block".into());
                };
                assert_eq!(
                    peer.as_ref().map(|peer| peer.id),
                    Some(meerkat_core::comms::PeerId::parse(peer_id).expect("valid peer id"))
                );
                assert_eq!(content, &blocks);
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn peer_image_only_blocks_keep_source_identity_without_text_duplication() -> Result<(), String>
    {
        let blocks = vec![meerkat_core::types::ContentBlock::Image {
            media_type: "image/png".into(),
            data: "abc123".into(),
        }];
        let peer_id = "018f6f79-7a82-7c4e-a552-a3b86f963003";
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            content: meerkat_core::types::ContentInput::Blocks(blocks.clone()),
            payload: None,
            handling_mode: None,
        });
        let staged = match input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict")
        {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                let Some(meerkat_core::types::SystemNoticeBlock::Comms { content, peer, .. }) =
                    got.first()
                else {
                    return Err("expected comms block".into());
                };
                assert_eq!(
                    peer.as_ref().map(|peer| peer.id),
                    Some(meerkat_core::comms::PeerId::parse(peer_id).expect("valid peer id"))
                );
                assert_eq!(content, &blocks);
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn peer_image_only_blocks_are_the_notice_content() -> Result<(), String> {
        let blocks = vec![meerkat_core::types::ContentBlock::Image {
            media_type: "image/png".into(),
            data: "abc123".into(),
        }];
        let peer_id = "018f6f79-7a82-7c4e-a552-a3b86f963004";
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            content: meerkat_core::types::ContentInput::Blocks(blocks.clone()),
            payload: None,
            handling_mode: None,
        });
        let staged = match input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict")
        {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                let Some(meerkat_core::types::SystemNoticeBlock::Comms { content, peer, .. }) =
                    got.first()
                else {
                    return Err("expected comms block".into());
                };
                assert_eq!(
                    peer.as_ref().map(|peer| peer.id),
                    Some(meerkat_core::comms::PeerId::parse(peer_id).expect("valid peer id"))
                );
                // Single-owner semantics: the typed blocks ARE the content;
                // no synthesized caption text is merged in beside them.
                assert_eq!(content, &blocks);
            }
            other => return Err(format!("expected blocks content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn flow_step_with_blocks_produces_blocks_renderable() -> Result<(), String> {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "analyze this screenshot".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
        ];
        let input = Input::FlowStep(FlowStepInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Flow {
                    flow_id: "flow-1".into(),
                    step_index: 0,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            step_id: "step-1".into(),
            content: meerkat_core::types::ContentInput::Blocks(blocks),
            directed_interaction_id: None,
            turn_metadata: None,
        });
        let input_id = input.id().clone();
        let primitive =
            input_to_primitive(&input, input_id).expect("single input metadata cannot conflict");

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.appends.len(), 1);
        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                assert!(matches!(
                    got.first(),
                    Some(meerkat_core::types::SystemNoticeBlock::RuntimeNotice { category, detail, .. })
                        if category == "flow_step"
                            && detail.as_deref()
                                == Some("analyze this screenshot\n[image: image/png]")
                ));
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn external_event_with_blocks_produces_blocks_renderable() -> Result<(), String> {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "see this event".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
        ];
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
            objective_id: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "webhook".into(),
            payload: serde_json::json!({"body": "see this event"}),
            blocks: Some(blocks.clone()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });
        let input_id = input.id().clone();
        let primitive =
            input_to_primitive(&input, input_id).expect("single input metadata cannot conflict");

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.appends.len(), 1);
        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                let Some(meerkat_core::types::SystemNoticeBlock::ExternalEvent { content, .. }) =
                    got.first()
                else {
                    return Err("expected external event block".into());
                };
                assert_eq!(content, &blocks);
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn external_event_with_image_only_blocks_keeps_event_identity() -> Result<(), String> {
        let blocks = vec![meerkat_core::types::ContentBlock::Image {
            media_type: "image/png".into(),
            data: "abc123".into(),
        }];
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
            objective_id: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "webhook".into(),
            payload: serde_json::json!({"body": "see attached screenshot"}),
            blocks: Some(blocks.clone()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });
        let primitive = input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict");

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                let Some(meerkat_core::types::SystemNoticeBlock::ExternalEvent { content, .. }) =
                    got.first()
                else {
                    return Err("expected external event block".into());
                };
                assert_eq!(content, &blocks);
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn external_event_prefers_source_name_over_event_type_without_body() -> Result<(), String> {
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
            objective_id: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "invoice.created".into(),
            payload: serde_json::json!({"invoice_id": "inv_123"}),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });

        assert_eq!(input_to_prompt(&input), "External event via webhook");
        Ok(())
    }

    #[test]
    fn plain_event_and_direct_runtime_external_event_share_projection() -> Result<(), String> {
        use crate::comms_bridge::classified_interaction_to_runtime_input;
        use crate::identifiers::LogicalRuntimeId;
        use meerkat_core::interaction::{
            InboxInteraction, InteractionContent, PeerInputCandidate, PeerInputClass,
        };

        let interaction_id = meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4());
        let from_comms = classified_interaction_to_runtime_input(
            &PeerInputCandidate {
                lifecycle_peer: None,
                response_terminality: None,
                ingress: meerkat_core::PeerIngressFact::plain_event(
                    interaction_id,
                    "webhook",
                    PeerInputClass::PlainEvent,
                    meerkat_core::PeerIngressKind::PlainEvent,
                ),
                interaction: InboxInteraction {
                    objective_id: None,
                    sender_taint: None,
                    id: interaction_id,
                    from_route: None,
                    from: "event:webhook".into(),
                    content: InteractionContent::Message {
                        body: "build failed".into(),
                        blocks: None,
                    },
                    rendered_text: "External event via webhook: build failed".into(),
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    render_metadata: None,
                },
            },
            &LogicalRuntimeId::new("test"),
        )
        .map_err(|err| err.to_string())?;

        let direct = Input::ExternalEvent(crate::input::ExternalEventInput {
            objective_id: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "webhook".into(),
            payload: serde_json::json!({"body": "build failed"}),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });

        assert_eq!(input_to_prompt(&from_comms), input_to_prompt(&direct));
        assert_eq!(
            input_to_prompt(&direct),
            "External event via webhook: build failed"
        );
        Ok(())
    }

    #[test]
    fn external_event_with_steer_preserves_runtime_hints() -> Result<(), String> {
        let render_metadata = meerkat_core::types::RenderMetadata {
            class: meerkat_core::types::RenderClass::ExternalEvent,
            salience: meerkat_core::types::RenderSalience::Urgent,
        };
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
            objective_id: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "webhook".into(),
            payload: serde_json::json!({"body": "urgent"}),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Steer,
            render_metadata: Some(render_metadata.clone()),
        });

        let staged = match input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict")
        {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        assert_eq!(staged.boundary, RunApplyBoundary::RunCheckpoint);
        assert_eq!(
            staged
                .turn_metadata
                .as_ref()
                .and_then(|meta| meta.handling_mode),
            Some(meerkat_core::types::HandlingMode::Steer)
        );
        assert_eq!(
            staged
                .turn_metadata
                .as_ref()
                .and_then(|meta| meta.render_metadata.clone()),
            Some(render_metadata)
        );
        Ok(())
    }

    #[tokio::test]
    async fn resolve_completion_waiters_surfaces_callback_pending() {
        let mut registry = crate::completion::CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());
        let terminal = Some(CoreApplyTerminal::CallbackPending {
            tool_name: "external_mock".to_string(),
            args: serde_json::json!({ "value": "browser" }),
        });

        registry.resolve_runtime_completion_authorized(
            std::iter::once(input_id.clone()),
            terminal.as_ref(),
            crate::meerkat_machine::driver::test_runtime_completion_authority(
                crate::meerkat_machine::dsl::RuntimeCompletionResultClass::CallbackPending,
                crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::CallbackPending,
            ),
            None,
        );

        match handle.wait_authorized().await {
            crate::completion::CompletionOutcome::CallbackPending { tool_name, args } => {
                assert_eq!(tool_name, "external_mock");
                assert_eq!(args, serde_json::json!({ "value": "browser" }));
            }
            other => panic!("Expected CallbackPending, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_completion_waiters_surfaces_terminal_run_result() {
        let mut registry = crate::completion::CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());
        let run_result = meerkat_core::types::RunResult {
            text: "terminal authority".to_string(),
            session_id: SessionId::new(),
            usage: meerkat_core::types::Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        };
        let terminal = Some(CoreApplyTerminal::RunResult(Box::new(run_result)));

        registry.resolve_runtime_completion_authorized(
            std::iter::once(input_id.clone()),
            terminal.as_ref(),
            crate::meerkat_machine::driver::test_runtime_completion_authority(
                crate::meerkat_machine::dsl::RuntimeCompletionResultClass::Completed,
                crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::Completed,
            ),
            None,
        );

        match handle.wait_authorized().await {
            crate::completion::CompletionOutcome::Completed(result) => {
                assert_eq!(result.text, "terminal authority");
            }
            other => panic!("Expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fail_completion_waiters_surfaces_wait_error() {
        let mut registry = crate::completion::CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        fail_completion_waiters(
            &mut registry,
            std::slice::from_ref(&input_id),
            "runtime loop failed before executor apply",
        );

        match handle.try_wait().await {
            Err(crate::completion::CompletionWaitError::AuthorityUnavailable(reason)) => {
                assert_eq!(reason, "runtime loop failed before executor apply");
            }
            other => panic!("Expected completion wait error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_feed_path_injects_inline_continuation_when_quiescent() {
        let driver = make_shared_ephemeral_driver("feed-inline");
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();

        let spec = background_spec("feed-inline");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(&op_id, op_result(&op_id, "done"))
            .unwrap();

        let feed = registry.completion_feed_handle();
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            Some(feed.as_ref()),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
            Some(&registry),
            &RuntimeLoopAuthorityBinding::detached_for_test(),
        )
        .await;

        assert_eq!(
            injected,
            FeedWakeOutcome::Injected,
            "feed-backed path should inject inline when quiescent"
        );
        assert_eq!(observed_seq, feed.watermark());
        assert_eq!(last_injected_seq, feed.watermark());

        let mut guard = driver.lock().await;
        assert_eq!(guard.as_driver().active_input_ids().len(), 1);
        let crate::meerkat_machine::DriverEntry::Ephemeral(ephemeral) = &mut *guard else {
            panic!("test uses an ephemeral driver");
        };
        let (_input_id, input) = ephemeral
            .dequeue_next()
            .expect("continuation should be queued inline");
        match input {
            Input::Continuation(continuation) => {
                assert_eq!(continuation.reason, "detached_background_op_completed");
                assert_eq!(
                    continuation.handling_mode,
                    meerkat_core::types::HandlingMode::Steer
                );
            }
            other => panic!("expected inline continuation injection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_without_feed_is_noop() {
        let driver = make_shared_ephemeral_driver("no-feed");
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            None,
            &mut observed_seq,
            &mut last_injected_seq,
            None,
            None,
            &RuntimeLoopAuthorityBinding::detached_for_test(),
        )
        .await;

        assert_eq!(
            injected,
            FeedWakeOutcome::Noop,
            "no feed means no injection"
        );
        assert_eq!(observed_seq, 0);
        assert_eq!(last_injected_seq, 0);

        let guard = driver.lock().await;
        assert!(
            guard.as_driver().active_input_ids().is_empty(),
            "no feed must not enqueue anything"
        );
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_with_feed_without_ops_authority_fails_closed() {
        let driver = make_shared_ephemeral_driver("feed-without-ops-authority");
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();

        let spec = background_spec("feed-without-ops-authority");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(&op_id, op_result(&op_id, "done"))
            .unwrap();

        let feed = registry.completion_feed_handle();
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            Some(feed.as_ref()),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
            None,
            &RuntimeLoopAuthorityBinding::detached_for_test(),
        )
        .await;

        assert_eq!(
            injected,
            FeedWakeOutcome::StaleAuthority,
            "feed-backed wake must fail closed without generated cursor authority"
        );
        assert_eq!(observed_seq, 0);
        assert_eq!(last_injected_seq, 0);

        let guard = driver.lock().await;
        assert!(
            guard.as_driver().active_input_ids().is_empty(),
            "missing cursor authority must not enqueue a detached continuation"
        );
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_empty_feed_advances_observed_seq() {
        let driver = make_shared_ephemeral_driver("advance-only");
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();
        let feed = registry.completion_feed_handle();
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            Some(feed.as_ref()),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
            Some(&registry),
            &RuntimeLoopAuthorityBinding::detached_for_test(),
        )
        .await;

        assert_eq!(
            injected,
            FeedWakeOutcome::Noop,
            "empty feed should not inject"
        );
        assert_eq!(observed_seq, feed.watermark());
        assert_eq!(last_injected_seq, 0);

        let guard = driver.lock().await;
        assert!(guard.as_driver().active_input_ids().is_empty());
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_generated_ignore_completion_advances_observed_seq() {
        let driver = make_shared_ephemeral_driver("advance-generated-ignore");
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();
        let spec = mob_child_spec("advance-generated-ignore");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(&op_id, op_result(&op_id, "done"))
            .unwrap();
        let feed = registry.completion_feed_handle();
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            Some(feed.as_ref()),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
            Some(&registry),
            &RuntimeLoopAuthorityBinding::detached_for_test(),
        )
        .await;

        assert_eq!(
            injected,
            FeedWakeOutcome::Noop,
            "generated observe-only completion should not inject"
        );
        assert_eq!(observed_seq, feed.watermark());
        assert_eq!(last_injected_seq, 0);

        let guard = driver.lock().await;
        assert!(guard.as_driver().active_input_ids().is_empty());
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_stale_observe_only_completion_does_not_advance_cursor() {
        let driver = make_shared_ephemeral_driver("advance-only-stale");
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();
        let spec = mob_child_spec("advance-only-stale");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(&op_id, op_result(&op_id, "done"))
            .unwrap();
        let feed = registry.completion_feed_handle();
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;
        let stale_binding = RuntimeLoopAuthorityBinding::new(
            std::sync::Weak::<crate::meerkat_machine::MeerkatMachine>::new(),
            SessionId::new(),
        );

        let injected = maybe_inject_feed_wake(
            &driver,
            Some(feed.as_ref()),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
            Some(&registry),
            &stale_binding,
        )
        .await;

        assert_eq!(injected, FeedWakeOutcome::StaleAuthority);
        assert_eq!(
            observed_seq, 0,
            "stale runtime loop must not advance observed cursor truth"
        );
        assert_eq!(last_injected_seq, 0);
    }

    /// Regression for #182: a detached wake whose continuation injection fails
    /// must leave the observed cursor UNCHANGED so the completion is
    /// re-presented on the next wake. A successful injection advances both
    /// cursors. The old code advanced the observed cursor unconditionally,
    /// laundering a failed injection into a watermark advance that hid the
    /// completion from retry.
    #[test]
    fn detached_wake_cursor_plan_leaves_completion_visible_on_injection_failure() {
        // Failed injection: leave both cursors untouched (completion stays
        // visible for retry).
        let failed = DetachedWakeCursorPlan::from_injection(false);
        assert_eq!(failed, DetachedWakeCursorPlan::LeaveVisibleForRetry);
        assert!(
            !failed.advances_observed_cursor(),
            "a failed injection must NOT advance the observed cursor"
        );

        // Successful injection: advance both injected and observed cursors.
        let succeeded = DetachedWakeCursorPlan::from_injection(true);
        assert_eq!(
            succeeded,
            DetachedWakeCursorPlan::AdvanceInjectedAndObserved
        );
        assert!(
            succeeded.advances_observed_cursor(),
            "a successful injection must advance the observed cursor"
        );
    }

    // --- execution_kind stamping tests ---

    #[test]
    fn primitive_from_prompt_has_content_turn() {
        let input = make_prompt("hello");
        let id = input.id().clone();
        let primitive =
            input_to_primitive(&input, id).expect("single input metadata cannot conflict");
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
        );
    }

    /// Delivery-order invariant: injected-context appends land in the staged
    /// primitive immediately BEFORE the turn's user append, in order.
    #[test]
    fn primitive_places_injected_context_appends_before_user_append() {
        let mut input = make_prompt("the prompt");
        let Input::Prompt(prompt) = &mut input else {
            panic!("make_prompt must build a prompt input");
        };
        prompt.injected_context = vec![
            meerkat_core::types::ContentInput::Text("ambient alpha".to_string()),
            meerkat_core::types::ContentInput::Text("ambient beta".to_string()),
        ];

        let primitive = input_to_primitive(&input, input.id().clone())
            .expect("single prompt input metadata cannot conflict");
        let RunPrimitive::StagedInput(staged) = &primitive else {
            panic!("expected staged input");
        };
        use meerkat_core::lifecycle::run_primitive::ConversationAppendRole as Role;
        let shapes = staged
            .appends
            .iter()
            .map(|append| (append.role, append.content.render_text()))
            .collect::<Vec<_>>();
        assert_eq!(
            shapes,
            vec![
                (Role::InjectedContext, "ambient alpha".to_string()),
                (Role::InjectedContext, "ambient beta".to_string()),
                (Role::User, "the prompt".to_string()),
            ],
        );
        // The extracted boundary prompt must not fold the injected context in
        // — it is delivered ALONGSIDE the user message, never inside it.
        assert_eq!(
            primitive.extract_content_input().text_content(),
            "the prompt"
        );
        // The promotion path's re-lowering projection recovers the entries in
        // delivery order.
        assert_eq!(
            primitive
                .injected_context_content_inputs()
                .iter()
                .map(meerkat_core::types::ContentInput::text_content)
                .collect::<Vec<_>>(),
            vec!["ambient alpha".to_string(), "ambient beta".to_string()],
        );
    }

    #[test]
    fn primitive_from_idle_peer_steer_normalizes_execution_handling_mode() {
        let mut input = make_peer_message("peer-steer", "urgent helper update");
        let Input::Peer(peer) = &mut input else {
            panic!("make_peer_message must build a peer input");
        };
        peer.handling_mode = Some(meerkat_core::types::HandlingMode::Steer);

        let primitive = input_to_primitive(&input, input.id().clone())
            .expect("single peer input metadata cannot conflict");
        let meta = primitive
            .turn_metadata()
            .expect("peer primitive should carry turn metadata");
        assert_eq!(
            meta.handling_mode,
            Some(meerkat_core::types::HandlingMode::Queue),
            "idle steer is already admitted into the steer lane; fresh turn execution must be queue-compatible"
        );
    }

    #[test]
    fn primitive_from_continuation_has_resume_pending() {
        let input = Input::Continuation(ContinuationInput::detached_background_op_completed());
        let id = input.id().clone();
        let primitive =
            input_to_primitive(&input, id).expect("single input metadata cannot conflict");
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending)
        );
    }

    #[test]
    fn admitted_input_primitive_uses_runtime_stamped_execution_kind() {
        let input = make_prompt("test prompt");
        let id = input.id().clone();
        let primitive = admitted_input_to_primitive(
            &input,
            id,
            crate::input::runtime_input_projection(&input),
            crate::ingress_types::RuntimeInputSemantics {
                boundary: RunApplyBoundary::RunStart,
                execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                execution_handling_mode: None,
                peer_response_terminal_apply_intent: None,
                live_interrupt_required: false,
            },
        )
        .expect("single input metadata cannot conflict");
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending),
            "primitive construction must use the runtime-stamped execution kind, not the local input kind"
        );
    }

    #[test]
    fn primitive_from_peer_terminal_has_content_turn() {
        let input = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            content: "done".into(),
            payload: None,
            handling_mode: None,
        });
        let id = input.id().clone();
        let primitive =
            input_to_primitive(&input, id).expect("single input metadata cannot conflict");
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
        );
    }

    #[test]
    fn mixed_batch_execution_kind_conflict_is_rejected() {
        // Admission batches are already grouped by execution kind. A direct
        // helper batch that mixes kinds must fail instead of inventing a local
        // ContentTurn default.
        let peer = Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::Message),
            content: "msg".into(),
            payload: None,
            handling_mode: None,
        });
        let continuation =
            Input::Continuation(ContinuationInput::detached_background_op_completed());
        let inputs = vec![
            (peer.id().clone(), peer),
            (continuation.id().clone(), continuation),
        ];
        let err = inputs_to_primitive_with_boundary(&inputs, RunApplyBoundary::RunCheckpoint)
            .expect_err("mixed execution kinds should be rejected");

        assert_eq!(err.field, "execution_kind");
    }

    #[test]
    fn batch_metadata_conflict_surfaces_typed_error() {
        let mut first = make_prompt("first");
        if let Input::Prompt(prompt) = &mut first {
            prompt.turn_metadata = Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                        "model-a",
                    )),
                    ..Default::default()
                },
            );
        }
        let mut second = make_prompt("second");
        if let Input::Prompt(prompt) = &mut second {
            prompt.turn_metadata = Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                        "model-b",
                    )),
                    ..Default::default()
                },
            );
        }
        let inputs = vec![(first.id().clone(), first), (second.id().clone(), second)];
        let semantics = fallback_batch_semantics(&inputs);

        let err =
            try_inputs_to_primitive_with_boundary(&inputs, RunApplyBoundary::RunStart, &semantics)
                .expect_err("conflicting batch metadata should be rejected");

        assert_eq!(err.field, "model");
    }
}
