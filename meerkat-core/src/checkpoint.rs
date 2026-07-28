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
use std::cell::Cell;
use std::fmt;

/// Base durable schema for [`SessionCheckpointStamp`]: the original (v1)
/// provenance vocabulary.
pub const SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION: u32 = 1;
/// Extended durable schema (v2): identical shape, provenance vocabulary
/// extended with the recovered-boundary variants
/// ([`SessionCheckpointProvenance::RecoveredRunBoundaryCommit`],
/// [`SessionCheckpointProvenance::RecoveredInterruptedBoundary`], and — as
/// of the legacy-tail adoption era —
/// [`SessionCheckpointProvenance::RecoveredLegacyBoundaryCommit`]; binaries
/// that know v2 but predate a variant refuse it as an unknown-variant
/// decode error, the same one-way door as the original v2 rollout).
///
/// Version selection is PER RECORD: a stamp whose provenance fits the v1
/// vocabulary is still written as v1, so ordinary sessions stay readable by
/// older binaries after a downgrade; only recovered stamps advertise v2 —
/// and only those refuse (typed, on this binary; as an unknown-variant
/// decode error on binaries predating the vocabulary) instead of silently
/// reinterpreting a fact they do not know.
pub const SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_RECOVERED: u32 = 2;
/// Extended durable schema (v3): identical shape, minted whenever the
/// document's canonical digest folds a FORMAT-3 transcript-history witness
/// (the revision-identity computation). The schema bump is the downgrade
/// one-way door: a pre-v3 binary refuses the stamp through its existing
/// typed future-schema path instead of recomputing a v2 witness, reading a
/// digest mismatch, and failing the whole document with a misleading error.
///
/// Version selection stays PER RECORD and is
/// `max(provenance-required, witness-format-required)`: a session with no
/// transcript graph (or v2 witness evidence) keeps minting the lowest
/// schema its provenance allows, so plain sessions stay readable by older
/// binaries after a downgrade.
pub const SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3: u32 = 3;

/// The schema version a stamp with this provenance must advertise, before
/// the witness-format axis is applied.
fn required_stamp_schema_version(provenance: SessionCheckpointProvenance) -> u32 {
    match provenance {
        SessionCheckpointProvenance::SessionCreated
        | SessionCheckpointProvenance::Forked
        | SessionCheckpointProvenance::IntraTurnCheckpoint
        | SessionCheckpointProvenance::RunBoundaryCommit
        | SessionCheckpointProvenance::TranscriptRewrite
        | SessionCheckpointProvenance::RecoveryMigration => SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION,
        SessionCheckpointProvenance::RecoveredRunBoundaryCommit
        | SessionCheckpointProvenance::RecoveredInterruptedBoundary
        | SessionCheckpointProvenance::RecoveredLegacyBoundaryCommit => {
            SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_RECOVERED
        }
    }
}

