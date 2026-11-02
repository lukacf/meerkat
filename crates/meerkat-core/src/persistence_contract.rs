//! Small, store-issued session-persistence carriers.
//!
//! These values cross the core/runtime boundary without entering [`crate::Session`].
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
    /// Rebind the actor's fixed-size committed base after a runtime-owned
    /// control-only store mutation.
    ///
    /// The receipt is minted from the store outcome that committed the
    /// control mutation. Implementations must confirm that exact authority;
    /// they must not reload or compare the accumulated Session document.
    /// `session` is the not-yet-published live successor: incremental
    /// (HeadCanonical) actors adopt the committed control-metadata identity
    /// as their new preparation baseline through it.
    async fn acknowledge_control_commit(
        &self,
        _session: &mut crate::Session,
        _receipt: &SessionControlCommitReceipt,
    ) -> Result<(), AgentError> {
        Err(AgentError::InternalError(
            "session checkpointer cannot acknowledge runtime control commits".to_string(),
        ))
    }

    /// Persist one in-run physical successor.
    ///
    /// `previous` is the exact last successful receipt for this session/run.
    /// The actor removes its retained copy before awaiting and installs only a
    /// successfully returned successor, so cancellation or error cannot promote
    /// stale physical state.
    ///
    /// The agent calls it twice in every tool round of a run (the round's
    /// commits, which carry the session's run cursor), once more when the run
    /// ends without an end-of-run save (a closing checkpoint without the
    /// cursor), and for its end-of-run save. `Ok(None)` means nothing was
    /// made durable: a round's first commit that gets it fails the round
    /// before any of its tools runs. Only a session that never committed a
    /// boundary has no base to chain a provisional tail from.
    ///
    /// A round commit or a closing save refused while the writer of a fenced
    /// store is stopped may wait for its re-acquire, as
    /// [`Self::dispatch_fence_ready`] does, and be written again once. When
    /// the writer stays stopped (a store fence gap), a checkpointer whose
    /// host dispatched the run may have it suspended at a round commit
    /// instead of answering, so the run resumes from its last round commit
    /// and does the refused round again; a closing save, written after the
    /// run's terminal, answers its refusal.
    async fn checkpoint_run(
        &self,
        session: &mut crate::Session,
        run_id: &RunId,
        previous: Option<&RunCheckpointReceipt>,
    ) -> Result<Option<RunCheckpointReceipt>, AgentError>;

    /// Whether this checkpointer re-bases a run whose transcript a compaction
    /// rewrote ([`Self::rebase_run`]).
    ///
    /// A session recovered only from strict continuations of its committed
    /// transcript (WholeBlob) cannot take a plain round commit after a
    /// rewrite: recovery would refuse the tail. A checkpointer that answers
    /// `true` turns the run's first round commit after a rewrite into a
    /// re-base. The default answers `false`: such a run then takes no round
    /// commit after its first rewrite, and its last round commit before it
    /// stays its resume point.
    fn rebases_runs(&self) -> bool {
        false
    }

    /// Write a run's checkpoint as a re-base, in one store call: the
    /// session, without its run cursor, becomes the session's committed
    /// base, and the run's provisional tail restarts on it as the same
    /// document, with the cursor when the session carries one (a round
    /// commit) and without it otherwise (an end-of-run or closing save).
    ///
    /// The agent asks, when [`Self::rebases_runs`] answers `true`, for the
    /// run's first round commit or save after a rewrite of a session
    /// recovered only from strict continuations. `previous` is the run's
    /// latest receipt, if any; the returned receipt names the new tail, with
    /// candidate sequence 1 on the new base, and the run's next checkpoints
    /// chain from it. Any error fails the write: a round whose first commit
    /// fails dispatches nothing.
    async fn rebase_run(
        &self,
        session: &mut crate::Session,
        _run_id: &RunId,
        _previous: Option<&RunCheckpointReceipt>,
    ) -> Result<RunCheckpointReceipt, AgentError> {
        Err(AgentError::InternalError(format!(
            "the session checkpointer of session {} does not re-base runs",
            session.id()
        )))
    }

    /// Whether this writer may still act on the session now, read without
    /// waiting.
    ///
    /// A checkpointer over a host store answers from its fence handle's
    /// self-fence window: past it, another process may already own the
    /// session, and nothing this process dispatches may run. A refusal
    /// returns the teardown-required class
    /// ([`AgentError::session_durable_projection_authority_unknown`]) so the
    /// live actor reloads instead of continuing on stale authority. Called
    /// synchronously, with no I/O. The agent asks
    /// [`Self::dispatch_fence_ready`], whose default asks this check. The
    /// default allows.
    fn dispatch_fence_check(&self) -> Result<(), AgentError> {
        Ok(())
    }

    /// Whether this writer may dispatch now, after any wait its fence needs.
    ///
    /// The agent awaits it right before each model request, right after a
    /// tool round's first commit, before any tool of the round is admitted,
    /// and right before each tool call starts, because a pause between a
    /// commit and a dispatch writes nothing that would meet the write path's
    /// own fence check. An error stops the run there, as a refused commit
    /// does: no later call of the round starts and the round's results are
    /// not committed.
    ///
    /// A checkpointer over a host store whose writer stopped (a renew failed
    /// or timed out, or its window closed) waits here, bounded, for the
    /// fresh acquire the writer makes before its next write
    /// ([`crate::host_store::SelfFence::dispatch_window`]), and allows once
    /// that acquire resumed the writer. When another writer is proven
    /// ([`crate::host_store::SelfFence::lost`]), it refuses as
    /// [`Self::dispatch_fence_check`] does, with the teardown class. When no
    /// acquire resumed the writer within the wait and no other writer is
    /// proven (a store fence gap), nothing is lost yet: `suspendable_run` is
    /// the run asking, when the agent could suspend it (a run that takes
    /// round commits and has not written its closing save), and a
    /// checkpointer whose host dispatched that run may have it suspended
    /// there instead of answering, so it resumes from its last round commit
    /// (or starts again from its inputs, before its first) once the fence is
    /// back. Otherwise it refuses with a retryable error, which requires no
    /// teardown. The default asks [`Self::dispatch_fence_check`] and never
    /// waits.
    async fn dispatch_fence_ready(
        &self,
        suspendable_run: Option<&RunId>,
    ) -> Result<(), AgentError> {
        let _ = suspendable_run;
        self.dispatch_fence_check()
    }

    /// Record how far the live session retired its transcript history once
    /// this run's latest checkpoint moved the durable head over its rewrites.
    ///
    /// Checkpoints prune no stored rewrite rows while the run is open; the
    /// boundary commit that promotes the run applies this cut to them. The
    /// default records nothing.
    fn note_run_transcript_retirement(&self, _run_id: &RunId, _retired_occurrences: u64) {}
}

