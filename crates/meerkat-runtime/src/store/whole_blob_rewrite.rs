//! Prepared one-write boundary for a logical transcript-rewrite suffix.
//!
//! Whole-blob stores cannot make the final document smaller, but they do not
//! need to publish one physical document per logical rewrite occurrence. This
//! module proves the complete suffix once, materializes the final document
//! once, and hands a backend only the exact predecessor authority plus the
//! successor digest and bytes needed for one atomic compare-and-swap.

use std::sync::Arc;

use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
use meerkat_core::{
    CompactionProjectionIntent, Session, SessionId, TranscriptRewriteAuditReceiptBatch,
    TranscriptRewriteCommit, TranscriptRewritePrefixAccumulator, WholeBlobProvisionalTailAuthority,
};

use super::{
    CommittedWholeBlobSnapshot, RuntimeSessionCatalogEntry, RuntimeSessionPersistenceProfile,
    RuntimeStoreError, WholeBlobStoreAuthority,
};

/// One self-consistent current-domain WholeBlob document bound to exact bytes.
///
/// [`CommittedWholeBlobSnapshot`] already proves the store-owned byte/digest
/// pairing. This wrapper adds current-domain graph validation without
/// re-hashing or decoding the accumulated document a second time. It
/// intentionally carries no Session checkpoint or projection authority.
#[derive(Debug, Clone)]
pub struct VerifiedCommittedWholeBlobPayload {
    session: Arc<Session>,
    bytes: Arc<Vec<u8>>,
    store_authority: WholeBlobStoreAuthority,
}

impl VerifiedCommittedWholeBlobPayload {
    /// Validate one exact committed current-domain document under its typed
    /// session key.
    pub fn from_committed(
        expected_session_id: &SessionId,
        committed: CommittedWholeBlobSnapshot,
    ) -> Result<Self, RuntimeStoreError> {
        let parsed = Self::from_committed_unkeyed(committed)?;
        if parsed.session.id() != expected_session_id {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: expected_session_id.clone(),
                actual: parsed.session.id().clone(),
            });
        }
        Ok(parsed)
    }

    /// Validate one exact durable current-domain document when the store key
    /// does not itself carry a typed [`SessionId`].
    ///
    /// The committed carrier has already decoded the bytes and verified their
    /// SHA-256 against store authority. Reusing its shared values here is
    /// required: parsing or hashing again would add a second O(document) pass
    /// to every rewrite preparation.
    pub(crate) fn from_committed_unkeyed(
        committed: CommittedWholeBlobSnapshot,
    ) -> Result<Self, RuntimeStoreError> {
        let session = committed.session_arc();
        let bytes = committed.bytes_arc();
        let store_authority = committed.authority().clone();
        session
            .validated_transcript_history_state()
            .map_err(
                |error| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: store_authority.session_id().to_string(),
                    detail: format!("committed WholeBlob transcript graph is invalid: {error}"),
                },
            )?;
        Ok(Self {
            session,
            bytes,
            store_authority,
        })
    }

    /// Typed document parsed from [`Self::bytes`].
    #[must_use]
    pub fn session(&self) -> &Session {
        self.session.as_ref()
    }

    /// Exact parsed serialized document.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_ref()
    }

    /// Exact store-issued identity paired atomically with [`Self::bytes`].
    #[must_use]
    pub fn store_authority(&self) -> &WholeBlobStoreAuthority {
        &self.store_authority
    }
}

/// Valid-by-construction WholeBlob rewrite boundary retained by the caller.
///
/// The typed successor and receipt never enter the backend. A store receives
/// only [`PreparedWholeBlobRewriteStoreParts`], preventing an implementation
/// from replacing the core proof with a hand-written rewrite predicate.
#[derive(Debug)]
pub struct PreparedWholeBlobRewriteBoundary {
    expected_authority: WholeBlobStoreAuthority,
    successor: Arc<Session>,
    successor_bytes: Arc<Vec<u8>>,
    successor_encode_bytes: u64,
    successor_blob_sha256: String,
    successor_catalog_entry: RuntimeSessionCatalogEntry,
    compaction_projection_intents: Arc<[CompactionProjectionIntent]>,
    audit_receipt: Arc<TranscriptRewriteAuditReceiptBatch>,
}

