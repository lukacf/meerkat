//! RuntimeLoop — per-session tokio task that processes queued inputs.
//!
//! When `MeerkatMachine::accept_input()` queues an input and sets
//! the wake flag, it sends a signal on the wake channel. The RuntimeLoop
//! picks it up, dequeues the input, converts it to a `RunPrimitive`,
//! and applies it via the `CoreExecutor` (which calls `SessionService::start_turn()`
//! under the hood).

use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
use meerkat_core::lifecycle::run_primitive::{
    ConversationAppend, ConversationAppendRole, ConversationContextAppend, CoreRenderable,
    RunApplyBoundary, RunPrimitive, StagedRunInput,
};
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::{
    PeerConversationProjection, PeerResponseProgressProjectionPhase,
    PeerResponseTerminalProjectionStatus,
};

use crate::input::Input;
use crate::tokio;

/// Extract a prompt string from an `Input`.
pub(crate) fn input_to_prompt(input: &Input) -> String {
    match input {
        Input::Prompt(p) => p.text.clone(),
        Input::Peer(p) => peer_prompt_text(p),
        Input::FlowStep(f) => f.instructions.clone(),
        Input::ExternalEvent(e) => external_event_projection_text(e),
        Input::Continuation(c) => format!("[Continuation] {}", c.reason),
        Input::Operation(operation) => {
            format!(
                "[Operation {}] {:?}",
                operation.operation_id, operation.event
            )
        }
    }
}

fn input_boundary(input: &Input) -> RunApplyBoundary {
    match input {
        Input::Peer(peer)
            if matches!(
                peer.convention,
                Some(crate::input::PeerConvention::ResponseProgress { .. })
            ) =>
        {
            RunApplyBoundary::RunCheckpoint
        }
        Input::Continuation(continuation) => match continuation.handling_mode {
            meerkat_core::types::HandlingMode::Queue => RunApplyBoundary::RunStart,
            meerkat_core::types::HandlingMode::Steer => RunApplyBoundary::RunCheckpoint,
        },
        Input::Prompt(prompt) => {
            match prompt.turn_metadata.as_ref().and_then(|m| m.handling_mode) {
                Some(meerkat_core::types::HandlingMode::Steer) => RunApplyBoundary::RunCheckpoint,
                _ => RunApplyBoundary::RunStart,
            }
        }
        Input::FlowStep(flow_step) => {
            match flow_step
                .turn_metadata
                .as_ref()
                .and_then(|m| m.handling_mode)
            {
                Some(meerkat_core::types::HandlingMode::Steer) => RunApplyBoundary::RunCheckpoint,
                _ => RunApplyBoundary::RunStart,
            }
        }
        Input::ExternalEvent(event) => match event.handling_mode {
            meerkat_core::types::HandlingMode::Steer => RunApplyBoundary::RunCheckpoint,
            meerkat_core::types::HandlingMode::Queue => RunApplyBoundary::RunStart,
        },
        _ => RunApplyBoundary::RunStart,
    }
}

/// Canonical runtime-side constructor for [`RuntimeTurnMetadata`].
///
/// This is the ONLY construction site for `RuntimeTurnMetadata` inside
/// `meerkat-runtime/src/`. Any other literal/default construction is an
/// RMAT-governed seam leak; the `turn_metadata_single_construction_site`
/// integration test greps for that invariant at build time.
pub(crate) fn for_input(
    input: &Input,
) -> Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata> {
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
    match input {
        Input::Prompt(prompt) => prompt.turn_metadata.clone(),
        Input::FlowStep(flow_step) => flow_step.turn_metadata.clone(),
        Input::ExternalEvent(event) => {
            let mut meta = RuntimeTurnMetadata::default();
            meta.handling_mode = Some(event.handling_mode);
            meta.render_metadata = event.render_metadata.clone();
            Some(meta)
        }
        Input::Continuation(continuation) => {
            let mut meta = RuntimeTurnMetadata::default();
            meta.handling_mode = Some(continuation.handling_mode);
            Some(meta)
        }
        _ => None,
    }
}

/// Merge the per-input turn metadata carried by a staged batch into a single
/// typed carrier. Scalar conflicts (two inputs disagreeing on e.g. `model`)
/// are refused with a typed error and the batch is treated as if no metadata
/// could be synthesized safely — the caller falls back to `None` and the
/// conflict is surfaced through structured tracing.
pub(crate) fn merge_batch_turn_metadata(
    inputs: &[(meerkat_core::lifecycle::InputId, Input)],
) -> Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata> {
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
    let mut acc: Option<RuntimeTurnMetadata> = None;
    for (_, input) in inputs {
        let Some(meta) = for_input(input) else {
            continue;
        };
        match acc.as_mut() {
            None => acc = Some(meta),
            Some(existing) => {
                if let Err(conflict) = existing.merge(meta) {
                    tracing::error!(
                        field = conflict.field,
                        reason = conflict.reason,
                        "batch turn-metadata merge conflict; dropping batch metadata"
                    );
                    return None;
                }
            }
        }
    }
    acc.filter(|m| !m.is_empty())
}

fn resolve_completion_waiters(
    registry: &mut crate::completion::CompletionRegistry,
    input_ids: &[InputId],
    run_result: Option<meerkat_core::types::RunResult>,
    terminal: Option<CoreApplyTerminal>,
) {
    if let Some(CoreApplyTerminal::CallbackPending { tool_name, args }) = terminal {
        for input_id in input_ids {
            registry.resolve_callback_pending(input_id, tool_name.clone(), args.clone());
        }
        return;
    }

    if let Some(result) = run_result {
        for input_id in input_ids {
            registry.resolve_completed(input_id, result.clone());
        }
        return;
    }

    match terminal {
        Some(CoreApplyTerminal::RunResult(_)) | None => {
            for input_id in input_ids {
                registry.resolve_without_result(input_id);
            }
        }
        Some(CoreApplyTerminal::CallbackPending { .. }) => unreachable!(),
    }
}

