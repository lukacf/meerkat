//! Provider contracts for identity-first continuity.
//!
//! Five provider traits define the extension points:
//! - [`ContinuityStore`] — authoritative durable state (CONTRACT-01)
//! - [`LeaseProvider`] — live ownership / fencing tokens (CONTRACT-02)
//! - [`RosterProvider`] — roster/discovery (CONTRACT-03)
//! - [`AgentCustomizer`] — build-time agent customization (CONTRACT-04)
//! - [`TopologyProvider`] — managed dynamic topology edges (CONTRACT-05)

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::types::{
    AgentBuildContext, AgentBuildDraft, AgentIdentity, CheckpointVersion, ContinuityGeneration,
    ContinuityRecord, ContinuityResolveState, ContinuityStoreError, CustomizerError,
    DurableAgentSpec, FencingToken, LeaseAcquireResult, LeaseError, LeaseGrant, LeaseRenewResult,
    ManagedPeerEdge, RosterContext, RosterError, SessionSnapshot, TopologyContext, TopologyError,
};
use crate::mob_handle_runtime::SessionCreatedContext;

// ---------------------------------------------------------------------------
// CONTRACT-01: ContinuityStore
// ---------------------------------------------------------------------------

/// Candidate used by stores that can prove an exact, provenance-preserving
/// snapshot no-op without returning and reparsing the durable document.
///
/// A store may report a match only when the bytes, identity, generation, and
/// checkpoint revision identify its durable snapshot and the presented fence
/// is current write authority. The snapshot's stored fence is historical
/// provenance: it may precede the current lease epoch, but may never exceed
/// it. Implementations that cannot prove the complete predicate must use the
/// trait's default `false` response.
#[derive(Debug, Clone)]
pub struct SessionSnapshotMatchCandidate {
    pub identity: AgentIdentity,
    pub session_id: meerkat_core::types::SessionId,
    pub generation: ContinuityGeneration,
    pub checkpoint_version: CheckpointVersion,
    pub fencing_token: FencingToken,
    pub snapshot: Arc<SessionSnapshot>,
}

/// Authoritative durable state provider for identity-first continuity.
///
/// Implementations are responsible for persisting `ContinuityRecord`s and
/// `SessionSnapshot`s. The store treats `FencingToken` as an opaque monotonic
/// write-precondition — stale tokens are rejected via compare-and-set.
/// `CheckpointVersion` is the monotonic snapshot/version counter for
/// `(AgentIdentity, ContinuityGeneration)`: it advances across session
/// rotations and resets only when a destructive continuity reset advances the
/// generation. Session-local snapshot storage may still be keyed by
/// `SessionId`, but stale snapshot rejection must compare against the current
/// identity/generation head rather than treating a rebind as a fresh version
/// stream.
///
/// `resolve_many` MUST return an entry for every requested identity. Missing
/// entries are treated as a provider error, not implicit `Uninitialized`.
#[async_trait]
pub trait ContinuityStore: Send + Sync {
    /// Enable immutable identity binding history before extension tools run.
    /// Repeated calls must preserve the same history and succeed idempotently.
    /// Unsupported providers must refuse rather than classify history as workers.
    #[cfg(feature = "extension-state")]
    async fn enable_identity_binding_history(&self) -> Result<(), ContinuityStoreError> {
        Err(ContinuityStoreError::UnsupportedAuthority {
            capability: "identity binding history",
        })
    }

    /// The identity authority's retained binding, including rotated or deleted
    /// session executions. None proves no retained binding; it does not prove a
    /// historical session was a worker. Callers must distinguish live absence.
    #[cfg(feature = "extension-state")]
    async fn historical_identity_binding(
        &self,
        _session_id: &meerkat_core::SessionId,
    ) -> Result<Option<AgentIdentity>, ContinuityStoreError> {
        Err(ContinuityStoreError::UnsupportedAuthority {
            capability: "identity binding history",
        })
    }