/// The schema floor a stamp minted over this witness format must advertise.
fn required_stamp_schema_version_for_witness(witness_format: Option<u32>) -> u32 {
    match witness_format {
        Some(format)
            if format
                >= crate::generated::session_persistence_version_authority::TRANSCRIPT_HISTORY_WITNESS_FORMAT =>
        {
            SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3
        }
        _ => SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION,
    }
}

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
    /// A machine-authorized recovery commit of a COMPLETED durable tail whose
    /// original run-boundary commit never landed (lost to a shutdown race).
    /// Anchored to the last committed runtime snapshot — never to the
    /// intra-turn projection, which remains forbidden as an authority base.
    RecoveredRunBoundaryCommit,
    /// A machine-authorized recovery commit that CLOSED an interrupted
    /// durable tail: content preserved, provider-invalid structure repaired
    /// (synthetic interrupted tool results, typed recovery notice), the
    /// original run terminalized as interrupted — never requeued.
    RecoveredInterruptedBoundary,
    /// A machine-authorized adoption of a COMPLETED durable tail written by
    /// a pre-run-identity legacy writer: digest-proven strict continuation,
    /// zero run identity anywhere in the tail (the bookkeeping did not exist
    /// when it was written), pre-witness-v3 stamp evidence, clean EndTurn
    /// shape. Committed under a domain-separated deterministic legacy run
    /// identity; anchored to the last committed runtime snapshot like every
    /// recovered boundary. Distinct from
    /// [`SessionCheckpointProvenance::RecoveredRunBoundaryCommit`] so the
    /// stamp never claims a modern run's boundary was recovered.
    RecoveredLegacyBoundaryCommit,
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
        digest: MintedCheckpointDigest,
        provenance: SessionCheckpointProvenance,
    ) -> Self {
        Self {
            schema_version: required_stamp_schema_version(provenance).max(
                required_stamp_schema_version_for_witness(digest.witness_format),
            ),
            session_id,
            lineage_id,
            generation,
            checkpoint_revision,
            authority_base,
            digest: digest.digest,
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
            session_checkpoint_digest_for_mint(session)?,
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
        // Evidence-format digests on BOTH sides: the equality check compares
        // the migrated document against its legacy source, so the witness
        // format must be the one those legacy documents declare, never a
        // mint-current upgrade (which would manufacture a mismatch).
        let digest = session_checkpoint_digest_selected(session, WitnessSelection::Evidence)?;
        let source_digest = session_checkpoint_digest(&source_session)?;
        if source_digest != digest.digest {
            return Err(SessionCheckpointError::LegacySourceBlobMismatch {
                expected: digest.digest,
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
                | SessionCheckpointProvenance::RecoveredRunBoundaryCommit
                | SessionCheckpointProvenance::RecoveredInterruptedBoundary
                | SessionCheckpointProvenance::RecoveredLegacyBoundaryCommit
        ) {
            return Err(SessionCheckpointError::AuthorityBaseConflict(
                "checkpoint successor provenance must be checkpoint, boundary, rewrite, \
                 or a recovered boundary"
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
            session_checkpoint_digest_for_mint(session)?,
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
            session_checkpoint_digest_for_mint(session)?,
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
        let digest = MintedCheckpointDigest {
            digest,
            witness_format: None,
        };
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
        // A version newer than this binary's vocabulary is a typed
        // future-schema refusal; a version that does not match the record's
        // own provenance vocabulary (e.g. a recovered provenance claiming
        // the v1 schema) is a mis-advertised record and refuses the same
        // way rather than letting the durable record lie about which readers
        // can decode it. The two legal versions per record are the
        // provenance floor and — when the document's canonical digest folds
        // a format-3 history witness — the witness-v3 schema; a stamp whose
        // schema mislabels the witness axis fails closed downstream at the
        // digest comparison (the digest values differ per witness format),
        // so this check owns vocabulary honesty, not witness binding.
        let provenance_floor = required_stamp_schema_version(self.provenance);
        let witness_v3 = provenance_floor.max(SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3);
        if self.schema_version != provenance_floor && self.schema_version != witness_v3 {
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
                        | SessionCheckpointProvenance::RecoveredRunBoundaryCommit
                        | SessionCheckpointProvenance::RecoveredInterruptedBoundary
                        | SessionCheckpointProvenance::RecoveredLegacyBoundaryCommit
                ) {
                    return Err(SessionCheckpointError::AuthorityBaseConflict(
                        "typed authority base requires checkpoint, boundary, rewrite, or \
                         recovered-boundary provenance"
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
        "unsupported transcript-history witness format {0}: this binary predates the format; \
         refusing before any normalization or healing of the row"
    )]
    UnsupportedTranscriptHistoryWitnessFormat(u32),
    #[error("unsupported transcript-history revision digest format {0}")]
    UnsupportedTranscriptHistoryRevisionDigestFormat(u32),
    #[error("malformed transcript-history witness carrier: {0}")]
    MalformedTranscriptHistoryWitness(String),
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

thread_local! {
    /// Per-thread count of full content-digest computations
    /// (canonical-JSON serialization + SHA-256 over session content).
    /// Structural regression tests assert a zero delta across steady-state
    /// reads; a thread-local counter keeps that assertion immune to
    /// unrelated tests on other threads. One `Cell` bump per multi-KB hash
    /// pass is noise in release builds.
    static CONTENT_DIGEST_COMPUTATIONS: Cell<u64> = const { Cell::new(0) };
}

/// Per-thread count of session content-digest computations. Observability
/// seam for structural no-rehash regression tests only; not a public API.
#[doc(hidden)]
#[must_use]
pub fn session_content_digest_computations() -> u64 {
    CONTENT_DIGEST_COMPUTATIONS.with(Cell::get)
}

pub(crate) fn record_content_digest_computation() {
    #[cfg(any(test, debug_assertions))]
    if DIGEST_ACCOUNTING_SUPPRESSED.with(Cell::get) {
        return;
    }
    CONTENT_DIGEST_COMPUTATIONS.with(|count| count.set(count.get().saturating_add(1)));
}

#[cfg(any(test, debug_assertions))]
thread_local! {
    /// Debug/test-only: digest accounting suppressed on this thread while a
    /// verification cross-check runs, so the budget regression tests keep
    /// measuring the production path instead of the scaffolding that proves
    /// it. Never compiled into release builds.
    static DIGEST_ACCOUNTING_SUPPRESSED: Cell<bool> = const { Cell::new(false) };
}

/// Restores digest accounting when a debug cross-check scope ends.
#[cfg(any(test, debug_assertions))]
pub(crate) struct DigestAccountingSuppressionScope(bool);

#[cfg(any(test, debug_assertions))]
impl Drop for DigestAccountingSuppressionScope {
    fn drop(&mut self) {
        DIGEST_ACCOUNTING_SUPPRESSED.with(|flag| flag.set(self.0));
    }
}

/// Suppress digest accounting on this thread until the guard drops.
///
/// Verification scaffolding only (debug/test builds): a cross-check that
/// re-runs a validator to prove a fast path must not appear in the
/// `session_content_digest_computations`/`..._bytes` budgets the structural
/// regression tests measure — they exist to observe the production path.
#[cfg(any(test, debug_assertions))]
pub(crate) fn suppress_digest_accounting() -> DigestAccountingSuppressionScope {
    DIGEST_ACCOUNTING_SUPPRESSED.with(|flag| {
        let previous = flag.get();
        flag.set(true);
        DigestAccountingSuppressionScope(previous)
    })
}

thread_local! {
    /// Per-thread count of BYTES canonicalized-and-hashed for session content
    /// digests. The pass counter alone cannot see size-dependence: one
    /// whole-graph canonical pass counts 1 at every transcript size, which is
    /// exactly how a 211x release-timing regression once hid behind an
    /// "equal counts" assertion. Structural regression tests assert byte
    /// deltas are equal across transcript sizes.
    static CONTENT_DIGEST_BYTES: Cell<u64> = const { Cell::new(0) };
}

/// Per-thread count of bytes fed into session content-digest passes.
/// Observability seam for structural size-independence regression tests
/// only; not a public API.
#[doc(hidden)]
#[must_use]
pub fn session_content_digest_bytes() -> u64 {
    CONTENT_DIGEST_BYTES.with(Cell::get)
}

pub(crate) fn record_content_digest_bytes(bytes: u64) {
    #[cfg(any(test, debug_assertions))]
    if DIGEST_ACCOUNTING_SUPPRESSED.with(Cell::get) {
        return;
    }
    CONTENT_DIGEST_BYTES.with(|count| count.set(count.get().saturating_add(bytes)));
    GLOBAL_CONTENT_DIGEST_BYTES.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
    DIGEST_SITE_BYTES[CURRENT_DIGEST_SITE.with(Cell::get)]
        .fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
}

/// Process-wide companion to [`session_content_digest_bytes`].
///
/// The thread-local counter cannot see a turn whose digest work is spread
/// across the runtime's worker and blocking pools, which is exactly where the
/// boundary passes run. Structural size-independence assertions need the
/// process total.
static GLOBAL_CONTENT_DIGEST_BYTES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Process-wide count of bytes fed into session content-digest passes.
#[doc(hidden)]
#[must_use]
pub fn global_session_content_digest_bytes() -> u64 {
    GLOBAL_CONTENT_DIGEST_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Process-wide count of bytes PRODUCED by whole-session boundary
/// serialization (`CoreApplyOutput::with_session`, prepared checkpoint
/// documents, recovery snapshots). The digest counter above cannot see an
/// O(document) reserialize that hashes nothing; structural
/// size-independence gates assert this counter alongside it so a serialize
/// or persist regression cannot hide behind a flat digest curve.
static GLOBAL_SESSION_ENCODE_BYTES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Record bytes produced by one whole-session boundary serialization.
#[doc(hidden)]
pub fn record_session_encode_bytes(bytes: u64) {
    GLOBAL_SESSION_ENCODE_BYTES.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
}

/// Process-wide count of whole-session boundary serialization output bytes.
#[doc(hidden)]
#[must_use]
pub fn global_session_encode_bytes() -> u64 {
    GLOBAL_SESSION_ENCODE_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Number of buckets in the content-digest byte attribution table.
pub(crate) const DIGEST_SITE_COUNT: usize = 8;

/// Digest-byte attribution bucket for work with no enclosing named pass.
pub(crate) const DIGEST_SITE_OTHER: usize = 0;
/// Typed `Session` deserialization (durable-document decode ingress).
pub(crate) const DIGEST_SITE_DECODE: usize = 1;
/// Typed `Session` serialization (snapshot mint).
pub(crate) const DIGEST_SITE_ENCODE: usize = 2;
/// Whole-document checkpoint digest.
pub(crate) const DIGEST_SITE_CHECKPOINT_DIGEST: usize = 3;
/// Transcript-history witness derivation.
pub(crate) const DIGEST_SITE_WITNESS: usize = 4;
/// Rewrite-commit chain discovery over a session's transcript graph.
pub(crate) const DIGEST_SITE_REWRITE_CHAIN_WALK: usize = 5;
/// Append-only save guard.
pub(crate) const DIGEST_SITE_APPEND_GUARD: usize = 6;
/// Run-boundary snapshot save guard, excluding the two buckets above.
pub(crate) const DIGEST_SITE_BOUNDARY_GUARD: usize = 7;

/// Human-readable names for [`digest_site_bytes`], in bucket order.
#[doc(hidden)]
pub const DIGEST_SITE_LABELS: [&str; DIGEST_SITE_COUNT] = [
    "other",
    "decode",
    "encode",
    "checkpoint-digest",
    "witness",
    "rewrite-chain-walk",
    "append-guard",
    "boundary-guard",
];

static DIGEST_SITE_BYTES: [std::sync::atomic::AtomicU64; DIGEST_SITE_COUNT] =
    [const { std::sync::atomic::AtomicU64::new(0) }; DIGEST_SITE_COUNT];

thread_local! {
    /// Innermost named digest pass currently executing on this thread.
    ///
    /// Attribution is innermost-wins so a pass that a broader guard delegates
    /// to (the rewrite-chain walk inside the boundary guard, the witness
    /// derivation inside the checkpoint digest) is charged to itself rather
    /// than disappearing into its caller.
    static CURRENT_DIGEST_SITE: Cell<usize> = const { Cell::new(DIGEST_SITE_OTHER) };
}

/// Process-wide content-digest bytes, split by the pass that requested them.
///
/// Observability seam for the turn-boundary size-independence work only: the
/// aggregate counter says a turn hashed the whole document N times but not
/// which passes did it, which is the difference between fixing a boundary and
/// guessing at one.
#[doc(hidden)]
#[must_use]
pub fn digest_site_bytes() -> [u64; DIGEST_SITE_COUNT] {
    std::array::from_fn(|site| DIGEST_SITE_BYTES[site].load(std::sync::atomic::Ordering::Relaxed))
}

/// Restores the enclosing attribution bucket when the named pass returns.
pub(crate) struct DigestSiteScope(usize);

impl Drop for DigestSiteScope {
    fn drop(&mut self) {
        CURRENT_DIGEST_SITE.with(|site| site.set(self.0));
    }
}

/// Charge content-digest bytes to `site` until the returned guard drops.
pub(crate) fn enter_digest_site(site: usize) -> DigestSiteScope {
    CURRENT_DIGEST_SITE.with(|current| {
        let enclosing = current.get();
        current.set(site);
        DigestSiteScope(enclosing)
    })
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
    Ok(session_checkpoint_digest_selected(session, WitnessSelection::Evidence)?.digest)
}

/// [`session_checkpoint_digest`] for stamp MINTS: the transcript-history
/// witness is computed at the CURRENT format for full-graph documents (the
/// per-session lazy v3 upgrade — the schema-3 stamp and the v3 witness land
/// inside the same document write), while slim projections keep the format
/// their carrier declares (a slim row can never relabel itself: it lacks the
/// retained bodies an authority would need to validate first). The returned
/// witness format selects the stamp schema floor.
pub(crate) fn session_checkpoint_digest_for_mint(
    session: &Session,
) -> Result<MintedCheckpointDigest, SessionCheckpointError> {
    session_checkpoint_digest_selected(session, WitnessSelection::MintCurrent)
}

/// A canonical checkpoint digest plus the transcript-history witness format
/// folded into it (`None` when the document carries no history witness).
pub(crate) struct MintedCheckpointDigest {
    pub(crate) digest: SessionCheckpointDigest,
    pub(crate) witness_format: Option<u32>,
}

fn session_checkpoint_digest_selected(
    session: &Session,
    selection: WitnessSelection,
) -> Result<MintedCheckpointDigest, SessionCheckpointError> {
    let _digest_site = enter_digest_site(DIGEST_SITE_CHECKPOINT_DIGEST);
    // No count here: every canonical pass this performs — the history-graph
    // witness (when computed) and the document pass — is counted at the pass
    // site, `canonical_value_digest` or the framed splice path. Counting the
    // caller too used to hide a whole-graph pass inside one recorded
    // "computation".
    let witness = resolve_transcript_history_witness(session, selection)?;
    let history_digest = witness.as_ref().map(TranscriptHistoryWitness::digest);
    let digest = match framed_session_checkpoint_digest(session, history_digest) {
        Some(digest) => digest,
        None => {
            let document = checkpoint_digest_document_for_hash(session, history_digest)?;
            canonical_value_digest(&document)?
        }
    };
    // Computing this digest IS a complete canonical verification of this
    // exact document, so seal the proof on the document itself — it cannot
    // survive a mutation of that document or leak to a different one. A
    // stamp minted here and installed back-to-back on the same unmutated
    // document then costs ONE canonical pass instead of two; any content
    // mutation in between clears the seal and pays the full recompute.
    session.seal_verified_checkpoint_digest(&digest);
    Ok(MintedCheckpointDigest {
        digest,
        witness_format: witness.map(|witness| witness.witness_format()),
    })
}

/// Build the exact value the canonical checkpoint digest hashes: the
/// session's digest document with the stamp/provenance/history keys removed
/// and the compact history marker substituted.
fn checkpoint_digest_document_for_hash(
    session: &Session,
    history_digest: Option<&SessionCheckpointDigest>,
) -> Result<serde_json::Value, SessionCheckpointError> {
    let mut document = session.checkpoint_digest_document()?;
    strip_checkpoint_digest_metadata(&mut document, history_digest);
    Ok(document)
}

fn strip_checkpoint_digest_metadata(
    document: &mut serde_json::Value,
    history_digest: Option<&SessionCheckpointDigest>,
) {
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
                checkpoint_history_digest_marker(digest),
            );
        }
    }
}

/// Framed-midstate fast path for the canonical checkpoint digest.
///
/// The canonical document is `{"created_at":C,"id":I,"messages":[…],…}`:
/// only the two immutable identity fields sort before the transcript, so a
/// retained SHA-256 midstate over `prefix ++ "[" ++ elements` finalized with
/// `"]" ++ suffix` reproduces the byte-identical digest while only the
/// turn-sized suffix is serialized per call. This is a representation
/// change, never a verdict: any surprise — marker not found exactly once,
/// parked accumulator, prefix mismatch, serialization failure — returns
/// `None` and the caller runs the full document path.
fn framed_session_checkpoint_digest(
    session: &Session,
    history_digest: Option<&SessionCheckpointDigest>,
) -> Option<SessionCheckpointDigest> {
    let (mut document, marker) = session.checkpoint_digest_framed_document().ok()?;
    strip_checkpoint_digest_metadata(&mut document, history_digest);
    let mut framed = Vec::new();
    write_canonical_json(&document, &mut framed).ok()?;
    let needle = serde_json::to_string(&marker).ok()?;
    let (prefix, suffix) = split_exactly_once(&framed, needle.as_bytes())?;
    let mut hasher = session.framed_document_hasher(prefix)?;
    hasher.update(b"]");
    hasher.update(suffix);
    record_content_digest_computation();
    record_content_digest_bytes(prefix.len() as u64 + 1 + suffix.len() as u64);
    let digest = SessionCheckpointDigest(format!("sha256:{:x}", hasher.finalize()));
    if crate::session::digest_accumulator_take_verification_sample() {
        // Debug builds verify every framed serve, release builds the first
        // N per process, both against the untouched full path with digest
        // accounting suppressed so the budget tests keep measuring the
        // production path (same discipline as the accumulator witness).
        #[cfg(any(test, debug_assertions))]
        let _suppress = suppress_digest_accounting();
        if let Ok(document) = checkpoint_digest_document_for_hash(session, history_digest)
            && let Ok(reference) = canonical_value_digest_uncounted(&document)
        {
            assert_eq!(
                digest, reference,
                "framed checkpoint digest diverged from the canonical document digest: a \
                 framing seam changed the canonical byte stream without invalidating the midstate"
            );
        }
    }
    Some(digest)
}

/// Seed the session's framed checkpoint midstate if it is absent.
///
/// A long-lived producer session (the agent's own) never computes a
/// checkpoint digest itself — mints and verifies run on copies decoded from
/// the bytes it seals — so without this, every sealed snapshot carries an
/// empty framed midstate and every downstream copy pays a fresh O(document)
/// reseed per checkpoint digest. Seeding once here (the prefix is a pure
/// function of the immutable `created_at`/`id`, so it is stable for the
/// session's lifetime) lets ordinary appends extend it and every sealed
/// snapshot carry it forward. Failure is fine: consumers fall back to the
/// full canonical path.
pub fn warm_framed_checkpoint_midstate(session: &Session) {
    let _digest_site = enter_digest_site(DIGEST_SITE_CHECKPOINT_DIGEST);
    let Ok((mut document, marker)) = session.checkpoint_digest_framed_document() else {
        return;
    };
    strip_checkpoint_digest_metadata(&mut document, None);
    let mut framed = Vec::new();
    if write_canonical_json(&document, &mut framed).is_err() {
        return;
    }
    let Ok(needle) = serde_json::to_string(&marker) else {
        return;
    };
    let Some((prefix, _)) = split_exactly_once(&framed, needle.as_bytes()) else {
        return;
    };
    let _ = session.framed_document_hasher(prefix);
}

/// Locate `needle` in `haystack` exactly once. Two or more occurrences —
/// e.g. a metadata value that happens to carry the marker text — return
/// `None` so the caller falls back instead of splicing at the wrong site.
fn split_exactly_once<'a>(haystack: &'a [u8], needle: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let first = haystack
        .windows(needle.len())
        .position(|window| window == needle)?;
    let rest = &haystack[first + 1..];
    if rest.len() >= needle.len() && rest.windows(needle.len()).any(|window| window == needle) {
        return None;
    }
    Some((&haystack[..first], &haystack[first + needle.len()..]))
}

/// Resolve the storage-invariant transcript-history witness carried by a
/// session document, verifying under the format the evidence declares.
///
/// Full documents derive it from the canonical retained graph. Incremental
/// projections carry the same digest under a reserved metadata key because
/// their revision bodies live out of line. If both representations are
/// present they must agree exactly; malformed or contradictory evidence is
/// never treated as absence.
pub fn session_transcript_history_checkpoint_digest(
    session: &Session,
) -> Result<Option<SessionCheckpointDigest>, SessionCheckpointError> {
    Ok(session_transcript_history_witness(session)?.map(TranscriptHistoryWitness::into_digest))
}

/// [`session_transcript_history_checkpoint_digest`] returning the full typed
/// carrier, for writers that persist the witness on slim projections.
pub fn session_transcript_history_witness(
    session: &Session,
) -> Result<Option<TranscriptHistoryWitness>, SessionCheckpointError> {
    resolve_transcript_history_witness(session, WitnessSelection::Evidence)
}

/// Which transcript-history witness format a derivation runs under.
#[derive(Debug, Clone, Copy)]
enum WitnessSelection {
    /// Verify under the format the document's own evidence declares: the
    /// typed carrier on slim rows, the stamp schema on full documents, and
    /// format 2 for pre-carrier legacy rows. v2 evidence verifies as v2
    /// indefinitely — mixed stores, no flag day.
    Evidence,
    /// Verify under the format a SPECIFIC stamp's schema declares — the
    /// install seam, where the stamp being installed is not (yet) the one
    /// in the document's metadata, so metadata evidence would resolve the
    /// PREDECESSOR's format and refuse a valid freshly minted stamp.
    DeclaredBySchema(u32),
    /// Mint at the CURRENT format for full-graph documents (the lazy
    /// per-session upgrade seam); slim rows keep their carried format.
    MintCurrent,
}

/// [`session_checkpoint_digest`] verified under the witness format `stamp`'s
/// schema declares. For the stamp-install seam only; see
/// [`WitnessSelection::DeclaredBySchema`].
pub(crate) fn session_checkpoint_digest_for_stamp(
    session: &Session,
    stamp: &SessionCheckpointStamp,
) -> Result<SessionCheckpointDigest, SessionCheckpointError> {
    Ok(session_checkpoint_digest_selected(
        session,
        WitnessSelection::DeclaredBySchema(stamp.schema_version()),
    )?
    .digest)
}

/// The two-axis typed transcript-history witness carrier.
///
/// `witness_format` names the WITNESS computation (2 = sequential canonical
/// whole-graph hash, 3 = revision-identity digest); `revision_digest_format`
/// names the revision-STRING format the graph's content addresses use
/// (`TRANSCRIPT_DIGEST_FORMAT_CURRENT`, independent axis — see the graph's
/// `digest_format` field, which this deliberately does not touch). Bare
/// digest strings — every pre-v3 durable row — normalize to
/// `{witness_format: 2, revision_digest_format: 2}`. An unknown
/// `witness_format` refuses typed BEFORE any normalization or healing, via
/// the generated `SessionPersistenceVersionAuthority` membership gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptHistoryWitness {
    witness_format: u32,
    revision_digest_format: u32,
    digest: SessionCheckpointDigest,
}

impl TranscriptHistoryWitness {
    #[must_use]
    pub const fn witness_format(&self) -> u32 {
        self.witness_format
    }

    #[must_use]
    pub const fn revision_digest_format(&self) -> u32 {
        self.revision_digest_format
    }

    #[must_use]
    pub fn digest(&self) -> &SessionCheckpointDigest {
        &self.digest
    }

    #[must_use]
    pub fn into_digest(self) -> SessionCheckpointDigest {
        self.digest
    }

    /// The metadata value a slim projection persists under
    /// `SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY`. v2 stays the bare
    /// string every pre-v3 reader understands; v3 persists the typed object.
    #[must_use]
    pub fn to_carried_value(&self) -> serde_json::Value {
        if self.witness_format <= 2 {
            serde_json::Value::String(self.digest.as_str().to_string())
        } else {
            serde_json::json!({
                "witness_format": self.witness_format,
                "revision_digest_format": self.revision_digest_format,
                "digest": self.digest.as_str(),
            })
        }
    }

    /// Parse a carried witness value. The format gate runs FIRST: an object
    /// whose `witness_format` the generated persistence-version authority
    /// does not accept refuses typed before any other field is interpreted.
    pub fn from_carried_value(value: &serde_json::Value) -> Result<Self, SessionCheckpointError> {
        match value {
            serde_json::Value::String(digest) => Ok(Self {
                witness_format: 2,
                revision_digest_format: crate::session::TRANSCRIPT_DIGEST_FORMAT_CURRENT,
                digest: SessionCheckpointDigest(digest.clone()),
            }),
            serde_json::Value::Object(fields) => {
                let witness_format = fields
                    .get("witness_format")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|format| u32::try_from(format).ok())
                    .ok_or_else(|| {
                        SessionCheckpointError::MalformedTranscriptHistoryWitness(
                            "carried witness object is missing a numeric witness_format"
                                .to_string(),
                        )
                    })?;
                crate::generated::session_persistence_version_authority::
                    restore_transcript_history_witness_format(witness_format)
                .map_err(|_| {
                    SessionCheckpointError::UnsupportedTranscriptHistoryWitnessFormat(
                        witness_format,
                    )
                })?;
                let revision_digest_format = fields
                    .get("revision_digest_format")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|format| u32::try_from(format).ok())
                    .ok_or_else(|| {
                        SessionCheckpointError::MalformedTranscriptHistoryWitness(
                            "carried witness object is missing a numeric revision_digest_format"
                                .to_string(),
                        )
                    })?;
                if revision_digest_format != crate::session::TRANSCRIPT_DIGEST_FORMAT_CURRENT {
                    return Err(
                        SessionCheckpointError::UnsupportedTranscriptHistoryRevisionDigestFormat(
                            revision_digest_format,
                        ),
                    );
                }
                let digest = fields
                    .get("digest")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        SessionCheckpointError::MalformedTranscriptHistoryWitness(
                            "carried witness object is missing a digest string".to_string(),
                        )
                    })?;
                Ok(Self {
                    witness_format,
                    revision_digest_format,
                    digest: SessionCheckpointDigest(digest.to_string()),
                })
            }
            other => Err(SessionCheckpointError::MalformedTranscriptHistoryWitness(
                format!("carried witness must be a digest string or a typed object, got {other}"),
            )),
        }
    }
}

/// Resolve, compute, and cross-check the document's history witness under
/// `selection`. Returns `None` only when the document carries neither a
/// graph nor a carried witness.
fn resolve_transcript_history_witness(
    session: &Session,
    selection: WitnessSelection,
) -> Result<Option<TranscriptHistoryWitness>, SessionCheckpointError> {
    let carried = session
        .metadata()
        .get(SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY)
        .map(TranscriptHistoryWitness::from_carried_value)
        .transpose()?;
    let Some(history) = session.metadata().get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY) else {
        return Ok(carried);
    };
    let format = match selection {
        WitnessSelection::MintCurrent => {
            crate::generated::session_persistence_version_authority::TRANSCRIPT_HISTORY_WITNESS_FORMAT
        }
        // The stamp being installed declares the format its digest was
        // minted under; a stale carried witness (a slim-to-full transitional
        // document can retain one) must not override it, or a valid
        // freshly minted schema-3 stamp would be recomputed under v2 and
        // refused as a mismatch. The carried witness is still independently
        // cross-checked under ITS OWN format below.
        WitnessSelection::DeclaredBySchema(schema) => witness_format_for_stamp_schema(schema),
        // Same precedence for document evidence on a GRAPH-BEARING document:
        // the stamp is the verification target, so its schema outranks a
        // transitional carried witness (which a graph-bearing document only
        // retains on hand-assembled or older-writer shapes); the carrier
        // decides only when no stamp is readable. Slim rows never reach
        // here — their carrier is returned before format selection.
        WitnessSelection::Evidence => match stamped_witness_format(session) {
            Some(format) => format,
            None => carried
                .as_ref()
                .map_or(2, TranscriptHistoryWitness::witness_format),
        },
    };
    let computed = computed_transcript_history_witness(session, history, format)?;
    if let Some(carrier) = &carried {
        let cross = if carrier.witness_format == format {
            computed.clone()
        } else {
            computed_transcript_history_witness(session, history, carrier.witness_format)?
        };
        if carrier.digest != cross {
            return Err(SessionCheckpointError::TranscriptHistoryWitnessMismatch {
                carried: carrier.digest.clone(),
                computed: cross,
            });
        }
    }
    Ok(Some(TranscriptHistoryWitness {
        witness_format: format,
        revision_digest_format: crate::session::TRANSCRIPT_DIGEST_FORMAT_CURRENT,
        digest: computed,
    }))
}