fn external_event_projection_text(event: &crate::input::ExternalEventInput) -> String {
    let source_name = match &event.header.source {
        crate::input::InputOrigin::External { source_name } if !source_name.trim().is_empty() => {
            source_name.as_str()
        }
        _ => event.event_type.as_str(),
    };
    let body = event
        .payload
        .get("body")
        .and_then(serde_json::Value::as_str)
        .map(str::trim);

    meerkat_core::interaction::format_external_event_projection(source_name, body)
}

/// Build a core-owned [`PeerConversationProjection`] from a runtime
/// [`crate::input::PeerInput`]. Returns `None` when the input has no
/// peer-originated source (e.g. a mob-internal peer input without a
/// `peer_id` header) or carries no convention — both shapes are
/// legitimate "no projection available" rather than errors, so the
/// caller falls through to the legacy body-as-text path.
///
/// The projection carries the full peer metadata (peer_id, request_id,
/// intent, payload, phase/status) and owns the semantic rendering
/// primitives (`prompt_text`, `block_prefix_text`, `context_key`) so
/// the runtime-loop shell never needs to format peer messages itself.
fn peer_projection_from_peer_input(
    peer: &crate::input::PeerInput,
) -> Option<PeerConversationProjection> {
    let crate::input::InputOrigin::Peer { peer_id, .. } = &peer.header.source else {
        return None;
    };

    match &peer.convention {
        Some(crate::input::PeerConvention::Message) => Some(PeerConversationProjection::Message {
            peer_id: peer_id.clone(),
        }),
        Some(crate::input::PeerConvention::Request { request_id, intent }) => {
            Some(PeerConversationProjection::Request {
                peer_id: peer_id.clone(),
                request_id: request_id.clone(),
                intent: intent.clone(),
                payload: peer.payload.clone(),
            })
        }
        Some(crate::input::PeerConvention::ResponseProgress { request_id, phase }) => {
            Some(PeerConversationProjection::ResponseProgress {
                peer_id: peer_id.clone(),
                request_id: request_id.clone(),
                phase: match phase {
                    crate::input::ResponseProgressPhase::Accepted => {
                        PeerResponseProgressProjectionPhase::Accepted
                    }
                    crate::input::ResponseProgressPhase::InProgress => {
                        PeerResponseProgressProjectionPhase::InProgress
                    }
                    crate::input::ResponseProgressPhase::PartialResult => {
                        PeerResponseProgressProjectionPhase::PartialResult
                    }
                },
                payload: peer.payload.clone(),
            })
        }
        Some(crate::input::PeerConvention::ResponseTerminal { request_id, status }) => {
            Some(PeerConversationProjection::ResponseTerminal {
                peer_id: peer_id.clone(),
                request_id: request_id.clone(),
                status: match status {
                    crate::input::ResponseTerminalStatus::Completed => {
                        PeerResponseTerminalProjectionStatus::Completed
                    }
                    crate::input::ResponseTerminalStatus::Failed => {
                        PeerResponseTerminalProjectionStatus::Failed
                    }
                    crate::input::ResponseTerminalStatus::Cancelled => {
                        PeerResponseTerminalProjectionStatus::Cancelled
                    }
                },
                payload: peer.payload.clone(),
            })
        }
        None => None,
    }
}

/// Borrowing shim — lift an [`Input`] to its core peer projection when
/// the input is a `PeerInput` with a peer-origin header. Used by the
/// context-append path where the caller already has an `&Input`.
fn peer_projection(input: &Input) -> Option<PeerConversationProjection> {
    let Input::Peer(peer) = input else {
        return None;
    };
    peer_projection_from_peer_input(peer)
}

/// Rendered prompt-text projection for a peer input.
///
/// For peer-originated inputs this returns the core-owned
/// `PeerConversationProjection::prompt_text()` (the typed semantic
/// rendering). For non-peer-originated sources (e.g. mob-internal
/// peer inputs with no `peer_id` header, or convention=None), the
/// raw `body` is the shell's best available fallback — the core
/// projection would return an empty string and the runtime would
/// otherwise ferry an empty prompt.
fn peer_prompt_text(peer: &crate::input::PeerInput) -> String {
    peer_projection_from_peer_input(peer)
        .map(|projection| {
            let prompt = projection.prompt_text();
            if prompt.is_empty() {
                peer.body.clone()
            } else {
                prompt
            }
        })
        .unwrap_or_else(|| peer.body.clone())
}

/// Optional block prefix (`"[COMMS MESSAGE from <peer_id>]"`) for peer
/// inputs that carry a `PeerConvention::Message`. Returns `None` for
/// request / response conventions (those route through the
/// context-append path instead) and for non-peer-originated inputs.
fn peer_block_prefix_text(peer: &crate::input::PeerInput) -> Option<String> {
    peer_projection_from_peer_input(peer).and_then(|projection| projection.block_prefix_text())
}