impl PreparedWholeBlobRewriteBoundary {
    /// Prove an exact ordered logical rewrite suffix and prepare its single
    /// physical successor document.
    pub fn prepare(
        expected_runtime: VerifiedCommittedWholeBlobPayload,
        successor_session: Session,
        commits: &[TranscriptRewriteCommit],
    ) -> Result<Self, RuntimeStoreError> {
        let session_id = expected_runtime.store_authority().session_id().clone();
        if successor_session.id() != &session_id {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: session_id,
                actual: successor_session.id().clone(),
            });
        }
        if commits.is_empty() {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: session_id.to_string(),
                detail: "prepared WholeBlob rewrite boundary has no logical occurrences"
                    .to_string(),
            });
        }
        if let Some(audit_receipt) = exact_committed_rewrite_receipt(&expected_runtime, commits)? {
            // The physical successor already landed and the only missing
            // effect is its audit receipt. Reuse the RuntimeStore-authenticated
            // typed document, bytes, and digest so the backend still performs
            // one exact-authority CAS check without re-encoding, re-hashing, or
            // re-minting successor semantics from a caller-carried Session.
            let VerifiedCommittedWholeBlobPayload {
                session,
                bytes,
                store_authority,
            } = expected_runtime;
            let successor_catalog_entry = RuntimeSessionCatalogEntry::from_session(
                session.as_ref(),
                RuntimeSessionPersistenceProfile::WholeBlobV1,
                None,
            )?;
            let compaction_projection_intents: Arc<[CompactionProjectionIntent]> =
                super::validated_compaction_projection_intents(session.as_ref())?.into();
            let successor_blob_sha256 = store_authority.blob_sha256().to_string();
            return Ok(Self {
                expected_authority: store_authority,
                successor: session,
                successor_bytes: bytes,
                successor_encode_bytes: 0,
                successor_blob_sha256,
                successor_catalog_entry,
                compaction_projection_intents,
                audit_receipt: Arc::new(audit_receipt),
            });
        }
        let committed_prefix = expected_runtime
            .session()
            .validated_transcript_history_state()
            .map_err(
                |error| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: session_id.to_string(),
                    detail: format!("committed WholeBlob transcript graph is invalid: {error}"),
                },
            )?
            .map_or_else(TranscriptRewritePrefixAccumulator::empty, |history| {
                history.state().rewrite_prefix().clone()
            });
        let successor_history = successor_session
            .validated_transcript_history_state()
            .map_err(
                |error| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: session_id.to_string(),
                    detail: format!("prepared WholeBlob successor graph is invalid: {error}"),
                },
            )?
            .ok_or_else(|| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: session_id.to_string(),
                detail: "prepared WholeBlob successor has no rewrite graph".to_string(),
            })?;
        let successor_prefix = successor_history.state().rewrite_prefix().clone();
        let suffix = successor_history
            .prove_commit_suffix_after(&committed_prefix)
            .map_err(
                |error| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: session_id.to_string(),
                    detail: format!("prepared WholeBlob rewrite suffix is invalid: {error}"),
                },
            )?;
        let selected = suffix.commits();
        if selected.len() != commits.len()
            || !selected
                .zip(commits)
                .all(|(selected, supplied)| selected == supplied)
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: session_id.to_string(),
                detail: "prepared WholeBlob commits are not the exact selected rewrite suffix"
                    .to_string(),
            });
        }
        if suffix.start_prefix() != &committed_prefix || suffix.end_prefix() != &successor_prefix {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: session_id.to_string(),
                detail:
                    "prepared WholeBlob rewrite suffix endpoints do not bind physical predecessor and successor graph prefixes"
                        .to_string(),
            });
        }
        let audit_receipt = TranscriptRewriteAuditReceiptBatch::new(
            suffix.start_prefix().clone(),
            commits.to_vec(),
            suffix.end_prefix().clone(),
        )
        .map_err(
            |error| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: session_id.to_string(),
                detail: format!("failed to prepare rewrite audit receipt: {error}"),
            },
        )?;
        let successor = Arc::new(successor_session);
        let successor_catalog_entry = RuntimeSessionCatalogEntry::from_session(
            successor.as_ref(),
            RuntimeSessionPersistenceProfile::WholeBlobV1,
            None,
        )?;
        let compaction_projection_intents: Arc<[CompactionProjectionIntent]> =
            super::validated_compaction_projection_intents(successor.as_ref())?.into();
        let carrier = BoundSessionCommit::sealed(Arc::clone(&successor)).map_err(|error| {
            RuntimeStoreError::WriteFailed(format!(
                "failed to seal prepared WholeBlob rewrite successor: {error}"
            ))
        })?;
        let artifact = carrier.whole_blob_artifact().map_err(|error| {
            RuntimeStoreError::WriteFailed(format!(
                "failed to materialize prepared WholeBlob rewrite successor: {error}"
            ))
        })?;
        let successor_bytes = artifact.bytes_arc();
        let successor_encode_bytes = successor_bytes.len() as u64;
        let successor_blob_sha256 = artifact.row_sha256_token().to_string();
        let expected_authority = expected_runtime.store_authority;
        Ok(Self {
            expected_authority,
            successor,
            successor_bytes,
            successor_encode_bytes,
            successor_blob_sha256,
            successor_catalog_entry,
            compaction_projection_intents,
            audit_receipt: Arc::new(audit_receipt),
        })
    }

    /// Exact store-only inputs. Cloning these parts copies only authority
    /// metadata and an `Arc` to the already-materialized successor bytes.
    #[must_use]
    pub fn store_parts(&self) -> PreparedWholeBlobRewriteStoreParts {
        PreparedWholeBlobRewriteStoreParts {
            expected_authority: self.expected_authority.clone(),
            successor_session_id: self.successor.id().clone(),
            successor_blob_sha256: self.successor_blob_sha256.clone(),
            successor_bytes: Arc::clone(&self.successor_bytes),
            successor_encode_bytes: self.successor_encode_bytes,
            successor_catalog_entry: self.successor_catalog_entry.clone(),
            compaction_projection_intents: Arc::clone(&self.compaction_projection_intents),
        }
    }

    /// Exact store-issued predecessor this boundary is allowed to replace.
    #[must_use]
    pub fn expected_authority(&self) -> &WholeBlobStoreAuthority {
        &self.expected_authority
    }

    /// Receipt-only ordered audit transition retained by the caller.
    #[must_use]
    pub fn audit_receipt(&self) -> &TranscriptRewriteAuditReceiptBatch {
        self.audit_receipt.as_ref()
    }

    /// Exact successor blob digest expected in the store-issued acknowledgement.
    #[must_use]
    pub fn successor_blob_sha256(&self) -> &str {
        &self.successor_blob_sha256
    }

    /// Check one backend acknowledgement against the prepared physical row.
    #[must_use]
    pub fn accepts_committed_authority(&self, authority: &WholeBlobStoreAuthority) -> bool {
        if authority.session_id() != self.expected_authority.session_id()
            || authority.blob_sha256() != self.successor_blob_sha256
        {
            return false;
        }
        (authority.store_revision() == self.expected_authority.store_revision()
            && self.successor_blob_sha256 == self.expected_authority.blob_sha256())
            || authority.store_revision()
                == self.expected_authority.store_revision().saturating_add(1)
    }

    /// Exact successor bytes retained for post-commit verification.
    #[must_use]
    pub fn successor_bytes(&self) -> &[u8] {
        self.successor_bytes.as_ref()
    }

    /// Borrow the typed successor for audit/event facts owned by the caller.
    #[must_use]
    pub fn successor(&self) -> &Session {
        self.successor.as_ref()
    }

    /// Consume the rich carrier and recover the sole owned typed successor
    /// without a document-sized clone.
    pub fn into_successor(self) -> Result<Session, Arc<Session>> {
        Arc::try_unwrap(self.successor)
    }
}