/// The witness format a FULL document's stamp evidence implies, when a
/// stamp schema is readable at all: schema-v3 stamps were minted over the
/// v3 witness, everything older over v2. `None` means no stamp (or a
/// malformed one — the stamp parse itself refuses those typed before any
/// digest comparison is trusted, so falling back to weaker evidence can
/// only make a broken document fail closed with a digest mismatch).
fn stamped_witness_format(session: &Session) -> Option<u32> {
    session
        .metadata()
        .get(SESSION_CHECKPOINT_STAMP_KEY)
        .and_then(|stamp| stamp.get("schema_version"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|schema| u32::try_from(schema).ok())
        .map(witness_format_for_stamp_schema)
}

fn witness_format_for_stamp_schema(schema: u32) -> u32 {
    if schema >= SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3 {
        crate::generated::session_persistence_version_authority::TRANSCRIPT_HISTORY_WITNESS_FORMAT
    } else {
        2
    }
}

/// Compute the history witness for `history` under `format`, serving and
/// feeding the per-session per-format memo.
fn computed_transcript_history_witness(
    session: &Session,
    history: &serde_json::Value,
    format: u32,
) -> Result<SessionCheckpointDigest, SessionCheckpointError> {
    if let Some(cached) = session.cached_transcript_history_witness(format) {
        return Ok(SessionCheckpointDigest(cached.to_string()));
    }
    let computed = match format {
        2 => {
            // Incremental assembly first: cached canonical segments plus
            // the retained sorted transcript stream reduce the derivation
            // to one raw hash pass. Any structural surprise falls back to
            // the full canonicalization below, which also remains the
            // error-reporting path for malformed graphs.
            match session.assemble_transcript_history_witness(history) {
                Some(assembled) => assembled,
                None => session_checkpoint_history_digest(history)?,
            }
        }
        3 => session_checkpoint_history_digest_v3(history)?,
        other => {
            return Err(SessionCheckpointError::UnsupportedTranscriptHistoryWitnessFormat(other));
        }
    };
    session.record_transcript_history_witness(format, computed.as_str());
    Ok(computed)
}

/// Domain separator for the format-3 transcript-history witness preimage.
pub(crate) const TRANSCRIPT_HISTORY_WITNESS_DOMAIN_V3: &str =
    "meerkat/transcript-history-witness/v3";

/// Format-3 (revision-identity) transcript-history witness.
///
/// The preimage pins the head revision, the digest of the canonical full
/// ordered commit log, and the digest of the sorted unique retained revision
/// IDs — content addresses that transitively pin canonical message content
/// (ingress verifies body bytes against revision strings; the integrity
/// budget moved there, it did not vanish). Complexity is honestly O(number
/// of retained revisions + commit log), never O(retained body BYTES): no
/// message body is touched.
fn session_checkpoint_history_digest_v3(
    history: &serde_json::Value,
) -> Result<SessionCheckpointDigest, SessionCheckpointError> {
    let _digest_site = enter_digest_site(DIGEST_SITE_WITNESS);
    let malformed = |what: &str| {
        SessionCheckpointError::MalformedTranscriptHistoryWitness(format!(
            "transcript history graph value is missing {what}"
        ))
    };
    let object = history
        .as_object()
        .ok_or_else(|| malformed("an object form"))?;
    let head = object
        .get("head")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| malformed("a head revision string"))?;
    // The commit log round-trips through the typed form so canonical bytes
    // are identical no matter which legacy spelling the raw value carries —
    // the same normalization the v2 witness assembly applies.
    let commits: Vec<crate::TranscriptRewriteCommit> = match object.get("commits") {
        Some(commits) => serde_json::from_value(commits.clone())?,
        None => Vec::new(),
    };
    let commits_value = serde_json::to_value(&commits)?;
    let mut commits_bytes = Vec::new();
    write_canonical_json(&commits_value, &mut commits_bytes)?;
    let mut revision_ids: Vec<&str> = match object.get("revisions") {
        Some(revisions) => revisions
            .as_array()
            .ok_or_else(|| malformed("a revisions array"))?
            .iter()
            .map(|body| {
                body.get("revision")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| malformed("a revision id on every retained body"))
            })
            .collect::<Result<_, _>>()?,
        None => Vec::new(),
    };
    revision_ids.sort_unstable();
    revision_ids.dedup();
    let ids_value = serde_json::Value::Array(
        revision_ids
            .into_iter()
            .map(|id| serde_json::Value::String(id.to_string()))
            .collect(),
    );
    let mut ids_bytes = Vec::new();
    write_canonical_json(&ids_value, &mut ids_bytes)?;
    record_content_digest_bytes((commits_bytes.len() + ids_bytes.len()) as u64);
    let commits_digest = format!("sha256:{:x}", Sha256::digest(&commits_bytes));
    let retained_revisions_digest = format!("sha256:{:x}", Sha256::digest(&ids_bytes));
    let preimage = serde_json::json!({
        "domain": TRANSCRIPT_HISTORY_WITNESS_DOMAIN_V3,
        "revision_digest_format": crate::session::TRANSCRIPT_DIGEST_FORMAT_CURRENT,
        "head_revision": head,
        "commits_digest": commits_digest,
        "retained_revisions_digest": retained_revisions_digest,
    });
    canonical_value_digest(&preimage)
}

