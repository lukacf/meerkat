//! In-memory session store (for testing)

use crate::{SessionFilter, SessionStore, SessionStoreError};
use async_trait::async_trait;
use meerkat_core::session_store::{
    IncrementalSessionStore, SessionHead, SessionHeadCas, head_canonical_plain_save_guard,
    reconstruct_rewrite_record, session_head_cas_token, strand_layout_for_history,
    validate_commit_rewrite_transition, validate_save_head_transition,
};
use meerkat_core::transcript_messages_digest;
use meerkat_core::types::Message;
use meerkat_core::{
    Session, SessionId, SessionMeta, TranscriptRewriteCommit, TranscriptRewriteRecord,
    TranscriptStrandId,
};
use std::collections::HashMap;
use std::sync::Arc;

// Use crate-level tokio alias for consistency with other crates.
#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use ::tokio::sync::RwLock;

/// Store-call counters the incremental integration tests assert on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryStoreStats {
    /// Compat whole-blob method calls (`save`, `save_transcript_rewrite`,
    /// `save_authoritative_projection*`).
    pub whole_blob_saves: u64,
    /// New strand rows written via `append_messages` (delta rows only;
    /// idempotent re-appends do not count).
    pub appended_message_rows: u64,
    /// `save_head` calls.
    pub head_saves: u64,
    /// `commit_rewrite` calls.
    pub rewrite_commits: u64,
    /// Compat `load` calls.
    pub full_loads: u64,
    /// `load_head` calls.
    pub head_loads: u64,
}

struct StoredRewriteRow {
    commit: TranscriptRewriteCommit,
    parent_strand: TranscriptStrandId,
    parent_len: u64,
    strand: TranscriptStrandId,
    strand_len: u64,
}

#[derive(Default)]
struct MemoryStoreState {
    /// Legacy whole-blob rows. Once a head exists for a session, its blob
    /// entry (if any) is a frozen migration archive, never read again.
    sessions: HashMap<SessionId, Session>,
    heads: HashMap<SessionId, (SessionHead, String)>,
    strands: HashMap<SessionId, HashMap<TranscriptStrandId, Vec<Vec<u8>>>>,
    rewrites: HashMap<SessionId, Vec<StoredRewriteRow>>,
    stats: MemoryStoreStats,
}