    /// Stable identity intent retained even when a provisional session launch
    /// never published its actual session binding. This only excludes a target
    /// from Worker classification; it never proves an exact-session principal.
    #[cfg(feature = "extension-state")]
    async fn has_historical_identity(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<bool, ContinuityStoreError> {
        Err(ContinuityStoreError::UnsupportedAuthority {
            capability: "identity target reservation",
        })
    }

    /// Establish the earliest covered journal cursor once, after history is
    /// enabled and before members can execute. Restart preserves the anchor.
    #[cfg(feature = "extension-state")]
    async fn establish_identity_history_coverage(
        &self,
        _mob: &str,
        _cursor: u64,
    ) -> Result<(), ContinuityStoreError> {
        Err(ContinuityStoreError::UnsupportedAuthority {
            capability: "identity history coverage",
        })
    }

    /// True proves binding history covers the session's canonical birth event.
    #[cfg(feature = "extension-state")]
    async fn identity_history_covers_birth(
        &self,
        _mob: &str,
        _birth_cursor: u64,
    ) -> Result<bool, ContinuityStoreError> {
        Err(ContinuityStoreError::UnsupportedAuthority {
            capability: "identity history coverage",
        })
    }
    /// Resolve continuity state for the given identities.
    ///
    /// Returns a `BTreeMap` with one entry per input identity.
    async fn resolve_many(
        &self,
        identities: &[AgentIdentity],
    ) -> Result<BTreeMap<AgentIdentity, ContinuityResolveState>, ContinuityStoreError>;

    /// The continuity record currently BINDING this session, with its stored
    /// fencing token and the SUBSTRATE's current checkpoint version for the
    /// session (the max across the session's snapshot/head rows - the
    /// version fence a write cursor must advance past): durable write
    /// authority for a session whose in-memory registration is gone (task
    /// #56 parked repair - the projection doors repairing a parked member's
    /// torn durable head hydrate their write cursor from these facts, the
    /// same facts registration would carry). The record's OWN checkpoint
    /// version is a checkpoint-time stamp and may trail the substrate's
    /// write counter; the third element is the fence-current value.
    ///
    /// `None` when no record binds the session (never-registered sessions,
    /// rotated-away sessions) or when the substrate does not support the
    /// lookup - callers must then keep their registration-required refusal.
    /// Resolve the record bound to `session_id`, with the facts a
    /// registration would carry.
    ///
    /// REQUIRED, deliberately. This used to default to `Ok(None)`, which made
    /// "authoritatively no record" and "this store cannot answer"
    /// indistinguishable at every call site. Both owner-authority passes read
    /// this to decide whether an identity has a durable owner, and both treat
    /// absence as ordinary and skip - so a store that merely inherited the
    /// default silently caused owner pre-registration to do nothing, and the
    /// prepare-time durable-tail refusal became unclearable. The gateway
    /// bridge inherited exactly that. An implementor that has no answer must
    /// now say so in its own words rather than borrow a negative that reads as
    /// fact.
    async fn resolve_record_by_session(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<(ContinuityRecord, FencingToken, CheckpointVersion)>, ContinuityStoreError>;

    /// Return the immutable owner of this exact session, including retained history.
    ///
    /// The default supports current bindings only. Stores retaining rotated-away
    /// sessions must override this method using their durable session ownership
    /// records; a missing current binding does not establish a historical owner.
    /// Conflicting ownership records must fail rather than select one identity.
    async fn session_owner(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<AgentIdentity>, ContinuityStoreError> {
        let Some((record, _, _)) = self.resolve_record_by_session(session_id).await? else {
            return Ok(None);
        };
        if record.session_id != *session_id {
            return Err(ContinuityStoreError::Corruption(
                "session owner lookup returned a different session".to_string(),
            ));
        }
        Ok(Some(record.identity))
    }

    /// Load a previously saved session snapshot.
    async fn load_session_snapshot(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<SessionSnapshot>, ContinuityStoreError>;

    /// Return `true` only when `candidate` is byte-for-byte identical to the
    /// current durable row and its identity, generation, and checkpoint
    /// version match. `candidate.fencing_token` must equal current write
    /// authority; the row's historical fence may be older but not newer.
    ///
    /// This additive capability lets adapters skip a full document load and
    /// parse for an already-durable save. The conservative default preserves
    /// compatibility and correctness for external stores.
    async fn session_snapshot_matches_current(
        &self,
        _candidate: SessionSnapshotMatchCandidate,
    ) -> Result<bool, ContinuityStoreError> {
        Ok(false)
    }

    /// Delete a saved session snapshot only if its serialized session
    /// projection still matches `expected_current_revision`.
    ///
    /// Implementations that cannot support session-scoped snapshot deletion
    /// must return `Ok(false)` rather than reporting a successful no-op.
    async fn delete_session_snapshot_if_current_revision(
        &self,
        _session_id: &meerkat_core::types::SessionId,
        _expected_current_revision: &str,
    ) -> Result<bool, ContinuityStoreError> {
        Ok(false)
    }

    /// Save a session snapshot with fencing and identity-generation version preconditions.
    async fn save_session_snapshot(
        &self,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        generation: ContinuityGeneration,
        version: CheckpointVersion,
        fencing_token: FencingToken,
        snapshot: &SessionSnapshot,
    ) -> Result<(), ContinuityStoreError>;

    /// Owned equivalent of [`Self::save_session_snapshot`].
    ///
    /// Stores backed by blocking workers can override this method to move a
    /// large snapshot into the worker without cloning it. Existing providers
    /// inherit the compatibility implementation.
    async fn save_session_snapshot_owned(
        &self,
        identity: AgentIdentity,
        session_id: meerkat_core::types::SessionId,
        generation: ContinuityGeneration,
        version: CheckpointVersion,
        fencing_token: FencingToken,
        snapshot: SessionSnapshot,
    ) -> Result<(), ContinuityStoreError> {
        self.save_session_snapshot(
            &identity,
            &session_id,
            generation,
            version,
            fencing_token,
            &snapshot,
        )
        .await
    }

    /// Upsert a continuity record with fencing precondition.
    ///
    /// Rebinding an identity to a new session without changing
    /// `ContinuityGeneration` must not rewind `record.checkpoint_version`.
    async fn upsert_continuity_record(
        &self,
        record: &ContinuityRecord,
        fencing_token: FencingToken,
    ) -> Result<(), ContinuityStoreError>;

    /// Compensate a tentative continuity-generation advance only when the
    /// durable row still belongs to that exact reset attempt.
    ///
    /// Ordinary upserts are generation-monotonic and must reject an older
    /// generation even under a newer fence. Reset is the one workflow that
    /// can need a compensating rollback after publishing a provisional row so
    /// the persistent session service can construct the replacement. Restore
    /// therefore has to bypass monotonicity, and the only non-monotonic write
    /// verb the trait offers is deletion — the compatibility implementation
    /// deletes the attempt row (after validating it is still the attempt)
    /// and re-upserts `previous` as a fresh insert, which every conforming
    /// generation-monotonic store accepts.
    ///
    /// Documented compatibility caveats (why stores should override this
    /// with one atomic compare-and-swap transaction, as
    /// `LocalContinuityStore` does):
    ///
    /// - **Non-atomic**: a crash or a concurrent writer between the delete
    ///   and the re-upsert can leave the identity `Uninitialized`.
    /// - **Prior-generation snapshots are lost**: `delete_continuity_record`
    ///   removes the identity's session snapshots along with the record, so
    ///   the restored `previous` row comes back without its rollback-
    ///   authority snapshots; they must be re-established by the next
    ///   checkpoint. An atomic override deletes only the provisional
    ///   generation's snapshots and retains the previous generation's.
    async fn rollback_continuity_record(
        &self,
        expected_attempt: &ContinuityRecord,
        previous: Option<&ContinuityRecord>,
        fencing_token: FencingToken,
    ) -> Result<(), ContinuityStoreError> {
        let resolved = self
            .resolve_many(std::slice::from_ref(&expected_attempt.identity))
            .await?;
        let Some(ContinuityResolveState::Ready { record: current }) =
            resolved.get(&expected_attempt.identity)
        else {
            return Err(ContinuityStoreError::NotFound {
                identity: expected_attempt.identity.clone(),
            });
        };
        if current.agent_runtime_id != expected_attempt.agent_runtime_id
            || current.session_id != expected_attempt.session_id
            || current.generation != expected_attempt.generation
        {
            return Err(ContinuityStoreError::StaleContinuityGeneration {
                identity: expected_attempt.identity.clone(),
                presented: expected_attempt.generation,
                current: current.generation,
            });
        }
        // Delete-then-reinsert: the delete abandons the provisional row under
        // the fence CAS; the re-upsert of `previous` is a fresh insert, so a
        // conforming store's generation monotonicity (measured against the
        // current row) admits the older generation.
        self.delete_continuity_record(&expected_attempt.identity, fencing_token)
            .await?;
        match previous {
            Some(previous) => self.upsert_continuity_record(previous, fencing_token).await,
            None => Ok(()),
        }
    }

    /// Delete a continuity record and associated session snapshots.
    ///
    /// After deletion, `resolve_many` for this identity returns `Uninitialized`.
    /// Rejects stale fencing tokens.
    async fn delete_continuity_record(
        &self,
        identity: &AgentIdentity,
        fencing_token: FencingToken,
    ) -> Result<(), ContinuityStoreError>;

    /// Typed, discoverable capability: a session-delta channel compatible
    /// with meerkat's incremental persistence contract (O(delta) appends +
    /// CAS-guarded small head writes instead of a whole session document per
    /// save).
    ///
    /// Contract for stores that advertise this (return `Some`):
    ///
    /// - The returned store is a **session-granular authority over the same
    ///   durable state** the whole-snapshot verbs serve: a session persisted
    ///   through the incremental channel must load back through
    ///   [`Self::load_session_snapshot`]-backed reads (the committed-
    ///   authority resolver path) byte-consistently, and the CAS revision
    ///   tokens consumed by
    ///   [`Self::delete_session_snapshot_if_current_revision`] must remain
    ///   derivable from the current head + rows.
    /// - Every mutation must be enforced under the store's own continuity
    ///   write discipline — fencing-token compare-and-set and per
    ///   `(identity, generation)` version monotonicity apply per append and
    ///   per head write, not just per whole-blob save.
    ///
    /// REQUIRED, with no default. A store returning `None` persists whole
    /// snapshots only: `ContinuitySessionStoreAdapter::as_incremental` stays
    /// `None` and meerkat writes the whole session document at every turn
    /// boundary. That must be a stated choice, never an inherited one: a
    /// default `None` is how a wrapper around an incremental-capable store
    /// (or a native store that never considered the channel) silently
    /// degraded to O(session) writes per turn. A decorator forwards its inner
    /// store's channel; a store without one returns `None` and says why.
    ///
    /// The bundled `LocalContinuityStore` advertises the channel (head+rows
    /// are its canonical durable session representation); the JSON-RPC
    /// `GatewayContinuityStore` returns `None` (its wire protocol has only
    /// whole-snapshot verbs).
    fn as_incremental_sessions(&self) -> Option<Arc<dyn ContinuityIncrementalSessions>>;

    /// The concrete store type, for operator-facing diagnostics such as the
    /// startup warning when a session store degrades to whole-blob saves.
    fn store_type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// The continuity write cursor a delta mutation is performed under.
///
/// Meerkat's `IncrementalSessionStore` verbs carry no continuity tuple, but
/// the advertiser contract above demands fencing-token compare-and-set and
/// per-`(identity, generation)` version monotonicity **per append and per
/// head write**. The adapter therefore presents the registered cursor with
/// every mutation, exactly as it does for whole-blob saves: the identity and
/// generation come from the session registry the identity runtime publishes,
/// `checkpoint_version` is minted by the adapter's one version allocator, and
/// `fencing_token` is the registered lease token.
/// The session envelope version every released 0.8.10 writer stamped into
/// its persisted documents and head rows. Not exported by the meerkat pin's
/// public surface; the one-time importer re-validates it, so this constant
/// only ROUTES refusals into the import/adoption lanes.
pub(crate) const RELEASED_0810_SESSION_ENVELOPE_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuityWriteCursor {
    pub identity: AgentIdentity,
    pub generation: ContinuityGeneration,
    pub checkpoint_version: CheckpointVersion,
    pub fencing_token: FencingToken,
}

/// Cursor-carrying session-delta channel: meerkat's incremental persistence
/// contract with the continuity write discipline threaded through every
/// mutation.
///
/// Verb semantics are meerkat's
/// [`IncrementalSessionStore`](meerkat_core::session_store::IncrementalSessionStore)
/// semantics verbatim — implementations reuse the published guard functions
/// (`validate_save_head_transition`, `validate_commit_rewrite_transition`,
/// `head_canonical_plain_save_guard`, `strand_layout_for_history`) so the
/// accept/reject boundary of a meta substrate can never differ from the
/// meerkat service's. The only additions are:
///
/// - a [`ContinuityWriteCursor`] on every mutating verb, enforced with the
///   same fence/version compare-and-set the whole-blob save verb applies;
/// - [`Self::load_canonical_head`], which reports whether a session is
///   head-canonical **without** synthesizing a head from a legacy blob (the
///   adapter needs that distinction to keep H3's byte custody on the blob
///   path).
///
/// Errors are meerkat `SessionStoreError`s: they flow to the session service
/// unchanged, and continuity-discipline failures map exactly as the
/// whole-blob path maps them today (stale fence / stale version / unknown
/// binding become `SessionStoreError::Internal`).
#[async_trait]
pub trait ContinuityIncrementalSessions: Send + Sync {
    async fn append_messages(
        &self,
        cursor: &ContinuityWriteCursor,
        id: &meerkat_core::types::SessionId,
        strand: &meerkat_core::session_store::TranscriptStrandId,
        base_seq: u64,
        messages: &[meerkat_core::types::Message],
    ) -> Result<(), meerkat_core::SessionStoreError>;

    async fn commit_rewrite(
        &self,
        cursor: &ContinuityWriteCursor,
        id: &meerkat_core::types::SessionId,
        record: &meerkat_core::TranscriptRewriteRecord,
        expected: meerkat_core::session_store::SessionHeadCas,
    ) -> Result<meerkat_core::session_store::SessionHead, meerkat_core::SessionStoreError>;

    async fn save_head(
        &self,
        cursor: &ContinuityWriteCursor,
        head: &meerkat_core::session_store::SessionHead,
        expected: meerkat_core::session_store::SessionHeadCas,
    ) -> Result<(), meerkat_core::SessionStoreError>;

    /// One-time durable adoption of a RELEASED 0.8.10 head-canonical
    /// document, authorized by the sanctioned import proof instead of by the
    /// stored head.
    ///
    /// A released head with retained rewrites structurally cannot authorize a
    /// current mutation (its rewrite-generation authority predates the
    /// compact graph/rewrite-prefix carriers; `session_head_cas_token`
    /// refuses it typed), so the ordinary write arms are unreachable for it.
    /// Implementations must, inside ONE write transaction: re-prove the
    /// stored released document through the one-time importer (the import
    /// receipt is the authorization), verify `session` is a legal successor
    /// of that imported reading (equal or append-extension; genuine
    /// divergence refuses typed), then replace the released representation
    /// wholesale with the current-format layout of `session`. Conservative
    /// default: refuse (external channels without the released corpus never
    /// take this lane).
    async fn adopt_released_head_document(
        &self,
        _cursor: &ContinuityWriteCursor,
        _session: &meerkat_core::Session,
    ) -> Result<(), meerkat_core::SessionStoreError> {
        Err(meerkat_core::SessionStoreError::Internal(
            "this continuity channel does not support released head-canonical adoption".to_string(),
        ))
    }

    /// Head-path sibling of `ContinuityStore::session_snapshot_matches_current`:
    /// does `head` equal the persisted head row for this session while
    /// `fencing_token` is STILL the identity's current write authority (same
    /// session binding and generation the mutating verbs enforce)?
    ///
    /// This is the ONLY probe that may turn an exact head resave into a
    /// no-op. A stale fence must report `false` — never a masked no-op — so
    /// the caller falls through to the fencing write verb and surfaces the
    /// ordinary stale-fence refusal a fenced-out writer must hear.
    /// Conservative default: `false` (callers take the ordinary guarded
    /// write path).
    async fn session_head_matches_current(
        &self,
        _identity: &AgentIdentity,
        _session_id: &meerkat_core::types::SessionId,
        _generation: ContinuityGeneration,
        _fencing_token: FencingToken,
        _head: &meerkat_core::session_store::SessionHead,
    ) -> Result<bool, meerkat_core::SessionStoreError> {
        Ok(false)
    }

    /// The head a reader should see: the persisted head row, or — for a
    /// session still stored as a legacy blob — the deterministic read-only
    /// synthesis of one (never a write). Mirrors meerkat's `load_head`.
    async fn load_head(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::session_store::SessionHead>, meerkat_core::SessionStoreError>;

    /// The persisted head row only: `None` means "this session is still
    /// blob-canonical". Never synthesizes.
    async fn load_canonical_head(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::session_store::SessionHead>, meerkat_core::SessionStoreError>;

    /// The head-canonical document, materialized from the head row and its
    /// strand rows **inside ONE substrate read transaction**. `None` means
    /// the session is still blob-canonical.
    ///
    /// Not `load_canonical_head` + `load_messages`. Head and rows are two
    /// halves of one document and a concurrent delta write advances both;
    /// reading them under two independent transactions can observe an old
    /// head against new rows (or the reverse), which materializes a torn
    /// document — `message_count` disagreeing with the rows behind it, or a
    /// `head_revision` that no longer digests the transcript it names.
    /// Implementations MUST take a single snapshot; there is deliberately
    /// no default body, because the obvious one is exactly the torn
    /// composition this method exists to replace.
    async fn load_canonical_session(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::Session>, meerkat_core::SessionStoreError>;

    /// The head-canonical document **and** its adopted rewrite commits,
    /// under ONE substrate read transaction — the `previous` shape the
    /// published head-canonical save guards expect.
    ///
    /// Same single-snapshot obligation as [`Self::load_canonical_session`],
    /// widened to the rewrite ledger: a guard that compared a fresh
    /// document against a stale commit list (or the reverse) would accept
    /// or refuse a save on a state that never existed.
    async fn load_canonical_previous(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<
        Option<(
            meerkat_core::Session,
            Vec<meerkat_core::TranscriptRewriteCommit>,
        )>,
        meerkat_core::SessionStoreError,
    >;

    async fn load_messages(
        &self,
        id: &meerkat_core::types::SessionId,
        strand: &meerkat_core::session_store::TranscriptStrandId,
        range: std::ops::Range<u64>,
    ) -> Result<Vec<meerkat_core::types::Message>, meerkat_core::SessionStoreError>;

    async fn load_rewrites(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Vec<meerkat_core::TranscriptRewriteRecord>, meerkat_core::SessionStoreError>;

    /// meerkat 0.8.21 format-door crossing, bridged for continuity stores.
    ///
    /// The default performs REAL verification, never a stub: an existing
    /// head row is rematerialized through the single-snapshot
    /// [`Self::load_canonical_session`] read and reverified against the
    /// stored CAS token before `AlreadyCurrent` is minted. Absent and
    /// blob-canonical sessions return `NotApplicable`, exactly the crossing
    /// contract's absence arm - the legacy WholeBlob lane is preserved
    /// unchanged, and physical blob-to-head CONVERSION (the 1.11 crossing)
    /// remains an explicit per-store implementation, not a default.
    async fn cross_head_canonical_authority(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<
        meerkat_core::session_store::HeadCanonicalAuthorityCrossing,
        meerkat_core::SessionStoreError,
    > {
        let Some(head) = self.load_canonical_head(id).await? else {
            return Ok(meerkat_core::session_store::HeadCanonicalAuthorityCrossing::NotApplicable);
        };
        let token = meerkat_core::session_store::session_head_cas_token(&head)?;
        let session = self
            .load_canonical_session(id)
            .await?
            .ok_or_else(|| meerkat_core::SessionStoreError::NotFound(id.clone()))?;
        let materialization = head.verify_materialized_session(session)?;
        meerkat_core::session_store::HeadCanonicalAuthorityCrossing::already_current(
            materialization,
            token,
        )
    }
}

// ---------------------------------------------------------------------------
// CONTRACT-02: LeaseProvider
// ---------------------------------------------------------------------------

/// Live ownership provider issuing monotonic fencing tokens.
///
/// The single source of truth for who may act on an identity now.
#[async_trait]
pub trait LeaseProvider: Send + Sync {
    /// Acquire leases for the given identities on behalf of `runtime_instance`.
    async fn acquire_leases(
        &self,
        identities: &[AgentIdentity],
        runtime_instance: &str,
    ) -> Result<BTreeMap<AgentIdentity, LeaseAcquireResult>, LeaseError>;

    /// Renew existing lease grants.
    ///
    /// Returning `Err` is a pre-commit failure: the provider must leave every
    /// supplied grant authoritative. Per-identity authority loss is reported as
    /// [`LeaseRenewResult::Lost`], and a committed rotation is returned as
    /// [`LeaseRenewResult::Renewed`] with the exact replacement token. This
    /// distinction lets runtimes safely quiesce and then either resume the old
    /// bridge authority or publish the returned replacement.
    async fn renew_leases(
        &self,
        grants: &[LeaseGrant],
    ) -> Result<BTreeMap<AgentIdentity, LeaseRenewResult>, LeaseError>;

    /// Release held leases.
    async fn release_leases(&self, grants: &[LeaseGrant]) -> Result<(), LeaseError>;
}

// ---------------------------------------------------------------------------
// CONTRACT-03: RosterProvider
// ---------------------------------------------------------------------------

/// Roster/discovery provider returning the desired set of durable agent specs.
#[async_trait]
pub trait RosterProvider: Send + Sync {
    /// Return the desired roster given the current context.
    async fn roster(&self, context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError>;
}

/// A pure projection of an application's current roster, supplied by a
/// native host composition for an authenticated prepared application.
///
/// Meta never interprets the application's own schema: the host's
/// implementation owns the meaning. It is applied to every current result
/// of the existing roster provider, so unrelated rows and removals pass
/// through as the provider reports them.
pub trait RosterProjection: Send + Sync {
    /// Project `roster`, the provider's current result for `context`. An
    /// error refuses the affected activation or update.
    fn project(
        &self,
        context: &RosterContext,
        roster: Vec<DurableAgentSpec>,
    ) -> Result<Vec<DurableAgentSpec>, String>;
}

/// The existing roster provider with a prepared application's projection
/// applied to each of its results. It is not a second roster authority:
/// every read goes to `inner`, and the provider's own errors pass through.
pub struct ProjectedRosterProvider {
    inner: std::sync::Arc<dyn RosterProvider>,
    projection: std::sync::Arc<dyn RosterProjection>,
}

impl ProjectedRosterProvider {
    pub fn new(
        inner: std::sync::Arc<dyn RosterProvider>,
        projection: std::sync::Arc<dyn RosterProjection>,
    ) -> Self {
        Self { inner, projection }
    }
}

#[async_trait]
impl RosterProvider for ProjectedRosterProvider {
    async fn roster(&self, context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        let roster = self.inner.roster(context).await?;
        self.projection
            .project(context, roster)
            .map_err(RosterError::Refused)
    }
}

// ---------------------------------------------------------------------------
// CONTRACT-04: AgentCustomizer
// ---------------------------------------------------------------------------

/// Build-time agent customization provider.
///
/// `customize_build` runs before session creation. It receives read-only
/// context (identity, peers, topology) and the roster spec, and may mutate
/// the `AgentBuildDraft` (model, prompts, labels, tools, etc.).
///
/// Resume selection is NOT part of this contract — Meta owns resume
/// injection after customize_build completes.
#[async_trait]
pub trait AgentCustomizer: Send + Sync {
    /// Customize the build draft for the given identity.
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError>;

    /// Called after a session is successfully created. Best-effort.
    async fn after_create(
        &self,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        let _ = (identity, session_id, context);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// CONTRACT-05: TopologyProvider
// ---------------------------------------------------------------------------

/// Dynamic topology provider computing managed peer edges.
///
/// `target_identities` is the set of identities that WILL be active after
/// the current bootstrap/reconcile cycle — not just currently active ones.
#[async_trait]
pub trait TopologyProvider: Send + Sync {
    /// Compute the desired managed peer edges for the target activation set.
    async fn compute_edges(
        &self,
        target_identities: &[AgentIdentity],
        context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError>;
}
