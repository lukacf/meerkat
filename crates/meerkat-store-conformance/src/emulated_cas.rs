//! Reference append-only "emulated CAS" session store.
//!
//! Models the class of backends (BigQuery-shaped append-only media) that
//! cannot do cheap in-place compare-and-swap:
//!
//! - every write **INSERTS a sibling row** with `version = current + 1`;
//!   nothing is ever updated in place;
//! - "the current row" is resolved with a **windowed read** over the most
//!   recent sibling rows;
//! - a write attempt inserts its sibling row **before** the revision guard is
//!   validated (the shape remote inserts actually have), so a failed or
//!   racing attempt leaves an **orphan higher-version sibling row** behind.
//!
//! The contract this reference implementation documents (and the
//! [`append_only`](crate::chapters::append_only) chapter enforces) is that
//! **superseded-sibling-row deduplication is the store's job**: an orphan
//! row that never committed must be invisible to reads and to revision-guard
//! resolution, and must be pruned once a later write commits. A store that
//! instead adopts the highest-version sibling as current lets one failed
//! attempt make every subsequent guarded save permanently stale — the ob3
//! zombie incident.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use meerkat_core::session_store::{
    append_only_save_guard, authoritative_projection_current_revision_guard,
    session_projection_cas_token, transcript_rewrite_save_guard,
};
use meerkat_core::{
    IncrementalSessionStore, Session, SessionFilter, SessionId, SessionMeta, SessionStore,
    SessionStoreError, TranscriptRewriteCommit,
};

/// Windowed-read span: only this many most-recent sibling rows are consulted
/// when resolving the current row (the emulated-CAS read shape).
const SIBLING_READ_WINDOW: usize = 32;

#[derive(Debug, Clone)]
struct SiblingRow {
    version: u64,
    committed: bool,
    tombstone: bool,
    bytes: Arc<[u8]>,
}

#[derive(Default)]
struct RowSet {
    rows: Vec<SiblingRow>,
}

impl RowSet {
    fn next_version(&self) -> u64 {
        self.rows
            .last()
            .map_or(1, |row| row.version.saturating_add(1))
    }

    fn current_session(
        &self,
        id: &SessionId,
        orphans_visible: bool,
    ) -> Result<Option<Session>, SessionStoreError> {
        current_session_in(&self.rows, id, orphans_visible)
    }

    fn mark_committed(&mut self, version: u64) {
        for row in &mut self.rows {
            if row.version == version {
                row.committed = true;
            }
        }
    }

    /// Deduplication ownership: once a write commits, every uncommitted
    /// sibling it supersedes is the store's garbage and is pruned.
    fn prune_superseded_orphans(&mut self, committed_version: u64) {
        self.rows
            .retain(|row| row.committed || row.version > committed_version);
    }
}

/// Windowed read: the newest visible row wins.
///
/// With `orphans_visible = false` (the correct contract) only committed rows
/// are visible; uncommitted orphans left by failed attempts are the store's
/// own garbage and never become current.
fn visible_current(rows: &[SiblingRow], orphans_visible: bool) -> Option<&SiblingRow> {
    rows.iter()
        .rev()
        .take(SIBLING_READ_WINDOW)
        .find(|row| row.committed || orphans_visible)
}

fn current_session_in(
    rows: &[SiblingRow],
    id: &SessionId,
    orphans_visible: bool,
) -> Result<Option<Session>, SessionStoreError> {
    let Some(row) = visible_current(rows, orphans_visible) else {
        return Ok(None);
    };
    if row.tombstone {
        return Ok(None);
    }
    decode(id, &row.bytes).map(Some)
}

fn decode(id: &SessionId, bytes: &[u8]) -> Result<Session, SessionStoreError> {
    serde_json::from_slice(bytes).map_err(|_| SessionStoreError::Corrupted(id.clone()))
}

fn encode(session: &Session) -> Result<Arc<[u8]>, SessionStoreError> {
    let bytes = serde_json::to_vec(session).map_err(SessionStoreError::from)?;
    Ok(Arc::from(bytes.into_boxed_slice()))
}

/// Reference append-only session store with emulated (windowed-read) CAS.
///
/// Deliberately stays on the whole-blob compat path: `as_incremental`
/// returns `None`. Uses a `std` mutex (never held across an await) so the
/// reference store carries no async-runtime dependency on wasm32.
pub struct EmulatedCasSessionStore {
    state: Mutex<HashMap<SessionId, RowSet>>,
    /// When true, uncommitted orphan siblings are visible to current-row
    /// resolution — the exact bug shape of the ob3 zombie incident. Only the
    /// in-crate self-tests construct the store in this mode, to prove the
    /// append-only chapter detects the class.
    orphan_rows_visible: bool,
}

