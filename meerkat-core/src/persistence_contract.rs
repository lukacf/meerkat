//! Small, store-issued session-persistence carriers.
//!
//! These values cross the core/runtime boundary without entering [`Session`].
//! They name provisional physical state only; a store must compare every field
//! with its exact current rows before promotion.

use crate::error::AgentError;
use crate::lifecycle::RunId;
use crate::types::SessionId;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProvisionalTailAuthorityError {
    #[error("provisional tail for session {session_id} has invalid physical identity: {detail}")]
    Invalid {
        session_id: SessionId,
        detail: String,
    },
}

/// Store-issued identity of one uncommitted WholeBlob candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WholeBlobProvisionalTailAuthority {
    session_id: SessionId,
    base_store_revision: u64,
    base_blob_sha256: String,
    run_id: RunId,
    candidate_blob_sha256: String,
    candidate_sequence: u64,
}

impl WholeBlobProvisionalTailAuthority {
    pub fn issued(
        session_id: SessionId,
        base_store_revision: u64,
        base_blob_sha256: String,
        run_id: RunId,
        candidate_blob_sha256: String,
        candidate_sequence: u64,
    ) -> Result<Self, ProvisionalTailAuthorityError> {
        if base_store_revision == 0
            || base_blob_sha256.is_empty()
            || candidate_blob_sha256.is_empty()
            || candidate_sequence == 0
        {
            return Err(ProvisionalTailAuthorityError::Invalid {
                session_id,
                detail: "WholeBlob authority requires a nonzero base revision and sequence plus nonempty base/candidate digests".to_string(),
            });
        }
        Ok(Self {
            session_id,
            base_store_revision,
            base_blob_sha256,
            run_id,
            candidate_blob_sha256,
            candidate_sequence,
        })
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn base_store_revision(&self) -> u64 {
        self.base_store_revision
    }

    #[must_use]
    pub fn base_blob_sha256(&self) -> &str {
        &self.base_blob_sha256
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
    pub const fn candidate_sequence(&self) -> u64 {
        self.candidate_sequence
    }
}

/// Store-issued identity of one uncommitted HeadCanonical physical tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadCanonicalProvisionalTailAuthority {
    authority_version: u16,
    session_id: SessionId,
    base_store_revision: u64,
    base_committed_head_token: String,
    physical_store_revision: u64,
    physical_head_token: String,
    run_id: RunId,
    candidate_sequence: u64,
}

impl HeadCanonicalProvisionalTailAuthority {
    pub const VERSION: u16 = 1;

    #[allow(clippy::too_many_arguments)]
    pub fn issued(
        session_id: SessionId,
        base_store_revision: u64,
        base_committed_head_token: String,
        physical_store_revision: u64,
        physical_head_token: String,
        run_id: RunId,
        candidate_sequence: u64,
    ) -> Result<Self, ProvisionalTailAuthorityError> {
        let expected_physical_revision = base_store_revision
            .checked_add(candidate_sequence)
            .filter(|_| candidate_sequence != 0);
        if base_store_revision == 0
            || expected_physical_revision != Some(physical_store_revision)
            || base_committed_head_token.is_empty()
            || physical_head_token.is_empty()
            || base_committed_head_token == physical_head_token
        {
            return Err(ProvisionalTailAuthorityError::Invalid {
                session_id,
                detail: "HeadCanonical authority requires an exact committed base, a nonzero contiguous candidate sequence, and a distinct matching physical revision/token".to_string(),
            });
        }
        Ok(Self {
            authority_version: Self::VERSION,
            session_id,
            base_store_revision,
            base_committed_head_token,
            physical_store_revision,
            physical_head_token,
            run_id,
            candidate_sequence,
        })
    }

    #[must_use]
    pub const fn authority_version(&self) -> u16 {
        self.authority_version
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn base_store_revision(&self) -> u64 {
        self.base_store_revision
    }

    #[must_use]
    pub fn base_committed_head_token(&self) -> &str {
        &self.base_committed_head_token
    }

    #[must_use]
    pub const fn physical_store_revision(&self) -> u64 {
        self.physical_store_revision
    }

    #[must_use]
    pub fn physical_head_token(&self) -> &str {
        &self.physical_head_token
    }

    #[must_use]
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    #[must_use]
    pub const fn candidate_sequence(&self) -> u64 {
        self.candidate_sequence
    }
}

/// Profile-specific authority for one successful provisional physical write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunCheckpointAuthority {
    WholeBlob(WholeBlobProvisionalTailAuthority),
    HeadCanonical(HeadCanonicalProvisionalTailAuthority),
}

impl RunCheckpointAuthority {
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::WholeBlob(authority) => authority.session_id(),
            Self::HeadCanonical(authority) => authority.session_id(),
        }
    }

    #[must_use]
    pub fn run_id(&self) -> &RunId {
        match self {
            Self::WholeBlob(authority) => authority.run_id(),
            Self::HeadCanonical(authority) => authority.run_id(),
        }
    }

    #[must_use]
    pub const fn candidate_sequence(&self) -> u64 {
        match self {
            Self::WholeBlob(authority) => authority.candidate_sequence(),
            Self::HeadCanonical(authority) => authority.candidate_sequence(),
        }
    }

    #[must_use]
    pub fn whole_blob(&self) -> Option<&WholeBlobProvisionalTailAuthority> {
        match self {
            Self::WholeBlob(authority) => Some(authority),
            Self::HeadCanonical(_) => None,
        }
    }

    #[must_use]
    pub fn head_canonical(&self) -> Option<&HeadCanonicalProvisionalTailAuthority> {
        match self {
            Self::WholeBlob(_) => None,
            Self::HeadCanonical(authority) => Some(authority),
        }
    }
}