/// Exact byte digest of a legacy source BLOB used only as migration custody.
#[must_use]
pub fn legacy_session_source_blob_digest(source_blob: &[u8]) -> SessionCheckpointDigest {
    record_content_digest_computation();
    record_content_digest_bytes(source_blob.len() as u64);
    SessionCheckpointDigest(format!("sha256:{:x}", Sha256::digest(source_blob)))
}

fn session_checkpoint_history_digest(
    history: &serde_json::Value,
) -> Result<SessionCheckpointDigest, SessionCheckpointError> {
    let history = crate::session::canonicalize_checkpoint_history_value(history)?;
    canonical_value_digest(&history)
}

/// Full history-witness recompute that does NOT bump the digest budget
/// counters. Reserved for the incremental-assembly cross-check: that
/// recompute is verification scaffolding, not production work.
pub(crate) fn session_checkpoint_history_digest_uncounted(
    history: &serde_json::Value,
) -> Result<SessionCheckpointDigest, SessionCheckpointError> {
    let history = crate::session::canonicalize_checkpoint_history_value(history)?;
    let mut canonical = Vec::new();
    write_canonical_json(&history, &mut canonical)?;
    Ok(SessionCheckpointDigest(format!(
        "sha256:{:x}",
        Sha256::digest(canonical)
    )))
}

