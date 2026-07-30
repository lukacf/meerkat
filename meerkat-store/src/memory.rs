//! In-memory session store (for testing)

use crate::{SessionFilter, SessionStore, SessionStoreError};
use async_trait::async_trait;
use meerkat_core::session_store::{
    IncrementalSessionStore, PreparedHeadCanonicalParentTransition, SaveGuardWitness, SessionHead,
    SessionHeadCas, StrandLayout, StrandSegment, StrandSplice,
    head_canonical_plain_save_guard_with_prefix_witness, reconstruct_rewrite_record,
    session_head_cas_token, strand_layout_for_history, validate_commit_rewrite_transition,
    validate_save_head_transition,
};
use meerkat_core::transcript_messages_digest;
use meerkat_core::types::Message;
use meerkat_core::{
    Session, SessionId, SessionMeta, TranscriptRewriteCommit, TranscriptRewriteRecord,
    TranscriptStrandId,
};
use std::collections::{BTreeMap, HashMap};
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
    _graph_edge_json: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredStrandLink {
    successor: TranscriptStrandId,
    splice: StrandSplice,
}

#[derive(Default)]
struct MemoryStoreState {
    /// Legacy whole-blob rows. Once a head exists for a session, its blob
    /// entry (if any) is a frozen migration archive, never read again.
    sessions: HashMap<SessionId, Session>,
    heads: HashMap<SessionId, (SessionHead, String)>,
    /// Physical rows only. Linked strands are sparse: the replacement span
    /// lives here while shared prefix/suffix rows resolve through `links`.
    strands: HashMap<SessionId, HashMap<TranscriptStrandId, BTreeMap<u64, Vec<u8>>>>,
    links: HashMap<SessionId, HashMap<TranscriptStrandId, StoredStrandLink>>,
    rewrites: HashMap<SessionId, Vec<StoredRewriteRow>>,
    stats: MemoryStoreStats,
}

impl MemoryStoreState {
    fn physical_rows(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
    ) -> Option<&BTreeMap<u64, Vec<u8>>> {
        self.strands.get(id).and_then(|strands| strands.get(strand))
    }

    fn physical_row_extent(&self, id: &SessionId, strand: &TranscriptStrandId) -> u64 {
        self.physical_rows(id, strand)
            .and_then(|rows| rows.last_key_value())
            .and_then(|(seq, _)| seq.checked_add(1))
            .unwrap_or(0)
    }

    fn strand_logical_len(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
    ) -> Result<u64, SessionStoreError> {
        let physical_extent = self.physical_row_extent(id, strand);
        match self.links.get(id).and_then(|links| links.get(strand)) {
            Some(link) if link.splice.is_well_formed() => {
                Ok(link.splice.strand_len.max(physical_extent))
            }
            Some(_) => Err(SessionStoreError::Corrupted(id.clone())),
            None => {
                let physical_count = self.physical_rows(id, strand).map_or(0, BTreeMap::len);
                if u64::try_from(physical_count)
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?
                    != physical_extent
                {
                    return Err(SessionStoreError::Corrupted(id.clone()));
                }
                Ok(physical_extent)
            }
        }
    }

    fn strand_bytes(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        range: std::ops::Range<u64>,
    ) -> Result<Vec<Vec<u8>>, SessionStoreError> {
        let hop_limit = self.links.get(id).map_or(0, HashMap::len);
        self.strand_bytes_hops(id, strand, range, hop_limit)
    }

