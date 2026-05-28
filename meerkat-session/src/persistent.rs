//! PersistentSessionService — wraps EphemeralSessionService with snapshot + event persistence.
//!
//! Gated behind the `session-store` feature.
//!
//! Runtime projection contract:
//!
//! - In runtime-backed mode, `RuntimeStore` snapshot commits own durable session
//!   lifecycle truth. The `SessionStore` row is a compatibility projection.
//! - `save_normalized_session` rebuilds that projection from the normalized
//!   authoritative snapshot immediately after the runtime snapshot commit.
//! - `read` and `list` must never treat raw `SessionStore` rows as canonical
//!   runtime truth. Listing may use projection rows only as discovery keys and
//!   then rebuild summaries from live/runtime authority.
//! - If the projection update fails after a runtime authority commit, the
//!   caller gets an error and the live handle is discarded. The stale row may
//!   remain in `SessionStore`, but consumers must keep honoring runtime
//!   authority or exclude/fail closed when no authority exists.
//! - If a runtime snapshot commit fails after a direct live mutation, the
//!   caller gets an error and the mutated live handle is discarded before it
//!   can drive reads or listing as clean truth.
//! - Read/list/live-export observation paths fail closed to durable runtime
//!   authority when a live transcript is ahead of the committed snapshot, but
//!   they do not drop the live handle that owns mechanical capabilities such as
//!   comms runtimes.
//!
//! Runtime-less compatibility mode still persists directly to `SessionStore`.
//! Event-log file projection is separate best-effort derived state.

#![cfg_attr(test, allow(dead_code))]

use async_trait::async_trait;
use futures::StreamExt;
use indexmap::IndexMap;
use meerkat_core::BlobStore;
use meerkat_core::PendingSystemContextAppend;
#[allow(unused_imports)] // Used in read() fallback path
use meerkat_core::Session;
use meerkat_core::SessionSystemContextState;
use meerkat_core::error::AgentError;
use meerkat_core::image_content::{externalize_deferred_turn_state, externalize_messages_from};
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreApplyTerminal};
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest, InitialTurnPolicy,
    MobToolAuthorityContext, SessionControlError, SessionError, SessionForkAtRequest,
    SessionForkReplaceRequest, SessionForkResult, SessionHistoryPage, SessionHistoryQuery,
    SessionInfo, SessionQuery, SessionService, SessionServiceCommsExt, SessionServiceControlExt,
    SessionServiceHistoryExt, SessionServiceTranscriptEditExt, SessionSummary,
    SessionTranscriptRestoreRevisionRequest, SessionTranscriptRevisionPage,
    SessionTranscriptRevisionQuery, SessionTranscriptRewriteRequest,
    SessionTranscriptRewriteResult, SessionUsage, SessionView, StageToolResultsRequest,
    StageToolResultsResult, StartTurnRequest,
};
use meerkat_core::types::{RunResult, SessionId, ToolResult};
use meerkat_core::{DeferredFirstTurnPhase, SessionDeferredTurnState};
use meerkat_core::{InputId, RunId};
use meerkat_runtime::identifiers::LogicalRuntimeId;
#[cfg(test)]
use meerkat_runtime::input_state::{
    InputLifecycleState, InputStatePersistenceRecord, InputTerminalOutcome, StoredInputState,
};
use meerkat_runtime::store::SessionDelta;
use meerkat_runtime::{
    MachineSessionControlAuthority, MeerkatMachine, RuntimeMode, RuntimeState, RuntimeStore,
};
use meerkat_store::{SessionFilter, SessionStore, SessionStoreError};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, watch};

use crate::SESSION_LABELS_KEY;
use crate::ephemeral::{EphemeralSessionService, SessionAgentBuilder};
use crate::event_store::EventStore;
use crate::projector::SessionProjector;

/// Re-export of the crate-root `migrations` module so the canonical
/// path `meerkat_session::persistent::migrations` (as named in the
/// wave-c plan) resolves unchanged.
pub use crate::migrations;

const SESSION_ARCHIVED_KEY: &str = "session_archived";

fn runtime_driver_error_to_session_error(err: meerkat_runtime::RuntimeDriverError) -> SessionError {
    SessionError::Agent(AgentError::InternalError(err.to_string()))
}

fn session_id_from_event(event: &meerkat_core::event::AgentEvent) -> Option<SessionId> {
    match event {
        meerkat_core::event::AgentEvent::RunStarted { session_id, .. }
        | meerkat_core::event::AgentEvent::RunCompleted { session_id, .. }
        | meerkat_core::event::AgentEvent::ExtractionSucceeded { session_id, .. }
        | meerkat_core::event::AgentEvent::ExtractionFailed { session_id, .. }
        | meerkat_core::event::AgentEvent::RunFailed { session_id, .. }
        | meerkat_core::event::AgentEvent::TranscriptRewriteCommitted { session_id, .. } => {
            Some(session_id.clone())
        }
        _ => None,
    }
}

async fn append_and_project_event(
    event_store: &Arc<dyn EventStore>,
    projector: &Arc<SessionProjector>,
    session_id: &SessionId,
    event: meerkat_core::event::AgentEvent,
) {
    let events = [event];
    match event_store.append(session_id, &events).await {
        Ok(seq) => {
            if let Err(error) = projector
                .project(event_store.as_ref(), session_id, seq)
                .await
            {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "failed to project persistent session events"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to append persistent session event"
            );
        }
    }
}

async fn flush_projected_events(
    event_store: &Arc<dyn EventStore>,
    projector: &Arc<SessionProjector>,
    session_id: &SessionId,
    pending: &mut Vec<meerkat_core::event::AgentEvent>,
) {
    for event in pending.drain(..) {
        append_and_project_event(event_store, projector, session_id, event).await;
    }
}

async fn project_create_time_events(
    event_store: Arc<dyn EventStore>,
    projector: Arc<SessionProjector>,
    mut projection_rx: mpsc::Receiver<
        meerkat_core::event::EventEnvelope<meerkat_core::event::AgentEvent>,
    >,
    mut session_rx: watch::Receiver<Option<SessionId>>,
    caller_event_tx: Option<
        mpsc::Sender<meerkat_core::event::EventEnvelope<meerkat_core::event::AgentEvent>>,
    >,
) {
    let mut session_id = session_rx.borrow().clone();
    let mut pending = Vec::new();

    loop {
        tokio::select! {
            envelope = projection_rx.recv() => {
                let Some(envelope) = envelope else {
                    break;
                };
                if session_id.is_none() {
                    session_id = session_id_from_event(&envelope.payload);
                }
                let event = envelope.payload.clone();
                if let Some(tx) = caller_event_tx.as_ref()
                    && tx.send(envelope).await.is_err()
                {
                    tracing::warn!("session event stream receiver dropped; continuing event projection");
                }
                if let Some(session_id) = session_id.as_ref() {
                    pending.push(event);
                    flush_projected_events(&event_store, &projector, session_id, &mut pending).await;
                } else {
                    pending.push(event);
                }
            }
            changed = session_rx.changed(), if session_id.is_none() => {
                if changed.is_err() {
                    break;
                }
                session_id = session_rx.borrow().clone();
                if let Some(session_id) = session_id.as_ref() {
                    flush_projected_events(&event_store, &projector, session_id, &mut pending).await;
                }
            }
        }
    }

    if let Some(session_id) = session_id.as_ref() {
        flush_projected_events(&event_store, &projector, session_id, &mut pending).await;
    }
}

fn write_system_context_state(
    session: &mut Session,
    state: SessionSystemContextState,
) -> Result<(), SessionControlError> {
    session.set_system_context_state(state).map_err(|err| {
        SessionControlError::Session(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize system-context state: {err}"
            )),
        ))
    })
}

fn write_deferred_turn_state(
    session: &mut Session,
    state: SessionDeferredTurnState,
) -> Result<(), SessionControlError> {
    session.set_deferred_turn_state(state).map_err(|err| {
        SessionControlError::Session(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize deferred-turn state: {err}"
            )),
        ))
    })
}

fn rollback_tool_visibility_state_snapshot(
    session: &Session,
) -> Result<Option<meerkat_core::SessionToolVisibilityState>, SessionError> {
    // This production rollback path must not promote legacy tool_scope_* metadata
    // into canonical runtime-backed visibility authority.
    session.try_tool_visibility_state().map_err(|err| {
        SessionError::Agent(AgentError::InternalError(format!(
            "invalid canonical tool visibility state: {err}"
        )))
    })
}

fn control_error_into_session_error(err: SessionControlError) -> SessionError {
    match err {
        SessionControlError::Session(session_err) => session_err,
        other => SessionError::Unsupported(other.to_string()),
    }
}

fn runtime_backed_store_projection_can_recover_authority(session: &Session) -> bool {
    session.session_metadata().is_some() || session.build_state().is_some()
}

fn is_durable_session_sync_unsupported(err: &SessionError) -> bool {
    matches!(
        err,
        SessionError::Agent(AgentError::ConfigError(message))
            if message.contains("durable session snapshot synchronization is not supported")
    )
}

fn validate_tool_result_video(results: &[ToolResult]) -> Result<(), SessionError> {
    if results.iter().any(ToolResult::has_video) {
        return Err(SessionError::Agent(AgentError::ConfigError(
            "video blocks are not supported in tool results".to_string(),
        )));
    }
    Ok(())
}

/// Shared gate between the checkpointer and archive.
///
/// The `Mutex` provides mutual exclusion so that `checkpoint()` cannot
/// race with `archive()`: both acquire the lock before touching the store,
/// and `archive()` sets `cancelled = true` under the lock before deleting.
struct CheckpointerGate {
    cancelled: Mutex<bool>,
}

#[derive(Clone, Copy)]
enum StoreOnlyArchiveMode {
    Reject,
    MachineAuthority,
}

#[derive(Clone, Copy)]
enum DirectStartTurnPersistence<'a> {
    RuntimelessImmediate,
    MachineCommitted(MachineServiceTurnCommitProtocol<'a>),
}

/// Checkpointer that saves sessions to a [`SessionStore`].
///
/// Used by keep-alive agents to persist the session after each interaction
/// without going through `SessionService::start_turn()`.
///
/// Tracks the message count from the last successful save so that
/// back-to-back checkpoints of an unchanged session are skipped.
/// This avoids redundant writes, especially the first checkpoint
/// after `create_session` which already persists an initial snapshot.
struct StoreCheckpointer {
    store: Arc<dyn SessionStore>,
    blob_store: Arc<dyn BlobStore>,
    event_store: Option<Arc<dyn EventStore>>,
    projector: Option<Arc<SessionProjector>>,
    gate: Arc<CheckpointerGate>,
    last_saved_revision: std::sync::Mutex<Option<String>>,
}

fn session_materialized_at_transcript_revision(
    session: &Session,
    revision: &str,
) -> Result<Session, SessionError> {
    let mut state = session
        .transcript_history_state()
        .map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to read transcript history for materialization: {err}"
            )))
        })?
        .ok_or_else(|| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "session has no transcript history state to materialize".to_string(),
            ))
        })?;
    let original_state = state.clone();
    state.head = revision.to_string();
    let mut retained_revisions = std::collections::BTreeSet::new();
    let mut cursor = Some(revision);
    while let Some(current) = cursor {
        retained_revisions.insert(current.to_string());
        cursor = original_state
            .revisions
            .iter()
            .find(|body| body.revision == current)
            .and_then(|body| body.parent_revision.as_deref());
    }
    let retain_commit_count = state
        .commits
        .iter()
        .rposition(|commit| commit.revision == revision)
        .or_else(|| {
            state.commits.iter().rposition(|commit| {
                transcript_state_revision_extends(&original_state, revision, &commit.revision)
            })
        })
        .map(|index| index + 1)
        .unwrap_or_default();
    state.commits.truncate(retain_commit_count);
    for commit in &state.commits {
        retained_revisions.insert(commit.parent_revision.clone());
        retained_revisions.insert(commit.revision.clone());
    }
    state
        .revisions
        .retain(|body| retained_revisions.contains(&body.revision));
    let mut materialized = session.clone();
    if state.commits.is_empty() {
        materialized
            .apply_transcript_history_state(state)
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to materialize transcript revision {revision}: {err}"
                )))
            })?;
        materialized.clear_transcript_history_state();
        return Ok(materialized);
    }
    materialized
        .apply_transcript_history_state(state)
        .map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to materialize transcript revision {revision}: {err}"
            )))
        })?;
    Ok(materialized)
}

async fn find_transcript_rewrite_commit_chain_extending_session_with_storage_normalization<'a>(
    blob_store: &dyn BlobStore,
    state: &'a meerkat_core::TranscriptHistoryState,
    previous: &Session,
    incoming_revision: &str,
    incoming_messages: &[meerkat_core::Message],
) -> Result<Option<Vec<&'a meerkat_core::TranscriptRewriteCommit>>, SessionStoreError> {
    if let Some(commits) =
        meerkat_core::session_store::find_transcript_rewrite_commit_chain_extending_session(
            state,
            previous,
            incoming_revision,
        )?
    {
        return Ok(Some(commits));
    }

    for commit in state.commits.iter().rev() {
        let Some(commits) =
            meerkat_core::session_store::find_transcript_rewrite_commit_chain_extending_session(
                state,
                previous,
                &commit.revision,
            )?
        else {
            continue;
        };
        if commits.is_empty() {
            continue;
        }
        let Some(revision_body) = state
            .revisions
            .iter()
            .find(|body| body.revision == commit.revision)
        else {
            continue;
        };
        let incoming_extends_commit =
            transcript_state_revision_extends(state, incoming_revision, &commit.revision)
                || transcript_revision_preserves_storage_normalized_append_prefix(
                    blob_store,
                    state,
                    incoming_revision,
                    incoming_messages,
                    &revision_body.messages,
                )
                .await?;
        if !incoming_extends_commit {
            continue;
        }
        if transcript_revision_preserves_storage_normalized_append_prefix(
            blob_store,
            state,
            incoming_revision,
            incoming_messages,
            &revision_body.messages,
        )
        .await?
        {
            return Ok(Some(commits));
        }
    }

    Ok(None)
}

async fn transcript_revision_preserves_storage_normalized_append_prefix(
    blob_store: &dyn BlobStore,
    state: &meerkat_core::TranscriptHistoryState,
    revision: &str,
    current_messages: &[meerkat_core::Message],
    ancestor_messages: &[meerkat_core::Message],
) -> Result<bool, SessionStoreError> {
    let mut revision_messages = if revision == state.head {
        current_messages.to_vec()
    } else if let Some(body) = state
        .revisions
        .iter()
        .find(|body| body.revision == revision)
    {
        body.messages.clone()
    } else if meerkat_core::transcript_messages_digest(current_messages)
        .map_err(SessionStoreError::from)?
        == revision
    {
        current_messages.to_vec()
    } else {
        return Ok(false);
    };
    let mut normalized_ancestor = ancestor_messages.to_vec();
    externalize_messages_from(blob_store, &mut normalized_ancestor, 0)
        .await
        .map_err(|err| {
            SessionStoreError::Internal(format!(
                "failed to normalize ancestor transcript revision body for rewrite chain discovery: {err}"
            ))
        })?;
    externalize_messages_from(blob_store, &mut revision_messages, 0)
        .await
        .map_err(|err| {
            SessionStoreError::Internal(format!(
                "failed to normalize incoming transcript revision body for rewrite chain discovery: {err}"
            ))
        })?;
    if revision_messages.len() < normalized_ancestor.len() {
        return Ok(false);
    }
    let ancestor_revision = meerkat_core::transcript_messages_digest(&normalized_ancestor)
        .map_err(SessionStoreError::from)?;
    let prefix_revision =
        meerkat_core::transcript_messages_digest(&revision_messages[..normalized_ancestor.len()])
            .map_err(SessionStoreError::from)?;
    Ok(prefix_revision == ancestor_revision)
}

fn transcript_state_revision_extends(
    state: &meerkat_core::TranscriptHistoryState,
    descendant: &str,
    ancestor: &str,
) -> bool {
    if descendant == ancestor {
        return true;
    }
    let mut cursor = descendant;
    while let Some(body) = state.revisions.iter().find(|body| body.revision == cursor) {
        let Some(parent) = body.parent_revision.as_deref() else {
            return false;
        };
        if parent == ancestor {
            return true;
        }
        cursor = parent;
    }
    false
}

fn transcript_rewrite_store_error_to_session_error(error: SessionStoreError) -> SessionError {
    match error {
        SessionStoreError::TranscriptRevisionConflict {
            expected, actual, ..
        } => meerkat_core::TranscriptEditError::RevisionConflict { expected, actual }
            .into_session_error(),
        other => SessionError::Store(Box::new(other)),
    }
}

fn transcript_rewrite_record_for_session(
    session: &Session,
    commit: &meerkat_core::TranscriptRewriteCommit,
) -> Result<meerkat_core::TranscriptRewriteRecord, SessionError> {
    let parent_body = session
        .transcript_revision_body(&commit.parent_revision)
        .map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to read parent transcript revision body for audit event: {err}"
            )))
        })?
        .ok_or_else(|| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "missing parent transcript revision body {} for audit event",
                commit.parent_revision
            )))
        })?;
    let revision_body = session
        .transcript_revision_body(&commit.revision)
        .map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to read transcript revision body for audit event: {err}"
            )))
        })?
        .ok_or_else(|| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "missing transcript revision body {} for audit event",
                commit.revision
            )))
        })?;
    meerkat_core::TranscriptRewriteRecord::new(commit.clone(), parent_body, revision_body).map_err(
        |err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "transcript rewrite audit record validation failed: {err}"
            )))
        },
    )
}

async fn append_transcript_rewrite_commit_events(
    event_store: Option<&Arc<dyn EventStore>>,
    projector: Option<&Arc<SessionProjector>>,
    session: &Session,
    commits: &[meerkat_core::TranscriptRewriteCommit],
) -> Result<(), SessionError> {
    let Some(event_store) = event_store else {
        return Ok(());
    };
    if commits.is_empty() {
        return Ok(());
    }
    let mut events = Vec::with_capacity(commits.len());
    for commit in commits {
        events.push(
            meerkat_core::event::AgentEvent::TranscriptRewriteCommitted {
                session_id: session.id().clone(),
                record: transcript_rewrite_record_for_session(session, commit)?,
            },
        );
    }
    let seq = match event_store.append(session.id(), &events).await {
        Ok(seq) => seq,
        Err(error) => {
            tracing::error!(
                session_id = %session.id(),
                revisions = ?commits.iter().map(|commit| commit.revision.as_str()).collect::<Vec<_>>(),
                error = %error,
                "failed to publish canonical transcript rewrite audit event after projection update"
            );
            return Err(SessionError::Store(Box::new(error)));
        }
    };
    if let Some(projector) = projector
        && let Err(error) = projector.resume(event_store.as_ref(), session.id()).await
    {
        tracing::warn!(
            session_id = %session.id(),
            seq,
            error = %error,
            "failed to project transcript rewrite commit event"
        );
    }
    Ok(())
}

async fn save_session_projection_allowing_internal_rewrite(
    store: &dyn SessionStore,
    blob_store: &dyn BlobStore,
    event_store: Option<&Arc<dyn EventStore>>,
    projector: Option<&Arc<SessionProjector>>,
    session: &Session,
) -> Result<(), SessionError> {
    let previous = store
        .load(session.id())
        .await
        .map_err(|err| SessionError::Store(Box::new(err)))?;
    let Some(previous) = previous else {
        return save_session_projection_with_storage_normalization_bridge(
            store, blob_store, session,
        )
        .await
        .map_err(|err| SessionError::Store(Box::new(err)));
    };
    let previous_revision =
        meerkat_core::transcript_messages_digest(previous.messages()).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to digest previous transcript for projection save: {err}"
            )))
        })?;
    let previous_projection_token =
        meerkat_core::session_store::session_projection_cas_token(&previous)
            .map_err(|err| SessionError::Store(Box::new(err)))?;
    let incoming_revision =
        meerkat_core::transcript_messages_digest(session.messages()).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to digest incoming transcript for projection save: {err}"
            )))
        })?;
    let Some(state) = session.transcript_history_state().map_err(|err| {
        SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
            "failed to read transcript history for projection save: {err}"
        )))
    })?
    else {
        return save_session_projection_with_storage_normalization_bridge(
            store, blob_store, session,
        )
        .await
        .map_err(|err| SessionError::Store(Box::new(err)));
    };
    let mut normalized_previous_for_chain = None;
    let mut commits =
        find_transcript_rewrite_commit_chain_extending_session_with_storage_normalization(
            blob_store,
            &state,
            &previous,
            &incoming_revision,
            session.messages(),
        )
        .await
        .map_err(|err| SessionError::Store(Box::new(err)))?;
    if commits.is_none() {
        let mut normalized_previous = previous.clone();
        normalized_previous
            .externalize_media(blob_store, 0)
            .await
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to externalize previous persisted projection for projection rewrite chain discovery: {err}"
                )))
            })?;
        let normalized_revision = meerkat_core::transcript_messages_digest(
            normalized_previous.messages(),
        )
        .map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to digest normalized previous transcript for projection save: {err}"
            )))
        })?;
        if normalized_revision != previous_revision {
            commits =
                find_transcript_rewrite_commit_chain_extending_session_with_storage_normalization(
                    blob_store,
                    &state,
                    &normalized_previous,
                    &incoming_revision,
                    session.messages(),
                )
                .await
                .map_err(|err| SessionError::Store(Box::new(err)))?;
            if commits.is_some() {
                normalized_previous_for_chain = Some(normalized_previous);
            }
        }
    }
    let Some(commits) = commits else {
        return save_session_projection_with_storage_normalization_bridge(
            store, blob_store, session,
        )
        .await
        .map_err(|err| SessionError::Store(Box::new(err)));
    };
    let proof_previous = normalized_previous_for_chain.as_ref().unwrap_or(&previous);
    if commits.is_empty() {
        if !state.commits.is_empty()
            && meerkat_core::session_store::run_boundary_snapshot_save_guard(
                session,
                Some(proof_previous),
            )
            .is_ok()
        {
            store
                .save_authoritative_projection_if_current_revision(
                    session,
                    Some(previous_projection_token.clone()),
                )
                .await
                .map_err(|err| SessionError::Store(Box::new(err)))?;
            return Ok(());
        }
        return save_session_projection_with_storage_normalization_bridge(
            store, blob_store, session,
        )
        .await
        .map_err(|err| SessionError::Store(Box::new(err)));
    }
    let mut last_audited_projection = Some(previous.clone());
    let mut persisted_revision = Some(previous_revision);
    let mut persisted_projection_token = Some(previous_projection_token);
    for commit in &commits {
        if persisted_revision.as_deref() != Some(commit.parent_revision.as_str()) {
            let bridge =
                session_materialized_at_transcript_revision(session, &commit.parent_revision)?;
            store
                .save_authoritative_projection_if_current_revision(
                    &bridge,
                    persisted_projection_token.clone(),
                )
                .await
                .map_err(|err| SessionError::Store(Box::new(err)))?;
        }
        let rewritten = session_materialized_at_transcript_revision(session, &commit.revision)?;
        store
            .save_transcript_rewrite(&rewritten, commit)
            .await
            .map_err(transcript_rewrite_store_error_to_session_error)?;
        let rewritten_projection_token =
            meerkat_core::session_store::session_projection_cas_token(&rewritten)
                .map_err(|err| SessionError::Store(Box::new(err)))?;
        let commit_record = (*commit).clone();
        if let Err(error) = append_transcript_rewrite_commit_events(
            event_store,
            projector,
            session,
            std::slice::from_ref(&commit_record),
        )
        .await
        {
            if let Some(rollback_target) = last_audited_projection.as_ref()
                && let Err(rollback_error) = store
                    .save_authoritative_projection_if_current_revision(
                        rollback_target,
                        Some(rewritten_projection_token.clone()),
                    )
                    .await
            {
                tracing::error!(
                    session_id = %session.id(),
                    error = %rollback_error,
                    "failed to roll back checkpoint transcript rewrite projection after audit append failure"
                );
                match store
                    .delete_if_current_revision(session.id(), &rewritten_projection_token)
                    .await
                {
                    Ok(true) => {
                        return Err(SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(format!(
                                "checkpoint transcript rewrite audit append failed ({error}); projection rollback also failed: {rollback_error}; unaudited projection was quarantined"
                            )),
                        ));
                    }
                    Ok(false) => {
                        return Err(SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(format!(
                                "checkpoint transcript rewrite audit append failed ({error}); projection rollback also failed: {rollback_error}; projection changed before unaudited quarantine"
                            )),
                        ));
                    }
                    Err(quarantine_error) => {
                        return Err(SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(format!(
                                "checkpoint transcript rewrite audit append failed ({error}); projection rollback also failed: {rollback_error}; unaudited projection quarantine also failed: {quarantine_error}"
                            )),
                        ));
                    }
                }
            }
            return Err(error);
        }
        last_audited_projection = Some(rewritten);
        persisted_revision = Some(commit.revision.clone());
        persisted_projection_token = Some(rewritten_projection_token);
    }
    if commits.last().map(|commit| commit.revision.as_str()) != Some(incoming_revision.as_str()) {
        save_session_projection_after_verified_rewrite_chain(
            store,
            blob_store,
            session,
            &incoming_revision,
        )
        .await
        .map_err(|err| SessionError::Store(Box::new(err)))?;
    }
    Ok(())
}

async fn save_session_projection_after_verified_rewrite_chain(
    store: &dyn SessionStore,
    blob_store: &dyn BlobStore,
    session: &Session,
    incoming_revision: &str,
) -> Result<(), SessionStoreError> {
    match save_session_projection_with_storage_normalization_bridge(store, blob_store, session)
        .await
    {
        Ok(()) => Ok(()),
        Err(error) => {
            let Some(previous) = store.load(session.id()).await? else {
                return Err(error);
            };
            let previous_projection_token =
                meerkat_core::session_store::session_projection_cas_token(&previous)?;
            let Some(state) = session.transcript_history_state().map_err(|err| {
                SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!("incoming transcript history state is malformed: {err}"),
                }
            })?
            else {
                return Err(error);
            };
            if transcript_rewrite_commit_metadata_preserved(&previous, &state)?
                && (meerkat_core::session_store::run_boundary_snapshot_save_guard(
                    session,
                    Some(&previous),
                )
                .is_ok()
                    || transcript_revision_preserves_storage_normalized_append_prefix(
                        blob_store,
                        &state,
                        incoming_revision,
                        session.messages(),
                        previous.messages(),
                    )
                    .await?)
            {
                return store
                    .save_authoritative_projection_if_current_revision(
                        session,
                        Some(previous_projection_token),
                    )
                    .await;
            }
            Err(error)
        }
    }
}

async fn save_session_projection_with_storage_normalization_bridge(
    store: &dyn SessionStore,
    blob_store: &dyn BlobStore,
    session: &Session,
) -> Result<(), SessionStoreError> {
    match store.save(session).await {
        Ok(()) => Ok(()),
        Err(
            error @ (SessionStoreError::TranscriptContinuityViolation { .. }
            | SessionStoreError::InvalidTranscriptRewrite { .. }),
        ) => {
            let Some(previous) = store.load(session.id()).await? else {
                return Err(error);
            };
            let previous_revision = meerkat_core::transcript_messages_digest(previous.messages())
                .map_err(SessionStoreError::from)?;
            let previous_projection_token =
                meerkat_core::session_store::session_projection_cas_token(&previous)?;
            let mut normalized_previous = previous.clone();
            normalized_previous
                .externalize_media(blob_store, 0)
                .await
                .map_err(|err| {
                    SessionStoreError::Internal(format!(
                        "failed to externalize previous persisted projection for save bridge: {err}"
                    ))
                })?;
            let normalized_revision =
                meerkat_core::transcript_messages_digest(normalized_previous.messages())
                    .map_err(SessionStoreError::from)?;
            if normalized_revision == previous_revision {
                if save_verified_transcript_history_projection(
                    store,
                    blob_store,
                    session,
                    &normalized_previous,
                    previous_projection_token.clone(),
                )
                .await?
                {
                    return Ok(());
                }
                return Err(error);
            }
            if let Err(bridge_error) = meerkat_core::session_store::append_only_save_guard(
                session,
                Some(&normalized_previous),
            ) {
                tracing::debug!(
                    session_id = %session.id(),
                    original_error = %error,
                    bridge_error = %bridge_error,
                    "storage-normalized previous projection does not prove incoming save continuity"
                );
                if save_verified_transcript_history_projection(
                    store,
                    blob_store,
                    session,
                    &normalized_previous,
                    previous_projection_token.clone(),
                )
                .await?
                {
                    return Ok(());
                }
                return Err(error);
            }
            store
                .save_authoritative_projection_if_current_revision(
                    &normalized_previous,
                    Some(previous_projection_token),
                )
                .await?;
            store.save(session).await
        }
        Err(error) => Err(error),
    }
}

async fn save_verified_transcript_history_projection(
    store: &dyn SessionStore,
    blob_store: &dyn BlobStore,
    session: &Session,
    previous: &Session,
    expected_current_revision: String,
) -> Result<bool, SessionStoreError> {
    let Some(state) = session.transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        }
    })?
    else {
        return Ok(false);
    };
    session.validate_transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        }
    })?;
    let incoming_revision = meerkat_core::transcript_messages_digest(session.messages())
        .map_err(SessionStoreError::from)?;
    if state.head != incoming_revision
        || !state
            .revisions
            .iter()
            .any(|body| body.revision == incoming_revision)
    {
        return Ok(false);
    }
    if !transcript_rewrite_commit_metadata_preserved(previous, &state)? {
        tracing::debug!(
            session_id = %session.id(),
            "transcript-history projection would introduce rewrite commits without audit authority"
        );
        return Ok(false);
    }
    if let Err(error) =
        meerkat_core::session_store::run_boundary_snapshot_save_guard(session, Some(previous))
        && !transcript_revision_preserves_storage_normalized_append_prefix(
            blob_store,
            &state,
            &incoming_revision,
            session.messages(),
            previous.messages(),
        )
        .await?
    {
        tracing::debug!(
            session_id = %session.id(),
            bridge_error = %error,
            "transcript-history projection does not extend persisted previous revision"
        );
        return Ok(false);
    }
    store
        .save_authoritative_projection_if_current_revision(session, Some(expected_current_revision))
        .await?;
    Ok(true)
}

fn transcript_rewrite_commit_metadata_preserved(
    previous: &Session,
    incoming_state: &meerkat_core::TranscriptHistoryState,
) -> Result<bool, SessionStoreError> {
    let previous_state = previous.transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: previous.id().clone(),
            reason: format!("previous transcript history state is malformed: {err}"),
        }
    })?;
    let previous_commits = previous_state
        .as_ref()
        .map(|state| state.commits.as_slice())
        .unwrap_or_default();
    Ok(previous_commits == incoming_state.commits.as_slice())
}

#[cfg(test)]
async fn save_authoritative_projection_after_persisted_continuity_guard(
    store: &dyn SessionStore,
    blob_store: &dyn BlobStore,
    session: &Session,
) -> Result<(), SessionStoreError> {
    reject_projection_only_rewrite_commits(session)?;
    save_audited_authoritative_projection_after_persisted_continuity_guard(
        store, blob_store, session,
    )
    .await
}

async fn save_audited_authoritative_projection_after_persisted_continuity_guard(
    store: &dyn SessionStore,
    blob_store: &dyn BlobStore,
    session: &Session,
) -> Result<(), SessionStoreError> {
    let expected_current_revision =
        verify_authoritative_projection_persisted_continuity(store, blob_store, session).await?;
    store
        .save_authoritative_projection_if_current_revision(session, expected_current_revision)
        .await
}

#[cfg(test)]
fn reject_projection_only_rewrite_commits(session: &Session) -> Result<(), SessionStoreError> {
    let Some(state) = session.transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        }
    })?
    else {
        return Ok(());
    };
    if state.commits.is_empty() {
        return Ok(());
    }
    Err(SessionStoreError::InvalidTranscriptRewrite {
        id: session.id().clone(),
        reason: "projection-only authoritative save cannot introduce transcript rewrite commits"
            .to_string(),
    })
}

async fn verify_authoritative_projection_persisted_continuity(
    store: &dyn SessionStore,
    blob_store: &dyn BlobStore,
    session: &Session,
) -> Result<Option<String>, SessionStoreError> {
    let Some(previous) = store.load(session.id()).await? else {
        return Ok(None);
    };
    let previous_revision = meerkat_core::transcript_messages_digest(previous.messages())
        .map_err(SessionStoreError::from)?;
    let previous_projection_token =
        meerkat_core::session_store::session_projection_cas_token(&previous)?;
    match meerkat_core::session_store::run_boundary_snapshot_save_guard(session, Some(&previous)) {
        Ok(()) => Ok(Some(previous_projection_token)),
        Err(raw_error) => {
            let mut normalized_previous = previous.clone();
            normalized_previous
                .externalize_media(blob_store, 0)
                .await
                .map_err(|err| {
                    SessionStoreError::Internal(format!(
                        "failed to externalize previous persisted projection for authoritative save guard: {err}"
                    ))
                })?;
            let normalized_revision =
                meerkat_core::transcript_messages_digest(normalized_previous.messages())
                    .map_err(SessionStoreError::from)?;
            if normalized_revision != previous_revision
                && meerkat_core::session_store::run_boundary_snapshot_save_guard(
                    session,
                    Some(&normalized_previous),
                )
                .is_ok()
            {
                return Ok(Some(previous_projection_token));
            }
            Err(raw_error)
        }
    }
}

#[async_trait]
impl meerkat_core::checkpoint::SessionCheckpointer for StoreCheckpointer {
    async fn checkpoint(&self, session: &Session) {
        let guard = self.gate.cancelled.lock().await;
        if *guard {
            return;
        }
        let current_revision = match meerkat_core::transcript_messages_digest(session.messages()) {
            Ok(revision) => revision,
            Err(error) => {
                tracing::warn!("Host-mode checkpoint transcript digest failed: {error}");
                return;
            }
        };
        if self
            .last_saved_revision
            .lock()
            .is_ok_and(|revision| revision.as_ref() == Some(&current_revision))
        {
            return;
        }
        let mut persisted = session.clone();
        if let Err(e) = persisted
            .externalize_media(self.blob_store.as_ref(), 0)
            .await
        {
            tracing::warn!("Host-mode checkpoint blob externalization failed: {e}");
            return;
        }
        if let Some(mut state) = persisted.deferred_turn_state() {
            if let Err(e) =
                externalize_deferred_turn_state(self.blob_store.as_ref(), &mut state).await
            {
                tracing::warn!("Host-mode checkpoint deferred-turn externalization failed: {e}");
                return;
            }
            if let Err(err) = persisted.set_deferred_turn_state(state) {
                tracing::warn!("Host-mode checkpoint deferred-turn serialization failed: {err}");
                return;
            }
        }
        if let Err(e) = save_session_projection_allowing_internal_rewrite(
            self.store.as_ref(),
            self.blob_store.as_ref(),
            self.event_store.as_ref(),
            self.projector.as_ref(),
            &persisted,
        )
        .await
        {
            tracing::warn!("Host-mode checkpoint failed: {e}");
        } else if let Ok(mut last_saved_revision) = self.last_saved_revision.lock() {
            *last_saved_revision = Some(current_revision);
        }
        drop(guard);
    }
}

/// Session service backed by persistent storage.
///
/// Wraps `EphemeralSessionService` and saves session snapshots to a
/// `SessionStore` after each turn completes. On `list` and `read`,
/// merges live sessions with persisted sessions from the store.
pub struct PersistentSessionService<B: SessionAgentBuilder> {
    inner: EphemeralSessionService<B>,
    store: Arc<dyn SessionStore>,
    runtime_store: Option<Arc<dyn RuntimeStore>>,
    blob_store: Arc<dyn BlobStore>,
    event_store: Option<Arc<dyn EventStore>>,
    projector: Option<Arc<SessionProjector>>,
    /// Gates for active keep-alive checkpointers, keyed by session ID.
    /// Archive acquires the gate's lock, sets cancelled, then saves the
    /// archived snapshot -- mutual exclusion prevents a concurrent checkpoint
    /// from overwriting the archived row with a live one.
    checkpointer_gates: Mutex<HashMap<SessionId, Arc<CheckpointerGate>>>,
    /// Gates lazy live-session recovery and archive against each other so a
    /// stored-only session is rebuilt at most once and archived snapshots
    /// cannot become writable again through rehydration races.
    recovery_gates: Mutex<HashMap<SessionId, Arc<Mutex<()>>>>,
    /// Runtime snapshots quarantined after rejecting a checkpoint may fall back
    /// to the latest store projection for recovery in this service process.
    quarantined_runtime_projection_fallbacks: Mutex<HashSet<SessionId>>,
}

/// Extract session labels from a metadata map.
///
/// Looks for `SESSION_LABELS_KEY` and deserializes the value as
/// `BTreeMap<String, String>`. Returns an empty map on missing or
/// malformed data.
#[allow(dead_code)]
fn extract_labels_from_metadata(
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> BTreeMap<String, String> {
    match metadata.get(SESSION_LABELS_KEY) {
        Some(v) => match serde_json::from_value::<BTreeMap<String, String>>(v.clone()) {
            Ok(labels) => labels,
            Err(e) => {
                tracing::warn!(
                    key = SESSION_LABELS_KEY,
                    error = %e,
                    "failed to deserialize session labels from metadata"
                );
                BTreeMap::new()
            }
        },
        None => BTreeMap::new(),
    }
}

fn summary_from_meta(meta: meerkat_core::SessionMeta) -> SessionSummary {
    SessionSummary {
        session_id: meta.id,
        created_at: meta.created_at,
        updated_at: meta.updated_at,
        message_count: meta.message_count,
        total_tokens: meta.total_tokens,
        is_active: false,
        labels: extract_labels_from_metadata(&meta.metadata),
    }
}

fn view_from_authoritative_session(session: &Session) -> SessionView {
    let metadata = session.session_metadata();
    SessionView {
        state: SessionInfo {
            session_id: session.id().clone(),
            created_at: session.created_at(),
            updated_at: session.updated_at(),
            message_count: session.messages().len(),
            is_active: false,
            model: metadata
                .as_ref()
                .map(|metadata| metadata.model.clone())
                .unwrap_or_default(),
            provider: metadata
                .as_ref()
                .map(|metadata| metadata.provider)
                .unwrap_or(meerkat_core::Provider::Other),
            last_assistant_text: session.last_assistant_text(),
            labels: extract_labels_from_metadata(session.metadata()),
        },
        billing: SessionUsage {
            total_tokens: session.total_tokens(),
            usage: session.total_usage(),
        },
    }
}

fn metadata_marks_archived(metadata: &serde_json::Map<String, serde_json::Value>) -> bool {
    metadata
        .get(SESSION_ARCHIVED_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

enum LiveSessionAuthority {
    NoLive,
    LiveAuthoritative,
    DurableAuthoritative {
        session: Box<Session>,
        reason: &'static str,
    },
}

#[derive(Clone, Copy)]
enum ArchivedResumeAuthorization {
    RejectArchived,
    MachinePendingPromotion {
        _authority: MachineSessionControlAuthority,
    },
}

impl ArchivedResumeAuthorization {
    fn allows_archived_resume(self) -> bool {
        matches!(
            self,
            ArchivedResumeAuthorization::MachinePendingPromotion { .. }
        )
    }
}

/// Runtime/session composition protocol for a live service turn whose terminal
/// publication is owned by `MeerkatMachine`.
#[derive(Clone, Copy)]
pub struct MachineServiceTurnCommitProtocol<'a> {
    runtime_adapter: &'a MeerkatMachine,
    _authority: MachineSessionControlAuthority,
}

impl<'a> MachineServiceTurnCommitProtocol<'a> {
    #[must_use]
    pub fn from_machine(runtime_adapter: &'a MeerkatMachine) -> Self {
        Self {
            runtime_adapter,
            _authority: runtime_adapter.session_control_authority(),
        }
    }
}

/// Runtime/session composition protocol for archiving a session.
///
/// Archive is a user-facing session lifecycle transition. Runtime-backed
/// callers must first land the machine-owned `Retire` transition durably, then
/// may update the session-store projection used for history reads.
#[derive(Clone, Copy)]
pub struct MachineSessionArchiveProtocol<'a> {
    runtime_adapter: &'a MeerkatMachine,
    _authority: MachineSessionControlAuthority,
}

impl<'a> MachineSessionArchiveProtocol<'a> {
    #[must_use]
    pub fn from_machine(runtime_adapter: &'a MeerkatMachine) -> Self {
        Self {
            runtime_adapter,
            _authority: runtime_adapter.session_control_authority(),
        }
    }

    async fn session_registered(&self, id: &SessionId) -> bool {
        self.runtime_adapter.contains_session(id).await
    }

    fn require_shared_runtime_store(
        &self,
        id: &SessionId,
        runtime_store: &Arc<dyn RuntimeStore>,
    ) -> Result<(), SessionError> {
        if self
            .runtime_adapter
            .shares_runtime_store_authority(runtime_store)
        {
            return Ok(());
        }

        Err(SessionError::Unsupported(format!(
            "archive for session {id} requires MachineSessionArchiveProtocol backed by the same durable runtime/session machine store"
        )))
    }

    async fn retire_session(&self, id: &SessionId) -> Result<(), SessionError> {
        let runtime_id = LogicalRuntimeId::for_session(id);
        let retire_once =
            meerkat_runtime::RuntimeControlPlane::retire(self.runtime_adapter, &runtime_id).await;
        match retire_once {
            Ok(_) => return Ok(()),
            Err(meerkat_runtime::RuntimeControlPlaneError::NotFound(_)) => {
                self.runtime_adapter.register_session(id.clone()).await;
            }
            Err(error) => {
                return Err(SessionError::Agent(AgentError::InternalError(format!(
                    "machine archive retire failed: {error}"
                ))));
            }
        }

        meerkat_runtime::RuntimeControlPlane::retire(self.runtime_adapter, &runtime_id)
            .await
            .map(|_| ())
            .map_err(|error| match error {
                meerkat_runtime::RuntimeControlPlaneError::NotFound(_) => {
                    SessionError::NotFound { id: id.clone() }
                }
                error => SessionError::Agent(AgentError::InternalError(format!(
                    "machine archive retire failed after registration: {error}"
                ))),
            })
    }
}

impl<B: SessionAgentBuilder + 'static> PersistentSessionService<B> {
    fn archived_not_found(id: &SessionId) -> SessionControlError {
        SessionControlError::Session(SessionError::NotFound { id: id.clone() })
    }

    async fn reject_if_archived_session(
        &self,
        id: &SessionId,
        session: &Session,
    ) -> Result<(), SessionControlError> {
        if self
            .session_archived_by_authority(id, session)
            .await
            .map_err(SessionControlError::Session)?
        {
            return Err(Self::archived_not_found(id));
        }
        Ok(())
    }

    pub async fn session_archived_by_authority(
        &self,
        id: &SessionId,
        session: &Session,
    ) -> Result<bool, SessionError> {
        let Some(runtime_store) = self.runtime_store.as_ref() else {
            return Ok(metadata_marks_archived(session.metadata()));
        };

        match Self::load_runtime_state_for_session(runtime_store, id).await? {
            Some(RuntimeState::Retired) => Ok(true),
            Some(_) => Ok(false),
            None if metadata_marks_archived(session.metadata()) => {
                Err(SessionError::Agent(AgentError::InternalError(format!(
                    "runtime-backed archived projection for session {id} is missing machine lifecycle state"
                ))))
            }
            None => Ok(false),
        }
    }

    fn runtime_id_for_session(id: &SessionId) -> LogicalRuntimeId {
        LogicalRuntimeId::for_session(id)
    }

    #[cfg(test)]
    async fn runtime_input_updates(
        &self,
        id: &SessionId,
        run_id: &RunId,
        sequence: u64,
        contributing_input_ids: &[InputId],
    ) -> Result<Vec<StoredInputState>, SessionError> {
        let Some(runtime_store) = self.runtime_store.as_ref() else {
            return Ok(Vec::new());
        };
        let runtime_id = Self::runtime_id_for_session(id);
        let stored_states = runtime_store
            .load_input_states(&runtime_id)
            .await
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to load runtime input states: {err}"
                )))
            })?;

        Ok(contributing_input_ids
            .iter()
            .filter_map(|input_id| {
                let mut bundle = stored_states
                    .iter()
                    .find(|candidate| &candidate.state.input_id == input_id)?
                    .clone();
                // Stamp receipt metadata and mirror the Consumed terminal on
                // the persisted snapshot. The authoritative DSL transition
                // fires on the live driver via `machine_realize_run_completed`;
                // this clone is only what the store persists alongside the
                // boundary receipt, so `updated_at` tracks that receipt's
                // logical moment rather than wall-clock.
                bundle.seed.last_run_id = Some(run_id.clone());
                bundle.seed.last_boundary_sequence = Some(sequence);
                bundle.seed.phase = InputLifecycleState::Consumed;
                bundle.seed.terminal_outcome = Some(InputTerminalOutcome::Consumed);
                bundle.state.terminal_outcome = Some(InputTerminalOutcome::Consumed);
                Some(bundle)
            })
            .collect())
    }

    async fn commit_runtime_apply(
        &self,
        _id: &SessionId,
        run_id: RunId,
        boundary: RunApplyBoundary,
        session: &Session,
        contributing_input_ids: &[InputId],
    ) -> Result<RunBoundaryReceipt, SessionError> {
        if self.runtime_store.is_none() {
            let persisted = self.save_normalized_session(session.clone()).await?;
            return Self::build_runtime_receipt(
                run_id,
                boundary,
                contributing_input_ids.to_vec(),
                &persisted,
            );
        }

        Self::build_runtime_receipt(run_id, boundary, contributing_input_ids.to_vec(), session)
    }

    async fn export_session_with_labels(&self, id: &SessionId) -> Result<Session, SessionError> {
        let mut session = self.inner.export_session(id).await?;
        if let Ok(view) = self.inner.read(id).await
            && !view.state.labels.is_empty()
            && let Ok(labels_value) = serde_json::to_value(&view.state.labels)
        {
            session.set_metadata(SESSION_LABELS_KEY, labels_value);
        }
        Ok(session)
    }

    /// Apply a runtime-turn metadata LLM identity update to a live session.
    ///
    /// This is intentionally not exposed through the public
    /// `SessionService::hot_swap_session_llm_identity` trait method: callers
    /// must arrive here through the runtime-owned turn/reconfigure path, after
    /// the runtime has resolved the typed metadata and built the client.
    pub async fn apply_runtime_session_llm_identity(
        &self,
        id: &SessionId,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        identity: meerkat_core::SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        self.inner
            .hot_swap_session_llm_identity(id, client, identity, request_policy)
            .await?;
        self.persist_full_session_or_discard_live(id)
            .await
            .map(|_| ())
    }

    /// Apply a runtime-turn metadata keep-alive update to a live session.
    ///
    /// The public trait method remains blocked so keep-alive changes do not
    /// bypass the runtime turn metadata seam.
    pub async fn apply_runtime_session_keep_alive(
        &self,
        id: &SessionId,
        keep_alive: bool,
    ) -> Result<(), SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        let previous = self
            .export_session_with_labels(id)
            .await
            .ok()
            .and_then(|session| session.session_metadata().map(|meta| meta.keep_alive));
        self.inner.update_session_keep_alive(id, keep_alive).await?;
        match self.persist_full_session(id).await {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(previous) = previous {
                    let _ = self.inner.update_session_keep_alive(id, previous).await;
                }
                Err(error)
            }
        }
    }

    async fn load_authoritative_session_base(
        &self,
        id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        let (session, _materialized_from_replay) = self
            .load_authoritative_session_base_with_replay_info(id)
            .await?;
        if let Some(session) = session.as_ref() {
            self.verify_transcript_rewrite_audit_events(session).await?;
        }
        Ok(session)
    }

    async fn load_authoritative_session_base_with_replay_info(
        &self,
        id: &SessionId,
    ) -> Result<(Option<Session>, bool), SessionError> {
        let session = if let Some(runtime_store) = self.runtime_store.as_ref() {
            match Self::load_runtime_session_snapshot_for_session(runtime_store, id).await? {
                Some(session) => Some(session),
                None => {
                    let store_projection = self
                        .store
                        .load(id)
                        .await
                        .map_err(|e| SessionError::Store(Box::new(e)))?;
                    match store_projection {
                        Some(session)
                            if runtime_backed_store_projection_can_recover_authority(&session)
                                || self.runtime_projection_fallback_quarantined(id).await =>
                        {
                            Some(session)
                        }
                        _ => None,
                    }
                }
            }
        } else {
            self.store
                .load(id)
                .await
                .map_err(|e| SessionError::Store(Box::new(e)))?
        };
        self.apply_transcript_rewrite_replay(id, session).await
    }

    async fn transcript_rewrite_event_records(
        &self,
        id: &SessionId,
    ) -> Result<Option<Vec<meerkat_core::TranscriptRewriteRecord>>, SessionError> {
        let Some(event_store) = self.event_store.as_ref() else {
            return Ok(None);
        };
        let events = event_store.read_from(id, 1).await.map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to read transcript rewrite events for session {id}: {err}"
            )))
        })?;
        let records = events
            .into_iter()
            .filter_map(|stored| match stored.event {
                meerkat_core::event::AgentEvent::TranscriptRewriteCommitted {
                    session_id,
                    record,
                } if session_id == *id => Some((stored.seq, record)),
                _ => None,
            })
            .collect::<Vec<_>>();
        Ok(Some(Self::ordered_transcript_rewrite_records(records)))
    }

    fn ordered_transcript_rewrite_records(
        mut records: Vec<(u64, meerkat_core::TranscriptRewriteRecord)>,
    ) -> Vec<meerkat_core::TranscriptRewriteRecord> {
        let mut ordered = Vec::with_capacity(records.len());
        let produced_revisions = records
            .iter()
            .map(|(_, record)| record.commit.revision.clone())
            .collect::<HashSet<_>>();
        let mut known_revisions = records
            .iter()
            .filter_map(|(_, record)| {
                let parent = &record.commit.parent_revision;
                (!produced_revisions.contains(parent)).then(|| parent.clone())
            })
            .collect::<HashSet<_>>();
        if known_revisions.is_empty()
            && let Some((_, first)) = records.iter().min_by_key(|(seq, _)| *seq)
        {
            known_revisions.insert(first.commit.parent_revision.clone());
        }

        while !records.is_empty() {
            let mut candidates = records
                .iter()
                .enumerate()
                .filter(|(_, (_, candidate))| {
                    known_revisions.contains(&candidate.commit.parent_revision)
                })
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                candidates = records.iter().enumerate().collect::<Vec<_>>();
            }
            candidates.sort_by_key(|(_, (seq, _))| *seq);
            let (index, _) = candidates[0];
            let (_, record) = records.remove(index);
            if !ordered
                .iter()
                .any(|existing: &meerkat_core::TranscriptRewriteRecord| {
                    existing.commit == record.commit
                })
            {
                known_revisions.insert(record.commit.revision.clone());
                ordered.push(record);
            }
        }

        ordered
    }

    async fn transcript_history_state_from_event_records(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::TranscriptHistoryState>, SessionError> {
        let Some(records) = self.transcript_rewrite_event_records(id).await? else {
            return Ok(None);
        };
        meerkat_core::TranscriptHistoryState::from_rewrite_records(records).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to rebuild transcript history for session {id}: {err}"
            )))
        })
    }

    async fn verify_transcript_rewrite_audit_events(
        &self,
        session: &Session,
    ) -> Result<(), SessionError> {
        self.verify_transcript_rewrite_audit_events_locked(session)
            .await
    }

    async fn verify_transcript_rewrite_audit_events_locked(
        &self,
        session: &Session,
    ) -> Result<(), SessionError> {
        if self.event_store.is_none() {
            return Ok(());
        }
        let Some(state) = session.transcript_history_state().map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to read transcript history for session {}: {err}",
                session.id()
            )))
        })?
        else {
            return Ok(());
        };
        if state.commits.is_empty() {
            return Ok(());
        }

        let existing_records = self
            .transcript_rewrite_event_records(session.id())
            .await?
            .unwrap_or_default();
        let existing_commits = existing_records
            .iter()
            .map(|record| &record.commit)
            .collect::<Vec<_>>();

        let missing_commits = state
            .commits
            .iter()
            .filter(|commit| !existing_commits.contains(commit))
            .cloned()
            .collect::<Vec<_>>();
        if !missing_commits.is_empty() {
            append_transcript_rewrite_commit_events(
                self.event_store.as_ref(),
                self.projector.as_ref(),
                session,
                &missing_commits,
            )
            .await?;
        }
        Ok(())
    }

    async fn apply_transcript_rewrite_replay(
        &self,
        id: &SessionId,
        session: Option<Session>,
    ) -> Result<(Option<Session>, bool), SessionError> {
        let Some(mut session) = session else {
            return Ok((None, false));
        };
        let Some(replayed_state) = self.transcript_history_state_from_event_records(id).await?
        else {
            return Ok((Some(session), false));
        };

        let current_digest =
            meerkat_core::transcript_messages_digest(session.messages()).map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to digest current transcript for session {id}: {err}"
                )))
            })?;
        let existing_state = session.transcript_history_state().map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to read transcript history for session {id}: {err}"
            )))
        })?;
        let current_revision = existing_state
            .as_ref()
            .map(|state| state.head.clone())
            .unwrap_or_else(|| current_digest.clone());
        let replay_contains_current_digest = replayed_state
            .revisions
            .iter()
            .any(|body| body.revision == current_digest);
        let replay_contains_current = replayed_state
            .revisions
            .iter()
            .any(|body| body.revision == current_revision);
        if let Some(existing_state) = existing_state {
            if replay_contains_current_digest || replay_contains_current {
                let existing_state_value = serde_json::to_value(&existing_state).map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to serialize existing transcript history for session {id}: {err}"
                    )))
                })?;
                let replay_covers_existing_commits = existing_state.commits.iter().all(|commit| {
                    replayed_state
                        .commits
                        .iter()
                        .any(|replayed| replayed == commit)
                });
                let materialized_state = Self::merge_transcript_history_replay(
                    existing_state,
                    replayed_state,
                    replay_covers_existing_commits,
                );
                let materialized_state_value =
                    serde_json::to_value(&materialized_state).map_err(|err| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!(
                                "failed to serialize replayed transcript history for session {id}: {err}"
                            ),
                        ))
                    })?;
                session
                    .apply_transcript_history_state(materialized_state)
                    .map_err(|err| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!(
                                "failed to materialize transcript history for session {id}: {err}"
                            ),
                        ))
                    })?;
                return Ok((
                    Some(session),
                    materialized_state_value != existing_state_value,
                ));
            }
            return Ok((Some(session), false));
        }

        if replay_contains_current_digest {
            session
                .apply_transcript_history_state(replayed_state)
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to materialize transcript history for session {id}: {err}"
                    )))
                })?;
            return Ok((Some(session), true));
        }
        if let Some(materialized_state) =
            Self::extend_replayed_history_to_current_projection(&session, replayed_state)?
        {
            session
                .apply_transcript_history_state(materialized_state)
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to materialize transcript history for session {id}: {err}"
                    )))
                })?;
            return Ok((Some(session), true));
        }
        Ok((Some(session), false))
    }

    fn extend_replayed_history_to_current_projection(
        session: &Session,
        mut replayed_state: meerkat_core::TranscriptHistoryState,
    ) -> Result<Option<meerkat_core::TranscriptHistoryState>, SessionError> {
        let current_revision = meerkat_core::transcript_messages_digest(session.messages())
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to digest current transcript for session {}: {err}",
                    session.id()
                )))
            })?;
        if replayed_state
            .revisions
            .iter()
            .any(|body| body.revision == current_revision)
        {
            replayed_state.head = current_revision;
            return Ok(Some(replayed_state));
        }
        let Some(replayed_head_body) = replayed_state
            .revisions
            .iter()
            .find(|body| body.revision == replayed_state.head)
        else {
            return Ok(None);
        };
        let replayed_len = replayed_head_body.messages.len();
        if session.messages().len() < replayed_len {
            return Ok(None);
        }
        let current_prefix_digest = meerkat_core::transcript_messages_digest(
            &session.messages()[..replayed_len],
        )
        .map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to digest current transcript prefix for session {}: {err}",
                session.id()
            )))
        })?;
        if current_prefix_digest != replayed_state.head {
            return Ok(None);
        }

        let previous_head = replayed_state.head.clone();
        replayed_state
            .revisions
            .push(meerkat_core::TranscriptRevisionBody {
                revision: current_revision.clone(),
                parent_revision: Some(previous_head),
                messages: session.messages().to_vec(),
                created_at: session.updated_at(),
            });
        replayed_state.head = current_revision;
        Ok(Some(replayed_state))
    }

    fn merge_transcript_history_replay(
        mut base: meerkat_core::TranscriptHistoryState,
        replayed: meerkat_core::TranscriptHistoryState,
        adopt_replayed_head: bool,
    ) -> meerkat_core::TranscriptHistoryState {
        let durable_head = base.head.clone();
        let replayed_head = replayed.head.clone();
        for revision in replayed.revisions {
            if !base
                .revisions
                .iter()
                .any(|existing| existing.revision == revision.revision)
            {
                base.revisions.push(revision);
            }
        }
        for commit in replayed.commits {
            if !base.commits.iter().any(|existing| existing == &commit) {
                base.commits.push(commit);
            }
        }
        base.head = if adopt_replayed_head {
            replayed_head
        } else {
            durable_head
        };
        base
    }

    async fn load_runtime_session_snapshot(
        runtime_store: &Arc<dyn RuntimeStore>,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Session>, SessionError> {
        runtime_store
            .load_session_snapshot(runtime_id)
            .await
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to load runtime session snapshot: {err}"
                )))
            })?
            .map(|bytes| {
                meerkat_core::session_migrations::deserialize_session_migrating(&bytes).map_err(
                    |err| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("failed to deserialize runtime session snapshot: {err}"),
                        ))
                    },
                )
            })
            .transpose()
    }

    async fn load_runtime_session_snapshot_for_session(
        runtime_store: &Arc<dyn RuntimeStore>,
        id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        let runtime_id = Self::runtime_id_for_session(id);
        Self::load_runtime_session_snapshot(runtime_store, &runtime_id).await
    }

    async fn load_runtime_state_for_session(
        runtime_store: &Arc<dyn RuntimeStore>,
        id: &SessionId,
    ) -> Result<Option<RuntimeState>, SessionError> {
        let runtime_id = Self::runtime_id_for_session(id);
        meerkat_runtime::store::load_runtime_state(runtime_store.as_ref(), &runtime_id)
            .await
            .map_err(|err| {
                SessionError::Agent(AgentError::InternalError(format!(
                    "failed to load runtime state: {err}"
                )))
            })
    }

    fn store_only_control_mutation_error(id: &SessionId, operation: &str) -> SessionError {
        SessionError::Unsupported(format!(
            "{operation} cannot mutate store-only compatibility projection for session {id}; session control mutations require an authoritative runtime/session machine snapshot"
        ))
    }

    async fn load_runtime_authority_session_for_control(
        &self,
        id: &SessionId,
        operation: &str,
    ) -> Result<Option<Session>, SessionError> {
        let Some(runtime_store) = self.runtime_store.as_ref() else {
            return Ok(None);
        };

        if let Some(runtime) =
            Self::load_runtime_session_snapshot_for_session(runtime_store, id).await?
        {
            return Ok(Some(runtime));
        }

        if self
            .store
            .exists(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?
        {
            return Err(Self::store_only_control_mutation_error(id, operation));
        }

        Ok(None)
    }

    async fn load_persisted_session_for_control(
        &self,
        id: &SessionId,
        operation: &str,
    ) -> Result<Option<Session>, SessionError> {
        if self.runtime_store.is_some() {
            return self
                .load_runtime_authority_session_for_control(id, operation)
                .await;
        }

        if self
            .store
            .exists(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?
        {
            return Err(Self::store_only_control_mutation_error(id, operation));
        }

        Ok(None)
    }

    async fn load_machine_authority_session_for_control(
        &self,
        id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        if self.runtime_store.is_some() {
            return self
                .load_runtime_authority_session_for_control(id, "archive")
                .await;
        }

        self.load_persisted_session_for_control(id, "archive").await
    }

    async fn live_session_authority(
        &self,
        id: &SessionId,
    ) -> Result<LiveSessionAuthority, SessionError> {
        let live = match self.export_session_with_labels(id).await {
            Ok(session) => session,
            Err(SessionError::NotFound { .. }) => return Ok(LiveSessionAuthority::NoLive),
            Err(err) => return Err(err),
        };

        let using_runtime_store = self.runtime_store.is_some();
        let stored = if let Some(runtime_store) = self.runtime_store.as_ref() {
            let Some(runtime) =
                Self::load_runtime_session_snapshot_for_session(runtime_store, id).await?
            else {
                return Ok(LiveSessionAuthority::LiveAuthoritative);
            };
            runtime
        } else {
            let Some(stored) = self
                .store
                .load(id)
                .await
                .map_err(|e| SessionError::Store(Box::new(e)))?
            else {
                return Ok(LiveSessionAuthority::LiveAuthoritative);
            };
            stored
        };

        let stored_revision = stored.transcript_revision().map_err(|err| {
            SessionError::Agent(AgentError::InternalError(format!(
                "failed to read durable transcript revision for session {id}: {err}"
            )))
        })?;
        let live_revision = live.transcript_revision().map_err(|err| {
            SessionError::Agent(AgentError::InternalError(format!(
                "failed to read live transcript revision for session {id}: {err}"
            )))
        })?;
        let stored_transcript_diverged = stored_revision != live_revision;
        let live_has_uncommitted_transcript =
            using_runtime_store && live.messages().len() > stored.messages().len();
        let runtime_system_context_diverged = using_runtime_store
            && stored.system_context_state().unwrap_or_default()
                != live.system_context_state().unwrap_or_default();
        let stored_is_archived = self.session_archived_by_authority(id, &stored).await?;

        // A durable snapshot timestamp is a projection witness, not live-session
        // authority. Runtime commits can normalize/persist an equivalent
        // transcript a few microseconds after the live session updates itself;
        // evicting the live handle on timestamp alone drops mechanical runtime
        // resources such as comms. For runtime-backed sessions, however, a
        // live transcript ahead of the durable runtime snapshot is an
        // uncommitted mutation and must fail closed so commit errors cannot
        // become visible session truth.
        if !stored_transcript_diverged
            && !stored_is_archived
            && !live_has_uncommitted_transcript
            && !runtime_system_context_diverged
        {
            return Ok(LiveSessionAuthority::LiveAuthoritative);
        }

        let reason = if stored_is_archived {
            "stored_archived"
        } else if live_has_uncommitted_transcript {
            "live_uncommitted_transcript"
        } else if runtime_system_context_diverged {
            "runtime_system_context_diverged"
        } else {
            "stored_transcript_revision_diverged"
        };

        Ok(LiveSessionAuthority::DurableAuthoritative {
            session: Box::new(stored),
            reason,
        })
    }

    async fn discard_stale_live_session_if_needed(
        &self,
        id: &SessionId,
    ) -> Result<bool, SessionError> {
        let LiveSessionAuthority::DurableAuthoritative { session, reason } =
            self.live_session_authority(id).await?
        else {
            return Ok(false);
        };

        if self
            .synchronize_runtime_backed_live_from_durable_authority(id, session.as_ref(), reason)
            .await?
        {
            return Ok(false);
        }

        let live = match self.export_session_with_labels(id).await {
            Ok(session) => session,
            Err(SessionError::NotFound { .. }) => return Ok(false),
            Err(err) => return Err(err),
        };

        tracing::debug!(
            session_id = %id,
            live_updated_at = ?live.updated_at(),
            stored_updated_at = ?session.updated_at(),
            live_message_count = live.messages().len(),
            stored_message_count = session.messages().len(),
            reason,
            "discarding stale live session in favor of newer durable session-store snapshot"
        );
        self.discard_live_session(id).await?;
        Ok(true)
    }

    async fn synchronize_live_runtime_context_state_from_durable(
        &self,
        id: &SessionId,
        durable: &Session,
        reason: &'static str,
    ) -> Result<(), SessionError> {
        if self.runtime_store.is_none() {
            return Ok(());
        }

        let durable_state = durable.system_context_state().unwrap_or_default();
        let state_handle = self.inner.system_context_state(id).await.ok_or_else(|| {
            SessionError::Agent(AgentError::InternalError(format!(
                "runtime-backed live session {id} is missing its system-context authority handle"
            )))
        })?;

        let changed = state_handle
            .replace_from_generated_restore_if_changed(durable_state)
            .map_err(|err| {
                SessionError::Agent(AgentError::InternalError(format!(
                    "failed to restore durable runtime-system-context state: {err}"
                )))
            })?;

        if changed {
            self.inner.sync_system_context_state(id).await?;
            tracing::debug!(
                session_id = %id,
                reason,
                "synchronized live runtime-system-context state from durable realtime authority"
            );
        }

        Ok(())
    }

    async fn synchronize_live_session_from_durable(
        &self,
        id: &SessionId,
        durable: &Session,
        reason: &'static str,
    ) -> Result<(), SessionError> {
        self.inner
            .sync_session_from_durable_snapshot(id, durable.clone())
            .await?;
        tracing::debug!(
            session_id = %id,
            reason,
            "synchronized live session snapshot from durable realtime authority"
        );
        Ok(())
    }

    async fn synchronize_runtime_backed_live_from_durable_authority(
        &self,
        id: &SessionId,
        durable: &Session,
        reason: &'static str,
    ) -> Result<bool, SessionError> {
        if self.session_archived_by_authority(id, durable).await? {
            return Ok(false);
        }

        if self.runtime_store.is_some() && reason == "runtime_system_context_diverged" {
            self.synchronize_live_runtime_context_state_from_durable(id, durable, reason)
                .await?;
        } else {
            match self
                .synchronize_live_session_from_durable(id, durable, reason)
                .await
            {
                Ok(()) => {}
                Err(error) if is_durable_session_sync_unsupported(&error) => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }

    pub async fn synchronize_live_session_from_durable_authority_if_needed(
        &self,
        id: &SessionId,
    ) -> Result<bool, SessionError> {
        match self.live_session_authority(id).await? {
            LiveSessionAuthority::DurableAuthoritative { session, reason } => {
                self.synchronize_runtime_backed_live_from_durable_authority(
                    id,
                    session.as_ref(),
                    reason,
                )
                .await
            }
            LiveSessionAuthority::NoLive | LiveSessionAuthority::LiveAuthoritative => Ok(false),
        }
    }

    async fn recover_live_session_from_store_if_needed_locked(
        &self,
        id: &SessionId,
    ) -> Result<bool, SessionError> {
        if self.inner.has_live_session(id).await? {
            return Ok(false);
        }

        let Some(stored) = self.load_authoritative_session_base(id).await? else {
            return Ok(false);
        };
        self.reject_if_archived_session(id, &stored)
            .await
            .map_err(control_error_into_session_error)?;

        let _ = stored;
        Err(SessionError::Agent(AgentError::InternalError(
            "stored-session recovery via non-canonical runtime-binding providers has been deleted; callers must materialize sessions through the canonical runtime-binding seam".to_string(),
        )))
    }

    pub async fn export_live_session(&self, id: &SessionId) -> Result<Session, SessionError> {
        if matches!(
            self.live_session_authority(id).await?,
            LiveSessionAuthority::DurableAuthoritative { .. }
        ) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        self.export_session_with_labels(id).await
    }

    async fn source_session_for_transcript_edit(
        &self,
        id: &SessionId,
    ) -> Result<Session, SessionError> {
        let _mutation_guard = self.transcript_edit_mutation_guard(id).await?;
        self.source_session_for_transcript_edit_locked(id).await
    }

    async fn source_session_for_transcript_edit_locked(
        &self,
        id: &SessionId,
    ) -> Result<Session, SessionError> {
        let view = self.read(id).await?;
        if view.state.is_active {
            return Err(SessionError::Busy { id: id.clone() });
        }

        let session = match self
            .load_authoritative_session_base_with_replay_info(id)
            .await?
        {
            (Some(session), materialized_from_replay) => {
                if materialized_from_replay {
                    self.persist_replayed_transcript_projection_for_mutation(&session)
                        .await?;
                }
                self.verify_transcript_rewrite_audit_events_locked(&session)
                    .await?;
                session
            }
            (None, _) => match self.export_session_with_labels(id).await {
                Ok(session) => session,
                Err(SessionError::NotFound { .. }) => {
                    return Err(SessionError::NotFound { id: id.clone() });
                }
                Err(err) => return Err(err),
            },
        };

        self.reject_if_archived_session(id, &session)
            .await
            .map_err(control_error_into_session_error)?;
        Ok(session)
    }

    async fn persist_replayed_transcript_projection_for_mutation(
        &self,
        session: &Session,
    ) -> Result<(), SessionError> {
        let expected_current_revision = self
            .store
            .load(session.id())
            .await
            .map_err(|err| SessionError::Store(Box::new(err)))?
            .as_ref()
            .map(meerkat_core::session_store::session_projection_cas_token)
            .transpose()
            .map_err(|err| SessionError::Store(Box::new(err)))?;
        let runtime_session_snapshot = if self.runtime_store.is_some() {
            Some(serde_json::to_vec(session).map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to serialize replay-recovered session snapshot: {err}"
                )))
            })?)
        } else {
            None
        };
        if let (Some(runtime_store), Some(session_snapshot)) = (
            self.runtime_store.as_ref(),
            runtime_session_snapshot.as_ref(),
        ) {
            runtime_store
                .commit_session_snapshot(
                    &Self::runtime_id_for_session(session.id()),
                    SessionDelta {
                        session_snapshot: session_snapshot.clone(),
                    },
                )
                .await
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "runtime replay projection persistence failed: {err}"
                    )))
                })?;
        }
        if let Err(error) = self
            .store
            .save_authoritative_projection_if_current_revision(session, expected_current_revision)
            .await
        {
            if self.runtime_store.is_some() {
                return Err(self
                    .fail_closed_runtime_projection_update(
                        session.id(),
                        error,
                        runtime_session_snapshot.as_deref(),
                    )
                    .await);
            }
            return Err(SessionError::Store(Box::new(error)));
        }
        Ok(())
    }

    async fn transcript_edit_mutation_guard(
        &self,
        id: &SessionId,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, SessionError> {
        match self.inner.join_active_runtime_context_admission(id).await {
            Ok(Some(active_admission)) => {
                drop(active_admission);
                return Err(SessionError::Busy { id: id.clone() });
            }
            Ok(None) | Err(SessionError::NotFound { .. }) => {}
            Err(error) => return Err(error),
        }

        let recovery_gate = self.recovery_gate_for_session(id).await;
        let guard = recovery_gate.lock_owned().await;
        match self.inner.join_active_runtime_context_admission(id).await {
            Ok(Some(active_admission)) => {
                drop(active_admission);
                return Err(SessionError::Busy { id: id.clone() });
            }
            Ok(None) | Err(SessionError::NotFound { .. }) => {}
            Err(error) => return Err(error),
        }

        let _ = self.discard_stale_live_session_if_needed(id).await?;
        Ok(guard)
    }

    async fn persist_transcript_fork(
        &self,
        source_session_id: SessionId,
        forked: Session,
    ) -> Result<SessionForkResult, SessionError> {
        let saved = self.save_normalized_session(forked).await?;
        Ok(SessionForkResult {
            source_session_id,
            session_id: saved.id().clone(),
            message_count: saved.messages().len(),
            session_ref: None,
        })
    }

    async fn persist_transcript_rewrite(
        &self,
        session: Session,
        commit: &meerkat_core::TranscriptRewriteCommit,
    ) -> Result<Session, SessionError> {
        let session = self.normalized_session_for_persistence(session).await?;
        self.persist_normalized_transcript_rewrite(session, commit, true)
            .await
    }

    async fn persist_normalized_transcript_rewrite(
        &self,
        session: Session,
        commit: &meerkat_core::TranscriptRewriteCommit,
        converge_live: bool,
    ) -> Result<Session, SessionError> {
        self.persist_normalized_transcript_rewrite_chain(
            session,
            std::slice::from_ref(commit),
            converge_live,
        )
        .await
    }

    async fn persist_normalized_transcript_rewrite_chain(
        &self,
        session: Session,
        commits: &[meerkat_core::TranscriptRewriteCommit],
        converge_live: bool,
    ) -> Result<Session, SessionError> {
        if commits.is_empty() {
            return Ok(session);
        }
        let previous = if let Some(runtime_store) = self.runtime_store.as_ref() {
            Self::load_runtime_session_snapshot_for_session(runtime_store, session.id()).await?
        } else {
            self.store
                .load(session.id())
                .await
                .map_err(|err| SessionError::Store(Box::new(err)))?
        };
        if let Some(runtime_store) = self.runtime_store.as_ref() {
            if let Err(error) = verify_authoritative_projection_persisted_continuity(
                self.store.as_ref(),
                self.blob_store.as_ref(),
                &session,
            )
            .await
            {
                return Err(self
                    .fail_closed_runtime_projection_preflight(session.id(), error)
                    .await);
            }
            let mut last_audited_projection = previous.clone();
            let mut persisted_revision = previous
                .as_ref()
                .map(meerkat_core::Session::transcript_revision)
                .transpose()
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to read previous transcript revision for runtime rewrite persistence: {err}"
                    )))
                })?;
            for commit in commits {
                if persisted_revision.as_deref() != Some(commit.parent_revision.as_str()) {
                    let bridge = session_materialized_at_transcript_revision(
                        &session,
                        &commit.parent_revision,
                    )?;
                    let session_snapshot = serde_json::to_vec(&bridge).map_err(|err| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!(
                                "failed to serialize bridged transcript rewrite snapshot for runtime persistence: {err}"
                            ),
                        ))
                    })?;
                    runtime_store
                        .commit_session_snapshot(
                            &Self::runtime_id_for_session(session.id()),
                            SessionDelta { session_snapshot },
                        )
                        .await
                        .map_err(|err| {
                            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                                format!("runtime bridged transcript rewrite snapshot persistence failed: {err}"),
                            ))
                        })?;
                }
                let rewritten =
                    session_materialized_at_transcript_revision(&session, &commit.revision)?;
                let session_snapshot = serde_json::to_vec(&rewritten).map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to serialize rewritten session snapshot for runtime persistence: {err}"
                    )))
                })?;
                runtime_store
                    .commit_session_transcript_rewrite_snapshot(
                        &Self::runtime_id_for_session(session.id()),
                        SessionDelta {
                            session_snapshot: session_snapshot.clone(),
                        },
                        commit,
                    )
                    .await
                    .map_err(|err| match err {
                        meerkat_runtime::store::RuntimeStoreError::TranscriptRevisionConflict {
                            expected,
                            actual,
                        } => {
                            meerkat_core::TranscriptEditError::RevisionConflict { expected, actual }
                                .into_session_error()
                        }
                        other => SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(format!(
                                "runtime transcript rewrite snapshot persistence failed: {other}"
                            )),
                        ),
                    })?;
                if let Err(error) = append_transcript_rewrite_commit_events(
                    self.event_store.as_ref(),
                    self.projector.as_ref(),
                    &session,
                    std::slice::from_ref(commit),
                )
                .await
                {
                    let mut rollback_failure = None;
                    if let Some(rollback_target) = last_audited_projection.as_ref() {
                        if let Some(runtime_store) = self.runtime_store.as_ref() {
                            match serde_json::to_vec(rollback_target) {
                                Ok(rollback_snapshot) => {
                                    match runtime_store
                                        .replace_session_snapshot_if_current(
                                            &Self::runtime_id_for_session(rollback_target.id()),
                                            &session_snapshot,
                                            rollback_snapshot,
                                        )
                                        .await
                                    {
                                        Ok(true) => {}
                                        Ok(false) => {
                                            tracing::warn!(
                                                session_id = %rollback_target.id(),
                                                "runtime snapshot changed before transcript rewrite audit rollback; leaving newer runtime authority intact"
                                            );
                                        }
                                        Err(rollback_error) => {
                                            tracing::error!(
                                                session_id = %rollback_target.id(),
                                                error = %rollback_error,
                                                "failed to roll back runtime transcript rewrite snapshot after audit append failure"
                                            );
                                            rollback_failure = Some(format!(
                                                "runtime snapshot rollback failed: {rollback_error}"
                                            ));
                                        }
                                    }
                                }
                                Err(rollback_error) => {
                                    tracing::error!(
                                        session_id = %rollback_target.id(),
                                        error = %rollback_error,
                                        "failed to serialize previous runtime transcript snapshot for rollback"
                                    );
                                    rollback_failure = Some(format!(
                                        "runtime rollback snapshot serialization failed: {rollback_error}"
                                    ));
                                }
                            }
                        }
                        if let Err(rollback_error) = self
                            .store
                            .save_authoritative_projection_if_current_revision(
                                rollback_target,
                                meerkat_core::session_store::session_projection_cas_token(
                                    rollback_target,
                                )
                                .ok(),
                            )
                            .await
                        {
                            tracing::error!(
                                session_id = %rollback_target.id(),
                                error = %rollback_error,
                                "failed to roll back transcript rewrite projection after audit append failure"
                            );
                            rollback_failure =
                                Some(format!("projection rollback failed: {rollback_error}"));
                        }
                    }
                    if let Some(rollback_failure) = rollback_failure {
                        let quarantine_error = self
                            .quarantine_runtime_session_snapshot_if_current(
                                session.id(),
                                session_snapshot.as_slice(),
                            )
                            .await
                            .err();
                        if let Some(quarantine_error) = quarantine_error {
                            return Err(SessionError::Agent(
                                meerkat_core::error::AgentError::InternalError(format!(
                                    "transcript rewrite audit append failed ({error}); {rollback_failure}; runtime snapshot quarantine also failed: {quarantine_error}"
                                )),
                            ));
                        }
                        return Err(SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(format!(
                                "transcript rewrite audit append failed ({error}); {rollback_failure}; runtime snapshot was quarantined"
                            )),
                        ));
                    }
                    return Err(error);
                }
                last_audited_projection = Some(rewritten);
                persisted_revision = Some(commit.revision.clone());
            }
            let incoming_revision = meerkat_core::transcript_messages_digest(session.messages())
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to digest incoming transcript rewrite snapshot: {err}"
                    )))
                })?;
            let latest_session_snapshot = serde_json::to_vec(&session).map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to serialize latest runtime transcript rewrite snapshot: {err}"
                )))
            })?;
            if commits.last().map(|commit| commit.revision.as_str())
                != Some(incoming_revision.as_str())
            {
                runtime_store
                    .commit_session_snapshot(
                        &Self::runtime_id_for_session(session.id()),
                        SessionDelta {
                            session_snapshot: latest_session_snapshot.clone(),
                        },
                    )
                    .await
                    .map_err(|err| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("runtime post-rewrite snapshot persistence failed: {err}"),
                        ))
                    })?;
            }
            if let Err(error) =
                save_audited_authoritative_projection_after_persisted_continuity_guard(
                    self.store.as_ref(),
                    self.blob_store.as_ref(),
                    &session,
                )
                .await
            {
                return Err(self
                    .fail_closed_runtime_projection_update(
                        session.id(),
                        error,
                        Some(latest_session_snapshot.as_slice()),
                    )
                    .await);
            }
        } else {
            let incoming_revision = meerkat_core::transcript_messages_digest(session.messages())
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to digest incoming transcript rewrite snapshot: {err}"
                    )))
                })?;
            let mut last_audited_projection = previous.clone();
            let mut persisted_revision = previous
                .as_ref()
                .map(meerkat_core::Session::transcript_revision)
                .transpose()
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to read previous transcript revision for rewrite persistence: {err}"
                    )))
                })?;
            for commit in commits {
                if persisted_revision.as_deref() != Some(commit.parent_revision.as_str()) {
                    let bridge = session_materialized_at_transcript_revision(
                        &session,
                        &commit.parent_revision,
                    )?;
                    save_session_projection_with_storage_normalization_bridge(
                        self.store.as_ref(),
                        self.blob_store.as_ref(),
                        &bridge,
                    )
                    .await
                    .map_err(|err| SessionError::Store(Box::new(err)))?;
                }
                let rewritten =
                    session_materialized_at_transcript_revision(&session, &commit.revision)?;
                self.store
                    .save_transcript_rewrite(&rewritten, commit)
                    .await
                    .map_err(transcript_rewrite_store_error_to_session_error)?;
                let rewritten_projection_token =
                    meerkat_core::session_store::session_projection_cas_token(&rewritten)
                        .map_err(|err| SessionError::Store(Box::new(err)))?;
                if let Err(error) = append_transcript_rewrite_commit_events(
                    self.event_store.as_ref(),
                    self.projector.as_ref(),
                    &session,
                    std::slice::from_ref(commit),
                )
                .await
                {
                    if let Some(rollback_target) = last_audited_projection.as_ref()
                        && let Err(rollback_error) = self
                            .store
                            .save_authoritative_projection_if_current_revision(
                                rollback_target,
                                Some(rewritten_projection_token.clone()),
                            )
                            .await
                    {
                        tracing::error!(
                            session_id = %rollback_target.id(),
                            error = %rollback_error,
                            "failed to roll back transcript rewrite projection after audit append failure"
                        );
                        match self
                            .store
                            .delete_if_current_revision(session.id(), &rewritten_projection_token)
                            .await
                        {
                            Ok(true) => {
                                return Err(SessionError::Agent(
                                    meerkat_core::error::AgentError::InternalError(format!(
                                        "transcript rewrite audit append failed ({error}); projection rollback also failed: {rollback_error}; unaudited projection was quarantined"
                                    )),
                                ));
                            }
                            Ok(false) => {
                                return Err(SessionError::Agent(
                                    meerkat_core::error::AgentError::InternalError(format!(
                                        "transcript rewrite audit append failed ({error}); projection rollback also failed: {rollback_error}; projection changed before unaudited quarantine"
                                    )),
                                ));
                            }
                            Err(quarantine_error) => {
                                return Err(SessionError::Agent(
                                    meerkat_core::error::AgentError::InternalError(format!(
                                        "transcript rewrite audit append failed ({error}); projection rollback also failed: {rollback_error}; unaudited projection quarantine also failed: {quarantine_error}"
                                    )),
                                ));
                            }
                        }
                    }
                    return Err(error);
                }
                last_audited_projection = Some(rewritten);
                persisted_revision = Some(commit.revision.clone());
            }
            if commits.last().map(|commit| commit.revision.as_str())
                != Some(incoming_revision.as_str())
            {
                save_session_projection_after_verified_rewrite_chain(
                    self.store.as_ref(),
                    self.blob_store.as_ref(),
                    &session,
                    &incoming_revision,
                )
                .await
                .map_err(|err| SessionError::Store(Box::new(err)))?;
            }
        }
        if converge_live {
            self.converge_live_session_after_transcript_rewrite(&session)
                .await?;
        }
        Ok(session)
    }

    async fn converge_live_session_after_transcript_rewrite(
        &self,
        session: &Session,
    ) -> Result<(), SessionError> {
        if !self.inner.has_live_session(session.id()).await? {
            return Ok(());
        }

        match self
            .synchronize_live_session_from_durable(
                session.id(),
                session,
                "transcript_rewrite_committed",
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(error) if is_durable_session_sync_unsupported(&error) => {
                self.discard_live_session(session.id()).await
            }
            Err(error) => Err(error),
        }
    }

    async fn normalized_transcript_rewrite_replacement(
        &self,
        mut replacement: Vec<meerkat_core::Message>,
    ) -> Result<Vec<meerkat_core::Message>, SessionError> {
        externalize_messages_from(self.blob_store.as_ref(), &mut replacement, 0)
            .await
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to externalize transcript rewrite replacement media: {err}"
                )))
            })?;
        Ok(replacement)
    }

    pub async fn export_realtime_open_session_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Session, SessionError> {
        match self.live_session_authority(id).await? {
            LiveSessionAuthority::DurableAuthoritative { session, reason } => {
                if self
                    .session_archived_by_authority(id, session.as_ref())
                    .await?
                {
                    return Err(SessionError::NotFound { id: id.clone() });
                }
                if reason == "runtime_system_context_diverged" {
                    self.synchronize_live_runtime_context_state_from_durable(
                        id,
                        session.as_ref(),
                        reason,
                    )
                    .await?;
                } else {
                    self.synchronize_live_session_from_durable(id, session.as_ref(), reason)
                        .await?;
                }
                tracing::debug!(
                    session_id = %id,
                    reason,
                    "using durable session authority for realtime open snapshot"
                );
                Ok(*session)
            }
            LiveSessionAuthority::NoLive | LiveSessionAuthority::LiveAuthoritative => {
                self.export_session_with_labels(id).await
            }
        }
    }

    pub async fn wait_for_session_mutation_after(
        &self,
        id: &SessionId,
        after: std::time::SystemTime,
    ) -> Result<std::time::SystemTime, meerkat_core::comms::StreamError> {
        self.inner.wait_for_session_mutation_after(id, after).await
    }

    pub async fn execution_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::AgentExecutionSnapshot>, SessionError> {
        self.inner.execution_snapshot(id).await
    }

    pub async fn tool_scope_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, SessionError> {
        self.inner.tool_scope_snapshot(id).await
    }

    pub async fn live_visible_tool_defs(
        &self,
        id: &SessionId,
    ) -> Result<Vec<meerkat_core::ToolDef>, SessionError> {
        self.inner.live_visible_tool_defs(id).await
    }

    pub async fn external_tool_surface_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::ExternalToolSurfaceSnapshot>, SessionError> {
        self.inner.external_tool_surface_snapshot(id).await
    }

    pub async fn live_session_llm_identity(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::SessionLlmIdentity, SessionError> {
        self.inner.live_session_llm_identity(id).await
    }

    pub async fn discard_live_session(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.discard_live_session(id).await?;
        self.checkpointer_gates.lock().await.remove(id);
        Ok(())
    }

    pub async fn persist_live_session_now(&self, id: &SessionId) -> Result<usize, SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        self.persist_full_session(id)
            .await
            .map(|(message_count, _revision)| message_count)
    }

    pub async fn dispatch_external_tool_call(
        &self,
        id: &SessionId,
        call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, SessionError> {
        self.dispatch_external_tool_call_with_timeout_policy(
            id,
            call,
            meerkat_core::ToolDispatchTimeoutPolicy::Disabled,
        )
        .await
    }

    pub async fn dispatch_external_tool_call_with_timeout_policy(
        &self,
        id: &SessionId,
        call: meerkat_core::ToolCall,
        timeout_policy: meerkat_core::ToolDispatchTimeoutPolicy,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        let outcome = self
            .inner
            .dispatch_external_tool_call_with_timeout_policy(id, call, timeout_policy)
            .await?;
        if let Err(error) = self.persist_full_session(id).await {
            let _ = self.discard_live_session(id).await;
            return Err(error);
        }
        Ok(outcome)
    }

    pub async fn append_external_user_content(
        &self,
        id: &SessionId,
        content: meerkat_core::types::ContentInput,
    ) -> Result<(), SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        self.inner.append_external_user_content(id, content).await?;
        if let Err(error) = self.persist_full_session(id).await {
            let _ = self.discard_live_session(id).await;
            return Err(error);
        }
        Ok(())
    }

    pub async fn append_external_assistant_output(
        &self,
        id: &SessionId,
        blocks: Vec<meerkat_core::types::AssistantBlock>,
        stop_reason: meerkat_core::types::StopReason,
        usage: meerkat_core::types::Usage,
    ) -> Result<(), SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        self.inner
            .append_external_assistant_output(id, blocks, stop_reason, usage)
            .await?;
        if let Err(error) = self.persist_full_session(id).await {
            let _ = self.discard_live_session(id).await;
            return Err(error);
        }
        Ok(())
    }

    pub async fn append_realtime_transcript_event(
        &self,
        id: &SessionId,
        event: meerkat_core::RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::RealtimeTranscriptApplyOutcome, SessionError> {
        let _mutation_guard = self.realtime_transcript_mutation_guard(id).await?;
        let outcome = self
            .inner
            .append_realtime_transcript_event(id, event)
            .await?;
        if let Err(error) = self.persist_full_session(id).await {
            let _ = self.discard_live_session(id).await;
            return Err(error);
        }
        Ok(outcome)
    }

    /// Create a new persistent session service.
    pub fn new(
        builder: B,
        max_sessions: usize,
        store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        Self::new_with_capacities(builder, max_sessions, 0, store, runtime_store, blob_store)
    }

    /// Create a persistent session service.
    ///
    /// The archived-history capacity parameter is retained for API
    /// compatibility, but archived history now reloads from durable authority
    /// instead of a process-local cache.
    pub fn new_with_archived_history_capacity(
        builder: B,
        max_sessions: usize,
        _archived_history_capacity: usize,
        store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        Self::new_with_capacities(builder, max_sessions, 0, store, runtime_store, blob_store)
    }

    /// Create a persistent session service with explicit active-session
    /// capacity. The archived-history capacity parameter is ignored because
    /// archived history is loaded from durable authority.
    pub fn new_with_capacities(
        builder: B,
        active_session_capacity: usize,
        _archived_history_capacity: usize,
        store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        Self {
            inner: EphemeralSessionService::new(builder, active_session_capacity),
            store,
            runtime_store,
            blob_store,
            event_store: None,
            projector: None,
            checkpointer_gates: Mutex::new(HashMap::new()),
            recovery_gates: Mutex::new(HashMap::new()),
            quarantined_runtime_projection_fallbacks: Mutex::new(HashSet::new()),
        }
    }

    /// Attach the append-only event log and derived file projector used by
    /// persistent sessions.
    ///
    /// Projection is best-effort derived state. Append/project failures are
    /// logged and do not fail the session turn; callers must continue to treat
    /// `SessionStore`/`RuntimeStore` as the source of truth, not the projected
    /// JSONL files. The spawned projection tasks exit when their session event
    /// streams close, and create-time events without a correlated session id are
    /// discarded when the create-time stream closes.
    pub fn with_event_projection(
        mut self,
        event_store: Arc<dyn EventStore>,
        projector: Arc<SessionProjector>,
    ) -> Self {
        self.event_store = Some(event_store);
        self.projector = Some(projector);
        self
    }

    pub fn runtime_mode(&self) -> RuntimeMode {
        RuntimeMode::V9Compliant
    }

    pub fn runtime_store(&self) -> Option<Arc<dyn RuntimeStore>> {
        self.runtime_store.clone()
    }

    pub async fn persisted_runtime_state(
        &self,
        id: &SessionId,
    ) -> Result<Option<RuntimeState>, SessionError> {
        let Some(runtime_store) = self.runtime_store.as_ref() else {
            return Ok(None);
        };
        Self::load_runtime_state_for_session(runtime_store, id).await
    }

    pub fn has_event_projection(&self) -> bool {
        self.event_store.is_some() && self.projector.is_some()
    }

    pub fn ensure_active_capacity_available(&self) -> Result<(), SessionError> {
        self.inner.ensure_active_capacity_available()
    }

    pub async fn reserve_create_session_admission(
        &self,
    ) -> Result<crate::ephemeral::RuntimeContextAdmissionGuard, SessionError> {
        self.inner.acquire_runtime_capacity_admission().await
    }

    pub async fn reserve_runtime_turn_admission(
        &self,
        id: &SessionId,
    ) -> Result<crate::ephemeral::RuntimeContextAdmissionGuard, SessionError> {
        match self.inner.join_active_runtime_context_admission(id).await {
            Ok(Some(admission)) => return Ok(admission),
            Ok(None) | Err(SessionError::NotFound { .. }) => {}
            Err(error) => return Err(error),
        }

        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _turn_guard = recovery_gate.lock().await;
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        if let Some(session) = self.load_authoritative_session_base(id).await? {
            self.reject_if_archived_session(id, &session)
                .await
                .map_err(control_error_into_session_error)?;
        }
        match self.inner.acquire_runtime_context_admission(id).await {
            Ok(admission) => Ok(admission),
            Err(SessionError::NotFound { .. }) => {
                if self.load_authoritative_session_base(id).await?.is_some() {
                    self.inner.acquire_runtime_capacity_admission().await
                } else {
                    Err(SessionError::NotFound { id: id.clone() })
                }
            }
            Err(error) => Err(error),
        }
    }

    pub async fn create_session_with_reserved_admission(
        &self,
        req: CreateSessionRequest,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<RunResult, SessionError> {
        self.create_session_with_admission(
            req,
            Some(admission),
            ArchivedResumeAuthorization::RejectArchived,
        )
        .await
    }

    pub async fn create_session_with_reserved_machine_archived_resume_admission(
        &self,
        req: CreateSessionRequest,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
        authority: MachineSessionControlAuthority,
    ) -> Result<RunResult, SessionError> {
        self.create_session_with_admission(
            req,
            Some(admission),
            ArchivedResumeAuthorization::MachinePendingPromotion {
                _authority: authority,
            },
        )
        .await
    }

    pub async fn start_turn_with_reserved_admission(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<RunResult, SessionError> {
        self.reject_runtime_backed_direct_start_turn()?;
        self.start_turn_with_recoverable_reserved_admission(id, req, admission)
            .await
            .map_err(|(error, _admission)| error)
    }

    pub async fn start_turn_with_recoverable_reserved_admission(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<
        RunResult,
        (
            SessionError,
            Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
        ),
    > {
        if let Err(error) = self.reject_runtime_backed_direct_start_turn() {
            return Err((error, Some(admission)));
        }
        self.start_turn_inner_with_admission(
            id,
            req,
            Some(admission),
            DirectStartTurnPersistence::RuntimelessImmediate,
        )
        .await
    }

    pub async fn run_machine_committed_live_turn(
        &self,
        protocol: MachineServiceTurnCommitProtocol<'_>,
        id: &SessionId,
        req: StartTurnRequest,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<
        RunResult,
        (
            SessionError,
            Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
        ),
    > {
        if let Err(error) = self.validate_machine_service_turn_protocol(protocol) {
            return Err((error, Some(admission)));
        }

        let result = self
            .start_turn_inner_with_admission(
                id,
                req,
                Some(admission),
                DirectStartTurnPersistence::MachineCommitted(protocol),
            )
            .await;
        match result {
            Ok(result) => Ok(result),
            Err((error, admission))
                if self
                    .service_turn_error_requires_machine_terminal_receipt(id, &error)
                    .await =>
            {
                Err((error, admission))
            }
            Err((error, admission)) => {
                if !matches!(error, SessionError::NotFound { .. }) {
                    let _ = self.discard_live_session(id).await;
                }
                Err((error, admission))
            }
        }
    }

    fn validate_machine_service_turn_protocol(
        &self,
        protocol: MachineServiceTurnCommitProtocol<'_>,
    ) -> Result<(), SessionError> {
        match self.runtime_store.as_ref() {
            Some(runtime_store) => {
                if protocol
                    .runtime_adapter
                    .shares_runtime_store_authority(runtime_store)
                {
                    return Ok(());
                }
                Err(SessionError::Unsupported(
                    "machine service-turn commit protocol runtime authority does not match the session service runtime store"
                        .to_string(),
                ))
            }
            None if !protocol.runtime_adapter.has_runtime_persistence() => Ok(()),
            None => Err(SessionError::Unsupported(
                "persistent machine service-turn commit protocol requires a runtime-backed session service"
                    .to_string(),
            )),
        }
    }

    pub async fn service_turn_error_requires_machine_terminal_receipt(
        &self,
        id: &SessionId,
        error: &SessionError,
    ) -> bool {
        if Self::callback_pending_terminal(error).is_some() {
            return true;
        }
        self.execution_snapshot(id)
            .await
            .ok()
            .flatten()
            .is_some_and(|snapshot| snapshot.turn_phase.is_terminal())
    }

    fn reject_runtime_backed_direct_start_turn(&self) -> Result<(), SessionError> {
        if self.runtime_store.is_some() {
            return Err(SessionError::Unsupported(
                "runtime-backed direct start_turn must route through the MeerkatMachine service-turn commit protocol"
                    .to_string(),
            ));
        }
        Ok(())
    }

    fn reject_runtime_backed_eager_create_session(
        &self,
        req: &CreateSessionRequest,
    ) -> Result<(), SessionError> {
        let runtime_backed = self.runtime_store.is_some()
            || req.build.as_ref().is_some_and(|build| {
                matches!(
                    &build.runtime_build_mode,
                    meerkat_core::RuntimeBuildMode::SessionOwned(_)
                )
            });
        if runtime_backed && req.initial_turn == InitialTurnPolicy::RunImmediately {
            return Err(SessionError::Unsupported(
                "runtime-backed eager create_session must route through the MeerkatMachine service-turn commit protocol"
                    .to_string(),
            ));
        }
        Ok(())
    }

    async fn start_turn_inner_with_admission(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        mut admission: Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
        persistence: DirectStartTurnPersistence<'_>,
    ) -> Result<
        RunResult,
        (
            SessionError,
            Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
        ),
    > {
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _turn_guard = recovery_gate.lock().await;
        let _ = self
            .discard_stale_live_session_if_needed(id)
            .await
            .map_err(|error| (error, admission.take()))?;
        let _ = self
            .recover_live_session_from_store_if_needed_locked(id)
            .await
            .map_err(|error| (error, admission.take()))?;
        let result = match admission.take() {
            Some(admission) => {
                self.inner
                    .start_turn_with_runtime_context_admission_recovering_not_found(
                        id, req, admission,
                    )
                    .await
            }
            None => self
                .inner
                .start_turn(id, req)
                .await
                .map_err(|error| (error, None)),
        };
        let result = match result {
            Ok(result) => result,
            Err((error, admission)) => {
                if matches!(
                    persistence,
                    DirectStartTurnPersistence::RuntimelessImmediate
                ) && Self::callback_pending_terminal(&error).is_some()
                    && let Err(persist_error) = self.persist_full_session_or_discard_live(id).await
                {
                    return Err((persist_error, admission));
                }
                if let DirectStartTurnPersistence::MachineCommitted(protocol) = persistence
                    && self
                        .service_turn_error_requires_machine_terminal_receipt(id, &error)
                        .await
                {
                    if let Err(commit_error) = protocol
                        .runtime_adapter
                        .commit_service_turn_terminal_receipt(id)
                        .await
                    {
                        let _ = self.discard_live_session(id).await;
                        return Err((
                            runtime_driver_error_to_session_error(commit_error),
                            admission,
                        ));
                    }
                    if let Err(persist_error) = self.persist_full_session_or_discard_live(id).await
                    {
                        return Err((persist_error, admission));
                    }
                }
                return Err((error, admission));
            }
        };

        match persistence {
            DirectStartTurnPersistence::RuntimelessImmediate => {
                let _ = self
                    .persist_full_session_or_discard_live(id)
                    .await
                    .map_err(|error| (error, None))?;
            }
            DirectStartTurnPersistence::MachineCommitted(protocol) => {
                if let Err(error) = protocol
                    .runtime_adapter
                    .commit_service_turn_terminal_receipt(id)
                    .await
                {
                    let _ = self.discard_live_session(id).await;
                    return Err((runtime_driver_error_to_session_error(error), None));
                }
                self.persist_full_session_or_discard_live(id)
                    .await
                    .map_err(|error| (error, None))?;
            }
        }

        Ok(result)
    }

    async fn legacy_start_turn_inner_with_admission(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        admission: Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
    ) -> Result<
        RunResult,
        (
            SessionError,
            Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
        ),
    > {
        self.start_turn_inner_with_admission(
            id,
            req,
            admission,
            DirectStartTurnPersistence::RuntimelessImmediate,
        )
        .await
    }

    pub async fn event_log_latest_seq(&self, id: &SessionId) -> Result<Option<u64>, SessionError> {
        let Some(event_store) = self.event_store.as_ref() else {
            return Ok(None);
        };
        event_store
            .last_seq(id)
            .await
            .map(Some)
            .map_err(|err| SessionError::Store(Box::new(err)))
    }

    pub async fn event_log_read_from(
        &self,
        id: &SessionId,
        from_seq: u64,
    ) -> Result<Option<Vec<crate::event_store::StoredEvent>>, SessionError> {
        let Some(event_store) = self.event_store.as_ref() else {
            return Ok(None);
        };
        event_store
            .read_from(id, from_seq)
            .await
            .map(Some)
            .map_err(|err| SessionError::Store(Box::new(err)))
    }

    pub fn blob_store(&self) -> Arc<dyn BlobStore> {
        self.blob_store.clone()
    }

    fn install_create_time_event_projection(
        &self,
        req: &mut CreateSessionRequest,
    ) -> Option<watch::Sender<Option<SessionId>>> {
        let (Some(event_store), Some(projector)) =
            (self.event_store.clone(), self.projector.clone())
        else {
            return None;
        };

        let caller_event_tx = req.event_tx.take();
        let (projection_tx, projection_rx) = mpsc::channel(128);
        let (session_tx, session_rx) = watch::channel(None);
        req.event_tx = Some(projection_tx);

        tokio::spawn(project_create_time_events(
            event_store,
            projector,
            projection_rx,
            session_rx,
            caller_event_tx,
        ));

        Some(session_tx)
    }

    async fn spawn_event_projection_task(&self, id: &SessionId) {
        let (Some(event_store), Some(projector)) =
            (self.event_store.clone(), self.projector.clone())
        else {
            return;
        };
        let session_id = id.clone();
        let stream = self.inner.subscribe_session_events(&session_id).await;
        let Ok(mut stream) = stream else {
            return;
        };

        tokio::spawn(async move {
            while let Some(envelope) = stream.next().await {
                append_and_project_event(&event_store, &projector, &session_id, envelope.payload)
                    .await;
            }
        });
    }

    async fn normalized_session_for_persistence(
        &self,
        mut session: Session,
    ) -> Result<Session, SessionError> {
        session
            .externalize_media(self.blob_store.as_ref(), 0)
            .await
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to externalize session images for persistence: {err}"
                )))
            })?;
        if let Some(mut state) = session.deferred_turn_state() {
            externalize_deferred_turn_state(self.blob_store.as_ref(), &mut state)
                .await
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to externalize deferred-turn images for persistence: {err}"
                    )))
                })?;
            session.set_deferred_turn_state(state).map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to serialize deferred-turn state for persistence: {err}"
                )))
            })?;
        }
        Ok(session)
    }

    async fn save_normalized_session(&self, session: Session) -> Result<Session, SessionError> {
        let session = self.normalized_session_for_persistence(session).await?;
        let commits = self
            .transcript_rewrite_commit_chain_for_persistence(&session)
            .await?;
        if !commits.is_empty() {
            return self
                .persist_normalized_transcript_rewrite_chain(session, &commits, false)
                .await;
        }
        if let Some(runtime_store) = self.runtime_store.as_ref() {
            if let Err(error) = verify_authoritative_projection_persisted_continuity(
                self.store.as_ref(),
                self.blob_store.as_ref(),
                &session,
            )
            .await
            {
                return Err(self
                    .fail_closed_runtime_projection_preflight(session.id(), error)
                    .await);
            }
            let session_snapshot = serde_json::to_vec(&session).map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to serialize session snapshot for runtime persistence: {err}"
                )))
            })?;
            runtime_store
                .commit_session_snapshot(
                    &Self::runtime_id_for_session(session.id()),
                    SessionDelta {
                        session_snapshot: session_snapshot.clone(),
                    },
                )
                .await
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "runtime snapshot persistence failed: {err}"
                    )))
                })?;
            if let Err(error) = save_session_projection_with_storage_normalization_bridge(
                self.store.as_ref(),
                self.blob_store.as_ref(),
                &session,
            )
            .await
            {
                return Err(self
                    .fail_closed_runtime_projection_update(
                        session.id(),
                        error,
                        Some(session_snapshot.as_slice()),
                    )
                    .await);
            }
        } else {
            save_session_projection_with_storage_normalization_bridge(
                self.store.as_ref(),
                self.blob_store.as_ref(),
                &session,
            )
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;
        }
        Ok(session)
    }

    async fn save_compatibility_projection_only(
        &self,
        session: Session,
    ) -> Result<Session, SessionError> {
        let session = self.normalized_session_for_persistence(session).await?;
        self.store
            .save(&session)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;
        Ok(session)
    }

    async fn transcript_rewrite_commit_chain_for_persistence(
        &self,
        session: &Session,
    ) -> Result<Vec<meerkat_core::TranscriptRewriteCommit>, SessionError> {
        let previous = if let Some(runtime_store) = self.runtime_store.as_ref() {
            Self::load_runtime_session_snapshot_for_session(runtime_store, session.id()).await?
        } else {
            self.store
                .load(session.id())
                .await
                .map_err(|e| SessionError::Store(Box::new(e)))?
        };
        let Some(previous) = previous else {
            return Ok(Vec::new());
        };
        let previous_revision = meerkat_core::transcript_messages_digest(previous.messages())
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to digest previous transcript for persistence: {err}"
                )))
            })?;
        let incoming_revision = meerkat_core::transcript_messages_digest(session.messages())
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to digest incoming transcript for persistence: {err}"
                )))
            })?;
        let incoming_state = session.transcript_history_state().map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to read transcript history for persistence: {err}"
            )))
        })?;
        if previous_revision == incoming_revision
            || Self::incoming_extends_previous_transcript(&previous, session, &previous_revision)?
        {
            let Some(state) = incoming_state.as_ref() else {
                return Ok(Vec::new());
            };
            if transcript_rewrite_commit_metadata_preserved(&previous, state)
                .map_err(|err| SessionError::Store(Box::new(err)))?
            {
                return Ok(Vec::new());
            }
        }

        let Some(state) = incoming_state else {
            return Ok(Vec::new());
        };
        let mut commits =
            find_transcript_rewrite_commit_chain_extending_session_with_storage_normalization(
                self.blob_store.as_ref(),
                &state,
                &previous,
                &incoming_revision,
                session.messages(),
            )
            .await
            .map_err(|err| SessionError::Store(Box::new(err)))?;
        if commits.is_none() {
            let mut normalized_previous = previous.clone();
            normalized_previous
                .externalize_media(self.blob_store.as_ref(), 0)
                .await
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to externalize previous persisted projection for rewrite chain discovery: {err}"
                    )))
                })?;
            let normalized_revision = meerkat_core::transcript_messages_digest(
                normalized_previous.messages(),
            )
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to digest normalized previous transcript for persistence: {err}"
                )))
            })?;
            if normalized_revision != previous_revision {
                commits = find_transcript_rewrite_commit_chain_extending_session_with_storage_normalization(
                    self.blob_store.as_ref(),
                    &state,
                    &normalized_previous,
                    &incoming_revision,
                    session.messages(),
                )
                .await
                .map_err(|err| SessionError::Store(Box::new(err)))?;
            }
        }
        Ok(commits.unwrap_or_default().into_iter().cloned().collect())
    }

    fn incoming_extends_previous_transcript(
        previous: &Session,
        incoming: &Session,
        previous_revision: &str,
    ) -> Result<bool, SessionError> {
        if incoming.messages().len() < previous.messages().len() {
            return Ok(false);
        }
        let prefix_revision = meerkat_core::transcript_messages_digest(
            &incoming.messages()[..previous.messages().len()],
        )
        .map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to digest incoming transcript prefix for persistence: {err}"
            )))
        })?;
        Ok(prefix_revision == previous_revision)
    }

    pub async fn checkpoint_committed_runtime_session_snapshot(
        &self,
        id: &SessionId,
        session_snapshot: &[u8],
    ) -> Result<(), SessionError> {
        if self.runtime_store.is_none() {
            return Ok(());
        }

        let session: Session = serde_json::from_slice(session_snapshot).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to deserialize committed runtime session snapshot: {err}"
            )))
        })?;
        if session.id() != id {
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "committed runtime session snapshot id mismatch: expected {id}, got {}",
                    session.id()
                )),
            ));
        }

        let _projection_guard = self.recovery_gate_for_session(id).await.lock_owned().await;
        let gate = self.gate_for_session(id).await;
        let guard = gate.cancelled.lock().await;
        if *guard {
            return Ok(());
        }
        if let Err(error) = save_session_projection_allowing_internal_rewrite(
            self.store.as_ref(),
            self.blob_store.as_ref(),
            self.event_store.as_ref(),
            self.projector.as_ref(),
            &session,
        )
        .await
        {
            drop(guard);
            return match error {
                SessionError::Store(store_error) => {
                    let store_error = SessionStoreError::Internal(store_error.to_string());
                    Err(self
                        .quarantine_failed_runtime_projection_update(
                            session.id(),
                            store_error,
                            session_snapshot,
                        )
                        .await)
                }
                other => {
                    let store_error = SessionStoreError::Internal(other.to_string());
                    Err(self
                        .quarantine_failed_runtime_projection_update(
                            session.id(),
                            store_error,
                            session_snapshot,
                        )
                        .await)
                }
            };
        }
        Ok(())
    }

    pub async fn stage_live_system_context_boundary_snapshot(
        &self,
        id: &SessionId,
        expected_run_id: &RunId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<Option<Vec<u8>>, SessionError> {
        if appends.is_empty() {
            return Ok(None);
        }

        let turn_state_handle = self.inner.active_turn_state_handle(id).await?;
        let turn_state_handle =
            turn_state_handle.ok_or_else(|| SessionError::NotRunning { id: id.clone() })?;
        let initial_token = EphemeralSessionService::<B>::active_turn_boundary_staging_token(
            turn_state_handle.as_ref(),
        )
        .ok_or_else(|| SessionError::NotRunning { id: id.clone() })?;
        if initial_token.active_run_id != *expected_run_id {
            return Err(SessionError::NotRunning { id: id.clone() });
        }

        let state_handle = self
            .inner
            .system_context_state(id)
            .await
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;

        let locked_token = EphemeralSessionService::<B>::active_turn_boundary_staging_token(
            turn_state_handle.as_ref(),
        )
        .ok_or_else(|| SessionError::NotRunning { id: id.clone() })?;
        if locked_token != initial_token {
            return Err(SessionError::NotRunning { id: id.clone() });
        }
        let stage_inputs = appends
            .into_iter()
            .map(|append| {
                (
                    AppendSystemContextRequest {
                        text: append.text,
                        source: append.source,
                        idempotency_key: append.idempotency_key,
                    },
                    append.accepted_at,
                )
            })
            .collect::<Vec<_>>();
        let (snapshot_state, staged_state) = state_handle
            .stage_active_turn_appends_with_snapshot(stage_inputs)
            .map_err(|err| control_error_into_session_error(err.into_control_error(id)))?;
        let staged_token = EphemeralSessionService::<B>::active_turn_boundary_staging_token(
            turn_state_handle.as_ref(),
        )
        .ok_or_else(|| SessionError::NotRunning { id: id.clone() })?;
        if staged_token != initial_token {
            state_handle
                .replace_from_generated_restore_if_current(&staged_state, snapshot_state.clone())
                .map_err(|err| {
                    SessionError::Agent(AgentError::InternalError(format!(
                        "failed to roll back stale live boundary context: {err}"
                    )))
                })?;
            return Err(SessionError::NotRunning { id: id.clone() });
        }
        tracing::debug!(
            session_id = %id,
            pending_count = staged_state.pending_len(),
            applied_count = staged_state.applied_len(),
            active_turn_pending_count = staged_state.active_turn_pending_len(),
            "staged live active-turn runtime system context"
        );

        let snapshot = async {
            let mut session = self
                .load_authoritative_session_base(id)
                .await?
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            self.reject_if_archived_session(id, &session)
                .await
                .map_err(control_error_into_session_error)?;
            write_system_context_state(&mut session, staged_state.clone())
                .map_err(control_error_into_session_error)?;
            let session = self.normalized_session_for_persistence(session).await?;
            serde_json::to_vec(&session).map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to serialize live boundary context session snapshot: {err}"
                )))
            })
        }
        .await;

        match snapshot {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(error) => {
                let rolled_back = state_handle
                    .replace_from_generated_restore_if_current(&staged_state, snapshot_state)
                    .map_err(|err| {
                        SessionError::Agent(AgentError::InternalError(format!(
                            "failed to roll back live boundary context: {err}"
                        )))
                    })?;
                if !rolled_back {
                    tracing::warn!(
                        session_id = %id,
                        "live system-context state diverged after failed boundary snapshot; leaving newer live state intact"
                    );
                }
                Err(error)
            }
        }
    }

    pub async fn active_turn_system_context_boundary_available(
        &self,
        id: &SessionId,
    ) -> Result<Option<bool>, SessionError> {
        self.inner
            .active_turn_system_context_boundary_available(id)
            .await
    }

    pub async fn discard_live_system_context_boundary_staging(
        &self,
        id: &SessionId,
        expected_run_id: &RunId,
        idempotency_keys: Vec<String>,
    ) -> Result<usize, SessionError> {
        self.inner
            .discard_runtime_system_context_for_active_turn(id, expected_run_id, idempotency_keys)
            .await
    }

    async fn fail_closed_runtime_projection_update(
        &self,
        id: &SessionId,
        error: SessionStoreError,
        _rejected_snapshot: Option<&[u8]>,
    ) -> SessionError {
        tracing::error!(
            session_id = %id,
            error = %error,
            "session-store projection update failed after runtime authority commit; failing closed"
        );
        match self.discard_live_session(id).await {
            Ok(()) | Err(SessionError::NotFound { .. }) => {}
            Err(discard_error) => {
                tracing::warn!(
                    session_id = %id,
                    error = %discard_error,
                    "failed to discard live session after runtime-backed projection update failure"
                );
            }
        }
        SessionError::Store(Box::new(error))
    }

    async fn quarantine_failed_runtime_projection_update(
        &self,
        id: &SessionId,
        error: SessionStoreError,
        rejected_snapshot: &[u8],
    ) -> SessionError {
        tracing::error!(
            session_id = %id,
            error = %error,
            "session-store projection update failed after committed runtime checkpoint; quarantining rejected runtime snapshot"
        );
        let quarantine_error = self
            .quarantine_runtime_session_snapshot_if_current(id, rejected_snapshot)
            .await
            .err();
        match self.discard_live_session(id).await {
            Ok(()) | Err(SessionError::NotFound { .. }) => {}
            Err(discard_error) => {
                tracing::warn!(
                    session_id = %id,
                    error = %discard_error,
                    "failed to discard live session after runtime checkpoint projection failure"
                );
            }
        }
        if let Some(quarantine_error) = quarantine_error {
            return SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "session-store projection update failed ({error}); rejected runtime snapshot quarantine also failed: {quarantine_error}"
            )));
        }
        SessionError::Store(Box::new(error))
    }

    async fn quarantine_runtime_session_snapshot_if_current(
        &self,
        id: &SessionId,
        rejected_snapshot: &[u8],
    ) -> Result<(), SessionError> {
        let Some(runtime_store) = self.runtime_store.as_ref() else {
            return Ok(());
        };
        let cleared = runtime_store
            .clear_session_snapshot_if_current(&Self::runtime_id_for_session(id), rejected_snapshot)
            .await
            .map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to quarantine rejected runtime session snapshot: {error}"
                )))
            })?;
        if cleared {
            self.quarantined_runtime_projection_fallbacks
                .lock()
                .await
                .insert(id.clone());
        } else {
            tracing::warn!(
                session_id = %id,
                "runtime snapshot changed before fail-closed quarantine; leaving newer runtime authority intact"
            );
        }
        Ok(())
    }

    async fn runtime_projection_fallback_quarantined(&self, id: &SessionId) -> bool {
        self.quarantined_runtime_projection_fallbacks
            .lock()
            .await
            .contains(id)
    }

    async fn fail_closed_runtime_projection_preflight(
        &self,
        id: &SessionId,
        error: SessionStoreError,
    ) -> SessionError {
        tracing::error!(
            session_id = %id,
            error = %error,
            "session-store projection continuity preflight failed before runtime authority commit; failing closed"
        );
        match self.discard_live_session(id).await {
            Ok(()) | Err(SessionError::NotFound { .. }) => {}
            Err(discard_error) => {
                tracing::warn!(
                    session_id = %id,
                    error = %discard_error,
                    "failed to discard live session after runtime-backed projection preflight failure"
                );
            }
        }
        SessionError::Store(Box::new(error))
    }

    async fn gate_for_session(&self, id: &SessionId) -> Arc<CheckpointerGate> {
        let mut gates = self.checkpointer_gates.lock().await;
        Arc::clone(gates.entry(id.clone()).or_insert_with(|| {
            Arc::new(CheckpointerGate {
                cancelled: Mutex::new(false),
            })
        }))
    }

    async fn existing_gate_for_session(&self, id: &SessionId) -> Option<Arc<CheckpointerGate>> {
        let gates = self.checkpointer_gates.lock().await;
        gates.get(id).cloned()
    }

    async fn recovery_gate_for_session(&self, id: &SessionId) -> Arc<Mutex<()>> {
        let mut gates = self.recovery_gates.lock().await;
        Arc::clone(
            gates
                .entry(id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    async fn live_persist_mutation_guard(
        &self,
        id: &SessionId,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, SessionError> {
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let guard = recovery_gate.lock_owned().await;
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        if let Some(session) = self.load_authoritative_session_base(id).await? {
            self.reject_if_archived_session(id, &session)
                .await
                .map_err(control_error_into_session_error)?;
        }
        Ok(guard)
    }

    async fn realtime_transcript_mutation_guard(
        &self,
        id: &SessionId,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, SessionError> {
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let guard = recovery_gate.lock_owned().await;
        match self.live_session_authority(id).await? {
            LiveSessionAuthority::DurableAuthoritative { session, reason }
                if reason == "runtime_system_context_diverged" =>
            {
                if self
                    .session_archived_by_authority(id, session.as_ref())
                    .await?
                {
                    return Err(SessionError::NotFound { id: id.clone() });
                }
                self.synchronize_live_runtime_context_state_from_durable(
                    id,
                    session.as_ref(),
                    reason,
                )
                .await?;
            }
            LiveSessionAuthority::DurableAuthoritative { session, reason } => {
                if self
                    .session_archived_by_authority(id, session.as_ref())
                    .await?
                {
                    return Err(SessionError::NotFound { id: id.clone() });
                }
                self.synchronize_live_session_from_durable(id, session.as_ref(), reason)
                    .await?;
            }
            LiveSessionAuthority::NoLive => {
                tracing::debug!(
                    session_id = %id,
                    "realtime transcript append found no live session before mutation"
                );
            }
            LiveSessionAuthority::LiveAuthoritative => {}
        }
        if let Some(session) = self.load_authoritative_session_base(id).await? {
            self.reject_if_archived_session(id, &session)
                .await
                .map_err(control_error_into_session_error)?;
        }
        Ok(guard)
    }

    fn build_runtime_receipt(
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        session: &Session,
    ) -> Result<RunBoundaryReceipt, SessionError> {
        let encoded_messages = serde_json::to_vec(session.messages()).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize session for runtime receipt digest: {err}"
            )))
        })?;
        let digest = format!("{:x}", Sha256::digest(encoded_messages));

        Ok(RunBoundaryReceipt {
            run_id,
            boundary,
            contributing_input_ids,
            conversation_digest: Some(digest),
            message_count: session.messages().len(),
            sequence: 0,
        })
    }

    fn callback_pending_terminal(error: &SessionError) -> Option<CoreApplyTerminal> {
        match error {
            SessionError::Agent(AgentError::CallbackPending { tool_name, args }) => {
                Some(CoreApplyTerminal::CallbackPending {
                    tool_name: tool_name.clone(),
                    args: args.clone(),
                })
            }
            _ => None,
        }
    }

    async fn build_runtime_output(
        &self,
        id: &SessionId,
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        terminal: Option<CoreApplyTerminal>,
        committed_context_events: Vec<PendingSystemContextAppend>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let session = self.export_session_with_labels(id).await?;
        let persisted_session = self
            .normalized_session_for_persistence(session.clone())
            .await?;
        let session_snapshot = serde_json::to_vec(&persisted_session).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize session snapshot for runtime commit: {err}"
            )))
        })?;

        let receipt = self
            .commit_runtime_apply(
                id,
                run_id,
                boundary,
                &persisted_session,
                &contributing_input_ids,
            )
            .await?;

        if !committed_context_events.is_empty()
            && let Err(error) = self
                .inner
                .publish_runtime_system_context_events(id, committed_context_events)
                .await
        {
            tracing::warn!(
                session_id = %id,
                error = %error,
                "failed to publish committed runtime system-context lifecycle events"
            );
        }

        let output = match terminal {
            Some(CoreApplyTerminal::RunResult(run_result)) => {
                CoreApplyOutput::with_run_result(receipt, Some(session_snapshot), *run_result)
            }
            Some(CoreApplyTerminal::CallbackPending { tool_name, args }) => {
                CoreApplyOutput::with_callback_pending(
                    receipt,
                    Some(session_snapshot),
                    tool_name,
                    args,
                )
            }
            Some(CoreApplyTerminal::NoPendingBoundary) => CoreApplyOutput {
                receipt,
                session_snapshot: Some(session_snapshot),
                terminal: Some(CoreApplyTerminal::NoPendingBoundary),
            },
            None => CoreApplyOutput::without_terminal(receipt, Some(session_snapshot)),
        };

        Ok(output)
    }

    async fn build_runtime_output_after_live_mutation(
        &self,
        id: &SessionId,
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        terminal: Option<CoreApplyTerminal>,
        committed_context_events: Vec<PendingSystemContextAppend>,
    ) -> Result<CoreApplyOutput, SessionError> {
        match self
            .build_runtime_output(
                id,
                run_id,
                boundary,
                contributing_input_ids,
                terminal,
                committed_context_events,
            )
            .await
        {
            Ok(output) => Ok(output),
            Err(error) => {
                if let Err(discard_error) = self.discard_live_session(id).await {
                    tracing::warn!(
                        session_id = %id,
                        error = %discard_error,
                        "failed to discard live session after runtime output build failure"
                    );
                }
                Err(error)
            }
        }
    }

    async fn build_runtime_context_output(
        &self,
        id: &SessionId,
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        committed_context_events: Vec<PendingSystemContextAppend>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let session = self.export_session_with_labels(id).await?;
        let persisted_session = self
            .normalized_session_for_persistence(session.clone())
            .await?;
        let session_snapshot = serde_json::to_vec(&persisted_session).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize session snapshot for runtime commit: {err}"
            )))
        })?;

        let receipt = self
            .commit_runtime_apply(
                id,
                run_id,
                boundary,
                &persisted_session,
                &contributing_input_ids,
            )
            .await?;

        if !committed_context_events.is_empty()
            && let Err(error) = self
                .inner
                .publish_runtime_system_context_events(id, committed_context_events)
                .await
        {
            tracing::warn!(
                session_id = %id,
                error = %error,
                "failed to publish committed runtime system-context lifecycle events"
            );
        }

        Ok(CoreApplyOutput::without_terminal(
            receipt,
            Some(session_snapshot),
        ))
    }

    async fn build_runtime_context_output_after_live_mutation(
        &self,
        id: &SessionId,
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        committed_context_events: Vec<PendingSystemContextAppend>,
    ) -> Result<CoreApplyOutput, SessionError> {
        match self
            .build_runtime_context_output(
                id,
                run_id,
                boundary,
                contributing_input_ids,
                committed_context_events,
            )
            .await
        {
            Ok(output) => Ok(output),
            Err(error) => {
                if let Err(discard_error) = self.discard_live_session(id).await {
                    tracing::warn!(
                        session_id = %id,
                        error = %discard_error,
                        "failed to discard live session after runtime context output build failure"
                    );
                }
                Err(error)
            }
        }
    }

    /// Apply a runtime-driven turn and return the boundary receipt plus the
    /// session snapshot that the runtime machine commits atomically.
    pub async fn apply_runtime_turn(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_turn_outcome(id, run_id, req, boundary, contributing_input_ids)
            .await
    }

    pub async fn apply_runtime_turn_outcome(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_turn_outcome_with_admission(
            id,
            run_id,
            req,
            boundary,
            contributing_input_ids,
            None,
        )
        .await
    }

    pub async fn apply_runtime_turn_with_reserved_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_turn_with_recoverable_reserved_admission(
            id,
            run_id,
            req,
            boundary,
            contributing_input_ids,
            admission,
        )
        .await
        .map_err(|(error, _admission)| error)
    }

    pub async fn apply_runtime_turn_with_recoverable_reserved_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<
        CoreApplyOutput,
        (
            SessionError,
            Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
        ),
    > {
        Self::require_runtime_execution_kind_stamp(&req).map_err(|error| (error, None))?;
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _turn_guard = recovery_gate.lock().await;
        let _ = self
            .discard_stale_live_session_if_needed(id)
            .await
            .map_err(|error| (error, None))?;
        let pre_turn_context_events = req.runtime.pre_turn_context_appends.clone();
        let start_turn_result = self
            .inner
            .start_turn_with_runtime_context_admission_recovering_not_found(id, req, admission)
            .await;
        match start_turn_result {
            Ok(run_result) => self
                .build_runtime_output_after_live_mutation(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(CoreApplyTerminal::RunResult(Box::new(run_result))),
                    pre_turn_context_events,
                )
                .await
                .map_err(|error| (error, None)),
            Err((
                SessionError::Agent(meerkat_core::error::AgentError::NoPendingBoundary),
                _admission,
            )) => {
                if !pre_turn_context_events.is_empty() {
                    self.inner
                        .apply_runtime_system_context_for_turn(id, pre_turn_context_events.clone())
                        .await
                        .map_err(|error| (error, None))?;
                }
                self.build_runtime_output(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(CoreApplyTerminal::NoPendingBoundary),
                    pre_turn_context_events,
                )
                .await
                .map_err(|error| (error, None))
            }
            Err((error @ SessionError::NotFound { .. }, admission)) if admission.is_some() => {
                Err((error, admission))
            }
            Err((error, _admission)) => {
                if let Some(terminal) = Self::callback_pending_terminal(&error) {
                    self.build_runtime_output_after_live_mutation(
                        id,
                        run_id,
                        boundary,
                        contributing_input_ids,
                        Some(terminal),
                        pre_turn_context_events,
                    )
                    .await
                    .map_err(|error| (error, None))
                } else {
                    if let Err(discard_error) = self.discard_live_session(id).await {
                        tracing::warn!(
                            session_id = %id,
                            error = %discard_error,
                            "failed to discard live session after failed runtime turn"
                        );
                    }
                    Err((error, None))
                }
            }
        }
    }

    async fn apply_runtime_turn_outcome_with_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
    ) -> Result<CoreApplyOutput, SessionError> {
        Self::require_runtime_execution_kind_stamp(&req)?;
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _turn_guard = recovery_gate.lock().await;
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        let pre_turn_context_events = req.runtime.pre_turn_context_appends.clone();
        let start_turn_result = match admission {
            Some(admission) => {
                self.inner
                    .start_turn_with_runtime_context_admission(id, req, admission)
                    .await
            }
            None => self.inner.start_turn(id, req).await,
        };
        match start_turn_result {
            Ok(run_result) => {
                self.build_runtime_output_after_live_mutation(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(CoreApplyTerminal::RunResult(Box::new(run_result))),
                    pre_turn_context_events,
                )
                .await
            }
            Err(SessionError::Agent(meerkat_core::error::AgentError::NoPendingBoundary)) => {
                if !pre_turn_context_events.is_empty() {
                    self.inner
                        .apply_runtime_system_context_for_turn(id, pre_turn_context_events.clone())
                        .await?;
                }
                self.build_runtime_output(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(CoreApplyTerminal::NoPendingBoundary),
                    pre_turn_context_events,
                )
                .await
            }
            Err(error) => {
                if let Some(terminal) = Self::callback_pending_terminal(&error) {
                    self.build_runtime_output_after_live_mutation(
                        id,
                        run_id,
                        boundary,
                        contributing_input_ids,
                        Some(terminal),
                        pre_turn_context_events,
                    )
                    .await
                } else {
                    if let Err(discard_error) = self.discard_live_session(id).await {
                        tracing::warn!(
                            session_id = %id,
                            error = %discard_error,
                            "failed to discard live session after failed runtime turn"
                        );
                    }
                    Err(error)
                }
            }
        }
    }

    fn require_runtime_execution_kind_stamp(req: &StartTurnRequest) -> Result<(), SessionError> {
        if req
            .runtime
            .turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.execution_kind)
            .is_some()
        {
            return Ok(());
        }

        Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(
                "runtime_execution_kind not set: runtime-backed turn did not stamp RuntimeTurnMetadata.execution_kind"
                    .to_string(),
            ),
        ))
    }

    pub async fn apply_runtime_context_appends(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_context_appends_with_boundary(
            id,
            run_id,
            appends,
            RunApplyBoundary::Immediate,
            contributing_input_ids,
        )
        .await
    }

    /// Apply runtime-owned system context to the live session before a
    /// reaction turn. The subsequent runtime turn commit owns durability and
    /// lifecycle for the combined context+run operation.
    pub async fn apply_runtime_system_context_for_turn(
        &self,
        id: &SessionId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<(), SessionError> {
        self.inner
            .apply_runtime_system_context_for_turn(id, appends)
            .await
    }

    pub async fn apply_runtime_context_appends_with_boundary(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_context_appends_with_admission(
            id,
            run_id,
            appends,
            boundary,
            contributing_input_ids,
            None,
        )
        .await
    }

    pub async fn apply_runtime_context_appends_with_reserved_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_context_appends_with_recoverable_reserved_admission(
            id,
            run_id,
            appends,
            boundary,
            contributing_input_ids,
            admission,
        )
        .await
        .map_err(|(error, _admission)| error)
    }

    pub async fn apply_runtime_context_appends_with_recoverable_reserved_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<
        CoreApplyOutput,
        (
            SessionError,
            Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
        ),
    > {
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _turn_guard = recovery_gate.lock().await;
        let _ = self
            .discard_stale_live_session_if_needed(id)
            .await
            .map_err(|error| (error, None))?;
        if let Some(session) = self
            .load_authoritative_session_base(id)
            .await
            .map_err(|error| (error, None))?
        {
            self.reject_if_archived_session(id, &session)
                .await
                .map_err(control_error_into_session_error)
                .map_err(|error| (error, None))?;
        }
        let mut active_guard = Some(admission);
        if let Err(error) = self
            .inner
            .apply_runtime_system_context_for_turn(id, appends.clone())
            .await
        {
            let admission = if matches!(error, SessionError::NotFound { .. }) {
                active_guard.take()
            } else {
                None
            };
            return Err((error, admission));
        }

        self.build_runtime_context_output_after_live_mutation(
            id,
            run_id,
            boundary,
            contributing_input_ids,
            appends,
        )
        .await
        .map_err(|error| (error, None))
    }

    async fn apply_runtime_context_appends_with_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _turn_guard = recovery_gate.lock().await;
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        if let Some(session) = self.load_authoritative_session_base(id).await? {
            self.reject_if_archived_session(id, &session)
                .await
                .map_err(control_error_into_session_error)?;
        }
        let _active_guard = match admission {
            Some(admission) => admission,
            None => self.inner.acquire_runtime_context_admission(id).await?,
        };

        self.inner
            .apply_runtime_system_context_for_turn(id, appends.clone())
            .await?;

        self.build_runtime_context_output_after_live_mutation(
            id,
            run_id,
            boundary,
            contributing_input_ids,
            appends,
        )
        .await
    }
}

impl<B: SessionAgentBuilder + 'static> PersistentSessionService<B> {
    async fn create_session_with_admission(
        &self,
        mut req: CreateSessionRequest,
        reserved_create_admission: Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
        archived_resume_authorization: ArchivedResumeAuthorization,
    ) -> Result<RunResult, SessionError> {
        self.reject_runtime_backed_eager_create_session(&req)?;

        // Inject a checkpointer for all sessions. The keep-alive attached loop
        // calls it after each interaction to keep the SessionStore projection
        // current. Runtime-backed sessions still commit runtime boundary
        // authority through RuntimeStore; this checkpointer writes only the
        // compatibility/session snapshot projection.
        let gate = Arc::new(CheckpointerGate {
            cancelled: Mutex::new(false),
        });
        let checkpointer = Arc::new(StoreCheckpointer {
            store: Arc::clone(&self.store),
            blob_store: Arc::clone(&self.blob_store),
            event_store: self.event_store.clone(),
            projector: self.projector.clone(),
            gate: Arc::clone(&gate),
            last_saved_revision: std::sync::Mutex::new(None),
        });
        let (resume_session_id, resume_session) = {
            let build = req.build.get_or_insert_with(Default::default);
            let resume_session_id = build
                .resume_session
                .as_ref()
                .map(|session| session.id().clone());
            let resume_session = build.resume_session.clone();
            build.checkpointer = Some(checkpointer.clone());
            build.blob_store_override = Some(Arc::clone(&self.blob_store));
            (resume_session_id, resume_session)
        };
        if let Some(session) = resume_session.as_ref()
            && self
                .session_archived_by_authority(session.id(), session)
                .await?
            && !archived_resume_authorization.allows_archived_resume()
        {
            return Err(SessionError::NotFound {
                id: session.id().clone(),
            });
        }
        let _resume_recovery_guard = if let Some(resume_session_id) = resume_session_id.as_ref() {
            let recovery_gate = self.recovery_gate_for_session(resume_session_id).await;
            let guard = recovery_gate.lock_owned().await;
            if let Some(session) = self
                .load_authoritative_session_base(resume_session_id)
                .await?
                && self
                    .session_archived_by_authority(resume_session_id, &session)
                    .await?
                && !archived_resume_authorization.allows_archived_resume()
            {
                self.reject_if_archived_session(resume_session_id, &session)
                    .await
                    .map_err(control_error_into_session_error)?;
            }
            Some(guard)
        } else {
            None
        };
        let create_projection_session_tx = self.install_create_time_event_projection(&mut req);
        let callback_session_id = resume_session_id.clone();
        let result = match self
            .inner
            .create_session_with_admission(req, reserved_create_admission)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                if Self::callback_pending_terminal(&error).is_some()
                    && let Some(session_id) = callback_session_id
                {
                    self.persist_full_session_or_discard_live(&session_id)
                        .await?;
                }
                return Err(error);
            }
        };

        // Track the gate so archive() can cancel checkpoint writes.
        {
            self.checkpointer_gates
                .lock()
                .await
                .insert(result.session_id.clone(), gate);
        }
        if let Some(session_tx) = create_projection_session_tx {
            let _ = session_tx.send(Some(result.session_id.clone()));
        }
        self.spawn_event_projection_task(&result.session_id).await;

        // Persist the full session snapshot (messages + metadata) after first
        // turn and seed the checkpointer so the next keep-alive checkpoint is
        // skipped if the session hasn't changed since this save.
        let saved_revision = self
            .persist_full_session_or_discard_live(&result.session_id)
            .await?;
        if let Ok(mut last_saved_revision) = checkpointer.last_saved_revision.lock() {
            *last_saved_revision = Some(saved_revision);
        }

        Ok(result)
    }

    async fn archive_with_store_only_mode(
        &self,
        id: &SessionId,
        store_only_mode: StoreOnlyArchiveMode,
        machine_archive: Option<MachineSessionArchiveProtocol<'_>>,
    ) -> Result<(), SessionError> {
        if let Some(protocol) = machine_archive.as_ref()
            && let Some(runtime_store) = self.runtime_store.as_ref()
        {
            protocol.require_shared_runtime_store(id, runtime_store)?;
        }
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _recovery_guard = recovery_gate.lock().await;
        let _ = self.discard_stale_live_session_if_needed(id).await?;

        let archived_snapshot = match self.export_session_with_labels(id).await {
            Ok(session) => Some(session),
            Err(SessionError::NotFound { .. }) => match store_only_mode {
                StoreOnlyArchiveMode::Reject if self.runtime_store.is_none() => self
                    .store
                    .load(id)
                    .await
                    .map_err(|e| SessionError::Store(Box::new(e)))?,
                StoreOnlyArchiveMode::Reject => {
                    self.load_persisted_session_for_control(id, "archive")
                        .await?
                }
                StoreOnlyArchiveMode::MachineAuthority => {
                    self.load_machine_authority_session_for_control(id).await?
                }
            },
            Err(err) => return Err(err),
        };
        let already_retired_by_machine = if let Some(ref session) = archived_snapshot {
            self.session_archived_by_authority(id, session).await?
        } else {
            false
        };
        if already_retired_by_machine {
            return Err(SessionError::NotFound { id: id.clone() });
        }

        // Acquire the checkpointer gate (if any) and hold it across the
        // archival save. This prevents a concurrent checkpoint() from saving
        // a live snapshot over the archived one. Setting cancelled under the
        // lock ensures all future checkpoints are no-ops.
        let gate = self.existing_gate_for_session(id).await;
        let mut gate_guard = if let Some(ref gate) = gate {
            let mut guard = gate.cancelled.lock().await;
            *guard = true;
            Some(guard)
        } else {
            None
        };

        let had_durable_snapshot = archived_snapshot.is_some();
        let mut machine_retired = already_retired_by_machine;
        if let Some(protocol) = machine_archive {
            let should_retire = !already_retired_by_machine
                && (archived_snapshot.is_some() || protocol.session_registered(id).await);
            if should_retire && let Err(err) = protocol.retire_session(id).await {
                if let Some(ref mut guard) = gate_guard {
                    **guard = false;
                }
                return Err(err);
            }
            machine_retired = should_retire;
        }

        let mut saved_archived_snapshot = false;
        if let Some(session) = archived_snapshot.clone() {
            let mut session = session;
            session.set_metadata(SESSION_ARCHIVED_KEY, serde_json::Value::Bool(true));
            match self.save_compatibility_projection_only(session).await {
                Ok(_) => {
                    saved_archived_snapshot = true;
                }
                Err(err) => {
                    if self.runtime_store.is_some() && machine_retired {
                        tracing::warn!(
                            session_id = %id,
                            error = %err,
                            "machine archive retired session but compatibility projection save failed"
                        );
                        saved_archived_snapshot = false;
                    } else {
                        if let Some(ref mut guard) = gate_guard {
                            **guard = false;
                        }
                        return Err(err);
                    }
                }
            }
        }

        let live_result = self.inner.archive(id).await;

        // Gate guard is dropped here - any in-flight checkpoint that was
        // blocked on the lock will now see cancelled == true and bail out.
        drop(gate_guard.take());
        self.checkpointer_gates.lock().await.remove(id);

        match (&live_result, saved_archived_snapshot) {
            // At least one side had the session - success.
            (Ok(()), _) | (_, true) => Ok(()),
            (_, false) if machine_retired => Ok(()),
            (_, false) if had_durable_snapshot => Ok(()),
            // Neither side had it - propagate NotFound from the live service.
            _ => live_result,
        }
    }

    /// Archive with a concrete machine protocol. Runtime-backed surfaces must
    /// use this path so archive behavior is backed by a durable
    /// `MeerkatMachine::Retire` transition. Compatibility projection writes
    /// happen only after that canonical lifecycle transition has landed.
    pub async fn archive_with_machine_protocol(
        &self,
        id: &SessionId,
        protocol: MachineSessionArchiveProtocol<'_>,
    ) -> Result<(), SessionError> {
        self.archive_with_store_only_mode(
            id,
            StoreOnlyArchiveMode::MachineAuthority,
            Some(protocol),
        )
        .await
    }

    /// Apply a live hard cancel from a `MeerkatMachine` executor handle.
    ///
    /// Public session-service control paths are blocked below so user-visible
    /// interruption cannot bypass the machine command/recovery path. The
    /// machine still needs this concrete, token-guarded live hook after its
    /// command reducer has accepted the cancel.
    pub async fn interrupt_with_machine_authority(
        &self,
        id: &SessionId,
        _authority: MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        self.inner.interrupt(id).await
    }

    /// Apply a cooperative boundary cancel from a `MeerkatMachine` executor handle.
    pub async fn cancel_after_boundary_with_machine_authority(
        &self,
        id: &SessionId,
        _authority: MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        self.inner.cancel_after_boundary(id).await
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionService for PersistentSessionService<B> {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        self.create_session_with_admission(req, None, ArchivedResumeAuthorization::RejectArchived)
            .await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.reject_runtime_backed_direct_start_turn()?;
        self.legacy_start_turn_inner_with_admission(id, req, None)
            .await
            .map_err(|(error, _admission)| error)
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "interrupt for runtime-backed session {id} must route through MeerkatMachine::hard_cancel_current_run"
        )))
    }

    async fn cancel_after_boundary(&self, id: &SessionId) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "cancel_after_boundary for runtime-backed session {id} must route through MeerkatMachine::cancel_after_boundary"
        )))
    }

    async fn set_session_client(
        &self,
        id: &SessionId,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
    ) -> Result<(), SessionError> {
        self.inner.set_session_client(id, client).await
    }

    async fn hot_swap_session_llm_identity(
        &self,
        _id: &SessionId,
        _client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        _identity: meerkat_core::SessionLlmIdentity,
        _request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "hot_swap_session_llm_identity is a bespoke metadata seam that bypasses the canonical RuntimeTurnMetadata carrier; model/provider/provider_params must travel through the single runtime-backed turn seam instead".to_string(),
        ))
    }

    async fn set_session_tool_visibility_state(
        &self,
        id: &SessionId,
        state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        self.inner
            .set_session_tool_visibility_state(id, state)
            .await
    }

    async fn set_session_tool_filter(
        &self,
        id: &SessionId,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        let previous_visibility_state = self
            .export_session_with_labels(id)
            .await
            .and_then(|session| rollback_tool_visibility_state_snapshot(&session))?;

        self.inner.set_session_tool_filter(id, filter).await?;

        if let Err(error) = self.persist_full_session(id).await {
            let _ = self
                .inner
                .set_session_tool_visibility_state(id, previous_visibility_state)
                .await;
            return Err(error);
        }
        Ok(())
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        if let LiveSessionAuthority::DurableAuthoritative { session, .. } =
            self.live_session_authority(id).await?
        {
            self.reject_if_archived_session(id, &session)
                .await
                .map_err(control_error_into_session_error)?;
            return Ok(view_from_authoritative_session(&session));
        }

        match self.inner.read(id).await {
            Ok(view) => Ok(view),
            Err(SessionError::NotFound { .. }) => {
                let Some(session) = self.load_authoritative_session_base(id).await? else {
                    return Err(SessionError::NotFound { id: id.clone() });
                };
                self.reject_if_archived_session(id, &session)
                    .await
                    .map_err(control_error_into_session_error)?;
                Ok(view_from_authoritative_session(&session))
            }
            Err(err) => Err(err),
        }
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let stored = self
            .store
            .list(SessionFilter::default())
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;
        let live_summaries = self.inner.list(SessionQuery::default()).await?;
        let live_ids: HashSet<SessionId> = live_summaries
            .iter()
            .map(|summary| summary.session_id.clone())
            .collect();
        let mut durable_live_summaries: IndexMap<SessionId, Option<SessionSummary>> =
            IndexMap::new();
        for summary in &live_summaries {
            if let LiveSessionAuthority::DurableAuthoritative { session, .. } =
                self.live_session_authority(&summary.session_id).await?
            {
                let durable_summary = if self
                    .session_archived_by_authority(&summary.session_id, session.as_ref())
                    .await?
                {
                    None
                } else {
                    Some(summary_from_meta(meerkat_core::SessionMeta::from(
                        session.as_ref(),
                    )))
                };
                durable_live_summaries.insert(summary.session_id.clone(), durable_summary);
            }
        }

        let mut summaries_by_id: IndexMap<SessionId, SessionSummary> = IndexMap::new();
        for meta in stored {
            if live_ids.contains(&meta.id) {
                continue;
            }
            // Runtime-backed rows are discovery keys only. A stale or
            // store-only projection must not contribute summary metadata unless
            // runtime authority can rebuild the summary below.
            if let Some(session) = self.load_authoritative_session_base(&meta.id).await? {
                if self
                    .session_archived_by_authority(&meta.id, &session)
                    .await?
                {
                    continue;
                }
                let summary = summary_from_meta(meerkat_core::SessionMeta::from(&session));
                summaries_by_id.insert(summary.session_id.clone(), summary);
            }
        }

        for (session_id, summary) in &durable_live_summaries {
            if let Some(summary) = summary {
                summaries_by_id.insert(session_id.clone(), summary.clone());
            }
        }

        for summary in live_summaries {
            if durable_live_summaries.contains_key(&summary.session_id) {
                continue;
            }
            summaries_by_id.insert(summary.session_id.clone(), summary);
        }

        let mut summaries: Vec<SessionSummary> = summaries_by_id.into_values().collect();

        // Filter by labels if specified (all k/v pairs must match).
        if let Some(ref filter_labels) = query.labels {
            summaries.retain(|s| {
                filter_labels
                    .iter()
                    .all(|(k, v)| s.labels.get(k) == Some(v))
            });
        }

        // Apply pagination
        if let Some(offset) = query.offset {
            if offset < summaries.len() {
                summaries = summaries.split_off(offset);
            } else {
                summaries.clear();
            }
        }
        if let Some(limit) = query.limit {
            summaries.truncate(limit);
        }

        Ok(summaries)
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        match self.live_session_authority(id).await? {
            LiveSessionAuthority::NoLive => Ok(false),
            LiveSessionAuthority::LiveAuthoritative => Ok(true),
            LiveSessionAuthority::DurableAuthoritative { session, .. } => {
                if self
                    .session_archived_by_authority(id, session.as_ref())
                    .await?
                {
                    self.discard_live_session(id).await?;
                    Ok(false)
                } else {
                    Ok(true)
                }
            }
        }
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        if self.runtime_store.is_some() {
            return Err(SessionError::Unsupported(format!(
                "runtime-backed archive for session {id} requires MachineSessionArchiveProtocol"
            )));
        }
        self.archive_with_store_only_mode(id, StoreOnlyArchiveMode::Reject, None)
            .await
    }

    async fn update_session_mob_authority_context(
        &self,
        id: &SessionId,
        authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        let previous = self
            .export_session_with_labels(id)
            .await?
            .mob_tool_authority_context();

        self.inner
            .update_session_mob_authority_context(id, authority_context.clone())
            .await?;

        if let Err(error) = self.persist_full_session(id).await {
            let _ = self
                .inner
                .update_session_mob_authority_context(id, previous)
                .await;
            return Err(error);
        }
        Ok(())
    }

    async fn subscribe_session_events(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        self.inner.subscribe_session_events(id).await
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionServiceHistoryExt for PersistentSessionService<B> {
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        let session = self
            .load_authoritative_session_base(id)
            .await?
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        Ok(SessionHistoryPage::from_messages(
            session.id().clone(),
            session.messages(),
            query,
        ))
    }

    async fn read_transcript_revision(
        &self,
        id: &SessionId,
        query: SessionTranscriptRevisionQuery,
    ) -> Result<SessionTranscriptRevisionPage, SessionError> {
        let session = self
            .load_authoritative_session_base(id)
            .await?
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let head_revision = session.transcript_revision().map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to read transcript head revision: {err}"
            )))
        })?;
        let revision = if query.revision == "current" {
            head_revision.clone()
        } else {
            query.revision
        };
        let has_transcript_history_state = session
            .transcript_history_state()
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to read transcript revision graph: {err}"
                )))
            })?
            .is_some();
        let messages = match session
            .transcript_revision_messages(&revision)
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to read transcript revision graph: {err}"
                )))
            })? {
            Some(messages) => messages,
            None if revision == head_revision && !has_transcript_history_state => {
                session.messages().to_vec()
            }
            None => {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::ConfigError(format!(
                        "transcript revision {revision} not found for session {id}",
                    )),
                ));
            }
        };
        Ok(SessionTranscriptRevisionPage::from_messages(
            session.id().clone(),
            revision,
            head_revision,
            &messages,
            query.offset,
            query.limit,
        ))
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionServiceTranscriptEditExt
    for PersistentSessionService<B>
{
    async fn fork_session_at(
        &self,
        id: &SessionId,
        req: SessionForkAtRequest,
    ) -> Result<SessionForkResult, SessionError> {
        let _ = req.running_behavior;
        let source = self.source_session_for_transcript_edit(id).await?;
        if req.message_index > source.messages().len() {
            return Err(meerkat_core::TranscriptEditError::MessageIndexOutOfBounds {
                message_index: req.message_index,
                message_count: source.messages().len(),
            }
            .into_session_error());
        }

        let forked = source.fork_at(req.message_index);
        self.persist_transcript_fork(id.clone(), forked).await
    }

    async fn fork_session_replace(
        &self,
        id: &SessionId,
        req: SessionForkReplaceRequest,
    ) -> Result<SessionForkResult, SessionError> {
        let _ = req.running_behavior;
        let source = self.source_session_for_transcript_edit(id).await?;
        let forked = source
            .fork_replacing(req.message_index, req.replacement)
            .map_err(meerkat_core::TranscriptEditError::into_session_error)?;
        self.persist_transcript_fork(id.clone(), forked).await
    }

    async fn rewrite_session_transcript(
        &self,
        id: &SessionId,
        req: SessionTranscriptRewriteRequest,
    ) -> Result<SessionTranscriptRewriteResult, SessionError> {
        let _ = req.running_behavior;
        let _mutation_guard = self.transcript_edit_mutation_guard(id).await?;
        let mut source = self.source_session_for_transcript_edit_locked(id).await?;
        source = self.normalized_session_for_persistence(source).await?;
        let replacement = self
            .normalized_transcript_rewrite_replacement(req.replacement)
            .await?;
        let commit = source
            .commit_transcript_rewrite(
                req.selection,
                replacement,
                req.reason,
                req.actor,
                req.expected_parent_revision,
            )
            .map_err(meerkat_core::TranscriptEditError::into_session_error)?;
        let saved = self.persist_transcript_rewrite(source, &commit).await?;
        Ok(SessionTranscriptRewriteResult {
            session_id: saved.id().clone(),
            parent_revision: commit.parent_revision.clone(),
            revision: commit.revision.clone(),
            message_count: saved.messages().len(),
            commit,
        })
    }

    async fn restore_session_transcript_revision(
        &self,
        id: &SessionId,
        req: SessionTranscriptRestoreRevisionRequest,
    ) -> Result<SessionTranscriptRewriteResult, SessionError> {
        let _ = req.running_behavior;
        let _mutation_guard = self.transcript_edit_mutation_guard(id).await?;
        let mut source = self.source_session_for_transcript_edit_locked(id).await?;
        source = self.normalized_session_for_persistence(source).await?;
        let replacement = source
            .transcript_revision_messages(&req.revision)
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to read transcript revision graph: {err}"
                )))
            })?
            .ok_or_else(|| {
                SessionError::Agent(meerkat_core::error::AgentError::ConfigError(format!(
                    "transcript revision {} not found for session {id}",
                    req.revision
                )))
            })?;
        let replacement = self
            .normalized_transcript_rewrite_replacement(replacement)
            .await?;
        let message_count = source.messages().len();
        let commit = source
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: message_count,
                },
                replacement,
                req.reason,
                req.actor,
                req.expected_parent_revision,
            )
            .map_err(meerkat_core::TranscriptEditError::into_session_error)?;
        let saved = self.persist_transcript_rewrite(source, &commit).await?;
        Ok(SessionTranscriptRewriteResult {
            session_id: saved.id().clone(),
            parent_revision: commit.parent_revision.clone(),
            revision: commit.revision.clone(),
            message_count: saved.messages().len(),
            commit,
        })
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionServiceCommsExt for PersistentSessionService<B> {
    async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.inner.comms_runtime(session_id).await
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::EventInjector>> {
        self.inner.event_injector(session_id).await
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        self.inner.interaction_event_injector(session_id).await
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionServiceControlExt for PersistentSessionService<B> {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        let existing_gate = self.existing_gate_for_session(id).await;
        if let Some(state_handle) = self.inner.system_context_state(id).await {
            let created_gate = existing_gate.is_none();
            let gate = match existing_gate {
                Some(gate) => gate,
                None => self.gate_for_session(id).await,
            };
            {
                let gate_guard = gate.cancelled.lock().await;
                if *gate_guard {
                    return Err(SessionControlError::Session(SessionError::NotFound {
                        id: id.clone(),
                    }));
                }
            }

            let accepted_at = meerkat_core::time_compat::SystemTime::now();
            let (status, snapshot_state, persisted_state) = state_handle
                .stage_append_with_snapshot(&req, accepted_at)
                .map_err(|err| err.into_control_error(id))?;

            let _projection_guard = self.recovery_gate_for_session(id).await.lock_owned().await;
            let mut session = if self.runtime_store.is_some() {
                match self.load_authoritative_session_base(id).await? {
                    Some(session) => session,
                    None => {
                        if created_gate {
                            self.checkpointer_gates.lock().await.remove(id);
                        }
                        return Err(SessionControlError::Session(SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(
                                "runtime-backed live session is missing its last committed snapshot"
                                    .to_string(),
                            ),
                        )));
                    }
                }
            } else {
                match self.export_session_with_labels(id).await {
                    Ok(session) => session,
                    Err(err) => {
                        if created_gate && matches!(err, SessionError::NotFound { .. }) {
                            self.checkpointer_gates.lock().await.remove(id);
                        }
                        return Err(SessionControlError::Session(err));
                    }
                }
            };

            let gate_guard = gate.cancelled.lock().await;
            if *gate_guard {
                let rollback_result = state_handle
                    .replace_from_generated_restore_if_current(&persisted_state, snapshot_state)
                    .map_err(|_| ());
                drop(gate_guard);
                if matches!(rollback_result, Ok(true)) {
                    let _ = self.inner.sync_system_context_state(id).await;
                } else {
                    tracing::warn!(
                        session_id = %id,
                        "live system-context state diverged after archive cancelled append; discarding live session"
                    );
                    let _ = self.discard_live_session(id).await;
                }
                return Err(SessionControlError::Session(SessionError::NotFound {
                    id: id.clone(),
                }));
            }

            let persist_result = async {
                self.reject_if_archived_session(id, &session).await?;
                write_system_context_state(&mut session, persisted_state.clone())?;
                self.save_normalized_session(session)
                    .await
                    .map_err(SessionControlError::Session)?;
                Ok::<(), SessionControlError>(())
            }
            .await;

            if let Err(err) = persist_result {
                let rollback_result = state_handle
                    .replace_from_generated_restore_if_current(&persisted_state, snapshot_state)
                    .map_err(|_| ());
                if matches!(rollback_result, Ok(true)) {
                    let _ = self.inner.sync_system_context_state(id).await;
                } else {
                    tracing::warn!(
                        session_id = %id,
                        "live system-context state diverged after a failed durable append; discarding the live session to restore authoritative state"
                    );
                    drop(gate_guard);
                    let _ = self.discard_live_session(id).await;
                }
                return Err(err);
            }

            let reconciled_state = state_handle.snapshot();
            if reconciled_state != persisted_state {
                let mut session = if self.runtime_store.is_some() {
                    match self.load_authoritative_session_base(id).await? {
                        Some(session) => session,
                        None => {
                            drop(gate_guard);
                            let _ = self.discard_live_session(id).await;
                            return Ok(AppendSystemContextResult { status });
                        }
                    }
                } else {
                    self.export_session_with_labels(id)
                        .await
                        .map_err(SessionControlError::Session)?
                };
                self.reject_if_archived_session(id, &session).await?;
                write_system_context_state(&mut session, reconciled_state)?;
                self.save_normalized_session(session)
                    .await
                    .map_err(SessionControlError::Session)?;
            }

            let _ = self.inner.sync_system_context_state(id).await;
            drop(gate_guard);
            return Ok(AppendSystemContextResult { status });
        }

        if let Some(gate) = existing_gate {
            let gate_guard = gate.cancelled.lock().await;
            if *gate_guard {
                return Err(SessionControlError::Session(SessionError::NotFound {
                    id: id.clone(),
                }));
            }
            drop(gate_guard);
        }

        let _projection_guard = self.recovery_gate_for_session(id).await.lock_owned().await;
        let mut session = match self
            .load_persisted_session_for_control(id, "append_system_context")
            .await?
        {
            Some(session) => session,
            None => {
                self.checkpointer_gates.lock().await.remove(id);
                return Err(SessionControlError::Session(SessionError::NotFound {
                    id: id.clone(),
                }));
            }
        };
        self.reject_if_archived_session(id, &session).await?;
        let mut state = session.system_context_state().unwrap_or_default();
        let status = state
            .stage_append(&req, meerkat_core::time_compat::SystemTime::now())
            .map_err(|err| err.into_control_error(id))?;
        write_system_context_state(&mut session, state)?;
        self.save_normalized_session(session)
            .await
            .map_err(SessionControlError::Session)?;
        Ok(AppendSystemContextResult { status })
    }

    async fn stage_tool_results(
        &self,
        id: &SessionId,
        req: StageToolResultsRequest,
    ) -> Result<StageToolResultsResult, SessionError> {
        validate_tool_result_video(&req.results)?;

        let existing_gate = self.existing_gate_for_session(id).await;
        if let Some(state_arc) = self.inner.deferred_turn_state(id).await {
            let created_gate = existing_gate.is_none();
            let gate = match existing_gate {
                Some(gate) => gate,
                None => self.gate_for_session(id).await,
            };
            let gate_guard = gate.cancelled.lock().await;
            if *gate_guard {
                return Err(SessionError::NotFound { id: id.clone() });
            }

            let accepted_at = meerkat_core::time_compat::SystemTime::now();
            let mut attempts = 0usize;
            loop {
                attempts += 1;
                let (accepted, snapshot_state, persisted_state) = {
                    let guard = match state_arc.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => {
                            tracing::warn!(
                                session_id = %id,
                                "deferred-turn state lock poisoned while snapshotting staged tool results"
                            );
                            poisoned.into_inner()
                        }
                    };
                    let snapshot_state = guard.clone();
                    let mut candidate = snapshot_state.clone();
                    let accepted = candidate.stage_tool_results(req.results.clone(), accepted_at);
                    (accepted, snapshot_state, candidate)
                };

                if accepted == 0 {
                    drop(gate_guard);
                    return Ok(StageToolResultsResult {
                        accepted_result_count: accepted,
                    });
                }

                let _projection_guard = self.recovery_gate_for_session(id).await.lock_owned().await;
                let mut session = if self.runtime_store.is_some() {
                    match self.load_authoritative_session_base(id).await? {
                        Some(session) => session,
                        None => {
                            if created_gate {
                                drop(gate_guard);
                                self.checkpointer_gates.lock().await.remove(id);
                            }
                            return Err(SessionError::Agent(
                                meerkat_core::error::AgentError::InternalError(
                                    "runtime-backed live session is missing its last committed snapshot"
                                        .to_string(),
                                ),
                            ));
                        }
                    }
                } else {
                    match self.export_session_with_labels(id).await {
                        Ok(session) => session,
                        Err(err) => {
                            if created_gate && matches!(err, SessionError::NotFound { .. }) {
                                drop(gate_guard);
                                self.checkpointer_gates.lock().await.remove(id);
                            }
                            return Err(err);
                        }
                    }
                };

                self.reject_if_archived_session(id, &session)
                    .await
                    .map_err(control_error_into_session_error)?;
                write_deferred_turn_state(&mut session, persisted_state)
                    .map_err(control_error_into_session_error)?;
                self.save_normalized_session(session).await?;

                let commit_result = {
                    let mut guard = match state_arc.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => {
                            tracing::warn!(
                                session_id = %id,
                                "deferred-turn state lock poisoned while committing staged tool results"
                            );
                            poisoned.into_inner()
                        }
                    };
                    if *guard == snapshot_state {
                        Some(guard.stage_tool_results(req.results.clone(), accepted_at))
                    } else {
                        None
                    }
                };

                if let Some(live_accepted) = commit_result {
                    debug_assert_eq!(live_accepted, accepted);
                    drop(gate_guard);
                    return Ok(StageToolResultsResult {
                        accepted_result_count: accepted,
                    });
                }

                if attempts >= 8 {
                    tracing::warn!(
                        session_id = %id,
                        "deferred-turn state kept changing after durable tool-result staging; discarding live session to force authoritative reload"
                    );
                    drop(gate_guard);
                    let _ = self.discard_live_session(id).await;
                    return Ok(StageToolResultsResult {
                        accepted_result_count: accepted,
                    });
                }
            }
        }

        if let Some(gate) = existing_gate {
            let gate_guard = gate.cancelled.lock().await;
            if *gate_guard {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            drop(gate_guard);
        }

        let _projection_guard = self.recovery_gate_for_session(id).await.lock_owned().await;
        let mut session = match self
            .load_persisted_session_for_control(id, "stage_tool_results")
            .await?
        {
            Some(session) => session,
            None => {
                self.checkpointer_gates.lock().await.remove(id);
                return Err(SessionError::NotFound { id: id.clone() });
            }
        };
        self.reject_if_archived_session(id, &session)
            .await
            .map_err(control_error_into_session_error)?;
        let mut state = session.deferred_turn_state().unwrap_or_default();
        let accepted =
            state.stage_tool_results(req.results, meerkat_core::time_compat::SystemTime::now());
        write_deferred_turn_state(&mut session, state).map_err(control_error_into_session_error)?;
        self.save_normalized_session(session).await?;
        Ok(StageToolResultsResult {
            accepted_result_count: accepted,
        })
    }
}

impl<B: SessionAgentBuilder + 'static> PersistentSessionService<B> {
    /// Get the event injector for a session, if available.
    pub async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::EventInjector>> {
        self.inner.event_injector(session_id).await
    }

    #[doc(hidden)]
    pub async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        self.inner.interaction_event_injector(session_id).await
    }

    /// Get the comms runtime for a session, if available.
    pub async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.inner.comms_runtime(session_id).await
    }

    /// Wait for a session to be registered.
    pub async fn wait_session_registered(&self) {
        self.inner.wait_session_registered().await;
    }

    /// Shut down all sessions.
    pub async fn shutdown(&self) {
        self.inner.shutdown().await;
    }

    /// Cancel all active checkpointer gates.
    ///
    /// After this call, in-flight checkpoints that are past the gate check
    /// will complete their current save, but subsequent checkpoint calls on
    /// any session will be no-ops. Use this during `stop()` to prevent
    /// checkpoint writes from racing with external cleanup operations.
    pub async fn cancel_all_checkpointers(&self) {
        let gates = self.checkpointer_gates.lock().await;
        for gate in gates.values() {
            let mut cancelled = gate.cancelled.lock().await;
            *cancelled = true;
        }
    }

    /// Re-enable checkpointer gates for all tracked sessions.
    ///
    /// Call this during `resume()` after `cancel_all_checkpointers()` was
    /// used during stop. Gates that were removed by `archive()` are not
    /// affected.
    pub async fn rearm_all_checkpointers(&self) {
        let gates = self.checkpointer_gates.lock().await;
        for gate in gates.values() {
            let mut cancelled = gate.cancelled.lock().await;
            *cancelled = false;
        }
    }

    /// Subscribe to session-wide events from the live inner service.
    pub async fn subscribe_session_events(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        self.inner.subscribe_session_events(id).await
    }

    /// Whether a live session still has its deferred first turn pending.
    pub async fn live_deferred_first_turn_pending(
        &self,
        id: &SessionId,
    ) -> Result<bool, SessionError> {
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        let Some(state) = self.inner.deferred_turn_state(id).await else {
            return Ok(false);
        };
        let state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(matches!(
            state.first_turn_phase(),
            DeferredFirstTurnPhase::Pending
        ))
    }

    /// Load the authoritative durable session view.
    ///
    /// Runtime snapshots are the session authority when a runtime store is
    /// available. The `SessionStore` row is only a compatibility projection in
    /// that mode. Store-only rows are not exposed as authoritative runtime
    /// truth; existing runtime snapshots are never merged with projection
    /// metadata.
    pub async fn load_authoritative_session(
        &self,
        id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        self.load_authoritative_session_base(id).await
    }

    /// Export the full session from the live task and persist it to the store.
    ///
    /// Returns the saved transcript digest so callers can seed a checkpointer
    /// without a second export round-trip.
    async fn persist_full_session(&self, id: &SessionId) -> Result<(usize, String), SessionError> {
        let session = self.export_session_with_labels(id).await?;
        let persisted = self.save_normalized_session(session).await?;
        let message_count = persisted.messages().len();
        let revision =
            meerkat_core::transcript_messages_digest(persisted.messages()).map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to digest persisted transcript for checkpoint seed: {err}"
                )))
            })?;
        Ok((message_count, revision))
    }

    async fn persist_full_session_or_discard_live(
        &self,
        id: &SessionId,
    ) -> Result<String, SessionError> {
        match self.persist_full_session(id).await {
            Ok((_message_count, revision)) => Ok(revision),
            Err(error) => {
                match self.discard_live_session(id).await {
                    Ok(()) | Err(SessionError::NotFound { .. }) => {}
                    Err(discard_error) => {
                        tracing::warn!(
                            session_id = %id,
                            error = %discard_error,
                            "failed to discard live session after full-session persistence failure"
                        );
                    }
                }
                Err(error)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::ephemeral::{
        EphemeralSessionService, ObservedSessionTailKind, SessionAgent, SessionAgentBuilder,
        SessionSnapshot,
    };
    use crate::event_store::{EVENT_SCHEMA_VERSION, EventStoreError, StoredEvent};
    use meerkat_core::ToolDispatchOutcome;
    use meerkat_core::checkpoint::SessionCheckpointer;
    use meerkat_core::event::AgentEvent;
    use meerkat_core::service::{
        DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionForkAtRequest,
        SessionForkReplaceRequest, SessionService, SessionServiceControlExt,
        SessionServiceTranscriptEditExt, StageToolResultsRequest, TranscriptEditRunningBehavior,
        TranscriptReplacement, TranscriptRewriteReason, TranscriptRewriteSelection,
    };
    use meerkat_core::session::SESSION_METADATA_KEY;
    use meerkat_core::types::{
        AssistantBlock, ContentBlock, ContentInput, ImageData, Message, StopReason, ToolCall,
        ToolResult, Usage, UserMessage,
    };
    use meerkat_core::{RunId, lifecycle::run_primitive::RunApplyBoundary};
    use meerkat_runtime::{InMemoryRuntimeStore, RuntimeStore};
    use meerkat_store::{MemoryBlobStore, MemoryStore, SessionStoreError};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn memory_blob_store() -> Arc<dyn BlobStore> {
        Arc::new(MemoryBlobStore::new())
    }

    struct RecordingEventStore {
        events: Mutex<HashMap<SessionId, Vec<StoredEvent>>>,
        notify: tokio::sync::Notify,
        fail_appends: AtomicBool,
        rewrite_append_calls: AtomicUsize,
        fail_rewrite_append_call: AtomicUsize,
    }

    impl Default for RecordingEventStore {
        fn default() -> Self {
            Self {
                events: Mutex::new(HashMap::new()),
                notify: tokio::sync::Notify::new(),
                fail_appends: AtomicBool::new(false),
                rewrite_append_calls: AtomicUsize::new(0),
                fail_rewrite_append_call: AtomicUsize::new(0),
            }
        }
    }

    impl RecordingEventStore {
        fn fail_appends(&self) {
            self.fail_appends.store(true, Ordering::Release);
        }

        fn allow_appends(&self) {
            self.fail_appends.store(false, Ordering::Release);
        }

        fn fail_rewrite_append_call(&self, call: usize) {
            self.fail_rewrite_append_call.store(call, Ordering::Release);
        }

        async fn wait_for_seq(&self, session_id: &SessionId, target_seq: u64) {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if self.last_seq(session_id).await.unwrap() >= target_seq {
                        return;
                    }
                    self.notify.notified().await;
                }
            })
            .await
            .expect("event store projection did not reach expected sequence");
        }
    }

    async fn read_projected_events_after(events_path: &std::path::Path, expected: &str) -> String {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Ok(projected) = tokio::fs::read_to_string(events_path).await
                    && projected.contains(expected)
                {
                    return projected;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("projected events.jsonl did not contain expected event")
    }

    #[async_trait::async_trait]
    impl EventStore for RecordingEventStore {
        async fn append(
            &self,
            session_id: &SessionId,
            events: &[AgentEvent],
        ) -> Result<u64, EventStoreError> {
            if self.fail_appends.load(Ordering::Acquire) {
                return Err(EventStoreError::Store(
                    "synthetic transcript rewrite audit append failure".to_string(),
                ));
            }
            if events
                .iter()
                .any(|event| matches!(event, AgentEvent::TranscriptRewriteCommitted { .. }))
            {
                let call = self.rewrite_append_calls.fetch_add(1, Ordering::AcqRel) + 1;
                if self.fail_rewrite_append_call.load(Ordering::Acquire) == call {
                    return Err(EventStoreError::Store(
                        "synthetic transcript rewrite audit append failure".to_string(),
                    ));
                }
            }
            let mut all_events = self.events.lock().await;
            let session_events = all_events.entry(session_id.clone()).or_default();
            for event in events {
                let seq = session_events.len() as u64 + 1;
                session_events.push(StoredEvent {
                    seq,
                    schema_version: EVENT_SCHEMA_VERSION,
                    timestamp: meerkat_core::time_compat::SystemTime::now(),
                    event: event.clone(),
                });
            }
            let last_seq = session_events.len() as u64;
            drop(all_events);
            self.notify.notify_waiters();
            Ok(last_seq)
        }

        async fn read_from(
            &self,
            session_id: &SessionId,
            from_seq: u64,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            let all_events = self.events.lock().await;
            Ok(all_events
                .get(session_id)
                .into_iter()
                .flat_map(|events| events.iter())
                .filter(|event| event.seq >= from_seq)
                .cloned()
                .collect())
        }

        async fn last_seq(&self, session_id: &SessionId) -> Result<u64, EventStoreError> {
            let all_events = self.events.lock().await;
            Ok(all_events
                .get(session_id)
                .map_or(0, |events| events.len() as u64))
        }
    }

    struct PausingTranscriptRewriteStore {
        inner: MemoryStore,
        pause_rewrite_save: AtomicBool,
        entered_rewrite_save: tokio::sync::Notify,
        release_rewrite_save: tokio::sync::Notify,
    }

    impl PausingTranscriptRewriteStore {
        fn new() -> Self {
            Self {
                inner: MemoryStore::new(),
                pause_rewrite_save: AtomicBool::new(false),
                entered_rewrite_save: tokio::sync::Notify::new(),
                release_rewrite_save: tokio::sync::Notify::new(),
            }
        }

        fn pause_rewrite_saves(&self) {
            self.pause_rewrite_save.store(true, Ordering::Release);
        }

        async fn wait_for_rewrite_save(&self) {
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.entered_rewrite_save.notified(),
            )
            .await
            .expect("rewrite save did not pause");
        }

        fn release_rewrite_save(&self) {
            self.release_rewrite_save.notify_waiters();
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for PausingTranscriptRewriteStore {
        async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
            self.inner.save(session).await
        }

        async fn save_transcript_rewrite(
            &self,
            session: &Session,
            commit: &meerkat_core::TranscriptRewriteCommit,
        ) -> Result<(), SessionStoreError> {
            if self.pause_rewrite_save.load(Ordering::Acquire) {
                self.entered_rewrite_save.notify_waiters();
                self.release_rewrite_save.notified().await;
            }
            self.inner.save_transcript_rewrite(session, commit).await
        }

        async fn save_authoritative_projection(
            &self,
            session: &Session,
        ) -> Result<(), SessionStoreError> {
            self.inner.save_authoritative_projection(session).await
        }

        async fn save_authoritative_projection_if_current_revision(
            &self,
            session: &Session,
            expected_current_revision: Option<String>,
        ) -> Result<(), SessionStoreError> {
            self.inner
                .save_authoritative_projection_if_current_revision(
                    session,
                    expected_current_revision,
                )
                .await
        }

        async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
            self.inner.load(id).await
        }

        async fn list(
            &self,
            filter: meerkat_store::SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, SessionStoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
            self.inner.delete(id).await
        }

        async fn delete_if_current_revision(
            &self,
            id: &SessionId,
            expected_current_revision: &str,
        ) -> Result<bool, SessionStoreError> {
            self.inner
                .delete_if_current_revision(id, expected_current_revision)
                .await
        }
    }

    struct ConflictOnTranscriptRewriteStore {
        inner: MemoryStore,
    }

    impl ConflictOnTranscriptRewriteStore {
        fn new() -> Self {
            Self {
                inner: MemoryStore::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for ConflictOnTranscriptRewriteStore {
        async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
            self.inner.save(session).await
        }

        async fn save_transcript_rewrite(
            &self,
            session: &Session,
            commit: &meerkat_core::TranscriptRewriteCommit,
        ) -> Result<(), SessionStoreError> {
            Err(SessionStoreError::TranscriptRevisionConflict {
                id: session.id().clone(),
                expected: commit.parent_revision.clone(),
                actual: "sha256:stored-head".to_string(),
            })
        }

        async fn save_authoritative_projection(
            &self,
            session: &Session,
        ) -> Result<(), SessionStoreError> {
            self.inner.save_authoritative_projection(session).await
        }

        async fn save_authoritative_projection_if_current_revision(
            &self,
            session: &Session,
            expected_current_revision: Option<String>,
        ) -> Result<(), SessionStoreError> {
            self.inner
                .save_authoritative_projection_if_current_revision(
                    session,
                    expected_current_revision,
                )
                .await
        }

        async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
            self.inner.load(id).await
        }

        async fn list(
            &self,
            filter: meerkat_store::SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, SessionStoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
            self.inner.delete(id).await
        }

        async fn delete_if_current_revision(
            &self,
            id: &SessionId,
            expected_current_revision: &str,
        ) -> Result<bool, SessionStoreError> {
            self.inner
                .delete_if_current_revision(id, expected_current_revision)
                .await
        }
    }

    struct FailSaveStore {
        inner: MemoryStore,
        fail_save: AtomicBool,
    }

    impl FailSaveStore {
        fn new() -> Self {
            Self {
                inner: MemoryStore::new(),
                fail_save: AtomicBool::new(false),
            }
        }

        fn set_fail_save(&self, fail: bool) {
            self.fail_save.store(fail, Ordering::Release);
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for FailSaveStore {
        async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
            if self.fail_save.load(Ordering::Acquire) {
                return Err(SessionStoreError::Internal(
                    "forced save failure".to_string(),
                ));
            }
            self.inner.save(session).await
        }

        async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
            self.inner.load(id).await
        }

        async fn list(
            &self,
            filter: meerkat_store::SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, SessionStoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
            self.inner.delete(id).await
        }

        async fn delete_if_current_revision(
            &self,
            id: &SessionId,
            expected_current_revision: &str,
        ) -> Result<bool, SessionStoreError> {
            self.inner
                .delete_if_current_revision(id, expected_current_revision)
                .await
        }
    }

    struct FailAuthoritativeProjectionStore {
        inner: MemoryStore,
        fail_authoritative_projection_cas: AtomicBool,
    }

    impl FailAuthoritativeProjectionStore {
        fn new() -> Self {
            Self {
                inner: MemoryStore::new(),
                fail_authoritative_projection_cas: AtomicBool::new(false),
            }
        }

        fn fail_authoritative_projection_cas(&self) {
            self.fail_authoritative_projection_cas
                .store(true, Ordering::Release);
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for FailAuthoritativeProjectionStore {
        async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
            self.inner.save(session).await
        }

        async fn save_transcript_rewrite(
            &self,
            session: &Session,
            commit: &meerkat_core::TranscriptRewriteCommit,
        ) -> Result<(), SessionStoreError> {
            self.inner.save_transcript_rewrite(session, commit).await
        }

        async fn save_authoritative_projection(
            &self,
            session: &Session,
        ) -> Result<(), SessionStoreError> {
            self.inner.save_authoritative_projection(session).await
        }

        async fn save_authoritative_projection_if_current_revision(
            &self,
            session: &Session,
            expected_current_revision: Option<String>,
        ) -> Result<(), SessionStoreError> {
            if self
                .fail_authoritative_projection_cas
                .load(Ordering::Acquire)
            {
                return Err(SessionStoreError::Internal(
                    "forced authoritative projection CAS failure".to_string(),
                ));
            }
            self.inner
                .save_authoritative_projection_if_current_revision(
                    session,
                    expected_current_revision,
                )
                .await
        }

        async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
            self.inner.load(id).await
        }

        async fn list(
            &self,
            filter: meerkat_store::SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, SessionStoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
            self.inner.delete(id).await
        }

        async fn delete_if_current_revision(
            &self,
            id: &SessionId,
            expected_current_revision: &str,
        ) -> Result<bool, SessionStoreError> {
            self.inner
                .delete_if_current_revision(id, expected_current_revision)
                .await
        }
    }

    struct BlockingArchiveSaveStore {
        inner: MemoryStore,
        block_archived_saves: AtomicBool,
        entered_archived_save: tokio::sync::Notify,
        release_archived_save: tokio::sync::Notify,
    }

    impl BlockingArchiveSaveStore {
        fn new() -> Self {
            Self {
                inner: MemoryStore::new(),
                block_archived_saves: AtomicBool::new(false),
                entered_archived_save: tokio::sync::Notify::new(),
                release_archived_save: tokio::sync::Notify::new(),
            }
        }

        fn block_archived_saves(&self) {
            self.block_archived_saves.store(true, Ordering::Release);
        }

        async fn wait_for_archived_save(&self) {
            self.entered_archived_save.notified().await;
        }

        fn release_archived_save(&self) {
            self.release_archived_save.notify_waiters();
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for BlockingArchiveSaveStore {
        async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
            if self.block_archived_saves.load(Ordering::Acquire)
                && metadata_marks_archived(session.metadata())
            {
                self.entered_archived_save.notify_waiters();
                self.release_archived_save.notified().await;
            }
            self.inner.save(session).await
        }

        async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
            self.inner.load(id).await
        }

        async fn list(
            &self,
            filter: meerkat_store::SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, SessionStoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
            self.inner.delete(id).await
        }

        async fn delete_if_current_revision(
            &self,
            id: &SessionId,
            expected_current_revision: &str,
        ) -> Result<bool, SessionStoreError> {
            self.inner
                .delete_if_current_revision(id, expected_current_revision)
                .await
        }
    }

    struct GatedSnapshotRuntimeStore {
        inner: InMemoryRuntimeStore,
        hidden_snapshot_loads: AtomicUsize,
        fail_snapshot_commits: AtomicBool,
        fail_snapshot_replaces: AtomicBool,
        session_snapshot_overrides: Mutex<HashMap<LogicalRuntimeId, Vec<u8>>>,
        replace_snapshot_interlopers: Mutex<HashMap<LogicalRuntimeId, Vec<u8>>>,
        clear_snapshot_interlopers: Mutex<HashMap<LogicalRuntimeId, Vec<u8>>>,
        input_state_load_errors: Mutex<HashSet<LogicalRuntimeId>>,
        boundary_commits: Mutex<Vec<meerkat_core::lifecycle::RunBoundaryReceipt>>,
    }

    impl GatedSnapshotRuntimeStore {
        fn new() -> Self {
            Self {
                inner: InMemoryRuntimeStore::new(),
                hidden_snapshot_loads: AtomicUsize::new(0),
                fail_snapshot_commits: AtomicBool::new(false),
                fail_snapshot_replaces: AtomicBool::new(false),
                session_snapshot_overrides: Mutex::new(HashMap::new()),
                replace_snapshot_interlopers: Mutex::new(HashMap::new()),
                clear_snapshot_interlopers: Mutex::new(HashMap::new()),
                input_state_load_errors: Mutex::new(HashSet::new()),
                boundary_commits: Mutex::new(Vec::new()),
            }
        }

        fn set_fail_snapshot_commits(&self, fail: bool) {
            self.fail_snapshot_commits.store(fail, Ordering::Release);
        }

        fn set_fail_snapshot_replaces(&self, fail: bool) {
            self.fail_snapshot_replaces.store(fail, Ordering::Release);
        }

        fn hide_next_session_snapshot_loads(&self, count: usize) {
            self.hidden_snapshot_loads.store(count, Ordering::Release);
        }

        fn should_hide_session_snapshot_load(&self) -> bool {
            let mut remaining = self.hidden_snapshot_loads.load(Ordering::Acquire);
            while remaining > 0 {
                match self.hidden_snapshot_loads.compare_exchange(
                    remaining,
                    remaining - 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return true,
                    Err(current) => remaining = current,
                }
            }
            false
        }

        async fn boundary_commits(&self) -> Vec<meerkat_core::lifecycle::RunBoundaryReceipt> {
            self.boundary_commits.lock().await.clone()
        }

        async fn reset_boundary_commits(&self) {
            self.boundary_commits.lock().await.clear();
        }

        async fn override_session_snapshot(&self, runtime_id: LogicalRuntimeId, snapshot: Vec<u8>) {
            self.session_snapshot_overrides
                .lock()
                .await
                .insert(runtime_id, snapshot);
        }

        async fn interlope_before_snapshot_replace(
            &self,
            runtime_id: LogicalRuntimeId,
            snapshot: Vec<u8>,
        ) {
            self.replace_snapshot_interlopers
                .lock()
                .await
                .insert(runtime_id, snapshot);
        }

        async fn interlope_before_snapshot_clear(
            &self,
            runtime_id: LogicalRuntimeId,
            snapshot: Vec<u8>,
        ) {
            self.clear_snapshot_interlopers
                .lock()
                .await
                .insert(runtime_id, snapshot);
        }

        async fn fail_input_state_load_for(&self, runtime_id: LogicalRuntimeId) {
            self.input_state_load_errors.lock().await.insert(runtime_id);
        }
    }

    #[async_trait::async_trait]
    impl RuntimeStore for GatedSnapshotRuntimeStore {
        async fn commit_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: meerkat_runtime::store::SessionDelta,
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            if self.fail_snapshot_commits.load(Ordering::Acquire) {
                return Err(meerkat_runtime::store::RuntimeStoreError::WriteFailed(
                    "synthetic runtime snapshot commit failure".to_string(),
                ));
            }
            if let Some(interloper) = self
                .replace_snapshot_interlopers
                .lock()
                .await
                .remove(runtime_id)
            {
                self.inner
                    .commit_session_snapshot(
                        runtime_id,
                        SessionDelta {
                            session_snapshot: interloper,
                        },
                    )
                    .await?;
            }
            self.inner
                .commit_session_snapshot(runtime_id, session_delta)
                .await
        }

        async fn commit_session_transcript_rewrite_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: meerkat_runtime::store::SessionDelta,
            commit: &meerkat_core::TranscriptRewriteCommit,
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            if self.fail_snapshot_commits.load(Ordering::Acquire) {
                return Err(meerkat_runtime::store::RuntimeStoreError::WriteFailed(
                    "synthetic runtime snapshot commit failure".to_string(),
                ));
            }
            self.inner
                .commit_session_transcript_rewrite_snapshot(runtime_id, session_delta, commit)
                .await
        }

        async fn atomic_apply(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: Option<meerkat_runtime::store::SessionDelta>,
            receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
            input_updates: Vec<InputStatePersistenceRecord>,
            session_store_key: Option<SessionId>,
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            self.inner
                .atomic_apply(
                    runtime_id,
                    session_delta,
                    receipt,
                    input_updates,
                    session_store_key,
                )
                .await
        }

        async fn load_input_states(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Vec<StoredInputState>, meerkat_runtime::store::RuntimeStoreError> {
            if self
                .input_state_load_errors
                .lock()
                .await
                .contains(runtime_id)
            {
                return Err(meerkat_runtime::store::RuntimeStoreError::ReadFailed(
                    "synthetic legacy input-state load failure".to_string(),
                ));
            }
            self.inner.load_input_states(runtime_id).await
        }

        async fn load_boundary_receipt(
            &self,
            runtime_id: &LogicalRuntimeId,
            run_id: &RunId,
            sequence: u64,
        ) -> Result<
            Option<meerkat_core::lifecycle::RunBoundaryReceipt>,
            meerkat_runtime::store::RuntimeStoreError,
        > {
            self.inner
                .load_boundary_receipt(runtime_id, run_id, sequence)
                .await
        }

        async fn load_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, meerkat_runtime::store::RuntimeStoreError> {
            if self.should_hide_session_snapshot_load() {
                return Ok(None);
            }
            if let Some(snapshot) = self
                .session_snapshot_overrides
                .lock()
                .await
                .get(runtime_id)
                .cloned()
            {
                return Ok(Some(snapshot));
            }
            self.inner.load_session_snapshot(runtime_id).await
        }

        async fn clear_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            if let Some(interloper) = self
                .clear_snapshot_interlopers
                .lock()
                .await
                .remove(runtime_id)
            {
                self.inner
                    .commit_session_snapshot(
                        runtime_id,
                        SessionDelta {
                            session_snapshot: interloper,
                        },
                    )
                    .await?;
            }
            self.session_snapshot_overrides
                .lock()
                .await
                .remove(runtime_id);
            self.inner.clear_session_snapshot(runtime_id).await
        }

        async fn replace_session_snapshot_if_current(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected_current: &[u8],
            replacement: Vec<u8>,
        ) -> Result<bool, meerkat_runtime::store::RuntimeStoreError> {
            if self.fail_snapshot_commits.load(Ordering::Acquire) {
                return Err(meerkat_runtime::store::RuntimeStoreError::WriteFailed(
                    "synthetic runtime snapshot commit failure".to_string(),
                ));
            }
            if self.fail_snapshot_replaces.load(Ordering::Acquire) {
                return Err(meerkat_runtime::store::RuntimeStoreError::WriteFailed(
                    "synthetic runtime snapshot replace failure".to_string(),
                ));
            }
            if let Some(interloper) = self
                .replace_snapshot_interlopers
                .lock()
                .await
                .remove(runtime_id)
            {
                self.inner
                    .commit_session_snapshot(
                        runtime_id,
                        SessionDelta {
                            session_snapshot: interloper,
                        },
                    )
                    .await?;
            }
            {
                let mut overrides = self.session_snapshot_overrides.lock().await;
                if let Some(current) = overrides.get_mut(runtime_id) {
                    if current.as_slice() != expected_current {
                        return Ok(false);
                    }
                    *current = replacement;
                    return Ok(true);
                }
            }
            self.inner
                .replace_session_snapshot_if_current(runtime_id, expected_current, replacement)
                .await
        }

        async fn clear_session_snapshot_if_current(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected_current: &[u8],
        ) -> Result<bool, meerkat_runtime::store::RuntimeStoreError> {
            if let Some(interloper) = self
                .clear_snapshot_interlopers
                .lock()
                .await
                .remove(runtime_id)
            {
                self.inner
                    .commit_session_snapshot(
                        runtime_id,
                        SessionDelta {
                            session_snapshot: interloper,
                        },
                    )
                    .await?;
            }
            {
                let mut overrides = self.session_snapshot_overrides.lock().await;
                if let Some(current) = overrides.get(runtime_id) {
                    if current.as_slice() != expected_current {
                        return Ok(false);
                    }
                    overrides.remove(runtime_id);
                    return Ok(true);
                }
            }
            self.inner
                .clear_session_snapshot_if_current(runtime_id, expected_current)
                .await
        }

        async fn persist_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: &InputStatePersistenceRecord,
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            self.inner.persist_input_state(runtime_id, state).await
        }

        async fn load_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            input_id: &InputId,
        ) -> Result<Option<StoredInputState>, meerkat_runtime::store::RuntimeStoreError> {
            self.inner.load_input_state(runtime_id, input_id).await
        }

        async fn load_machine_lifecycle_record(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, meerkat_runtime::store::RuntimeStoreError> {
            self.inner.load_machine_lifecycle_record(runtime_id).await
        }

        async fn commit_machine_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
            commit: meerkat_runtime::store::MachineLifecycleCommit,
            input_states: &[InputStatePersistenceRecord],
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            self.inner
                .commit_machine_lifecycle(runtime_id, commit, input_states)
                .await
        }

        async fn persist_ops_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
            snapshot: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            self.inner.persist_ops_lifecycle(runtime_id, snapshot).await
        }

        async fn load_ops_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<
            Option<meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot>,
            meerkat_runtime::store::RuntimeStoreError,
        > {
            self.inner.load_ops_lifecycle(runtime_id).await
        }
    }

    struct DummyAgent {
        session: Arc<std::sync::Mutex<Session>>,
        system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
        run_failure: Option<String>,
        flow_overlay_failure: Option<String>,
        callback_pending_after_run: bool,
    }

    #[async_trait::async_trait]
    impl SessionAgent for DummyAgent {
        async fn run_with_events(
            &mut self,
            prompt: meerkat_core::types::ContentInput,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            if let Some(message) = self.run_failure.clone() {
                return Err(meerkat_core::error::AgentError::InternalError(message));
            }
            let session_id = self.session_id();
            let result = {
                let mut session = match self.session.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                session.push(meerkat_core::types::Message::User(
                    meerkat_core::types::UserMessage::text(prompt.text_content()),
                ));
                session.push(meerkat_core::types::Message::Assistant(
                    meerkat_core::types::AssistantMessage {
                        content: "ok".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: meerkat_core::types::StopReason::EndTurn,
                        usage: meerkat_core::types::Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    },
                ));
                RunResult {
                    text: "ok".to_string(),
                    session_id,
                    usage: meerkat_core::types::Usage::default(),
                    turns: 1,
                    tool_calls: 0,
                    terminal_cause_kind: None,
                    structured_output: None,
                    extraction_error: None,
                    schema_warnings: None,
                    skill_diagnostics: None,
                }
            };
            if self.callback_pending_after_run {
                return Err(meerkat_core::error::AgentError::CallbackPending {
                    tool_name: "test_callback".to_string(),
                    args: serde_json::json!({}),
                });
            }
            Ok(result)
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_flow_tool_overlay(
            &mut self,
            _overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            if let Some(message) = self.flow_overlay_failure.clone() {
                return Err(meerkat_core::error::AgentError::InternalError(message));
            }
            Ok(())
        }

        fn cancel(&mut self) {}

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut metadata =
                session
                    .session_metadata()
                    .unwrap_or(meerkat_core::SessionMetadata {
                        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                        model: identity.model.clone(),
                        max_tokens: 0,
                        structured_output_retries: 2,
                        provider: identity.provider,
                        self_hosted_server_id: None,
                        provider_params: identity.provider_params.clone(),
                        tooling: meerkat_core::SessionTooling::default(),
                        keep_alive: false,
                        comms_name: None,
                        peer_meta: None,
                        realm_id: None,
                        instance_id: None,
                        backend: None,
                        config_generation: None,
                        auth_binding: None,
                    });
            metadata.apply_llm_identity(&identity);
            session.set_session_metadata(metadata).map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to update dummy session metadata: {err}"
                ))
            })
        }

        fn stage_external_tool_filter(
            &mut self,
            filter: meerkat_core::ToolFilter,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut state = {
                let session = match self.session.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                session
                    .tool_visibility_state()
                    .map_err(|err| {
                        meerkat_core::error::AgentError::InternalError(format!(
                            "failed to decode dummy visibility state: {err}"
                        ))
                    })?
                    .unwrap_or_default()
            };
            state.staged_filter = filter;
            state.staged_revision = state.staged_revision.max(state.active_revision) + 1;
            self.set_tool_visibility_state(Some(state))
        }

        fn set_tool_visibility_state(
            &mut self,
            state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let _ = state;
            Ok(())
        }

        fn session_id(&self) -> SessionId {
            match self.session.lock() {
                Ok(guard) => guard.id().clone(),
                Err(poisoned) => poisoned.into_inner().id().clone(),
            }
        }

        fn snapshot(&self) -> SessionSnapshot {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            SessionSnapshot {
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                total_tokens: session.total_tokens(),
                usage: session.total_usage(),
                last_assistant_text: session.last_assistant_text(),
            }
        }

        fn session_clone(&self) -> Session {
            match self.session.lock() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }

        fn durable_llm_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            Some(test_durable_llm_identity(&session))
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            meerkat_core::pending_continuation_admission::observe_session_tail(session.messages())
        }

        fn update_keep_alive(&mut self, keep_alive: bool) {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(mut meta) = session.session_metadata() {
                meta.keep_alive = keep_alive;
                let _ = session.set_session_metadata(meta);
            }
        }

        fn update_mob_tool_authority_context(
            &mut self,
            authority_context: Option<MobToolAuthorityContext>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            session
                .set_mob_tool_authority_context(authority_context)
                .map_err(|err| {
                    meerkat_core::error::AgentError::InternalError(format!(
                        "failed to update dummy mob authority context: {err}"
                    ))
                })
        }

        fn update_system_prompt(
            &mut self,
            system_prompt: String,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            session.set_system_prompt(system_prompt);
            Ok(())
        }

        fn apply_runtime_system_context(
            &mut self,
            appends: &[meerkat_core::PendingSystemContextAppend],
        ) {
            let state = {
                let mut guard = match self.session.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                guard.append_system_context_blocks(appends);
                guard.system_context_state().unwrap_or_default()
            };
            match self.system_context_state.lock() {
                Ok(mut guard) => {
                    *guard = state;
                }
                Err(poisoned) => {
                    *poisoned.into_inner() = state;
                }
            }
        }

        fn system_context_state(&self) -> meerkat_core::SystemContextStateHandle {
            meerkat_core::SystemContextStateHandle::from_shared_authority_state(Arc::clone(
                &self.system_context_state,
            ))
        }

        fn sync_session_from_durable_snapshot(
            &mut self,
            session: Session,
        ) -> Result<(), meerkat_core::error::AgentError> {
            if session.id() != &self.session_id() {
                return Err(meerkat_core::error::AgentError::InternalError(format!(
                    "durable snapshot session id {} does not match live session {}",
                    session.id(),
                    self.session_id()
                )));
            }
            let system_context_state = session.system_context_state().unwrap_or_default();
            match self.session.lock() {
                Ok(mut guard) => {
                    *guard = session;
                }
                Err(poisoned) => {
                    *poisoned.into_inner() = session;
                }
            }
            match self.system_context_state.lock() {
                Ok(mut guard) => {
                    *guard = system_context_state;
                }
                Err(poisoned) => {
                    *poisoned.into_inner() = system_context_state;
                }
            }
            Ok(())
        }
    }

    struct EventfulDummyAgent {
        inner: DummyAgent,
    }

    #[async_trait::async_trait]
    impl SessionAgent for EventfulDummyAgent {
        async fn run_with_events(
            &mut self,
            prompt: meerkat_core::types::ContentInput,
            event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            let session_id = self.inner.session_id();
            let _ = event_tx
                .send(AgentEvent::RunStarted {
                    session_id: session_id.clone(),
                    prompt: prompt.clone(),
                })
                .await;
            let result = self.inner.run_with_events(prompt, event_tx.clone()).await?;
            let _ = event_tx
                .send(AgentEvent::RunCompleted {
                    session_id,
                    result: result.text.clone(),
                    structured_output: result.structured_output.clone(),
                    extraction_required: false,
                    usage: result.usage.clone(),
                    terminal_cause_kind: result.terminal_cause_kind,
                })
                .await;
            Ok(result)
        }

        fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
            self.inner.set_skill_references(refs);
        }

        fn set_flow_tool_overlay(
            &mut self,
            overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.set_flow_tool_overlay(overlay)
        }

        fn cancel(&mut self) {
            self.inner.cancel();
        }

        fn hot_swap_llm_identity(
            &mut self,
            client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner
                .hot_swap_llm_identity(client, identity, request_policy)
        }

        fn stage_external_tool_filter(
            &mut self,
            filter: meerkat_core::ToolFilter,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.stage_external_tool_filter(filter)
        }

        fn set_tool_visibility_state(
            &mut self,
            state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.set_tool_visibility_state(state)
        }

        fn session_id(&self) -> SessionId {
            self.inner.session_id()
        }

        fn snapshot(&self) -> SessionSnapshot {
            self.inner.snapshot()
        }

        fn session_clone(&self) -> Session {
            self.inner.session_clone()
        }

        fn durable_llm_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
            self.inner.durable_llm_identity()
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            self.inner.observed_session_tail()
        }

        fn update_keep_alive(&mut self, keep_alive: bool) {
            self.inner.update_keep_alive(keep_alive);
        }

        fn update_mob_tool_authority_context(
            &mut self,
            authority_context: Option<MobToolAuthorityContext>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner
                .update_mob_tool_authority_context(authority_context)
        }

        fn update_system_prompt(
            &mut self,
            system_prompt: String,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.update_system_prompt(system_prompt)
        }

        fn apply_runtime_system_context(
            &mut self,
            appends: &[meerkat_core::PendingSystemContextAppend],
        ) {
            self.inner.apply_runtime_system_context(appends);
        }

        fn system_context_state(&self) -> meerkat_core::SystemContextStateHandle {
            self.inner.system_context_state()
        }
    }

    struct NoopSubscribableInjector;

    impl meerkat_core::EventInjector for NoopSubscribableInjector {
        fn inject(
            &self,
            _content: meerkat_core::types::ContentInput,
            _source: meerkat_core::PlainEventSource,
            _handling_mode: meerkat_core::types::HandlingMode,
            _render_metadata: Option<meerkat_core::types::RenderMetadata>,
        ) -> Result<(), meerkat_core::event_injector::EventInjectorError> {
            Ok(())
        }
    }

    impl meerkat_core::event_injector::SubscribableInjector for NoopSubscribableInjector {
        fn inject_with_subscription(
            &self,
            _content: meerkat_core::types::ContentInput,
            _source: meerkat_core::PlainEventSource,
            _handling_mode: meerkat_core::types::HandlingMode,
            _render_metadata: Option<meerkat_core::types::RenderMetadata>,
        ) -> Result<
            meerkat_core::event_injector::InteractionSubscription,
            meerkat_core::event_injector::EventInjectorError,
        > {
            let (_tx, events) = tokio::sync::mpsc::channel(1);
            Ok(meerkat_core::event_injector::InteractionSubscription {
                id: serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"")
                    .expect("static interaction id should deserialize"),
                events,
            })
        }
    }

    struct NoopCommsRuntime {
        notify: Arc<tokio::sync::Notify>,
        injector: Arc<NoopSubscribableInjector>,
    }

    impl NoopCommsRuntime {
        fn new(injector: Arc<NoopSubscribableInjector>) -> Self {
            Self {
                notify: Arc::new(tokio::sync::Notify::new()),
                injector,
            }
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::agent::CommsRuntime for NoopCommsRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            Arc::clone(&self.notify)
        }

        fn interaction_event_injector(
            &self,
        ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
            let injector: Arc<dyn meerkat_core::event_injector::SubscribableInjector> =
                self.injector.clone();
            Some(injector)
        }
    }

    struct CapabilityAgent {
        inner: DummyAgent,
        comms: Arc<NoopCommsRuntime>,
        injector: Arc<NoopSubscribableInjector>,
    }

    #[async_trait::async_trait]
    impl SessionAgent for CapabilityAgent {
        async fn run_with_events(
            &mut self,
            prompt: meerkat_core::types::ContentInput,
            event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            self.inner.run_with_events(prompt, event_tx).await
        }

        fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
            self.inner.set_skill_references(refs);
        }

        fn set_flow_tool_overlay(
            &mut self,
            overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.set_flow_tool_overlay(overlay)
        }

        fn cancel(&mut self) {
            self.inner.cancel();
        }

        fn hot_swap_llm_identity(
            &mut self,
            client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner
                .hot_swap_llm_identity(client, identity, request_policy)
        }

        fn stage_external_tool_filter(
            &mut self,
            filter: meerkat_core::ToolFilter,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.stage_external_tool_filter(filter)
        }

        fn set_tool_visibility_state(
            &mut self,
            state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.set_tool_visibility_state(state)
        }

        fn session_id(&self) -> SessionId {
            self.inner.session_id()
        }

        fn snapshot(&self) -> SessionSnapshot {
            self.inner.snapshot()
        }

        fn session_clone(&self) -> Session {
            self.inner.session_clone()
        }

        fn durable_llm_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
            self.inner.durable_llm_identity()
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            self.inner.observed_session_tail()
        }

        fn update_keep_alive(&mut self, keep_alive: bool) {
            self.inner.update_keep_alive(keep_alive);
        }

        fn update_mob_tool_authority_context(
            &mut self,
            authority_context: Option<MobToolAuthorityContext>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner
                .update_mob_tool_authority_context(authority_context)
        }

        fn update_system_prompt(
            &mut self,
            system_prompt: String,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.update_system_prompt(system_prompt)
        }

        fn apply_runtime_system_context(
            &mut self,
            appends: &[meerkat_core::PendingSystemContextAppend],
        ) {
            self.inner.apply_runtime_system_context(appends);
        }

        fn system_context_state(&self) -> meerkat_core::SystemContextStateHandle {
            self.inner.system_context_state()
        }

        fn sync_session_from_durable_snapshot(
            &mut self,
            session: Session,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.sync_session_from_durable_snapshot(session)
        }

        fn interaction_event_injector(
            &self,
        ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
            let injector: Arc<dyn meerkat_core::event_injector::SubscribableInjector> =
                self.injector.clone();
            Some(injector)
        }

        fn comms_runtime(&self) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
            let comms: Arc<dyn meerkat_core::agent::CommsRuntime> = self.comms.clone();
            Some(comms)
        }
    }

    struct CapabilityBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for CapabilityBuilder {
        type Agent = CapabilityAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            let injector = Arc::new(NoopSubscribableInjector);
            let comms = Arc::new(NoopCommsRuntime::new(Arc::clone(&injector)));
            Ok(CapabilityAgent {
                inner: DummyAgent {
                    session: Arc::new(std::sync::Mutex::new(session)),
                    system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                    run_failure: None,
                    flow_overlay_failure: None,
                    callback_pending_after_run: false,
                },
                comms,
                injector,
            })
        }
    }

    struct DummyBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for DummyBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: None,
                flow_overlay_failure: None,
                callback_pending_after_run: false,
            })
        }
    }

    struct CallbackPendingBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for CallbackPendingBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: None,
                flow_overlay_failure: None,
                callback_pending_after_run: true,
            })
        }
    }

    struct FailingOverlayBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for FailingOverlayBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: None,
                flow_overlay_failure: Some("synthetic flow overlay failure".to_string()),
                callback_pending_after_run: false,
            })
        }
    }

    #[derive(Clone)]
    struct BlockingBuildBuilder {
        entered_builds: Arc<AtomicUsize>,
        max_concurrent_builds: Arc<AtomicUsize>,
        active_builds: Arc<AtomicUsize>,
        entered_notify: Arc<tokio::sync::Notify>,
        release_notify: Arc<tokio::sync::Semaphore>,
    }

    impl BlockingBuildBuilder {
        fn new() -> Self {
            Self {
                entered_builds: Arc::new(AtomicUsize::new(0)),
                max_concurrent_builds: Arc::new(AtomicUsize::new(0)),
                active_builds: Arc::new(AtomicUsize::new(0)),
                entered_notify: Arc::new(tokio::sync::Notify::new()),
                release_notify: Arc::new(tokio::sync::Semaphore::new(0)),
            }
        }

        fn record_build_start(&self) {
            self.entered_builds.fetch_add(1, Ordering::AcqRel);
            let active = self.active_builds.fetch_add(1, Ordering::AcqRel) + 1;
            let mut observed = self.max_concurrent_builds.load(Ordering::Acquire);
            while active > observed {
                match self.max_concurrent_builds.compare_exchange(
                    observed,
                    active,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(current) => observed = current,
                }
            }
            self.entered_notify.notify_waiters();
        }

        fn record_build_finish(&self) {
            self.active_builds.fetch_sub(1, Ordering::AcqRel);
        }

        async fn wait_for_entered_builds(&self, expected: usize) {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if self.entered_builds.load(Ordering::Acquire) >= expected {
                        return;
                    }
                    self.entered_notify.notified().await;
                }
            })
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "blocking build builder did not enter {expected} build(s); observed {}",
                    self.entered_builds.load(Ordering::Acquire)
                )
            });
        }
    }

    #[async_trait::async_trait]
    impl SessionAgentBuilder for BlockingBuildBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            self.record_build_start();
            self.release_notify
                .acquire()
                .await
                .expect("blocking build release semaphore should stay open")
                .forget();
            self.record_build_finish();
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: None,
                flow_overlay_failure: None,
                callback_pending_after_run: false,
            })
        }
    }

    #[derive(Clone)]
    struct BlockingRunBuilder {
        entered_runs: Arc<AtomicUsize>,
        entered_notify: Arc<tokio::sync::Notify>,
        release_notify: Arc<tokio::sync::Semaphore>,
    }

    impl BlockingRunBuilder {
        fn new() -> Self {
            Self {
                entered_runs: Arc::new(AtomicUsize::new(0)),
                entered_notify: Arc::new(tokio::sync::Notify::new()),
                release_notify: Arc::new(tokio::sync::Semaphore::new(0)),
            }
        }

        async fn wait_for_entered_runs(&self, expected: usize) {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if self.entered_runs.load(Ordering::Acquire) >= expected {
                        return;
                    }
                    self.entered_notify.notified().await;
                }
            })
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "blocking run builder did not enter {expected} run(s); observed {}",
                    self.entered_runs.load(Ordering::Acquire)
                )
            });
        }
    }

    struct BlockingRunAgent {
        inner: DummyAgent,
        entered_runs: Arc<AtomicUsize>,
        entered_notify: Arc<tokio::sync::Notify>,
        release_notify: Arc<tokio::sync::Semaphore>,
    }

    #[async_trait::async_trait]
    impl SessionAgentBuilder for BlockingRunBuilder {
        type Agent = BlockingRunAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(BlockingRunAgent {
                inner: DummyAgent {
                    session: Arc::new(std::sync::Mutex::new(session)),
                    system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                    run_failure: None,
                    flow_overlay_failure: None,
                    callback_pending_after_run: false,
                },
                entered_runs: Arc::clone(&self.entered_runs),
                entered_notify: Arc::clone(&self.entered_notify),
                release_notify: Arc::clone(&self.release_notify),
            })
        }
    }

    #[async_trait::async_trait]
    impl SessionAgent for BlockingRunAgent {
        async fn run_with_events(
            &mut self,
            prompt: meerkat_core::types::ContentInput,
            event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            self.entered_runs.fetch_add(1, Ordering::AcqRel);
            self.entered_notify.notify_waiters();
            self.release_notify
                .acquire()
                .await
                .expect("blocking run release semaphore should stay open")
                .forget();
            self.inner.run_with_events(prompt, event_tx).await
        }

        fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
            self.inner.set_skill_references(refs);
        }

        fn set_flow_tool_overlay(
            &mut self,
            overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.set_flow_tool_overlay(overlay)
        }

        fn cancel(&mut self) {
            self.inner.cancel();
        }

        fn hot_swap_llm_identity(
            &mut self,
            client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner
                .hot_swap_llm_identity(client, identity, request_policy)
        }

        fn stage_external_tool_filter(
            &mut self,
            filter: meerkat_core::ToolFilter,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.stage_external_tool_filter(filter)
        }

        fn set_tool_visibility_state(
            &mut self,
            state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.set_tool_visibility_state(state)
        }

        fn session_id(&self) -> SessionId {
            self.inner.session_id()
        }

        fn snapshot(&self) -> SessionSnapshot {
            self.inner.snapshot()
        }

        fn session_clone(&self) -> Session {
            self.inner.session_clone()
        }

        fn durable_llm_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
            self.inner.durable_llm_identity()
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            self.inner.observed_session_tail()
        }

        fn update_keep_alive(&mut self, keep_alive: bool) {
            self.inner.update_keep_alive(keep_alive);
        }

        fn update_mob_tool_authority_context(
            &mut self,
            authority_context: Option<MobToolAuthorityContext>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner
                .update_mob_tool_authority_context(authority_context)
        }

        fn update_system_prompt(
            &mut self,
            system_prompt: String,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.update_system_prompt(system_prompt)
        }

        fn apply_runtime_system_context(
            &mut self,
            appends: &[meerkat_core::PendingSystemContextAppend],
        ) {
            self.inner.apply_runtime_system_context(appends);
        }

        fn system_context_state(&self) -> meerkat_core::SystemContextStateHandle {
            self.inner.system_context_state()
        }
    }

    struct EventfulBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for EventfulBuilder {
        type Agent = EventfulDummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(EventfulDummyAgent {
                inner: DummyAgent {
                    session: Arc::new(std::sync::Mutex::new(session)),
                    system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                    run_failure: None,
                    flow_overlay_failure: None,
                    callback_pending_after_run: false,
                },
            })
        }
    }

    struct FailingRunBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for FailingRunBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: Some("synthetic run failure".to_string()),
                flow_overlay_failure: None,
                callback_pending_after_run: false,
            })
        }
    }

    struct CapturingBuildBuilder {
        captured_builds: Arc<tokio::sync::Mutex<Vec<SessionBuildOptions>>>,
    }

    impl CapturingBuildBuilder {
        fn new() -> Self {
            Self {
                captured_builds: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionAgentBuilder for CapturingBuildBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            if let Some(build) = req.build.clone() {
                self.captured_builds.lock().await.push(build);
            }
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: None,
                flow_overlay_failure: None,
                callback_pending_after_run: false,
            })
        }
    }

    struct ImagePreservingAgent {
        session: Arc<std::sync::Mutex<Session>>,
        system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
    }

    #[async_trait::async_trait]
    impl SessionAgent for ImagePreservingAgent {
        async fn run_with_events(
            &mut self,
            prompt: meerkat_core::types::ContentInput,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            session.push(Message::User(UserMessage::with_blocks(
                prompt.into_blocks(),
            )));
            session.push(Message::Assistant(meerkat_core::types::AssistantMessage {
                content: "ok".to_string(),
                tool_calls: vec![],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                usage: meerkat_core::types::Usage::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            }));
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: session.id().clone(),
                usage: meerkat_core::types::Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_flow_tool_overlay(
            &mut self,
            _overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut metadata =
                session
                    .session_metadata()
                    .unwrap_or(meerkat_core::SessionMetadata {
                        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                        model: identity.model.clone(),
                        max_tokens: 0,
                        structured_output_retries: 2,
                        provider: identity.provider,
                        self_hosted_server_id: None,
                        provider_params: identity.provider_params.clone(),
                        tooling: meerkat_core::SessionTooling::default(),
                        keep_alive: false,
                        comms_name: None,
                        peer_meta: None,
                        realm_id: None,
                        instance_id: None,
                        backend: None,
                        config_generation: None,
                        auth_binding: None,
                    });
            metadata.apply_llm_identity(&identity);
            session.set_session_metadata(metadata).map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to update image-preserving session metadata: {err}"
                ))
            })
        }

        fn cancel(&mut self) {}

        fn snapshot(&self) -> SessionSnapshot {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            SessionSnapshot {
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                total_tokens: session.total_tokens(),
                usage: session.total_usage(),
                last_assistant_text: session.last_assistant_text(),
            }
        }

        fn session_clone(&self) -> Session {
            match self.session.lock() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }

        fn durable_llm_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            Some(test_durable_llm_identity(&session))
        }

        fn session_id(&self) -> SessionId {
            let guard = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.id().clone()
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            meerkat_core::pending_continuation_admission::observe_session_tail(session.messages())
        }

        fn apply_runtime_system_context(
            &mut self,
            appends: &[meerkat_core::PendingSystemContextAppend],
        ) {
            let mut guard = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.append_system_context_blocks(appends);
            let state = guard.system_context_state().unwrap_or_default();
            self.system_context_state = Arc::new(std::sync::Mutex::new(state));
        }

        fn system_context_state(&self) -> meerkat_core::SystemContextStateHandle {
            meerkat_core::SystemContextStateHandle::from_shared_authority_state(Arc::clone(
                &self.system_context_state,
            ))
        }

        fn sync_session_from_durable_snapshot(
            &mut self,
            session: Session,
        ) -> Result<(), meerkat_core::error::AgentError> {
            if session.id() != &self.session_id() {
                return Err(meerkat_core::error::AgentError::InternalError(format!(
                    "durable snapshot session id {} does not match live session {}",
                    session.id(),
                    self.session_id()
                )));
            }
            let system_context_state = session.system_context_state().unwrap_or_default();
            match self.session.lock() {
                Ok(mut guard) => {
                    *guard = session;
                }
                Err(poisoned) => {
                    *poisoned.into_inner() = session;
                }
            }
            self.system_context_state = Arc::new(std::sync::Mutex::new(system_context_state));
            Ok(())
        }
    }

    struct ImagePreservingBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for ImagePreservingBuilder {
        type Agent = ImagePreservingAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(ImagePreservingAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
            })
        }
    }

    struct CapturingCheckpointerBuilder {
        captured:
            Arc<tokio::sync::Mutex<Option<Arc<dyn meerkat_core::checkpoint::SessionCheckpointer>>>>,
    }

    impl CapturingCheckpointerBuilder {
        fn new() -> Self {
            Self {
                captured: Arc::new(tokio::sync::Mutex::new(None)),
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionAgentBuilder for CapturingCheckpointerBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            *self.captured.lock().await = req
                .build
                .as_ref()
                .and_then(|build| build.checkpointer.clone());

            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: None,
                flow_overlay_failure: None,
                callback_pending_after_run: false,
            })
        }
    }

    struct ToolDispatchAgent {
        session: Arc<std::sync::Mutex<Session>>,
        system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
    }

    fn expected_tool_dispatch_witness(tool_name: &str) -> meerkat_core::ToolVisibilityWitness {
        meerkat_core::ToolVisibilityWitness {
            stable_owner_key: Some(format!("callback:{tool_name}")),
            last_seen_provenance: Some(meerkat_core::ToolProvenance {
                kind: meerkat_core::ToolSourceKind::Callback,
                source_id: tool_name.to_string().into(),
            }),
        }
    }

    #[async_trait::async_trait]
    impl SessionAgent for ToolDispatchAgent {
        async fn run_with_events(
            &mut self,
            prompt: meerkat_core::types::ContentInput,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            let session_id = self.session_id();
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            session.push(meerkat_core::types::Message::User(
                meerkat_core::types::UserMessage::text(prompt.text_content()),
            ));
            session.push(meerkat_core::types::Message::Assistant(
                meerkat_core::types::AssistantMessage {
                    content: "ok".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: meerkat_core::types::StopReason::EndTurn,
                    usage: meerkat_core::types::Usage::default(),
                    created_at: meerkat_core::types::message_timestamp_now(),
                },
            ));
            Ok(RunResult {
                text: "ok".to_string(),
                session_id,
                usage: meerkat_core::types::Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_flow_tool_overlay(
            &mut self,
            _overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            Ok(())
        }

        async fn dispatch_external_tool_call(
            &mut self,
            call: ToolCall,
        ) -> Result<ToolDispatchOutcome, meerkat_core::error::AgentError> {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut state = session
                .tool_visibility_state()
                .map_err(|err| {
                    meerkat_core::error::AgentError::InternalError(format!(
                        "failed to decode dummy visibility state: {err}"
                    ))
                })?
                .unwrap_or_default();
            let requested_name = format!("requested:{}", call.name);
            state
                .staged_requested_deferred_names
                .insert(requested_name.clone());
            state
                .requested_witnesses
                .insert(requested_name, expected_tool_dispatch_witness(&call.name));
            let _ = state;
            Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
                call.id,
                format!("handled {}", call.name),
                false,
            )))
        }

        fn cancel(&mut self) {}

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut metadata =
                session
                    .session_metadata()
                    .unwrap_or(meerkat_core::SessionMetadata {
                        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                        model: identity.model.clone(),
                        max_tokens: 0,
                        structured_output_retries: 2,
                        provider: identity.provider,
                        self_hosted_server_id: None,
                        provider_params: identity.provider_params.clone(),
                        tooling: meerkat_core::SessionTooling::default(),
                        keep_alive: false,
                        comms_name: None,
                        peer_meta: None,
                        realm_id: None,
                        instance_id: None,
                        backend: None,
                        config_generation: None,
                        auth_binding: None,
                    });
            metadata.apply_llm_identity(&identity);
            session.set_session_metadata(metadata).map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to update tool-dispatch session metadata: {err}"
                ))
            })
        }

        fn session_id(&self) -> SessionId {
            match self.session.lock() {
                Ok(guard) => guard.id().clone(),
                Err(poisoned) => poisoned.into_inner().id().clone(),
            }
        }

        fn snapshot(&self) -> SessionSnapshot {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            SessionSnapshot {
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                total_tokens: session.total_tokens(),
                usage: session.total_usage(),
                last_assistant_text: session.last_assistant_text(),
            }
        }

        fn session_clone(&self) -> Session {
            match self.session.lock() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }

        fn durable_llm_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            Some(test_durable_llm_identity(&session))
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            meerkat_core::pending_continuation_admission::observe_session_tail(session.messages())
        }

        fn apply_runtime_system_context(
            &mut self,
            appends: &[meerkat_core::PendingSystemContextAppend],
        ) {
            let mut guard = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.append_system_context_blocks(appends);
            let state = guard.system_context_state().unwrap_or_default();
            self.system_context_state = Arc::new(std::sync::Mutex::new(state));
        }

        fn system_context_state(&self) -> meerkat_core::SystemContextStateHandle {
            meerkat_core::SystemContextStateHandle::from_shared_authority_state(Arc::clone(
                &self.system_context_state,
            ))
        }
    }

    struct ToolDispatchBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for ToolDispatchBuilder {
        type Agent = ToolDispatchAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(ToolDispatchAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
            })
        }
    }

    fn create_request(prompt: &str, initial_turn: InitialTurnPolicy) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "test".to_string(),
            prompt: prompt.to_string().into(),
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn,
            build: None,
            labels: None,
        }
    }

    fn test_durable_llm_identity(session: &Session) -> meerkat_core::SessionLlmIdentity {
        session
            .session_metadata()
            .map(|metadata| metadata.llm_identity())
            .unwrap_or(meerkat_core::SessionLlmIdentity {
                model: "test".to_string(),
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            })
    }

    fn resume_request(session: Session) -> CreateSessionRequest {
        let mut req = create_request("", InitialTurnPolicy::Defer);
        req.build = Some(SessionBuildOptions {
            resume_session: Some(session),
            ..Default::default()
        });
        req
    }

    fn inline_image_block(label: &str) -> ContentBlock {
        ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: ImageData::Inline {
                data: format!("base64-{label}"),
            },
        }
    }

    fn image_prompt(label: &str) -> ContentInput {
        ContentInput::Blocks(vec![
            ContentBlock::Text {
                text: format!("look at {label}"),
            },
            inline_image_block(label),
        ])
    }

    fn assert_no_inline_images_in_session(session: &Session) {
        for message in session.messages() {
            match message {
                Message::User(user) => {
                    for block in &user.content {
                        assert!(
                            !matches!(
                                block,
                                ContentBlock::Image {
                                    data: ImageData::Inline { .. },
                                    ..
                                }
                            ),
                            "persisted session unexpectedly retained inline image bytes: {session:?}"
                        );
                    }
                }
                Message::ToolResults { results, .. } => {
                    for result in results {
                        for block in &result.content {
                            assert!(
                                !matches!(
                                    block,
                                    ContentBlock::Image {
                                        data: ImageData::Inline { .. },
                                        ..
                                    }
                                ),
                                "persisted session unexpectedly retained inline image bytes: {session:?}"
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn start_turn_request(prompt: &str) -> StartTurnRequest {
        StartTurnRequest {
            prompt: prompt.to_string().into(),
            system_prompt: None,
            event_tx: None,
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
        }
    }

    fn assert_runtime_backed_direct_start_turn_rejected(error: &SessionError) {
        match error {
            SessionError::Unsupported(message) => assert!(
                message.contains(
                    "runtime-backed direct start_turn must route through the MeerkatMachine service-turn commit protocol"
                ),
                "unexpected unsupported error: {message}"
            ),
            other => panic!("expected runtime-backed direct start_turn rejection, got {other:?}"),
        }
    }

    fn assert_runtime_backed_eager_create_rejected(error: &SessionError) {
        match error {
            SessionError::Unsupported(message) => assert!(
                message.contains(
                    "runtime-backed eager create_session must route through the MeerkatMachine service-turn commit protocol"
                ),
                "unexpected unsupported error: {message}"
            ),
            other => panic!("expected runtime-backed eager create rejection, got {other:?}"),
        }
    }

    fn runtime_content_turn_request(prompt: &str) -> StartTurnRequest {
        let mut req = start_turn_request(prompt);
        req.runtime.turn_metadata = Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn),
                ..Default::default()
            },
        );
        req
    }

    async fn machine_commit_runtime_output(
        runtime_store: &dyn RuntimeStore,
        session_id: &SessionId,
        output: &CoreApplyOutput,
    ) {
        let runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(session_id);
        let session_delta = output
            .session_snapshot
            .clone()
            .map(|session_snapshot| meerkat_runtime::store::SessionDelta { session_snapshot });
        runtime_store
            .atomic_apply(
                &runtime_id,
                session_delta,
                output.receipt.clone(),
                Vec::new(),
                Some(session_id.clone()),
            )
            .await
            .expect("machine-owned runtime output commit should succeed");
    }

    fn recovered_input_persistence_record(
        runtime_id: &LogicalRuntimeId,
        mut bundle: StoredInputState,
    ) -> InputStatePersistenceRecord {
        if bundle.state.persisted_input.is_none() {
            let mut input = meerkat_runtime::PromptInput::new("recovered test input", None);
            input.header.id = bundle.state.input_id.clone();
            let input = meerkat_runtime::Input::Prompt(input);
            bundle.state.runtime_semantics.get_or_insert_with(|| {
                meerkat_runtime::ingress_types::RuntimeInputSemantics::try_from_generated_admission(
                    &input, true,
                )
                .expect("test input should receive generated runtime semantics")
            });
            bundle.state.persisted_input = Some(input);
        }
        bundle
            .seed
            .recovery_lane
            .get_or_insert(meerkat_core::types::HandlingMode::Queue);
        let mut driver = meerkat_runtime::EphemeralRuntimeDriver::new(runtime_id.clone());
        driver
            .recover_input_state_persistence_record(bundle)
            .expect("test input-state bundle should recover through generated authority")
    }

    fn applied_system_context_state(
        append: PendingSystemContextAppend,
    ) -> SessionSystemContextState {
        let mut state = SessionSystemContextState::default();
        state
            .stage_append(
                &AppendSystemContextRequest {
                    text: append.text,
                    source: append.source,
                    idempotency_key: append.idempotency_key,
                },
                append.accepted_at,
            )
            .expect("test system-context append should stage");
        state.mark_pending_applied();
        state
    }

    fn start_turn_request_with_system_prompt(
        prompt: &str,
        system_prompt: Option<&str>,
    ) -> StartTurnRequest {
        StartTurnRequest {
            prompt: prompt.to_string().into(),
            system_prompt: system_prompt.map(str::to_string),
            event_tx: None,
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
        }
    }

    #[tokio::test]
    async fn test_persistent_load_persisted_returns_stored_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session = Session::new();
        let id = session.id().clone();
        store.save(&session).await.unwrap();

        // Verify load_persisted returns the session.
        // We can't construct a full PersistentSessionService without a SessionAgentBuilder,
        // so test the store path directly via the same logic.
        let loaded = store.load(&id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().id(), &id);
    }

    #[tokio::test]
    async fn test_persistent_load_persisted_returns_none_for_unknown() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let unknown = SessionId::new();
        let loaded = store.load(&unknown).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_persistent_archive_deletes_from_store() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session = Session::new();
        let id = session.id().clone();
        store.save(&session).await.unwrap();

        // Verify it exists
        assert!(store.load(&id).await.unwrap().is_some());

        // Delete (simulating archive store cleanup)
        store.delete(&id).await.unwrap();

        // Verify it's gone
        assert!(store.load(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_store_checkpointer_saves_session() {
        use meerkat_core::checkpoint::SessionCheckpointer;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gate = Arc::new(super::CheckpointerGate {
            cancelled: tokio::sync::Mutex::new(false),
        });
        let checkpointer = super::StoreCheckpointer {
            store: Arc::clone(&store),
            blob_store: memory_blob_store(),
            event_store: None,
            projector: None,
            gate,
            last_saved_revision: std::sync::Mutex::new(None),
        };

        let mut session = Session::new();
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("hello".to_string()),
        ));

        // Checkpoint should persist the session
        checkpointer.checkpoint(&session).await;

        let loaded = store.load(session.id()).await.unwrap();
        assert!(
            loaded.is_some(),
            "session should be persisted after checkpoint"
        );
        let loaded = loaded.unwrap();
        assert_eq!(loaded.id(), session.id());
        assert_eq!(loaded.messages().len(), session.messages().len());
    }

    #[tokio::test]
    async fn test_store_checkpointer_suppressed_after_cancellation() {
        use meerkat_core::checkpoint::SessionCheckpointer;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gate = Arc::new(super::CheckpointerGate {
            cancelled: tokio::sync::Mutex::new(false),
        });
        let checkpointer = super::StoreCheckpointer {
            store: Arc::clone(&store),
            blob_store: memory_blob_store(),
            event_store: None,
            projector: None,
            gate: Arc::clone(&gate),
            last_saved_revision: std::sync::Mutex::new(None),
        };

        let mut session = Session::new();
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("hello".to_string()),
        ));

        // First checkpoint should persist (message count changed)
        checkpointer.checkpoint(&session).await;
        assert!(store.load(session.id()).await.unwrap().is_some());

        // Simulate archive: acquire gate, set cancelled, delete
        {
            let mut guard = gate.cancelled.lock().await;
            *guard = true;
            store.delete(session.id()).await.unwrap();
        }

        // Checkpoint after cancellation should be a no-op
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("world".to_string()),
        ));
        checkpointer.checkpoint(&session).await;
        assert!(
            store.load(session.id()).await.unwrap().is_none(),
            "cancelled checkpointer should not write session back"
        );
    }

    #[tokio::test]
    async fn test_store_checkpointer_skips_unchanged_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gate = Arc::new(super::CheckpointerGate {
            cancelled: tokio::sync::Mutex::new(false),
        });
        let checkpointer = super::StoreCheckpointer {
            store: Arc::clone(&store),
            blob_store: memory_blob_store(),
            event_store: None,
            projector: None,
            gate,
            last_saved_revision: std::sync::Mutex::new(None),
        };

        let mut session = Session::new();
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("hello".to_string()),
        ));

        // First checkpoint saves (message count changed from 0 -> 1)
        checkpointer.checkpoint(&session).await;
        assert!(store.load(session.id()).await.unwrap().is_some());

        // Delete from store to detect whether the next checkpoint writes
        store.delete(session.id()).await.unwrap();

        // Second checkpoint with same session is skipped (count still 1)
        checkpointer.checkpoint(&session).await;
        assert!(
            store.load(session.id()).await.unwrap().is_none(),
            "unchanged session should not be re-saved"
        );

        // Add a message and checkpoint again — should save
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("world".to_string()),
        ));
        checkpointer.checkpoint(&session).await;
        assert!(
            store.load(session.id()).await.unwrap().is_some(),
            "changed session should be saved"
        );
    }

    #[tokio::test]
    async fn test_service_installed_store_checkpointer_saves_with_and_without_runtime_store() {
        for runtime_backed in [false, true] {
            let mode = if runtime_backed {
                "runtime-backed"
            } else {
                "runtime-less"
            };
            let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
            let runtime_store = runtime_backed.then(|| Arc::new(InMemoryRuntimeStore::new()));
            let service_runtime_store = runtime_store
                .as_ref()
                .map(|store| Arc::clone(store) as Arc<dyn RuntimeStore>);
            let builder = CapturingCheckpointerBuilder::new();
            let captured = Arc::clone(&builder.captured);
            let service = PersistentSessionService::new(
                builder,
                4,
                Arc::clone(&store),
                service_runtime_store,
                memory_blob_store(),
            );

            let result = service
                .create_session(create_request(
                    &format!("{mode} deferred create"),
                    InitialTurnPolicy::Defer,
                ))
                .await
                .unwrap_or_else(|err| panic!("{mode} deferred create should succeed: {err:?}"));
            let checkpointer = captured
                .lock()
                .await
                .clone()
                .unwrap_or_else(|| panic!("{mode} create should install a checkpointer"));
            let mut session = store
                .load(&result.session_id)
                .await
                .expect("store load should succeed")
                .unwrap_or_else(|| panic!("{mode} create should persist an initial projection"));
            let baseline_len = session.messages().len();
            session.push(Message::User(UserMessage::text(format!(
                "{mode} checkpointed turn"
            ))));

            checkpointer.checkpoint(&session).await;

            let persisted = store
                .load(&result.session_id)
                .await
                .expect("store load should succeed")
                .unwrap_or_else(|| panic!("{mode} checkpoint should persist projection"));
            assert_eq!(
                persisted.messages().len(),
                baseline_len + 1,
                "{mode} installed checkpointer should save changed sessions"
            );
        }
    }

    #[tokio::test]
    async fn test_runtime_store_backed_service_checkpointer_respects_cancellation() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let builder = CapturingCheckpointerBuilder::new();
        let captured = Arc::clone(&builder.captured);
        let service = PersistentSessionService::new(
            builder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "runtime-backed deferred create",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("runtime-backed deferred create should succeed");
        let checkpointer = captured
            .lock()
            .await
            .clone()
            .expect("runtime-backed create should install a checkpointer");
        let mut session = store
            .load(&result.session_id)
            .await
            .expect("store load should succeed")
            .expect("create should persist an initial projection");
        let baseline_len = session.messages().len();

        service.cancel_all_checkpointers().await;
        session.push(Message::User(UserMessage::text(
            "checkpoint after cancellation must not save".to_string(),
        )));

        checkpointer.checkpoint(&session).await;

        let persisted = store
            .load(&result.session_id)
            .await
            .expect("store load should succeed")
            .expect("cancelled checkpoint should leave initial projection present");
        assert_eq!(
            persisted.messages().len(),
            baseline_len,
            "cancelled runtime-backed checkpointer must not write a later projection"
        );
    }

    #[tokio::test]
    async fn test_create_session_externalizes_inline_images_in_persisted_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let blob_store = memory_blob_store();
        let service = PersistentSessionService::new(
            ImagePreservingBuilder,
            4,
            Arc::clone(&store),
            None,
            blob_store.clone(),
        );

        let result = service
            .create_session(CreateSessionRequest {
                prompt: image_prompt("create"),
                ..create_request("ignored", InitialTurnPolicy::RunImmediately)
            })
            .await
            .expect("create_session should succeed");

        let persisted = store
            .load(&result.session_id)
            .await
            .expect("load should succeed")
            .expect("persisted session should exist");

        assert_no_inline_images_in_session(&persisted);
        let blob_id = persisted
            .messages()
            .iter()
            .find_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Image {
                        data: ImageData::Blob { blob_id },
                        ..
                    } => Some(blob_id.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .expect("persisted image should be externalized");
        assert!(
            blob_store
                .get(&blob_id)
                .await
                .expect("blob should be persisted")
                .data
                .contains("base64-create")
        );
    }

    #[tokio::test]
    async fn test_start_turn_externalizes_new_inline_images_in_persisted_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            ImagePreservingBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        service
            .start_turn(
                &created.session_id,
                StartTurnRequest {
                    prompt: image_prompt("turn"),
                    ..start_turn_request("ignored")
                },
            )
            .await
            .expect("start_turn should succeed");

        let persisted = store
            .load(&created.session_id)
            .await
            .expect("load should succeed")
            .expect("persisted session should exist");
        assert_no_inline_images_in_session(&persisted);
    }

    #[tokio::test]
    async fn test_save_normalized_session_bridges_previous_inline_media_projection() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            ImagePreservingBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let mut previous = store
            .load(&created.session_id)
            .await
            .expect("load should succeed")
            .expect("created session should exist");
        previous.push(Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Text {
                text: "inline image from legacy projection".to_string(),
            },
            inline_image_block("legacy-projection"),
        ])));
        store
            .save_authoritative_projection(&previous)
            .await
            .expect("test setup should install legacy inline projection");

        let mut incoming = previous.clone();
        incoming.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "after image".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));

        service
            .save_normalized_session(incoming)
            .await
            .expect("media-normalized append should bridge the previous projection");
        let saved = store
            .load(&created.session_id)
            .await
            .expect("load should succeed")
            .expect("saved session should exist");
        assert_no_inline_images_in_session(&saved);
        assert_eq!(saved.messages().len(), previous.messages().len() + 1);
    }

    #[tokio::test]
    async fn test_save_normalized_session_bridges_compaction_after_uncheckpointed_append() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let previous = store
            .load(&created.session_id)
            .await
            .expect("load should succeed")
            .expect("created session should exist");

        let mut parent = previous.clone();
        parent.set_system_prompt("refreshed runtime system projection".to_string());
        parent.push(Message::User(UserMessage::text(
            "runtime-only prompt".to_string(),
        )));
        parent.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "runtime-only answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let parent_revision = parent.transcript_revision().expect("parent revision");

        let mut incoming = parent.clone();
        let mut replacement = vec![
            parent.messages()[0].clone(),
            Message::User(UserMessage::text(
                "[Context compacted] fast regression summary".to_string(),
            )),
        ];
        replacement.extend_from_slice(&parent.messages()[1..]);
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: parent.messages().len(),
                },
                replacement,
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .expect("compaction rewrite should commit");
        let incoming_revision = incoming.transcript_revision().expect("incoming revision");

        service
            .save_normalized_session(incoming)
            .await
            .expect("compaction after uncheckpointed append should persist through a bridge");
        let saved = store
            .load(&created.session_id)
            .await
            .expect("load should succeed")
            .expect("saved session should exist");
        assert_eq!(
            saved.transcript_revision().expect("saved revision"),
            incoming_revision
        );
    }

    #[tokio::test]
    async fn test_projection_continuity_save_normalized_session_bridges_inline_media_compaction_history()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let blob_store = memory_blob_store();
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir()?;
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            Arc::clone(&blob_store),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await?;
        let mut previous = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("created session should exist"))?;
        previous.push(Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Text {
                text: "inline image from legacy projection".to_string(),
            },
            inline_image_block("legacy-compaction-parent"),
        ])));
        store.save_authoritative_projection(&previous).await?;

        let mut normalized_parent = previous.clone();
        normalized_parent
            .externalize_media(blob_store.as_ref(), 0)
            .await?;
        let normalized_parent_revision = normalized_parent.transcript_revision()?;

        let mut incoming = normalized_parent.clone();
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: normalized_parent.messages().len(),
                },
                vec![Message::User(UserMessage::text(
                    "[Context compacted] normalized legacy image prompt".to_string(),
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(normalized_parent_revision.clone()),
            )
            .map_err(|err| std::io::Error::other(format!("rewrite should commit: {err}")))?;
        let incoming_revision = incoming.transcript_revision()?;

        service.save_normalized_session(incoming).await?;

        let saved = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("saved session should exist"))?;
        assert_eq!(saved.transcript_revision()?, incoming_revision);
        assert_no_inline_images_in_session(&saved);
        let events = service
            .event_log_read_from(&created.session_id, 1)
            .await?
            .ok_or_else(|| std::io::Error::other("event projection should be installed"))?;
        let audit = events
            .iter()
            .find_map(|stored| match &stored.event {
                AgentEvent::TranscriptRewriteCommitted { record, .. } => Some(record),
                _ => None,
            })
            .ok_or_else(|| std::io::Error::other("rewrite audit event should be appended"))?;
        assert_eq!(audit.commit.parent_revision, normalized_parent_revision);
        assert_eq!(audit.commit.revision, incoming_revision);
        Ok(())
    }

    #[tokio::test]
    async fn test_projection_continuity_verified_projection_refuses_unaudited_rewrite_commits()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let blob_store = memory_blob_store();

        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text(
            "persisted prompt".to_string(),
        )));
        store.save(&previous).await?;
        let previous_revision = previous.transcript_revision()?;

        let mut parent = previous.clone();
        parent.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "runtime-only answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let parent_revision = parent.transcript_revision()?;

        let mut incoming = parent.clone();
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: parent.messages().len(),
                },
                vec![Message::User(UserMessage::text(
                    "[Context compacted] unaudited summary".to_string(),
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .map_err(|err| std::io::Error::other(format!("rewrite should commit: {err}")))?;

        let saved = save_verified_transcript_history_projection(
            store.as_ref(),
            memory_blob_store().as_ref(),
            &incoming,
            &previous,
            previous_revision.clone(),
        )
        .await?;
        assert!(
            !saved,
            "projection-only bridge must not introduce rewrite commits without audit authority"
        );

        save_session_projection_with_storage_normalization_bridge(
            store.as_ref(),
            blob_store.as_ref(),
            &incoming,
        )
        .await
        .expect_err("projection-only bridge should not bypass audit for new rewrite commits");

        let persisted = store
            .load(previous.id())
            .await?
            .ok_or_else(|| std::io::Error::other("previous session should remain persisted"))?;
        assert_eq!(persisted.transcript_revision()?, previous_revision);
        Ok(())
    }

    #[tokio::test]
    async fn test_projection_continuity_checkpoint_committed_runtime_snapshot_audits_inline_media_compaction_history()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let blob_store = memory_blob_store();
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir()?;
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            Arc::clone(&blob_store),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await?;
        let mut previous = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("created session should exist"))?;
        previous.push(Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Text {
                text: "inline image from legacy checkpoint projection".to_string(),
            },
            inline_image_block("legacy-checkpoint-compaction-parent"),
        ])));
        store.save_authoritative_projection(&previous).await?;

        let mut normalized_parent = previous.clone();
        normalized_parent
            .externalize_media(blob_store.as_ref(), 0)
            .await?;
        let normalized_parent_revision = normalized_parent.transcript_revision()?;

        let mut incoming = normalized_parent.clone();
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: normalized_parent.messages().len(),
                },
                vec![Message::User(UserMessage::text(
                    "[Context compacted] checkpoint normalized legacy image prompt".to_string(),
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(normalized_parent_revision.clone()),
            )
            .map_err(|err| std::io::Error::other(format!("rewrite should commit: {err}")))?;
        let incoming_revision = incoming.transcript_revision()?;
        let snapshot = serde_json::to_vec(&incoming)?;
        let runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&created.session_id);
        runtime_store
            .commit_session_snapshot(
                &runtime_id,
                SessionDelta {
                    session_snapshot: snapshot.clone(),
                },
            )
            .await?;

        service
            .checkpoint_committed_runtime_session_snapshot(&created.session_id, &snapshot)
            .await?;

        let saved = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("saved session should exist"))?;
        assert_eq!(saved.transcript_revision()?, incoming_revision);
        assert_no_inline_images_in_session(&saved);
        let events = service
            .event_log_read_from(&created.session_id, 1)
            .await?
            .ok_or_else(|| std::io::Error::other("event projection should be installed"))?;
        let audit = events
            .iter()
            .find_map(|stored| match &stored.event {
                AgentEvent::TranscriptRewriteCommitted { record, .. } => Some(record),
                _ => None,
            })
            .ok_or_else(|| std::io::Error::other("rewrite audit event should be appended"))?;
        assert_eq!(audit.commit.parent_revision, normalized_parent_revision);
        assert_eq!(audit.commit.revision, incoming_revision);
        Ok(())
    }

    #[tokio::test]
    async fn test_checkpoint_committed_runtime_snapshot_bridges_externalized_compaction_append() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let blob_store = memory_blob_store();
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            Arc::clone(&blob_store),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let previous = store
            .load(&created.session_id)
            .await
            .expect("load should succeed")
            .expect("created session should exist");

        let mut parent = previous.clone();
        parent.set_system_prompt("refreshed runtime system projection".to_string());
        parent.push(Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Text {
                text: "runtime-only image prompt".to_string(),
            },
            inline_image_block("runtime-parent"),
        ])));
        parent.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "runtime-only answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let parent_revision = parent.transcript_revision().expect("parent revision");

        let mut incoming = parent.clone();
        let mut replacement = vec![
            parent.messages()[0].clone(),
            Message::User(UserMessage::text(
                "[Context compacted] retained image prompt".to_string(),
            )),
        ];
        replacement.extend_from_slice(&parent.messages()[1..]);
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: parent.messages().len(),
                },
                replacement,
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .expect("compaction rewrite should commit");
        incoming.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "final peer response".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        incoming
            .externalize_media(blob_store.as_ref(), 0)
            .await
            .expect("runtime snapshot normalization should succeed");
        let incoming_revision = incoming.transcript_revision().expect("incoming revision");
        let snapshot = serde_json::to_vec(&incoming).expect("snapshot should serialize");

        service
            .checkpoint_committed_runtime_session_snapshot(&created.session_id, &snapshot)
            .await
            .expect("checkpoint should bridge externalized compaction append");
        let saved = store
            .load(&created.session_id)
            .await
            .expect("load should succeed")
            .expect("saved session should exist");
        assert_no_inline_images_in_session(&saved);
        assert_eq!(
            saved.transcript_revision().expect("saved revision"),
            incoming_revision
        );
    }

    #[tokio::test]
    async fn test_projection_continuity_checkpoint_committed_runtime_snapshot_rejects_unrelated_history_projection()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await?;
        let baseline = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("created session should exist"))?;

        let mut newer_projection = baseline.clone();
        newer_projection.push(Message::User(UserMessage::text(
            "newer persisted turn that stale runtime snapshots must preserve".to_string(),
        )));
        let newer_revision = newer_projection.transcript_revision()?;
        store
            .save_authoritative_projection(&newer_projection)
            .await?;

        let mut stale_parent = baseline;
        stale_parent.set_system_prompt("stale runtime system projection".to_string());
        stale_parent.push(Message::User(UserMessage::text(
            "stale runtime branch prompt".to_string(),
        )));
        stale_parent.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "stale runtime branch answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let stale_parent_revision = stale_parent.transcript_revision()?;

        let mut incoming = stale_parent.clone();
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: stale_parent.messages().len(),
                },
                vec![Message::User(UserMessage::text(
                    "[Context compacted] stale runtime branch".to_string(),
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(stale_parent_revision.clone()),
            )
            .map_err(|err| std::io::Error::other(format!("rewrite should commit: {err}")))?;
        incoming.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "stale branch final response".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let incoming_revision = incoming.transcript_revision()?;
        let snapshot = serde_json::to_vec(&incoming)?;
        runtime_store
            .commit_session_snapshot(
                &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(
                    &created.session_id,
                ),
                SessionDelta {
                    session_snapshot: snapshot.clone(),
                },
            )
            .await?;

        let checkpoint_result = service
            .checkpoint_committed_runtime_session_snapshot(&created.session_id, &snapshot)
            .await;
        assert!(
            checkpoint_result.is_err(),
            "stale runtime history must not bypass the persisted-row continuity guard"
        );

        let saved = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("persisted session should remain present"))?;
        assert_eq!(saved.transcript_revision()?, newer_revision);
        assert_ne!(saved.transcript_revision()?, incoming_revision);
        assert!(
            runtime_store
                .load_session_snapshot(
                    &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(
                        &created.session_id,
                    ),
                )
                .await?
                .is_none(),
            "rejected checkpoint snapshot should be quarantined instead of restored from a possibly-mutated projection"
        );
        let recovered = service
            .load_authoritative_session_base(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("store fallback should recover latest row"))?;
        assert_eq!(recovered.transcript_revision()?, newer_revision);
        assert_ne!(recovered.transcript_revision()?, incoming_revision);
        Ok(())
    }

    #[tokio::test]
    async fn test_projection_continuity_checkpoint_restore_failure_quarantines_runtime_snapshot()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gated_runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = gated_runtime_store.clone();
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await?;
        let baseline = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("created session should exist"))?;

        let mut newer_projection = baseline.clone();
        newer_projection.push(Message::User(UserMessage::text(
            "newer persisted turn that must survive quarantine".to_string(),
        )));
        let newer_revision = newer_projection.transcript_revision()?;
        store
            .save_authoritative_projection(&newer_projection)
            .await?;

        let mut incoming = baseline;
        incoming.push(Message::User(UserMessage::text(
            "stale runtime branch that should be rejected".to_string(),
        )));
        let incoming_revision = incoming.transcript_revision()?;
        let snapshot = serde_json::to_vec(&incoming)?;
        let runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&created.session_id);
        runtime_store
            .commit_session_snapshot(
                &runtime_id,
                SessionDelta {
                    session_snapshot: snapshot.clone(),
                },
            )
            .await?;

        gated_runtime_store.set_fail_snapshot_commits(true);
        let checkpoint_result = service
            .checkpoint_committed_runtime_session_snapshot(&created.session_id, &snapshot)
            .await;
        assert!(
            checkpoint_result.is_err(),
            "projection rejection plus restore failure must fail closed"
        );
        let saved = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("persisted session should remain present"))?;
        assert_eq!(saved.transcript_revision()?, newer_revision);
        assert_ne!(saved.transcript_revision()?, incoming_revision);
        assert!(
            runtime_store
                .load_session_snapshot(&runtime_id)
                .await?
                .is_none(),
            "failed restore must quarantine the rejected runtime snapshot"
        );
        let recovered = service
            .load_authoritative_session_base(&created.session_id)
            .await?
            .ok_or_else(|| {
                std::io::Error::other("quarantined runtime snapshot should fall back to store row")
            })?;
        assert_eq!(recovered.transcript_revision()?, newer_revision);
        Ok(())
    }

    #[tokio::test]
    async fn test_projection_continuity_checkpoint_audit_rollback_failure_quarantines_store_projection()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let store = Arc::new(FailAuthoritativeProjectionStore::new());
        let store_trait: Arc<dyn SessionStore> = store.clone();
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir()?;
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store_trait),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await?;
        let original = store_trait
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("created session should exist"))?;
        let parent_revision = original.transcript_revision()?;

        let mut incoming = original.clone();
        let commit = incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: original.messages().len(),
                },
                vec![Message::User(UserMessage::text(
                    "[Context compacted] unaudited checkpoint".to_string(),
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .map_err(|err| std::io::Error::other(format!("rewrite should commit: {err}")))?;
        let snapshot = serde_json::to_vec(&incoming)?;
        let runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&created.session_id);
        runtime_store
            .commit_session_snapshot(
                &runtime_id,
                SessionDelta {
                    session_snapshot: snapshot.clone(),
                },
            )
            .await?;

        event_store.fail_appends();
        store.fail_authoritative_projection_cas();
        let checkpoint_result = service
            .checkpoint_committed_runtime_session_snapshot(&created.session_id, &snapshot)
            .await;
        let err = checkpoint_result.expect_err(
            "checkpoint rewrite must fail closed when audit append and projection rollback fail",
        );
        assert!(
            err.to_string()
                .contains("unaudited projection was quarantined"),
            "unexpected error: {err}"
        );
        assert!(
            store_trait.load(&created.session_id).await?.is_none(),
            "failed audit rollback must not leave the unaudited rewrite as store fallback"
        );
        assert!(
            runtime_store
                .load_session_snapshot(&runtime_id)
                .await?
                .is_none(),
            "runtime snapshot should be quarantined after failed checkpoint projection update"
        );
        assert_ne!(original.transcript_revision()?, commit.revision);
        Ok(())
    }

    #[tokio::test]
    async fn test_projection_continuity_fail_closed_cleanup_preserves_newer_runtime_snapshot()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        );

        let mut persisted = Session::new();
        persisted.push(Message::User(UserMessage::text("persisted".to_string())));
        store.save(&persisted).await?;

        let mut rejected = persisted.clone();
        rejected.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "rejected runtime snapshot".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let rejected_snapshot = serde_json::to_vec(&rejected)?;

        let mut newer = rejected.clone();
        newer.push(Message::User(UserMessage::text(
            "newer runtime authority".to_string(),
        )));
        let newer_snapshot = serde_json::to_vec(&newer)?;
        let runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(persisted.id());
        runtime_store
            .commit_session_snapshot(
                &runtime_id,
                SessionDelta {
                    session_snapshot: newer_snapshot.clone(),
                },
            )
            .await?;

        let err = service
            .fail_closed_runtime_projection_update(
                persisted.id(),
                SessionStoreError::Internal("forced projection rejection".to_string()),
                Some(rejected_snapshot.as_slice()),
            )
            .await;
        assert!(
            err.to_string().contains("forced projection rejection"),
            "unexpected error: {err}"
        );
        let current = runtime_store
            .load_session_snapshot(&runtime_id)
            .await?
            .ok_or_else(|| std::io::Error::other("runtime snapshot should remain present"))?;
        assert_eq!(current, newer_snapshot);
        Ok(())
    }

    #[tokio::test]
    async fn test_projection_continuity_fail_closed_quarantine_preserves_newer_runtime_snapshot()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gated_runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = gated_runtime_store.clone();
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        );

        let mut persisted = Session::new();
        persisted.push(Message::User(UserMessage::text("persisted".to_string())));
        store.save(&persisted).await?;

        let mut rejected = persisted.clone();
        rejected.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "rejected runtime snapshot".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let rejected_snapshot = serde_json::to_vec(&rejected)?;

        let mut newer = rejected.clone();
        newer.push(Message::User(UserMessage::text(
            "newer runtime authority before quarantine".to_string(),
        )));
        let newer_snapshot = serde_json::to_vec(&newer)?;
        let runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(persisted.id());
        runtime_store
            .commit_session_snapshot(
                &runtime_id,
                SessionDelta {
                    session_snapshot: newer_snapshot.clone(),
                },
            )
            .await?;

        gated_runtime_store.set_fail_snapshot_commits(true);
        let err = service
            .fail_closed_runtime_projection_update(
                persisted.id(),
                SessionStoreError::Internal("forced projection rejection".to_string()),
                Some(rejected_snapshot.as_slice()),
            )
            .await;
        assert!(
            err.to_string().contains("forced projection rejection"),
            "unexpected error: {err}"
        );
        let current = runtime_store
            .load_session_snapshot(&runtime_id)
            .await?
            .ok_or_else(|| std::io::Error::other("newer runtime snapshot should remain present"))?;
        assert_eq!(current, newer_snapshot);
        Ok(())
    }

    #[tokio::test]
    async fn test_projection_continuity_runtime_audit_rollback_preserves_newer_runtime_snapshot()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gated_runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = gated_runtime_store.clone();
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir()?;
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await?;
        let original = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("created session should exist"))?;
        let parent_revision = original.transcript_revision()?;

        let mut incoming = original.clone();
        let commit = incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: original.messages().len(),
                },
                vec![Message::User(UserMessage::text(
                    "[Context compacted] audit failure".to_string(),
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .map_err(|err| std::io::Error::other(format!("rewrite should commit: {err}")))?;
        let runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&created.session_id);
        runtime_store
            .commit_session_snapshot(
                &runtime_id,
                SessionDelta {
                    session_snapshot: serde_json::to_vec(&original)?,
                },
            )
            .await?;

        let mut newer = incoming.clone();
        newer.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "newer runtime authority".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let newer_snapshot = serde_json::to_vec(&newer)?;
        gated_runtime_store
            .interlope_before_snapshot_replace(runtime_id.clone(), newer_snapshot.clone())
            .await;

        event_store.fail_appends();
        let save_result = service.save_normalized_session(incoming).await;
        let err = save_result
            .expect_err("rewrite snapshot persistence must fail closed when audit append fails");
        assert!(
            err.to_string()
                .contains("synthetic transcript rewrite audit append failure"),
            "unexpected error: {err}"
        );

        let saved = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("persisted session should remain present"))?;
        assert_eq!(
            saved.transcript_revision()?,
            original.transcript_revision()?
        );
        assert_ne!(saved.transcript_revision()?, commit.revision);
        let current = runtime_store
            .load_session_snapshot(&runtime_id)
            .await?
            .ok_or_else(|| std::io::Error::other("newer runtime snapshot should remain present"))?;
        assert_eq!(current, newer_snapshot);
        Ok(())
    }

    #[tokio::test]
    async fn test_projection_continuity_runtime_audit_quarantine_preserves_newer_runtime_snapshot()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gated_runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = gated_runtime_store.clone();
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir()?;
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await?;
        let original = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("created session should exist"))?;
        let parent_revision = original.transcript_revision()?;

        let mut incoming = original.clone();
        let commit = incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: original.messages().len(),
                },
                vec![Message::User(UserMessage::text(
                    "[Context compacted] audit failure".to_string(),
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .map_err(|err| std::io::Error::other(format!("rewrite should commit: {err}")))?;
        let runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&created.session_id);
        runtime_store
            .commit_session_snapshot(
                &runtime_id,
                SessionDelta {
                    session_snapshot: serde_json::to_vec(&original)?,
                },
            )
            .await?;

        let mut newer = incoming.clone();
        newer.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "newer runtime authority before quarantine".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let newer_snapshot = serde_json::to_vec(&newer)?;
        gated_runtime_store.set_fail_snapshot_replaces(true);
        gated_runtime_store
            .interlope_before_snapshot_clear(runtime_id.clone(), newer_snapshot.clone())
            .await;

        event_store.fail_appends();
        let save_result = service.save_normalized_session(incoming).await;
        let err = save_result.expect_err(
            "rewrite snapshot persistence must fail closed when audit append and rollback fail",
        );
        assert!(
            err.to_string().contains("runtime snapshot was quarantined"),
            "unexpected error: {err}"
        );

        let saved = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("persisted session should remain present"))?;
        assert_eq!(
            saved.transcript_revision()?,
            original.transcript_revision()?
        );
        assert_ne!(saved.transcript_revision()?, commit.revision);
        let current = runtime_store
            .load_session_snapshot(&runtime_id)
            .await?
            .ok_or_else(|| std::io::Error::other("newer runtime snapshot should remain present"))?;
        assert_eq!(current, newer_snapshot);
        Ok(())
    }

    #[tokio::test]
    async fn test_projection_continuity_save_normalized_session_rejects_runtime_rewrite_chain_not_connected_to_store()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await?;
        let baseline = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("created session should exist"))?;

        let mut newer_projection = baseline.clone();
        newer_projection.push(Message::User(UserMessage::text(
            "newer persisted turn that runtime rewrite persistence must preserve".to_string(),
        )));
        let newer_revision = newer_projection.transcript_revision()?;
        store
            .save_authoritative_projection(&newer_projection)
            .await?;

        let mut stale_parent = baseline;
        stale_parent.set_system_prompt("stale runtime system projection".to_string());
        stale_parent.push(Message::User(UserMessage::text(
            "stale runtime branch prompt".to_string(),
        )));
        stale_parent.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "stale runtime branch answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let stale_parent_revision = stale_parent.transcript_revision()?;
        runtime_store
            .commit_session_snapshot(
                &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(
                    &created.session_id,
                ),
                SessionDelta {
                    session_snapshot: serde_json::to_vec(&stale_parent)?,
                },
            )
            .await?;

        let mut incoming = stale_parent.clone();
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: stale_parent.messages().len(),
                },
                vec![Message::User(UserMessage::text(
                    "[Context compacted] stale runtime branch".to_string(),
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(stale_parent_revision.clone()),
            )
            .map_err(|err| std::io::Error::other(format!("rewrite should commit: {err}")))?;
        let incoming_revision = incoming.transcript_revision()?;

        let save_result = service.save_normalized_session(incoming).await;
        assert!(
            save_result.is_err(),
            "runtime rewrite-chain persistence must not overwrite a newer SessionStore row"
        );

        let runtime_saved =
            PersistentSessionService::<DummyBuilder>::load_runtime_session_snapshot(
                &runtime_store,
                &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(
                    &created.session_id,
                ),
            )
            .await?
            .ok_or_else(|| std::io::Error::other("runtime snapshot should remain present"))?;
        assert_eq!(runtime_saved.transcript_revision()?, stale_parent_revision);
        assert_ne!(runtime_saved.transcript_revision()?, incoming_revision);

        let saved = store
            .load(&created.session_id)
            .await?
            .ok_or_else(|| std::io::Error::other("persisted session should remain present"))?;
        assert_eq!(saved.transcript_revision()?, newer_revision);
        assert_ne!(saved.transcript_revision()?, incoming_revision);
        Ok(())
    }

    #[tokio::test]
    async fn test_apply_runtime_turn_externalizes_inline_images_in_runtime_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            ImagePreservingBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        let run_id = RunId::new();
        let mut req = runtime_content_turn_request("ignored");
        req.prompt = image_prompt("runtime");
        service
            .apply_runtime_turn(
                &created.session_id,
                run_id,
                req,
                RunApplyBoundary::RunStart,
                vec![],
            )
            .await
            .expect("apply_runtime_turn should succeed");

        let runtime_id =
            super::PersistentSessionService::<ImagePreservingBuilder>::runtime_id_for_session(
                &created.session_id,
            );
        let snapshot = runtime_store
            .load_session_snapshot(&runtime_id)
            .await
            .expect("runtime snapshot load should succeed")
            .expect("runtime snapshot should exist");
        let persisted: Session =
            serde_json::from_slice(&snapshot).expect("runtime snapshot should deserialize");
        assert_no_inline_images_in_session(&persisted);
    }

    #[tokio::test]
    async fn test_apply_runtime_turn_resume_pending_without_boundary_is_not_run_result() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let run_id = RunId::new();
        let contributing_input_ids = vec![meerkat_core::lifecycle::InputId::new()];
        let mut req = start_turn_request("resume");
        req.runtime.turn_metadata = Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending),
                ..Default::default()
            },
        );

        let output = service
            .apply_runtime_turn(
                &created.session_id,
                run_id.clone(),
                req,
                RunApplyBoundary::RunStart,
                contributing_input_ids.clone(),
            )
            .await
            .expect("runtime apply should commit typed no-pending terminal");

        assert_eq!(output.receipt.run_id, run_id);
        assert_eq!(
            output.receipt.contributing_input_ids,
            contributing_input_ids
        );
        assert!(matches!(
            output.terminal,
            Some(CoreApplyTerminal::NoPendingBoundary)
        ));
    }

    #[tokio::test]
    async fn test_apply_runtime_turn_rejects_missing_execution_kind_before_no_pending_terminal() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        let error = service
            .apply_runtime_turn(
                &created.session_id,
                RunId::new(),
                start_turn_request(""),
                RunApplyBoundary::RunStart,
                vec![meerkat_core::lifecycle::InputId::new()],
            )
            .await
            .expect_err(
                "runtime apply must reject missing execution kind before no-pending commit",
            );

        assert!(
            error.to_string().contains("runtime_execution_kind not set"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn test_failed_runtime_turn_discards_live_pre_turn_context() {
        use futures::StreamExt;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            FailingRunBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let mut events = service
            .subscribe_session_events(&created.session_id)
            .await
            .expect("subscribe_session_events");

        let mut req = start_turn_request("runtime failed turn");
        req.runtime.pre_turn_context_appends = vec![PendingSystemContextAppend {
            text: "failed-turn context must not leak".to_string(),
            source: Some("peer_response_terminal:test:req".to_string()),
            idempotency_key: Some("peer_response_terminal:test:req".to_string()),
            accepted_at: meerkat_core::time_compat::SystemTime::now(),
        }];
        req.runtime.turn_metadata = Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn),
                ..Default::default()
            },
        );

        let error = service
            .apply_runtime_turn(
                &created.session_id,
                RunId::new(),
                req,
                RunApplyBoundary::RunStart,
                vec![meerkat_core::lifecycle::InputId::new()],
            )
            .await
            .expect_err("synthetic run failure should propagate");

        assert!(
            error.to_string().contains("synthetic run failure"),
            "unexpected error: {error}"
        );
        let event =
            tokio::time::timeout(std::time::Duration::from_millis(100), events.next()).await;
        assert!(
            matches!(event, Err(_) | Ok(None)),
            "failed runtime turn must not publish pre-turn context lifecycle events: {event:?}"
        );
        assert!(
            !service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "failed runtime turn must discard the live session carrying uncommitted pre-turn context"
        );

        let authoritative = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("created session should remain durable");
        assert!(
            authoritative
                .system_context_state()
                .is_none_or(|state| state.applied().is_empty()),
            "failed pre-turn context must not be committed to durable session state"
        );
        assert!(
            authoritative.messages().iter().all(|message| {
                !format!("{message:?}").contains("failed-turn context must not leak")
            }),
            "failed pre-turn context must not be committed into durable messages"
        );
    }

    #[tokio::test]
    async fn test_failed_runtime_turn_output_commit_discards_live_session() {
        let store = Arc::new(FailSaveStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        store.set_fail_save(true);
        let error = service
            .apply_runtime_turn(
                &created.session_id,
                RunId::new(),
                runtime_content_turn_request("runtime turn with failed commit"),
                RunApplyBoundary::RunStart,
                vec![meerkat_core::lifecycle::InputId::new()],
            )
            .await
            .expect_err("runtime output commit failure should propagate");

        assert!(
            matches!(error, SessionError::Store(_)),
            "expected store error after runtime output commit failure, got {error:?}"
        );
        assert!(
            !service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "failed runtime output commit must discard the mutated live session"
        );

        store.set_fail_save(false);
        let persisted = store
            .load(&created.session_id)
            .await
            .expect("load should succeed")
            .expect("deferred session row should remain durable");
        assert!(
            persisted.messages().is_empty(),
            "failed runtime output commit must not persist the mutated turn"
        );
    }

    #[tokio::test]
    async fn test_context_only_runtime_apply_defers_runtime_store_commit_to_machine() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        runtime_store.reset_boundary_commits().await;
        let run_id = RunId::new();
        let output = service
            .apply_runtime_context_appends(
                &created.session_id,
                run_id.clone(),
                vec![PendingSystemContextAppend {
                    text: "context-only apply waits for machine commit".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("deferred-context-only-apply".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await
            .expect("session runtime apply should build output without committing runtime store");

        assert_eq!(output.receipt.run_id, run_id);
        assert!(
            runtime_store.boundary_commits().await.is_empty(),
            "session service must not use the old runtime boundary commit path"
        );
        assert!(
            runtime_store
                .load_boundary_receipt(
                    &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(
                        &created.session_id
                    ),
                    &run_id,
                    0,
                )
                .await
                .expect("receipt lookup should succeed")
                .is_none(),
            "runtime receipt must be durable only after the machine-owned commit"
        );
        let staged_snapshot: Session = serde_json::from_slice(
            output
                .session_snapshot
                .as_deref()
                .expect("runtime output should carry a staged session snapshot"),
        )
        .expect("staged session snapshot should deserialize");
        assert!(
            staged_snapshot.messages().iter().any(|message| {
                format!("{message:?}").contains("context-only apply waits for machine commit")
            }),
            "runtime output should carry the staged context for the machine-owned commit"
        );
        let live_export = service.export_live_session(&created.session_id).await;
        assert!(
            matches!(live_export, Err(SessionError::NotFound { .. })),
            "uncommitted staged context must not export as public live truth"
        );
        assert!(
            service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "context-only staging must retain the mechanical live handle for the runtime machine"
        );
        let authoritative = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("created runtime snapshot should remain durable");
        assert!(
            authoritative.messages().iter().all(|message| {
                !format!("{message:?}").contains("context-only apply waits for machine commit")
            }),
            "durable runtime authority must not expose staged context before the machine commit"
        );
    }

    #[tokio::test]
    async fn test_reserved_context_only_runtime_apply_defers_runtime_store_commit_to_machine() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let admission = service
            .reserve_runtime_turn_admission(&created.session_id)
            .await
            .expect("reserve context-only active admission");

        runtime_store.reset_boundary_commits().await;
        let run_id = RunId::new();
        let output = service
            .apply_runtime_context_appends_with_reserved_admission(
                &created.session_id,
                run_id.clone(),
                vec![PendingSystemContextAppend {
                    text: "reserved context apply waits for machine commit".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("deferred-reserved-context-apply".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                RunApplyBoundary::Immediate,
                vec![InputId::new()],
                admission,
            )
            .await
            .expect("reserved runtime apply should build output without committing runtime store");

        assert_eq!(output.receipt.run_id, run_id);
        assert!(
            runtime_store.boundary_commits().await.is_empty(),
            "reserved session runtime apply must not use the old runtime boundary commit path"
        );
        assert!(
            runtime_store
                .load_boundary_receipt(
                    &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(
                        &created.session_id
                    ),
                    &run_id,
                    0,
                )
                .await
                .expect("receipt lookup should succeed")
                .is_none(),
            "reserved runtime receipt must be durable only after the machine-owned commit"
        );
    }

    #[tokio::test]
    async fn test_runtime_apply_without_machine_commit_fails_closed_live_export() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        let output = service
            .apply_runtime_turn(
                &created.session_id,
                RunId::new(),
                runtime_content_turn_request("uncommitted runtime turn"),
                RunApplyBoundary::Immediate,
                vec![InputId::new()],
            )
            .await
            .expect("runtime apply should build a machine-owned commit output");
        let staged_snapshot: Session = serde_json::from_slice(
            output
                .session_snapshot
                .as_deref()
                .expect("runtime output should carry a staged session snapshot"),
        )
        .expect("staged session snapshot should deserialize");
        assert!(
            !staged_snapshot.messages().is_empty(),
            "runtime apply should mutate live session state before the machine commit"
        );

        let error = service
            .export_live_session(&created.session_id)
            .await
            .expect_err("uncommitted live session must not remain externally visible");
        assert!(
            matches!(error, SessionError::NotFound { .. }),
            "unexpected export error after stale-live discard: {error:?}"
        );
        assert!(
            service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "live export must fail closed without discarding the mechanical live handle"
        );

        let authoritative = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("created runtime snapshot should remain durable");
        assert!(
            authoritative.messages().is_empty(),
            "without the machine atomic commit, durable runtime authority must not expose the staged turn"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_read_uses_durable_authority_without_discarding_live_handle() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        service
            .apply_runtime_turn(
                &created.session_id,
                RunId::new(),
                runtime_content_turn_request("uncommitted runtime turn"),
                RunApplyBoundary::Immediate,
                vec![InputId::new()],
            )
            .await
            .expect("runtime apply should build output without committing authority");

        let view = service
            .read(&created.session_id)
            .await
            .expect("read should fail closed to durable authority");
        assert_eq!(
            view.state.message_count, 0,
            "read() must not publish uncommitted live transcript as durable truth"
        );
        assert!(
            service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "read() must not discard the live handle that owns mechanical capabilities"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_list_uses_durable_summary_without_discarding_live_handle() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        let output = service
            .apply_runtime_turn(
                &created.session_id,
                RunId::new(),
                runtime_content_turn_request("uncommitted runtime turn"),
                RunApplyBoundary::Immediate,
                vec![InputId::new()],
            )
            .await
            .expect("runtime apply should build output without committing authority");
        let staged_snapshot: Session = serde_json::from_slice(
            output
                .session_snapshot
                .as_deref()
                .expect("runtime output should carry a staged session snapshot"),
        )
        .expect("staged session snapshot should deserialize");
        assert!(
            !staged_snapshot.messages().is_empty(),
            "test must create an uncommitted live transcript before list()"
        );

        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should fail closed to durable authority");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == created.session_id)
            .expect("created session should remain discoverable");
        assert_eq!(
            summary.message_count, 0,
            "list() must not publish uncommitted live transcript as summary truth"
        );
        assert!(
            service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "list() must not discard the live handle that owns mechanical capabilities"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_observations_preserve_live_capability_handles() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            CapabilityBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        assert!(
            service.comms_runtime(&created.session_id).await.is_some(),
            "test setup should install a live comms runtime"
        );
        assert!(
            service
                .interaction_event_injector(&created.session_id)
                .await
                .is_some(),
            "test setup should install a live interaction injector"
        );

        service
            .apply_runtime_turn(
                &created.session_id,
                RunId::new(),
                runtime_content_turn_request("uncommitted runtime turn"),
                RunApplyBoundary::Immediate,
                vec![InputId::new()],
            )
            .await
            .expect("runtime apply should build output without committing authority");

        let view = service
            .read(&created.session_id)
            .await
            .expect("read should fail closed to durable authority");
        assert_eq!(view.state.message_count, 0);

        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should fail closed to durable authority");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == created.session_id)
            .expect("created session should remain discoverable");
        assert_eq!(summary.message_count, 0);

        let export = service.export_live_session(&created.session_id).await;
        assert!(
            matches!(export, Err(SessionError::NotFound { .. })),
            "live export must fail closed while durable authority is behind"
        );

        assert!(
            service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "observation paths must not discard the live session"
        );
        assert!(
            service.comms_runtime(&created.session_id).await.is_some(),
            "observation paths must not drop the comms runtime"
        );
        assert!(
            service
                .interaction_event_injector(&created.session_id)
                .await
                .is_some(),
            "observation paths must not drop the interaction event injector"
        );
    }

    #[tokio::test]
    async fn test_realtime_open_snapshot_synchronizes_stale_live_transcript_authority() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            CapabilityBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        assert!(
            service.comms_runtime(&created.session_id).await.is_some(),
            "test setup should install a live comms runtime"
        );

        let mut durable = service
            .export_live_session(&created.session_id)
            .await
            .expect("initial live session should export");
        durable.push(Message::User(UserMessage::text(
            "durable realtime seed".to_string(),
        )));
        runtime_store
            .commit_session_snapshot(
                &LogicalRuntimeId::for_session(&created.session_id),
                SessionDelta {
                    session_snapshot: serde_json::to_vec(&durable)
                        .expect("serialize durable session"),
                },
            )
            .await
            .expect("commit durable runtime snapshot");

        let live_export = service.export_live_session(&created.session_id).await;
        assert!(
            matches!(live_export, Err(SessionError::NotFound { .. })),
            "public live export must fail closed while durable semantic authority wins"
        );

        let realtime_snapshot = service
            .export_realtime_open_session_snapshot(&created.session_id)
            .await
            .expect("realtime open should use durable authority after syncing live mechanics");
        assert!(
            realtime_snapshot.messages().iter().any(|message| matches!(
                message,
                Message::User(user) if user.text_content() == "durable realtime seed"
            )),
            "realtime-open snapshot must be the durable semantic authority"
        );
        assert!(
            service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "stale live transcript handle must remain installed after semantic sync"
        );
        assert!(
            service.comms_runtime(&created.session_id).await.is_some(),
            "semantic sync must preserve the live comms runtime"
        );
        assert!(
            service
                .interaction_event_injector(&created.session_id)
                .await
                .is_some(),
            "semantic sync must preserve the live interaction event injector"
        );
        let synced_live = service.export_live_session(&created.session_id).await;
        assert!(
            synced_live
                .expect("synced live session should export")
                .messages()
                .iter()
                .any(|message| matches!(
                    message,
                    Message::User(user) if user.text_content() == "durable realtime seed"
                )),
            "live session semantics must match the durable snapshot after realtime-open recovery"
        );
    }

    #[tokio::test]
    async fn test_runtime_turn_synchronizes_stale_live_transcript_authority() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            CapabilityBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let mut durable = service
            .export_live_session(&created.session_id)
            .await
            .expect("initial live session should export");
        durable.push(Message::User(UserMessage::text(
            "durable runtime turn seed".to_string(),
        )));
        runtime_store
            .commit_session_snapshot(
                &LogicalRuntimeId::for_session(&created.session_id),
                SessionDelta {
                    session_snapshot: serde_json::to_vec(&durable)
                        .expect("serialize durable session"),
                },
            )
            .await
            .expect("commit durable runtime snapshot");

        let output = service
            .apply_runtime_turn(
                &created.session_id,
                RunId::new(),
                runtime_content_turn_request("next turn"),
                RunApplyBoundary::RunStart,
                vec![InputId::new()],
            )
            .await
            .expect("runtime turn should synchronize live semantics instead of rebuilding");

        let output_session: Session = serde_json::from_slice(
            output
                .session_snapshot
                .as_ref()
                .expect("runtime output should carry a session snapshot"),
        )
        .expect("runtime output session snapshot should deserialize");
        assert!(
            output_session.messages().iter().any(|message| matches!(
                message,
                Message::User(user) if user.text_content() == "durable runtime turn seed"
            )),
            "runtime output must start from durable semantic authority"
        );
        assert!(
            service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "runtime turn sync must preserve the live handle"
        );
        assert!(
            service.comms_runtime(&created.session_id).await.is_some(),
            "runtime turn sync must preserve live comms mechanics"
        );
    }

    #[tokio::test]
    async fn test_realtime_open_snapshot_uses_durable_runtime_system_context_authority() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            CapabilityBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let mut durable = Session::with_id(created.session_id.clone());
        durable
            .set_system_context_state(applied_system_context_state(
                PendingSystemContextAppend {
                    text: "Peer terminal response from 550e8400-e29b-41d4-a716-446655440000\nRequest ID: req-123\nStatus: completed\ntoken birch seventeen".to_string(),
                    source: Some(
                        "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-123"
                            .to_string(),
                    ),
                    idempotency_key: Some("req-123".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::UNIX_EPOCH,
                },
            ))
            .expect("set durable system context state");
        runtime_store
            .commit_session_snapshot(
                &LogicalRuntimeId::for_session(&created.session_id),
                SessionDelta {
                    session_snapshot: serde_json::to_vec(&durable)
                        .expect("serialize durable session"),
                },
            )
            .await
            .expect("commit durable runtime snapshot");

        let live_export = service.export_live_session(&created.session_id).await;
        assert!(
            matches!(live_export, Err(SessionError::NotFound { .. })),
            "public live export must fail closed when durable runtime context diverges"
        );

        let realtime_snapshot = service
            .export_realtime_open_session_snapshot(&created.session_id)
            .await
            .expect("realtime open should use durable context authority");
        let runtime_context = realtime_snapshot.system_context_state().unwrap_or_default();
        assert!(
            runtime_context
                .applied()
                .iter()
                .any(|append| append.text.contains("birch seventeen")),
            "realtime open snapshot should preserve durable runtime context: {runtime_context:?}"
        );
        assert!(
            service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "context-authority fail closed must not discard the live capability handle"
        );
    }

    #[tokio::test]
    async fn test_realtime_open_snapshot_synchronizes_live_context_before_live_persist() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            CapabilityBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let mut durable = Session::with_id(created.session_id.clone());
        durable
            .set_system_context_state(applied_system_context_state(
                PendingSystemContextAppend {
                    text: "Peer terminal response from 550e8400-e29b-41d4-a716-446655440000\nRequest ID: req-123\nStatus: completed\ntoken birch seventeen".to_string(),
                    source: Some(
                        "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-123"
                            .to_string(),
                    ),
                    idempotency_key: Some("req-123".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::UNIX_EPOCH,
                },
            ))
            .expect("set durable system context state");
        runtime_store
            .commit_session_snapshot(
                &LogicalRuntimeId::for_session(&created.session_id),
                SessionDelta {
                    session_snapshot: serde_json::to_vec(&durable)
                        .expect("serialize durable session"),
                },
            )
            .await
            .expect("commit durable runtime snapshot");

        service
            .export_realtime_open_session_snapshot(&created.session_id)
            .await
            .expect("realtime open should synchronize durable context into live handle");
        service
            .persist_full_session(&created.session_id)
            .await
            .expect("live session persist should preserve synchronized context");

        let persisted = service
            .load_authoritative_session_base(&created.session_id)
            .await
            .expect("load authoritative session")
            .expect("authoritative session should exist");
        let runtime_context = persisted.system_context_state().unwrap_or_default();
        assert!(
            runtime_context
                .applied()
                .iter()
                .any(|append| append.text.contains("birch seventeen")),
            "live persistence after realtime open must not erase durable runtime context: {runtime_context:?}"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_archive_discards_uncommitted_live_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(Arc::clone(&runtime_store)),
            memory_blob_store(),
        );
        let machine = MeerkatMachine::persistent(Arc::clone(&runtime_store), memory_blob_store());

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        let output = service
            .apply_runtime_turn(
                &created.session_id,
                RunId::new(),
                runtime_content_turn_request("uncommitted runtime turn"),
                RunApplyBoundary::Immediate,
                vec![InputId::new()],
            )
            .await
            .expect("runtime apply should build output without committing authority");
        let staged_snapshot: Session = serde_json::from_slice(
            output
                .session_snapshot
                .as_deref()
                .expect("runtime output should carry a staged session snapshot"),
        )
        .expect("staged session snapshot should deserialize");
        assert!(
            !staged_snapshot.messages().is_empty(),
            "test must create an uncommitted live transcript before archive()"
        );

        service
            .archive_with_machine_protocol(
                &created.session_id,
                MachineSessionArchiveProtocol::from_machine(&machine),
            )
            .await
            .expect("archive should retire through machine authority");
        let archived_projection = store
            .load(&created.session_id)
            .await
            .expect("compatibility projection load should succeed")
            .expect("archive should persist the compatibility projection");
        assert!(
            metadata_marks_archived(archived_projection.metadata()),
            "archive should mirror retired lifecycle into the compatibility projection"
        );
        assert_eq!(
            meerkat_runtime::store::load_runtime_state(
                runtime_store.as_ref(),
                &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(
                    &created.session_id
                ),
            )
            .await
            .expect("runtime state load should succeed"),
            Some(RuntimeState::Retired),
            "archive must persist machine-owned retired lifecycle"
        );
        assert!(
            archived_projection.messages().is_empty(),
            "archive() must not persist uncommitted live transcript as durable truth"
        );
        assert!(
            !service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "archive() should discard the stale live session"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_public_interrupt_paths_are_not_callable() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        let err = service
            .interrupt(&created.session_id)
            .await
            .expect_err("public interrupt should not bypass MeerkatMachine");
        assert!(
            matches!(err, SessionError::Unsupported(_)),
            "unexpected public interrupt error: {err:?}"
        );

        let err = service
            .cancel_after_boundary(&created.session_id)
            .await
            .expect_err("public boundary cancel should not bypass MeerkatMachine");
        assert!(
            matches!(err, SessionError::Unsupported(_)),
            "unexpected public boundary cancel error: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_create_session_installs_active_store_checkpointer() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let builder = CapturingCheckpointerBuilder::new();
        let captured = Arc::clone(&builder.captured);
        let service = PersistentSessionService::new(
            builder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        let checkpointer = captured
            .lock()
            .await
            .clone()
            .expect("runtime-backed create_session should still inject a checkpointer");
        let original = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("create_session should persist an authoritative snapshot");
        let raw_before = store
            .load(&result.session_id)
            .await
            .expect("raw store load should succeed");

        let mut mutated = original.clone();
        mutated.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text(
                "checkpoint should update the session store projection".to_string(),
            ),
        ));
        checkpointer.checkpoint(&mutated).await;

        let raw_after = store
            .load(&result.session_id)
            .await
            .expect("raw store load should succeed");
        assert_eq!(
            raw_after.as_ref().map(|session| session.messages().len()),
            raw_before
                .as_ref()
                .map(|session| session.messages().len() + 1),
            "runtime-backed sessions must checkpoint changed turns into the SessionStore projection"
        );
    }

    #[tokio::test]
    async fn test_committed_runtime_session_checkpoint_updates_session_store_projection() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let original = store
            .load(&result.session_id)
            .await
            .expect("raw store load should succeed")
            .expect("create_session should persist a projection");

        let mut committed = original.clone();
        committed.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text(
                "runtime machine committed this turn first".to_string(),
            ),
        ));
        let session_snapshot =
            serde_json::to_vec(&committed).expect("committed snapshot should serialize");

        service
            .checkpoint_committed_runtime_session_snapshot(&result.session_id, &session_snapshot)
            .await
            .expect("committed runtime snapshot should update SessionStore projection");

        let raw_after = store
            .load(&result.session_id)
            .await
            .expect("raw store load should succeed")
            .expect("projection should remain present");
        assert_eq!(
            raw_after.messages().len(),
            original.messages().len() + 1,
            "post-commit runtime checkpoint must make the SessionStore projection current"
        );
    }

    #[tokio::test]
    async fn test_raw_store_delete_removes_seeded_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session = Session::new();
        let id = session.id().clone();
        store.save(&session).await.unwrap();

        assert!(store.load(&id).await.unwrap().is_some());
        store.delete(&id).await.unwrap();
        assert!(store.load(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_append_system_context_does_not_mutate_archived_store_row() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let mut archived = Session::new();
        let id = archived.id().clone();
        archived.set_metadata(SESSION_ARCHIVED_KEY, serde_json::Value::Bool(true));
        store
            .save(&archived)
            .await
            .expect("test should seed an archived compatibility row");

        let err = service
            .append_system_context(
                &id,
                AppendSystemContextRequest {
                    text: "runtime notice".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-persistent-archive".to_string()),
                },
            )
            .await
            .expect_err("archived session must not be recreated by append");
        assert_eq!(err.code(), "SESSION_UNSUPPORTED");
        let persisted = store
            .load(&id)
            .await
            .unwrap()
            .expect("append after archive must preserve the archived store row");
        assert_eq!(persisted.metadata(), archived.metadata());
        assert_eq!(persisted.messages().len(), archived.messages().len());
    }

    #[tokio::test]
    async fn test_persistent_read_history_returns_messages_for_live_and_archived_sessions() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let id = created.session_id;

        service
            .start_turn(&id, start_turn_request("follow up"))
            .await
            .expect("second turn should succeed");

        let page = service
            .read_history(
                &id,
                SessionHistoryQuery {
                    offset: 1,
                    limit: Some(2),
                },
            )
            .await
            .expect("live history should be readable");
        assert_eq!(page.session_id, id);
        assert_eq!(page.message_count, 4);
        assert_eq!(page.offset, 1);
        assert_eq!(page.limit, Some(2));
        assert!(page.has_more);
        assert_eq!(page.messages.len(), 2);

        let mut archived_projection = store
            .load(&id)
            .await
            .expect("raw store load should succeed")
            .expect("session-store projection should exist");
        archived_projection.set_metadata(SESSION_ARCHIVED_KEY, serde_json::Value::Bool(true));
        store
            .save(&archived_projection)
            .await
            .expect("test should seed archived compatibility projection");

        let archived = service
            .read_history(
                &id,
                SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("archived history should remain readable");
        assert_eq!(archived.session_id, id);
        assert_eq!(archived.message_count, 4);
        assert!(!archived.has_more);
        assert_eq!(archived.messages.len(), 4);
    }

    #[tokio::test]
    async fn test_persistent_fork_at_creates_new_idle_session_without_shrinking_parent() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let parent_id = created.session_id;
        service
            .start_turn(&parent_id, start_turn_request("follow up"))
            .await
            .expect("second turn should succeed");

        let forked = service
            .fork_session_at(
                &parent_id,
                SessionForkAtRequest {
                    message_index: 2,
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("fork_at should create a branch session");

        assert_ne!(forked.session_id, parent_id);
        assert_eq!(forked.message_count, 2);

        let parent_history = service
            .read_history(&parent_id, SessionHistoryQuery::default())
            .await
            .expect("parent history should remain readable");
        assert_eq!(parent_history.message_count, 4);

        let fork_history = service
            .read_history(&forked.session_id, SessionHistoryQuery::default())
            .await
            .expect("fork history should be persisted");
        assert_eq!(fork_history.message_count, 2);
        assert_eq!(fork_history.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_persistent_fork_replace_message_creates_changed_branch() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let parent_id = created.session_id;
        service
            .start_turn(&parent_id, start_turn_request("follow up"))
            .await
            .expect("second turn should succeed");

        let forked = service
            .fork_session_replace(
                &parent_id,
                SessionForkReplaceRequest {
                    message_index: 2,
                    replacement: TranscriptReplacement::Message {
                        message: Message::User(UserMessage::text("edited follow up")),
                    },
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("fork_replace should create a branch session");

        assert_ne!(forked.session_id, parent_id);
        assert_eq!(forked.message_count, 3);

        let fork_history = service
            .read_history(&forked.session_id, SessionHistoryQuery::default())
            .await
            .expect("fork history should be persisted");
        assert_eq!(fork_history.message_count, 3);
        assert!(matches!(
            &fork_history.messages[2],
            Message::User(user) if user.text_content() == "edited follow up"
        ));

        let parent_history = service
            .read_history(&parent_id, SessionHistoryQuery::default())
            .await
            .expect("parent history should remain unchanged");
        assert!(matches!(
            &parent_history.messages[2],
            Message::User(user) if user.text_content() == "follow up"
        ));
    }

    #[tokio::test]
    async fn test_persistent_rewrite_transcript_advances_same_session_head() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        service
            .start_turn(&session_id, start_turn_request("follow up"))
            .await
            .expect("second turn should succeed");

        let before = service
            .read_history(&session_id, SessionHistoryQuery::default())
            .await
            .expect("history before rewrite");
        assert_eq!(before.message_count, 4);

        let parent_revision = store
            .load(&session_id)
            .await
            .expect("load before rewrite")
            .expect("session exists")
            .transcript_revision()
            .expect("parent revision");

        let result = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 4 },
                    replacement: vec![Message::BlockAssistant(
                        meerkat_core::BlockAssistantMessage::new(
                            vec![AssistantBlock::Text {
                                text: "compacted assistant trace".to_string(),
                                meta: None,
                            }],
                            StopReason::EndTurn,
                        ),
                    )],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("test".to_string()),
                    expected_parent_revision: Some(parent_revision.clone()),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("rewrite should commit");

        assert_eq!(result.session_id, session_id);
        assert_eq!(result.parent_revision, parent_revision);
        assert_ne!(result.revision, result.parent_revision);
        assert_eq!(result.message_count, 2);

        let after = service
            .read_history(&session_id, SessionHistoryQuery::default())
            .await
            .expect("history after rewrite");
        assert_eq!(after.session_id, session_id);
        assert_eq!(after.message_count, 2);
        assert!(matches!(
            &after.messages[1],
            Message::BlockAssistant(assistant)
                if assistant.to_string() == "compacted assistant trace"
        ));

        let saved = store
            .load(&session_id)
            .await
            .expect("load after rewrite")
            .expect("session exists");
        let state = saved
            .transcript_history_state()
            .expect("history state should decode")
            .expect("history state should exist");
        assert_eq!(state.head, result.revision);
        assert_eq!(state.commits.len(), 1);
        assert_eq!(state.commits[0].parent_revision, parent_revision);
        assert_eq!(state.revisions.len(), 2);
        let parent_body = state
            .revisions
            .iter()
            .find(|body| body.revision == parent_revision)
            .expect("parent revision body retained");
        assert_eq!(
            serde_json::to_value(&parent_body.messages).expect("parent body serializes"),
            serde_json::to_value(&before.messages).expect("before history serializes")
        );
        let rewritten_body = state
            .revisions
            .iter()
            .find(|body| body.revision == result.revision)
            .expect("rewritten revision body retained");
        assert_eq!(
            serde_json::to_value(&rewritten_body.messages).expect("rewritten body serializes"),
            serde_json::to_value(&after.messages).expect("after history serializes")
        );

        let parent_page = service
            .read_transcript_revision(
                &session_id,
                SessionTranscriptRevisionQuery {
                    revision: parent_revision,
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("parent revision should be recoverable");
        assert_eq!(parent_page.message_count, 4);
        assert_eq!(
            serde_json::to_value(&parent_page.messages).expect("parent page serializes"),
            serde_json::to_value(&before.messages).expect("before history serializes")
        );

        let restored = service
            .restore_session_transcript_revision(
                &session_id,
                SessionTranscriptRestoreRevisionRequest {
                    revision: result.parent_revision.clone(),
                    reason: TranscriptRewriteReason::new("restore"),
                    actor: Some("test".to_string()),
                    expected_parent_revision: Some(result.revision.clone()),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("restore should commit");
        assert_eq!(restored.session_id, session_id);
        assert_eq!(restored.parent_revision, result.revision);
        assert_eq!(restored.revision, result.parent_revision);
        assert_eq!(restored.message_count, before.message_count);

        let restored_history = service
            .read_history(&session_id, SessionHistoryQuery::default())
            .await
            .expect("history after restore");
        assert_eq!(
            serde_json::to_value(&restored_history.messages).expect("restored history serializes"),
            serde_json::to_value(&before.messages).expect("before history serializes")
        );

        let restored_saved = store
            .load(&session_id)
            .await
            .expect("load after restore")
            .expect("session exists");
        let restored_state = restored_saved
            .transcript_history_state()
            .expect("history state should decode")
            .expect("history state should exist");
        assert_eq!(restored_state.head, restored.revision);
        assert_eq!(restored_state.commits.len(), 2);
        assert_eq!(
            restored_state.commits[1].replacement_digest,
            meerkat_core::transcript_messages_digest(&before.messages)
                .expect("before history digest should compute")
        );
    }

    #[tokio::test]
    async fn test_persistent_rewrite_transcript_rejects_stale_parent_revision() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;

        let err = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::BlockAssistant(
                        meerkat_core::BlockAssistantMessage::new(
                            vec![AssistantBlock::Text {
                                text: "replacement".to_string(),
                                meta: None,
                            }],
                            StopReason::EndTurn,
                        ),
                    )],
                    reason: TranscriptRewriteReason::new("correction"),
                    actor: None,
                    expected_parent_revision: Some("sha256:not-the-head".to_string()),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect_err("stale parent revision must fail");

        assert!(
            err.to_string().contains("parent revision mismatch"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn test_persistent_rewrite_transcript_maps_store_cas_conflict_to_revision_conflict() {
        let store: Arc<dyn SessionStore> = Arc::new(ConflictOnTranscriptRewriteStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let parent_revision = store
            .load(&session_id)
            .await
            .expect("load source")
            .expect("session exists")
            .transcript_revision()
            .expect("parent revision");

        let err = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::BlockAssistant(
                        meerkat_core::BlockAssistantMessage::new(
                            vec![AssistantBlock::Text {
                                text: "replacement".to_string(),
                                meta: None,
                            }],
                            StopReason::EndTurn,
                        ),
                    )],
                    reason: TranscriptRewriteReason::new("correction"),
                    actor: None,
                    expected_parent_revision: Some(parent_revision),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect_err("store CAS conflict must fail");

        assert!(
            matches!(
                err,
                SessionError::Agent(meerkat_core::AgentError::ConfigError(_))
            ),
            "store CAS conflict should surface as typed revision conflict, not store/internal: {err:?}"
        );
        assert!(
            err.to_string().contains("parent revision mismatch"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn test_persistent_rewrite_transcript_excludes_runtime_turn_admission_until_commit_finishes()
     {
        let pausing_store = Arc::new(PausingTranscriptRewriteStore::new());
        let store: Arc<dyn SessionStore> = pausing_store.clone();
        let service = Arc::new(PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        ));

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let parent_revision = store
            .load(&session_id)
            .await
            .expect("load before rewrite")
            .expect("session exists")
            .transcript_revision()
            .expect("parent revision");

        pausing_store.pause_rewrite_saves();
        let rewrite_service = Arc::clone(&service);
        let rewrite_session_id = session_id.clone();
        let rewrite = tokio::spawn(async move {
            rewrite_service
                .rewrite_session_transcript(
                    &rewrite_session_id,
                    meerkat_core::SessionTranscriptRewriteRequest {
                        selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                        replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                            content: "compact answer".to_string(),
                            tool_calls: Vec::new(),
                            stop_reason: StopReason::EndTurn,
                            usage: Usage::default(),
                            created_at: meerkat_core::types::message_timestamp_now(),
                        })],
                        reason: TranscriptRewriteReason::new("compaction"),
                        actor: Some("test".to_string()),
                        expected_parent_revision: Some(parent_revision),
                        running_behavior: TranscriptEditRunningBehavior::Reject,
                    },
                )
                .await
        });

        pausing_store.wait_for_rewrite_save().await;
        let blocked_admission = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            service.reserve_runtime_turn_admission(&session_id),
        )
        .await;
        assert!(
            blocked_admission.is_err(),
            "runtime turn admission must wait while transcript rewrite is committing"
        );

        pausing_store.release_rewrite_save();
        rewrite
            .await
            .expect("rewrite task should join")
            .expect("rewrite should commit");

        let admission = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            service.reserve_runtime_turn_admission(&session_id),
        )
        .await
        .expect("runtime turn admission should resume after rewrite")
        .expect("runtime turn admission should succeed");
        drop(admission);
    }

    #[tokio::test]
    async fn test_persistent_rewrite_transcript_externalizes_media_before_digesting_commit() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            ImagePreservingBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let parent_revision = store
            .load(&session_id)
            .await
            .expect("load before rewrite")
            .expect("session exists")
            .transcript_revision()
            .expect("parent revision");

        let result = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::User(UserMessage::with_blocks(
                        image_prompt("rewrite").into_blocks(),
                    ))],
                    reason: TranscriptRewriteReason::new("media rewrite"),
                    actor: Some("test".to_string()),
                    expected_parent_revision: Some(parent_revision),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("rewrite with inline media replacement should commit");

        let saved = store
            .load(&session_id)
            .await
            .expect("load after rewrite")
            .expect("session exists");
        assert_no_inline_images_in_session(&saved);
        let saved_digest = meerkat_core::transcript_messages_digest(saved.messages())
            .expect("saved messages should digest");
        assert_eq!(
            saved.transcript_revision().expect("saved revision"),
            saved_digest
        );
        assert_eq!(result.revision, saved_digest);
    }

    #[tokio::test]
    async fn test_persistent_post_rewrite_media_externalization_refreshes_transcript_head() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            ImagePreservingBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let parent_revision = store
            .load(&session_id)
            .await
            .expect("load before rewrite")
            .expect("session exists")
            .transcript_revision()
            .expect("parent revision");

        service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "compact answer".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("test".to_string()),
                    expected_parent_revision: Some(parent_revision),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("rewrite should commit");

        service
            .start_turn(
                &session_id,
                StartTurnRequest {
                    prompt: image_prompt("post-rewrite"),
                    ..start_turn_request("ignored")
                },
            )
            .await
            .expect("post-rewrite media turn should persist");

        let saved = store
            .load(&session_id)
            .await
            .expect("load after media turn")
            .expect("session exists");
        assert_no_inline_images_in_session(&saved);
        let saved_digest = meerkat_core::transcript_messages_digest(saved.messages())
            .expect("saved messages should digest");
        assert_eq!(
            saved.transcript_revision().expect("saved revision"),
            saved_digest
        );
    }

    #[tokio::test]
    async fn test_persistent_rewrite_transcript_appends_audit_event() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let parent_revision = store
            .load(&session_id)
            .await
            .expect("load before rewrite")
            .expect("session exists")
            .transcript_revision()
            .expect("parent revision");

        let rewrite = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::BlockAssistant(
                        meerkat_core::BlockAssistantMessage::new(
                            vec![AssistantBlock::Text {
                                text: "audit compacted trace".to_string(),
                                meta: None,
                            }],
                            StopReason::EndTurn,
                        ),
                    )],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("audit-test".to_string()),
                    expected_parent_revision: Some(parent_revision.clone()),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("rewrite should commit");

        let events = service
            .event_log_read_from(&session_id, 1)
            .await
            .expect("event log should read")
            .expect("event projection installed");
        let audit = events
            .iter()
            .find_map(|stored| match &stored.event {
                AgentEvent::TranscriptRewriteCommitted { session_id, record } => {
                    Some((session_id, record))
                }
                _ => None,
            })
            .expect("rewrite audit event should be appended");
        assert_eq!(audit.0, &session_id);
        assert_eq!(audit.1.commit.parent_revision, parent_revision);
        assert_eq!(audit.1.commit.revision, rewrite.revision);
        assert_eq!(audit.1.commit.reason.kind, "compaction");
        let rebuilt = meerkat_core::TranscriptHistoryState::from_rewrite_records(
            events.iter().filter_map(|stored| match &stored.event {
                AgentEvent::TranscriptRewriteCommitted { record, .. } => Some(record.clone()),
                _ => None,
            }),
        )
        .expect("rewrite records should replay")
        .expect("rewrite records should exist");
        assert_eq!(rebuilt.head, rewrite.revision);
        assert_eq!(rebuilt.commits.len(), 1);
        assert_eq!(rebuilt.revisions.len(), 2);
        let replayed_head = rebuilt
            .revisions
            .iter()
            .find(|body| body.revision == rebuilt.head)
            .expect("replayed head body should exist");
        assert_eq!(
            serde_json::to_value(&replayed_head.messages).expect("head serializes"),
            serde_json::to_value(
                &service
                    .read_history(&session_id, SessionHistoryQuery::default())
                    .await
                    .expect("history after rewrite")
                    .messages
            )
            .expect("history serializes")
        );

        let events_path = dir
            .path()
            .join(".rkat")
            .join("sessions")
            .join(session_id.to_string())
            .join("events.jsonl");
        let projected =
            read_projected_events_after(&events_path, "transcript_rewrite_committed").await;
        assert!(projected.contains(&rewrite.revision));
    }

    #[tokio::test]
    async fn test_persistent_rewrite_transcript_audit_append_failure_does_not_mutate_projection() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        event_store.fail_appends();
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let parent_revision = store
            .load(&session_id)
            .await
            .expect("load before rewrite")
            .expect("session exists")
            .transcript_revision()
            .expect("parent revision");

        let rewrite_err = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "compact answer despite audit outage".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("audit-failure-test".to_string()),
                    expected_parent_revision: Some(parent_revision),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect_err(
                "rewrite must fail closed when the canonical audit event cannot be appended",
            );
        assert!(
            rewrite_err
                .to_string()
                .contains("synthetic transcript rewrite audit append failure"),
            "unexpected error: {rewrite_err}"
        );

        assert_eq!(event_store.last_seq(&session_id).await.unwrap(), 0);
        let raw_saved = store
            .load(&session_id)
            .await
            .expect("raw load after failed audit append")
            .expect("session projection remains present");
        assert!(!matches!(
            &raw_saved.messages()[1],
            Message::Assistant(assistant)
                if assistant.content == "compact answer despite audit outage"
        ));
        service
            .read_history(&session_id, SessionHistoryQuery::default())
            .await
            .expect("failed audit append must leave the previous projection readable");
    }

    #[tokio::test]
    async fn test_persistent_save_normalized_quarantines_unaudited_projection_when_rollback_fails()
    {
        let fail_store = Arc::new(FailAuthoritativeProjectionStore::new());
        let store: Arc<dyn SessionStore> = fail_store.clone();
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let original = store
            .load(&session_id)
            .await
            .expect("load before rewrite")
            .expect("session should exist");
        let parent_revision = original
            .transcript_revision()
            .expect("parent revision should digest");

        let mut incoming = original.clone();
        let commit = incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: original.messages().len(),
                },
                vec![Message::User(UserMessage::text(
                    "[Context compacted] unaudited no-runtime save".to_string(),
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("test".to_string()),
                Some(parent_revision),
            )
            .expect("rewrite should commit");

        event_store.fail_appends();
        fail_store.fail_authoritative_projection_cas();
        let err = service
            .save_normalized_session(incoming)
            .await
            .expect_err("audit append plus rollback failure must fail closed");
        assert!(
            err.to_string()
                .contains("unaudited projection was quarantined"),
            "unexpected error: {err}"
        );
        assert!(
            store
                .load(&session_id)
                .await
                .expect("load after quarantine")
                .is_none(),
            "failed rollback must not leave unaudited rewrite as store fallback"
        );
        assert_ne!(
            original
                .transcript_revision()
                .expect("original revision should digest"),
            commit.revision
        );
    }

    #[tokio::test]
    async fn test_persistent_rewrite_transcript_recovers_missing_audit_event_from_graph() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let parent_revision = store
            .load(&session_id)
            .await
            .expect("load before rewrite")
            .expect("session exists")
            .transcript_revision()
            .expect("parent revision");

        let rewrite = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "compact answer before audit projection existed".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("audit-repair-test".to_string()),
                    expected_parent_revision: Some(parent_revision),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("rewrite should commit without event projection");

        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let recovery_service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        recovery_service
            .read_history(&session_id, SessionHistoryQuery::default())
            .await
            .expect("missing audit event should be repaired from retained graph");
        assert_eq!(event_store.last_seq(&session_id).await.unwrap(), 1);
        let events = event_store
            .read_from(&session_id, 1)
            .await
            .expect("audit event should read back");
        let AgentEvent::TranscriptRewriteCommitted { record, .. } = &events[0].event else {
            panic!("repaired event should be a transcript rewrite commit");
        };
        assert_eq!(record.commit.revision, rewrite.revision);
    }

    #[tokio::test]
    async fn test_persistent_internal_rewrite_then_append_persists_bridged_head() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let original = store
            .load(&session_id)
            .await
            .expect("load original")
            .expect("session exists");
        let parent_revision = original.transcript_revision().expect("parent revision");

        let mut incoming = original.clone();
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(meerkat_core::AssistantMessage {
                    content: "internal compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                    created_at: meerkat_core::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .expect("internal rewrite should commit");
        incoming.push(Message::User(UserMessage::text("follow-up".to_string())));
        incoming.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "follow-up answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let incoming_revision =
            meerkat_core::transcript_messages_digest(incoming.messages()).expect("digest");

        let persisted = service
            .save_normalized_session(incoming)
            .await
            .expect("bridged internal rewrite plus append should persist");
        assert_eq!(
            meerkat_core::transcript_messages_digest(persisted.messages()).expect("digest"),
            incoming_revision
        );
        let saved = store
            .load(&session_id)
            .await
            .expect("load saved")
            .expect("saved session exists");
        assert_eq!(
            meerkat_core::transcript_messages_digest(saved.messages()).expect("digest"),
            incoming_revision
        );
    }

    #[tokio::test]
    async fn test_persistent_internal_rewrite_chain_appends_all_audit_events() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let original = store
            .load(&session_id)
            .await
            .expect("load original")
            .expect("session exists");
        let parent_revision = original.transcript_revision().expect("parent revision");

        let mut incoming = original.clone();
        let first = incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(meerkat_core::AssistantMessage {
                    content: "first compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                    created_at: meerkat_core::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .expect("first internal rewrite should commit");
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(meerkat_core::AssistantMessage {
                    content: "second compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                    created_at: meerkat_core::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("synthetic_notice_cleanup"),
                Some("meerkat-core".to_string()),
                Some(first.revision),
            )
            .expect("second internal rewrite should commit");

        let persisted = service
            .save_normalized_session(incoming)
            .await
            .expect("internal rewrite chain should persist");
        assert_eq!(event_store.last_seq(&session_id).await.unwrap(), 2);
        let history = service
            .read_history(&session_id, SessionHistoryQuery::default())
            .await
            .expect("audited internal rewrite chain should remain readable");
        assert_eq!(
            serde_json::to_value(&history.messages).expect("history serializes"),
            serde_json::to_value(persisted.messages()).expect("persisted serializes")
        );
    }

    #[tokio::test]
    async fn test_persistent_internal_rewrite_chain_persists_normal_append_bridge() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let original = store
            .load(&session_id)
            .await
            .expect("load original")
            .expect("session exists");
        let parent_revision = original.transcript_revision().expect("parent revision");

        let mut incoming = original.clone();
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(meerkat_core::AssistantMessage {
                    content: "first compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                    created_at: meerkat_core::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .expect("first internal rewrite should commit");
        incoming.push(Message::User(UserMessage::text("second".to_string())));
        incoming.push(Message::Assistant(meerkat_core::AssistantMessage {
            content: "verbose second answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let bridge_revision = incoming.transcript_revision().expect("bridge revision");
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 3, end: 4 },
                vec![Message::Assistant(meerkat_core::AssistantMessage {
                    content: "second compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                    created_at: meerkat_core::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(bridge_revision),
            )
            .expect("second internal rewrite should commit after normal append");

        let persisted = service
            .save_normalized_session(incoming)
            .await
            .expect("bridged internal rewrite chain should persist");
        assert_eq!(event_store.last_seq(&session_id).await.unwrap(), 2);
        let history = service
            .read_history(&session_id, SessionHistoryQuery::default())
            .await
            .expect("audited bridged rewrite chain should remain readable");
        assert_eq!(
            serde_json::to_value(&history.messages).expect("history serializes"),
            serde_json::to_value(persisted.messages()).expect("persisted serializes")
        );
    }

    #[tokio::test]
    async fn test_persistent_internal_rewrite_chain_audit_failure_keeps_last_audited_projection() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        event_store.fail_rewrite_append_call(2);
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let original = store
            .load(&session_id)
            .await
            .expect("load original")
            .expect("session exists");
        let parent_revision = original.transcript_revision().expect("parent revision");

        let mut incoming = original.clone();
        let first = incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(meerkat_core::AssistantMessage {
                    content: "first compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                    created_at: meerkat_core::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .expect("first internal rewrite should commit");
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(meerkat_core::AssistantMessage {
                    content: "second compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                    created_at: meerkat_core::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("synthetic_notice_cleanup"),
                Some("meerkat-core".to_string()),
                Some(first.revision.clone()),
            )
            .expect("second internal rewrite should commit");

        service
            .save_normalized_session(incoming)
            .await
            .expect_err("second audit append failure should surface");
        assert_eq!(event_store.last_seq(&session_id).await.unwrap(), 1);
        let raw_saved = store
            .load(&session_id)
            .await
            .expect("raw load after failed second audit append")
            .expect("session exists");
        assert_eq!(
            raw_saved.transcript_revision().expect("raw saved revision"),
            first.revision
        );
        let history = service
            .read_history(&session_id, SessionHistoryQuery::default())
            .await
            .expect("last audited projection should remain readable");
        assert!(matches!(
            &history.messages[1],
            Message::Assistant(assistant) if assistant.content == "first compact answer"
        ));
    }

    #[tokio::test]
    async fn test_transcript_rewrite_updates_stale_same_length_live_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let parent_revision = store
            .load(&session_id)
            .await
            .expect("load before rewrite")
            .expect("session exists")
            .transcript_revision()
            .expect("parent revision");

        service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "same length durable rewrite".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("stale-live-test".to_string()),
                    expected_parent_revision: Some(parent_revision),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("rewrite should commit");

        let view = service.read(&session_id).await.expect("read after rewrite");
        assert_eq!(
            view.state.last_assistant_text.as_deref(),
            Some("same length durable rewrite")
        );
    }

    #[tokio::test]
    async fn test_transcript_rewrite_event_replay_uses_event_sequence_not_commit_clock() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let stale_projection = store
            .load(&session_id)
            .await
            .expect("load original")
            .expect("session exists");
        let original_revision = stale_projection
            .transcript_revision()
            .expect("original revision");
        let original_messages = stale_projection.messages().to_vec();

        let compact = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "clock skew compact".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("clock-skew-test".to_string()),
                    expected_parent_revision: Some(original_revision.clone()),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("compact rewrite should commit");
        let restore = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange {
                        start: 0,
                        end: compact.message_count,
                    },
                    replacement: original_messages.clone(),
                    reason: TranscriptRewriteReason::new("restore"),
                    actor: Some("clock-skew-test".to_string()),
                    expected_parent_revision: Some(compact.revision),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("restore rewrite should commit");
        assert_eq!(restore.revision, original_revision);

        {
            let mut events_by_session = event_store.events.lock().await;
            let events = events_by_session
                .get_mut(&session_id)
                .expect("rewrite events should exist");
            assert_eq!(events.len(), 2);
            let (first, rest) = events.split_at_mut(1);
            let second = &mut rest[0];
            let AgentEvent::TranscriptRewriteCommitted {
                record: first_record,
                ..
            } = &mut first[0].event
            else {
                panic!("first event should be rewrite commit");
            };
            let AgentEvent::TranscriptRewriteCommitted {
                record: second_record,
                ..
            } = &mut second.event
            else {
                panic!("second event should be rewrite commit");
            };
            std::mem::swap(
                &mut first_record.commit.committed_at,
                &mut second_record.commit.committed_at,
            );
        }

        let recovery_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        recovery_store
            .save(&stale_projection)
            .await
            .expect("seed stale projection");
        let recovery_dir = tempfile::tempdir().expect("recovery tempdir");
        let recovery_service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&recovery_store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store,
            Arc::new(SessionProjector::new(recovery_dir.path().join(".rkat"))),
        );

        let recovered = recovery_service
            .read_transcript_revision(
                &session_id,
                SessionTranscriptRevisionQuery {
                    revision: "current".to_string(),
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("current revision should recover from event sequence");
        assert_eq!(recovered.revision, original_revision);
        assert_eq!(
            serde_json::to_value(&recovered.messages).expect("recovered serializes"),
            serde_json::to_value(&original_messages).expect("original serializes")
        );
    }

    #[tokio::test]
    async fn test_transcript_rewrite_events_recover_stale_session_projection() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let stale_projection = store
            .load(&session_id)
            .await
            .expect("load stale projection")
            .expect("session exists before rewrite");
        let parent_revision = stale_projection
            .transcript_revision()
            .expect("parent revision should digest");

        let rewrite = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::BlockAssistant(
                        meerkat_core::BlockAssistantMessage::new(
                            vec![AssistantBlock::Text {
                                text: "event replay compacted trace".to_string(),
                                meta: None,
                            }],
                            StopReason::EndTurn,
                        ),
                    )],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("replay-test".to_string()),
                    expected_parent_revision: Some(parent_revision.clone()),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("rewrite should commit");

        let recovery_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        recovery_store
            .save(&stale_projection)
            .await
            .expect("seed stale projection");
        let recovery_dir = tempfile::tempdir().expect("recovery tempdir");
        let recovery_service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&recovery_store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store.clone(),
            Arc::new(SessionProjector::new(recovery_dir.path().join(".rkat"))),
        );

        let recovered = recovery_service
            .read_history(&session_id, SessionHistoryQuery::default())
            .await
            .expect("history should recover from rewrite event replay");
        assert_eq!(recovered.session_id, session_id);
        assert!(matches!(
            &recovered.messages[1],
            Message::BlockAssistant(assistant)
                if assistant.to_string() == "event replay compacted trace"
        ));

        let retained_parent = recovery_service
            .read_transcript_revision(
                &session_id,
                SessionTranscriptRevisionQuery {
                    revision: parent_revision.clone(),
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("event replay should recover the retained parent revision");
        assert_eq!(retained_parent.head_revision, rewrite.revision);
        assert_eq!(retained_parent.revision, parent_revision);
        assert_eq!(
            serde_json::to_value(&retained_parent.messages).expect("parent serializes"),
            serde_json::to_value(stale_projection.messages()).expect("stale serializes")
        );

        let current_revision = recovery_service
            .read_transcript_revision(
                &session_id,
                SessionTranscriptRevisionQuery {
                    revision: "current".to_string(),
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("current alias should resolve to the active transcript revision");
        assert_eq!(current_revision.revision, rewrite.revision);
        assert_eq!(current_revision.head_revision, rewrite.revision);
        let still_stale = recovery_store
            .load(&session_id)
            .await
            .expect("load recovery store")
            .expect("recovery projection remains present");
        assert_eq!(
            serde_json::to_value(still_stale.messages()).expect("stale projection serializes"),
            serde_json::to_value(stale_projection.messages()).expect("original stale serializes"),
            "read-side replay recovery must not overwrite the durable projection"
        );
    }

    #[tokio::test]
    async fn test_transcript_rewrite_replay_materializes_projection_before_followup_rewrite() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let stale_projection = store
            .load(&session_id)
            .await
            .expect("load stale projection")
            .expect("session exists before rewrite");
        let parent_revision = stale_projection
            .transcript_revision()
            .expect("parent revision should digest");

        let first = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "event replay first compacted trace".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("replay-followup-test".to_string()),
                    expected_parent_revision: Some(parent_revision),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("first rewrite should commit");

        let recovery_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        recovery_store
            .save(&stale_projection)
            .await
            .expect("seed stale projection");
        let recovery_dir = tempfile::tempdir().expect("recovery tempdir");
        let recovery_service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&recovery_store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store.clone(),
            Arc::new(SessionProjector::new(recovery_dir.path().join(".rkat"))),
        );

        let second = recovery_service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "event replay second compacted trace".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("replay-followup-test".to_string()),
                    expected_parent_revision: Some(first.revision),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("replay-recovered projection should be a valid parent for follow-up rewrite");

        let saved = recovery_store
            .load(&session_id)
            .await
            .expect("load recovered store")
            .expect("recovered session exists");
        assert_eq!(
            saved.transcript_revision().expect("saved revision"),
            second.revision
        );
        assert!(matches!(
            &saved.messages()[1],
            Message::Assistant(assistant)
                if assistant.content == "event replay second compacted trace"
        ));
    }

    #[tokio::test]
    async fn test_transcript_rewrite_replay_materializes_projection_before_restore() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let stale_projection = store
            .load(&session_id)
            .await
            .expect("load stale projection")
            .expect("session exists before rewrite");
        let parent_revision = stale_projection
            .transcript_revision()
            .expect("parent revision should digest");

        let first = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "event replay compacted trace".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("replay-restore-test".to_string()),
                    expected_parent_revision: Some(parent_revision.clone()),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("first rewrite should commit");

        let recovery_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        recovery_store
            .save(&stale_projection)
            .await
            .expect("seed stale projection");
        let recovery_dir = tempfile::tempdir().expect("recovery tempdir");
        let recovery_service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&recovery_store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store.clone(),
            Arc::new(SessionProjector::new(recovery_dir.path().join(".rkat"))),
        );

        let restored = recovery_service
            .restore_session_transcript_revision(
                &session_id,
                SessionTranscriptRestoreRevisionRequest {
                    revision: parent_revision.clone(),
                    reason: TranscriptRewriteReason::new("restore"),
                    actor: Some("replay-restore-test".to_string()),
                    expected_parent_revision: Some(first.revision),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("replay-recovered projection should be a valid parent for restore");

        assert_eq!(restored.revision, parent_revision);
        let saved = recovery_store
            .load(&session_id)
            .await
            .expect("load recovered store")
            .expect("recovered session exists");
        assert_eq!(
            saved.transcript_revision().expect("saved revision"),
            restored.revision
        );
        assert_eq!(
            serde_json::to_value(saved.messages()).expect("saved messages serialize"),
            serde_json::to_value(stale_projection.messages()).expect("stale messages serialize")
        );
    }

    #[tokio::test]
    async fn test_transcript_rewrite_replay_materializes_partial_graph_before_restore() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let original_revision = store
            .load(&session_id)
            .await
            .expect("load original projection")
            .expect("session exists")
            .transcript_revision()
            .expect("original revision");

        let first = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "first compacted trace".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("partial-graph-replay-test".to_string()),
                    expected_parent_revision: Some(original_revision),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("first rewrite should commit");
        let stale_partial_graph = store
            .load(&session_id)
            .await
            .expect("load partial graph")
            .expect("session exists after first rewrite");

        let second = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "second compacted trace".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("partial-graph-replay-test".to_string()),
                    expected_parent_revision: Some(first.revision),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("second rewrite should commit");

        let recovery_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        recovery_store
            .save_authoritative_projection_if_current_revision(&stale_partial_graph, None)
            .await
            .expect("seed stale partial graph projection");
        let recovery_dir = tempfile::tempdir().expect("recovery tempdir");
        let recovery_service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&recovery_store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store.clone(),
            Arc::new(SessionProjector::new(recovery_dir.path().join(".rkat"))),
        );

        recovery_service
            .restore_session_transcript_revision(
                &session_id,
                SessionTranscriptRestoreRevisionRequest {
                    revision: stale_partial_graph
                        .transcript_revision()
                        .expect("stale revision"),
                    reason: TranscriptRewriteReason::new("restore"),
                    actor: Some("partial-graph-replay-test".to_string()),
                    expected_parent_revision: Some(second.revision.clone()),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("replay-merged partial graph should be materialized before restore");
    }

    #[tokio::test]
    async fn test_transcript_rewrite_replay_recovers_graph_lost_after_normal_turn() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let original = store
            .load(&session_id)
            .await
            .expect("load original")
            .expect("session exists");
        let original_revision = original.transcript_revision().expect("original revision");

        let rewrite = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "compacted trace before normal turn".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("graph-loss-replay-test".to_string()),
                    expected_parent_revision: Some(original_revision.clone()),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("rewrite should commit");

        let mut graphless_post_turn = store
            .load(&session_id)
            .await
            .expect("load rewritten")
            .expect("session exists after rewrite");
        graphless_post_turn.push(Message::User(UserMessage::text(
            "normal turn after rewrite".to_string(),
        )));
        graphless_post_turn.clear_transcript_history_state();

        let recovery_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        recovery_store
            .save(&graphless_post_turn)
            .await
            .expect("seed graphless post-turn projection");
        let recovery_dir = tempfile::tempdir().expect("recovery tempdir");
        let recovery_service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&recovery_store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store,
            Arc::new(SessionProjector::new(recovery_dir.path().join(".rkat"))),
        );

        let retained_original = recovery_service
            .read_transcript_revision(
                &session_id,
                SessionTranscriptRevisionQuery {
                    revision: original_revision.clone(),
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("replay should restore retained graph before serving revision");
        assert_eq!(retained_original.revision, original_revision);
        assert_ne!(retained_original.head_revision, rewrite.parent_revision);
    }

    #[tokio::test]
    async fn test_read_current_transcript_revision_works_without_rewrite_graph() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let session = store
            .load(&session_id)
            .await
            .expect("load saved session")
            .expect("session exists");
        assert!(
            session
                .transcript_history_state()
                .expect("history state should decode")
                .is_none()
        );
        let head_revision = session.transcript_revision().expect("implicit head digest");

        let current = service
            .read_transcript_revision(
                &session_id,
                SessionTranscriptRevisionQuery {
                    revision: "current".to_string(),
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("current alias should page over current messages without rewrite metadata");

        assert_eq!(current.revision, head_revision);
        assert_eq!(current.head_revision, head_revision);
        assert_eq!(current.message_count, session.messages().len());
        assert_eq!(
            serde_json::to_value(&current.messages).expect("current messages serialize"),
            serde_json::to_value(session.messages()).expect("session messages serialize")
        );
    }

    #[tokio::test]
    async fn test_transcript_rewrite_event_replay_accepts_normal_turn_between_rewrites() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let session_id = created.session_id;
        let initial_projection = store
            .load(&session_id)
            .await
            .expect("load initial projection")
            .expect("session exists before rewrite");
        let initial_parent = initial_projection
            .transcript_revision()
            .expect("initial parent revision");

        service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "first compact answer".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: meerkat_core::Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("replay-test".to_string()),
                    expected_parent_revision: Some(initial_parent),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("first rewrite should commit");
        service
            .start_turn(&session_id, start_turn_request("normal follow up"))
            .await
            .expect("normal turn should commit between rewrites");
        let stale_projection = store
            .load(&session_id)
            .await
            .expect("load projection after normal turn")
            .expect("session exists after normal turn");
        let bridge_parent = stale_projection
            .transcript_revision()
            .expect("normal turn should advance transcript head");

        let second = service
            .rewrite_session_transcript(
                &session_id,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: TranscriptRewriteSelection::MessageRange { start: 3, end: 4 },
                    replacement: vec![Message::Assistant(meerkat_core::AssistantMessage {
                        content: "second compact answer".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: meerkat_core::Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    reason: TranscriptRewriteReason::new("compaction"),
                    actor: Some("replay-test".to_string()),
                    expected_parent_revision: Some(bridge_parent.clone()),
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("second rewrite should commit after normal turn");

        let recovery_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        recovery_store
            .save_authoritative_projection_if_current_revision(&stale_projection, None)
            .await
            .expect("seed stale projection before second rewrite");
        let recovery_dir = tempfile::tempdir().expect("recovery tempdir");
        let recovery_service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&recovery_store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store,
            Arc::new(SessionProjector::new(recovery_dir.path().join(".rkat"))),
        );

        let recovered = recovery_service
            .read_history(&session_id, SessionHistoryQuery::default())
            .await
            .expect("history should recover across rewrite-normal-turn-rewrite replay");
        assert_eq!(recovered.session_id, session_id);
        assert!(matches!(
            &recovered.messages[3],
            Message::Assistant(assistant) if assistant.content == "second compact answer"
        ));
        let retained_bridge = recovery_service
            .read_transcript_revision(
                &session_id,
                SessionTranscriptRevisionQuery {
                    revision: bridge_parent,
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("bridge parent revision should be retained from second rewrite record");
        assert_eq!(retained_bridge.head_revision, second.revision);
    }

    #[tokio::test]
    async fn test_persistent_fork_at_rejects_running_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let builder = BlockingRunBuilder::new();
        let service = Arc::new(PersistentSessionService::new(
            builder.clone(),
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        ));

        let service_for_create = Arc::clone(&service);
        let initial_turn = tokio::spawn(async move {
            service_for_create
                .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
                .await
        });
        builder.wait_for_entered_runs(1).await;
        builder.release_notify.add_permits(1);
        let created = initial_turn
            .await
            .expect("initial turn task should join")
            .expect("initial turn should create a session");
        let parent_id = created.session_id;

        let service_for_turn = Arc::clone(&service);
        let session_for_turn = parent_id.clone();
        let active_turn = tokio::spawn(async move {
            service_for_turn
                .start_turn(&session_for_turn, start_turn_request("slow follow up"))
                .await
        });
        builder.wait_for_entered_runs(2).await;

        let rejected = service
            .fork_session_at(
                &parent_id,
                SessionForkAtRequest {
                    message_index: 1,
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await;
        assert!(
            matches!(rejected, Err(SessionError::Busy { ref id }) if id == &parent_id),
            "fork_at should reject a running session: {rejected:?}"
        );

        builder.release_notify.add_permits(1);
        active_turn
            .await
            .expect("active turn task should join")
            .expect("active turn should complete after fork rejection");
    }

    #[tokio::test]
    async fn test_persistent_restore_transcript_revision_rejects_running_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let builder = BlockingRunBuilder::new();
        let service = Arc::new(PersistentSessionService::new(
            builder.clone(),
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        ));

        let service_for_create = Arc::clone(&service);
        let initial_turn = tokio::spawn(async move {
            service_for_create
                .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
                .await
        });
        builder.wait_for_entered_runs(1).await;
        builder.release_notify.add_permits(1);
        let created = initial_turn
            .await
            .expect("initial turn task should join")
            .expect("initial turn should create a session");
        let session_id = created.session_id;
        let original_revision = store
            .load(&session_id)
            .await
            .expect("load before rewrite")
            .expect("session exists")
            .transcript_revision()
            .expect("original revision");
        let service_for_turn = Arc::clone(&service);
        let session_for_turn = session_id.clone();
        let active_turn = tokio::spawn(async move {
            service_for_turn
                .start_turn(&session_for_turn, start_turn_request("slow follow up"))
                .await
        });
        builder.wait_for_entered_runs(2).await;

        let rejected = service
            .restore_session_transcript_revision(
                &session_id,
                SessionTranscriptRestoreRevisionRequest {
                    revision: original_revision,
                    reason: TranscriptRewriteReason::new("restore"),
                    actor: Some("restore-active-test".to_string()),
                    expected_parent_revision: None,
                    running_behavior: TranscriptEditRunningBehavior::Reject,
                },
            )
            .await;
        assert!(
            matches!(rejected, Err(SessionError::Busy { ref id }) if id == &session_id),
            "restore should reject a running session: {rejected:?}"
        );

        builder.release_notify.add_permits(1);
        active_turn
            .await
            .expect("active turn task should join")
            .expect("active turn should complete after restore rejection");
    }

    #[tokio::test]
    async fn test_persistent_archived_history_survives_restart_and_cache_eviction() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new_with_archived_history_capacity(
            DummyBuilder,
            1,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let first = service
            .create_session(create_request("first", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create first session");
        service
            .archive(&first.session_id)
            .await
            .expect("archive first session");

        let second = service
            .create_session(create_request("second", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create second session");
        service
            .archive(&second.session_id)
            .await
            .expect("archive second session");

        let restarted = PersistentSessionService::new_with_archived_history_capacity(
            DummyBuilder,
            1,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let archived = restarted
            .read_history(
                &first.session_id,
                SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("archived history should survive restart and cache eviction");
        assert_eq!(archived.session_id, first.session_id);
        assert_eq!(archived.message_count, 2);
        assert_eq!(archived.messages.len(), 2);

        let listed = restarted
            .list(SessionQuery::default())
            .await
            .expect("list sessions");
        assert!(
            listed.is_empty(),
            "archived sessions should remain hidden from list even when stored durably"
        );
    }

    #[tokio::test]
    async fn test_persistent_completed_sessions_do_not_consume_active_capacity() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        for index in 0..3 {
            service
                .create_session(create_request(
                    &format!("completed {index}"),
                    InitialTurnPolicy::RunImmediately,
                ))
                .await
                .unwrap_or_else(|err| {
                    panic!("completed session {index} should release active capacity: {err}")
                });
        }

        let sessions = service
            .list(SessionQuery::default())
            .await
            .expect("list sessions");
        assert_eq!(
            sessions.len(),
            3,
            "completed live sessions should remain readable without holding active capacity"
        );
    }

    #[tokio::test]
    async fn test_persistent_deferred_sessions_consume_capacity_until_archived() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let staged = service
            .create_session(create_request("staged", InitialTurnPolicy::Defer))
            .await
            .expect("first deferred session should be staged");
        let blocked = service
            .create_session(create_request("blocked", InitialTurnPolicy::Defer))
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "second deferred session should be rejected while first is staged: {blocked:?}"
        );

        service
            .archive(&staged.session_id)
            .await
            .expect("archiving staged session should release capacity");
        service
            .create_session(create_request("after archive", InitialTurnPolicy::Defer))
            .await
            .expect("deferred capacity should be reusable after archive");
    }

    #[tokio::test]
    async fn test_persistent_context_only_runtime_apply_respects_active_capacity() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let candidate = service
            .create_session(create_request(
                "candidate",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("candidate session should complete and release capacity");
        let blocker = service
            .create_session(create_request("blocker", InitialTurnPolicy::Defer))
            .await
            .expect("deferred blocker should hold active capacity");

        let blocked = service
            .apply_runtime_context_appends(
                &candidate.session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "capacity-bounded context append".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("ctx-capacity".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "context-only runtime apply should respect active capacity: {blocked:?}"
        );

        service
            .archive(&blocker.session_id)
            .await
            .expect("archive should release deferred blocker capacity");
        service
            .apply_runtime_context_appends(
                &candidate.session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "capacity-bounded context append after archive".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("ctx-capacity-after-archive".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await
            .expect("context-only runtime apply should proceed after capacity is released");
    }

    #[tokio::test]
    async fn test_reserved_start_turn_preserves_admission_for_not_found_recovery() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let candidate = service
            .create_session(create_request(
                "candidate",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("candidate session should complete and release capacity");
        let persisted = store
            .load(&candidate.session_id)
            .await
            .expect("load candidate")
            .expect("candidate should be persisted");
        let admission = service
            .reserve_runtime_turn_admission(&candidate.session_id)
            .await
            .expect("candidate admission should reserve active capacity");

        store
            .delete(&candidate.session_id)
            .await
            .expect("delete persisted candidate");
        service
            .discard_live_session(&candidate.session_id)
            .await
            .expect("discard live candidate");

        let start_req = StartTurnRequest {
            prompt: "recover".to_string().into(),
            system_prompt: None,
            event_tx: None,
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
        };
        let (error, recovered_admission) = service
            .start_turn_with_recoverable_reserved_admission(
                &candidate.session_id,
                start_req,
                admission,
            )
            .await
            .expect_err("missing live and persisted session should return NotFound");
        assert!(
            matches!(error, SessionError::NotFound { .. }),
            "expected recoverable NotFound, got {error:?}"
        );
        let recovered_admission =
            recovered_admission.expect("NotFound must return the reserved admission");

        let blocker = Session::new();
        store.save(&blocker).await.expect("persist blocker");
        let blocked = service.reserve_runtime_turn_admission(blocker.id()).await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "recovered admission should still hold active capacity"
        );

        service
            .create_session_with_reserved_admission(resume_request(persisted), recovered_admission)
            .await
            .expect("fallback materialization should reuse recovered admission");
    }

    #[tokio::test]
    async fn test_runtime_turn_admission_joins_existing_live_session_capacity() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("joiner", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create live session");
        let first = service
            .reserve_runtime_turn_admission(&created.session_id)
            .await
            .expect("first active admission should reserve capacity");
        let second = service
            .reserve_runtime_turn_admission(&created.session_id)
            .await
            .expect("second same-session admission should join existing capacity");

        let blocker = Session::new();
        let blocker_id = blocker.id().clone();
        store.save(&blocker).await.expect("persist blocker");
        let blocked = service.reserve_runtime_turn_admission(&blocker_id).await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "joined same-session admission must still consume only one active slot"
        );

        drop(first);
        let still_blocked = service.reserve_runtime_turn_admission(&blocker_id).await;
        assert!(
            still_blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "capacity must remain held until the final same-session lease drops"
        );

        drop(second);
        service
            .reserve_runtime_turn_admission(&blocker_id)
            .await
            .expect("dropping all same-session leases should release capacity");
    }

    #[tokio::test]
    async fn test_deferred_runtime_admission_restore_survives_joined_active_lease() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            FailingOverlayBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("staged", InitialTurnPolicy::Defer))
            .await
            .expect("create deferred session");
        let promoted = service
            .reserve_runtime_turn_admission(&created.session_id)
            .await
            .expect("promote staged admission");
        let joined = service
            .reserve_runtime_turn_admission(&created.session_id)
            .await
            .expect("join promoted active admission");

        let mut start_req = start_turn_request("resume staged");
        start_req.runtime.flow_tool_overlay = Some(meerkat_core::service::TurnToolOverlay {
            allowed_tools: Some(vec!["blocked-before-run".to_string()]),
            blocked_tools: None,
            dispatch_context: Default::default(),
        });
        let error = service
            .start_turn_with_reserved_admission(&created.session_id, start_req, promoted)
            .await
            .expect_err("flow overlay setup should fail before the staged turn runs");
        assert!(
            error.to_string().contains("synthetic flow overlay failure"),
            "unexpected start_turn error: {error:?}"
        );

        let blocker = Session::new();
        let blocker_id = blocker.id().clone();
        store.save(&blocker).await.expect("persist blocker");
        let blocked = service.reserve_runtime_turn_admission(&blocker_id).await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "joined active lease should keep capacity busy until it drops"
        );

        drop(joined);
        let still_blocked = service.reserve_runtime_turn_admission(&blocker_id).await;
        assert!(
            still_blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "final joined release should restore the staged session capacity instead of freeing it"
        );
    }

    #[tokio::test]
    async fn test_reserved_create_admission_cancel_during_create_releases_capacity() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let builder = BlockingBuildBuilder::new();
        let service = Arc::new(PersistentSessionService::new(
            builder.clone(),
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        ));

        let persisted = Session::new();
        let persisted_id = persisted.id().clone();
        store
            .save(&persisted)
            .await
            .expect("persist source session");
        let admission = service
            .reserve_runtime_turn_admission(&persisted_id)
            .await
            .expect("reserve create admission");

        let service_for_task = Arc::clone(&service);
        let create_task = tokio::spawn(async move {
            service_for_task
                .create_session_with_reserved_admission(resume_request(persisted), admission)
                .await
        });
        builder.wait_for_entered_builds(1).await;

        create_task.abort();
        let aborted = create_task
            .await
            .expect_err("aborted reserved create task should report cancellation");
        assert!(aborted.is_cancelled());

        let blocker = Session::new();
        let blocker_id = blocker.id().clone();
        store.save(&blocker).await.expect("persist blocker");
        service
            .reserve_runtime_turn_admission(&blocker_id)
            .await
            .expect("aborted reserved create should release active capacity");
    }

    #[tokio::test]
    async fn test_persistent_deferred_create_save_failure_discards_live_capacity() {
        let store = Arc::new(FailSaveStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        );

        store.set_fail_save(true);
        let failed = service
            .create_session(create_request("staged", InitialTurnPolicy::Defer))
            .await;
        assert!(
            failed
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("forced save failure")),
            "deferred create should surface durable save failure: {failed:?}"
        );

        store.set_fail_save(false);
        service
            .create_session(create_request(
                "after failed create",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("failed deferred create should discard live capacity");
    }

    #[tokio::test]
    async fn test_persistent_archived_resume_session_rejected_without_capacity_leak() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let mut archived = Session::new();
        archived.set_metadata(SESSION_ARCHIVED_KEY, serde_json::Value::Bool(true));
        store.save(&archived).await.expect("save archived session");

        let rejected = service.create_session(resume_request(archived)).await;
        assert!(
            matches!(rejected, Err(SessionError::NotFound { .. })),
            "archived resume should be rejected before reserving capacity: {rejected:?}"
        );

        service
            .create_session(create_request(
                "after archived resume",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("rejected archived resume should not leak active capacity");
    }

    #[tokio::test]
    async fn test_persistent_stale_resume_rechecks_current_archived_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            2,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create session");
        let stale_resume = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("load authoritative")
            .expect("session should exist");

        let mut archived = store
            .load(&created.session_id)
            .await
            .expect("raw store load should succeed")
            .expect("session-store projection should exist");
        archived.set_metadata(SESSION_ARCHIVED_KEY, serde_json::Value::Bool(true));
        store
            .save(&archived)
            .await
            .expect("test should seed archived compatibility projection");
        service
            .discard_live_session(&created.session_id)
            .await
            .expect("test archive seed should release live capacity");

        let rejected = service.create_session(resume_request(stale_resume)).await;
        assert!(
            matches!(rejected, Err(SessionError::NotFound { .. })),
            "resume must recheck the current durable archive state: {rejected:?}"
        );
        assert!(
            !service
                .has_live_session(&created.session_id)
                .await
                .expect("live check should succeed"),
            "stale resume must not recreate an archived live session"
        );
    }

    #[tokio::test]
    async fn test_machine_authorized_archived_resume_is_not_metadata_driven() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create session");
        let stale_resume = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("load authoritative")
            .expect("session should exist");

        let mut archived = store
            .load(&created.session_id)
            .await
            .expect("raw store load should succeed")
            .expect("session-store projection should exist");
        archived.set_metadata(SESSION_ARCHIVED_KEY, serde_json::Value::Bool(true));
        store
            .save(&archived)
            .await
            .expect("test should seed archived compatibility projection");
        service
            .discard_live_session(&created.session_id)
            .await
            .expect("test archive seed should release live capacity");
        let stored = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("load archived snapshot")
            .expect("archived snapshot should exist");
        assert!(metadata_marks_archived(stored.metadata()));

        let stale_resume_for_authorized = stale_resume.clone();
        let rejected = service.create_session(resume_request(stale_resume)).await;
        assert!(
            matches!(rejected, Err(SessionError::NotFound { .. })),
            "ordinary stale resume must not resurrect an archived snapshot: {rejected:?}"
        );
        let machine = meerkat_runtime::MeerkatMachine::ephemeral();
        let admission = service
            .reserve_create_session_admission()
            .await
            .expect("reserve machine-authorized retry admission");
        service
            .create_session_with_reserved_machine_archived_resume_admission(
                resume_request(stale_resume_for_authorized),
                admission,
                machine.session_control_authority(),
            )
            .await
            .expect("machine-authorized pending-promotion retry may resume archived durable base");
        service
            .discard_live_session(&created.session_id)
            .await
            .expect("authorized retry should remain controllable and release capacity");

        service
            .create_session(create_request(
                "after stale archived resume",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("archived resume checks should not leak active capacity");
    }

    #[tokio::test]
    async fn test_persistent_deferred_capacity_releases_after_first_turn() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let staged = service
            .create_session(create_request("staged", InitialTurnPolicy::Defer))
            .await
            .expect("deferred session should be staged");
        service
            .start_turn(&staged.session_id, start_turn_request("materialize"))
            .await
            .expect("first turn should materialize the deferred session");
        service
            .create_session(create_request("next staged", InitialTurnPolicy::Defer))
            .await
            .expect("completed first turn should release deferred capacity");
    }

    #[tokio::test]
    async fn test_persistent_runtime_turn_waits_for_archive_gate() {
        let store = Arc::new(BlockingArchiveSaveStore::new());
        let service = Arc::new(PersistentSessionService::new(
            DummyBuilder,
            2,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        ));
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create live session");

        store.block_archived_saves();
        let archive_service = Arc::clone(&service);
        let archive_id = created.session_id.clone();
        let archive_task = tokio::spawn(async move { archive_service.archive(&archive_id).await });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_archived_save(),
        )
        .await
        .expect("archive should reach blocked durable save");

        let turn_service = Arc::clone(&service);
        let turn_id = created.session_id.clone();
        let mut turn_task = Box::pin(tokio::spawn(async move {
            turn_service
                .apply_runtime_turn(
                    &turn_id,
                    RunId::new(),
                    runtime_content_turn_request("turn during archive"),
                    RunApplyBoundary::RunStart,
                    vec![meerkat_core::lifecycle::InputId::new()],
                )
                .await
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut turn_task)
                .await
                .is_err(),
            "runtime turn should wait while archive owns the per-session gate"
        );

        store.release_archived_save();
        archive_task
            .await
            .expect("archive task should join")
            .expect("archive should succeed");
        let turn_result = turn_task.await.expect("turn task should join");
        assert!(
            matches!(turn_result, Err(SessionError::NotFound { .. })),
            "turn that waited behind archive should see archived session as not found: {turn_result:?}"
        );
    }

    #[tokio::test]
    async fn test_persistent_context_only_runtime_apply_waits_for_archive_gate() {
        let store = Arc::new(BlockingArchiveSaveStore::new());
        let service = Arc::new(PersistentSessionService::new(
            DummyBuilder,
            2,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        ));
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create live session");

        store.block_archived_saves();
        let archive_service = Arc::clone(&service);
        let archive_id = created.session_id.clone();
        let archive_task = tokio::spawn(async move { archive_service.archive(&archive_id).await });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_archived_save(),
        )
        .await
        .expect("archive should reach blocked durable save");

        let apply_service = Arc::clone(&service);
        let apply_id = created.session_id.clone();
        let mut apply_task = Box::pin(tokio::spawn(async move {
            apply_service
                .apply_runtime_context_appends(
                    &apply_id,
                    RunId::new(),
                    vec![PendingSystemContextAppend {
                        text: "context during archive".to_string(),
                        source: Some("test".to_string()),
                        idempotency_key: Some("archive-race".to_string()),
                        accepted_at: meerkat_core::time_compat::SystemTime::now(),
                    }],
                    vec![meerkat_core::lifecycle::InputId::new()],
                )
                .await
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut apply_task)
                .await
                .is_err(),
            "context-only runtime apply should wait while archive owns the per-session gate"
        );

        store.release_archived_save();
        archive_task
            .await
            .expect("archive task should join")
            .expect("archive should succeed");
        let apply_result = apply_task.await.expect("apply task should join");
        assert!(
            matches!(apply_result, Err(SessionError::NotFound { .. })),
            "context-only apply that waited behind archive should see archived session as not found: {apply_result:?}"
        );
    }

    #[tokio::test]
    async fn test_persistent_external_user_append_waits_for_archive_gate() {
        let store = Arc::new(BlockingArchiveSaveStore::new());
        let service = Arc::new(PersistentSessionService::new(
            DummyBuilder,
            2,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        ));
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create live session");

        store.block_archived_saves();
        let archive_service = Arc::clone(&service);
        let archive_id = created.session_id.clone();
        let archive_task = tokio::spawn(async move { archive_service.archive(&archive_id).await });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_archived_save(),
        )
        .await
        .expect("archive should reach blocked durable save");

        let append_service = Arc::clone(&service);
        let append_id = created.session_id.clone();
        let mut append_task = Box::pin(tokio::spawn(async move {
            append_service
                .append_external_user_content(
                    &append_id,
                    ContentInput::Text("external user during archive".to_string()),
                )
                .await
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut append_task)
                .await
                .is_err(),
            "external user append should wait while archive owns the per-session gate"
        );

        store.release_archived_save();
        archive_task
            .await
            .expect("archive task should join")
            .expect("archive should succeed");
        let append_result = append_task.await.expect("append task should join");
        assert!(
            matches!(append_result, Err(SessionError::NotFound { .. })),
            "external user append that waited behind archive should see archived session as not found: {append_result:?}"
        );
    }

    #[tokio::test]
    async fn test_persistent_external_assistant_append_waits_for_archive_gate() {
        let store = Arc::new(BlockingArchiveSaveStore::new());
        let service = Arc::new(PersistentSessionService::new(
            DummyBuilder,
            2,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        ));
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create live session");

        store.block_archived_saves();
        let archive_service = Arc::clone(&service);
        let archive_id = created.session_id.clone();
        let archive_task = tokio::spawn(async move { archive_service.archive(&archive_id).await });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_archived_save(),
        )
        .await
        .expect("archive should reach blocked durable save");

        let append_service = Arc::clone(&service);
        let append_id = created.session_id.clone();
        let mut append_task = Box::pin(tokio::spawn(async move {
            append_service
                .append_external_assistant_output(
                    &append_id,
                    vec![AssistantBlock::Text {
                        text: "external assistant during archive".to_string(),
                        meta: None,
                    }],
                    StopReason::EndTurn,
                    Usage::default(),
                )
                .await
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut append_task)
                .await
                .is_err(),
            "external assistant append should wait while archive owns the per-session gate"
        );

        store.release_archived_save();
        archive_task
            .await
            .expect("archive task should join")
            .expect("archive should succeed");
        let append_result = append_task.await.expect("append task should join");
        assert!(
            matches!(append_result, Err(SessionError::NotFound { .. })),
            "external assistant append that waited behind archive should see archived session as not found: {append_result:?}"
        );
    }

    #[tokio::test]
    async fn test_persistent_keep_alive_update_waits_for_archive_gate() {
        let store = Arc::new(BlockingArchiveSaveStore::new());
        let service = Arc::new(PersistentSessionService::new(
            DummyBuilder,
            2,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        ));
        let created = service
            .create_session(create_request_with_metadata(
                "seed",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create live session");

        store.block_archived_saves();
        let archive_service = Arc::clone(&service);
        let archive_id = created.session_id.clone();
        let archive_task = tokio::spawn(async move { archive_service.archive(&archive_id).await });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_archived_save(),
        )
        .await
        .expect("archive should reach blocked durable save");

        let update_service = Arc::clone(&service);
        let update_id = created.session_id.clone();
        let mut update_task = Box::pin(tokio::spawn(async move {
            update_service
                .apply_runtime_session_keep_alive(&update_id, true)
                .await
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut update_task)
                .await
                .is_err(),
            "keep-alive update should wait while archive owns the per-session gate"
        );

        store.release_archived_save();
        archive_task
            .await
            .expect("archive task should join")
            .expect("archive should succeed");
        let update_result = update_task.await.expect("update task should join");
        assert!(
            matches!(update_result, Err(SessionError::NotFound { .. })),
            "keep-alive update that waited behind archive should see archived session as not found: {update_result:?}"
        );
    }

    #[tokio::test]
    async fn test_persistent_external_tool_dispatch_waits_for_archive_gate() {
        let store = Arc::new(BlockingArchiveSaveStore::new());
        let service = Arc::new(PersistentSessionService::new(
            ToolDispatchBuilder,
            2,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        ));
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create live session");

        store.block_archived_saves();
        let archive_service = Arc::clone(&service);
        let archive_id = created.session_id.clone();
        let archive_task = tokio::spawn(async move { archive_service.archive(&archive_id).await });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_archived_save(),
        )
        .await
        .expect("archive should reach blocked durable save");

        let dispatch_service = Arc::clone(&service);
        let dispatch_id = created.session_id.clone();
        let mut dispatch_task = Box::pin(tokio::spawn(async move {
            dispatch_service
                .dispatch_external_tool_call(
                    &dispatch_id,
                    ToolCall::new(
                        "call-during-archive".to_string(),
                        "tool_catalog_load".to_string(),
                        serde_json::json!({}),
                    ),
                )
                .await
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut dispatch_task)
                .await
                .is_err(),
            "external tool dispatch should wait while archive owns the per-session gate"
        );

        store.release_archived_save();
        archive_task
            .await
            .expect("archive task should join")
            .expect("archive should succeed");
        let dispatch_result = dispatch_task.await.expect("dispatch task should join");
        assert!(
            matches!(dispatch_result, Err(SessionError::NotFound { .. })),
            "external tool dispatch that waited behind archive should see archived session as not found: {dispatch_result:?}"
        );
    }

    #[tokio::test]
    async fn test_persistent_eager_create_bounds_agent_builds() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let builder = BlockingBuildBuilder::new();
        let service = Arc::new(PersistentSessionService::new(
            builder.clone(),
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        ));

        let service_for_first = Arc::clone(&service);
        let first = tokio::spawn(async move {
            service_for_first
                .create_session(create_request("first", InitialTurnPolicy::RunImmediately))
                .await
        });

        builder.wait_for_entered_builds(1).await;
        let blocked = service
            .create_session(create_request("blocked", InitialTurnPolicy::RunImmediately))
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "second eager create should be rejected before entering build: {blocked:?}"
        );
        assert_eq!(
            builder.entered_builds.load(Ordering::Acquire),
            1,
            "capacity should prevent the second create from starting agent build"
        );
        assert_eq!(
            builder.max_concurrent_builds.load(Ordering::Acquire),
            1,
            "agent build concurrency must stay bounded by max_sessions"
        );

        builder.release_notify.add_permits(1);
        first
            .await
            .expect("first create task should join")
            .expect("first eager create should complete");
    }

    #[tokio::test]
    async fn test_ephemeral_archive_during_deferred_first_turn_keeps_capacity_until_turn_stops() {
        let builder = BlockingRunBuilder::new();
        let service = Arc::new(EphemeralSessionService::new(builder.clone(), 1));

        let staged = service
            .create_session(create_request("staged", InitialTurnPolicy::Defer))
            .await
            .expect("deferred session should be staged");
        let service_for_turn = Arc::clone(&service);
        let session_for_turn = staged.session_id.clone();
        let first_turn = tokio::spawn(async move {
            service_for_turn
                .start_turn(&session_for_turn, start_turn_request("materialize"))
                .await
        });

        builder.wait_for_entered_runs(1).await;
        service
            .archive(&staged.session_id)
            .await
            .expect("archive should request shutdown while first turn runs");
        let blocked = service
            .create_session(create_request("blocked", InitialTurnPolicy::Defer))
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "archive during a deferred first turn must not release capacity early: {blocked:?}"
        );

        builder.release_notify.add_permits(1);
        let _ = first_turn.await.expect("first turn task should join");
        service
            .create_session(create_request("after turn", InitialTurnPolicy::Defer))
            .await
            .expect("capacity should release after archived turn stops");
    }

    #[tokio::test]
    async fn test_persistent_discard_during_deferred_first_turn_keeps_capacity_until_turn_stops() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let builder = BlockingRunBuilder::new();
        let service = Arc::new(PersistentSessionService::new(
            builder.clone(),
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        ));

        let staged = service
            .create_session(create_request("staged", InitialTurnPolicy::Defer))
            .await
            .expect("deferred session should be staged");
        let service_for_turn = Arc::clone(&service);
        let session_for_turn = staged.session_id.clone();
        let first_turn = tokio::spawn(async move {
            service_for_turn
                .start_turn(&session_for_turn, start_turn_request("materialize"))
                .await
        });

        builder.wait_for_entered_runs(1).await;
        service
            .discard_live_session(&staged.session_id)
            .await
            .expect("discard should request shutdown while first turn runs");
        let blocked = service
            .create_session(create_request("blocked", InitialTurnPolicy::Defer))
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "discard during a deferred first turn must not release capacity early: {blocked:?}"
        );

        builder.release_notify.add_permits(1);
        let _ = first_turn.await.expect("first turn task should join");
        service
            .create_session(create_request("after turn", InitialTurnPolicy::Defer))
            .await
            .expect("capacity should release after discarded turn stops");
    }

    #[tokio::test]
    async fn test_append_system_context_repersist_live_session_when_store_row_missing() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let id = result.session_id;

        store
            .delete(&id)
            .await
            .expect("test should be able to remove persisted row");
        assert!(
            store.load(&id).await.unwrap().is_none(),
            "store row should be absent before append"
        );

        let result = service
            .append_system_context(
                &id,
                AppendSystemContextRequest {
                    text: "runtime notice".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-persistent-live".to_string()),
                },
            )
            .await
            .expect("live append should repersist from the live session snapshot");
        assert_eq!(
            result.status,
            meerkat_core::AppendSystemContextStatus::Staged
        );

        let stored = store
            .load(&id)
            .await
            .expect("load should succeed")
            .expect("append should restore the persisted row");
        let state = stored
            .system_context_state()
            .expect("restored row should contain pending system-context state");
        assert_eq!(state.pending().len(), 1);
        assert_eq!(state.pending()[0].text, "runtime notice");
    }

    #[tokio::test]
    async fn test_append_system_context_live_save_failure_does_not_mutate_runtime_state() {
        let store = Arc::new(FailSaveStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create_session should succeed");
        let id = result.session_id;

        store.set_fail_save(true);
        let err = service
            .append_system_context(
                &id,
                AppendSystemContextRequest {
                    text: "runtime notice".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-save-failure".to_string()),
                },
            )
            .await
            .expect_err("append should surface the store failure");
        assert_eq!(err.code(), "SESSION_STORE_ERROR");

        let state = service
            .inner
            .system_context_state(&id)
            .await
            .expect("live session should still exist");
        let guard = state.snapshot();
        assert!(
            guard.pending().is_empty(),
            "failed append must not mutate live runtime state"
        );
        assert!(
            !guard.seen().contains_key("ctx-save-failure"),
            "failed append must not reserve the idempotency key in live state"
        );
    }

    #[tokio::test]
    async fn test_export_live_session_merges_shared_system_context_state() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service =
            PersistentSessionService::new(DummyBuilder, 4, store, None, memory_blob_store());

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create_session should succeed");
        let id = result.session_id;

        let state = service
            .inner
            .system_context_state(&id)
            .await
            .expect("live session should expose shared system-context state");
        state
            .stage_append_with_snapshot(
                &AppendSystemContextRequest {
                    text: "queued live append".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-export-merge".to_string()),
                },
                meerkat_core::time_compat::SystemTime::now(),
            )
            .expect("staging into shared state should succeed");

        let exported = service
            .export_live_session(&id)
            .await
            .expect("export should succeed");
        let exported_state = exported
            .system_context_state()
            .expect("exported session should include merged system-context state");
        assert_eq!(exported_state.pending().len(), 1);
        assert_eq!(exported_state.pending()[0].text, "queued live append");
        assert!(
            exported_state.seen().contains_key("ctx-export-merge"),
            "exported session should include the staged idempotency key"
        );
    }

    #[tokio::test]
    async fn test_append_system_context_unknown_session_does_not_allocate_gate() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let unknown = SessionId::new();

        let err = service
            .append_system_context(
                &unknown,
                AppendSystemContextRequest {
                    text: "runtime notice".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-unknown".to_string()),
                },
            )
            .await
            .expect_err("unknown session must fail");
        assert_eq!(err.code(), "SESSION_NOT_FOUND");
        assert!(
            service.checkpointer_gates.lock().await.is_empty(),
            "unknown-session append must not allocate a checkpointer gate"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_eager_create_session_is_rejected_before_authority_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let error = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect_err("runtime-backed eager create_session must be removed");
        assert_runtime_backed_eager_create_rejected(&error);
        assert!(
            service.checkpointer_gates.lock().await.is_empty(),
            "rejected eager create must not allocate persistence/checkpointer state"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_list_retains_projected_session_after_live_handle_discard() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("test should be able to discard the live session");

        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");
        assert!(
            listed
                .iter()
                .any(|summary| summary.session_id == result.session_id),
            "runtime-backed sessions should remain listable through the session-store projection after the live handle is gone"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_direct_start_turn_is_rejected_before_projection_or_runtime_mutation()
     {
        let fail_store = Arc::new(FailSaveStore::new());
        let store = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed before projection failures");
        let raw_before = store
            .load(&result.session_id)
            .await
            .expect("raw projection load should succeed")
            .expect("initial projection should exist");
        let initial_authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should exist before rejected direct turn");

        fail_store.set_fail_save(true);
        let error = service
            .start_turn(
                &result.session_id,
                start_turn_request("runtime-backed direct start_turn must not commit"),
            )
            .await
            .expect_err("runtime-backed direct start_turn must be removed");
        assert_runtime_backed_direct_start_turn_rejected(&error);

        fail_store.set_fail_save(false);
        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should remain present after rejected direct turn");
        assert_eq!(
            authoritative.messages().len(),
            initial_authoritative.messages().len(),
            "rejected direct start_turn must not advance runtime authority"
        );
        let raw_after = store
            .load(&result.session_id)
            .await
            .expect("raw projection load should succeed after rejection")
            .expect("projection should remain present");
        assert_eq!(
            raw_after.messages().len(),
            raw_before.messages().len(),
            "rejected direct start_turn must not update the session-store projection"
        );
        assert!(
            service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed after rejection"),
            "rejection happens before live state is mutated or evicted"
        );
    }

    #[tokio::test]
    async fn test_machine_committed_live_turn_rejects_mismatched_runtime_authority() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service_runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let builder = BlockingRunBuilder::new();
        let service = PersistentSessionService::new(
            builder.clone(),
            4,
            Arc::clone(&store),
            Some(Arc::clone(&service_runtime_store)),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let raw_before = store
            .load(&created.session_id)
            .await
            .expect("raw projection load should succeed")
            .expect("initial projection should exist");
        let authoritative_before = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should exist before rejected protocol");
        let admission = service
            .reserve_runtime_turn_admission(&created.session_id)
            .await
            .expect("runtime turn admission should be reserved");
        let mismatched_runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let mismatched_machine =
            meerkat_runtime::MeerkatMachine::persistent_without_blobs(mismatched_runtime_store);

        let (error, returned_admission) = service
            .run_machine_committed_live_turn(
                MachineServiceTurnCommitProtocol::from_machine(&mismatched_machine),
                &created.session_id,
                start_turn_request("must not start"),
                admission,
            )
            .await
            .expect_err("mismatched machine/service runtime authority must fail closed");

        assert!(
            returned_admission.is_some(),
            "authority rejection must return the unconsumed admission guard"
        );
        match error {
            SessionError::Unsupported(message) => assert!(
                message.contains("runtime authority does not match"),
                "unexpected unsupported error: {message}"
            ),
            other => panic!("expected unsupported authority rejection, got {other:?}"),
        }
        assert_eq!(
            builder.entered_runs.load(Ordering::Acquire),
            0,
            "authority rejection must happen before the live turn starts"
        );
        let authoritative_after = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("authoritative load should still succeed")
            .expect("runtime authority should remain present");
        assert_eq!(
            authoritative_after.messages().len(),
            authoritative_before.messages().len(),
            "rejected protocol must not advance runtime authority"
        );
        let raw_after = store
            .load(&created.session_id)
            .await
            .expect("raw projection load should still succeed")
            .expect("projection should remain present");
        assert_eq!(
            raw_after.messages().len(),
            raw_before.messages().len(),
            "rejected protocol must not update the session-store projection"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_callback_pending_direct_start_turn_cannot_bypass_machine_commit() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            CallbackPendingBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed before projection failures");
        let raw_before = store
            .load(&result.session_id)
            .await
            .expect("raw projection load should succeed")
            .expect("initial projection should exist");
        let initial_authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should exist before rejected direct turn");

        fail_store.set_fail_save(true);
        let error = service
            .start_turn(
                &result.session_id,
                start_turn_request("callback pending direct path must not commit"),
            )
            .await
            .expect_err("runtime-backed direct start_turn must be removed before builder output");
        assert_runtime_backed_direct_start_turn_rejected(&error);

        fail_store.set_fail_save(false);
        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should remain present after rejected direct turn");
        assert_eq!(
            authoritative.messages().len(),
            initial_authoritative.messages().len(),
            "rejected direct start_turn must not advance runtime authority"
        );
        let raw_after = store
            .load(&result.session_id)
            .await
            .expect("raw projection load should succeed after rejection")
            .expect("projection should remain present");
        assert_eq!(
            raw_after.messages().len(),
            raw_before.messages().len(),
            "rejected direct start_turn must not update the session-store projection"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_resume_eager_create_is_rejected_before_projection_or_runtime_mutation()
     {
        let fail_store = Arc::new(FailSaveStore::new());
        let store = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            CallbackPendingBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed before projection failures");
        let resume_source = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should exist");
        let raw_before = store
            .load(&result.session_id)
            .await
            .expect("raw projection load should succeed")
            .expect("initial projection should exist");
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("test should model resume after live handle discard");

        let mut req = create_request(
            "resume callback pending after projection failure",
            meerkat_core::service::InitialTurnPolicy::RunImmediately,
        );
        req.build = Some(SessionBuildOptions {
            resume_session: Some(resume_source),
            ..Default::default()
        });

        fail_store.set_fail_save(true);
        let error = service
            .create_session(req)
            .await
            .expect_err("runtime-backed eager resume create must be removed");
        assert_runtime_backed_eager_create_rejected(&error);
        assert!(
            !service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed after eager create rejection"),
            "eager create rejection happens before recovering live state"
        );
        fail_store.set_fail_save(false);
        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should remain unchanged after eager create rejection");
        assert_eq!(
            authoritative.messages().len(),
            raw_before.messages().len(),
            "runtime authority must not advance through rejected eager create"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_direct_start_turn_rejection_ignores_snapshot_store_failures() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let initial_authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should exist before rejected direct turn");

        runtime_store.set_fail_snapshot_commits(true);
        let error = service
            .start_turn(
                &result.session_id,
                start_turn_request("removed direct path must not reach snapshot commit"),
            )
            .await
            .expect_err("runtime-backed direct start_turn must be rejected before snapshot commit");
        assert_runtime_backed_direct_start_turn_rejected(&error);
        assert!(
            service
                .has_live_session(&result.session_id)
                .await
                .expect("live-session status should succeed"),
            "rejection happens before live state is mutated or evicted"
        );

        runtime_store.set_fail_snapshot_commits(false);
        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("created session should remain durable");
        assert_eq!(
            authoritative.messages().len(),
            initial_authoritative.messages().len(),
            "failed direct start_turn must not advance runtime authority"
        );
        assert!(
            authoritative.messages().iter().all(|message| {
                !format!("{message:?}")
                    .contains("removed direct path must not reach snapshot commit")
            }),
            "rejected direct start_turn must not leak into durable authority"
        );

        let view = service
            .read(&result.session_id)
            .await
            .expect("read should fall back to durable runtime authority");
        assert_eq!(
            view.state.message_count,
            initial_authoritative.messages().len(),
            "read() must not report the rejected live mutation as clean truth"
        );
        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed after rejected direct start_turn");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == result.session_id)
            .expect("durable session should remain listable");
        assert_eq!(
            summary.message_count,
            initial_authoritative.messages().len(),
            "list() must not report the failed live mutation as clean truth"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_control_projection_save_failure_discards_live_state() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let append_result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed before projection failures");

        fail_store.set_fail_save(true);
        let append_error = service
            .append_system_context(
                &append_result.session_id,
                AppendSystemContextRequest {
                    text: "runtime-backed control context".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("control-projection-failure".to_string()),
                },
            )
            .await
            .expect_err("control projection save failure must fail closed");
        assert!(
            matches!(
                append_error,
                SessionControlError::Session(SessionError::Store(_))
            ),
            "projection failure should surface as a store error, got {append_error:?}"
        );
        assert!(
            !service
                .has_live_session(&append_result.session_id)
                .await
                .expect("status should succeed after append projection failure"),
            "append projection failure after runtime commit must evict stale live state"
        );
        let append_authoritative = service
            .load_authoritative_session(&append_result.session_id)
            .await
            .expect("authoritative load should succeed after append failure")
            .expect("runtime authority should retain the committed append");
        let append_state = append_authoritative
            .system_context_state()
            .expect("runtime authority should carry the committed append");
        assert_eq!(append_state.pending().len(), 1);
        assert_eq!(
            append_state.pending()[0].text,
            "runtime-backed control context"
        );
        let append_raw = store
            .load(&append_result.session_id)
            .await
            .expect("raw projection load should succeed after append failure")
            .expect("stale append projection should remain present");
        let raw_pending_context_count = append_raw
            .system_context_state()
            .map_or(0, |state| state.pending().len());
        assert_eq!(
            raw_pending_context_count, 0,
            "failed append projection update must not silently refresh raw store state"
        );

        fail_store.set_fail_save(false);
        let stage_result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("second create_session should succeed before projection failures");

        fail_store.set_fail_save(true);
        let stage_error = service
            .stage_tool_results(
                &stage_result.session_id,
                StageToolResultsRequest {
                    results: vec![ToolResult::new(
                        "tool-call-1".to_string(),
                        "callback result".to_string(),
                        false,
                    )],
                },
            )
            .await
            .expect_err("staged tool-result projection save failure must fail closed");
        assert!(
            matches!(stage_error, SessionError::Store(_)),
            "projection failure should surface as a store error, got {stage_error:?}"
        );
        assert!(
            !service
                .has_live_session(&stage_result.session_id)
                .await
                .expect("status should succeed after stage projection failure"),
            "stage projection failure after runtime commit must evict stale live state"
        );
        let stage_authoritative = service
            .load_authoritative_session(&stage_result.session_id)
            .await
            .expect("authoritative load should succeed after stage failure")
            .expect("runtime authority should retain the staged tool results");
        let stage_deferred = stage_authoritative
            .deferred_turn_state()
            .expect("runtime authority should carry staged tool results");
        assert_eq!(stage_deferred.pending_tool_results().len(), 1);
        let stage_raw = store
            .load(&stage_result.session_id)
            .await
            .expect("raw projection load should succeed after stage failure")
            .expect("stale stage projection should remain present");
        let raw_tool_result_count = stage_raw
            .deferred_turn_state()
            .map_or(0, |state| state.pending_tool_results().len());
        assert_eq!(
            raw_tool_result_count, 0,
            "failed stage projection update must not silently refresh raw store state"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_follow_up_start_turn_must_use_machine_commit_protocol() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let id = result.session_id;
        let initial_count = service
            .load_authoritative_session(&id)
            .await
            .expect("authoritative load should succeed")
            .expect("create_session should persist authoritative snapshot")
            .messages()
            .len();

        let error = service
            .start_turn(&id, start_turn_request("follow up"))
            .await
            .expect_err("runtime-backed follow-up start_turn must be removed");
        assert_runtime_backed_direct_start_turn_rejected(&error);

        let stored = service
            .load_authoritative_session(&id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime-backed session should remain durable");
        assert_eq!(
            stored.messages().len(),
            initial_count,
            "rejected follow-up direct start_turn must not update authoritative snapshot"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_stages_successfully() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        let append = service
            .append_system_context(
                &result.session_id,
                AppendSystemContextRequest {
                    text: "Remember runtime-backed context".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("runtime-backed-ctx".to_string()),
                },
            )
            .await
            .expect("runtime-backed append_system_context should remain available");
        assert_eq!(
            append.status,
            meerkat_core::AppendSystemContextStatus::Staged
        );

        let stored = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("append should persist updated system-context state");
        let state = stored
            .system_context_state()
            .expect("runtime-backed append should persist pending context");
        assert_eq!(state.pending().len(), 1);
        assert_eq!(state.pending()[0].text, "Remember runtime-backed context");
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_does_not_persist_uncommitted_live_state() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        service
            .inner
            .apply_runtime_system_context(
                &result.session_id,
                vec![PendingSystemContextAppend {
                    text: "uncommitted live context".to_string(),
                    source: Some("live".to_string()),
                    idempotency_key: Some("uncommitted".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
            )
            .await
            .expect("live runtime context mutation should succeed");

        service
            .append_system_context(
                &result.session_id,
                AppendSystemContextRequest {
                    text: "durable pending context".to_string(),
                    source: Some("api".to_string()),
                    idempotency_key: Some("durable-pending".to_string()),
                },
            )
            .await
            .expect("append_system_context should succeed");

        let stored = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session snapshot should exist");
        assert!(
            stored.messages().is_empty(),
            "runtime-backed append_system_context must not snapshot uncommitted live session messages into durable authority"
        );
        let state = stored
            .system_context_state()
            .expect("runtime-backed append should persist pending control state");
        assert_eq!(state.pending().len(), 1);
        assert_eq!(state.pending()[0].text, "durable pending context");
        let raw = store
            .load(&result.session_id)
            .await
            .expect("raw store load should succeed")
            .expect("session-store projection should be kept for listability");
        assert!(
            raw.messages().is_empty(),
            "projection must not include uncommitted live session messages"
        );
        let raw_state = raw
            .system_context_state()
            .expect("projection should mirror committed control state");
        assert_eq!(raw_state.pending().len(), 1);
        assert_eq!(raw_state.pending()[0].text, "durable pending context");
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_persists_snapshot_without_boundary_receipt()
    {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        runtime_store.reset_boundary_commits().await;

        service
            .append_system_context(
                &result.session_id,
                AppendSystemContextRequest {
                    text: "control snapshot should not mint a receipt".to_string(),
                    source: Some("api".to_string()),
                    idempotency_key: Some("no-receipt-control-snapshot".to_string()),
                },
            )
            .await
            .expect("append_system_context should succeed");

        let boundary_commits = runtime_store.boundary_commits().await;
        assert!(
            boundary_commits.is_empty(),
            "non-run control snapshots must not mint synthetic boundary receipts: {boundary_commits:?}"
        );

        let runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&result.session_id);
        let snapshot = runtime_store
            .load_session_snapshot(&runtime_id)
            .await
            .expect("runtime snapshot load should succeed")
            .expect("control snapshot should still be durable");
        let stored: Session =
            serde_json::from_slice(&snapshot).expect("runtime snapshot should deserialize");
        let state = stored
            .system_context_state()
            .expect("runtime snapshot should carry pending control state");
        assert_eq!(state.pending().len(), 1);
        assert_eq!(
            state.pending()[0].text,
            "control snapshot should not mint a receipt"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_uses_newer_runtime_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        let output = service
            .apply_runtime_turn(
                &result.session_id,
                RunId::new(),
                runtime_content_turn_request("runtime committed turn"),
                RunApplyBoundary::Immediate,
                vec![],
            )
            .await
            .expect("runtime-backed turn should succeed");
        machine_commit_runtime_output(runtime_store.as_ref(), &result.session_id, &output).await;

        let stale_store_row = store
            .load(&result.session_id)
            .await
            .expect("raw store load should succeed");
        assert!(
            stale_store_row
                .as_ref()
                .is_none_or(|session| session.messages().is_empty()),
            "session store should still lag behind the runtime-backed committed snapshot before append"
        );
        assert!(
            runtime_store
                .load_session_snapshot(
                    &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(
                        &result.session_id
                    )
                )
                .await
                .expect("runtime snapshot load should succeed")
                .is_some(),
            "machine-owned runtime turn commit should persist a runtime snapshot"
        );

        service
            .append_system_context(
                &result.session_id,
                AppendSystemContextRequest {
                    text: "runtime append".to_string(),
                    source: Some("api".to_string()),
                    idempotency_key: Some("runtime-base".to_string()),
                },
            )
            .await
            .expect("append_system_context should succeed");

        let stored = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("append should persist a refreshed authoritative snapshot");
        assert_eq!(
            stored.messages().len(),
            2,
            "append must preserve the newest runtime-committed conversation state instead of rewinding to the stale session-store row"
        );
        let state = stored
            .system_context_state()
            .expect("append should persist pending control state");
        assert_eq!(state.pending().len(), 1);
        assert_eq!(state.pending()[0].text, "runtime append");
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_without_live_handle_uses_runtime_authority()
    {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        let output = service
            .apply_runtime_turn(
                &result.session_id,
                RunId::new(),
                runtime_content_turn_request("runtime committed turn"),
                RunApplyBoundary::Immediate,
                vec![],
            )
            .await
            .expect("runtime-backed turn should succeed");
        machine_commit_runtime_output(runtime_store.as_ref(), &result.session_id, &output).await;
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("test should be able to discard the live session");

        let append = service
            .append_system_context(
                &result.session_id,
                AppendSystemContextRequest {
                    text: "persisted runtime append".to_string(),
                    source: Some("api".to_string()),
                    idempotency_key: Some("persisted-runtime-append".to_string()),
                },
            )
            .await
            .expect("persisted runtime-backed append should update runtime authority");
        assert_eq!(
            append.status,
            meerkat_core::AppendSystemContextStatus::Staged
        );

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session should exist");
        assert_eq!(
            authoritative.messages().len(),
            2,
            "persisted append must preserve committed runtime transcript truth"
        );
        let state = authoritative
            .system_context_state()
            .expect("runtime authority should carry pending context");
        assert_eq!(state.pending().len(), 1);
        assert_eq!(state.pending()[0].text, "persisted runtime append");

        let view = service
            .read(&result.session_id)
            .await
            .expect("read should use durable runtime truth after live discard");
        assert_eq!(view.state.message_count, authoritative.messages().len());
        assert!(!view.state.is_active);
        assert!(
            !service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed"),
            "status should not recreate a live handle"
        );
        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == result.session_id)
            .expect("list should include persisted runtime-backed session");
        assert_eq!(summary.message_count, authoritative.messages().len());

        let restarted = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );
        let resume_source = restarted
            .load_authoritative_session(&result.session_id)
            .await
            .expect("resume source should load from runtime authority")
            .expect("runtime authority should remain present after restart");
        let resumed = restarted
            .create_session(resume_request(resume_source))
            .await
            .expect("resume should materialize the runtime-authoritative snapshot");
        assert_eq!(resumed.session_id, result.session_id);
        let resumed_live = restarted
            .export_live_session(&result.session_id)
            .await
            .expect("resumed live session should export");
        let resumed_state = resumed_live
            .system_context_state()
            .expect("resumed session should preserve runtime-authoritative pending context");
        assert_eq!(resumed_state.pending()[0].text, "persisted runtime append");
    }

    #[tokio::test]
    async fn test_authoritative_runtime_snapshot_ignores_store_owned_session_metadata() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request_with_metadata(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let mut store_session = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("initial runtime snapshot should exist");
        store_session
            .set_build_state(meerkat_core::SessionBuildState {
                system_prompt: Some("store-owned recovery prompt".to_string()),
                ..Default::default()
            })
            .expect("build state should serialize");
        store
            .save(&store_session)
            .await
            .expect("test should seed a compatibility store projection");

        let mut runtime_session = store_session.clone();
        runtime_session.push(Message::User(UserMessage::text_with_render_metadata(
            "runtime-only turn",
            None,
        )));
        let mut runtime_session_value =
            serde_json::to_value(&runtime_session).expect("runtime snapshot should serialize");
        runtime_session_value
            .get_mut("metadata")
            .and_then(serde_json::Value::as_object_mut)
            .expect("serialized session metadata should be an object")
            .retain(|key, _| {
                key != SESSION_METADATA_KEY && key != meerkat_core::SESSION_BUILD_STATE_KEY
            });
        let session_snapshot = serde_json::to_vec(&runtime_session_value)
            .expect("runtime snapshot value should serialize");
        runtime_store
            .commit_session_snapshot(
                &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(
                    &result.session_id,
                ),
                meerkat_runtime::SessionDelta { session_snapshot },
            )
            .await
            .expect("runtime snapshot commit should succeed");

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session should exist");

        assert_eq!(
            authoritative.messages().len(),
            runtime_session.messages().len(),
            "newer runtime snapshot still owns committed conversation state"
        );
        assert_eq!(
            authoritative.updated_at(),
            runtime_session.updated_at(),
            "store projection metadata must not change runtime snapshot timestamps"
        );
        assert!(
            authoritative.session_metadata().is_none(),
            "runtime authority must not backfill metadata from the store projection"
        );
        assert!(
            authoritative.build_state().is_none(),
            "runtime authority must not backfill build state from the store projection"
        );
    }

    #[tokio::test]
    async fn test_persisted_runtime_state_loads_runtime_store_state() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );
        let session_id = SessionId::new();
        let runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&session_id);
        let machine = meerkat_runtime::MeerkatMachine::persistent(
            runtime_store.clone() as Arc<dyn RuntimeStore>,
            memory_blob_store(),
        );
        machine.register_session(session_id.clone()).await;
        meerkat_runtime::RuntimeControlPlane::retire(&machine, &runtime_id)
            .await
            .expect("runtime state should persist");

        assert_eq!(
            service
                .persisted_runtime_state(&session_id)
                .await
                .expect("runtime state load should succeed"),
            Some(meerkat_runtime::RuntimeState::Retired)
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_authoritative_load_ignores_store_only_projection() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let mut raw_projection = Session::new();
        raw_projection.push(Message::User(UserMessage::text(
            "legacy raw store row".to_string(),
        )));
        let id = raw_projection.id().clone();
        store
            .save(&raw_projection)
            .await
            .expect("test should seed a raw store projection");

        let authoritative = service
            .load_authoritative_session(&id)
            .await
            .expect("authoritative load should succeed");
        assert!(
            authoritative.is_none(),
            "store-only projections must not be promoted into runtime authority"
        );
        assert!(
            runtime_store
                .load_session_snapshot(
                    &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id)
                )
                .await
                .expect("runtime snapshot load should succeed")
                .is_none(),
            "authoritative load must not create runtime authority from a store-only projection"
        );
        let raw_store_row = store
            .load(&id)
            .await
            .expect("raw store load should succeed")
            .expect("test projection should remain present");
        assert_eq!(
            raw_store_row.messages().len(),
            raw_projection.messages().len(),
            "compatibility projection should remain inert in the raw store"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_authoritative_load_ignores_legacy_session_uuid_runtime_alias() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let mut legacy_runtime_session = Session::new();
        legacy_runtime_session.push(Message::User(UserMessage::text(
            "legacy runtime alias row".to_string(),
        )));
        let id = legacy_runtime_session.id().clone();
        let snapshot =
            serde_json::to_vec(&legacy_runtime_session).expect("legacy session should serialize");
        let legacy_runtime_alias = LogicalRuntimeId::legacy_session_uuid_alias(&id);
        let canonical_runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id);
        assert_ne!(
            canonical_runtime_id, legacy_runtime_alias,
            "canonical runtime id must be distinct from the legacy raw session UUID alias"
        );

        runtime_store
            .commit_session_snapshot(
                &legacy_runtime_alias,
                meerkat_runtime::store::SessionDelta {
                    session_snapshot: snapshot,
                },
            )
            .await
            .expect("test should seed a legacy runtime alias snapshot");

        let authoritative = service
            .load_authoritative_session(&id)
            .await
            .expect("authoritative load should succeed");
        assert!(
            authoritative.is_none(),
            "legacy raw alias snapshots must not drive runtime-backed resume authority"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_authoritative_load_ignores_newer_legacy_runtime_alias_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let mut canonical_session = Session::new();
        canonical_session.push(Message::User(UserMessage::text(
            "canonical runtime alias row".to_string(),
        )));
        let id = canonical_session.id().clone();
        let canonical_runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id);
        runtime_store
            .commit_session_snapshot(
                &canonical_runtime_id,
                meerkat_runtime::store::SessionDelta {
                    session_snapshot: serde_json::to_vec(&canonical_session)
                        .expect("canonical session should serialize"),
                },
            )
            .await
            .expect("test should seed a canonical runtime alias snapshot");

        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        let mut legacy_session = canonical_session.clone();
        legacy_session.push(Message::User(UserMessage::text(
            "newer legacy runtime alias row".to_string(),
        )));
        runtime_store
            .commit_session_snapshot(
                &LogicalRuntimeId::legacy_session_uuid_alias(&id),
                meerkat_runtime::store::SessionDelta {
                    session_snapshot: serde_json::to_vec(&legacy_session)
                        .expect("legacy session should serialize"),
                },
            )
            .await
            .expect("test should seed a newer legacy runtime alias snapshot");

        let authoritative = service
            .load_authoritative_session(&id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime alias snapshot should exist");
        assert_eq!(
            authoritative.messages().len(),
            canonical_session.messages().len(),
            "canonical runtime authority must not be overwritten by a newer legacy raw alias snapshot"
        );
    }

    #[tokio::test]
    async fn test_runtime_authority_ignores_corrupt_legacy_alias_after_canonical_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let mut canonical_session = Session::new();
        canonical_session.push(Message::User(UserMessage::text(
            "canonical runtime alias row".to_string(),
        )));
        let id = canonical_session.id().clone();
        let canonical_runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id);
        runtime_store
            .commit_session_snapshot(
                &canonical_runtime_id,
                meerkat_runtime::store::SessionDelta {
                    session_snapshot: serde_json::to_vec(&canonical_session)
                        .expect("canonical session should serialize"),
                },
            )
            .await
            .expect("test should seed a canonical runtime snapshot");

        runtime_store
            .override_session_snapshot(
                LogicalRuntimeId::legacy_session_uuid_alias(&id),
                b"{not valid json".to_vec(),
            )
            .await;

        let authoritative = service
            .load_authoritative_session(&id)
            .await
            .expect("valid canonical authority must not be poisoned by corrupt legacy fallback")
            .expect("canonical runtime snapshot should exist");
        assert_eq!(
            authoritative.messages().len(),
            canonical_session.messages().len(),
            "canonical runtime authority must win over corrupt legacy fallback data"
        );
    }

    #[tokio::test]
    async fn test_runtime_input_updates_ignore_legacy_alias_load_error_after_canonical_states() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let id = SessionId::new();
        let input_id = InputId::new();
        let canonical_runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id);
        let mut input_state = meerkat_runtime::InputState::new_accepted(input_id.clone());
        input_state.durability = Some(meerkat_runtime::InputDurability::Durable);
        let stored = StoredInputState {
            state: input_state,
            seed: meerkat_runtime::input_state::InputStateSeed::new_accepted(),
        };
        let stored = recovered_input_persistence_record(&canonical_runtime_id, stored);
        runtime_store
            .persist_input_state(&canonical_runtime_id, &stored)
            .await
            .expect("test should seed canonical input state");
        runtime_store
            .fail_input_state_load_for(LogicalRuntimeId::legacy_session_uuid_alias(&id))
            .await;

        let run_id = RunId::new();
        let updates = service
            .runtime_input_updates(&id, &run_id, 7, std::slice::from_ref(&input_id))
            .await
            .expect("legacy input-state load failure must not poison canonical updates");

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].state.input_id, input_id);
        assert_eq!(updates[0].seed.phase, InputLifecycleState::Consumed);
        assert_eq!(updates[0].seed.last_run_id, Some(run_id));
        assert_eq!(updates[0].seed.last_boundary_sequence, Some(7));
    }

    #[tokio::test]
    async fn test_runtime_input_updates_ignore_legacy_alias_load_error_after_empty_canonical_read()
    {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let id = SessionId::new();
        runtime_store
            .fail_input_state_load_for(LogicalRuntimeId::legacy_session_uuid_alias(&id))
            .await;

        let updates = service
            .runtime_input_updates(&id, &RunId::new(), 0, &[])
            .await
            .expect("legacy input-state load failure must not poison empty canonical updates");

        assert!(
            updates.is_empty(),
            "no-contributor boundary should not need legacy input-state data"
        );
    }

    #[tokio::test]
    async fn test_runtime_input_updates_ignores_legacy_alias_when_contributor_missing_from_canonical()
     {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let id = SessionId::new();
        let input_id = InputId::new();
        runtime_store
            .fail_input_state_load_for(LogicalRuntimeId::legacy_session_uuid_alias(&id))
            .await;

        let updates = service
            .runtime_input_updates(&id, &RunId::new(), 0, std::slice::from_ref(&input_id))
            .await
            .expect("legacy input-state load failure must not poison canonical-only updates");

        assert!(
            updates.is_empty(),
            "missing canonical contributor must not be recovered from the legacy alias"
        );
    }

    #[tokio::test]
    async fn test_runtime_input_updates_ignore_newer_stale_legacy_row() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let id = SessionId::new();
        let input_id = InputId::new();
        let canonical_runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id);
        let legacy_runtime_id = LogicalRuntimeId::legacy_session_uuid_alias(&id);

        let mut canonical_state = meerkat_runtime::InputState::new_accepted(input_id.clone());
        canonical_state.durability = Some(meerkat_runtime::InputDurability::Durable);
        canonical_state.recovery_count = 7;
        let canonical_stored = StoredInputState {
            state: canonical_state.clone(),
            seed: meerkat_runtime::input_state::InputStateSeed::new_accepted(),
        };
        let canonical_stored =
            recovered_input_persistence_record(&canonical_runtime_id, canonical_stored);
        runtime_store
            .persist_input_state(&canonical_runtime_id, &canonical_stored)
            .await
            .expect("test should seed canonical input state");

        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        let mut legacy_state = meerkat_runtime::InputState::new_accepted(input_id.clone());
        legacy_state.durability = Some(meerkat_runtime::InputDurability::Durable);
        legacy_state.recovery_count = 99;
        runtime_store
            .persist_input_state(
                &legacy_runtime_id,
                &recovered_input_persistence_record(
                    &legacy_runtime_id,
                    StoredInputState {
                        state: legacy_state,
                        seed: meerkat_runtime::input_state::InputStateSeed::new_accepted(),
                    },
                ),
            )
            .await
            .expect("test should seed newer stale legacy input state");

        let updates = service
            .runtime_input_updates(&id, &RunId::new(), 3, std::slice::from_ref(&input_id))
            .await
            .expect("runtime input updates should read canonical input state only");

        assert_eq!(updates.len(), 1);
        assert_eq!(
            updates[0].state.recovery_count, 7,
            "canonical contributor state must win over newer stale legacy duplicate"
        );
        assert_eq!(updates[0].seed.phase, InputLifecycleState::Consumed);
        assert_eq!(updates[0].seed.last_boundary_sequence, Some(3));
    }

    #[tokio::test]
    async fn test_runtime_backed_read_and_list_ignore_store_only_projection_without_authority() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let mut raw_projection = Session::new();
        raw_projection.push(Message::User(UserMessage::text(
            "store-only row must stay compatibility-only".to_string(),
        )));
        let id = raw_projection.id().clone();
        store
            .save(&raw_projection)
            .await
            .expect("test should seed a raw store projection");

        let read_err = service
            .read(&id)
            .await
            .expect_err("read must not promote a store-only projection");
        assert!(matches!(read_err, SessionError::NotFound { .. }));

        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");
        assert!(
            listed.iter().all(|summary| summary.session_id != id),
            "list() must not expose store-only projections as runtime authority"
        );
        assert!(
            runtime_store
                .load_session_snapshot(
                    &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id)
                )
                .await
                .expect("runtime snapshot load should succeed")
                .is_none(),
            "read/list must not create runtime authority from a store-only projection"
        );
    }

    #[tokio::test]
    async fn test_authoritative_load_ignores_newer_raw_store_projection_when_runtime_exists() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let runtime_authority = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should exist");

        let mut raw_store_projection = runtime_authority.clone();
        raw_store_projection.push(Message::User(UserMessage::text(
            "raw store projection must not become authority".to_string(),
        )));
        store
            .save(&raw_store_projection)
            .await
            .expect("test should be able to write a stale raw projection");

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session should exist");
        assert_eq!(
            authoritative.messages().len(),
            runtime_authority.messages().len(),
            "raw session-store rows must not override an existing runtime authority snapshot"
        );
    }

    #[tokio::test]
    async fn test_live_runtime_read_ignores_stale_raw_store_fallback_when_snapshot_load_misses() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let output = service
            .apply_runtime_turn(
                &result.session_id,
                RunId::new(),
                runtime_content_turn_request("live runtime truth"),
                RunApplyBoundary::Immediate,
                vec![],
            )
            .await
            .expect("machine-owned runtime turn should succeed");
        machine_commit_runtime_output(runtime_store.as_ref(), &result.session_id, &output).await;
        let live = service
            .export_live_session(&result.session_id)
            .await
            .expect("live session should export");

        let mut raw_store_projection = live.clone();
        raw_store_projection.push(Message::User(UserMessage::text(
            "stale raw compatibility row".to_string(),
        )));
        raw_store_projection.set_metadata(
            SESSION_LABELS_KEY,
            serde_json::json!({
                "source": "raw-store",
            }),
        );
        store
            .save(&raw_store_projection)
            .await
            .expect("test should seed a stale raw store projection");

        runtime_store.hide_next_session_snapshot_loads(1);
        let view = service
            .read(&result.session_id)
            .await
            .expect("read should preserve live runtime truth");
        assert_eq!(
            view.state.message_count,
            live.messages().len(),
            "read() must not expose a raw store row when live runtime truth exists"
        );
        assert!(
            view.state.labels.is_empty(),
            "read() must not expose raw store metadata when live runtime truth exists"
        );
        assert!(
            service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed"),
            "status must not let raw fallback evict a live runtime session"
        );

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should remain present");
        assert_eq!(
            authoritative.messages().len(),
            live.messages().len(),
            "raw fallback must not replace durable runtime authority"
        );
    }

    #[tokio::test]
    async fn test_live_runtime_list_status_and_resume_fail_closed_on_stale_raw_store_metadata() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let output = service
            .apply_runtime_turn(
                &result.session_id,
                RunId::new(),
                runtime_content_turn_request("live runtime truth"),
                RunApplyBoundary::Immediate,
                vec![],
            )
            .await
            .expect("machine-owned runtime turn should succeed");
        machine_commit_runtime_output(runtime_store.as_ref(), &result.session_id, &output).await;
        let live = service
            .export_live_session(&result.session_id)
            .await
            .expect("live session should export");

        let mut raw_store_projection = live.clone();
        raw_store_projection.push(Message::User(UserMessage::text(
            "stale raw compatibility row".to_string(),
        )));
        raw_store_projection.set_metadata(
            SESSION_LABELS_KEY,
            serde_json::json!({
                "source": "raw-store",
            }),
        );
        store
            .save(&raw_store_projection)
            .await
            .expect("test should seed a stale raw store projection");

        runtime_store.hide_next_session_snapshot_loads(1);
        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == result.session_id)
            .expect("list should include the live runtime-backed session");
        assert_eq!(
            summary.message_count,
            live.messages().len(),
            "list() must report live/runtime metadata instead of raw store fallback"
        );
        assert!(
            summary.labels.is_empty(),
            "list() must not expose labels from a stale raw store projection"
        );
        assert!(
            service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed"),
            "status must remain live after list inspects stale raw projections"
        );

        service
            .discard_live_session(&result.session_id)
            .await
            .expect("test should evict the live handle before resume");
        let restarted = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );
        let resume_source = restarted
            .load_authoritative_session(&result.session_id)
            .await
            .expect("resume source should load from runtime authority")
            .expect("runtime authority should remain present");
        assert_eq!(
            resume_source.messages().len(),
            live.messages().len(),
            "resume source must not be rebuilt from stale raw store fallback"
        );
        assert!(
            !resume_source.metadata().contains_key(SESSION_LABELS_KEY),
            "resume source must not inherit raw store metadata"
        );
        let resume_error = restarted
            .create_session(resume_request(resume_source))
            .await
            .expect_err(
                "resume must fail closed when projection refresh would shrink stale raw state",
            );
        assert!(
            matches!(resume_error, SessionError::Store(_)),
            "stale projection refresh failure should surface as a store error, got {resume_error:?}"
        );
        assert!(
            !restarted
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed after failed resume"),
            "failed projection refresh must discard the materialized live session"
        );
        let authoritative_after_failure = restarted
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should still succeed after failed resume")
            .expect("runtime authority should remain present after failed resume");
        assert_eq!(
            authoritative_after_failure.messages().len(),
            live.messages().len(),
            "failed projection refresh must not replace runtime-authoritative metadata"
        );
        let raw_after_failure = store
            .load(&result.session_id)
            .await
            .expect("raw projection load should still succeed")
            .expect("stale projection should remain present after failed resume");
        assert_eq!(
            raw_after_failure.messages().len(),
            live.messages().len() + 1,
            "failed projection refresh must not silently rewrite stale raw projection state"
        );
    }

    #[tokio::test]
    async fn test_authoritative_load_ignores_newer_raw_store_metadata_projection() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let runtime_authority = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should exist");

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let mut raw_store_projection = runtime_authority.clone();
        raw_store_projection.set_metadata(
            SESSION_LABELS_KEY,
            serde_json::json!({
                "source": "raw-store",
            }),
        );
        store
            .save(&raw_store_projection)
            .await
            .expect("test should be able to write a stale raw projection");
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("test should evict the live session so read/list use durable state");

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session should exist");
        assert_eq!(
            authoritative.updated_at(),
            runtime_authority.updated_at(),
            "newer raw metadata projections must not advance runtime-authoritative timestamps"
        );
        assert!(
            !authoritative.metadata().contains_key(SESSION_LABELS_KEY),
            "newer raw metadata projections must not be backfilled into runtime authority"
        );

        let view = service
            .read(&result.session_id)
            .await
            .expect("read should use runtime authority");
        assert!(
            view.state.labels.is_empty(),
            "read() must not expose labels from a newer raw store projection"
        );
        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should use runtime authority");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == result.session_id)
            .expect(
                "runtime-backed session should still be listed via its compatibility projection",
            );
        assert!(
            summary.labels.is_empty(),
            "list() must not expose labels from a newer raw store projection"
        );
        assert_eq!(
            summary.updated_at,
            runtime_authority.updated_at(),
            "list() must report the runtime-authoritative timestamp"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_uses_runtime_authority_after_runtime_turn() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        let output = service
            .apply_runtime_turn(
                &result.session_id,
                RunId::new(),
                runtime_content_turn_request("runtime committed turn"),
                RunApplyBoundary::Immediate,
                vec![],
            )
            .await
            .expect("runtime-backed turn should succeed");
        machine_commit_runtime_output(runtime_store.as_ref(), &result.session_id, &output).await;

        service
            .append_system_context(
                &result.session_id,
                AppendSystemContextRequest {
                    text: "prefer store".to_string(),
                    source: Some("api".to_string()),
                    idempotency_key: Some("store-base".to_string()),
                },
            )
            .await
            .expect("append_system_context should succeed");

        let stored = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("append should preserve the latest machine-owned runtime turn");
        assert_eq!(
            stored.messages().len(),
            2,
            "append must preserve runtime authority instead of using the stale session-store projection"
        );
        let state = stored
            .system_context_state()
            .expect("append should persist pending control state");
        assert_eq!(state.pending().len(), 1);
        assert_eq!(state.pending()[0].text, "prefer store");
    }

    #[tokio::test]
    async fn test_runtime_backed_stage_tool_results_without_live_handle_updates_runtime_authority()
    {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("test should be able to discard the live session");

        let staged = service
            .stage_tool_results(
                &result.session_id,
                StageToolResultsRequest {
                    results: vec![ToolResult::new(
                        "tool-call-1".to_string(),
                        "callback result".to_string(),
                        false,
                    )],
                },
            )
            .await
            .expect("stage_tool_results should update runtime authority");
        assert_eq!(staged.accepted_result_count, 1);

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session should exist");
        let deferred = authoritative
            .deferred_turn_state()
            .expect("runtime authority should carry deferred-turn state");
        assert_eq!(deferred.pending_tool_results().len(), 1);
        assert_eq!(deferred.pending_tool_results()[0].results.len(), 1);

        let raw = store
            .load(&result.session_id)
            .await
            .expect("raw store load should succeed")
            .expect("session-store projection should be kept for listability");
        let raw_deferred = raw
            .deferred_turn_state()
            .expect("projection should mirror committed deferred-turn state");
        assert_eq!(raw_deferred.pending_tool_results().len(), 1);

        let view = service
            .read(&result.session_id)
            .await
            .expect("read should use durable runtime truth after live discard");
        assert_eq!(view.state.message_count, authoritative.messages().len());
        assert!(
            !service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed"),
            "status should not recreate a live handle"
        );
        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == result.session_id)
            .expect("list should include persisted runtime-backed session");
        assert_eq!(summary.message_count, authoritative.messages().len());

        let restarted = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );
        let resume_source = restarted
            .load_authoritative_session(&result.session_id)
            .await
            .expect("resume source should load from runtime authority")
            .expect("runtime authority should remain present after restart");
        let resumed = restarted
            .create_session(resume_request(resume_source))
            .await
            .expect("resume should materialize the runtime-authoritative snapshot");
        assert_eq!(resumed.session_id, result.session_id);
        let resumed_live = restarted
            .export_live_session(&result.session_id)
            .await
            .expect("resumed live session should export");
        let resumed_deferred = resumed_live
            .deferred_turn_state()
            .expect("resumed session should preserve staged tool results");
        assert_eq!(resumed_deferred.pending_tool_results().len(), 1);
    }

    #[tokio::test]
    async fn test_store_only_control_mutations_fail_closed_without_runtime_divergence() {
        for runtime_backed in [true, false] {
            let mode = if runtime_backed {
                "runtime-backed"
            } else {
                "runtime-less"
            };
            let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
            let runtime_store = runtime_backed.then(|| Arc::new(InMemoryRuntimeStore::new()));
            let service_runtime_store = runtime_store
                .as_ref()
                .map(|store| Arc::clone(store) as Arc<dyn RuntimeStore>);
            let service = PersistentSessionService::new(
                DummyBuilder,
                4,
                Arc::clone(&store),
                service_runtime_store,
                memory_blob_store(),
            );
            let session = Session::new();
            let id = session.id().clone();
            store
                .save(&session)
                .await
                .expect("test should seed a store-only compatibility projection");

            let append_err = service
                .append_system_context(
                    &id,
                    AppendSystemContextRequest {
                        text: format!("{mode} store-only append must fail closed"),
                        source: Some("api".to_string()),
                        idempotency_key: Some(format!("store-only-{mode}-append")),
                    },
                )
                .await
                .expect_err("store-only projection must not be promoted by append");
            assert!(
                matches!(
                    append_err,
                    SessionControlError::Session(SessionError::Unsupported(ref message))
                        if message.contains("store-only compatibility projection")
                            && message.contains("machine snapshot")
                ),
                "{mode} append error should require machine authority: {append_err:?}"
            );

            let stage_err = service
                .stage_tool_results(
                    &id,
                    StageToolResultsRequest {
                        results: vec![ToolResult::new(
                            "tool-call-1".to_string(),
                            "callback result".to_string(),
                            false,
                        )],
                    },
                )
                .await
                .expect_err("store-only projection must not be promoted by staged tool results");
            assert!(
                matches!(
                    stage_err,
                    SessionError::Unsupported(ref message)
                        if message.contains("store-only compatibility projection")
                            && message.contains("machine snapshot")
                ),
                "{mode} stage error should require machine authority: {stage_err:?}"
            );

            let archive_result = service.archive(&id).await;
            if runtime_backed {
                let archive_err =
                    archive_result.expect_err("runtime-backed archive must route through machine");
                assert!(
                    matches!(
                        archive_err,
                        SessionError::Unsupported(ref message)
                            if message.contains("MachineSessionArchiveProtocol")
                    ),
                    "{mode} archive error should require machine authority: {archive_err:?}"
                );
            } else {
                archive_result.expect("runtime-less compatibility archive should remain supported");
            }

            let raw = store
                .load(&id)
                .await
                .expect("raw store load should succeed")
                .expect("store-only projection should remain present");
            assert!(
                raw.system_context_state().is_none(),
                "{mode} append rejection must not mutate the store-only projection"
            );
            assert!(
                raw.deferred_turn_state().is_none(),
                "{mode} stage rejection must not mutate the store-only projection"
            );
            assert_eq!(
                metadata_marks_archived(raw.metadata()),
                !runtime_backed,
                "{mode} archive lifecycle metadata should reflect runtime-less compatibility support"
            );
            if let Some(runtime_store) = runtime_store {
                assert!(
                    runtime_store
                        .load_session_snapshot(
                            &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id)
                        )
                        .await
                        .expect("runtime snapshot load should succeed")
                        .is_none(),
                    "store-only control mutations must not create runtime authority"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_machine_authorized_archive_rejects_store_only_projection() {
        for runtime_backed in [true, false] {
            let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
            let runtime_store = runtime_backed.then(|| Arc::new(InMemoryRuntimeStore::new()));
            let service_runtime_store = runtime_store
                .as_ref()
                .map(|store| Arc::clone(store) as Arc<dyn RuntimeStore>);
            let service = PersistentSessionService::new(
                DummyBuilder,
                4,
                Arc::clone(&store),
                service_runtime_store,
                memory_blob_store(),
            );
            let session = Session::new();
            let id = session.id().clone();
            store
                .save(&session)
                .await
                .expect("test should seed a store-only compatibility projection");

            let machine = if let Some(runtime_store) = runtime_store.as_ref() {
                meerkat_runtime::MeerkatMachine::persistent(
                    Arc::clone(runtime_store) as Arc<dyn RuntimeStore>,
                    memory_blob_store(),
                )
            } else {
                meerkat_runtime::MeerkatMachine::ephemeral()
            };
            let err = service
                .archive_with_machine_protocol(
                    &id,
                    MachineSessionArchiveProtocol::from_machine(&machine),
                )
                .await
                .expect_err("machine-routed archive must not promote store-only projection truth");
            assert!(
                matches!(err, SessionError::Unsupported(ref message)
                    if message.contains("store-only compatibility projection")
                        && message.contains("machine snapshot")),
                "unexpected store-only archive error: {err:?}"
            );

            let raw = store
                .load(&id)
                .await
                .expect("raw store load should succeed")
                .expect("store-only projection should remain present");
            assert!(
                !metadata_marks_archived(raw.metadata()),
                "machine-routed archive rejection must not persist archived lifecycle metadata"
            );
            assert!(
                raw.system_context_state().is_none(),
                "archive must not add control append state"
            );
            assert!(
                raw.deferred_turn_state().is_none(),
                "archive must not add deferred-turn control state"
            );
            if let Some(runtime_store) = runtime_store {
                assert!(
                    runtime_store
                        .load_session_snapshot(
                            &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id),
                        )
                        .await
                        .expect("runtime snapshot load should succeed")
                        .is_none(),
                    "store-only machine archive rejection must not create runtime authority"
                );
                assert_eq!(
                    meerkat_runtime::store::load_runtime_state(
                        runtime_store.as_ref(),
                        &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id),
                    )
                    .await
                    .expect("runtime state load should succeed"),
                    None,
                    "store-only machine archive rejection must not create retired lifecycle"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_apply_runtime_context_appends_rejects_missing_runtime_bindings() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let mut session = Session::new();
        let id = session.id().clone();
        session
            .set_session_metadata(meerkat_core::SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "test-model".to_string(),
                max_tokens: 1024,
                structured_output_retries: 2,
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: Some("realm-test".to_string()),
                instance_id: Some("instance-test".to_string()),
                backend: Some("sqlite".to_string()),
                config_generation: Some(7),
                auth_binding: None,
            })
            .expect("session metadata should serialize");
        store
            .save(&session)
            .await
            .expect("persisted session should save");

        let error = service
            .apply_runtime_context_appends(
                &id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "recover me".to_string(),
                    source: Some("system-generated:test".to_string()),
                    idempotency_key: None,
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await
            .expect_err("runtime-backed recovery should reject missing bindings");

        assert!(
            error.to_string().contains("session not found"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn test_start_turn_recovery_rejects_missing_runtime_bindings() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create deferred session");
        let id = created.session_id;

        service
            .discard_live_session(&id)
            .await
            .expect("discard live session");

        let error = service
            .start_turn(&id, start_turn_request("follow up"))
            .await
            .expect_err("runtime-backed recovery should reject missing bindings");

        assert!(
            error
                .to_string()
                .contains("stored-session recovery via non-canonical runtime-binding providers has been deleted"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn test_metadata_only_projection_does_not_discard_live_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::Defer))
            .await
            .expect("create deferred session");
        let id = created.session_id;

        let mut projected = service
            .export_live_session(&id)
            .await
            .expect("live session should export");
        projected.set_metadata("projection_only", serde_json::json!(true));
        store
            .save(&projected)
            .await
            .expect("projection snapshot should save");

        let discarded = service
            .discard_stale_live_session_if_needed(&id)
            .await
            .expect("discard check should succeed");

        assert!(
            !discarded,
            "metadata/timestamp-only durable projection must not evict live runtime mechanics"
        );
        service
            .export_live_session(&id)
            .await
            .expect("live session should remain available");
    }

    #[tokio::test]
    async fn test_apply_runtime_context_appends_emits_run_lifecycle_events() {
        use futures::StreamExt;
        use meerkat_core::event::AgentEvent;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let run = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create session");
        let session_id = run.session_id;
        let mut events = service
            .subscribe_session_events(&session_id)
            .await
            .expect("subscribe_session_events");
        let baseline_summary = service
            .read(&session_id)
            .await
            .expect("read baseline summary");

        service
            .apply_runtime_context_appends(
                &session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "Peer terminal response from analyst-rt\nRequest ID: req-123\nStatus: completed\nPayload: {\"request_intent\":\"checksum_token\",\"request_subject\":\"alpha beta gamma\",\"token\":\"birch seventeen\"}".to_string(),
                    source: Some("peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-123".to_string()),
                    idempotency_key: Some("peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-123".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await
            .expect("apply_runtime_context_appends");

        let post_context_summary = service
            .read(&session_id)
            .await
            .expect("read post-context summary");
        assert!(
            post_context_summary.state.message_count > baseline_summary.state.message_count,
            "committed runtime context must advance the live summary/projection watcher"
        );

        let started = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
            .await
            .expect("run_started timeout")
            .expect("run_started event should exist");
        match started.payload {
            AgentEvent::RunStarted { prompt, .. } => {
                let normalized = prompt.text_content().to_lowercase();
                assert!(
                    normalized.contains(
                        "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-123"
                    ),
                    "run_started prompt should expose runtime system-context source: {normalized}"
                );
                assert!(
                    normalized.contains("birch seventeen"),
                    "run_started prompt should expose authoritative terminal peer payload: {normalized}"
                );
            }
            other => panic!("expected run_started, got {other:?}"),
        }

        let completed = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
            .await
            .expect("run_completed timeout")
            .expect("run_completed event should exist");
        match completed.payload {
            AgentEvent::RunCompleted { result, usage, .. } => {
                assert!(
                    result.is_empty(),
                    "context-only runtime apply should not synthesize assistant output: {result:?}"
                );
                assert_eq!(
                    usage,
                    meerkat_core::types::Usage::default(),
                    "context-only runtime apply should not report model usage"
                );
            }
            other => panic!("expected run_completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_no_pending_runtime_turn_preserves_pre_turn_context_events() {
        use futures::StreamExt;
        use meerkat_core::event::AgentEvent;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let run = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create session");
        let session_id = run.session_id;
        let mut events = service
            .subscribe_session_events(&session_id)
            .await
            .expect("subscribe_session_events");
        let mut req = start_turn_request("resume");
        req.runtime.turn_metadata = Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending),
                ..Default::default()
            },
        );
        req.runtime.pre_turn_context_appends = vec![PendingSystemContextAppend {
            text: "Peer terminal response\nPayload: {\"token\":\"birch seventeen\"}".to_string(),
            source: Some(
                "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-123".to_string(),
            ),
            idempotency_key: Some(
                "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-123".to_string(),
            ),
            accepted_at: meerkat_core::time_compat::SystemTime::now(),
        }];

        let output = service
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                req,
                RunApplyBoundary::RunStart,
                vec![InputId::new()],
            )
            .await
            .expect("runtime no-pending apply should commit context");
        assert!(matches!(
            output.terminal,
            Some(CoreApplyTerminal::NoPendingBoundary)
        ));
        let snapshot = output
            .session_snapshot
            .as_ref()
            .expect("no-pending output should include committed session snapshot");
        let session: Session =
            serde_json::from_slice(snapshot).expect("deserialize no-pending session snapshot");
        let context = session.system_context_state().unwrap_or_default();
        assert!(
            context
                .applied()
                .iter()
                .any(|append| append.text.contains("birch seventeen")),
            "no-pending terminal snapshot should preserve pre-turn context: {context:?}"
        );

        let started = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
            .await
            .expect("run_started timeout")
            .expect("run_started event should exist");
        match started.payload {
            AgentEvent::RunStarted { prompt, .. } => {
                let normalized = prompt.text_content().to_lowercase();
                assert!(
                    normalized.contains("birch seventeen"),
                    "run_started prompt should expose no-pending runtime context: {normalized}"
                );
            }
            other => panic!("expected run_started, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_event_store_projection_records_persistent_session_events() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let run = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create session");
        let session_id = run.session_id;

        service
            .apply_runtime_context_appends(
                &session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "project this durable event".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("test".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await
            .expect("apply context append");

        event_store.wait_for_seq(&session_id, 2).await;
        assert_eq!(event_store.last_seq(&session_id).await.unwrap(), 2);
        let events_path = dir
            .path()
            .join(".rkat")
            .join("sessions")
            .join(session_id.to_string())
            .join("events.jsonl");
        let projected = read_projected_events_after(&events_path, "run_completed").await;
        assert!(projected.contains("run_started"));
        assert!(projected.contains("run_completed"));
    }

    #[tokio::test]
    async fn test_event_replay_projection_reads_ordered_session_events() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );
        let session_id = SessionId::new();

        event_store
            .append(
                &session_id,
                &[
                    AgentEvent::TurnStarted { turn_number: 0 },
                    AgentEvent::TextComplete {
                        content: "two".to_string(),
                    },
                ],
            )
            .await
            .expect("append event fixtures");

        assert_eq!(
            service
                .event_log_latest_seq(&session_id)
                .await
                .expect("latest seq"),
            Some(2)
        );
        let events = service
            .event_log_read_from(&session_id, 2)
            .await
            .expect("read event log")
            .expect("event projection enabled");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 2);
        assert!(matches!(events[0].event, AgentEvent::TextComplete { .. }));
    }

    #[tokio::test]
    async fn test_event_replay_projection_reports_unsupported_when_not_installed() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let session_id = SessionId::new();

        assert_eq!(
            service
                .event_log_latest_seq(&session_id)
                .await
                .expect("latest seq"),
            None
        );
        assert!(
            service
                .event_log_read_from(&session_id, 1)
                .await
                .expect("read event log")
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_event_store_projection_records_eager_initial_turn_events() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            EventfulBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let run = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create session");
        let session_id = run.session_id;

        event_store.wait_for_seq(&session_id, 2).await;
        assert_eq!(event_store.last_seq(&session_id).await.unwrap(), 2);
        let events_path = dir
            .path()
            .join(".rkat")
            .join("sessions")
            .join(session_id.to_string())
            .join("events.jsonl");
        let projected = read_projected_events_after(&events_path, "run_completed").await;
        assert!(projected.contains("run_started"));
        assert!(projected.contains("run_completed"));
    }

    /// Create a session request that seeds initial SessionMetadata so
    /// update_keep_alive has something to mutate (mirrors what the real factory does).
    fn create_request_with_metadata(
        prompt: &str,
        initial_turn: InitialTurnPolicy,
    ) -> CreateSessionRequest {
        let mut session = Session::new();
        let metadata = meerkat_core::SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "test".to_string(),
            max_tokens: 1024,
            structured_output_retries: 2,
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: meerkat_core::SessionTooling::default(),
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
        };
        session.set_session_metadata(metadata).unwrap();
        let mut req = create_request(prompt, initial_turn);
        req.build = Some(SessionBuildOptions {
            resume_session: Some(session),
            ..Default::default()
        });
        req
    }

    #[tokio::test]
    async fn test_update_keep_alive_persists_to_store() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create session");
        let id = created.session_id;

        // Verify initial keep_alive is false (default).
        let persisted = store.load(&id).await.unwrap().unwrap();
        let meta = persisted.session_metadata().expect("metadata present");
        assert!(!meta.keep_alive, "initial keep_alive should be false");

        // Update keep_alive to true on the live session.
        service
            .apply_runtime_session_keep_alive(&id, true)
            .await
            .expect("runtime keep_alive update should succeed");

        // Verify the store reflects the update.
        let persisted = store.load(&id).await.unwrap().unwrap();
        let meta = persisted.session_metadata().expect("metadata present");
        assert!(
            meta.keep_alive,
            "persisted keep_alive should be true after update"
        );
    }

    #[tokio::test]
    async fn test_update_keep_alive_rolls_back_on_store_failure() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store: Arc<dyn SessionStore> = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let service =
            PersistentSessionService::new(DummyBuilder, 4, store, None, memory_blob_store());

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create session");
        let id = created.session_id;

        // --- Transition: false → true with store failure ---
        fail_store.set_fail_save(true);
        let result = service.apply_runtime_session_keep_alive(&id, true).await;
        assert!(result.is_err(), "false→true should fail when store fails");
        fail_store.set_fail_save(false);
        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            !exported.session_metadata().unwrap().keep_alive,
            "should roll back to false after failed false→true"
        );

        // --- Bring live state to true successfully ---
        service
            .apply_runtime_session_keep_alive(&id, true)
            .await
            .unwrap();
        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(exported.session_metadata().unwrap().keep_alive);

        // --- Transition: true → true with store failure (idempotent retry) ---
        fail_store.set_fail_save(true);
        let result = service.apply_runtime_session_keep_alive(&id, true).await;
        assert!(result.is_err(), "true→true should fail when store fails");
        fail_store.set_fail_save(false);
        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            exported.session_metadata().unwrap().keep_alive,
            "should stay true after failed true→true (not flip to false)"
        );

        // --- Transition: true → false with store failure ---
        fail_store.set_fail_save(true);
        let result = service.apply_runtime_session_keep_alive(&id, false).await;
        assert!(result.is_err(), "true→false should fail when store fails");
        fail_store.set_fail_save(false);
        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            exported.session_metadata().unwrap().keep_alive,
            "should roll back to true after failed true→false (not flip to false)"
        );
    }

    #[tokio::test]
    async fn test_set_session_tool_filter_does_not_forge_visibility_state() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create session");
        let id = created.session_id;

        let filter =
            meerkat_core::ToolFilter::Deny(["view_image".to_string()].into_iter().collect());

        service
            .set_session_tool_filter(&id, filter.clone())
            .await
            .expect("set_session_tool_filter should succeed");

        let exported = service.export_session_with_labels(&id).await.unwrap();
        let exported_state = exported
            .tool_visibility_state()
            .expect("live visibility state should decode");
        assert!(
            exported_state.is_none(),
            "store-only filter update must not forge generated visibility authority"
        );

        let persisted = store.load(&id).await.unwrap().unwrap();
        let persisted_state = persisted
            .tool_visibility_state()
            .expect("persisted visibility state should decode");
        assert_eq!(persisted_state, exported_state);
    }

    #[tokio::test]
    async fn test_set_session_tool_filter_rolls_back_on_store_failure() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store: Arc<dyn SessionStore> = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let service =
            PersistentSessionService::new(DummyBuilder, 4, store, None, memory_blob_store());

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create session");
        let id = created.session_id;

        let baseline = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            baseline
                .tool_visibility_state()
                .expect("baseline visibility state should decode")
                .is_none(),
            "new sessions should not materialize visibility metadata before updates"
        );

        let filter =
            meerkat_core::ToolFilter::Deny(["view_image".to_string()].into_iter().collect());

        fail_store.set_fail_save(true);
        let result = service.set_session_tool_filter(&id, filter).await;
        assert!(
            result.is_err(),
            "store failure should abort the filter update"
        );
        fail_store.set_fail_save(false);

        let exported = service.export_session_with_labels(&id).await.unwrap();
        let exported_visibility_state = exported
            .tool_visibility_state()
            .expect("exported visibility state should decode");
        let baseline_visibility_state = baseline
            .tool_visibility_state()
            .expect("baseline visibility state should decode");
        assert_eq!(
            exported_visibility_state, baseline_visibility_state,
            "live session should roll back to the pre-mutation visibility state"
        );

        let persisted = fail_store.inner.load(&id).await.unwrap().unwrap();
        let persisted_visibility_state = persisted
            .tool_visibility_state()
            .expect("persisted visibility state should decode");
        assert_eq!(
            persisted_visibility_state, baseline_visibility_state,
            "store should retain the pre-mutation visibility state after rollback"
        );
    }

    #[test]
    fn rollback_snapshot_ignores_legacy_filter_metadata_when_canonical_state_is_absent() {
        let mut session = Session::new();
        session.set_metadata(
            meerkat_core::EXTERNAL_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(meerkat_core::ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .unwrap(),
        );
        session.set_metadata(
            meerkat_core::tool_scope::INHERITED_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(meerkat_core::ToolFilter::Allow(
                ["visible".to_string()].into_iter().collect(),
            ))
            .unwrap(),
        );

        let snapshot = rollback_tool_visibility_state_snapshot(&session)
            .expect("legacy-only metadata should not fail canonical visibility parsing");

        assert_eq!(
            snapshot, None,
            "legacy-only metadata must not become rollback visibility authority"
        );
    }

    #[tokio::test]
    async fn test_set_session_tool_filter_rollback_does_not_promote_legacy_metadata() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store: Arc<dyn SessionStore> = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let service =
            PersistentSessionService::new(DummyBuilder, 4, store, None, memory_blob_store());

        let mut request = create_request_with_metadata("hello", InitialTurnPolicy::RunImmediately);
        let session = request
            .build
            .as_mut()
            .and_then(|build| build.resume_session.as_mut())
            .expect("request should carry a resumable session");
        session.set_metadata(
            meerkat_core::EXTERNAL_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(meerkat_core::ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .unwrap(),
        );
        session.set_metadata(
            meerkat_core::tool_scope::INHERITED_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(meerkat_core::ToolFilter::Allow(
                ["visible".to_string()].into_iter().collect(),
            ))
            .unwrap(),
        );

        let created = service
            .create_session(request)
            .await
            .expect("create session");
        let id = created.session_id;

        let baseline = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            baseline
                .tool_visibility_state()
                .expect("canonical visibility metadata should parse")
                .is_none(),
            "legacy-only metadata must not materialize canonical visibility state"
        );
        assert!(
            baseline
                .metadata()
                .contains_key(meerkat_core::EXTERNAL_TOOL_FILTER_METADATA_KEY),
            "fixture should retain the stale external filter metadata key"
        );
        assert!(
            baseline
                .metadata()
                .contains_key(meerkat_core::tool_scope::INHERITED_TOOL_FILTER_METADATA_KEY),
            "fixture should retain the stale inherited filter metadata key"
        );

        let filter =
            meerkat_core::ToolFilter::Deny(["view_image".to_string()].into_iter().collect());

        fail_store.set_fail_save(true);
        let result = service.set_session_tool_filter(&id, filter).await;
        assert!(
            result.is_err(),
            "store failure should abort the filter update"
        );
        fail_store.set_fail_save(false);

        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            exported
                .tool_visibility_state()
                .expect("canonical visibility metadata should parse")
                .is_none(),
            "failed rollback must not promote stale legacy metadata into canonical visibility"
        );

        let persisted = fail_store.inner.load(&id).await.unwrap().unwrap();
        assert!(
            persisted
                .tool_visibility_state()
                .expect("canonical visibility metadata should parse")
                .is_none(),
            "store should retain no canonical visibility state after failed rollback"
        );
    }

    #[tokio::test]
    async fn test_set_session_tool_filter_rollback_rejects_malformed_canonical_visibility_state() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store: Arc<dyn SessionStore> = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let service =
            PersistentSessionService::new(DummyBuilder, 4, store, None, memory_blob_store());

        let mut request = create_request_with_metadata("hello", InitialTurnPolicy::RunImmediately);
        let malformed_visibility_state = serde_json::json!("not-a-visibility-state");
        let session = request
            .build
            .as_mut()
            .and_then(|build| build.resume_session.as_mut())
            .expect("request should carry a resumable session");
        let mut raw_session = serde_json::to_value(&*session).expect("session should serialize");
        raw_session
            .get_mut("metadata")
            .and_then(serde_json::Value::as_object_mut)
            .expect("session JSON should carry metadata")
            .insert(
                meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY.to_string(),
                malformed_visibility_state.clone(),
            );
        *session = serde_json::from_value(raw_session).expect("session should deserialize");

        let created = service
            .create_session(request)
            .await
            .expect("create session");
        let id = created.session_id;

        let baseline = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            baseline.try_tool_visibility_state().is_err(),
            "fixture should carry malformed canonical visibility metadata"
        );
        assert_eq!(
            baseline
                .metadata()
                .get(meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY),
            Some(&malformed_visibility_state),
            "fixture should retain the raw malformed canonical metadata"
        );

        let filter =
            meerkat_core::ToolFilter::Deny(["view_image".to_string()].into_iter().collect());

        fail_store.set_fail_save(true);
        let result = service.set_session_tool_filter(&id, filter).await;
        let err = result
            .expect_err("malformed canonical visibility should fail before staging or rollback");
        assert!(
            err.to_string()
                .contains("invalid canonical tool visibility state"),
            "unexpected error: {err}"
        );
        fail_store.set_fail_save(false);

        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert_eq!(
            exported
                .metadata()
                .get(meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY),
            Some(&malformed_visibility_state),
            "failed mutation must preserve malformed canonical visibility metadata"
        );
        assert!(
            exported.try_tool_visibility_state().is_err(),
            "failed mutation must not replace malformed canonical visibility with default state"
        );

        let persisted = fail_store.inner.load(&id).await.unwrap().unwrap();
        assert_eq!(
            persisted
                .metadata()
                .get(meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY),
            Some(&malformed_visibility_state),
            "store should retain the raw malformed canonical metadata after failed mutation"
        );
    }

    #[tokio::test]
    async fn test_deferred_first_turn_system_prompt_is_applied_and_persisted() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create deferred session");
        let id = created.session_id;

        service
            .start_turn(
                &id,
                start_turn_request_with_system_prompt(
                    "first turn",
                    Some("You are a deferred-session reviewer."),
                ),
            )
            .await
            .expect("deferred first turn should succeed");

        let restored = service
            .export_live_session(&id)
            .await
            .expect("live session");
        let system_prompt = restored
            .messages()
            .first()
            .and_then(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("restored session should contain a system prompt");
        assert!(system_prompt.contains("deferred-session reviewer"));

        let persisted = store
            .load(&id)
            .await
            .expect("load should succeed")
            .expect("session should be persisted");
        let persisted_prompt = persisted
            .messages()
            .first()
            .and_then(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("persisted session should contain a system prompt");
        assert!(persisted_prompt.contains("deferred-session reviewer"));
    }

    #[tokio::test]
    async fn test_deferred_first_turn_system_prompt_overrides_create_time_prompt() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let mut req = create_request_with_metadata("hello", InitialTurnPolicy::Defer);
        req.system_prompt = Some("You are the old prompt.".to_string());
        let created = service
            .create_session(req)
            .await
            .expect("create deferred session");
        let id = created.session_id;

        service
            .start_turn(
                &id,
                start_turn_request_with_system_prompt(
                    "first turn",
                    Some("You are the new prompt."),
                ),
            )
            .await
            .expect("deferred first turn should succeed");

        let restored = service
            .export_live_session(&id)
            .await
            .expect("live session");
        let system_prompt = restored
            .messages()
            .first()
            .and_then(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("restored session should contain a system prompt");
        assert!(system_prompt.contains("new prompt"));
        assert!(!system_prompt.contains("old prompt"));
    }

    #[tokio::test]
    async fn test_materialized_start_turn_rejects_system_prompt_override() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create immediate session");
        let id = created.session_id;

        let error = service
            .start_turn(
                &id,
                start_turn_request_with_system_prompt(
                    "follow-up",
                    Some("You are a different prompt."),
                ),
            )
            .await
            .expect_err("materialized session should reject turn-time system_prompt");

        match error {
            SessionError::Unsupported(message) => {
                assert!(message.contains("deferred session's first turn"));
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_dispatch_external_tool_call_does_not_forge_tool_visibility_state() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            ToolDispatchBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::Defer))
            .await
            .expect("create session");
        let id = created.session_id;

        let outcome = service
            .dispatch_external_tool_call(
                &id,
                ToolCall::new(
                    "call-1".to_string(),
                    "callback_probe_tool".to_string(),
                    serde_json::json!({}),
                ),
            )
            .await
            .expect("dispatch external tool call");

        assert_eq!(outcome.result.text_content(), "handled callback_probe_tool");

        let persisted = store
            .load(&id)
            .await
            .expect("load persisted session")
            .expect("persisted session should exist");
        let visibility_state = persisted
            .tool_visibility_state()
            .expect("persisted visibility state should decode");
        assert!(
            visibility_state.is_none(),
            "external dispatch must not forge generated tool-visibility authority: {visibility_state:?}"
        );
    }

    #[tokio::test]
    async fn test_dispatch_external_tool_call_discards_live_session_when_persist_fails() {
        let store = Arc::new(FailSaveStore::new());
        let service = PersistentSessionService::new(
            ToolDispatchBuilder,
            4,
            store.clone(),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::Defer))
            .await
            .expect("create session");
        let id = created.session_id;

        store.set_fail_save(true);
        let error = service
            .dispatch_external_tool_call(
                &id,
                ToolCall::new(
                    "call-2".to_string(),
                    "callback_probe_tool".to_string(),
                    serde_json::json!({}),
                ),
            )
            .await
            .expect_err("persist failure should bubble out");

        assert!(
            matches!(error, SessionError::Store(_)),
            "expected store error after persistence failure, got {error:?}"
        );
        let live = service.export_live_session(&id).await;
        assert!(
            matches!(live, Err(SessionError::NotFound { .. })),
            "expected live session to be discarded after persist failure, got {live:?}"
        );
    }
}