fn exact_committed_rewrite_receipt(
    expected_runtime: &VerifiedCommittedWholeBlobPayload,
    commits: &[TranscriptRewriteCommit],
) -> Result<Option<TranscriptRewriteAuditReceiptBatch>, RuntimeStoreError> {
    let Some(first) = commits.first() else {
        return Ok(None);
    };
    let Some(history) = expected_runtime
        .session()
        .validated_transcript_history_state()
        .map_err(
            |error| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: expected_runtime.store_authority().session_id().to_string(),
                detail: format!("committed WholeBlob rewrite-repair graph is invalid: {error}"),
            },
        )?
    else {
        return Ok(None);
    };
    let Some(start_index) = first
        .rewrite_generation
        .checked_sub(1)
        .and_then(|index| usize::try_from(index).ok())
    else {
        return Ok(None);
    };
    if start_index.checked_add(commits.len()) != Some(history.state().commit_count())
        || history.state().commit(start_index) != Some(first)
    {
        return Ok(None);
    }
    // Receipt-only proof: it needs no edge bodies, so it also covers
    // occurrences the graph has retired.
    let receipt = history
        .audit_receipt_starting_with(first)
        .map_err(
            |error| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: expected_runtime.store_authority().session_id().to_string(),
                detail: format!("failed to prepare committed WholeBlob rewrite repair: {error}"),
            },
        )?;
    if receipt.commits() != commits {
        return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: expected_runtime.store_authority().session_id().to_string(),
            detail: "committed WholeBlob rewrite-repair commits differ from the exact sealed tail"
                .to_string(),
        });
    }
    Ok(Some(receipt))
}