    fn strand_bytes_hops(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        range: std::ops::Range<u64>,
        hops: usize,
    ) -> Result<Vec<Vec<u8>>, SessionStoreError> {
        let logical_len = self.strand_logical_len(id, strand)?;
        if range.start > range.end || range.end > logical_len {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        if range.is_empty() {
            return Ok(Vec::new());
        }
        let wanted = usize::try_from(range.end - range.start)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let Some(link) = self.links.get(id).and_then(|links| links.get(strand)) else {
            let rows = self
                .physical_rows(id, strand)
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            return range
                .map(|seq| {
                    rows.get(&seq)
                        .cloned()
                        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))
                })
                .collect();
        };
        if hops == 0 || !link.splice.is_well_formed() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let mut resolved = Vec::with_capacity(wanted);
        let linked_end = range.end.min(link.splice.strand_len);
        if range.start < linked_end {
            for segment in link.splice.segments(range.start..linked_end) {
                match segment {
                    StrandSegment::Retained(span) => {
                        let physical = self
                            .physical_rows(id, strand)
                            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
                        for seq in span {
                            resolved.push(
                                physical
                                    .get(&seq)
                                    .cloned()
                                    .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?,
                            );
                        }
                    }
                    StrandSegment::Successor(span) => {
                        resolved.extend(self.strand_bytes_hops(
                            id,
                            &link.successor,
                            span,
                            hops - 1,
                        )?);
                    }
                }
            }
        }
        let tail_start = range.start.max(link.splice.strand_len);
        if tail_start < range.end {
            let physical = self
                .physical_rows(id, strand)
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            for seq in tail_start..range.end {
                resolved.push(
                    physical
                        .get(&seq)
                        .cloned()
                        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?,
                );
            }
        }
        if resolved.len() != wanted {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        Ok(resolved)
    }

