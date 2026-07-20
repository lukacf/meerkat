//! Typed session checkpoint identity and periodic persistence.
//!
//! Checkpoint content identity is deliberately independent of runtime
//! ownership. Leases, fencing tokens, process incarnations, and runtime epochs
//! decide whether a write is admitted; they are not part of the transcript
//! lineage described here.

use crate::session::{
    SESSION_CHECKPOINT_STAMP_KEY, SESSION_RUNTIME_CHECKPOINT_PROVENANCE_KEY,
    SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY, SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
    Session,
};
use crate::types::SessionId;
use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// Current durable schema for [`SessionCheckpointStamp`].
pub const SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION: u32 = 1;

/// Stable identity of one session-authority lineage.
///
/// A transcript fork mints a new lineage. Process restarts, lease rotations,
/// runtime epochs, and ownership-fence changes do not.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionLineageId(String);

impl SessionLineageId {
    /// Construct a validated lineage identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, SessionCheckpointError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(SessionCheckpointError::EmptyLineage);
        }
        Ok(Self(value))
    }

    /// Deterministic lineage for a session's generation-zero root.
    #[must_use]
    pub fn for_session(session_id: &SessionId) -> Self {
        Self(format!("session:{session_id}"))
    }

    /// Borrow the opaque lineage string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionLineageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for SessionLineageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Observed session-authority generation.
///
/// Ordinary runtime or host restarts preserve this value. Legacy migration
/// retains the exact observed generation without minting replacement
/// authority.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SessionGeneration(u64);

impl SessionGeneration {
    pub const INITIAL: Self = Self(0);

    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Monotonic checkpoint revision within one lineage generation.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SessionCheckpointRevision(u64);

impl SessionCheckpointRevision {
    pub const INITIAL: Self = Self(0);

    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Result<Self, SessionCheckpointError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(SessionCheckpointError::RevisionOverflow)
    }
}

/// Canonical SHA-256 of a versioned session document.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionCheckpointDigest(String);

impl SessionCheckpointDigest {
    pub fn parse(value: impl Into<String>) -> Result<Self, SessionCheckpointError> {
        let value = value.into();
        let Some(hex) = value.strip_prefix("sha256:") else {
            return Err(SessionCheckpointError::InvalidDigest(value));
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(SessionCheckpointError::InvalidDigest(value));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionCheckpointDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for SessionCheckpointDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// Typed origin of a durable session checkpoint.
///
/// No runtime epoch, lease, ownership fence, or process identifier is carried
/// here. Those facts fence writes; they do not version transcript content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionCheckpointProvenance {
    SessionCreated,
    Forked,
    IntraTurnCheckpoint,
    RunBoundaryCommit,
    TranscriptRewrite,
    RecoveryMigration,
}

/// Exact canonical authority from which a non-root checkpoint was derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SessionCheckpointAnchor {
    pub session_id: SessionId,
    pub lineage_id: SessionLineageId,
    pub generation: SessionGeneration,
    pub checkpoint_revision: SessionCheckpointRevision,
    pub digest: SessionCheckpointDigest,
    pub provenance: SessionCheckpointProvenance,
}

impl SessionCheckpointAnchor {
    #[must_use]
    pub fn from_stamp(stamp: &SessionCheckpointStamp) -> Self {
        Self {
            session_id: stamp.session_id.clone(),
            lineage_id: stamp.lineage_id.clone(),
            generation: stamp.generation,
            checkpoint_revision: stamp.checkpoint_revision,
            digest: stamp.digest.clone(),
            provenance: stamp.provenance,
        }
    }

    pub fn validate_for_session(
        &self,
        session_id: &SessionId,
        lineage_id: &SessionLineageId,
    ) -> Result<(), SessionCheckpointError> {
        if &self.session_id != session_id {
            return Err(SessionCheckpointError::SessionIdMismatch {
                expected: session_id.clone(),
                actual: self.session_id.clone(),
            });
        }
        if &self.lineage_id != lineage_id {
            return Err(SessionCheckpointError::AuthorityBaseConflict(format!(
                "checkpoint authority-base lineage {} differs from outer lineage {}",
                self.lineage_id, lineage_id
            )));
        }
        SessionCheckpointDigest::parse(self.digest.as_str())?;
        if self.provenance == SessionCheckpointProvenance::IntraTurnCheckpoint {
            return Err(SessionCheckpointError::AuthorityBaseConflict(
                "an intra-turn projection cannot be a checkpoint authority base".to_string(),
            ));
        }
        Ok(())
    }
}

/// Explicit ancestry of one typed checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionCheckpointAuthorityBase {
    /// Atomic session creation or fork root.
    Absent,
    /// Exact untyped document observed during a one-time migration.
    Legacy {
        source_blob_digest: SessionCheckpointDigest,
        observed_generation: SessionGeneration,
        observed_checkpoint_revision: SessionCheckpointRevision,
    },
    /// Exact typed predecessor.
    Typed { anchor: SessionCheckpointAnchor },
}

/// Durable semantic checkpoint identity embedded in a session document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SessionCheckpointStamp {
    schema_version: u32,
    session_id: SessionId,
    lineage_id: SessionLineageId,
    generation: SessionGeneration,
    checkpoint_revision: SessionCheckpointRevision,
    authority_base: SessionCheckpointAuthorityBase,
    digest: SessionCheckpointDigest,
    provenance: SessionCheckpointProvenance,
}

impl SessionCheckpointStamp {
    fn from_parts(
        session_id: SessionId,
        lineage_id: SessionLineageId,
        generation: SessionGeneration,
        checkpoint_revision: SessionCheckpointRevision,
        authority_base: SessionCheckpointAuthorityBase,
        digest: SessionCheckpointDigest,
        provenance: SessionCheckpointProvenance,
    ) -> Self {
        Self {
            schema_version: SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION,
            session_id,
            lineage_id,
            generation,
            checkpoint_revision,
            authority_base,
            digest,
            provenance,
        }
    }

    /// Construct a generation-zero create or fork root for this exact session
    /// document.
    pub fn root(
        session: &Session,
        provenance: SessionCheckpointProvenance,
    ) -> Result<Self, SessionCheckpointError> {
        if !matches!(
            provenance,
            SessionCheckpointProvenance::SessionCreated | SessionCheckpointProvenance::Forked
        ) {
            return Err(SessionCheckpointError::AuthorityBaseConflict(
                "checkpoint root provenance must be session_created or forked".to_string(),
            ));
        }
        let stamp = Self::from_parts(
            session.id().clone(),
            SessionLineageId::for_session(session.id()),
            SessionGeneration::INITIAL,
            SessionCheckpointRevision::INITIAL,
            SessionCheckpointAuthorityBase::Absent,
            session_checkpoint_digest(session)?,
            provenance,
        );
        stamp.validate_for_session(session.id())?;
        Ok(stamp)
    }