impl SessionCheckpointDigest {
    /// Adopt a digest string minted by the incremental history-witness
    /// assembly, which produces the exact `sha256:<64 hex>` spelling of
    /// [`canonical_value_digest`] byte-for-byte.
    pub(crate) fn from_assembled(digest: String) -> Self {
        Self(digest)
    }
}

/// Compute the storage-invariant witness for a reconstructed transcript
/// history graph.
pub fn transcript_history_checkpoint_digest(
    history: &crate::TranscriptHistoryState,
) -> Result<SessionCheckpointDigest, SessionCheckpointError> {
    let value = serde_json::to_value(history)?;
    session_checkpoint_history_digest(&value)
}

/// [`transcript_history_checkpoint_digest`] under an explicit witness format.
///
/// Store guards that compare a graph against the witness an incoming slim
/// document CARRIES must derive under the format that carrier declares —
/// format 2 (sequential canonical whole-graph hash) or format 3
/// (revision-identity digest). An unknown format refuses typed, the same
/// verdict as the carrier ingress gate.
pub(crate) fn transcript_history_checkpoint_digest_in_format(
    history: &crate::TranscriptHistoryState,
    witness_format: u32,
) -> Result<SessionCheckpointDigest, SessionCheckpointError> {
    let value = serde_json::to_value(history)?;
    match witness_format {
        2 => session_checkpoint_history_digest(&value),
        3 => session_checkpoint_history_digest_v3(&value),
        other => Err(SessionCheckpointError::UnsupportedTranscriptHistoryWitnessFormat(other)),
    }
}