    fn strand_messages(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        range: std::ops::Range<u64>,
    ) -> Result<Vec<Message>, SessionStoreError> {
        self.strand_bytes(id, strand, range)?
            .iter()
            .map(|bytes| {
                serde_json::from_slice::<Message>(bytes)
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))
            })
            .collect()
    }

    fn install_link(
        &mut self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        successor: &TranscriptStrandId,
        splice: StrandSplice,
    ) -> Result<(), SessionStoreError> {
        if strand == successor || !splice.is_well_formed() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        if self.strand_logical_len(id, successor)? != splice.successor_len() {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let incoming = StoredStrandLink {
            successor: successor.clone(),
            splice,
        };
        if let Some(existing) = self.links.get(id).and_then(|links| links.get(strand)) {
            return if existing == &incoming {
                Ok(())
            } else {
                Err(SessionStoreError::Corrupted(id.clone()))
            };
        }
        if self
            .physical_rows(id, strand)
            .is_some_and(|rows| !rows.is_empty())
        {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        let links = self.links.entry(id.clone()).or_default();
        match links.get(strand) {
            Some(existing) if existing == &incoming => Ok(()),
            Some(_) => Err(SessionStoreError::Corrupted(id.clone())),
            None => {
                links.insert(strand.clone(), incoming);
                Ok(())
            }
        }
    }

    fn put_physical_rows(
        &mut self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        base_seq: u64,
        serialized: &[Vec<u8>],
    ) -> Result<u64, SessionStoreError> {
        let rows = self
            .strands
            .entry(id.clone())
            .or_default()
            .entry(strand.clone())
            .or_default();
        let mut inserted = 0u64;
        for (offset, bytes) in serialized.iter().enumerate() {
            let offset =
                u64::try_from(offset).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            let seq = base_seq
                .checked_add(offset)
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            match rows.get(&seq) {
                Some(stored) if stored == bytes => {}
                Some(_) => {
                    return Err(SessionStoreError::TranscriptContinuityViolation {
                        id: id.clone(),
                        previous_revision: format!("strand:{strand} seq:{seq}"),
                        incoming_revision: "divergent-bytes".to_string(),
                        reason: "physical row already exists with different bytes".to_string(),
                    });
                }
                None => {
                    rows.insert(seq, bytes.clone());
                    inserted = inserted
                        .checked_add(1)
                        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
                }
            }
        }
        self.stats.appended_message_rows =
            self.stats
                .appended_message_rows
                .checked_add(inserted)
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        Ok(inserted)
    }

    fn append_serialized_rows(
        &mut self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        base_seq: u64,
        serialized: &[Vec<u8>],
    ) -> Result<u64, SessionStoreError> {
        let existing = self.strand_logical_len(id, strand)?;
        if base_seq > existing {
            return Err(SessionStoreError::TranscriptContinuityViolation {
                id: id.clone(),
                previous_revision: format!("strand-rows:{existing}"),
                incoming_revision: format!("append-base-seq:{base_seq}"),
                reason: format!(
                    "append at base_seq {base_seq} would leave a gap in strand {strand} with \
                     {existing} logical rows"
                ),
            });
        }
        let incoming_len = u64::try_from(serialized.len())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let incoming_end = base_seq
            .checked_add(incoming_len)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let overlap_end = incoming_end.min(existing);
        if base_seq < overlap_end {
            let overlap = self.strand_bytes(id, strand, base_seq..overlap_end)?;
            let overlap_len = usize::try_from(overlap_end - base_seq)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            if overlap.as_slice() != &serialized[..overlap_len] {
                return Err(SessionStoreError::TranscriptContinuityViolation {
                    id: id.clone(),
                    previous_revision: format!("strand:{strand} rows:{base_seq}..{overlap_end}"),
                    incoming_revision: "divergent-bytes".to_string(),
                    reason: "append would overwrite immutable logical rows".to_string(),
                });
            }
        }
        if incoming_end <= existing {
            return Ok(0);
        }
        let suffix_offset = usize::try_from(existing.saturating_sub(base_seq))
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        self.put_physical_rows(id, strand, existing, &serialized[suffix_offset..])
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
        self.append_serialized_rows(id, strand, base_seq, &serialized)
    }

    /// Persist the compact physical layout of one sealed WholeBlob session.
    ///
    /// The anchor is the only full historical vector. Every rewrite installs
    /// an immutable link plus the exact replacement span, and parent/live
    /// advances append only their suffix rows.
    fn install_layout(
        &mut self,
        id: &SessionId,
        layout: &StrandLayout,
    ) -> Result<(), SessionStoreError> {
        self.append_serialized_rows(id, &layout.anchor_strand, 0, &layout.serialized_anchor)?;
        let mut expected_source = layout.anchor_strand.clone();
        let mut expected_source_len = u64::try_from(layout.serialized_anchor.len())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let mut rewrite_rows = Vec::with_capacity(layout.rewrites.len());
        for rewrite in &layout.rewrites {
            if rewrite.parent_base_seq != expected_source_len {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            match &rewrite.parent_transition {
                PreparedHeadCanonicalParentTransition::ExactAppend => {
                    if rewrite.parent_strand != expected_source {
                        return Err(SessionStoreError::Corrupted(id.clone()));
                    }
                }
                PreparedHeadCanonicalParentTransition::ExactSplice(splice) => {
                    if splice.source_strand() != &expected_source
                        || &rewrite.parent_strand == splice.source_strand()
                        || splice.link_splice().retained_rows()
                            != u64::try_from(splice.serialized_replacement().len())
                                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?
                    {
                        return Err(SessionStoreError::Corrupted(id.clone()));
                    }
                    self.install_link(
                        id,
                        &rewrite.parent_strand,
                        splice.source_strand(),
                        splice.link_splice(),
                    )?;
                    self.put_physical_rows(
                        id,
                        &rewrite.parent_strand,
                        splice.link_splice().splice_start,
                        splice.serialized_replacement(),
                    )?;
                }
            }
            self.append_serialized_rows(
                id,
                &rewrite.parent_strand,
                rewrite.parent_base_seq,
                &rewrite.serialized_parent_suffix,
            )?;
            let parent_len = u64::try_from(rewrite.commit.messages_before)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            if self.strand_logical_len(id, &rewrite.parent_strand)? != parent_len {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            self.install_link(
                id,
                &rewrite.strand,
                &rewrite.parent_strand,
                rewrite.link_splice,
            )?;
            if rewrite.link_splice.retained_rows()
                != u64::try_from(rewrite.serialized_replacement.len())
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?
            {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            self.put_physical_rows(
                id,
                &rewrite.strand,
                rewrite.link_splice.splice_start,
                &rewrite.serialized_replacement,
            )?;
            let strand_len = u64::try_from(rewrite.commit.messages_after)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            if self.strand_logical_len(id, &rewrite.strand)? != strand_len {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
            rewrite_rows.push(StoredRewriteRow {
                commit: rewrite.commit.clone(),
                parent_strand: rewrite.parent_strand.clone(),
                parent_len,
                strand: rewrite.strand.clone(),
                strand_len,
                _graph_edge_json: Some(rewrite.serialized_graph_edge.clone()),
            });
            expected_source = rewrite.strand.clone();
            expected_source_len = strand_len;
        }
        if layout.head_strand != expected_source {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        if layout.tail_base_seq != expected_source_len {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        self.append_serialized_rows(
            id,
            &layout.head_strand,
            layout.tail_base_seq,
            &layout.serialized_tail,
        )?;
        if self.strand_logical_len(id, &layout.head_strand)? != layout.head_len {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        self.rewrites.insert(id.clone(), rewrite_rows);
        Ok(())
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
        self.install_layout(id, &layout)?;
        let token = session_head_cas_token(&head)?;
        self.heads.insert(id.clone(), (head.clone(), token.clone()));
        Ok(Some((head, token)))
    }

    fn write_head(&mut self, head: &SessionHead) -> Result<(), SessionStoreError> {
        if head.metadata_identity().is_some() || head.realtime_event_prefix.is_some() {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: head.id.clone(),
                reason: "MemoryStore has a WholeBlob runtime profile and does not persist \
                         authenticated HeadCanonical metadata or realtime component events"
                    .to_string(),
            });
        }
        let token = session_head_cas_token(head)?;
        self.heads.insert(head.id.clone(), (head.clone(), token));
        Ok(())
    }

    fn materialize_slim(&self, head: &SessionHead) -> Result<Session, SessionStoreError> {
        let messages = self.strand_messages(&head.id, &head.strand, 0..head.message_count)?;
        head.clone().into_session(messages)
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
        let before = u64::try_from(record.commit.messages_before)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let after = u64::try_from(record.commit.messages_after)
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        if before > self.strand_logical_len(id, &stored_head.strand)? {
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
            strand_len: after,
            _graph_edge_json: None,
        };
        let recorded = self.rewrites.get(id).map_or(0, Vec::len);
        if idx > recorded {
            return Err(SessionStoreError::Corrupted(id.clone()));
        }
        // `validate_commit_rewrite_transition` proved that every Message
        // outside the selected span is equal; MemoryStore serializes every
        // physical row through the same canonical serde path. Derive the
        // physical splice from that proved edit shape and serialize only the
        // replacement; comparing/serializing both full endpoint vectors here
        // would put O(document) copying back into every rewrite.
        let (start, end) = record.commit.selection.bounds();
        let removed = end
            .checked_sub(start)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let retained = record
            .commit
            .messages_before
            .checked_sub(removed)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let replacement_len = record
            .commit
            .messages_after
            .checked_sub(retained)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let replacement_end = start
            .checked_add(replacement_len)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let replacement = record
            .revision_body
            .messages
            .get(start..replacement_end)
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let serialized_replacement = replacement
            .iter()
            .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
            .collect::<Result<Vec<_>, _>>()?;
        let splice = StrandSplice {
            strand_len: after,
            splice_start: u64::try_from(start)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            splice_end: u64::try_from(replacement_end)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            successor_end: u64::try_from(end)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
        };
        self.install_link(id, &next.strand, &stored_head.strand, splice)?;
        self.put_physical_rows(
            id,
            &next.strand,
            splice.splice_start,
            &serialized_replacement,
        )?;
        let rows = self.rewrites.entry(id.clone()).or_default();
        if idx < rows.len() {
            // Replace the unadopted row at this idx (idempotent retry).
            rows[idx] = row;
            rows.truncate(idx + 1);
        } else {
            rows.push(row);
        }
        Ok(next)
    }
}

fn layout_for_blob_session(
    session: &Session,
) -> Result<(meerkat_core::StrandLayout, SessionHead), SessionStoreError> {
    let history = session
        .validated_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("stored transcript history state is malformed: {err}"),
        })?;
    let layout = strand_layout_for_history(session, history.as_ref())?;
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
            let previous = state.materialize_slim(&head)?;
            head_canonical_plain_save_guard_with_prefix_witness(
                session,
                &previous,
                head.rewrite_count,
                &head.rewrite_prefix,
                SaveGuardWitness::none().with_previous_revision(&head.head_revision),
            )?;
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
            let incoming_revision = session
                .transcript_content_digest()
                .map_err(SessionStoreError::from)?;
            if incoming_revision != commit.revision {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!(
                        "incoming current transcript digest {incoming_revision} does not match commit revision {}",
                        commit.revision
                    ),
                });
            }
            let history = session
                .validated_transcript_history_state()
                .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!("incoming transcript graph is not sealed: {err}"),
                })?
                .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "incoming rewrite omitted its transcript graph".to_string(),
                })?;
            let parent_body = history.materialize_rewrite_parent(commit).map_err(|err| {
                SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!(
                        "incoming rewrite omitted parent revision body {}: {err}",
                        commit.parent_revision
                    ),
                }
            })?;
            let revision_body = history.materialize_rewrite_child(commit).map_err(|err| {
                SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!(
                        "incoming rewrite omitted new revision body {}: {err}",
                        commit.revision
                    ),
                }
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
                metadata: head.materialized_metadata()?,
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
        state.links.remove(id);
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
        state.links.remove(id);
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
        let strand_len = state.strand_logical_len(&head.id, &head.strand)?;
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

    async fn materialize_head(
        &self,
        expected: &SessionHead,
    ) -> Result<meerkat_core::VerifiedSessionHeadMaterialization, SessionStoreError> {
        let state = self.state.read().await;
        let (current, stored_token) = state
            .heads
            .get(&expected.id)
            .ok_or_else(|| SessionStoreError::NotFound(expected.id.clone()))?;
        let current_token = session_head_cas_token(current)?;
        if &current_token != stored_token {
            return Err(SessionStoreError::Corrupted(expected.id.clone()));
        }
        let expected_token = session_head_cas_token(expected)?;
        if expected_token != current_token {
            return Err(SessionStoreError::TranscriptRevisionConflict {
                id: expected.id.clone(),
                expected: expected_token,
                actual: current_token,
            });
        }
        if current.metadata_identity().is_some() || current.realtime_event_prefix.is_some() {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: expected.id.clone(),
                reason: "MemoryStore cannot materialize a rooted HeadCanonical head because it \
                         does not persist the authenticated realtime event sequence; its runtime \
                         persistence profile is WholeBlob"
                    .to_string(),
            });
        }
        let session = state.materialize_slim(current)?;
        expected.clone().verify_materialized_session(session)
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
        let mut staged = MemoryStoreState::default();
        staged.install_layout(id, &layout)?;
        staged.strand_messages(id, strand, range)
    }

    async fn load_rewrites(
        &self,
        id: &SessionId,
    ) -> Result<Vec<TranscriptRewriteRecord>, SessionStoreError> {
        let state = self.state.read().await;
        if let Some((head, _token)) = state.heads.get(id) {
            let adopted = usize::try_from(head.rewrite_count)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            let rows = state
                .rewrites
                .get(id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if adopted > rows.len() {
                return Err(SessionStoreError::Corrupted(id.clone()));
            }
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
        let mut staged = MemoryStoreState::default();
        staged.install_layout(id, &layout)?;
        let rows = staged
            .rewrites
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        rows.iter()
            .map(|row| {
                let parent_messages =
                    staged.strand_messages(id, &row.parent_strand, 0..row.parent_len)?;
                let revision_messages =
                    staged.strand_messages(id, &row.strand, 0..row.strand_len)?;
                reconstruct_rewrite_record(
                    id,
                    row.commit.clone(),
                    parent_messages,
                    revision_messages,
                )
            })
            .collect()
    }

    async fn load_canonical_head(
        &self,
        id: &SessionId,
    ) -> Result<Option<SessionHead>, SessionStoreError> {
        // Head row ONLY: unlike `load_head`, a blob-only session gets `None`
        // rather than a synthesized head (no blob layout at all).
        let state = self.state.read().await;
        Ok(state.heads.get(id).map(|(head, _token)| head.clone()))
    }

    async fn load_rewrite_commits(
        &self,
        id: &SessionId,
    ) -> Result<Vec<TranscriptRewriteCommit>, SessionStoreError> {
        let state = self.state.read().await;
        if let Some((head, _token)) = state.heads.get(id) {
            // Adopted commit rows only; no strand-body reads.
            let adopted = usize::try_from(head.rewrite_count)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            let rows = state
                .rewrites
                .get(id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let adopted_rows = rows
                .get(..adopted)
                .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
            return Ok(adopted_rows.iter().map(|row| row.commit.clone()).collect());
        }
        // Blob-only session: derive the commits from the blob's layout so the
        // answer always equals `load_rewrites`' commits (never served by the
        // fast read path — `load_canonical_head` is `None`).
        let Some(session) = state.sessions.get(id) else {
            return Ok(Vec::new());
        };
        let (layout, _head) = layout_for_blob_session(session)?;
        Ok(layout
            .rewrites
            .into_iter()
            .map(|rewrite| rewrite.commit)
            .collect())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::{Message, TranscriptRewriteReason, TranscriptRewriteSelection, UserMessage};

    #[test]
    fn blob_migration_stores_one_anchor_plus_rewrite_deltas() {
        let mut session = Session::new();
        for index in 0..64 {
            session.push(Message::User(UserMessage::text(format!("message {index}"))));
        }
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 8, end: 48 },
                vec![Message::User(UserMessage::text("summary".to_string()))],
                TranscriptRewriteReason::new("first compaction"),
                Some("test".to_string()),
                None,
            )
            .expect("first compact edge");
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::User(UserMessage::text("replacement".to_string()))],
                TranscriptRewriteReason::new("second compaction"),
                Some("test".to_string()),
                None,
            )
            .expect("second compact edge");

        let id = session.id().clone();
        let mut state = MemoryStoreState::default();
        state.sessions.insert(id.clone(), session.clone());
        let (head, _token) = state
            .ensure_head_canonical_for_write(&id)
            .expect("compact blob migration")
            .expect("migrated head");
        assert_eq!(head.rewrite_count, 2);
        let physical_rows = state
            .strands
            .get(&id)
            .into_iter()
            .flat_map(|strands| strands.values())
            .map(|rows| rows.len())
            .sum::<usize>();
        assert_eq!(
            physical_rows, 66,
            "64-row anchor plus one retained replacement row per rewrite"
        );
        assert_eq!(
            state
                .materialize_slim(&head)
                .expect("resolve sparse head")
                .messages(),
            session.messages()
        );
    }

    #[tokio::test]
    async fn canonical_head_is_row_only_and_rewrite_commits_match_load_rewrites() {
        let store = Arc::new(MemoryStore::new());

        // Blob-only session: plain save keeps the legacy blob entry; no head.
        let mut blob_session = Session::new();
        blob_session.push(Message::User(UserMessage::text("blob one".to_string())));
        store.save(&blob_session).await.unwrap();
        let inc = Arc::clone(&store)
            .as_incremental()
            .expect("memory store must expose the incremental capability");
        // `load_head` still synthesizes (compat contract, unchanged)...
        assert!(inc.load_head(blob_session.id()).await.unwrap().is_some());
        // ...but the canonical probe answers None without synthesizing.
        assert!(
            inc.load_canonical_head(blob_session.id())
                .await
                .unwrap()
                .is_none(),
            "a blob-only session has no canonical head row"
        );
        // Absent session: None, not an error.
        assert!(
            inc.load_canonical_head(&SessionId::new())
                .await
                .unwrap()
                .is_none()
        );

        // Head-canonical session through the incremental write path.
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("one".to_string())));
        session.push(Message::User(UserMessage::text("two".to_string())));
        let root = TranscriptStrandId::root();
        inc.append_messages(session.id(), &root, 0, session.messages())
            .await
            .unwrap();
        let head = SessionHead::from_session(&session, root, 0).unwrap();
        inc.save_head(&head, SessionHeadCas::Create).await.unwrap();
        let canonical = inc
            .load_canonical_head(session.id())
            .await
            .unwrap()
            .expect("head-canonical session must advertise its head row");
        assert_eq!(canonical, head);
        assert_eq!(Some(canonical), inc.load_head(session.id()).await.unwrap());
        let materialized = inc
            .materialize_head(&head)
            .await
            .expect("unrooted exact head must materialize");
        assert_eq!(materialized.session().messages(), session.messages());

        // Rewrite commit view: empty while recorded-but-unadopted, then
        // exactly load_rewrites' commits after adoption.
        assert!(
            inc.load_rewrite_commits(session.id())
                .await
                .unwrap()
                .is_empty()
        );
        let commit = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                vec![Message::User(UserMessage::text(
                    "[compacted] summary".to_string(),
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("test".to_string()),
                None,
            )
            .unwrap();
        let history = session
            .validated_transcript_history_state()
            .unwrap()
            .expect("rewrite must install a sealed compact graph");
        let parent_body = history.materialize_rewrite_parent(&commit).unwrap();
        let revision_body = history.materialize_rewrite_child(&commit).unwrap();
        let record =
            TranscriptRewriteRecord::new(commit.clone(), parent_body, revision_body).unwrap();
        let token = meerkat_core::session_store::session_head_cas_token(&head).unwrap();
        let next = inc
            .commit_rewrite(
                session.id(),
                &record,
                SessionHeadCas::IfToken(token.clone()),
            )
            .await
            .unwrap();
        assert!(
            inc.load_rewrite_commits(session.id())
                .await
                .unwrap()
                .is_empty(),
            "recorded-but-unadopted commits must not be served"
        );
        inc.save_head(&next, SessionHeadCas::IfToken(token))
            .await
            .unwrap();
        let commits = inc.load_rewrite_commits(session.id()).await.unwrap();
        assert_eq!(commits, vec![commit]);
        assert_eq!(
            commits,
            inc.load_rewrites(session.id())
                .await
                .unwrap()
                .into_iter()
                .map(|record| record.commit)
                .collect::<Vec<_>>()
        );

        // Blob-only parity: the commit view equals load_rewrites' commits for
        // a blob-only session too (both derived from the frozen blob layout).
        assert_eq!(
            inc.load_rewrite_commits(blob_session.id()).await.unwrap(),
            inc.load_rewrites(blob_session.id())
                .await
                .unwrap()
                .into_iter()
                .map(|record| record.commit)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn rooted_head_save_fails_closed_without_component_sequences() {
        let store = Arc::new(MemoryStore::new());
        let inc = Arc::clone(&store)
            .as_incremental()
            .expect("memory store must expose the incremental capability");
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("root".to_string())));
        let mutation =
            meerkat_core::session_store::PreparedHeadCanonicalMutation::prepare_root(&session)
                .expect("prepare rooted HeadCanonical mutation");
        let head = mutation.successor_head().clone();
        inc.append_messages(session.id(), &head.strand, 0, session.messages())
            .await
            .expect("persist exact transcript rows");
        let error = inc
            .save_head(&head, SessionHeadCas::Create)
            .await
            .expect_err("MemoryStore must refuse rooted heads it cannot persist completely");
        assert!(
            matches!(
                error,
                SessionStoreError::InvalidTranscriptRewrite { ref reason, .. }
                    if reason.contains("PreparedHeadCanonicalMutation")
            ),
            "unexpected rooted save error: {error}"
        );
    }

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
        let history = compacted
            .validated_transcript_history_state()?
            .expect("rewrite must install a sealed compact graph");
        let parent_body = history.materialize_rewrite_parent(&commit)?;
        let revision_body = history.materialize_rewrite_child(&commit)?;
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
        assert!(slim.transcript_history_state_shared()?.is_none());

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