/// Store-facing exact CAS inputs for a prepared WholeBlob rewrite.
#[derive(Debug, Clone)]
pub struct PreparedWholeBlobRewriteStoreParts {
    expected_authority: WholeBlobStoreAuthority,
    successor_session_id: SessionId,
    successor_blob_sha256: String,
    successor_bytes: Arc<Vec<u8>>,
    successor_encode_bytes: u64,
    successor_catalog_entry: RuntimeSessionCatalogEntry,
    compaction_projection_intents: Arc<[CompactionProjectionIntent]>,
}

impl PreparedWholeBlobRewriteStoreParts {
    #[must_use]
    pub fn expected_authority(&self) -> &WholeBlobStoreAuthority {
        &self.expected_authority
    }

    #[must_use]
    pub fn successor_session_id(&self) -> &SessionId {
        &self.successor_session_id
    }

    #[must_use]
    pub fn successor_blob_sha256(&self) -> &str {
        &self.successor_blob_sha256
    }

    #[must_use]
    pub fn successor_bytes(&self) -> &[u8] {
        self.successor_bytes.as_ref()
    }

    /// Exact bytes encoded while preparing this physical successor.
    ///
    /// Receipt-only repair carries the already-committed bytes and reports
    /// zero. This value otherwise binds the same artifact handed to the CAS.
    #[doc(hidden)]
    #[must_use]
    pub fn successor_encode_bytes(&self) -> u64 {
        self.successor_encode_bytes
    }

    /// Exact successor compaction intents proved once by rich preparation.
    ///
    /// Backends compare these opaque typed values against their already
    /// committed non-finalized outbox rows inside the same CAS lock or
    /// transaction; they never deserialize the successor document.
    #[must_use]
    pub fn compaction_projection_intents(&self) -> &[CompactionProjectionIntent] {
        self.compaction_projection_intents.as_ref()
    }