fn canonical_value_digest(
    value: &serde_json::Value,
) -> Result<SessionCheckpointDigest, SessionCheckpointError> {
    let mut canonical = Vec::new();
    write_canonical_json(value, &mut canonical)?;
    // Every call is one full canonical-JSON + SHA-256 pass over a whole
    // document or graph value: this is THE pass the digest budget exists to
    // observe. It went uncounted once and the budget reported zero while a
    // whole-graph pass per boundary grew release timing 211x.
    record_content_digest_computation();
    record_content_digest_bytes(canonical.len() as u64);
    Ok(SessionCheckpointDigest(format!(
        "sha256:{:x}",
        Sha256::digest(canonical)
    )))
}

/// [`canonical_value_digest`] without digest accounting. Reserved for the
/// framed-fast-path cross-check: verification scaffolding must not appear in
/// the budgets the structural regression tests measure.
fn canonical_value_digest_uncounted(
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

pub(crate) fn write_canonical_json(
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
    Ok(verified_checkpoint_stamp_relation(&left, &right))
}

/// Classify two ALREADY-VERIFIED checkpoint stamps without touching either
/// document's content.
///
/// Each stamp must have been proved against its enclosing document first
/// ([`Session::try_checkpoint_state`] or its cached form); this function
/// compares stamp identity fields only and never re-hashes. Legacy
/// (unstamped) documents cannot reach this seam, so the legacy relation
/// variants are never returned.
#[must_use]
pub fn verified_checkpoint_stamp_relation(
    left: &SessionCheckpointStamp,
    right: &SessionCheckpointStamp,
) -> SessionCheckpointRelation {
    if left.session_id != right.session_id {
        return SessionCheckpointRelation::DifferentSessionIdentity;
    }
    if left.lineage_id != right.lineage_id {
        return SessionCheckpointRelation::DifferentLineage;
    }
    if left.generation < right.generation {
        return SessionCheckpointRelation::LeftGenerationOlder;
    }
    if left.generation > right.generation {
        return SessionCheckpointRelation::LeftGenerationNewer;
    }
    if left.checkpoint_revision < right.checkpoint_revision {
        return SessionCheckpointRelation::LeftRevisionOlder;
    }
    if left.checkpoint_revision > right.checkpoint_revision {
        return SessionCheckpointRelation::LeftRevisionNewer;
    }
    if left == right {
        SessionCheckpointRelation::Exact
    } else {
        SessionCheckpointRelation::RevisionConflict
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

/// Mechanical transcript relation between two legacy (unstamped) copies of the
/// same session document, observed during one-time recovery migration.
///
/// Legacy documents carry no stamps, so ancestry cannot be proven typed. The
/// only mechanical proof available is per-message canonical-JSON prefix
/// equality over the transcripts. Non-transcript fields are deliberately not
/// compared: whichever copy migration adopts carries its own non-transcript
/// fields wholesale, and pre-typed writers routinely differ on save-time
/// bookkeeping without diverging conversation content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacySessionTranscriptRelation {
    /// Same message count and per-message canonical equality.
    Identical,
    /// The snapshot's transcript is a strict prefix of the projection's.
    ProjectionExtendsSnapshot,
    /// The projection's transcript is a strict prefix of the snapshot's.
    SnapshotExtendsProjection,
    /// Neither transcript is a prefix of the other.
    Divergent,
}

/// Classify the transcript relation between the committed runtime snapshot
/// copy and the session-store projection copy of one legacy session document.
///
/// Both documents must decode as `LegacyUnverified` and carry the same
/// session id; typed documents must use `session_checkpoint_relation`.
pub fn legacy_session_transcript_relation(
    snapshot: &Session,
    projection: &Session,
) -> Result<LegacySessionTranscriptRelation, SessionCheckpointError> {
    for (side, session) in [("snapshot", snapshot), ("projection", projection)] {
        if !matches!(
            session.try_checkpoint_state()?,
            SessionCheckpointState::LegacyUnverified { .. }
        ) {
            return Err(SessionCheckpointError::AuthorityBaseConflict(format!(
                "legacy transcript relation requires an untyped legacy {side} document"
            )));
        }
    }
    transcript_prefix_relation(snapshot, projection)
}

/// Classify the transcript relation between a pre-typed (legacy-unverified)
/// committed runtime snapshot copy and the TYPED (verified) session-store
/// projection copy of one session document, observed during one-time
/// recovery migration.
///
/// This is the sanctioned-adoption shape: downstream adoption (for example
/// MobKit lazy-at-restore or the bulk operator sweep) stamps the continuity
/// store row while the runtime store still holds the pre-adoption legacy
/// snapshot. The snapshot must decode as `LegacyUnverified` and the
/// projection must carry verified typed checkpoint authority; both-legacy
/// pairs use [`legacy_session_transcript_relation`] and typed pairs use
/// [`session_checkpoint_relation`]. The comparison itself is identical to
/// the both-legacy classifier: per-message canonical-JSON equality over the
/// shared prefix, then length ordering. Non-transcript fields are
/// deliberately not compared: adoption stamps the row without touching
/// conversation content, and save-time bookkeeping (for example
/// `metadata.session_build_state`) routinely differs between the copies
/// without diverging conversation content.
pub fn legacy_snapshot_vs_typed_projection_transcript_relation(
    snapshot: &Session,
    projection: &Session,
) -> Result<LegacySessionTranscriptRelation, SessionCheckpointError> {
    if !matches!(
        snapshot.try_checkpoint_state()?,
        SessionCheckpointState::LegacyUnverified { .. }
    ) {
        return Err(SessionCheckpointError::AuthorityBaseConflict(
            "legacy-snapshot-vs-typed-projection transcript relation requires an \
             untyped legacy snapshot document"
                .to_string(),
        ));
    }
    if !matches!(
        projection.try_checkpoint_state()?,
        SessionCheckpointState::Verified(_)
    ) {
        return Err(SessionCheckpointError::AuthorityBaseConflict(
            "legacy-snapshot-vs-typed-projection transcript relation requires a \
             verified typed projection document"
                .to_string(),
        ));
    }
    transcript_prefix_relation(snapshot, projection)
}

/// Shared mechanical core of the migration-time transcript classifiers:
/// exact session identity, per-message canonical-JSON equality over the
/// shared prefix, then message-count ordering. Callers own the
/// checkpoint-state admission; this helper compares transcripts only.
fn transcript_prefix_relation(
    snapshot: &Session,
    projection: &Session,
) -> Result<LegacySessionTranscriptRelation, SessionCheckpointError> {
    if snapshot.id() != projection.id() {
        return Err(SessionCheckpointError::SessionIdMismatch {
            expected: snapshot.id().clone(),
            actual: projection.id().clone(),
        });
    }
    let snapshot_messages = snapshot.messages();
    let projection_messages = projection.messages();
    let shared = snapshot_messages.len().min(projection_messages.len());
    for (snapshot_message, projection_message) in snapshot_messages
        .iter()
        .take(shared)
        .zip(projection_messages.iter().take(shared))
    {
        let snapshot_value = serde_json::to_value(snapshot_message)?;
        let projection_value = serde_json::to_value(projection_message)?;
        if snapshot_value != projection_value {
            return Ok(LegacySessionTranscriptRelation::Divergent);
        }
    }
    Ok(
        match snapshot_messages.len().cmp(&projection_messages.len()) {
            std::cmp::Ordering::Equal => LegacySessionTranscriptRelation::Identical,
            std::cmp::Ordering::Less => LegacySessionTranscriptRelation::ProjectionExtendsSnapshot,
            std::cmp::Ordering::Greater => {
                LegacySessionTranscriptRelation::SnapshotExtendsProjection
            }
        },
    )
}

/// One adopted legacy session: the typed migration stamp bound to the exact
/// source bytes, the stamped document, and its serialized durable form.
pub struct AdoptedLegacySession {
    pub session: Session,
    pub stamp: SessionCheckpointStamp,
    pub serialized: Vec<u8>,
}

/// Adopt one pre-typed (legacy-unverified) session BLOB into typed checkpoint
/// authority via a one-time recovery migration.
///
/// This is the shared stamping seam for every backend that holds pre-typed
/// documents — the disk resolver, remote store implementations, and
/// continuity-snapshot adoption — so the ordering subtleties live in exactly
/// one place. Contract for callers:
///
/// - `source_blob` must be the FINAL bytes: perform any metadata repair
///   (for example comms-name rewrites at restore) before calling, because
///   the stamp's `Legacy` authority base takes byte custody of exactly these
///   bytes.
/// - `observed_generation` / `observed_checkpoint_revision` must come from
///   the externally observed continuity cursor when one exists. `INITIAL`
///   cursors are correct only for lineages that never minted authority
///   (pre-typed fleets with no continuity generation floor); stamping a
///   lower generation than the continuity row records makes the mismatch
///   sticky, because the document then verifies and no longer re-migrates.
/// - The blob must decode as a legacy-unverified document; typed documents
///   are refused with a typed error, never re-stamped.
pub fn adopt_legacy_session(
    source_blob: &[u8],
    observed_generation: SessionGeneration,
    observed_checkpoint_revision: SessionCheckpointRevision,
) -> Result<AdoptedLegacySession, SessionCheckpointError> {
    let mut session: Session = serde_json::from_slice(source_blob)?;
    let stamp = SessionCheckpointStamp::recovery_migration(
        &session,
        source_blob,
        observed_generation,
        observed_checkpoint_revision,
    )?;
    session.install_checkpoint_stamp(stamp.clone())?;
    let serialized = serde_json::to_vec(&session)?;
    match session.try_checkpoint_state()? {
        SessionCheckpointState::Verified(verified) if verified == stamp => {}
        _ => {
            return Err(SessionCheckpointError::AuthorityBaseConflict(
                "adopted legacy session failed post-install verification".to_string(),
            ));
        }
    }
    Ok(AdoptedLegacySession {
        session,
        stamp,
        serialized,
    })
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
    fn cached_checkpoint_state_verifies_first_sight_then_skips_recomputation() {
        // `stamped_root` builds the session through `Session::deserialize`,
        // so its per-session seal starts empty.
        let session = stamped_root(&session_with_text("cached read"));
        let before = session_content_digest_computations();
        assert!(matches!(
            session.try_checkpoint_state_cached().expect("first read"),
            SessionCheckpointState::Verified(_)
        ));
        let after_first = session_content_digest_computations();
        assert!(
            after_first > before,
            "first sight of a digest key in this process must fully verify"
        );
        for _ in 0..4 {
            assert!(matches!(
                session
                    .try_checkpoint_state_cached()
                    .expect("memoized read"),
                SessionCheckpointState::Verified(_)
            ));
        }
        assert_eq!(
            session_content_digest_computations(),
            after_first,
            "steady-state cached reads of an unchanged document must not recompute digests"
        );

        // The exact seam never consults the memo: write/adopt/convergence
        // verification stays full.
        assert!(matches!(
            session.try_checkpoint_state().expect("exact read"),
            SessionCheckpointState::Verified(_)
        ));
        assert!(
            session_content_digest_computations() > after_first,
            "try_checkpoint_state must keep re-verifying content"
        );
    }

    #[test]
    fn cached_checkpoint_state_fails_closed_on_unproved_digest_key() {
        let session = session_with_text("verify me");
        let mut other = session.clone();
        other.push(Message::User(UserMessage::text("diverged".to_string())));
        // Syntactically valid stamp naming a digest this document does not
        // have and this process never proved: the memo cannot admit it, so
        // the cached seam re-verifies and rejects.
        let wrong = SessionCheckpointStamp::new(
            session.id().clone(),
            SessionLineageId::for_session(session.id()),
            SessionGeneration::INITIAL,
            SessionCheckpointRevision::INITIAL,
            SessionCheckpointAuthorityBase::Absent,
            session_checkpoint_digest(&other).expect("digest"),
            SessionCheckpointProvenance::SessionCreated,
        );
        let mismatched = install_stamp(&session, &wrong);
        assert!(matches!(
            mismatched.try_checkpoint_state_cached(),
            Err(SessionCheckpointError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn cached_checkpoint_state_reverifies_after_in_process_content_mutation() {
        let session = stamped_root(&session_with_text("shape keyed"));
        assert!(matches!(
            session.try_checkpoint_state_cached().expect("seed memo"),
            SessionCheckpointState::Verified(_)
        ));
        // Metadata mutation with the stale stamp left in place: set_metadata
        // clears the per-session seal, so the cached seam re-verifies and
        // fails closed within the same process.
        let mut mutated = session;
        mutated.set_metadata("caller_fact", serde_json::json!("drift"));
        assert!(matches!(
            mutated.try_checkpoint_state_cached(),
            Err(SessionCheckpointError::DigestMismatch { .. })
        ));
    }

    /// Minimal in-memory [`crate::blob::BlobStore`] for the seal-invalidation
    /// tests: `put_image` really stores and returns a `BlobRef` (so
    /// externalization rewrites the block), `get` serves stored payloads (so
    /// hydration rewrites the block).
    #[derive(Default)]
    struct StubBlobStore {
        blobs: std::sync::Mutex<
            std::collections::HashMap<crate::blob::BlobId, crate::blob::BlobPayload>,
        >,
    }

    impl StubBlobStore {
        fn with_payload(payload: crate::blob::BlobPayload) -> Self {
            Self {
                blobs: std::sync::Mutex::new(std::collections::HashMap::from([(
                    payload.blob_id.clone(),
                    payload,
                )])),
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl crate::blob::BlobStore for StubBlobStore {
        async fn put_image(
            &self,
            media_type: &str,
            data: &str,
        ) -> Result<crate::blob::BlobRef, crate::blob::BlobStoreError> {
            let blob_id = crate::blob::BlobId::new(format!("sha256:stub-{}", data.len()));
            self.blobs.lock().expect("stub blob store lock").insert(
                blob_id.clone(),
                crate::blob::BlobPayload {
                    blob_id: blob_id.clone(),
                    media_type: media_type.to_string(),
                    data: data.to_string(),
                },
            );
            Ok(crate::blob::BlobRef {
                blob_id,
                media_type: media_type.to_string(),
            })
        }

        async fn get(
            &self,
            blob_id: &crate::blob::BlobId,
        ) -> Result<crate::blob::BlobPayload, crate::blob::BlobStoreError> {
            self.blobs
                .lock()
                .expect("stub blob store lock")
                .get(blob_id)
                .cloned()
                .ok_or_else(|| crate::blob::BlobStoreError::NotFound(blob_id.clone()))
        }

        async fn delete(
            &self,
            _blob_id: &crate::blob::BlobId,
        ) -> Result<(), crate::blob::BlobStoreError> {
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            false
        }
    }

    fn stamped_session_with_image(data: crate::types::ImageData) -> Session {
        let mut session = Session::new();
        session.push(Message::User(crate::types::UserMessage::with_blocks(vec![
            crate::types::ContentBlock::Image {
                media_type: "image/png".to_string(),
                data,
            },
        ])));
        stamped_root(&session)
    }

    // The three content-mutation seams that deliberately do NOT advance
    // `updated_at`. Under the retired process-global VerifiedStampKey memo,
    // `externalize_media` and `hydrate_realtime_user_images_with_usage`
    // rewrote message content while leaving every key field byte-identical,
    // so the cached seam kept reporting Verified over a stale stamp — a
    // live integrity hole needing no key collision. Each seam must clear
    // the per-session seal explicitly.

    #[tokio::test]
    async fn cached_checkpoint_state_fails_closed_after_media_externalization() {
        let mut session = stamped_session_with_image(crate::types::ImageData::Inline {
            data: "iVBORw0KGgo=".to_string(),
        });
        // Seed the seal with one full verification of the stamped document.
        assert!(matches!(
            session.try_checkpoint_state_cached().expect("seed seal"),
            SessionCheckpointState::Verified(_)
        ));
        let store = StubBlobStore::default();
        session
            .externalize_media(&store, 0)
            .await
            .expect("externalize inline image");
        // Message content changed in place; message count, metadata entry
        // count, created_at, and updated_at are all unchanged. The stale
        // stamp must no longer be served from memoized trust.
        assert!(matches!(
            session.try_checkpoint_state_cached(),
            Err(SessionCheckpointError::DigestMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn cached_checkpoint_state_reproves_after_realtime_image_hydration() {
        // Hydration verifies the blob id against content, so the fixture id
        // must be the canonical content-addressed one — which also makes
        // hydration digest-INVARIANT (the canonical digest form of an image
        // IS its content-addressed identity, see
        // `canonicalize_digest_image_blocks`). The seam's obligation is
        // therefore not a mismatch: it exposed the message buffer for
        // mutation, so it must clear the seal and force the next cached
        // read to RE-PROVE the stamp against current bytes instead of
        // serving the pre-hydration seal.
        let blob_id = crate::blob::content_blob_id("image/png", "iVBORw0KGgo=");
        let store = StubBlobStore::with_payload(crate::blob::BlobPayload {
            blob_id: blob_id.clone(),
            media_type: "image/png".to_string(),
            data: "iVBORw0KGgo=".to_string(),
        });
        let mut session = stamped_session_with_image(crate::types::ImageData::Blob { blob_id });
        assert!(matches!(
            session.try_checkpoint_state_cached().expect("seed seal"),
            SessionCheckpointState::Verified(_)
        ));
        let sealed_reads = session_content_digest_computations();
        assert!(matches!(
            session.try_checkpoint_state_cached().expect("sealed read"),
            SessionCheckpointState::Verified(_)
        ));
        assert_eq!(
            session_content_digest_computations(),
            sealed_reads,
            "steady-state sealed reads must not recompute digests"
        );
        session
            .hydrate_realtime_user_images_with_usage(&store, 1024 * 1024)
            .await
            .expect("hydrate blob-backed image");
        let before_reproof = session_content_digest_computations();
        assert!(matches!(
            session.try_checkpoint_state_cached().expect("re-proof"),
            SessionCheckpointState::Verified(_)
        ));
        assert!(
            session_content_digest_computations() > before_reproof,
            "hydration exposed the buffer for in-place mutation, so the \
             cached seam must re-prove the stamp, not trust the stale seal"
        );
    }

    #[test]
    fn cached_checkpoint_state_fails_closed_after_metadata_backfill() {
        let mut session = stamped_root(&session_with_text("seal backfill"));
        assert!(matches!(
            session.try_checkpoint_state_cached().expect("seed seal"),
            SessionCheckpointState::Verified(_)
        ));
        assert!(session.backfill_metadata_if_absent("compat_projection", serde_json::json!("v1")));
        assert!(matches!(
            session.try_checkpoint_state_cached(),
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
        let messages = reconstructed.messages.mutate_in_place();
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

    #[test]
    fn legacy_transcript_relation_classifies_prefix_extension_and_divergence() {
        let snapshot = session_with_text("turn one");

        let identical = snapshot.clone();
        assert_eq!(
            legacy_session_transcript_relation(&snapshot, &identical).expect("identical relation"),
            LegacySessionTranscriptRelation::Identical
        );

        let mut extended = snapshot.clone();
        extended.push(Message::User(UserMessage::text("turn two".to_string())));
        assert_eq!(
            legacy_session_transcript_relation(&snapshot, &extended).expect("extension relation"),
            LegacySessionTranscriptRelation::ProjectionExtendsSnapshot
        );
        assert_eq!(
            legacy_session_transcript_relation(&extended, &snapshot)
                .expect("stale projection relation"),
            LegacySessionTranscriptRelation::SnapshotExtendsProjection
        );

        let mut divergent = snapshot.clone();
        divergent.messages.mutate_in_place().clear();
        divergent.push(Message::User(UserMessage::text(
            "a different turn one".to_string(),
        )));
        assert_eq!(
            legacy_session_transcript_relation(&snapshot, &divergent).expect("divergent relation"),
            LegacySessionTranscriptRelation::Divergent
        );
    }

    #[test]
    fn legacy_snapshot_vs_typed_projection_relation_classifies_prefix_extension_and_divergence() {
        let snapshot = session_with_text("turn one");

        let identical = stamped_root(&snapshot);
        assert_eq!(
            legacy_snapshot_vs_typed_projection_transcript_relation(&snapshot, &identical)
                .expect("identical relation"),
            LegacySessionTranscriptRelation::Identical
        );

        let mut extended = snapshot.clone();
        extended.push(Message::User(UserMessage::text("turn two".to_string())));
        let typed_extended = stamped_root(&extended);
        assert_eq!(
            legacy_snapshot_vs_typed_projection_transcript_relation(&snapshot, &typed_extended)
                .expect("extension relation"),
            LegacySessionTranscriptRelation::ProjectionExtendsSnapshot
        );

        let typed_prefix = stamped_root(&snapshot);
        assert_eq!(
            legacy_snapshot_vs_typed_projection_transcript_relation(&extended, &typed_prefix)
                .expect("stale typed projection relation"),
            LegacySessionTranscriptRelation::SnapshotExtendsProjection
        );

        let mut divergent = snapshot.clone();
        divergent.messages.mutate_in_place().clear();
        divergent.push(Message::User(UserMessage::text(
            "a different turn one".to_string(),
        )));
        let typed_divergent = stamped_root(&divergent);
        assert_eq!(
            legacy_snapshot_vs_typed_projection_transcript_relation(&snapshot, &typed_divergent)
                .expect("divergent relation"),
            LegacySessionTranscriptRelation::Divergent
        );
    }

    #[test]
    fn legacy_snapshot_vs_typed_projection_relation_refuses_wrong_checkpoint_states() {
        let legacy = session_with_text("legacy copy");
        let typed = stamped_root(&legacy);

        // A typed snapshot side is refused: that shape belongs to
        // session_checkpoint_relation (typed pairs) or the rebuild arm.
        assert!(matches!(
            legacy_snapshot_vs_typed_projection_transcript_relation(&typed, &typed),
            Err(SessionCheckpointError::AuthorityBaseConflict(_))
        ));
        // A legacy projection side is refused: both-legacy pairs use
        // legacy_session_transcript_relation.
        assert!(matches!(
            legacy_snapshot_vs_typed_projection_transcript_relation(&legacy, &legacy),
            Err(SessionCheckpointError::AuthorityBaseConflict(_))
        ));

        let foreign = stamped_root(&session_with_text("legacy copy"));
        assert!(matches!(
            legacy_snapshot_vs_typed_projection_transcript_relation(&legacy, &foreign),
            Err(SessionCheckpointError::SessionIdMismatch { .. })
        ));
    }

    #[test]
    fn adopt_legacy_session_stamps_blob_and_refuses_typed_documents() {
        let legacy = session_with_text("legacy blob");
        let blob = serde_json::to_vec(&legacy).expect("legacy session should serialize");
        let adopted = adopt_legacy_session(
            &blob,
            SessionGeneration::INITIAL,
            SessionCheckpointRevision::INITIAL,
        )
        .expect("legacy blob should adopt");
        assert_eq!(
            adopted.stamp.provenance(),
            SessionCheckpointProvenance::RecoveryMigration
        );
        assert_eq!(adopted.stamp.generation(), SessionGeneration::INITIAL);
        let reloaded: Session =
            serde_json::from_slice(&adopted.serialized).expect("adopted bytes should decode");
        assert!(matches!(
            reloaded
                .try_checkpoint_state()
                .expect("adopted checkpoint state should decode"),
            SessionCheckpointState::Verified(stamp) if stamp == adopted.stamp
        ));

        let typed_blob =
            serde_json::to_vec(&stamped_root(&legacy)).expect("typed session should serialize");
        assert!(matches!(
            adopt_legacy_session(
                &typed_blob,
                SessionGeneration::INITIAL,
                SessionCheckpointRevision::INITIAL,
            ),
            Err(SessionCheckpointError::AuthorityBaseConflict(_))
        ));
    }

    #[test]
    fn legacy_transcript_relation_refuses_typed_documents_and_foreign_sessions() {
        let legacy = session_with_text("legacy copy");
        let typed = stamped_root(&session_with_text("typed copy"));
        assert!(matches!(
            legacy_session_transcript_relation(&typed, &legacy),
            Err(SessionCheckpointError::AuthorityBaseConflict(_))
        ));
        assert!(matches!(
            legacy_session_transcript_relation(&legacy, &typed),
            Err(SessionCheckpointError::AuthorityBaseConflict(_))
        ));

        let foreign = session_with_text("legacy copy");
        assert!(matches!(
            legacy_session_transcript_relation(&legacy, &foreign),
            Err(SessionCheckpointError::SessionIdMismatch { .. })
        ));
    }
}