    /// Construct a typed migration root from one exact legacy session source
    /// BLOB and its externally observed continuity cursor.
    ///
    /// Nonzero cursors are retained exactly. Callers must establish coherence
    /// between the supplied cursor and their continuity row before invoking
    /// this function. The source bytes are decoded and checked against
    /// `session`; the authority base then binds to the exact legacy BLOB, not
    /// to a reserialization of the decoded value.
    pub fn recovery_migration(
        session: &Session,
        source_blob: &[u8],
        observed_generation: SessionGeneration,
        observed_checkpoint_revision: SessionCheckpointRevision,
    ) -> Result<Self, SessionCheckpointError> {
        if !matches!(
            session.try_checkpoint_state()?,
            SessionCheckpointState::LegacyUnverified { .. }
        ) {
            return Err(SessionCheckpointError::AuthorityBaseConflict(
                "recovery migration requires an untyped legacy session".to_string(),
            ));
        }
        let source_session: Session = serde_json::from_slice(source_blob)?;
        if !matches!(
            source_session.try_checkpoint_state()?,
            SessionCheckpointState::LegacyUnverified { .. }
        ) {
            return Err(SessionCheckpointError::AuthorityBaseConflict(
                "recovery migration source BLOB must be an untyped legacy session".to_string(),
            ));
        }
        if source_session.id() != session.id() {
            return Err(SessionCheckpointError::SessionIdMismatch {
                expected: session.id().clone(),
                actual: source_session.id().clone(),
            });
        }
        let digest = session_checkpoint_digest(session)?;
        let source_digest = session_checkpoint_digest(&source_session)?;
        if source_digest != digest {
            return Err(SessionCheckpointError::LegacySourceBlobMismatch {
                expected: digest,
                actual: source_digest,
            });
        }
        let source_blob_digest = legacy_session_source_blob_digest(source_blob);
        let stamp = Self::from_parts(
            session.id().clone(),
            SessionLineageId::for_session(session.id()),
            observed_generation,
            observed_checkpoint_revision,
            SessionCheckpointAuthorityBase::Legacy {
                source_blob_digest,
                observed_generation,
                observed_checkpoint_revision,
            },
            digest,
            SessionCheckpointProvenance::RecoveryMigration,
        );
        stamp.validate_for_session(session.id())?;
        Ok(stamp)
    }

    /// Construct the exact next checkpoint derived from `authority`.
    ///
    /// This is semantic construction only. It does not admit a target-store
    /// write; stores must still atomically revalidate their resource-local CAS
    /// and lease/fencing preconditions.
    pub fn successor(
        session: &Session,
        authority: &Self,
        provenance: SessionCheckpointProvenance,
    ) -> Result<Self, SessionCheckpointError> {
        authority.validate_for_session(session.id())?;
        if !matches!(
            provenance,
            SessionCheckpointProvenance::IntraTurnCheckpoint
                | SessionCheckpointProvenance::RunBoundaryCommit
                | SessionCheckpointProvenance::TranscriptRewrite
        ) {
            return Err(SessionCheckpointError::AuthorityBaseConflict(
                "checkpoint successor provenance must be checkpoint, boundary, or rewrite"
                    .to_string(),
            ));
        }
        let stamp = Self::from_parts(
            session.id().clone(),
            authority.lineage_id().clone(),
            authority.generation(),
            authority.checkpoint_revision().checked_next()?,
            SessionCheckpointAuthorityBase::Typed {
                anchor: SessionCheckpointAnchor::from_stamp(authority),
            },
            session_checkpoint_digest(session)?,
            provenance,
        );
        stamp.validate_for_session(session.id())?;
        Ok(stamp)
    }

    /// Construct a replaceable intra-turn projection of the current committed
    /// checkpoint authority.
    ///
    /// A projection may itself already carry `IntraTurnCheckpoint`
    /// provenance. Such a row is never promoted into an authority base;
    /// another projection remains a sibling anchored to the same committed
    /// checkpoint. This lets incremental stores persist crash-safe
    /// intermediate heads without turning projection order into semantic
    /// session authority.
    pub fn intra_turn_projection(
        session: &Session,
        observed: &Self,
    ) -> Result<Self, SessionCheckpointError> {
        observed.validate_for_session(session.id())?;
        let anchor = match (&observed.authority_base, observed.provenance) {
            (
                SessionCheckpointAuthorityBase::Typed { anchor },
                SessionCheckpointProvenance::IntraTurnCheckpoint,
            ) => anchor.clone(),
            _ => SessionCheckpointAnchor::from_stamp(observed),
        };
        anchor.validate_for_session(session.id(), &observed.lineage_id)?;
        let stamp = Self::from_parts(
            session.id().clone(),
            observed.lineage_id.clone(),
            anchor.generation,
            anchor.checkpoint_revision.checked_next()?,
            SessionCheckpointAuthorityBase::Typed { anchor },
            session_checkpoint_digest(session)?,
            SessionCheckpointProvenance::IntraTurnCheckpoint,
        );
        stamp.validate_for_session(session.id())?;
        Ok(stamp)
    }

    #[cfg(test)]
    fn new(
        session_id: SessionId,
        lineage_id: SessionLineageId,
        generation: SessionGeneration,
        checkpoint_revision: SessionCheckpointRevision,
        authority_base: SessionCheckpointAuthorityBase,
        digest: SessionCheckpointDigest,
        provenance: SessionCheckpointProvenance,
    ) -> Self {
        Self::from_parts(
            session_id,
            lineage_id,
            generation,
            checkpoint_revision,
            authority_base,
            digest,
            provenance,
        )
    }

    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn lineage_id(&self) -> &SessionLineageId {
        &self.lineage_id
    }

    #[must_use]
    pub const fn generation(&self) -> SessionGeneration {
        self.generation
    }

    #[must_use]
    pub const fn checkpoint_revision(&self) -> SessionCheckpointRevision {
        self.checkpoint_revision
    }

    #[must_use]
    pub fn authority_base(&self) -> &SessionCheckpointAuthorityBase {
        &self.authority_base
    }

    #[must_use]
    pub fn digest(&self) -> &SessionCheckpointDigest {
        &self.digest
    }

    #[must_use]
    pub const fn provenance(&self) -> SessionCheckpointProvenance {
        self.provenance
    }