    #[must_use]
    pub fn into_tuple(
        self,
    ) -> (
        WholeBlobStoreAuthority,
        SessionId,
        String,
        Arc<Vec<u8>>,
        RuntimeSessionCatalogEntry,
        Arc<[CompactionProjectionIntent]>,
    ) {
        (
            self.expected_authority,
            self.successor_session_id,
            self.successor_blob_sha256,
            self.successor_bytes,
            self.successor_catalog_entry,
            self.compaction_projection_intents,
        )
    }
}

/// A WholeBlob run's re-base at a compaction: the compacted document becomes
/// the session's committed base, and the run's provisional tail restarts on
/// top of it, in ONE store call
/// ([`super::RuntimeStore::commit_prepared_whole_blob_run_rebase`]).
///
/// - The committed successor is the compacted document with its run cursor
///   cleared: a cursor inside a committed body never makes a run resumable.
///   Its descent from the committed base is proven by
///   [`PreparedWholeBlobRewriteBoundary::prepare`] over the exact rewrite
///   suffix. That proof binds rewrite occurrences only, so rounds the run
///   appended after the committed base (its own uncommitted rows) need no
///   other proof: they travel inside the successor document.
/// - The provisional candidate is the same document with the run cursor, a
///   metadata-only tail whose resume point is the one the run holds at the
///   compaction. The store mints its authority on the just-issued committed
///   authority, for the same run, at candidate sequence one.
/// - A closing save re-bases too: a run whose last rewrite has no round
///   commit after it (a compaction at its final boundary, a closing write
///   after a turn cap or a hard failure) saves a document without a run
///   cursor, as a committed boundary never carries one. The committed
///   successor and the candidate are then the same document, and the tail
///   (equal to its base, with no cursor) is the run's closing save.
///
/// Like the rewrite boundary, the typed documents never reach the backend:
/// it receives only [`PreparedWholeBlobRunRebaseStoreParts`].
#[derive(Debug)]
pub struct PreparedWholeBlobRunRebase {
    rewrite: PreparedWholeBlobRewriteBoundary,
    expected_tail: Option<WholeBlobProvisionalTailAuthority>,
    run_id: RunId,
    candidate_bytes: Arc<Vec<u8>>,
    candidate_blob_sha256: String,
    conversation_digest: String,
    message_count: u64,
    candidate_catalog_entry: RuntimeSessionCatalogEntry,
    candidate_compaction_intents: Vec<CompactionProjectionIntent>,
}

fn rebase_conflict(session_id: &SessionId, detail: impl Into<String>) -> RuntimeStoreError {
    RuntimeStoreError::SessionPersistenceAuthorityConflict {
        runtime_id: session_id.to_string(),
        detail: detail.into(),
    }
}