impl MemoryStoreState {
    fn strand_rows(&self, id: &SessionId, strand: &TranscriptStrandId) -> &[Vec<u8>] {
        self.strands
            .get(id)
            .and_then(|strands| strands.get(strand))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn strand_messages(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        range: std::ops::Range<u64>,
    ) -> Result<Vec<Message>, SessionStoreError> {
        let rows = self.strand_rows(id, strand);
        let start =
            usize::try_from(range.start).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let end =
            usize::try_from(range.end).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        if start > end || end > rows.len() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        rows[start..end]
            .iter()
            .map(|bytes| {
                serde_json::from_slice::<Message>(bytes)
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))
            })
            .collect()
    }

    /// Trait-contract append: contiguity, immutable overlap, idempotency.
    /// Returns the number of NEW rows written.
    fn append_rows(
        &mut self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        base_seq: u64,
        messages: &[Message],
    ) -> Result<u64, SessionStoreError> {
        let serialized: Vec<Vec<u8>> = messages
            .iter()
            .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
            .collect::<Result<_, _>>()?;
        let rows = self
            .strands
            .entry(id.clone())
            .or_default()
            .entry(strand.clone())
            .or_default();
        let existing = rows.len() as u64;
        if base_seq > existing {
            return Err(SessionStoreError::TranscriptContinuityViolation {
                id: id.clone(),
                previous_revision: format!("strand-rows:{existing}"),
                incoming_revision: format!("append-base-seq:{base_seq}"),
                reason: format!(
                    "append at base_seq {base_seq} would leave a gap in strand {strand} with {existing} rows"
                ),
            });
        }
        for (offset, bytes) in serialized.iter().enumerate() {
            let seq = base_seq + offset as u64;
            if seq < existing {
                let stored = &rows
                    [usize::try_from(seq).map_err(|_| SessionStoreError::Corrupted(id.clone()))?];
                if stored != bytes {
                    return Err(SessionStoreError::TranscriptContinuityViolation {
                        id: id.clone(),
                        previous_revision: format!("strand:{strand} seq:{seq}"),
                        incoming_revision: "divergent-bytes".to_string(),
                        reason: format!(
                            "append would overwrite immutable row (strand {strand}, seq {seq}) with different bytes"
                        ),
                    });
                }
            }
        }
        let mut appended = 0u64;
        for (offset, bytes) in serialized.into_iter().enumerate() {
            let seq = base_seq + offset as u64;
            if seq >= existing {
                rows.push(bytes);
                appended += 1;
            }
        }
        self.stats.appended_message_rows += appended;
        Ok(appended)
    }

    /// Head row if present; otherwise migrate a legacy blob (first
    /// incremental WRITE migrates; reads synthesize without writing).
    fn ensure_head_canonical_for_write(
        &mut self,
        id: &SessionId,
    ) -> Result<Option<(SessionHead, String)>, SessionStoreError> {
        if let Some(existing) = self.heads.get(id) {
            return Ok(Some(existing.clone()));
        }
        let Some(session) = self.sessions.get(id).cloned() else {
            return Ok(None);
        };
        let (layout, head) = layout_for_blob_session(&session)?;
        for (strand, rows) in &layout.strands {
            self.append_rows(id, strand, 0, rows)?;
        }
        let rewrites = self.rewrites.entry(id.clone()).or_default();
        for rewrite in layout.rewrites {
            rewrites.push(StoredRewriteRow {
                commit: rewrite.commit,
                parent_strand: rewrite.parent_strand,
                parent_len: rewrite.parent_len,
                strand: rewrite.strand,
                strand_len: rewrite.strand_len,
            });
        }
        let token = session_head_cas_token(&head)?;
        self.heads.insert(id.clone(), (head.clone(), token.clone()));
        Ok(Some((head, token)))
    }

    fn write_head(&mut self, head: &SessionHead) -> Result<(), SessionStoreError> {
        let token = session_head_cas_token(head)?;
        self.heads.insert(head.id.clone(), (head.clone(), token));
        Ok(())
    }

    fn materialize_slim(&self, head: &SessionHead) -> Result<Session, SessionStoreError> {
        let messages = self.strand_messages(&head.id, &head.strand, 0..head.message_count)?;
        head.clone().into_session(messages)
    }

    fn adopted_commits(&self, id: &SessionId, rewrite_count: u64) -> Vec<TranscriptRewriteCommit> {
        self.rewrites
            .get(id)
            .map(|rows| {
                rows.iter()
                    .take(usize::try_from(rewrite_count).unwrap_or(usize::MAX))
                    .map(|row| row.commit.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Head-canonical compat write: delta-append when the incoming
    /// transcript extends the persisted head strand, otherwise a `rebase:`
    /// strand switch.
    fn write_head_canonical_session(
        &mut self,
        session: &Session,
        head: &SessionHead,
    ) -> Result<(), SessionStoreError> {
        let id = session.id();
        let live = session.messages();
        let prev_count = usize::try_from(head.message_count)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let plain_append = live.len() >= prev_count
            && transcript_messages_digest(&live[..prev_count]).map_err(SessionStoreError::from)?
                == head.head_revision;
        let strand = if plain_append {
            if live.len() > prev_count {
                self.append_rows(id, &head.strand, head.message_count, &live[prev_count..])?;
            }
            head.strand.clone()
        } else {
            let live_digest = transcript_messages_digest(live).map_err(SessionStoreError::from)?;
            let rebased = TranscriptStrandId::rebase(&live_digest);
            self.append_rows(id, &rebased, 0, live)?;
            rebased
        };
        let new_head = SessionHead::from_session(session, strand, head.rewrite_count)?;
        self.write_head(&new_head)
    }

    fn commit_rewrite(
        &mut self,
        id: &SessionId,
        record: &TranscriptRewriteRecord,
        expected: &SessionHeadCas,
        stored: &(SessionHead, String),
    ) -> Result<SessionHead, SessionStoreError> {
        let (stored_head, stored_token) = stored;
        // CAS races and stale parents must surface as
        // TranscriptRevisionConflict BEFORE the parent strand range read (the
        // advanced head strand can be shorter than the stale commit's
        // messages_before).
        match expected {
            SessionHeadCas::Create => {
                return Err(SessionStoreError::TranscriptRevisionConflict {
                    id: id.clone(),
                    expected: "<create>".to_string(),
                    actual: stored_token.clone(),
                });
            }
            SessionHeadCas::IfToken(expected_token) => {
                if expected_token != stored_token {
                    return Err(SessionStoreError::TranscriptRevisionConflict {
                        id: id.clone(),
                        expected: expected_token.clone(),
                        actual: stored_token.clone(),
                    });
                }
            }
        }
        if record.commit.parent_revision != stored_head.head_revision {
            return Err(SessionStoreError::TranscriptRevisionConflict {
                id: id.clone(),
                expected: record.commit.parent_revision.clone(),
                actual: stored_head.head_revision.clone(),
            });
        }
        let before = record.commit.messages_before as u64;
        if before > self.strand_rows(id, &stored_head.strand).len() as u64 {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: id.clone(),
                reason: format!(
                    "commit messages_before {before} exceeds persisted rows of strand {}",
                    stored_head.strand
                ),
            });
        }
        let parent_rows = self.strand_messages(id, &stored_head.strand, 0..before)?;
        let parent_digest =
            transcript_messages_digest(&parent_rows).map_err(SessionStoreError::from)?;
        let next = validate_commit_rewrite_transition(
            id,
            record,
            stored_head,
            stored_token,
            expected,
            &parent_digest,
        )?;
        let idx = usize::try_from(stored_head.rewrite_count)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let row = StoredRewriteRow {
            commit: record.commit.clone(),
            parent_strand: stored_head.strand.clone(),
            parent_len: before,
            strand: next.strand.clone(),
            strand_len: record.commit.messages_after as u64,
        };
        let rows = self.rewrites.entry(id.clone()).or_default();
        if idx < rows.len() {
            // Replace the unadopted row at this idx (idempotent retry).
            rows[idx] = row;
            rows.truncate(idx + 1);
        } else {
            rows.push(row);
        }
        self.append_rows(id, &next.strand, 0, &record.revision_body.messages)?;
        Ok(next)
    }
}

fn layout_for_blob_session(
    session: &Session,
) -> Result<(meerkat_core::StrandLayout, SessionHead), SessionStoreError> {
    let state = session.transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("stored transcript history state is malformed: {err}"),
        }
    })?;
    let layout = strand_layout_for_history(session.id(), state.as_ref(), session.messages())?;
    let head = SessionHead::from_session(
        session,
        layout.head_strand.clone(),
        layout.rewrites.len() as u64,
    )?;
    Ok((layout, head))
}