    /// Validate all self-contained fields and the enclosing session identity.
    pub fn validate_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionCheckpointError> {
        if self.schema_version != SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION {
            return Err(SessionCheckpointError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        if &self.session_id != session_id {
            return Err(SessionCheckpointError::SessionIdMismatch {
                expected: session_id.clone(),
                actual: self.session_id.clone(),
            });
        }
        SessionLineageId::new(self.lineage_id.as_str())?;
        SessionCheckpointDigest::parse(self.digest.as_str())?;
        match &self.authority_base {
            SessionCheckpointAuthorityBase::Absent => {
                if self.generation != SessionGeneration::INITIAL
                    || self.checkpoint_revision != SessionCheckpointRevision::INITIAL
                    || !matches!(
                        self.provenance,
                        SessionCheckpointProvenance::SessionCreated
                            | SessionCheckpointProvenance::Forked
                    )
                {
                    return Err(SessionCheckpointError::AuthorityBaseConflict(
                        "absent authority base is legal only for a generation-zero create or fork root"
                            .to_string(),
                    ));
                }
            }
            SessionCheckpointAuthorityBase::Legacy {
                source_blob_digest,
                observed_generation,
                observed_checkpoint_revision,
            } => {
                SessionCheckpointDigest::parse(source_blob_digest.as_str())?;
                if self.lineage_id != SessionLineageId::for_session(session_id)
                    || self.generation != *observed_generation
                    || self.checkpoint_revision != *observed_checkpoint_revision
                    || self.provenance != SessionCheckpointProvenance::RecoveryMigration
                {
                    return Err(SessionCheckpointError::AuthorityBaseConflict(
                        "legacy migration must retain its exact observed cursor under the deterministic session lineage"
                            .to_string(),
                    ));
                }
            }
            SessionCheckpointAuthorityBase::Typed { anchor } => {
                anchor.validate_for_session(session_id, &self.lineage_id)?;
                if !matches!(
                    self.provenance,
                    SessionCheckpointProvenance::IntraTurnCheckpoint
                        | SessionCheckpointProvenance::RunBoundaryCommit
                        | SessionCheckpointProvenance::TranscriptRewrite
                ) {
                    return Err(SessionCheckpointError::AuthorityBaseConflict(
                        "typed authority base requires checkpoint, boundary, or rewrite provenance"
                            .to_string(),
                    ));
                }
                if self.generation != anchor.generation
                    || self.checkpoint_revision != anchor.checkpoint_revision.checked_next()?
                {
                    return Err(SessionCheckpointError::AuthorityBaseConflict(format!(
                        "checkpoint must be the exact successor of authority generation {} revision {}",
                        anchor.generation.get(),
                        anchor.checkpoint_revision.get()
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Compact proof that one exact checkpoint descends from another.
///
/// Revisions alone never prove ancestry. Every adjacent child must carry a
/// typed authority-base anchor naming the complete previous stamp, including
/// its digest and provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCheckpointAncestryProof {
    ancestor: SessionCheckpointStamp,
    descendant: SessionCheckpointStamp,
    edge_count: u64,
    path_digest: SessionCheckpointDigest,
}

impl SessionCheckpointAncestryProof {
    /// Validate an ordered ancestor-to-descendant stamp chain in streaming,
    /// constant memory.
    pub fn try_from_stamps(
        chain: impl IntoIterator<Item = SessionCheckpointStamp>,
    ) -> Result<Self, SessionCheckpointError> {
        let mut chain = chain.into_iter();
        let Some(first) = chain.next() else {
            return Err(SessionCheckpointError::EmptyAncestryProof);
        };
        first.validate_for_session(first.session_id())?;
        let ancestor = first.clone();
        let mut previous = first;
        let mut edge_count = 0_u64;
        let mut path_hasher = Sha256::new();
        path_hasher.update(b"meerkat:session-checkpoint-ancestry-proof:v1\0");
        update_ancestry_path_digest(&mut path_hasher, &previous)?;

        for child in chain {
            edge_count = edge_count
                .checked_add(1)
                .ok_or(SessionCheckpointError::AncestryEdgeCountOverflow)?;
            child.validate_for_session(child.session_id())?;
            if child.session_id() != ancestor.session_id() {
                return Err(SessionCheckpointError::AncestrySessionMismatch {
                    index: edge_count,
                    expected: ancestor.session_id().clone(),
                    actual: child.session_id().clone(),
                });
            }
            if child.lineage_id() != ancestor.lineage_id() {
                return Err(SessionCheckpointError::AncestryLineageMismatch {
                    index: edge_count,
                    expected: ancestor.lineage_id().clone(),
                    actual: child.lineage_id().clone(),
                });
            }
            if child.generation() != ancestor.generation() {
                return Err(SessionCheckpointError::AncestryGenerationMismatch {
                    index: edge_count,
                    expected: ancestor.generation().get(),
                    actual: child.generation().get(),
                });
            }
            if child.checkpoint_revision() <= previous.checkpoint_revision() {
                return Err(SessionCheckpointError::AncestryRevisionNotIncreasing {
                    index: edge_count,
                    previous: previous.checkpoint_revision().get(),
                    actual: child.checkpoint_revision().get(),
                });
            }
            if !matches!(
                child.authority_base(),
                SessionCheckpointAuthorityBase::Typed { anchor }
                    if anchor == &SessionCheckpointAnchor::from_stamp(&previous)
            ) {
                return Err(SessionCheckpointError::AncestryAuthorityBaseMismatch {
                    index: edge_count,
                });
            }
            update_ancestry_path_digest(&mut path_hasher, &child)?;
            previous = child;
        }
        Ok(Self {
            ancestor,
            descendant: previous,
            edge_count,
            path_digest: SessionCheckpointDigest(format!("sha256:{:x}", path_hasher.finalize())),
        })
    }

    /// Validate a materialized chain through the streaming constructor.
    pub fn from_chain(chain: Vec<SessionCheckpointStamp>) -> Result<Self, SessionCheckpointError> {
        Self::try_from_stamps(chain)
    }

    #[must_use]
    pub fn ancestor(&self) -> &SessionCheckpointStamp {
        &self.ancestor
    }

    #[must_use]
    pub fn descendant(&self) -> &SessionCheckpointStamp {
        &self.descendant
    }

    #[must_use]
    pub const fn edge_count(&self) -> u64 {
        self.edge_count
    }

    #[must_use]
    pub fn path_digest(&self) -> &SessionCheckpointDigest {
        &self.path_digest
    }

    #[must_use]
    pub fn proves(
        &self,
        ancestor: &SessionCheckpointStamp,
        descendant: &SessionCheckpointStamp,
    ) -> bool {
        self.ancestor() == ancestor && self.descendant() == descendant
    }
}

impl TryFrom<Vec<SessionCheckpointStamp>> for SessionCheckpointAncestryProof {
    type Error = SessionCheckpointError;

    fn try_from(value: Vec<SessionCheckpointStamp>) -> Result<Self, Self::Error> {
        Self::from_chain(value)
    }
}

fn update_ancestry_path_digest(
    hasher: &mut Sha256,
    stamp: &SessionCheckpointStamp,
) -> Result<(), SessionCheckpointError> {
    let value = serde_json::to_value(stamp)?;
    let mut canonical = Vec::new();
    write_canonical_json(&value, &mut canonical)?;
    let length = u64::try_from(canonical.len())
        .map_err(|_| SessionCheckpointError::AncestryPathElementTooLarge)?;
    hasher.update(length.to_be_bytes());
    hasher.update(canonical);
    Ok(())
}

/// Result of decoding and verifying the reserved checkpoint metadata key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCheckpointState {
    Verified(SessionCheckpointStamp),
    /// This document predates the typed stamp and must not be treated as
    /// verified absence.
    LegacyUnverified {
        legacy_runtime_checkpoint: bool,
    },
}

/// Structurally decoded checkpoint metadata from a metadata-only projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCheckpointMetadataState {
    Stamped(SessionCheckpointStamp),
    LegacyUnverified { legacy_runtime_checkpoint: bool },
}

/// Total semantic relation between two decodable checkpoint observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCheckpointRelation {
    Exact,
    LeftRevisionOlder,
    LeftRevisionNewer,
    RevisionConflict,
    LeftGenerationOlder,
    LeftGenerationNewer,
    DifferentSessionIdentity,
    DifferentLineage,
    BothLegacyUnverified,
    LeftLegacyUnverified,
    RightLegacyUnverified,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionCheckpointError {
    #[error("session checkpoint lineage must not be empty")]
    EmptyLineage,
    #[error("unsupported session checkpoint stamp schema version {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("session checkpoint revision overflow")]
    RevisionOverflow,
    #[error("invalid session checkpoint digest `{0}`")]
    InvalidDigest(String),
    #[error("checkpoint stamp session id mismatch: expected {expected}, got {actual}")]
    SessionIdMismatch {
        expected: SessionId,
        actual: SessionId,
    },
    #[error("checkpoint stamp digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch {
        expected: SessionCheckpointDigest,
        actual: SessionCheckpointDigest,
    },
    #[error(
        "transcript-history checkpoint witness mismatch: carried {carried}, computed {computed}"
    )]
    TranscriptHistoryWitnessMismatch {
        carried: SessionCheckpointDigest,
        computed: SessionCheckpointDigest,
    },
    #[error(
        "legacy migration source BLOB semantic digest mismatch: expected {expected}, got {actual}"
    )]
    LegacySourceBlobMismatch {
        expected: SessionCheckpointDigest,
        actual: SessionCheckpointDigest,
    },
    #[error("malformed legacy checkpoint provenance: expected boolean")]
    MalformedLegacyProvenance,
    #[error("legacy checkpoint provenance is unverified; explicit migration is required")]
    LegacyCheckpointUnverified,
    #[error("legacy checkpoint provenance cannot mutate a typed checkpoint")]
    LegacyProvenanceMutationOnTypedCheckpoint,
    #[error("session checkpoint ancestry proof must contain at least one stamp")]
    EmptyAncestryProof,
    #[error("session checkpoint ancestry edge count overflow")]
    AncestryEdgeCountOverflow,
    #[error("session checkpoint ancestry path element is too large")]
    AncestryPathElementTooLarge,
    #[error(
        "checkpoint ancestry stamp {index} has session {actual}, expected exact session {expected}"
    )]
    AncestrySessionMismatch {
        index: u64,
        expected: SessionId,
        actual: SessionId,
    },
    #[error(
        "checkpoint ancestry stamp {index} has lineage {actual}, expected exact lineage {expected}"
    )]
    AncestryLineageMismatch {
        index: u64,
        expected: SessionLineageId,
        actual: SessionLineageId,
    },
    #[error(
        "checkpoint ancestry stamp {index} has generation {actual}, expected generation {expected}"
    )]
    AncestryGenerationMismatch {
        index: u64,
        expected: u64,
        actual: u64,
    },
    #[error(
        "checkpoint ancestry stamp {index} revision {actual} is not newer than previous revision {previous}"
    )]
    AncestryRevisionNotIncreasing {
        index: u64,
        previous: u64,
        actual: u64,
    },
    #[error("checkpoint ancestry stamp {index} does not name the exact previous authority base")]
    AncestryAuthorityBaseMismatch { index: u64 },
    #[error("checkpoint authority-base conflict: {0}")]
    AuthorityBaseConflict(String),
    #[error("session checkpoint serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Decode checkpoint metadata without laundering malformed facts into absence.