/// Convert an `Input` into a `ConversationAppend` on the `User` role,
/// or `None` if the input contributes no user-visible append. The
/// main exclusion is `ResponseTerminal` peer inputs — those land on
/// the typed context-append path (`input_to_context_append`) so the
/// LLM sees them as system context, not as a fresh user turn.
fn input_to_append(input: &Input) -> Option<ConversationAppend> {
    if matches!(
        input,
        Input::Peer(crate::input::PeerInput {
            convention: Some(crate::input::PeerConvention::ResponseTerminal { .. }),
            ..
        })
    ) {
        return None;
    }

    let content = match input {
        Input::Prompt(p) if p.blocks.is_some() => CoreRenderable::Blocks {
            blocks: p.blocks.clone().unwrap_or_default(),
        },
        Input::Peer(p) if p.blocks.is_some() => {
            let raw_blocks = p.blocks.clone().unwrap_or_default();
            if let Some(prefix) = peer_block_prefix_text(p) {
                let mut blocks = vec![meerkat_core::types::ContentBlock::Text { text: prefix }];
                blocks.extend(raw_blocks);
                CoreRenderable::Blocks { blocks }
            } else {
                // Non-peer-identified peer inputs: keep the body-as-prefix
                // fallback that predated the typed peer projection; the
                // projection seam is limited to conventioned peer-origin
                // inputs. Empty or already-embedded body is a no-op.
                let body_already_in_blocks = raw_blocks.first().is_some_and(|b| {
                    matches!(b, meerkat_core::types::ContentBlock::Text { text } if text == &p.body)
                });
                if p.body.is_empty() || body_already_in_blocks {
                    CoreRenderable::Blocks { blocks: raw_blocks }
                } else {
                    let mut blocks = vec![meerkat_core::types::ContentBlock::Text {
                        text: p.body.clone(),
                    }];
                    blocks.extend(raw_blocks);
                    CoreRenderable::Blocks { blocks }
                }
            }
        }
        Input::FlowStep(f) if f.blocks.is_some() => CoreRenderable::Blocks {
            blocks: f.blocks.clone().unwrap_or_default(),
        },
        Input::ExternalEvent(e) if e.blocks.is_some() => CoreRenderable::Blocks {
            blocks: e.blocks.clone().unwrap_or_default(),
        },
        Input::Prompt(_) | Input::Peer(_) | Input::FlowStep(_) | Input::ExternalEvent(_) => {
            CoreRenderable::Text {
                text: input_to_prompt(input),
            }
        }
        Input::Continuation(_) | Input::Operation(_) => return None,
    };

    Some(ConversationAppend {
        role: ConversationAppendRole::User,
        content,
    })
}

/// Convert an `Input` into a `ConversationContextAppend` when the
/// input is a peer `ResponseTerminal` (the only current context-append
/// shape). The context key and rendered text are both owned by the
/// core projection; the runtime-loop shell only maps from the typed
/// projection into the core-renderable payload.
fn input_to_context_append(input: &Input) -> Option<ConversationContextAppend> {
    let projection = peer_projection(input)?;

    Some(ConversationContextAppend {
        key: projection.context_key()?,
        content: CoreRenderable::Text {
            text: projection.prompt_text(),
        },
    })
}

pub(crate) fn inputs_to_primitive_with_boundary(
    inputs: &[(InputId, Input)],
    boundary: RunApplyBoundary,
) -> RunPrimitive {
    let appends = inputs
        .iter()
        .filter_map(|(_, input)| input_to_append(input))
        .collect::<Vec<_>>();
    let context_appends = inputs
        .iter()
        .filter_map(|(_, input)| input_to_context_append(input))
        .collect::<Vec<_>>();
    let contributing_input_ids = inputs
        .iter()
        .map(|(input_id, _)| input_id.clone())
        .collect::<Vec<_>>();
    // Merge turn metadata from ALL inputs in the batch, not just the first.
    // Later inputs override scalar fields; collection fields accumulate.
    let turn_metadata = merge_batch_turn_metadata(inputs);

    // Derive typed execution intent from the full batch.
    // If ANY input is ContentTurn, the whole batch is ContentTurn (safe default).
    // Only a fully-homogeneous ResumePending batch becomes ResumePending.
    let execution_kind = if inputs.is_empty() {
        None
    } else if inputs.iter().all(|(_, input)| {
        crate::input::classify_execution_kind(input)
            == meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending
    }) {
        Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending)
    } else {
        Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
    };
    let turn_metadata = {
        let mut meta = turn_metadata.unwrap_or_default();
        meta.execution_kind = execution_kind;
        Some(meta)
    };

    RunPrimitive::StagedInput(StagedRunInput {
        boundary,
        appends,
        context_appends,
        contributing_input_ids,
        turn_metadata,
    })
}

pub(crate) fn inputs_to_primitive(inputs: &[(InputId, Input)]) -> RunPrimitive {
    let boundary = inputs
        .first()
        .map(|(_, input)| input_boundary(input))
        .unwrap_or(RunApplyBoundary::RunStart);
    inputs_to_primitive_with_boundary(inputs, boundary)
}

/// Convert an `Input` + its ID to a `RunPrimitive` for `CoreExecutor::apply()`.
pub(crate) fn input_to_primitive(input: &Input, input_id: InputId) -> RunPrimitive {
    inputs_to_primitive(&[(input_id, input.clone())])
}