/// In-memory session store
pub struct MemoryStore {
    state: RwLock<MemoryStoreState>,
}

impl MemoryStore {
    /// Create a new in-memory store
    pub fn new() -> Self {
        Self {
            state: RwLock::new(MemoryStoreState::default()),
        }
    }

    /// Snapshot of the store-call counters.
    pub async fn stats(&self) -> MemoryStoreStats {
        self.state.read().await.stats.clone()
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionStore for MemoryStore {
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
        let mut state = self.state.write().await;
        state.stats.whole_blob_saves += 1;
        if let Some((head, _token)) = state.heads.get(session.id()).cloned() {
            let adopted = state.adopted_commits(session.id(), head.rewrite_count);
            let previous = state.materialize_slim(&head)?;
            head_canonical_plain_save_guard(session, &previous, &adopted)?;
            return state.write_head_canonical_session(session, &head);
        }
        // F1 closure (wave-c C-H1): same shrink-guard as persistent
        // backends so behaviour is uniform across `SessionStore`
        // implementations.
        let previous = state.sessions.get(session.id());
        meerkat_core::session_store::append_only_save_guard(session, previous)?;
        state.sessions.insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn save_transcript_rewrite(
        &self,
        session: &Session,
        commit: &meerkat_core::TranscriptRewriteCommit,
    ) -> Result<(), SessionStoreError> {
        let mut state = self.state.write().await;
        state.stats.whole_blob_saves += 1;
        if let Some(stored) = state.heads.get(session.id()).cloned() {
            let incoming_revision =
                transcript_messages_digest(session.messages()).map_err(SessionStoreError::from)?;
            if incoming_revision != commit.revision {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!(
                        "incoming current transcript digest {incoming_revision} does not match commit revision {}",
                        commit.revision
                    ),
                });
            }
            let parent_body = session
                .transcript_revision_body(&commit.parent_revision)
                .map_err(SessionStoreError::from)?
                .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!(
                        "incoming rewrite omitted parent revision body {}",
                        commit.parent_revision
                    ),
                })?;
            let revision_body = session
                .transcript_revision_body(&commit.revision)
                .map_err(SessionStoreError::from)?
                .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!(
                        "incoming rewrite omitted new revision body {}",
                        commit.revision
                    ),
                })?;
            let record = TranscriptRewriteRecord::new(commit.clone(), parent_body, revision_body)
                .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!("transcript rewrite record failed validation: {err}"),
            })?;
            let expected = SessionHeadCas::IfToken(stored.1.clone());
            let next = state.commit_rewrite(session.id(), &record, &expected, &stored)?;
            let adopted_head =
                SessionHead::from_session(session, next.strand.clone(), next.rewrite_count)?;
            return state.write_head(&adopted_head);
        }
        let previous = state.sessions.get(session.id());
        meerkat_core::session_store::transcript_rewrite_save_guard(session, previous, commit)?;
        state.sessions.insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn save_authoritative_projection(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        let mut state = self.state.write().await;
        state.stats.whole_blob_saves += 1;
        if let Some((head, _token)) = state.heads.get(session.id()).cloned() {
            return state.write_head_canonical_session(session, &head);
        }
        state.sessions.insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), SessionStoreError> {
        let mut state = self.state.write().await;
        state.stats.whole_blob_saves += 1;
        if let Some((head, _token)) = state.heads.get(session.id()).cloned() {
            let previous = state.materialize_slim(&head)?;
            meerkat_core::session_store::authoritative_projection_current_revision_guard(
                session,
                Some(&previous),
                expected_current_revision.as_deref(),
            )?;
            return state.write_head_canonical_session(session, &head);
        }
        let previous = state.sessions.get(session.id());
        meerkat_core::session_store::authoritative_projection_current_revision_guard(
            session,
            previous,
            expected_current_revision.as_deref(),
        )?;
        state.sessions.insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        let mut state = self.state.write().await;
        state.stats.full_loads += 1;
        if let Some((head, _token)) = state.heads.get(id).cloned() {
            // Slim, no history metadata — the O(live) cold-resume contract.
            return Ok(Some(state.materialize_slim(&head)?));
        }
        Ok(state.sessions.get(id).cloned())
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        let state = self.state.read().await;
        let mut metas: Vec<SessionMeta> = Vec::new();
        for (id, (head, _token)) in &state.heads {
            metas.push(SessionMeta {
                id: id.clone(),
                created_at: head.created_at,
                updated_at: head.updated_at,
                message_count: usize::try_from(head.message_count).unwrap_or(usize::MAX),
                total_tokens: head.usage.total_tokens(),
                metadata: head.metadata.clone(),
            });
        }
        for (id, session) in &state.sessions {
            if state.heads.contains_key(id) {
                continue;
            }
            metas.push(SessionMeta::from(session));
        }
        metas.retain(|meta| {
            if let Some(created_after) = filter.created_after
                && meta.created_at < created_after
            {
                return false;
            }
            if let Some(updated_after) = filter.updated_after
                && meta.updated_at < updated_after
            {
                return false;
            }
            true
        });

        // Sort by updated_at descending
        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        // Apply pagination
        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(usize::MAX);

        Ok(metas.into_iter().skip(offset).take(limit).collect())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
        let mut state = self.state.write().await;
        state.sessions.remove(id);
        state.heads.remove(id);
        state.strands.remove(id);
        state.rewrites.remove(id);
        Ok(())
    }

    async fn delete_if_current_revision(
        &self,
        id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        let mut state = self.state.write().await;
        let previous = if let Some((head, _token)) = state.heads.get(id).cloned() {
            Some(state.materialize_slim(&head)?)
        } else {
            state.sessions.get(id).cloned()
        };
        let Some(previous) = previous else {
            return Ok(false);
        };
        let previous_token = meerkat_core::session_store::session_projection_cas_token(&previous)?;
        if previous_token != expected_current_revision {
            return Ok(false);
        }
        state.sessions.remove(id);
        state.heads.remove(id);
        state.strands.remove(id);
        state.rewrites.remove(id);
        Ok(true)
    }

    fn as_incremental(self: Arc<Self>) -> Option<Arc<dyn IncrementalSessionStore>> {
        Some(self)
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl IncrementalSessionStore for MemoryStore {
    async fn append_messages(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        base_seq: u64,
        messages: &[Message],
    ) -> Result<(), SessionStoreError> {
        let mut state = self.state.write().await;
        let _ = state.ensure_head_canonical_for_write(id)?;
        let _appended = state.append_rows(id, strand, base_seq, messages)?;
        Ok(())
    }

    async fn commit_rewrite(
        &self,
        id: &SessionId,
        record: &TranscriptRewriteRecord,
        expected: SessionHeadCas,
    ) -> Result<SessionHead, SessionStoreError> {
        let mut state = self.state.write().await;
        state.stats.rewrite_commits += 1;
        let stored = state.ensure_head_canonical_for_write(id)?.ok_or_else(|| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: id.clone(),
                reason: "rewrite target has no persisted session head".to_string(),
            }
        })?;
        state.commit_rewrite(id, record, &expected, &stored)
    }

    async fn save_head(
        &self,
        head: &SessionHead,
        expected: SessionHeadCas,
    ) -> Result<(), SessionStoreError> {
        let mut state = self.state.write().await;
        state.stats.head_saves += 1;
        let stored = state.ensure_head_canonical_for_write(&head.id)?;
        let strand_len = state.strand_rows(&head.id, &head.strand).len() as u64;
        let recorded = state
            .rewrites
            .get(&head.id)
            .map(|rows| rows.len() as u64)
            .unwrap_or(0);
        validate_save_head_transition(
            head,
            stored.as_ref().map(|(h, t)| (h, t.as_str())),
            &expected,
            strand_len,
            recorded,
        )?;
        state.write_head(head)
    }

    async fn load_head(&self, id: &SessionId) -> Result<Option<SessionHead>, SessionStoreError> {
        let mut state = self.state.write().await;
        state.stats.head_loads += 1;
        if let Some((head, _token)) = state.heads.get(id) {
            return Ok(Some(head.clone()));
        }
        // Blob-only session: synthesize read-only (no write).
        let Some(session) = state.sessions.get(id) else {
            return Ok(None);
        };
        let (_layout, head) = layout_for_blob_session(session)?;
        Ok(Some(head))
    }

    async fn load_messages(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        range: std::ops::Range<u64>,
    ) -> Result<Vec<Message>, SessionStoreError> {
        let state = self.state.read().await;
        if state.heads.contains_key(id) {
            return state.strand_messages(id, strand, range);
        }
        let Some(session) = state.sessions.get(id) else {
            return Err(SessionStoreError::NotFound(id.clone()));
        };
        let (layout, _head) = layout_for_blob_session(session)?;
        let rows = layout
            .strands
            .iter()
            .find(|(sid, _)| sid == strand)
            .map(|(_, rows)| rows.as_slice())
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let start =
            usize::try_from(range.start).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let end =
            usize::try_from(range.end).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        if start > end || end > rows.len() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        Ok(rows[start..end].to_vec())
    }

    async fn load_rewrites(
        &self,
        id: &SessionId,
    ) -> Result<Vec<TranscriptRewriteRecord>, SessionStoreError> {
        let state = self.state.read().await;
        if let Some((head, _token)) = state.heads.get(id) {
            let adopted = usize::try_from(head.rewrite_count)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            let rows = state.rewrites.get(id).map(Vec::as_slice).unwrap_or(&[]);
            return rows
                .iter()
                .take(adopted)
                .map(|row| {
                    let parent_messages =
                        state.strand_messages(id, &row.parent_strand, 0..row.parent_len)?;
                    let revision_messages =
                        state.strand_messages(id, &row.strand, 0..row.strand_len)?;
                    reconstruct_rewrite_record(
                        id,
                        row.commit.clone(),
                        parent_messages,
                        revision_messages,
                    )
                })
                .collect();
        }
        let Some(session) = state.sessions.get(id) else {
            return Ok(Vec::new());
        };
        let (layout, _head) = layout_for_blob_session(session)?;
        layout
            .rewrites
            .into_iter()
            .map(|rewrite| {
                let parent_len = usize::try_from(rewrite.parent_len)
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
                let strand_len = usize::try_from(rewrite.strand_len)
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
                let parent_messages = layout
                    .strands
                    .iter()
                    .find(|(sid, _)| *sid == rewrite.parent_strand)
                    .map(|(_, rows)| rows[..parent_len].to_vec())
                    .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
                let revision_messages = layout
                    .strands
                    .iter()
                    .find(|(sid, _)| *sid == rewrite.strand)
                    .map(|(_, rows)| rows[..strand_len].to_vec())
                    .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
                reconstruct_rewrite_record(id, rewrite.commit, parent_messages, revision_messages)
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::{Message, TranscriptRewriteReason, TranscriptRewriteSelection, UserMessage};

    #[tokio::test]
    async fn test_memory_store_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let store = MemoryStore::new();

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("Test".to_string())));

        let id = session.id().clone();

        store.save(&session).await?;

        let loaded = store.load(&id).await?.ok_or("session not found")?;
        assert_eq!(loaded.id(), &id);
        assert_eq!(loaded.messages().len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn authoritative_projection_expected_revision_rejects_stale_writer()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = MemoryStore::new();

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("base".to_string())));
        store.save(&session).await?;
        let expected_revision = session.transcript_revision()?;

        let mut newer = session.clone();
        newer.push(Message::User(UserMessage::text("newer".to_string())));
        store.save(&newer).await?;

        let mut stale_projection = session.clone();
        stale_projection.push(Message::User(UserMessage::text("stale".to_string())));
        let err = store
            .save_authoritative_projection_if_current_revision(
                &stale_projection,
                Some(expected_revision),
            )
            .await
            .expect_err("stale authoritative projection should be rejected");
        assert!(matches!(
            err,
            SessionStoreError::TranscriptContinuityViolation { .. }
        ));

        let saved = store
            .load(session.id())
            .await?
            .expect("session should remain saved");
        assert_eq!(saved.messages().len(), newer.messages().len());
        assert_eq!(saved.transcript_revision()?, newer.transcript_revision()?);
        Ok(())
    }

    #[tokio::test]
    async fn delete_if_current_revision_only_deletes_matching_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = MemoryStore::new();

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("base".to_string())));
        store.save(&session).await?;
        let stale_token = meerkat_core::session_store::session_projection_cas_token(&session)?;

        let mut newer = session.clone();
        newer.push(Message::User(UserMessage::text("newer".to_string())));
        store.save(&newer).await?;
        assert!(
            !store
                .delete_if_current_revision(session.id(), &stale_token)
                .await?
        );
        assert!(store.load(session.id()).await?.is_some());

        let current_token = meerkat_core::session_store::session_projection_cas_token(&newer)?;
        assert!(
            store
                .delete_if_current_revision(session.id(), &current_token)
                .await?
        );
        assert!(store.load(session.id()).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn incremental_capability_and_guard_parity() -> Result<(), Box<dyn std::error::Error>> {
        let store = Arc::new(MemoryStore::new());
        let inc: Arc<dyn IncrementalSessionStore> = Arc::clone(&store)
            .as_incremental()
            .expect("memory store must expose the incremental capability");

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("one".to_string())));
        session.push(Message::User(UserMessage::text("two".to_string())));
        let root = TranscriptStrandId::root();
        inc.append_messages(session.id(), &root, 0, session.messages())
            .await?;
        let head = SessionHead::from_session(&session, root.clone(), 0)?;
        inc.save_head(&head, SessionHeadCas::Create).await?;

        // Gap append fails closed with the same error as sqlite.
        let err = inc
            .append_messages(
                session.id(),
                &root,
                7,
                &[Message::User(UserMessage::text("gap".to_string()))],
            )
            .await
            .expect_err("gap append must be rejected");
        assert!(matches!(
            err,
            SessionStoreError::TranscriptContinuityViolation { .. }
        ));

        // Rewrite commit + adoption; load() serves the slim head materialization.
        let mut compacted = session.clone();
        let commit = compacted.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
            vec![Message::User(UserMessage::text(
                "[compacted] summary".to_string(),
            ))],
            TranscriptRewriteReason::new("compaction"),
            Some("test".to_string()),
            None,
        )?;
        let parent_body = compacted
            .transcript_revision_body(&commit.parent_revision)?
            .expect("parent body retained");
        let revision_body = compacted
            .transcript_revision_body(&commit.revision)?
            .expect("revision body retained");
        let record = TranscriptRewriteRecord::new(commit.clone(), parent_body, revision_body)?;
        let token = session_head_cas_token(&head)?;
        let next = inc
            .commit_rewrite(
                session.id(),
                &record,
                SessionHeadCas::IfToken(token.clone()),
            )
            .await?;
        assert!(inc.load_rewrites(session.id()).await?.is_empty());
        inc.save_head(&next, SessionHeadCas::IfToken(token)).await?;
        assert_eq!(inc.load_rewrites(session.id()).await?.len(), 1);

        let slim = store.load(session.id()).await?.expect("slim load");
        assert_eq!(slim.messages().len(), 1);
        assert!(slim.transcript_history_state()?.is_none());

        let stats = store.stats().await;
        assert_eq!(stats.rewrite_commits, 1);
        assert_eq!(stats.head_saves, 2);
        assert_eq!(
            stats.appended_message_rows, 3,
            "2 root rows + 1 rewrite base row"
        );
        Ok(())
    }
}