///
/// `Stamped` validates the schema and enclosing session identity. The full
/// document digest is verified only by [`Session::try_checkpoint_state`].
pub fn session_checkpoint_metadata_state(
    session_id: &SessionId,
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> Result<SessionCheckpointMetadataState, SessionCheckpointError> {
    let legacy_runtime_checkpoint = match metadata.get(SESSION_RUNTIME_CHECKPOINT_PROVENANCE_KEY) {
        Some(value) => value
            .as_bool()
            .ok_or(SessionCheckpointError::MalformedLegacyProvenance)?,
        None => false,
    };
    let Some(value) = metadata.get(SESSION_CHECKPOINT_STAMP_KEY) else {
        return Ok(SessionCheckpointMetadataState::LegacyUnverified {
            legacy_runtime_checkpoint,
        });
    };
    let stamp = serde_json::from_value::<SessionCheckpointStamp>(value.clone())?;
    stamp.validate_for_session(session_id)?;
    Ok(SessionCheckpointMetadataState::Stamped(stamp))
}

/// Compute the pinned canonical checkpoint digest.
///
/// Canonicalization uses recursive lexicographic object-key ordering, stable
/// array ordering, and serde_json scalar spelling. The typed stamp and legacy
/// compatibility key are removed first, so the digest is neither
/// self-referential nor tied to ownership-era duplicate metadata.
pub fn session_checkpoint_digest(
    session: &Session,
) -> Result<SessionCheckpointDigest, SessionCheckpointError> {
    let history_digest = session_transcript_history_checkpoint_digest(session)?;
    let mut document = session.checkpoint_digest_document()?;
    if let Some(metadata) = document
        .as_object_mut()
        .and_then(|session| session.get_mut("metadata"))
        .and_then(serde_json::Value::as_object_mut)
    {
        metadata.remove(SESSION_CHECKPOINT_STAMP_KEY);
        metadata.remove(SESSION_RUNTIME_CHECKPOINT_PROVENANCE_KEY);
        metadata.remove(SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
        metadata.remove(SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY);
        if let Some(digest) = history_digest {
            metadata.insert(
                SESSION_TRANSCRIPT_HISTORY_STATE_KEY.to_string(),
                checkpoint_history_digest_marker(&digest),
            );
        }
    }
    canonical_value_digest(&document)
}

/// Resolve the storage-invariant transcript-history witness carried by a
/// session document.
///
/// Full documents derive it from the canonical retained graph. Incremental
/// projections carry the same digest under a reserved metadata key because
/// their revision bodies live out of line. If both representations are
/// present they must agree exactly; malformed or contradictory evidence is
/// never treated as absence.
pub fn session_transcript_history_checkpoint_digest(
    session: &Session,
) -> Result<Option<SessionCheckpointDigest>, SessionCheckpointError> {
    let carried = session
        .metadata()
        .get(SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY)
        .cloned()
        .map(serde_json::from_value::<SessionCheckpointDigest>)
        .transpose()?;
    let computed = session
        .metadata()
        .get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
        .map(session_checkpoint_history_digest)
        .transpose()?;
    match (carried, computed) {
        (Some(carried), Some(computed)) if carried != computed => {
            Err(SessionCheckpointError::TranscriptHistoryWitnessMismatch { carried, computed })
        }
        (Some(carried), Some(_) | None) => Ok(Some(carried)),
        (None, Some(computed)) => Ok(Some(computed)),
        (None, None) => Ok(None),
    }
}

/// Exact byte digest of a legacy source BLOB used only as migration custody.
#[must_use]
pub fn legacy_session_source_blob_digest(source_blob: &[u8]) -> SessionCheckpointDigest {
    SessionCheckpointDigest(format!("sha256:{:x}", Sha256::digest(source_blob)))
}

fn session_checkpoint_history_digest(
    history: &serde_json::Value,
) -> Result<SessionCheckpointDigest, SessionCheckpointError> {
    let history = crate::session::canonicalize_checkpoint_history_value(history)?;
    canonical_value_digest(&history)
}

/// Compute the storage-invariant witness for a reconstructed transcript
/// history graph.
pub fn transcript_history_checkpoint_digest(
    history: &crate::TranscriptHistoryState,
) -> Result<SessionCheckpointDigest, SessionCheckpointError> {
    let value = serde_json::to_value(history)?;
    session_checkpoint_history_digest(&value)
}

fn canonical_value_digest(
    value: &serde_json::Value,
) -> Result<SessionCheckpointDigest, SessionCheckpointError> {
    let mut canonical = Vec::new();
    write_canonical_json(value, &mut canonical)?;
    Ok(SessionCheckpointDigest(format!(
        "sha256:{:x}",
        Sha256::digest(canonical)
    )))
}

fn checkpoint_history_digest_marker(digest: &SessionCheckpointDigest) -> serde_json::Value {
    serde_json::json!({
        "semantic_checkpoint_history_digest_v1": digest.as_str(),
    })
}

fn write_canonical_json(
    value: &serde_json::Value,
    output: &mut Vec<u8>,
) -> Result<(), serde_json::Error> {
    match value {
        serde_json::Value::Null => output.extend_from_slice(b"null"),
        serde_json::Value::Bool(value) => {
            output.extend_from_slice(if *value { b"true" } else { b"false" });
        }
        serde_json::Value::Number(value) => output.extend_from_slice(value.to_string().as_bytes()),
        serde_json::Value::String(value) => {
            output.extend_from_slice(serde_json::to_string(value)?.as_bytes());
        }
        serde_json::Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        serde_json::Value::Object(values) => {
            output.push(b'{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                output.extend_from_slice(serde_json::to_string(key)?.as_bytes());
                output.push(b':');
                write_canonical_json(value, output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

/// Classify two session checkpoint observations after independently verifying
/// each document's digest and stamp.
pub fn session_checkpoint_relation(
    left: &Session,
    right: &Session,
) -> Result<SessionCheckpointRelation, SessionCheckpointError> {
    let left = left.try_checkpoint_state()?;
    let right = right.try_checkpoint_state()?;
    let (left, right) = match (left, right) {
        (SessionCheckpointState::Verified(left), SessionCheckpointState::Verified(right)) => {
            (left, right)
        }
        (
            SessionCheckpointState::LegacyUnverified { .. },
            SessionCheckpointState::LegacyUnverified { .. },
        ) => return Ok(SessionCheckpointRelation::BothLegacyUnverified),
        (SessionCheckpointState::LegacyUnverified { .. }, SessionCheckpointState::Verified(_)) => {
            return Ok(SessionCheckpointRelation::LeftLegacyUnverified);
        }
        (SessionCheckpointState::Verified(_), SessionCheckpointState::LegacyUnverified { .. }) => {
            return Ok(SessionCheckpointRelation::RightLegacyUnverified);
        }
    };
    if left.session_id != right.session_id {
        return Ok(SessionCheckpointRelation::DifferentSessionIdentity);
    }
    if left.lineage_id != right.lineage_id {
        return Ok(SessionCheckpointRelation::DifferentLineage);
    }
    if left.generation < right.generation {
        return Ok(SessionCheckpointRelation::LeftGenerationOlder);
    }
    if left.generation > right.generation {
        return Ok(SessionCheckpointRelation::LeftGenerationNewer);
    }
    if left.checkpoint_revision < right.checkpoint_revision {
        return Ok(SessionCheckpointRelation::LeftRevisionOlder);
    }
    if left.checkpoint_revision > right.checkpoint_revision {
        return Ok(SessionCheckpointRelation::LeftRevisionNewer);
    }
    if left == right {
        Ok(SessionCheckpointRelation::Exact)
    } else {
        Ok(SessionCheckpointRelation::RevisionConflict)
    }
}

/// Whether two verified documents name the exact same semantic checkpoint.
/// Raw byte identity is deliberately not part of this predicate.
pub fn session_checkpoints_are_exact(
    left: &Session,
    right: &Session,
) -> Result<bool, SessionCheckpointError> {
    Ok(session_checkpoint_relation(left, right)? == SessionCheckpointRelation::Exact)
}

/// Periodic session persistence hook.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionCheckpointer: Send + Sync {
    /// Save a snapshot of the current session state.
    async fn checkpoint(&self, session: &Session);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{Message, UserMessage};

    fn session_with_text(text: &str) -> Session {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text(text.to_string())));
        session
    }

    fn install_stamp(session: &Session, stamp: &SessionCheckpointStamp) -> Session {
        let mut document = serde_json::to_value(session).expect("serialize session");
        document["metadata"][SESSION_CHECKPOINT_STAMP_KEY] =
            serde_json::to_value(stamp).expect("serialize stamp");
        serde_json::from_value(document).expect("deserialize stamped session")
    }

    fn root_stamp(session: &Session) -> SessionCheckpointStamp {
        SessionCheckpointStamp::new(
            session.id().clone(),
            SessionLineageId::for_session(session.id()),
            SessionGeneration::INITIAL,
            SessionCheckpointRevision::INITIAL,
            SessionCheckpointAuthorityBase::Absent,
            session_checkpoint_digest(session).expect("digest"),
            SessionCheckpointProvenance::SessionCreated,
        )
    }

    fn stamped_root(session: &Session) -> Session {
        let stamp = root_stamp(session);
        stamp
            .validate_for_session(session.id())
            .expect("valid root");
        install_stamp(session, &stamp)
    }

    fn verified_stamp(session: &Session) -> SessionCheckpointStamp {
        match session.try_checkpoint_state().expect("checkpoint state") {
            SessionCheckpointState::Verified(stamp) => stamp,
            SessionCheckpointState::LegacyUnverified { .. } => {
                panic!("expected verified checkpoint")
            }
        }
    }

    fn successor_stamp(
        session: &Session,
        prior: &SessionCheckpointStamp,
        provenance: SessionCheckpointProvenance,
    ) -> SessionCheckpointStamp {
        SessionCheckpointStamp::new(
            session.id().clone(),
            prior.lineage_id().clone(),
            prior.generation(),
            prior
                .checkpoint_revision()
                .checked_next()
                .expect("next revision"),
            SessionCheckpointAuthorityBase::Typed {
                anchor: SessionCheckpointAnchor::from_stamp(prior),
            },
            session_checkpoint_digest(session).expect("digest"),
            provenance,
        )
    }

    fn advance_checkpoint(
        session: &Session,
        prior: &SessionCheckpointStamp,
        text: &str,
    ) -> (Session, SessionCheckpointStamp) {
        let mut candidate = session.clone();
        candidate.push(Message::User(UserMessage::text(text.to_string())));
        let stamp = successor_stamp(
            &candidate,
            prior,
            SessionCheckpointProvenance::RunBoundaryCommit,
        );
        stamp
            .validate_for_session(candidate.id())
            .expect("valid successor");
        (install_stamp(&candidate, &stamp), stamp)
    }

    #[test]
    fn checkpoint_stamp_round_trips_without_ownership_atoms() {
        let session = stamped_root(&session_with_text("hello"));
        let stamp = verified_stamp(&session);
        let encoded = serde_json::to_vec(&session).expect("serialize");
        let decoded: Session = serde_json::from_slice(&encoded).expect("deserialize");
        assert_eq!(verified_stamp(&decoded), stamp);
        assert_eq!(
            session_checkpoint_relation(&session, &decoded).expect("relation"),
            SessionCheckpointRelation::Exact
        );

        let encoded_stamp = serde_json::to_string(&stamp).expect("stamp json");
        for forbidden in ["epoch", "lease", "fence", "runtime_id", "incarnation"] {
            assert!(
                !encoded_stamp.contains(forbidden),
                "checkpoint content identity must exclude {forbidden}: {encoded_stamp}"
            );
        }
    }

    #[test]
    fn canonical_digest_is_ordered_and_excludes_only_checkpoint_metadata() {
        let left = serde_json::json!({"outer": {"b": 2, "a": 1}});
        let right = serde_json::json!({"outer": {"a": 1, "b": 2}});
        assert_eq!(
            canonical_value_digest(&left).expect("left"),
            canonical_value_digest(&right).expect("right")
        );

        let legacy = session_with_text("digest");
        let before = session_checkpoint_digest(&legacy).expect("digest");
        let stamped = stamped_root(&legacy);
        assert_eq!(session_checkpoint_digest(&stamped).expect("digest"), before);

        let mut document = serde_json::to_value(&stamped).expect("serialize");
        document["metadata"][SESSION_RUNTIME_CHECKPOINT_PROVENANCE_KEY] =
            serde_json::Value::Bool(true);
        let with_legacy_fact: Session = serde_json::from_value(document).expect("deserialize");
        assert_eq!(
            session_checkpoint_digest(&with_legacy_fact).expect("digest"),
            before
        );

        let mut changed = stamped;
        changed.set_metadata("caller_fact", serde_json::json!({"b": 2, "a": 1}));
        assert_ne!(session_checkpoint_digest(&changed).expect("digest"), before);
    }

    #[test]
    fn malformed_present_stamp_and_legacy_fact_are_errors_not_absence() {
        let legacy = session_with_text("legacy");
        assert_eq!(
            legacy.try_checkpoint_state().expect("legacy state"),
            SessionCheckpointState::LegacyUnverified {
                legacy_runtime_checkpoint: false
            }
        );

        let mut malformed = serde_json::to_value(&legacy).expect("serialize");
        malformed["metadata"][SESSION_CHECKPOINT_STAMP_KEY] =
            serde_json::json!({"schema_version": 1});
        let malformed: Session = serde_json::from_value(malformed).expect("session envelope");
        assert!(matches!(
            malformed.try_checkpoint_state(),
            Err(SessionCheckpointError::Serialization(_))
        ));

        let mut malformed_legacy = serde_json::to_value(&legacy).expect("serialize");
        malformed_legacy["metadata"][SESSION_RUNTIME_CHECKPOINT_PROVENANCE_KEY] =
            serde_json::json!("yes");
        let malformed_legacy: Session =
            serde_json::from_value(malformed_legacy).expect("session envelope");
        assert!(matches!(
            malformed_legacy.try_checkpoint_state(),
            Err(SessionCheckpointError::MalformedLegacyProvenance)
        ));

        let mut mutated = stamped_root(&session_with_text("before"));
        mutated.push(Message::User(UserMessage::text("after".to_string())));
        assert!(matches!(
            mutated.try_checkpoint_state(),
            Err(SessionCheckpointError::DigestMismatch { .. })
        ));
    }

    #[test]
    #[allow(deprecated)]
    fn typed_provenance_is_authoritative_and_legacy_mutators_refuse_it() {
        let legacy = session_with_text("legacy provenance");
        assert!(matches!(
            legacy.try_has_runtime_checkpoint_provenance(),
            Err(SessionCheckpointError::LegacyCheckpointUnverified)
        ));

        let root = stamped_root(&session_with_text("typed provenance"));
        assert!(
            !root
                .try_has_runtime_checkpoint_provenance()
                .expect("typed root provenance")
        );
        let root_stamp = verified_stamp(&root);
        let mut checkpoint = root;
        checkpoint.push(Message::User(UserMessage::text("intra-turn".to_string())));
        let checkpoint_stamp = SessionCheckpointStamp::successor(
            &checkpoint,
            &root_stamp,
            SessionCheckpointProvenance::IntraTurnCheckpoint,
        )
        .expect("intra-turn stamp");
        checkpoint
            .install_checkpoint_stamp(checkpoint_stamp.clone())
            .expect("install intra-turn stamp");
        assert!(
            checkpoint
                .try_has_runtime_checkpoint_provenance()
                .expect("typed intra-turn provenance")
        );

        assert!(matches!(
            checkpoint.clear_runtime_checkpoint_provenance(),
            Err(SessionCheckpointError::LegacyProvenanceMutationOnTypedCheckpoint)
        ));
        assert!(matches!(
            checkpoint.set_runtime_checkpoint_provenance(),
            Err(SessionCheckpointError::LegacyProvenanceMutationOnTypedCheckpoint)
        ));
        assert_eq!(verified_stamp(&checkpoint), checkpoint_stamp);
    }

    #[test]
    fn intra_turn_projection_replacement_remains_a_sibling_of_committed_authority() {
        let root = stamped_root(&session_with_text("committed"));
        let root_stamp = verified_stamp(&root);

        let mut first = root.clone();
        first.push(Message::User(UserMessage::text(
            "first projection".to_string(),
        )));
        let first_stamp = SessionCheckpointStamp::intra_turn_projection(&first, &root_stamp)
            .expect("first projection stamp");
        first
            .install_checkpoint_stamp(first_stamp.clone())
            .expect("install first projection stamp");

        let mut replacement = root;
        replacement.push(Message::User(UserMessage::text(
            "replacement projection".to_string(),
        )));
        let replacement_stamp =
            SessionCheckpointStamp::intra_turn_projection(&replacement, &first_stamp)
                .expect("replacement projection stamp");
        replacement
            .install_checkpoint_stamp(replacement_stamp.clone())
            .expect("install replacement projection stamp");

        assert_eq!(
            first_stamp.checkpoint_revision(),
            replacement_stamp.checkpoint_revision()
        );
        assert_eq!(
            first_stamp.authority_base(),
            replacement_stamp.authority_base()
        );
        assert!(matches!(
            replacement.try_checkpoint_state(),
            Ok(SessionCheckpointState::Verified(stamp)) if stamp == replacement_stamp
        ));
    }

    #[test]
    fn relation_classifies_lineage_revision_and_conflict() {
        let root = stamped_root(&session_with_text("base"));
        let root_stamp = verified_stamp(&root);

        let mut advanced_document = root.clone();
        advanced_document.push(Message::User(UserMessage::text("next".to_string())));
        let advanced_stamp = successor_stamp(
            &advanced_document,
            &root_stamp,
            SessionCheckpointProvenance::RunBoundaryCommit,
        );
        advanced_stamp
            .validate_for_session(advanced_document.id())
            .expect("valid successor");
        let advanced = install_stamp(&advanced_document, &advanced_stamp);
        assert_eq!(
            session_checkpoint_relation(&root, &advanced).expect("relation"),
            SessionCheckpointRelation::LeftRevisionOlder
        );

        let conflict_stamp = SessionCheckpointStamp::new(
            advanced_stamp.session_id().clone(),
            advanced_stamp.lineage_id().clone(),
            advanced_stamp.generation(),
            advanced_stamp.checkpoint_revision(),
            advanced_stamp.authority_base().clone(),
            advanced_stamp.digest().clone(),
            SessionCheckpointProvenance::TranscriptRewrite,
        );
        conflict_stamp
            .validate_for_session(advanced.id())
            .expect("valid sibling");
        let conflict = install_stamp(&advanced, &conflict_stamp);
        assert_eq!(
            session_checkpoint_relation(&advanced, &conflict).expect("relation"),
            SessionCheckpointRelation::RevisionConflict
        );

        let different_lineage_stamp = SessionCheckpointStamp::new(
            root_stamp.session_id().clone(),
            SessionLineageId::new("session:other").expect("lineage"),
            SessionGeneration::INITIAL,
            SessionCheckpointRevision::INITIAL,
            SessionCheckpointAuthorityBase::Absent,
            root_stamp.digest().clone(),
            SessionCheckpointProvenance::Forked,
        );
        let different_lineage = install_stamp(&root, &different_lineage_stamp);
        assert_eq!(
            session_checkpoint_relation(&root, &different_lineage).expect("relation"),
            SessionCheckpointRelation::DifferentLineage
        );
    }

    #[test]
    fn ancestry_proof_requires_every_exact_authority_link() {
        let root = stamped_root(&session_with_text("r0"));
        let r0 = verified_stamp(&root);
        let (session_r1, r1) = advance_checkpoint(&root, &r0, "r1");
        let (session_r2, r2) = advance_checkpoint(&session_r1, &r1, "r2");
        let (_session_r3, r3) = advance_checkpoint(&session_r2, &r2, "r3");

        let proof = SessionCheckpointAncestryProof::from_chain(vec![
            r0.clone(),
            r1.clone(),
            r2.clone(),
            r3.clone(),
        ])
        .expect("complete exact chain");
        assert!(proof.proves(&r0, &r3));
        assert_eq!(proof.edge_count(), 3);

        assert!(matches!(
            SessionCheckpointAncestryProof::from_chain(vec![r0.clone(), r2]),
            Err(SessionCheckpointError::AncestryAuthorityBaseMismatch { index: 1 })
        ));

        let mut sibling_document = session_r1;
        sibling_document.push(Message::User(UserMessage::text("sibling-r2".to_string())));
        let sibling_r2 = successor_stamp(
            &sibling_document,
            &r1,
            SessionCheckpointProvenance::TranscriptRewrite,
        );
        sibling_r2
            .validate_for_session(sibling_document.id())
            .expect("valid sibling");
        assert!(matches!(
            SessionCheckpointAncestryProof::from_chain(vec![r0, r1, sibling_r2, r3]),
            Err(SessionCheckpointError::AncestryAuthorityBaseMismatch { index: 3 })
        ));
    }

    #[test]
    fn ancestry_proof_streams_more_than_1024_exact_links() {
        let session = stamped_root(&session_with_text("long ancestry"));
        let root = verified_stamp(&session);
        let chain = std::iter::successors(Some(root.clone()), |prior| {
            Some(
                SessionCheckpointStamp::successor(
                    &session,
                    prior,
                    SessionCheckpointProvenance::RunBoundaryCommit,
                )
                .expect("exact successor"),
            )
        })
        .take(1_501);

        let proof =
            SessionCheckpointAncestryProof::try_from_stamps(chain).expect("streaming proof");
        assert_eq!(proof.ancestor(), &root);
        assert_eq!(proof.edge_count(), 1_500);
        assert_eq!(
            proof.descendant().checkpoint_revision().get(),
            root.checkpoint_revision().get() + 1_500
        );
        assert!(proof.path_digest().as_str().starts_with("sha256:"));
    }

    #[test]
    fn metadata_only_decode_validates_identity_without_claiming_digest_verification() {
        let session = stamped_root(&session_with_text("metadata"));
        let encoded = serde_json::to_vec(&session).expect("serialize");
        let metadata = crate::session_metadata_document_from_slice(&encoded).expect("metadata");
        assert_eq!(
            metadata
                .try_checkpoint_metadata_state()
                .expect("metadata checkpoint"),
            SessionCheckpointMetadataState::Stamped(verified_stamp(&session))
        );

        let mut document = serde_json::to_value(&session).expect("serialize");
        document["metadata"][SESSION_CHECKPOINT_STAMP_KEY]["session_id"] =
            serde_json::to_value(SessionId::new()).expect("session id");
        let encoded = serde_json::to_vec(&document).expect("encode document");
        let metadata = crate::session_metadata_document_from_slice(&encoded).expect("metadata");
        assert!(matches!(
            metadata.try_checkpoint_metadata_state(),
            Err(SessionCheckpointError::SessionIdMismatch { .. })
        ));
    }

    #[test]
    fn checked_revision_never_wraps() {
        assert!(
            SessionCheckpointRevision::new(u64::MAX)
                .checked_next()
                .is_err()
        );
    }

    #[test]
    fn coherent_nonzero_legacy_cursor_migrates_and_missing_stays_unverified() {
        let legacy = session_with_text("legacy nonzero");
        let source_blob = serde_json::to_vec(&legacy).expect("legacy source BLOB");
        let stamp = SessionCheckpointStamp::recovery_migration(
            &legacy,
            &source_blob,
            SessionGeneration::new(3),
            SessionCheckpointRevision::new(17),
        )
        .expect("coherent nonzero migration");
        assert_eq!(stamp.generation(), SessionGeneration::new(3));
        assert_eq!(
            stamp.checkpoint_revision(),
            SessionCheckpointRevision::new(17)
        );
        assert!(matches!(
            stamp.authority_base(),
            SessionCheckpointAuthorityBase::Legacy {
                observed_generation,
                observed_checkpoint_revision,
                ..
            } if *observed_generation == SessionGeneration::new(3)
                && *observed_checkpoint_revision == SessionCheckpointRevision::new(17)
        ));
        let mut migrated = legacy;
        migrated
            .install_checkpoint_stamp(stamp.clone())
            .expect("install migration");
        assert_eq!(
            migrated.try_checkpoint_state().expect("verified migration"),
            SessionCheckpointState::Verified(stamp)
        );

        let missing = Session::new();
        assert_eq!(
            missing.try_checkpoint_state().expect("missing state"),
            SessionCheckpointState::LegacyUnverified {
                legacy_runtime_checkpoint: false,
            }
        );
    }

    #[test]
    fn legacy_migration_custody_distinguishes_byte_different_equal_documents() {
        let legacy = session_with_text("legacy custody");
        let compact = serde_json::to_vec(&legacy).expect("compact legacy source");
        let pretty = serde_json::to_vec_pretty(&legacy).expect("pretty legacy source");
        assert_ne!(compact, pretty);

        let compact_stamp = SessionCheckpointStamp::recovery_migration(
            &legacy,
            &compact,
            SessionGeneration::new(4),
            SessionCheckpointRevision::new(19),
        )
        .expect("compact migration");
        let pretty_stamp = SessionCheckpointStamp::recovery_migration(
            &legacy,
            &pretty,
            SessionGeneration::new(4),
            SessionCheckpointRevision::new(19),
        )
        .expect("pretty migration");

        assert_eq!(compact_stamp.digest(), pretty_stamp.digest());
        let SessionCheckpointAuthorityBase::Legacy {
            source_blob_digest: compact_source_digest,
            ..
        } = compact_stamp.authority_base()
        else {
            panic!("expected compact legacy authority base");
        };
        let SessionCheckpointAuthorityBase::Legacy {
            source_blob_digest: pretty_source_digest,
            ..
        } = pretty_stamp.authority_base()
        else {
            panic!("expected pretty legacy authority base");
        };
        assert_ne!(compact_source_digest, pretty_source_digest);
        assert_eq!(
            compact_source_digest,
            &legacy_session_source_blob_digest(&compact)
        );
        assert_eq!(
            pretty_source_digest,
            &legacy_session_source_blob_digest(&pretty)
        );
    }

    #[test]
    fn production_stamp_constructors_require_exact_successors_and_refresh_after_mutation() {
        let mut session = session_with_text("root");
        let root =
            SessionCheckpointStamp::root(&session, SessionCheckpointProvenance::SessionCreated)
                .expect("root");
        session
            .install_checkpoint_stamp(root.clone())
            .expect("install root");
        assert_eq!(verified_stamp(&session), root);

        session.push(Message::User(UserMessage::text("next".to_string())));
        assert!(matches!(
            session.try_checkpoint_state(),
            Err(SessionCheckpointError::DigestMismatch { .. })
        ));
        let successor = SessionCheckpointStamp::successor(
            &session,
            &root,
            SessionCheckpointProvenance::RunBoundaryCommit,
        )
        .expect("successor");
        assert_eq!(
            successor.checkpoint_revision(),
            root.checkpoint_revision().checked_next().expect("next")
        );
        session
            .install_checkpoint_stamp(successor.clone())
            .expect("install successor");
        assert_eq!(verified_stamp(&session), successor);

        let gap = SessionCheckpointStamp::new(
            successor.session_id().clone(),
            successor.lineage_id().clone(),
            successor.generation(),
            SessionCheckpointRevision::new(successor.checkpoint_revision().get() + 2),
            SessionCheckpointAuthorityBase::Typed {
                anchor: SessionCheckpointAnchor::from_stamp(&successor),
            },
            successor.digest().clone(),
            SessionCheckpointProvenance::RunBoundaryCommit,
        );
        assert!(matches!(
            gap.validate_for_session(session.id()),
            Err(SessionCheckpointError::AuthorityBaseConflict(_))
        ));
    }

    #[test]
    fn checkpoint_digest_erases_transcript_construction_timestamps() {
        let session = session_with_text("same semantic message");
        let mut reconstructed = session.clone();
        let messages = std::sync::Arc::make_mut(&mut reconstructed.messages);
        let Some(Message::User(user)) = messages.first_mut() else {
            panic!("expected user message");
        };
        user.created_at = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH;
        user.identity.run_id = Some(crate::RunId::new());
        assert_eq!(
            session_checkpoint_digest(&session).expect("original digest"),
            session_checkpoint_digest(&reconstructed).expect("reconstructed digest")
        );
    }
}