/// Spawn the per-session runtime loop with optional completion registry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_runtime_loop_with_completions(
    driver: crate::meerkat_machine::SharedDriver,
    mut executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    mut wake_rx: tokio::sync::mpsc::Receiver<()>,
    mut control_rx: tokio::sync::mpsc::Receiver<
        meerkat_core::lifecycle::run_control::RunControlCommand,
    >,
    completions: Option<crate::meerkat_machine::SharedCompletionRegistry>,
    detached_wake: Option<std::sync::Arc<crate::detached_wake::DetachedWakeState>>,
    completion_feed: Option<std::sync::Arc<dyn meerkat_core::completion_feed::CompletionFeed>>,
    epoch_cursor_state: Option<std::sync::Arc<meerkat_core::EpochCursorState>>,
    machine_weak: std::sync::Weak<crate::meerkat_machine::MeerkatMachine>,
    session_id: meerkat_core::types::SessionId,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Feed-based idle wake state (local to this loop).
        // Seed from epoch cursor state when available (runtime-backed surfaces).
        // Even an all-zero cursor must win over the feed watermark so a fresh
        // runtime loop cannot silently skip background completions that land
        // before the task reaches its first select iteration. Only callers that
        // do not provide cursor state fall back to the feed watermark to avoid
        // replaying historical completions.
        let initial_watermark = epoch_cursor_state
            .as_ref()
            .map(|cs| {
                let obs = cs
                    .runtime_observed_seq
                    .load(std::sync::atomic::Ordering::Acquire);
                let inj = cs
                    .runtime_last_injected_seq
                    .load(std::sync::atomic::Ordering::Acquire);
                obs.max(inj)
            })
            .unwrap_or_else(|| completion_feed.as_ref().map(|f| f.watermark()).unwrap_or(0));
        let mut observed_seq: meerkat_core::completion_feed::CompletionSeq = initial_watermark;
        let mut last_injected_seq: meerkat_core::completion_feed::CompletionSeq =
            epoch_cursor_state
                .as_ref()
                .map(|cs| {
                    cs.runtime_last_injected_seq
                        .load(std::sync::atomic::Ordering::Acquire)
                })
                .filter(|&v| v > 0)
                .unwrap_or(initial_watermark);

        loop {
            // Build a future for the idle wake. Feed-based if available,
            // otherwise falls back to DetachedWakeState Notify.
            let idle_wake = async {
                if let Some(ref feed) = completion_feed {
                    feed.wait_for_advance(observed_seq).await;
                } else if let Some(ref state) = detached_wake {
                    state.notify.notified().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };

            tokio::select! {
                biased;
                maybe_command = control_rx.recv() => {
                    match maybe_command {
                        Some(command) => {
                            if crate::control_plane::apply_executor_control(
                                &driver,
                                completions.as_ref(),
                                &mut *executor,
                                command,
                                &machine_weak,
                                &session_id,
                            )
                            .await
                            {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                maybe_wake = wake_rx.recv() => {
                    match maybe_wake {
                        Some(()) => {
                            if process_queue(
                                &driver,
                                &mut *executor,
                                &mut control_rx,
                                completions.as_ref(),
                            &machine_weak, &session_id,)
                            .await
                            {
                                break;
                            }
                            // Secondary wake path: re-check after queue drain.
                            // If a completion arrived during process_queue, inject
                            // and immediately process the continuation.
                            if maybe_inject_feed_wake(
                                &driver,
                                completion_feed.as_deref(),
                                detached_wake.as_ref(),
                                &mut observed_seq,
                                &mut last_injected_seq,
                                epoch_cursor_state.as_deref(),
                            )
                            .await
                                && process_queue(
                                    &driver,
                                    &mut *executor,
                                    &mut control_rx,
                                    completions.as_ref(),
                                &machine_weak, &session_id,)
                                .await
                            {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                () = idle_wake => {
                    // A completion arrived while idle. Check if it's a
                    // BackgroundToolOp that hasn't been injected yet.
                    // Only BackgroundToolOp triggers idle wake — MobMemberChild
                    // completions already wake through comms terminal response.
                    if let Some(ref feed) = completion_feed {
                        let batch = feed.list_since(observed_seq);

                        let has_new_bg_completion = batch.entries.iter().any(|e| {
                            e.kind
                                == meerkat_core::ops_lifecycle::OperationKind::BackgroundToolOp
                                && e.seq > last_injected_seq
                        });

                        if has_new_bg_completion {
                            // Verify quiescence before injecting.
                            let d = driver.lock().await;
                            let quiescent = d.is_quiescent_for_detached_wake();
                            drop(d);

                            if quiescent {
                                let input = crate::input::Input::Continuation(
                                    crate::input::ContinuationInput::detached_background_op_completed(),
                                );
                                let mut d = driver.lock().await;
                                if d.as_driver_mut().accept_input(input).await.is_ok() {
                                    last_injected_seq = batch.watermark;
                                    if let Some(ref cs) = epoch_cursor_state {
                                        cs.runtime_last_injected_seq.store(batch.watermark, std::sync::atomic::Ordering::Release);
                                    }
                                }
                                // Advance cursor only after successful injection
                                // or when quiescent (no pending BG work to retry).
                                observed_seq = batch.watermark;
                                if let Some(ref cs) = epoch_cursor_state {
                                    cs.runtime_observed_seq.store(batch.watermark, std::sync::atomic::Ordering::Release);
                                }
                                drop(d);
                                if process_queue(
                                    &driver,
                                    &mut *executor,
                                    &mut control_rx,
                                    completions.as_ref(),
                                &machine_weak, &session_id,)
                                .await
                                {
                                    break;
                                }
                            }
                            // Non-quiescent: do NOT advance observed_seq.
                            // The completion stays visible for the next wake
                            // so it's not permanently lost.
                        } else {
                            // No new BG completions — advance to prevent hot-spin
                            // on non-BG entries (MobMemberChild, etc.).
                            observed_seq = batch.watermark;
                            if let Some(ref cs) = epoch_cursor_state {
                                cs.runtime_observed_seq.store(batch.watermark, std::sync::atomic::Ordering::Release);
                            }
                        }
                    } else if let Some(ref state) = detached_wake
                        && state.pending.load(std::sync::atomic::Ordering::Acquire)
                    {
                        // Legacy detached wake path (no feed).
                        let input = crate::input::Input::Continuation(
                            crate::input::ContinuationInput::detached_background_op_completed(),
                        );
                        let mut d = driver.lock().await;
                        if d.as_driver_mut().accept_input(input).await.is_ok() {
                            clear_legacy_detached_wake_signal(state);
                        }
                        drop(d);
                        if process_queue(
                            &driver,
                            &mut *executor,
                            &mut control_rx,
                            completions.as_ref(),
                        &machine_weak, &session_id,)
                        .await
                        {
                            break;
                        }
                    }
                }
            }
        }

        // Loop exiting — resolve any pending completion waiters as terminated.
        if let Some(ref completions) = completions {
            let mut reg = completions.lock().await;
            reg.resolve_all_terminated("runtime loop exited");
        }
    })
}

/// Check for new background op completions and inject a continuation if needed.
///
/// Called after queue processing completes (session has returned to idle).
/// Returns `true` if a continuation was injected (caller should process_queue).
/// Supports both feed-based (new) and DetachedWakeState (legacy) paths.
async fn maybe_inject_feed_wake(
    driver: &crate::meerkat_machine::SharedDriver,
    feed: Option<&dyn meerkat_core::completion_feed::CompletionFeed>,
    wake_state: Option<&std::sync::Arc<crate::detached_wake::DetachedWakeState>>,
    observed_seq: &mut meerkat_core::completion_feed::CompletionSeq,
    last_injected_seq: &mut meerkat_core::completion_feed::CompletionSeq,
    epoch_cursor_state: Option<&meerkat_core::EpochCursorState>,
) -> bool {
    if let Some(feed) = feed {
        let batch = feed.list_since(*observed_seq);

        let has_new_bg_completion = batch.entries.iter().any(|e| {
            e.kind == meerkat_core::ops_lifecycle::OperationKind::BackgroundToolOp
                && e.seq > *last_injected_seq
        });

        if !has_new_bg_completion {
            // No new BG completions — advance to prevent hot-spin
            // on non-BG entries (MobMemberChild, etc.).
            *observed_seq = batch.watermark;
            if let Some(cs) = epoch_cursor_state {
                cs.runtime_observed_seq
                    .store(batch.watermark, std::sync::atomic::Ordering::Release);
            }
            return false;
        }

        // Verify quiescence before injecting.
        let d = driver.lock().await;
        if !d.is_quiescent_for_detached_wake() {
            // Non-quiescent: do NOT advance observed_seq. The completion
            // stays visible for the next wake so it's not permanently lost.
            return false;
        }
        drop(d);

        let input = crate::input::Input::Continuation(
            crate::input::ContinuationInput::detached_background_op_completed(),
        );
        let mut d = driver.lock().await;
        if d.as_driver_mut().accept_input(input).await.is_ok() {
            *last_injected_seq = batch.watermark;
            if let Some(cs) = epoch_cursor_state {
                cs.runtime_last_injected_seq
                    .store(batch.watermark, std::sync::atomic::Ordering::Release);
            }
        }
        // Advance cursor after injection attempt (quiescent path).
        *observed_seq = batch.watermark;
        if let Some(cs) = epoch_cursor_state {
            cs.runtime_observed_seq
                .store(batch.watermark, std::sync::atomic::Ordering::Release);
        }
        true
    } else {
        // Legacy DetachedWakeState path
        let Some(state) = wake_state else {
            return false;
        };

        if !state.pending.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }
        if state.signaled.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }

        let d = driver.lock().await;
        if !d.is_quiescent_for_detached_wake() {
            return false;
        }
        drop(d);

        // Legacy path: fire notify to wake the idle arm on next iteration.
        state
            .signaled
            .store(true, std::sync::atomic::Ordering::Release);
        state.notify.notify_one();
        false // Legacy path doesn't inject inline — idle arm handles it
    }
}

fn clear_legacy_detached_wake_signal(state: &crate::detached_wake::DetachedWakeState) {
    state
        .pending
        .store(false, std::sync::atomic::Ordering::Release);
    state
        .signaled
        .store(false, std::sync::atomic::Ordering::Release);
}

/// Process all queued inputs until the queue is empty.
#[allow(clippy::too_many_arguments)]
async fn process_queue(
    driver: &crate::meerkat_machine::SharedDriver,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    control_rx: &mut tokio::sync::mpsc::Receiver<
        meerkat_core::lifecycle::run_control::RunControlCommand,
    >,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    machine_weak: &std::sync::Weak<crate::meerkat_machine::MeerkatMachine>,
    session_id: &meerkat_core::types::SessionId,
) -> bool {
    loop {
        if crate::control_plane::drain_ready_executor_controls(
            driver,
            completions,
            executor,
            control_rx,
            machine_weak,
            session_id,
        )
        .await
        {
            return true;
        }

        // Dequeue and prepare under the driver lock
        let dequeued = {
            let mut d = driver.lock().await;

            // Only process if the runtime can process queue (Idle or Retired)
            if !d.can_process_queue() {
                return false;
            }

            // Ask the ingress authority for the next batch of input IDs.
            // The authority implements steer-first priority and same-boundary
            // batching for steer inputs; queue inputs follow the prompt-aware
            // batching policy.
            let batch_ids = crate::meerkat_machine::machine_select_runtime_loop_batch(&d);

            if batch_ids.is_empty() {
                return false;
            }

            // Dequeue the batch members from the physical queues.
            let staged_inputs: Vec<_> = batch_ids
                .iter()
                .filter_map(|id| d.dequeue_by_id(id))
                .collect();

            if staged_inputs.is_empty() {
                return false;
            }

            let run_id = RunId::new();

            // The checked-in Meerkat machine owns the coarse "this dequeued
            // batch has now become a run" transition, including unwind on
            // staging failure.
            let staged_ids: Vec<_> = staged_inputs.iter().map(|(id, _)| id.clone()).collect();

            // The checked-in Meerkat machine now owns runtime-loop batch
            // boundary classification over the stored ingress metadata.
            let boundary = staged_inputs
                .first()
                .map(|(id, _)| crate::meerkat_machine::machine_input_boundary(&d, id))
                .unwrap_or(RunApplyBoundary::RunStart);
            let primitive = inputs_to_primitive_with_boundary(&staged_inputs, boundary);
            let contributing_input_ids = staged_inputs
                .iter()
                .map(|(staged_input_id, _)| staged_input_id.clone())
                .collect::<Vec<_>>();
            Some((contributing_input_ids, staged_ids, run_id, primitive))
        };

        match dequeued {
            Some((input_ids, staged_ids, run_id, primitive)) => {
                if crate::meerkat_machine::prepare_runtime_loop_batch_start(
                    driver,
                    run_id.clone(),
                    &staged_ids,
                )
                .await
                .is_err()
                {
                    return false;
                }

                // Execute outside the driver lock (this calls start_turn, which is slow)
                let result = executor.apply(run_id.clone(), primitive).await;

                // Lock again to update driver state
                let d = driver.lock().await;
                match result {
                    Ok(output) => {
                        let meerkat_core::lifecycle::core_executor::CoreApplyOutput {
                            receipt,
                            session_snapshot,
                            run_result,
                            terminal,
                        } = output;
                        drop(d);
                        if let Err(err) = crate::meerkat_machine::commit_runtime_loop_run(
                            driver,
                            run_id.clone(),
                            input_ids.clone(),
                            receipt,
                            session_snapshot,
                        )
                        .await
                        {
                            tracing::error!(%run_id, error = %err, "failed to commit runtime loop run");
                            let _ = executor
                                .control(meerkat_core::lifecycle::run_control::RunControlCommand::StopRuntimeExecutor {
                                    reason: format!("runtime loop commit failed for run {run_id}: {err}"),
                                })
                                .await;
                            return true;
                        }

                        // Resolve completion waiters unconditionally
                        if let Some(completions) = completions.as_ref() {
                            let mut reg = completions.lock().await;
                            resolve_completion_waiters(&mut reg, &input_ids, run_result, terminal);
                        }
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        drop(d);
                        // RunFailed rolls back Staged → Queued and returns to Idle
                        if let Err(err) = crate::meerkat_machine::fail_runtime_loop_run(
                            driver,
                            run_id,
                            error_msg.clone(),
                        )
                        .await
                        {
                            tracing::error!(error = %err, "failed to record run-failed event");
                            let _ = executor
                                .control(meerkat_core::lifecycle::run_control::RunControlCommand::StopRuntimeExecutor {
                                    reason: format!("runtime failure snapshot failed: {err}"),
                                })
                                .await;
                            // Resolve waiter before breaking so callers don't hang.
                            if let Some(completions) = completions.as_ref() {
                                let mut completions = completions.lock().await;
                                for input_id in &input_ids {
                                    completions.resolve_abandoned(
                                        input_id,
                                        format!("runtime failure snapshot failed: {err}"),
                                    );
                                }
                            }
                            return true;
                        }
                        // Resolve completion waiter so callers don't hang.
                        if let Some(completions) = completions.as_ref() {
                            let mut completions = completions.lock().await;
                            for input_id in &input_ids {
                                completions.resolve_abandoned(
                                    input_id,
                                    format!("apply failed: {error_msg}"),
                                );
                            }
                        }
                        let mut d = driver.lock().await;
                        let should_continue =
                            d.take_wake_requested() && d.has_queued_input_outside(&input_ids);
                        drop(d);
                        if should_continue {
                            continue;
                        }
                        // Leave the failing input queued for a future wake instead of
                        // hot-looping on the same payload indefinitely.
                        return false;
                    }
                }
            }
            None => return false, // Queue empty
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::input::*;
    use chrono::Utc;
    use std::sync::Arc;

    use meerkat_core::ops_lifecycle::{
        OperationKind, OperationResult, OperationSpec, OpsLifecycleRegistry,
    };
    use meerkat_core::types::SessionId;

    fn background_spec(name: &str) -> OperationSpec {
        OperationSpec {
            id: meerkat_core::ops_lifecycle::OperationId::new(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: SessionId::new(),
            display_name: name.into(),
            source_label: "runtime-loop-test".into(),
            child_session_id: None,
            expect_peer_channel: false,
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

    fn make_prompt(text: &str) -> Input {
        Input::Prompt(PromptInput {
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
            text: text.into(),
            blocks: None,
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
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: None,
            body: "peer message".into(),
            payload: None,
            blocks: None,
            handling_mode: None,
        });
        assert_eq!(input_to_prompt(&input), "peer message");
    }

    #[test]
    fn input_to_prompt_peer_message_uses_body_when_projection_text_is_empty() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            body: "[COMMS MESSAGE from peer-1]\nplain body payload".into(),
            payload: None,
            blocks: None,
            handling_mode: None,
        });

        assert_eq!(
            input_to_prompt(&input),
            "[COMMS MESSAGE from peer-1]\nplain body payload"
        );
    }

    #[test]
    fn input_to_prompt_peer_request_is_runtime_owned_and_ignores_bogus_body() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "operator-rt".into(),
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
            body: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({"subject": "alpha beta gamma"})),
            blocks: None,
            handling_mode: None,
        });

        assert_eq!(
            input_to_prompt(&input),
            "[SYSTEM NOTICE][PEER_REQUEST] Correlated peer request from operator-rt. Intent: checksum_token. Request ID: req-123. Params: {\n  \"subject\": \"alpha beta gamma\"\n}. This is not a normal user request and not a prompt for direct user-facing output. Handle it by calling send_response with to=\"operator-rt\", in_reply_to=\"req-123\", status=\"completed\" or \"failed\", and result=<JSON payload>. Do not use send_message for this reply."
        );
    }

    #[test]
    fn input_to_prompt_peer_response_terminal_is_runtime_owned_from_typed_payload() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "analyst-rt".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "req-123".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            body: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "token": "birch seventeen"
            })),
            blocks: None,
            handling_mode: None,
        });

        assert_eq!(
            input_to_prompt(&input),
            "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Correlated peer response from analyst-rt. Request ID: req-123. Status: completed. Result: {\n  \"request_intent\": \"checksum_token\",\n  \"token\": \"birch seventeen\"\n}."
        );
    }

    #[test]
    fn input_to_prompt_peer_response_terminal_omits_payload_key_extraction() {
        // Regression: runtime must not reach into the peer response payload
        // to extract ad-hoc fields (request_intent, token, etc.) and bake
        // them into prompt text. The canonical projection is the typed
        // convention + the Result JSON verbatim; any per-fixture semantics
        // belong to the peer application, not the runtime.
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "analyst-rt".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "req-123".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            body: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "token": "birch seventeen"
            })),
            blocks: None,
            handling_mode: None,
        });

        let rendered = input_to_prompt(&input);
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
        let primitive = input_to_primitive(&input, input_id.clone());

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
    fn peer_response_terminal_creates_immediate_context_staged_input() -> Result<(), String> {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "analyst-rt".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "req-123".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            body: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "token": "birch seventeen"
            })),
            blocks: None,
            handling_mode: None,
        });
        let input_id = input.id().clone();

        let primitive = inputs_to_primitive_with_boundary(
            &[(input_id.clone(), input)],
            RunApplyBoundary::Immediate,
        );
        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.boundary, RunApplyBoundary::Immediate);
        assert_eq!(staged.contributing_input_ids, vec![input_id]);
        assert!(staged.appends.is_empty());
        assert_eq!(staged.context_appends.len(), 1);
        assert_eq!(
            staged.context_appends[0].key,
            "peer_response_terminal:analyst-rt:req-123"
        );
        match &staged.context_appends[0].content {
            CoreRenderable::Text { text } => {
                assert!(text.contains("[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL]"));
                assert!(text.contains("birch seventeen"));
            }
            other => return Err(format!("expected text content, got {other:?}")),
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
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "analyst-rt".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "req-123".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            body: "done".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "token": "birch seventeen"
            })),
            blocks: None,
            handling_mode: None,
        });

        assert_eq!(input_boundary(&input), RunApplyBoundary::RunStart);

        let primitive = inputs_to_primitive(&[(input.id().clone(), input)]);
        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.boundary, RunApplyBoundary::RunStart);
        // Terminal peer payload still routes through the context-append
        // path (not user-visible appends) since `input_to_append` returns
        // None for the ResponseTerminal convention.
        assert!(staged.appends.is_empty());
        assert_eq!(staged.context_appends.len(), 1);
        Ok(())
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
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            body: "see this image".into(),
            payload: None,
            blocks: Some(blocks.clone()),
            handling_mode: None,
        });
        let input_id = input.id().clone();
        let primitive = input_to_primitive(&input, input_id);

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.appends.len(), 1);
        match &staged.appends[0].content {
            CoreRenderable::Blocks { blocks: got } => {
                assert_eq!(got.len(), 3);
                assert_eq!(
                    got[0],
                    meerkat_core::types::ContentBlock::Text {
                        text: "[COMMS MESSAGE from p]".into(),
                    }
                );
                assert_eq!(got[1], blocks[0]);
                assert_eq!(got[2], blocks[1]);
            }
            other => return Err(format!("expected blocks content, got {other:?}")),
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
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            body: "[COMMS MESSAGE from peer-1]\ncaption text\n[image: image/png]".into(),
            payload: None,
            blocks: Some(blocks.clone()),
            handling_mode: None,
        });
        let staged = match input_to_primitive(&input, input.id().clone()) {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        match &staged.appends[0].content {
            CoreRenderable::Blocks { blocks: got } => {
                assert_eq!(got.len(), 3);
                assert_eq!(
                    got[0],
                    meerkat_core::types::ContentBlock::Text {
                        text: "[COMMS MESSAGE from peer-1]".into(),
                    }
                );
                assert_eq!(got[1], blocks[0]);
                assert_eq!(got[2], blocks[1]);
            }
            other => return Err(format!("expected blocks content, got {other:?}")),
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
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            body: "[COMMS MESSAGE from peer-1]\n[image: image/png]".into(),
            payload: None,
            blocks: Some(blocks.clone()),
            handling_mode: None,
        });
        let staged = match input_to_primitive(&input, input.id().clone()) {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        match &staged.appends[0].content {
            CoreRenderable::Blocks { blocks: got } => {
                assert_eq!(got.len(), 2);
                assert_eq!(
                    got[0],
                    meerkat_core::types::ContentBlock::Text {
                        text: "[COMMS MESSAGE from peer-1]".into(),
                    }
                );
                assert_eq!(got[1], blocks[0]);
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
            instructions: "analyze this screenshot".into(),
            blocks: Some(blocks),
            turn_metadata: None,
        });
        let input_id = input.id().clone();
        let primitive = input_to_primitive(&input, input_id);

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.appends.len(), 1);
        match &staged.appends[0].content {
            CoreRenderable::Blocks { blocks: got } => assert_eq!(got.len(), 2),
            other => return Err(format!("expected blocks content, got {other:?}")),
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
        let primitive = input_to_primitive(&input, input_id);

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.appends.len(), 1);
        match &staged.appends[0].content {
            CoreRenderable::Blocks { blocks: got } => {
                // Blocks are preserved directly — no synthetic text header prepended.
                assert_eq!(got.len(), 2);
                assert_eq!(got[0], blocks[0]);
                assert_eq!(got[1], blocks[1]);
            }
            other => return Err(format!("expected blocks content, got {other:?}")),
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
        let primitive = input_to_primitive(&input, input.id().clone());

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        match &staged.appends[0].content {
            CoreRenderable::Blocks { blocks: got } => {
                // Image-only blocks preserved directly without text header.
                assert_eq!(got.len(), 1);
                assert_eq!(got[0], blocks[0]);
            }
            other => return Err(format!("expected blocks content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn external_event_prefers_source_name_over_event_type_without_body() -> Result<(), String> {
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
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

        assert_eq!(input_to_prompt(&input), "[EVENT via webhook]");
        Ok(())
    }

    #[test]
    fn plain_event_and_direct_runtime_external_event_share_projection() -> Result<(), String> {
        use crate::comms_bridge::peer_input_candidate_to_runtime_input;
        use crate::identifiers::LogicalRuntimeId;
        use meerkat_core::interaction::{
            InboxInteraction, InteractionContent, PeerInputCandidate, PeerInputClass,
        };

        let from_comms = peer_input_candidate_to_runtime_input(
            &PeerInputCandidate {
                class: PeerInputClass::PlainEvent,
                lifecycle_peer: None,
                interaction: InboxInteraction {
                    id: meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4()),
                    from: "event:webhook".into(),
                    content: InteractionContent::Message {
                        body: "build failed".into(),
                        blocks: None,
                    },
                    rendered_text: "[EVENT via webhook] build failed".into(),
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    render_metadata: None,
                },
            },
            &LogicalRuntimeId::new("test"),
        );

        let direct = Input::ExternalEvent(crate::input::ExternalEventInput {
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
        assert_eq!(input_to_prompt(&direct), "[EVENT via webhook] build failed");
        Ok(())
    }

    #[test]
    fn external_event_with_steer_preserves_runtime_hints() -> Result<(), String> {
        let render_metadata = meerkat_core::types::RenderMetadata {
            class: meerkat_core::types::RenderClass::ExternalEvent,
            salience: meerkat_core::types::RenderSalience::Urgent,
        };
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
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

        let staged = match input_to_primitive(&input, input.id().clone()) {
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

        resolve_completion_waiters(
            &mut registry,
            std::slice::from_ref(&input_id),
            None,
            Some(CoreApplyTerminal::CallbackPending {
                tool_name: "external_mock".to_string(),
                args: serde_json::json!({ "value": "browser" }),
            }),
        );

        match handle.wait().await {
            crate::completion::CompletionOutcome::CallbackPending { tool_name, args } => {
                assert_eq!(tool_name, "external_mock");
                assert_eq!(args, serde_json::json!({ "value": "browser" }));
            }
            other => panic!("Expected CallbackPending, got {other:?}"),
        }
    }

    #[test]
    fn clear_legacy_detached_wake_signal_resets_pending_and_signaled() {
        let state = crate::detached_wake::DetachedWakeState::new();
        state
            .pending
            .store(true, std::sync::atomic::Ordering::Release);
        state
            .signaled
            .store(true, std::sync::atomic::Ordering::Release);

        clear_legacy_detached_wake_signal(&state);

        assert!(!state.pending.load(std::sync::atomic::Ordering::Acquire));
        assert!(!state.signaled.load(std::sync::atomic::Ordering::Acquire));
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
            None,
            &mut observed_seq,
            &mut last_injected_seq,
            None,
        )
        .await;

        assert!(
            injected,
            "feed-backed path should inject inline when quiescent"
        );
        assert_eq!(observed_seq, feed.watermark());
        assert_eq!(last_injected_seq, feed.watermark());

        let mut guard = driver.lock().await;
        assert_eq!(guard.as_driver().active_input_ids().len(), 1);
        let (_input_id, input) = guard
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
    async fn maybe_inject_feed_wake_legacy_path_sets_signaled_without_inline_injection() {
        let driver = make_shared_ephemeral_driver("legacy-signal");
        let state = Arc::new(crate::detached_wake::DetachedWakeState::new());
        state
            .pending
            .store(true, std::sync::atomic::Ordering::Release);

        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            None,
            Some(&state),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
        )
        .await;

        assert!(
            !injected,
            "legacy detached wake should only signal the idle arm, not inject inline"
        );
        assert!(state.pending.load(std::sync::atomic::Ordering::Acquire));
        assert!(state.signaled.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(observed_seq, 0);
        assert_eq!(last_injected_seq, 0);

        let guard = driver.lock().await;
        assert!(
            guard.as_driver().active_input_ids().is_empty(),
            "legacy signaling path should not enqueue a continuation inline"
        );
    }

    // --- execution_kind stamping tests ---

    #[test]
    fn primitive_from_prompt_has_content_turn() {
        let input = make_prompt("hello");
        let id = input.id().clone();
        let primitive = input_to_primitive(&input, id);
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
        );
    }

    #[test]
    fn primitive_from_continuation_has_resume_pending() {
        let input = Input::Continuation(ContinuationInput::detached_background_op_completed());
        let id = input.id().clone();
        let primitive = input_to_primitive(&input, id);
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending)
        );
    }

    #[test]
    fn primitive_from_peer_terminal_has_content_turn() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "r".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            body: "done".into(),
            payload: None,
            blocks: None,
            handling_mode: None,
        });
        let id = input.id().clone();
        let primitive = input_to_primitive(&input, id);
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
        );
    }

    #[test]
    fn mixed_batch_defaults_to_content_turn() {
        // If a batch contains both ContentTurn and ResumePending inputs,
        // the whole batch must be ContentTurn (safe default).
        let peer = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::Message),
            body: "msg".into(),
            payload: None,
            blocks: None,
            handling_mode: None,
        });
        let continuation =
            Input::Continuation(ContinuationInput::detached_background_op_completed());
        let inputs = vec![
            (peer.id().clone(), peer),
            (continuation.id().clone(), continuation),
        ];
        let primitive = inputs_to_primitive_with_boundary(&inputs, RunApplyBoundary::RunCheckpoint);
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn),
            "mixed batch must default to ContentTurn"
        );
    }
}