/// Fixed-size store authority returned by a runtime-owned control mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionControlCommitReceipt {
    session_id: SessionId,
    store_revision: u64,
    authority_token: String,
}

impl SessionControlCommitReceipt {
    pub fn new(
        session_id: SessionId,
        store_revision: u64,
        authority_token: impl Into<String>,
    ) -> Result<Self, String> {
        let authority_token = authority_token.into();
        if store_revision == 0 {
            return Err("session control commit revision must be non-zero".to_string());
        }
        if authority_token.is_empty() {
            return Err("session control commit authority token must be non-empty".to_string());
        }
        Ok(Self {
            session_id,
            store_revision,
            authority_token,
        })
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn store_revision(&self) -> u64 {
        self.store_revision
    }

    #[must_use]
    pub fn authority_token(&self) -> &str {
        &self.authority_token
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
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

    #[test]
    fn session_control_commit_receipt_requires_bounded_store_authority() {
        let session_id = SessionId::new();
        let receipt = SessionControlCommitReceipt::new(session_id.clone(), 7, "row-sha256:exact")
            .expect("non-zero revision and non-empty token should be accepted");
        assert_eq!(receipt.session_id(), &session_id);
        assert_eq!(receipt.store_revision(), 7);
        assert_eq!(receipt.authority_token(), "row-sha256:exact");

        assert!(
            SessionControlCommitReceipt::new(session_id.clone(), 0, "row-sha256:exact").is_err()
        );
        assert!(SessionControlCommitReceipt::new(session_id, 7, "").is_err());
    }
}
