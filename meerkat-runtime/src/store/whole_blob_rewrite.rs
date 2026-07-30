//! Prepared one-write boundary for a logical transcript-rewrite suffix.
//!
//! Whole-blob stores cannot make the final document smaller, but they do not
//! need to publish one physical document per logical rewrite occurrence. This
//! module proves the complete suffix once, materializes the final document
//! once, and hands a backend only the exact predecessor authority plus the
//! successor digest and bytes needed for one atomic compare-and-swap.

use std::sync::Arc;

use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
use meerkat_core::{
    CompactionProjectionIntent, Session, SessionId, TranscriptRewriteAuditReceiptBatch,
    TranscriptRewriteCommit, TranscriptRewritePrefixAccumulator,
};

use super::{
    CommittedWholeBlobSnapshot, RuntimeSessionCatalogEntry, RuntimeSessionPersistenceProfile,
    RuntimeStoreError, WholeBlobStoreAuthority,
};

static PREPARED_WHOLE_BLOB_SUCCESSOR_DOCUMENT_HASHES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static PREPARED_WHOLE_BLOB_SUCCESSOR_SEMANTIC_MINTS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Global count of semantic suffix proofs performed for prepared WholeBlob
/// rewrite successors.
///
/// This counts the one graph-suffix proof invocation, separately from
/// serialization, exact serialized-byte hashing, and the backend CAS.
#[doc(hidden)]
#[must_use]
pub fn prepared_whole_blob_successor_semantic_mints() -> u64 {
    PREPARED_WHOLE_BLOB_SUCCESSOR_SEMANTIC_MINTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Global acceptance counter for full successor-document hashes performed by
/// prepared WholeBlob rewrites.
///
/// This is intentionally process-wide like the core serialization counters:
/// acceptance tests compare deltas around one isolated operation.
#[doc(hidden)]
#[must_use]
pub fn prepared_whole_blob_successor_document_hashes() -> u64 {
    PREPARED_WHOLE_BLOB_SUCCESSOR_DOCUMENT_HASHES.load(std::sync::atomic::Ordering::Relaxed)
}

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
        PREPARED_WHOLE_BLOB_SUCCESSOR_SEMANTIC_MINTS
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        PREPARED_WHOLE_BLOB_SUCCESSOR_DOCUMENT_HASHES
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let successor_bytes = artifact.bytes_arc();
        let successor_blob_sha256 = artifact.row_sha256_token().to_string();
        let expected_authority = expected_runtime.store_authority;
        Ok(Self {
            expected_authority,
            successor,
            successor_bytes,
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

/// Store-facing exact CAS inputs for a prepared WholeBlob rewrite.
#[derive(Debug, Clone)]
pub struct PreparedWholeBlobRewriteStoreParts {
    expected_authority: WholeBlobStoreAuthority,
    successor_session_id: SessionId,
    successor_blob_sha256: String,
    successor_bytes: Arc<Vec<u8>>,
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