impl PreparedWholeBlobRunRebase {
    /// Prove the re-base of run `run_id` onto its compacted document.
    ///
    /// `expected_runtime` is the committed base the run started from (or
    /// its last re-base); `expected_tail` is the run's current provisional
    /// tail on that base, or `None` when the run has written none yet;
    /// `compacted` is the run's live session after the compaction, carrying
    /// the run cursor of `run_id`, or no cursor for a closing save; `commits`
    /// is the exact rewrite suffix since the committed base.
    ///
    /// # Errors
    /// The compacted document's cursor is malformed or names another run,
    /// the expected tail does not belong to this base and run, or the
    /// rewrite proof refuses the committed successor.
    pub fn prepare(
        expected_runtime: VerifiedCommittedWholeBlobPayload,
        expected_tail: Option<WholeBlobProvisionalTailAuthority>,
        run_id: RunId,
        compacted: Session,
        commits: &[TranscriptRewriteCommit],
    ) -> Result<Self, RuntimeStoreError> {
        let base = expected_runtime.store_authority().clone();
        let session_id = base.session_id().clone();
        if compacted.id() != &session_id {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: session_id,
                actual: compacted.id().clone(),
            });
        }
        // A closing save carries no cursor; a cursor, when present, must
        // name this run.
        let cursor = compacted.try_run_cursor().map_err(|error| {
            rebase_conflict(
                &session_id,
                format!("WholeBlob re-base candidate run cursor is malformed: {error}"),
            )
        })?;
        if cursor.is_some_and(|cursor| cursor.run_id != run_id) {
            return Err(rebase_conflict(
                &session_id,
                "WholeBlob re-base candidate cursor names another run",
            ));
        }
        if let Some(tail) = expected_tail.as_ref()
            && (tail.session_id() != &session_id
                || tail.run_id() != &run_id
                || tail.base_store_revision() != base.store_revision()
                || tail.base_blob_sha256() != base.blob_sha256())
        {
            return Err(rebase_conflict(
                &session_id,
                "WholeBlob re-base expected tail does not belong to this base and run",
            ));
        }
        let candidate_artifact = compacted.to_persisted_artifact().map_err(|error| {
            RuntimeStoreError::WriteFailed(format!(
                "failed to materialize WholeBlob re-base candidate: {error}"
            ))
        })?;
        let candidate_catalog_entry = RuntimeSessionCatalogEntry::from_session(
            &compacted,
            RuntimeSessionPersistenceProfile::WholeBlobV1,
            None,
        )?;
        let conversation_digest = compacted.transcript_content_digest().map_err(|error| {
            rebase_conflict(
                &session_id,
                format!("failed to derive WholeBlob re-base conversation digest: {error}"),
            )
        })?;
        let message_count = u64::try_from(compacted.messages().len()).map_err(|_| {
            rebase_conflict(
                &session_id,
                "WholeBlob re-base message count exceeds the durable range",
            )
        })?;
        let candidate_compaction_intents =
            super::validated_compaction_projection_intents(&compacted)?;
        let mut committed = compacted;
        committed.clear_run_cursor();
        let rewrite =
            PreparedWholeBlobRewriteBoundary::prepare(expected_runtime, committed, commits)?;
        Ok(Self {
            rewrite,
            expected_tail,
            run_id,
            candidate_bytes: candidate_artifact.bytes_arc(),
            candidate_blob_sha256: candidate_artifact.row_sha256_token().to_string(),
            conversation_digest,
            message_count,
            candidate_catalog_entry,
            candidate_compaction_intents,
        })
    }

    /// The proven committed side: its audit receipt, successor and digest,
    /// for the caller's audit receipt write after the store call.
    #[must_use]
    pub fn rewrite(&self) -> &PreparedWholeBlobRewriteBoundary {
        &self.rewrite
    }

    /// The run this re-base belongs to.
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// The exact provisional candidate digest the store's new tail names.
    #[must_use]
    pub fn candidate_blob_sha256(&self) -> &str {
        &self.candidate_blob_sha256
    }

    /// The candidate's transcript content digest.
    #[must_use]
    pub fn conversation_digest(&self) -> &str {
        &self.conversation_digest
    }

    /// The candidate's message count.
    #[must_use]
    pub const fn message_count(&self) -> u64 {
        self.message_count
    }

    /// Exact store-only inputs. Cloning them copies authority metadata and
    /// `Arc`s to the already-materialized documents.
    #[must_use]
    pub fn store_parts(&self) -> PreparedWholeBlobRunRebaseStoreParts {
        PreparedWholeBlobRunRebaseStoreParts {
            rewrite: self.rewrite.store_parts(),
            expected_tail: self.expected_tail.clone(),
            run_id: self.run_id.clone(),
            candidate_blob_sha256: self.candidate_blob_sha256.clone(),
            candidate_bytes: Arc::clone(&self.candidate_bytes),
            conversation_digest: self.conversation_digest.clone(),
            message_count: self.message_count,
            candidate_catalog_entry: self.candidate_catalog_entry.clone(),
            candidate_compaction_intents: self.candidate_compaction_intents.clone(),
        }
    }
}