impl EmulatedCasSessionStore {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
            orphan_rows_visible: false,
        }
    }

    /// The buggy variant: windowed reads adopt orphan (uncommitted) sibling
    /// rows as current. Used by the in-crate self-tests to prove the
    /// append-only chapter fails loudly on the incident class.
    #[cfg(test)]
    pub(crate) fn new_naive() -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
            orphan_rows_visible: true,
        }
    }

    /// Lock the row state; a poisoned lock (a prior panic mid-write) is a
    /// typed internal failure, never a panic in library code.
    fn lock_state(&self) -> Result<MutexGuard<'_, HashMap<SessionId, RowSet>>, SessionStoreError> {
        self.state.lock().map_err(|_| {
            SessionStoreError::Internal("emulated-CAS row state mutex poisoned".to_string())
        })
    }

    async fn append_guarded<G>(&self, session: &Session, guard: G) -> Result<(), SessionStoreError>
    where
        G: FnOnce(&Session, Option<&Session>) -> Result<(), SessionStoreError>,
    {
        let bytes = encode(session)?;
        let mut state = self.lock_state()?;
        let rows = state.entry(session.id().clone()).or_default();
        let attempt_version = rows.next_version();
        // The INSERT happens before validation — this is the append-only
        // media shape. A failed attempt leaves this row as an orphan.
        rows.rows.push(SiblingRow {
            version: attempt_version,
            committed: false,
            tombstone: false,
            bytes,
        });
        // The attempt row itself must never be its own guard baseline: the
        // baseline is what a windowed read observed before this insert.
        let below_attempt = &rows.rows[..rows.rows.len().saturating_sub(1)];
        let previous = current_session_in(below_attempt, session.id(), self.orphan_rows_visible)?;
        guard(session, previous.as_ref())?;
        rows.mark_committed(attempt_version);
        rows.prune_superseded_orphans(attempt_version);
        Ok(())
    }
}

impl Default for EmulatedCasSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionStore for EmulatedCasSessionStore {
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
        self.append_guarded(session, append_only_save_guard).await
    }

    async fn save_transcript_rewrite(
        &self,
        session: &Session,
        commit: &TranscriptRewriteCommit,
    ) -> Result<(), SessionStoreError> {
        self.append_guarded(session, |incoming, previous| {
            transcript_rewrite_save_guard(incoming, previous, commit)
        })
        .await
    }

    async fn save_authoritative_projection(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        self.append_guarded(session, |_incoming, _previous| Ok(()))
            .await
    }

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), SessionStoreError> {
        self.append_guarded(session, |incoming, previous| {
            authoritative_projection_current_revision_guard(
                incoming,
                previous,
                expected_current_revision.as_deref(),
            )
        })
        .await
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        let state = self.lock_state()?;
        let Some(rows) = state.get(id) else {
            return Ok(None);
        };
        rows.current_session(id, self.orphan_rows_visible)
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        let state = self.lock_state()?;
        let mut metas = Vec::new();
        for (id, rows) in state.iter() {
            if let Some(session) = rows.current_session(id, self.orphan_rows_visible)? {
                metas.push(SessionMeta::from(&session));
            }
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
        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(usize::MAX);
        Ok(metas.into_iter().skip(offset).take(limit).collect())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
        let mut state = self.lock_state()?;
        let Some(rows) = state.get_mut(id) else {
            return Ok(());
        };
        let version = rows.next_version();
        rows.rows.push(SiblingRow {
            version,
            committed: true,
            tombstone: true,
            bytes: Arc::from(Vec::new().into_boxed_slice()),
        });
        rows.prune_superseded_orphans(version);
        Ok(())
    }

    async fn delete_if_current_revision(
        &self,
        id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        let mut state = self.lock_state()?;
        let Some(rows) = state.get_mut(id) else {
            return Ok(false);
        };
        let Some(current) = rows.current_session(id, self.orphan_rows_visible)? else {
            return Ok(false);
        };
        let current_token = session_projection_cas_token(&current)?;
        if current_token != expected_current_revision {
            return Ok(false);
        }
        let version = rows.next_version();
        rows.rows.push(SiblingRow {
            version,
            committed: true,
            tombstone: true,
            bytes: Arc::from(Vec::new().into_boxed_slice()),
        });
        rows.prune_superseded_orphans(version);
        Ok(true)
    }

    // Deliberately on the whole-blob compat path: no incremental capability.
    fn as_incremental(self: Arc<Self>) -> Option<Arc<dyn IncrementalSessionStore>> {
        None
    }
}