/// Latest successful provisional physical write for one active run.
///
/// The store binds the profile authority to the exact candidate's bounded
/// logical facts when it mints the receipt. Promotion must revalidate all three
/// against the same physical candidate; the authority alone is insufficient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCheckpointReceipt {
    authority: RunCheckpointAuthority,
    conversation_digest: String,
    message_count: u64,
}

impl RunCheckpointReceipt {
    pub fn issued(
        authority: RunCheckpointAuthority,
        conversation_digest: String,
        message_count: u64,
    ) -> Result<Self, ProvisionalTailAuthorityError> {
        let session_id = authority.session_id().clone();
        if conversation_digest.is_empty() {
            return Err(ProvisionalTailAuthorityError::Invalid {
                session_id,
                detail: "run checkpoint receipt requires a nonempty conversation digest"
                    .to_string(),
            });
        }
        if usize::try_from(message_count).is_err() {
            return Err(ProvisionalTailAuthorityError::Invalid {
                session_id,
                detail: "run checkpoint receipt message count exceeds the host index range"
                    .to_string(),
            });
        }
        Ok(Self {
            authority,
            conversation_digest,
            message_count,
        })
    }

    #[must_use]
    pub fn authority(&self) -> &RunCheckpointAuthority {
        &self.authority
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        self.authority.session_id()
    }

    #[must_use]
    pub fn run_id(&self) -> &RunId {
        self.authority.run_id()
    }

    #[must_use]
    pub const fn candidate_sequence(&self) -> u64 {
        self.authority.candidate_sequence()
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
    pub fn whole_blob(&self) -> Option<&WholeBlobProvisionalTailAuthority> {
        self.authority.whole_blob()
    }

    #[must_use]
    pub fn head_canonical(&self) -> Option<&HeadCanonicalProvisionalTailAuthority> {
        self.authority.head_canonical()
    }
}

/// Periodic session persistence hook.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionCheckpointer: Send + Sync {
    /// Persist one in-run physical successor.
    ///
    /// `previous` is the exact last successful receipt for this session/run.
    /// The actor removes its retained copy before awaiting and installs only a
    /// successfully returned successor, so cancellation or error cannot promote
    /// stale physical state.
    async fn checkpoint_run(
        &self,
        session: &mut crate::Session,
        run_id: &RunId,
        previous: Option<&RunCheckpointReceipt>,
    ) -> Result<Option<RunCheckpointReceipt>, AgentError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_canonical_authority_requires_exact_base_plus_candidate_sequence() {
        let session_id = SessionId::new();
        let run_id = RunId::new();
        let issued = HeadCanonicalProvisionalTailAuthority::issued(
            session_id.clone(),
            41,
            "committed".to_string(),
            44,
            "physical".to_string(),
            run_id.clone(),
            3,
        )
        .expect("base 41 plus sequence 3 is physical revision 44");
        assert_eq!(issued.physical_store_revision(), 44);
        assert_eq!(issued.candidate_sequence(), 3);

        for (physical_revision, physical_token, sequence) in [
            (43, "physical", 3),
            (44, "physical", 0),
            (44, "", 3),
            (44, "committed", 3),
        ] {
            assert!(
                HeadCanonicalProvisionalTailAuthority::issued(
                    session_id.clone(),
                    41,
                    "committed".to_string(),
                    physical_revision,
                    physical_token.to_string(),
                    run_id.clone(),
                    sequence,
                )
                .is_err()
            );
        }
        assert!(
            HeadCanonicalProvisionalTailAuthority::issued(
                session_id,
                41,
                String::new(),
                44,
                "physical".to_string(),
                run_id,
                3,
            )
            .is_err()
        );
    }

    #[test]
    fn run_checkpoint_receipt_binds_authority_digest_and_count() {
        let authority = WholeBlobProvisionalTailAuthority::issued(
            SessionId::new(),
            7,
            "base-blob".to_string(),
            RunId::new(),
            "candidate-blob".to_string(),
            1,
        )
        .expect("valid WholeBlob authority");
        let receipt = RunCheckpointReceipt::issued(
            RunCheckpointAuthority::WholeBlob(authority),
            "conversation".to_string(),
            9,
        )
        .expect("bounded candidate facts");

        assert!(receipt.whole_blob().is_some());
        assert!(receipt.head_canonical().is_none());
        assert_eq!(receipt.conversation_digest(), "conversation");
        assert_eq!(receipt.message_count(), 9);
        assert_eq!(receipt.candidate_sequence(), 1);
    }

    #[test]
    fn run_checkpoint_receipt_rejects_empty_conversation_digest() {
        let authority = WholeBlobProvisionalTailAuthority::issued(
            SessionId::new(),
            7,
            "base-blob".to_string(),
            RunId::new(),
            "candidate-blob".to_string(),
            1,
        )
        .expect("valid WholeBlob authority");

        assert!(
            RunCheckpointReceipt::issued(
                RunCheckpointAuthority::WholeBlob(authority),
                String::new(),
                9,
            )
            .is_err()
        );
    }
}