/// Store-facing inputs of a prepared WholeBlob run re-base.
#[derive(Debug, Clone)]
pub struct PreparedWholeBlobRunRebaseStoreParts {
    rewrite: PreparedWholeBlobRewriteStoreParts,
    expected_tail: Option<WholeBlobProvisionalTailAuthority>,
    run_id: RunId,
    candidate_blob_sha256: String,
    candidate_bytes: Arc<Vec<u8>>,
    conversation_digest: String,
    message_count: u64,
    candidate_catalog_entry: RuntimeSessionCatalogEntry,
    candidate_compaction_intents: Vec<CompactionProjectionIntent>,
}

/// The parts of a WholeBlob run re-base a backend writes, in order: the
/// committed side, the expected current tail, the run, and the candidate's
/// digest, bytes, conversation digest, message count, catalog entry and
/// compaction intents.
pub(crate) type WholeBlobRunRebaseParts = (
    PreparedWholeBlobRewriteStoreParts,
    Option<WholeBlobProvisionalTailAuthority>,
    RunId,
    String,
    Arc<Vec<u8>>,
    String,
    u64,
    RuntimeSessionCatalogEntry,
    Vec<CompactionProjectionIntent>,
);

impl PreparedWholeBlobRunRebaseStoreParts {
    /// The committed side: exactly a rewrite boundary's store parts.
    #[must_use]
    pub fn rewrite(&self) -> &PreparedWholeBlobRewriteStoreParts {
        &self.rewrite
    }

    /// The provisional tail the store must hold now (`None`: no tail).
    #[must_use]
    pub fn expected_tail(&self) -> Option<&WholeBlobProvisionalTailAuthority> {
        self.expected_tail.as_ref()
    }

    #[must_use]
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    #[must_use]
    pub fn candidate_blob_sha256(&self) -> &str {
        &self.candidate_blob_sha256
    }

    #[must_use]
    pub fn candidate_bytes(&self) -> &[u8] {
        self.candidate_bytes.as_ref()
    }

    #[must_use]
    pub fn conversation_digest(&self) -> &str {
        &self.conversation_digest
    }

    #[must_use]
    pub const fn message_count(&self) -> u64 {
        self.message_count
    }

    #[must_use]
    pub fn candidate_catalog_entry(&self) -> &RuntimeSessionCatalogEntry {
        &self.candidate_catalog_entry
    }

    #[must_use]
    pub fn candidate_compaction_intents(&self) -> &[CompactionProjectionIntent] {
        &self.candidate_compaction_intents
    }

    /// The store authorities the re-base issues when it applies: the
    /// committed successor (one revision after the expected base, with the
    /// successor digest) and the new tail on it (same run, sequence one).
    ///
    /// # Errors
    /// An exhausted revision, or parts that cannot mint an authority.
    pub fn issued_authorities(
        &self,
    ) -> Result<(WholeBlobStoreAuthority, WholeBlobProvisionalTailAuthority), RuntimeStoreError>
    {
        let expected = self.rewrite.expected_authority();
        let revision = expected.store_revision().checked_add(1).ok_or_else(|| {
            RuntimeStoreError::WriteFailed(format!(
                "WholeBlob store revision exhausted for session {}",
                expected.session_id()
            ))
        })?;
        let committed = WholeBlobStoreAuthority::issued(
            self.rewrite.successor_session_id().clone(),
            revision,
            self.rewrite.successor_blob_sha256().to_string(),
        )?;
        let tail = WholeBlobProvisionalTailAuthority::issued(
            self.rewrite.successor_session_id().clone(),
            revision,
            self.rewrite.successor_blob_sha256().to_string(),
            self.run_id.clone(),
            self.candidate_blob_sha256.clone(),
            1,
        )
        .map_err(|error| rebase_conflict(self.rewrite.successor_session_id(), error.to_string()))?;
        Ok((committed, tail))
    }

    pub(crate) fn into_parts(self) -> WholeBlobRunRebaseParts {
        (
            self.rewrite,
            self.expected_tail,
            self.run_id,
            self.candidate_blob_sha256,
            self.candidate_bytes,
            self.conversation_digest,
            self.message_count,
            self.candidate_catalog_entry,
            self.candidate_compaction_intents,
        )
    }
}
