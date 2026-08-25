//! RuntimeStore — atomic persistence for runtime state.
//!
//! Machine-owned runtime commands durably persist [`RunBoundaryReceipt`] values
//! atomically with their session and input-state effects.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

pub mod memory;
#[cfg(feature = "sqlite-store")]
pub mod sqlite;
mod whole_blob_rewrite;

pub use meerkat_core::{HeadCanonicalProvisionalTailAuthority, WholeBlobProvisionalTailAuthority};
pub use whole_blob_rewrite::{
    PreparedWholeBlobRewriteBoundary, PreparedWholeBlobRewriteStoreParts,
    VerifiedCommittedWholeBlobPayload,
};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
use sha2::{Digest, Sha256};

use crate::identifiers::{IdempotencyKey, LogicalRuntimeId};
use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
use crate::runtime_state::RuntimeState;

const LEGACY_MACHINE_LIFECYCLE_STORE_RECORD_VERSION: u16 = 1;
const SUPERVISOR_MACHINE_LIFECYCLE_STORE_RECORD_VERSION: u16 = 2;
const UNREGISTER_MACHINE_LIFECYCLE_STORE_RECORD_VERSION: u16 = 3;
const RUN_MACHINE_LIFECYCLE_STORE_RECORD_VERSION: u16 = 4;
pub(crate) const MACHINE_LIFECYCLE_STORE_RECORD_VERSION: u16 = 5;

/// Maximum number of exact input-state rows admitted by one compare-and-swap
/// boundary. Directed-terminal outbox batches share the same 256-row bound as
/// their publication seam.
pub const MAX_INPUT_STATE_BATCH_CAS: usize = 256;

/// Maximum number of canonical pending-terminal owner ids returned by one
/// discovery page.
pub const MAX_PENDING_TERMINAL_OWNER_PAGE: usize = 256;

pub(crate) fn input_state_is_pending_terminal_owner(
    state: &crate::input_state::InputState,
) -> bool {
    let owns_pending_completion = state
        .terminal_completion
        .as_ref()
        .is_some_and(|completion| {
            completion.owner_input_id == state.input_id
                && matches!(
                    &completion.phase,
                    crate::input_state::InputTerminalCompletionPhase::Pending
                )
        });
    let owns_unpublished_interaction =
        state
            .interaction_terminal_outbox
            .as_ref()
            .is_some_and(|outbox| {
                outbox.candidate_owner_input_id == state.input_id
                    && !matches!(
                        &outbox.phase,
                        crate::input_state::InteractionTerminalOutboxPhase::Published { .. }
                    )
            });
    owns_pending_completion || owns_unpublished_interaction
}

pub(crate) fn input_state_is_recovery_nonterminal(state: &StoredInputState) -> bool {
    !matches!(
        state.seed.phase,
        crate::input_state::InputLifecycleState::Consumed
            | crate::input_state::InputLifecycleState::Superseded
            | crate::input_state::InputLifecycleState::Coalesced
            | crate::input_state::InputLifecycleState::Abandoned
    )
}

/// Whether an input row has enough durable terminal evidence to retire its
/// original ingress payload.
///
/// The payload is crash-redelivery and durable-tail-attribution material, not
/// terminal history. Keep it until the generated lifecycle is terminal and
/// every row-local completion/publication saga is closed. Callers must apply
/// the resulting omission in the same transaction that commits the closing
/// evidence (or in a later exact-CAS compaction); serialize-time projection
/// alone would leave the live shell and durable row divergent.
pub(crate) fn input_state_payload_is_retirable(state: &StoredInputState) -> bool {
    let lifecycle_terminal = matches!(
        state.seed.phase,
        crate::input_state::InputLifecycleState::Consumed
            | crate::input_state::InputLifecycleState::Superseded
            | crate::input_state::InputLifecycleState::Coalesced
            | crate::input_state::InputLifecycleState::Abandoned
    ) && state.seed.terminal_outcome.is_some();
    let completion_closed = state
        .state
        .terminal_completion
        .as_ref()
        .is_none_or(|completion| {
            matches!(
                completion.phase,
                crate::input_state::InputTerminalCompletionPhase::Finalized { .. }
            )
        });
    let publication_closed =
        state
            .state
            .interaction_terminal_outbox
            .as_ref()
            .is_none_or(|outbox| {
                matches!(
                    outbox.phase,
                    crate::input_state::InteractionTerminalOutboxPhase::Published { .. }
                )
            });
    // A directed input's payload is itself the only pre-outbox carrier of
    // the Interaction identity. A terminal row without an outbox therefore
    // has an unmaterialized publication obligation and must retain content.
    // Malformed payloads also fail closed here; their validation error must
    // remain observable rather than being erased as if they were ordinary
    // terminal inputs.
    let has_unmaterialized_directed_terminal =
        state.state.persisted_input.as_ref().is_some_and(|input| {
            crate::input::validated_directed_interaction_id(input)
                .map(|interaction_id| {
                    interaction_id.is_some() && state.state.interaction_terminal_outbox.is_none()
                })
                .unwrap_or(true)
        });
    let has_compact_directed_attribution = state
        .state
        .interaction_terminal_outbox
        .as_ref()
        .is_none_or(|_| {
            state
                .state
                .directed_run_started_attribution
                .as_ref()
                .is_some_and(|attribution| !attribution.content_digest().is_empty())
        });
    lifecycle_terminal
        && completion_closed
        && publication_closed
        && !has_unmaterialized_directed_terminal
        && has_compact_directed_attribution
}

#[cfg(test)]
mod terminal_payload_retirement_tests {
    use super::*;
    use crate::input::{Input, PromptInput};
    use crate::input_state::{InputLifecycleState, InputTerminalOutcome, StoredInputState};

    fn prompt_payload() -> Input {
        Input::Prompt(PromptInput::new("large durable prompt", None))
    }

    fn with_terminal_seed(mut stored: StoredInputState) -> StoredInputState {
        stored.seed.phase = InputLifecycleState::Consumed;
        stored.seed.terminal_outcome = Some(InputTerminalOutcome::Consumed);
        stored.seed.recovery_lane = None;
        stored
    }

    #[test]
    fn payload_retirement_requires_terminal_lifecycle_and_closed_obligations() {
        let input_id = InputId::new();
        let mut accepted = StoredInputState::new_accepted(input_id.clone());
        accepted.state.persisted_input = Some(prompt_payload());
        assert!(!input_state_payload_is_retirable(&accepted));

        let mut staged = accepted.clone();
        staged.seed.phase = InputLifecycleState::Staged;
        assert!(!input_state_payload_is_retirable(&staged));

        let terminal = with_terminal_seed(accepted);
        assert!(input_state_payload_is_retirable(&terminal));

        let (mut pending, _) = pending_terminal_owner_fixture(input_id.clone(), false);
        pending.state.persisted_input = Some(prompt_payload());
        pending = with_terminal_seed(pending);
        assert!(!input_state_payload_is_retirable(&pending));

        let (mut published, _) = pending_terminal_owner_fixture(input_id, true);
        published.state.persisted_input = Some(prompt_payload());
        published = with_terminal_seed(published);
        assert!(input_state_payload_is_retirable(&published));
    }

    #[test]
    fn retired_terminal_wire_image_omits_only_the_payload() {
        let mut terminal = StoredInputState::new_accepted(InputId::new());
        terminal.state.persisted_input = Some(prompt_payload());
        terminal = with_terminal_seed(terminal);
        let before_seed = terminal.seed.clone();

        assert!(input_state_payload_is_retirable(&terminal));
        terminal.state.persisted_input = None;
        let encoded = serde_json::to_value(&terminal).unwrap();
        let decoded: StoredInputState = serde_json::from_value(encoded.clone()).unwrap();

        assert!(encoded.get("persisted_input").is_none());
        assert_eq!(decoded.seed, before_seed);
        assert_eq!(decoded.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(
            decoded.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
    }
}

pub(crate) fn validate_pending_terminal_owner_page(
    after: Option<&InputId>,
    limit: usize,
    owner_input_ids: &[InputId],
) -> Result<(), RuntimeStoreError> {
    if limit == 0 || limit > MAX_PENDING_TERMINAL_OWNER_PAGE {
        return Err(RuntimeStoreError::InvalidInputStateBatchCas {
            reason: format!(
                "pending-terminal owner page limit {limit} is outside 1..={MAX_PENDING_TERMINAL_OWNER_PAGE}"
            ),
        });
    }
    if owner_input_ids.len() > limit {
        return Err(RuntimeStoreError::ReadFailed(format!(
            "pending-terminal owner page returned {} ids for limit {limit}",
            owner_input_ids.len()
        )));
    }
    if owner_input_ids
        .windows(2)
        .any(|window| window[0].0 >= window[1].0)
    {
        return Err(RuntimeStoreError::ReadFailed(
            "pending-terminal owner page is not strictly ordered".to_string(),
        ));
    }
    if let (Some(after), Some(first)) = (after, owner_input_ids.first())
        && first.0 <= after.0
    {
        return Err(RuntimeStoreError::ReadFailed(
            "pending-terminal owner page did not advance its stable cursor".to_string(),
        ));
    }
    Ok(())
}

/// Result of an exact input-state batch compare-and-swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputStateBatchCasOutcome {
    /// Every expected durable row matched and every replacement committed, or
    /// every durable row was already byte-identical to its replacement from an
    /// earlier invocation whose acknowledgement was lost.
    Swapped,
    /// At least one expected row was missing or no longer byte-identical; no
    /// replacement was written.
    Stale,
}

/// Backend realization profile for logical exact-batch input-state CAS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputStateBatchCasImplementationProfile {
    /// The store does not provide the exact atomic batch contract.
    Unsupported,
    /// Per-row comparison and the complete replacement set commit in one
    /// durable multi-writer transaction.
    MultiWriter,
    /// A whole-batch backend write is safe only while every write validates a
    /// durable exclusive-writer fence epoch.
    ///
    /// A process-local mutex alone never satisfies this profile. The epoch
    /// token must be conditionally checked by the backing store, the complete
    /// batch must persist in one conditional write, and publication must wait
    /// for that durable success. A crash leaves the receipt Pending for cold
    /// deterministic retry.
    ExclusiveWriterFenced,
}

/// Result of an exact input-state batch compare-and-swap performed under an
/// external authority fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FencedInputStateBatchCasOutcome {
    /// The target rows matched and the replacements committed, or were already
    /// byte-identical, while the external authority was current.
    Swapped,
    /// At least one target row no longer matched. The external fence was not
    /// consulted and no replacement was written.
    Stale,
    /// The external authority was superseded. No replacement was written.
    FenceConflict { reason: String },
    /// The external authority could not be checked temporarily. No replacement
    /// was written.
    FenceBackoff { reason: String },
}

#[derive(Debug)]
struct PreparedInputStateBatchCasRow {
    input_id: InputId,
    expected_json: Vec<u8>,
    replacement: StoredInputState,
    // SQLite writes the already-validated bytes; the in-memory implementation
    // stores the typed replacement directly.
    #[cfg_attr(not(feature = "sqlite-store"), allow(dead_code))]
    replacement_json: Vec<u8>,
}

fn prepare_input_state_batch_cas(
    expected: &[StoredInputState],
    replacements: &[InputStatePersistenceRecord],
) -> Result<Vec<PreparedInputStateBatchCasRow>, RuntimeStoreError> {
    if expected.len() != replacements.len() {
        return Err(RuntimeStoreError::InvalidInputStateBatchCas {
            reason: format!(
                "expected row count {} does not match replacement row count {}",
                expected.len(),
                replacements.len()
            ),
        });
    }
    if expected.len() > MAX_INPUT_STATE_BATCH_CAS {
        return Err(RuntimeStoreError::InvalidInputStateBatchCas {
            reason: format!(
                "batch contains {} rows, exceeding the maximum of {MAX_INPUT_STATE_BATCH_CAS}",
                expected.len()
            ),
        });
    }

    let mut expected_ids = HashSet::with_capacity(expected.len());
    for row in expected {
        if !expected_ids.insert(row.state.input_id.clone()) {
            return Err(RuntimeStoreError::InvalidInputStateBatchCas {
                reason: format!("expected batch repeats input {}", row.state.input_id),
            });
        }
    }

    let mut replacement_by_id = HashMap::with_capacity(replacements.len());
    for record in replacements {
        let replacement = record.clone_stored();
        let input_id = replacement.state.input_id.clone();
        let replacement_json = serde_json::to_vec(&replacement)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        if replacement_by_id
            .insert(input_id.clone(), (replacement, replacement_json))
            .is_some()
        {
            return Err(RuntimeStoreError::InvalidInputStateBatchCas {
                reason: format!("replacement batch repeats input {input_id}"),
            });
        }
    }

    let mut prepared = Vec::with_capacity(expected.len());
    for expected_row in expected {
        let input_id = expected_row.state.input_id.clone();
        let Some((replacement, replacement_json)) = replacement_by_id.remove(&input_id) else {
            return Err(RuntimeStoreError::InvalidInputStateBatchCas {
                reason: format!("replacement batch does not contain expected input {input_id}"),
            });
        };
        let expected_json = serde_json::to_vec(expected_row)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        prepared.push(PreparedInputStateBatchCasRow {
            input_id,
            expected_json,
            replacement,
            replacement_json,
        });
    }
    if let Some(extra) = replacement_by_id.keys().next() {
        return Err(RuntimeStoreError::InvalidInputStateBatchCas {
            reason: format!("replacement batch contains unexpected input {extra}"),
        });
    }
    Ok(prepared)
}

pub(crate) fn validate_input_state_batch_read_ids(
    input_ids: &[InputId],
) -> Result<(), RuntimeStoreError> {
    if input_ids.len() > MAX_INPUT_STATE_BATCH_CAS {
        return Err(RuntimeStoreError::InvalidInputStateBatchCas {
            reason: format!(
                "batch read contains {} rows, exceeding the maximum of {MAX_INPUT_STATE_BATCH_CAS}",
                input_ids.len()
            ),
        });
    }
    let mut unique = HashSet::with_capacity(input_ids.len());
    for input_id in input_ids {
        if !unique.insert(input_id.clone()) {
            return Err(RuntimeStoreError::InvalidInputStateBatchCas {
                reason: format!("batch read repeats input {input_id}"),
            });
        }
    }
    Ok(())
}

/// Durable representation selected for runtime-owned session authority.
///
/// The profile is explicit because the physical commit is materially
/// different: a whole-blob store owns one lazy body encoding plus its exact
/// row authority, while a head-canonical store owns delta rows and the small
/// head inside the runtime transaction itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeSessionPersistenceProfile {
    /// RuntimeStore retains an authoritative serialized Session document.
    WholeBlobV1,
    /// RuntimeStore atomically commits canonical strand rows and a small head.
    HeadCanonicalV1,
}

/// Small RuntimeStore-owned catalog projection for session listing and
/// lifecycle discovery.
///
/// This is intentionally incapable of carrying transcript, graph, component,
/// or serialized Session body data. WholeBlob and HeadCanonical stores update
/// it in the same atomic commit as their physical session authority.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeSessionCatalogEntry {
    session_id: meerkat_core::types::SessionId,
    persistence_profile: RuntimeSessionPersistenceProfile,
    created_at: meerkat_core::time_compat::SystemTime,
    updated_at: meerkat_core::time_compat::SystemTime,
    message_count: usize,
    total_tokens: u64,
    labels: BTreeMap<String, String>,
    lifecycle_terminal: Option<meerkat_core::SessionLifecycleTerminal>,
    runtime_state: Option<RuntimeState>,
}

impl RuntimeSessionCatalogEntry {
    const SESSION_LABELS_KEY: &'static str = "session_labels";

    /// Derive a validated, body-free catalog projection from a typed Session.
    ///
    /// This value is listing metadata only. It does not certify the Session,
    /// issue store authority, authorize adoption, or prove that any physical
    /// row is current. A custom [`RuntimeStore`] must commit the returned entry
    /// atomically with the physical body and its own store-issued authority.
    ///
    /// Malformed catalog-owned metadata is rejected rather than omitted.
    pub fn from_session(
        session: &meerkat_core::Session,
        persistence_profile: RuntimeSessionPersistenceProfile,
        runtime_state: Option<RuntimeState>,
    ) -> Result<Self, RuntimeStoreError> {
        let labels = session
            .metadata()
            .get(Self::SESSION_LABELS_KEY)
            .map(|value| {
                serde_json::from_value::<BTreeMap<String, String>>(value.clone()).map_err(|error| {
                    RuntimeStoreError::WriteFailed(format!(
                        "session {} has malformed catalog labels: {error}",
                        session.id()
                    ))
                })
            })
            .transpose()?
            .unwrap_or_default();
        let lifecycle_terminal = session.try_lifecycle_terminal().map_err(|error| {
            RuntimeStoreError::WriteFailed(format!(
                "session {} has malformed lifecycle-terminal metadata: {error}",
                session.id()
            ))
        })?;
        Ok(Self {
            session_id: session.id().clone(),
            persistence_profile,
            created_at: session.created_at(),
            updated_at: session.updated_at(),
            message_count: session.messages().len(),
            total_tokens: session.total_tokens(),
            labels,
            lifecycle_terminal,
            runtime_state,
        })
    }

    /// Derive a validated, body-free catalog projection from a canonical head.
    ///
    /// This is the HeadCanonical counterpart of [`Self::from_session`]. It
    /// materializes only the head's bounded catalog metadata projection and
    /// does not mint CAS authority or authorize a store transition. The custom
    /// backend remains responsible for atomically committing this projection
    /// with its exact physical head and store-issued authority.
    pub fn from_head(
        head: &meerkat_core::session_store::SessionHead,
        persistence_profile: RuntimeSessionPersistenceProfile,
        runtime_state: Option<RuntimeState>,
    ) -> Result<Self, RuntimeStoreError> {
        let metadata = head.materialized_metadata().map_err(|error| {
            RuntimeStoreError::WriteFailed(format!(
                "session {} head has no exact catalog metadata projection: {error}",
                head.id
            ))
        })?;
        let labels = metadata
            .get(Self::SESSION_LABELS_KEY)
            .map(|value| {
                serde_json::from_value::<BTreeMap<String, String>>(value.clone()).map_err(|error| {
                    RuntimeStoreError::WriteFailed(format!(
                        "session {} head has malformed catalog labels: {error}",
                        head.id
                    ))
                })
            })
            .transpose()?
            .unwrap_or_default();
        let lifecycle_terminal =
            meerkat_core::try_lifecycle_terminal_from_map(&metadata).map_err(|error| {
                RuntimeStoreError::WriteFailed(format!(
                    "session {} head has malformed lifecycle-terminal metadata: {error}",
                    head.id
                ))
            })?;
        Self::from_head_facts(
            head,
            labels,
            lifecycle_terminal,
            persistence_profile,
            runtime_state,
        )
    }

    pub(crate) fn from_head_facts(
        head: &meerkat_core::session_store::SessionHead,
        labels: BTreeMap<String, String>,
        lifecycle_terminal: Option<meerkat_core::SessionLifecycleTerminal>,
        persistence_profile: RuntimeSessionPersistenceProfile,
        runtime_state: Option<RuntimeState>,
    ) -> Result<Self, RuntimeStoreError> {
        Ok(Self {
            session_id: head.id.clone(),
            persistence_profile,
            created_at: head.created_at,
            updated_at: head.updated_at,
            message_count: usize::try_from(head.message_count).map_err(|_| {
                RuntimeStoreError::WriteFailed(format!(
                    "session {} head message count exceeds host range",
                    head.id
                ))
            })?,
            total_tokens: head.usage.total_tokens(),
            labels,
            lifecycle_terminal,
            runtime_state,
        })
    }

    #[must_use]
    pub fn session_id(&self) -> &meerkat_core::types::SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn persistence_profile(&self) -> RuntimeSessionPersistenceProfile {
        self.persistence_profile
    }

    #[must_use]
    pub const fn created_at(&self) -> meerkat_core::time_compat::SystemTime {
        self.created_at
    }

    #[must_use]
    pub const fn updated_at(&self) -> meerkat_core::time_compat::SystemTime {
        self.updated_at
    }

    #[must_use]
    pub const fn message_count(&self) -> usize {
        self.message_count
    }

    #[must_use]
    pub const fn total_tokens(&self) -> u64 {
        self.total_tokens
    }

    #[must_use]
    pub fn labels(&self) -> &BTreeMap<String, String> {
        &self.labels
    }

    #[must_use]
    pub const fn lifecycle_terminal(&self) -> Option<meerkat_core::SessionLifecycleTerminal> {
        self.lifecycle_terminal
    }

    #[must_use]
    pub const fn runtime_state(&self) -> Option<RuntimeState> {
        self.runtime_state
    }

    pub(crate) fn set_runtime_state(&mut self, runtime_state: Option<RuntimeState>) {
        self.runtime_state = runtime_state;
    }
}

/// Upper bound for observing the currently committed session authority.
///
/// Reconciliation and health probes may poll this seam. A store must therefore
/// state whether the observation is a bounded metadata read; silently parsing
/// an accumulated WholeBlob document here recreates an O(document) hot loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSessionAuthorityReadCost {
    /// Reads only a fixed-size authority row or in-memory authority value.
    Bounded,
    /// This implementation has no bounded authority observation.
    Unsupported,
}

/// Store-issued physical identity of one committed WholeBlob row.
///
/// The Session is ordinary domain payload. Currentness is owned only by the
/// store revision and digest of the exact bytes in the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WholeBlobStoreAuthority {
    authority_version: u16,
    session_id: meerkat_core::types::SessionId,
    store_revision: u64,
    blob_sha256: String,
}

fn is_canonical_row_sha256_token(token: &str) -> bool {
    let Some(hex) = token.strip_prefix("row-sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl WholeBlobStoreAuthority {
    /// Current fixed-size ledger format.
    pub const VERSION: u16 = 1;

    /// Validate the fixed-size fields decoded from one backend store record.
    ///
    /// This constructs a value carrier only. Calling it does not make the
    /// record current and does not mint authority for a caller. The value is
    /// authoritative only when an honest [`RuntimeStore`] returns it from the
    /// same atomic observation or commit that proved the named physical row.
    pub fn from_store_record(
        authority_version: u16,
        session_id: meerkat_core::types::SessionId,
        store_revision: u64,
        blob_sha256: String,
    ) -> Result<Self, RuntimeStoreError> {
        if authority_version != Self::VERSION
            || store_revision == 0
            || !is_canonical_row_sha256_token(&blob_sha256)
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: session_id.to_string(),
                detail: "WholeBlob store authority requires the current version, nonzero \
                         revision, and canonical physical row digest"
                    .to_string(),
            });
        }
        Ok(Self {
            authority_version,
            session_id,
            store_revision,
            blob_sha256,
        })
    }

    pub(crate) fn issued(
        session_id: meerkat_core::types::SessionId,
        store_revision: u64,
        blob_sha256: String,
    ) -> Result<Self, RuntimeStoreError> {
        Self::from_store_record(Self::VERSION, session_id, store_revision, blob_sha256)
    }

    #[must_use]
    pub const fn authority_version(&self) -> u16 {
        self.authority_version
    }

    #[must_use]
    pub fn session_id(&self) -> &meerkat_core::types::SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn store_revision(&self) -> u64 {
        self.store_revision
    }

    #[must_use]
    pub fn blob_sha256(&self) -> &str {
        &self.blob_sha256
    }
}

/// One atomically observed WholeBlob row and its store-issued identity.
///
/// Backends construct this under one lock/transaction so rewrite preparation
/// cannot accidentally pair bytes from one revision with another revision's
/// digest.
#[derive(Debug, Clone)]
pub struct CommittedWholeBlobSnapshot {
    session: Arc<meerkat_core::Session>,
    bytes: Arc<Vec<u8>>,
    authority: WholeBlobStoreAuthority,
}

/// One typed whole-document successor bound to an exact store-issued
/// predecessor.
///
/// This carrier is intentionally free of Session checkpoint facts. The
/// predecessor is identified only by the store revision and exact physical
/// digest, while the successor is encoded and hashed once inside this
/// WholeBlob preparation boundary.
#[derive(Debug, Clone)]
pub struct PreparedWholeBlobSnapshotCas {
    expected_authority: WholeBlobStoreAuthority,
    candidate_session: Arc<meerkat_core::Session>,
    candidate_bytes: Arc<Vec<u8>>,
    candidate_blob_sha256: String,
}

/// Result of one exact-authority WholeBlob snapshot compare-and-swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WholeBlobSnapshotCasOutcome {
    /// The candidate is current, either after this write or as an exact
    /// idempotent observation.
    Committed(WholeBlobStoreAuthority),
    /// The expected store-issued predecessor is no longer current.
    Conflict,
}

impl PreparedWholeBlobSnapshotCas {
    pub fn prepare(
        expected_authority: WholeBlobStoreAuthority,
        candidate: BoundSessionCommit,
    ) -> Result<Self, RuntimeStoreError> {
        let candidate_session = candidate.into_session_arc().ok_or_else(|| {
            RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: expected_authority.session_id().to_string(),
                detail: "WholeBlob snapshot CAS requires a sealed typed Session".to_string(),
            }
        })?;
        if candidate_session.id() != expected_authority.session_id() {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: expected_authority.session_id().clone(),
                actual: candidate_session.id().clone(),
            });
        }
        let (candidate_bytes, candidate_blob_sha256) =
            encode_whole_blob_session(candidate_session.as_ref())?;
        Ok(Self {
            expected_authority,
            candidate_session,
            candidate_bytes,
            candidate_blob_sha256,
        })
    }

    #[must_use]
    pub fn expected_authority(&self) -> &WholeBlobStoreAuthority {
        &self.expected_authority
    }

    #[must_use]
    pub fn candidate_blob_sha256(&self) -> &str {
        &self.candidate_blob_sha256
    }

    /// Whether a bounded store observation proves this candidate current.
    #[must_use]
    pub fn accepts_committed_authority(&self, observed: &WholeBlobStoreAuthority) -> bool {
        if observed.session_id() != self.expected_authority.session_id()
            || observed.blob_sha256() != self.candidate_blob_sha256
        {
            return false;
        }
        (observed.store_revision() == self.expected_authority.store_revision()
            && self.candidate_blob_sha256 == self.expected_authority.blob_sha256())
            || self
                .expected_authority
                .store_revision()
                .checked_add(1)
                .is_some_and(|successor| observed.store_revision() == successor)
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        WholeBlobStoreAuthority,
        Arc<meerkat_core::Session>,
        Arc<Vec<u8>>,
        String,
    ) {
        (
            self.expected_authority,
            self.candidate_session,
            self.candidate_bytes,
            self.candidate_blob_sha256,
        )
    }
}

struct WholeBlobSessionWriter {
    bytes: Vec<u8>,
    hasher: Sha256,
}

impl std::io::Write for WholeBlobSessionWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(buffer);
        self.hasher.update(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encode_whole_blob_session(
    session: &meerkat_core::Session,
) -> Result<(Arc<Vec<u8>>, String), RuntimeStoreError> {
    let mut writer = WholeBlobSessionWriter {
        bytes: Vec::new(),
        hasher: Sha256::new(),
    };
    serde_json::to_writer(&mut writer, session)
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    let blob_sha256 = format!("row-sha256:{:x}", writer.hasher.finalize());
    Ok((Arc::new(writer.bytes), blob_sha256))
}

/// Opaque typed candidate prepared for one provisional WholeBlob body write.
#[derive(Debug, Clone)]
pub struct PreparedWholeBlobProvisionalTail {
    authority: WholeBlobProvisionalTailAuthority,
    candidate_artifact: meerkat_core::SerializedSessionArtifact,
    conversation_digest: String,
    message_count: u64,
    catalog_entry: RuntimeSessionCatalogEntry,
    compaction_projection_intents: Vec<meerkat_core::CompactionProjectionIntent>,
    #[cfg(test)]
    whole_blob_encode_count: Arc<std::sync::atomic::AtomicUsize>,
}

impl PreparedWholeBlobProvisionalTail {
    /// Prepare from an existing sealed boundary.
    ///
    /// Compatibility callers may already own a `BoundSessionCommit`; the
    /// artifact's single-assignment cache is reused without retaining the
    /// candidate `Session` in this carrier.
    pub fn prepare(
        base: WholeBlobStoreAuthority,
        run_id: RunId,
        candidate_sequence: u64,
        candidate: &BoundSessionCommit,
    ) -> Result<Self, RuntimeStoreError> {
        let candidate_session = candidate.session_arc_cloned().ok_or_else(|| {
            RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: base.session_id().to_string(),
                detail: "WholeBlob provisional candidate requires a sealed typed Session"
                    .to_string(),
            }
        })?;
        let artifact = candidate
            .whole_blob_artifact()
            .map_err(|error| {
                RuntimeStoreError::WriteFailed(format!(
                    "failed to materialize WholeBlob provisional candidate: {error}"
                ))
            })?
            .clone();
        Self::prepare_from_artifact(
            base,
            run_id,
            candidate_sequence,
            candidate_session.as_ref(),
            artifact,
            #[cfg(test)]
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        )
    }

    /// Borrow one live candidate while deriving the exact persisted artifact
    /// and every bounded projection.
    ///
    /// JSON bytes and SHA-256 are produced in one streaming pass. The returned
    /// carrier owns only the sealed artifact and bounded metadata; it does not
    /// clone or retain the accumulated `Session`.
    pub fn prepare_from_session(
        base: WholeBlobStoreAuthority,
        run_id: RunId,
        candidate_sequence: u64,
        candidate: &meerkat_core::Session,
    ) -> Result<Self, RuntimeStoreError> {
        #[cfg(test)]
        let whole_blob_encode_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let artifact = candidate.to_persisted_artifact().map_err(|error| {
            RuntimeStoreError::WriteFailed(format!(
                "failed to materialize WholeBlob provisional candidate: {error}"
            ))
        })?;
        #[cfg(test)]
        whole_blob_encode_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self::prepare_from_artifact(
            base,
            run_id,
            candidate_sequence,
            candidate,
            artifact,
            #[cfg(test)]
            whole_blob_encode_count,
        )
    }

    fn prepare_from_artifact(
        base: WholeBlobStoreAuthority,
        run_id: RunId,
        candidate_sequence: u64,
        candidate_session: &meerkat_core::Session,
        candidate_artifact: meerkat_core::SerializedSessionArtifact,
        #[cfg(test)] whole_blob_encode_count: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Result<Self, RuntimeStoreError> {
        if candidate_session.id() != base.session_id() {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: base.session_id().clone(),
                actual: candidate_session.id().clone(),
            });
        }
        let authority = WholeBlobProvisionalTailAuthority::issued(
            base.session_id().clone(),
            base.store_revision(),
            base.blob_sha256().to_string(),
            run_id,
            candidate_artifact.row_sha256_token().to_string(),
            candidate_sequence,
        )
        .map_err(
            |error| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: base.session_id().to_string(),
                detail: error.to_string(),
            },
        )?;
        let catalog_entry = RuntimeSessionCatalogEntry::from_session(
            candidate_session,
            RuntimeSessionPersistenceProfile::WholeBlobV1,
            None,
        )?;
        let conversation_digest =
            candidate_session
                .transcript_content_digest()
                .map_err(
                    |error| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                        runtime_id: base.session_id().to_string(),
                        detail: format!(
                            "failed to derive WholeBlob provisional conversation digest: {error}"
                        ),
                    },
                )?;
        let message_count = u64::try_from(candidate_session.messages().len()).map_err(|_| {
            RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: base.session_id().to_string(),
                detail: "WholeBlob provisional message count exceeds the durable range".to_string(),
            }
        })?;
        let compaction_projection_intents =
            validated_compaction_projection_intents(candidate_session)?;
        Ok(Self {
            authority,
            candidate_artifact,
            conversation_digest,
            message_count,
            catalog_entry,
            compaction_projection_intents,
            #[cfg(test)]
            whole_blob_encode_count,
        })
    }

    #[must_use]
    pub fn authority(&self) -> &WholeBlobProvisionalTailAuthority {
        &self.authority
    }

    #[must_use]
    pub fn conversation_digest(&self) -> &str {
        &self.conversation_digest
    }

    #[must_use]
    pub const fn message_count(&self) -> u64 {
        self.message_count
    }

    #[cfg(test)]
    fn whole_blob_encode_count(&self) -> usize {
        self.whole_blob_encode_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        WholeBlobProvisionalTailAuthority,
        meerkat_core::SerializedSessionArtifact,
        String,
        u64,
        RuntimeSessionCatalogEntry,
        Vec<meerkat_core::CompactionProjectionIntent>,
    ) {
        (
            self.authority,
            self.candidate_artifact,
            self.conversation_digest,
            self.message_count,
            self.catalog_entry,
            self.compaction_projection_intents,
        )
    }
}

/// One atomically observed provisional authority and its exact candidate body.
#[derive(Debug, Clone)]
pub struct CommittedWholeBlobProvisionalTail {
    authority: WholeBlobProvisionalTailAuthority,
    candidate_bytes: Arc<Vec<u8>>,
}

/// Exact metadata-only request to promote one store-owned WholeBlob candidate.
///
/// The candidate body, digest, catalog facts, and compaction intents were
/// already committed by [`PreparedWholeBlobProvisionalTail`]. This carrier
/// deliberately has no `Session` or serialized artifact, so the final boundary
/// cannot encode or hash the accumulated document a second time.
#[derive(Debug, Clone)]
pub struct PreparedWholeBlobProvisionalPromotion {
    authority: WholeBlobProvisionalTailAuthority,
    conversation_digest: String,
    message_count: u64,
}

impl PreparedWholeBlobProvisionalPromotion {
    pub fn prepare(
        checkpoint: meerkat_core::RunCheckpointReceipt,
        run_id: &RunId,
    ) -> Result<Self, RuntimeStoreError> {
        let authority = checkpoint.whole_blob().cloned().ok_or_else(|| {
            RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: checkpoint.session_id().to_string(),
                detail: "HeadCanonical checkpoint cannot authorize WholeBlob promotion".to_string(),
            }
        })?;
        if authority.run_id() != run_id {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: authority.session_id().to_string(),
                detail: "WholeBlob promotion run differs from store-issued candidate run"
                    .to_string(),
            });
        }
        Ok(Self {
            authority,
            conversation_digest: checkpoint.conversation_digest().to_string(),
            message_count: checkpoint.message_count(),
        })
    }

    #[must_use]
    pub fn authority(&self) -> &WholeBlobProvisionalTailAuthority {
        &self.authority
    }

    #[must_use]
    pub(crate) fn into_parts(self) -> (WholeBlobProvisionalTailAuthority, String, u64) {
        (self.authority, self.conversation_digest, self.message_count)
    }
}

/// Exact WholeBlob recovery action bound to one store-owned provisional row.
///
/// A completed candidate carries no body: the store promotes its existing
/// candidate allocation with metadata-only writes. An interrupted repair
/// carries the one sealed successor artifact produced during classification;
/// the backend installs those exact bytes without encoding or hashing again.
#[derive(Debug, Clone)]
pub(crate) struct PreparedWholeBlobRecoveryPromotion {
    authority: WholeBlobProvisionalTailAuthority,
    repaired_snapshot: Option<PreparedWholeBlobSnapshot>,
}

impl PreparedWholeBlobRecoveryPromotion {
    fn prepare(
        repaired_document: Option<&BoundSessionCommit>,
        evidence: &PreparedRecoveryEvidence,
    ) -> Result<Self, RuntimeStoreError> {
        let (
            base_store_revision,
            base_blob_sha256,
            candidate_blob_sha256,
            candidate_sequence,
            recovered_blob_sha256,
        ) = evidence.whole_blob_authority_transition().ok_or_else(|| {
            RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: evidence.session_id().to_string(),
                detail: "HeadCanonical recovery evidence cannot authorize WholeBlob promotion"
                    .to_string(),
            }
        })?;
        let authority = WholeBlobProvisionalTailAuthority::issued(
            evidence.session_id().clone(),
            base_store_revision,
            base_blob_sha256.to_string(),
            evidence.candidate_run_id().clone(),
            candidate_blob_sha256.to_string(),
            candidate_sequence,
        )
        .map_err(
            |error| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: evidence.session_id().to_string(),
                detail: error.to_string(),
            },
        )?;
        let repaired_snapshot = if recovered_blob_sha256 == candidate_blob_sha256 {
            if repaired_document.is_some() {
                return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: evidence.session_id().to_string(),
                    detail: "completed WholeBlob recovery must not carry a materialized body"
                        .to_string(),
                });
            }
            None
        } else {
            let document = repaired_document.ok_or_else(|| {
                RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: evidence.session_id().to_string(),
                    detail: "WholeBlob repair has no sealed successor artifact".to_string(),
                }
            })?;
            let prepared = prepared_whole_blob_snapshot(document)?;
            if prepared.session().id() != evidence.session_id()
                || prepared.blob_sha256() != recovered_blob_sha256
            {
                return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: evidence.session_id().to_string(),
                    detail: "WholeBlob repaired artifact differs from sealed recovery authority"
                        .to_string(),
                });
            }
            Some(prepared)
        };
        Ok(Self {
            authority,
            repaired_snapshot,
        })
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        WholeBlobProvisionalTailAuthority,
        Option<PreparedWholeBlobSnapshot>,
    ) {
        (self.authority, self.repaired_snapshot)
    }
}

/// Opaque metadata-only promotion of an already-applied HeadCanonical tail.
///
/// The physical rows were committed by the checkpoint CAS named by
/// `authority`; final runtime commit consumes only this fixed-size receipt.
#[derive(Debug, Clone)]
pub struct PreparedHeadCanonicalProvisionalPromotion {
    checkpoint: meerkat_core::RunCheckpointReceipt,
    authority: HeadCanonicalProvisionalTailAuthority,
}

impl PreparedHeadCanonicalProvisionalPromotion {
    pub fn prepare(
        checkpoint: meerkat_core::RunCheckpointReceipt,
        run_id: &RunId,
    ) -> Result<Self, RuntimeStoreError> {
        let authority = checkpoint.head_canonical().cloned().ok_or_else(|| {
            RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: checkpoint.session_id().to_string(),
                detail: "HeadCanonical promotion received a WholeBlob checkpoint".to_string(),
            }
        })?;
        if authority.run_id() != run_id {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: authority.session_id().to_string(),
                detail: "HeadCanonical promotion run differs from store-issued physical tail"
                    .to_string(),
            });
        }
        Ok(Self {
            checkpoint,
            authority,
        })
    }

    #[must_use]
    pub fn authority(&self) -> &HeadCanonicalProvisionalTailAuthority {
        &self.authority
    }

    #[must_use]
    pub fn checkpoint(&self) -> &meerkat_core::RunCheckpointReceipt {
        &self.checkpoint
    }

    #[must_use]
    pub(crate) fn into_parts(
        self,
    ) -> (
        meerkat_core::RunCheckpointReceipt,
        HeadCanonicalProvisionalTailAuthority,
    ) {
        (self.checkpoint, self.authority)
    }
}

impl CommittedWholeBlobProvisionalTail {
    pub(crate) fn new(
        authority: WholeBlobProvisionalTailAuthority,
        candidate_bytes: Arc<Vec<u8>>,
    ) -> Self {
        Self {
            authority,
            candidate_bytes,
        }
    }

    #[must_use]
    pub fn authority(&self) -> &WholeBlobProvisionalTailAuthority {
        &self.authority
    }

    #[must_use]
    pub fn candidate_bytes(&self) -> &[u8] {
        self.candidate_bytes.as_ref()
    }

    #[must_use]
    pub fn candidate_bytes_arc(&self) -> Arc<Vec<u8>> {
        Arc::clone(&self.candidate_bytes)
    }
}

impl CommittedWholeBlobSnapshot {
    pub(crate) fn new(
        bytes: Arc<Vec<u8>>,
        authority: WholeBlobStoreAuthority,
    ) -> Result<Self, RuntimeStoreError> {
        let decoded =
            meerkat_core::Session::decode_whole_blob_document(bytes.as_ref()).map_err(|error| {
                RuntimeStoreError::ReadFailed(format!(
                    "WholeBlob body is not a valid current Session: {error}"
                ))
            })?;
        if decoded.row_sha256_token() != authority.blob_sha256() {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: authority.session_id().to_string(),
                detail: "WholeBlob body digest differs from store authority".to_string(),
            });
        }
        let session = Arc::new(decoded.into_session());
        if session.id() != authority.session_id() {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: authority.session_id().to_string(),
                detail: "WholeBlob body session differs from store authority".to_string(),
            });
        }
        Ok(Self {
            session,
            bytes,
            authority,
        })
    }

    /// Typed domain document decoded from these exact committed bytes.
    #[must_use]
    pub fn session(&self) -> &meerkat_core::Session {
        self.session.as_ref()
    }

    /// Shared typed domain document decoded from these exact committed bytes.
    #[must_use]
    pub fn session_arc(&self) -> Arc<meerkat_core::Session> {
        Arc::clone(&self.session)
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_ref()
    }

    #[must_use]
    pub fn bytes_arc(&self) -> Arc<Vec<u8>> {
        Arc::clone(&self.bytes)
    }

    #[must_use]
    pub fn authority(&self) -> &WholeBlobStoreAuthority {
        &self.authority
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        Arc<meerkat_core::Session>,
        Arc<Vec<u8>>,
        WholeBlobStoreAuthority,
    ) {
        (self.session, self.bytes, self.authority)
    }
}

impl std::fmt::Display for RuntimeSessionPersistenceProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WholeBlobV1 => f.write_str("whole_blob_v1"),
            Self::HeadCanonicalV1 => f.write_str("head_canonical_v1"),
        }
    }
}

/// Store-issued identity of one committed HeadCanonical boundary.
///
/// The small head carries the exact row, rewrite, graph, component, and
/// metadata-prefix facts. Currentness is the store revision plus the exact
/// committed head token; no fact inside a materialized `Session` participates
/// in authority.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadCanonicalStoreAuthority {
    authority_version: u16,
    session_id: meerkat_core::types::SessionId,
    store_revision: u64,
    boundary_head: meerkat_core::session_store::SessionHead,
    committed_head_token: String,
}

impl HeadCanonicalStoreAuthority {
    pub const VERSION: u16 = 1;

    /// Validate one fixed-size authority record and its exact canonical head.
    ///
    /// Construction validates representation and identity only; it does not
    /// certify that the record is current or authorize a transition. The value
    /// becomes authority only when an honest [`RuntimeStore`] returns it from
    /// the atomic observation or commit that proved the matching physical
    /// head and store revision.
    pub fn from_store_record(
        authority_version: u16,
        session_id: meerkat_core::types::SessionId,
        store_revision: u64,
        boundary_head: meerkat_core::session_store::SessionHead,
        committed_head_token: String,
    ) -> Result<Self, RuntimeStoreError> {
        let conflict = |detail: String| RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: session_id.to_string(),
            detail,
        };
        if authority_version != Self::VERSION
            || store_revision == 0
            || committed_head_token.is_empty()
        {
            return Err(conflict(
                "HeadCanonical authority requires the current version, nonzero store revision, \
                 and head token"
                    .to_string(),
            ));
        }
        if boundary_head.id != session_id {
            return Err(conflict(format!(
                "HeadCanonical boundary belongs to {}, not {session_id}",
                boundary_head.id
            )));
        }
        let row_prefix = boundary_head.message_row_prefix.as_ref().ok_or_else(|| {
            conflict("HeadCanonical boundary has no exact message-row prefix".to_string())
        })?;
        if row_prefix.row_count() != boundary_head.message_count {
            return Err(conflict(
                "HeadCanonical boundary message count and row prefix differ".to_string(),
            ));
        }
        if boundary_head.rewrite_prefix.occurrence_count() != boundary_head.rewrite_count {
            return Err(conflict(
                "HeadCanonical boundary rewrite count and prefix differ".to_string(),
            ));
        }
        let derived = meerkat_core::session_head_cas_token(&boundary_head)
            .map_err(|error| conflict(format!("HeadCanonical head token is invalid: {error}")))?;
        if derived != committed_head_token {
            return Err(conflict(
                "store-issued HeadCanonical token differs from the exact boundary head".to_string(),
            ));
        }
        Ok(Self {
            authority_version,
            session_id,
            store_revision,
            boundary_head,
            committed_head_token,
        })
    }

    pub(crate) fn issued(
        session_id: meerkat_core::types::SessionId,
        store_revision: u64,
        boundary_head: meerkat_core::session_store::SessionHead,
        committed_head_token: String,
    ) -> Result<Self, RuntimeStoreError> {
        Self::from_store_record(
            Self::VERSION,
            session_id,
            store_revision,
            boundary_head,
            committed_head_token,
        )
    }

    #[must_use]
    pub const fn authority_version(&self) -> u16 {
        self.authority_version
    }

    #[must_use]
    pub fn session_id(&self) -> &meerkat_core::types::SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn store_revision(&self) -> u64 {
        self.store_revision
    }

    #[must_use]
    pub fn boundary_head(&self) -> &meerkat_core::session_store::SessionHead {
        &self.boundary_head
    }

    #[must_use]
    pub fn committed_head_token(&self) -> &str {
        &self.committed_head_token
    }
}

/// Result of consuming one store-verified HeadCanonical activation proof.
///
/// The runtime backend owns the monotonic runtime revision. The physical
/// backend owns the verified head/token pair. This result records how the
/// runtime side aligned itself without inventing a second physical authority.
#[derive(Debug, Clone, PartialEq)]
#[must_use]
pub enum HeadCanonicalRuntimeAuthorityActivation {
    /// No runtime authority existed, so revision one was installed.
    Installed(HeadCanonicalStoreAuthority),
    /// The exact verified head/token was already installed.
    AlreadyAligned(HeadCanonicalStoreAuthority),
    /// A semantically identical pre-activation boundary was advanced to the
    /// verified representation-side successor.
    RepresentationAdvanced(HeadCanonicalStoreAuthority),
}

impl HeadCanonicalRuntimeAuthorityActivation {
    #[must_use]
    pub const fn authority(&self) -> &HeadCanonicalStoreAuthority {
        match self {
            Self::Installed(authority)
            | Self::AlreadyAligned(authority)
            | Self::RepresentationAdvanced(authority) => authority,
        }
    }
}

fn head_canonical_activation_predecessor_matches(
    predecessor: &meerkat_core::session_store::SessionHead,
    successor: &meerkat_core::session_store::SessionHead,
) -> Result<bool, RuntimeStoreError> {
    if predecessor.id != successor.id
        || predecessor.version != successor.version
        || predecessor.strand != successor.strand
        || predecessor.head_revision != successor.head_revision
        || predecessor.message_count != successor.message_count
        || predecessor.rewrite_count != successor.rewrite_count
        || predecessor.created_at != successor.created_at
        || predecessor.updated_at != successor.updated_at
        || predecessor.usage != successor.usage
    {
        return Ok(false);
    }
    if predecessor
        .message_row_prefix
        .as_ref()
        .is_some_and(|prefix| Some(prefix) != successor.message_row_prefix.as_ref())
        || predecessor
            .row_lineage_anchor
            .as_ref()
            .is_some_and(|anchor| Some(anchor) != successor.row_lineage_anchor.as_ref())
        || (predecessor.rewrite_prefix.occurrence_count() != 0
            && predecessor.rewrite_prefix != successor.rewrite_prefix)
        || predecessor
            .graph_prefix
            .as_ref()
            .is_some_and(|prefix| Some(prefix) != successor.graph_prefix.as_ref())
        || predecessor
            .realtime_event_prefix
            .as_ref()
            .is_some_and(|prefix| Some(prefix) != successor.realtime_event_prefix.as_ref())
    {
        return Ok(false);
    }
    match predecessor.metadata_identity() {
        Some(identity) => Ok(successor.metadata_identity() == Some(identity)
            && predecessor.metadata == successor.metadata),
        None => {
            let mut predecessor_metadata = predecessor.metadata.clone();
            predecessor_metadata.remove(meerkat_core::SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
            predecessor_metadata
                .remove(meerkat_core::SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
            predecessor_metadata.remove(meerkat_core::SESSION_REALTIME_TRANSCRIPT_STATE_KEY);
            successor
                .materialized_metadata()
                .map(|metadata| metadata == predecessor_metadata)
                .map_err(|error| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: predecessor.id.to_string(),
                    detail: format!(
                        "verified HeadCanonical activation metadata cannot be materialized: {error}"
                    ),
                })
        }
    }
}

/// Opaque intent written before a HeadCanonical physical-head CAS.
///
/// The carrier binds the exact committed parent, explicit run identity, and
/// exact successor head/token. It also captures the bounded catalog,
/// compaction, conversation-digest, and message-count projections from that
/// same live successor. Backends persist those facts with the fixed-size
/// authority; the physical SessionStore CAS realizes the already-bound head
/// separately.
#[derive(Debug, Clone)]
pub struct PreparedHeadCanonicalProvisionalTail {
    committed: HeadCanonicalStoreAuthority,
    run_id: RunId,
    successor_head: meerkat_core::session_store::SessionHead,
    successor_head_token: String,
    candidate_message_count: usize,
    candidate_conversation_digest: String,
    catalog_entry: RuntimeSessionCatalogEntry,
    compaction_projection_intents: Vec<meerkat_core::CompactionProjectionIntent>,
}

impl PreparedHeadCanonicalProvisionalTail {
    pub fn prepare(
        committed: HeadCanonicalStoreAuthority,
        run_id: RunId,
        successor_head: &meerkat_core::session_store::SessionHead,
        successor_head_token: &str,
        candidate_session: &meerkat_core::Session,
    ) -> Result<Self, RuntimeStoreError> {
        let session_id = committed.session_id().clone();
        let conflict = |detail: String| RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: session_id.to_string(),
            detail,
        };
        if successor_head.id != session_id || successor_head_token.is_empty() {
            return Err(conflict(
                "HeadCanonical provisional intent names the wrong session or an empty successor"
                    .to_string(),
            ));
        }
        let derived = meerkat_core::session_head_cas_token(successor_head).map_err(|error| {
            conflict(format!(
                "HeadCanonical provisional successor is invalid: {error}"
            ))
        })?;
        if derived != successor_head_token
            || successor_head_token == committed.committed_head_token()
        {
            return Err(conflict(
                "HeadCanonical provisional successor token is not the exact distinct target head"
                    .to_string(),
            ));
        }
        if candidate_session.id() != &session_id
            || candidate_session.messages().len() as u64 != successor_head.message_count
            || candidate_session.version() != successor_head.version
            || candidate_session.created_at() != successor_head.created_at
            || candidate_session.updated_at() != successor_head.updated_at
            || candidate_session.total_usage() != successor_head.usage
            || !successor_head
                .matches_session_metadata(candidate_session)
                .map_err(|error| {
                    conflict(format!(
                        "HeadCanonical provisional candidate metadata is invalid: {error}"
                    ))
                })?
        {
            return Err(conflict(
                "HeadCanonical provisional successor does not describe the exact candidate Session"
                    .to_string(),
            ));
        }
        let candidate_conversation_digest =
            candidate_session
                .transcript_content_digest()
                .map_err(|error| {
                    conflict(format!(
                        "HeadCanonical provisional candidate transcript is invalid: {error}"
                    ))
                })?;
        if candidate_conversation_digest != successor_head.head_revision {
            return Err(conflict(
                "HeadCanonical provisional candidate digest differs from its successor head"
                    .to_string(),
            ));
        }
        let catalog_entry = RuntimeSessionCatalogEntry::from_session(
            candidate_session,
            RuntimeSessionPersistenceProfile::HeadCanonicalV1,
            None,
        )?;
        let compaction_projection_intents =
            validated_compaction_projection_intents(candidate_session)?;
        Ok(Self {
            committed,
            run_id,
            successor_head: successor_head.clone(),
            successor_head_token: successor_head_token.to_string(),
            candidate_message_count: candidate_session.messages().len(),
            candidate_conversation_digest,
            catalog_entry,
            compaction_projection_intents,
        })
    }

    #[must_use]
    pub(crate) fn committed(&self) -> &HeadCanonicalStoreAuthority {
        &self.committed
    }

    #[must_use]
    pub(crate) fn run_id(&self) -> &RunId {
        &self.run_id
    }

    #[must_use]
    pub(crate) fn successor_head(&self) -> &meerkat_core::session_store::SessionHead {
        &self.successor_head
    }

    #[must_use]
    pub(crate) fn successor_head_token(&self) -> &str {
        &self.successor_head_token
    }

    #[must_use]
    pub(crate) const fn candidate_message_count(&self) -> usize {
        self.candidate_message_count
    }

    #[must_use]
    pub(crate) fn candidate_conversation_digest(&self) -> &str {
        &self.candidate_conversation_digest
    }

    #[must_use]
    pub(crate) fn catalog_entry(&self) -> &RuntimeSessionCatalogEntry {
        &self.catalog_entry
    }

    #[must_use]
    pub(crate) fn compaction_projection_intents(
        &self,
    ) -> &[meerkat_core::CompactionProjectionIntent] {
        &self.compaction_projection_intents
    }
}

/// Singular store-issued committed authority for a runtime session.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeSessionAuthority {
    WholeBlob(WholeBlobStoreAuthority),
    HeadCanonical(HeadCanonicalStoreAuthority),
}

impl RuntimeSessionAuthority {
    #[must_use]
    pub const fn profile(&self) -> RuntimeSessionPersistenceProfile {
        match self {
            Self::WholeBlob(_) => RuntimeSessionPersistenceProfile::WholeBlobV1,
            Self::HeadCanonical(_) => RuntimeSessionPersistenceProfile::HeadCanonicalV1,
        }
    }

    #[must_use]
    pub fn session_id(&self) -> &meerkat_core::types::SessionId {
        match self {
            Self::WholeBlob(authority) => authority.session_id(),
            Self::HeadCanonical(authority) => authority.session_id(),
        }
    }

    #[must_use]
    pub fn whole_blob(&self) -> Option<&WholeBlobStoreAuthority> {
        match self {
            Self::WholeBlob(authority) => Some(authority),
            Self::HeadCanonical(_) => None,
        }
    }

    #[must_use]
    pub fn head_canonical(&self) -> Option<&HeadCanonicalStoreAuthority> {
        match self {
            Self::WholeBlob(_) => None,
            Self::HeadCanonical(authority) => Some(authority),
        }
    }

    /// Exact revision issued by the physical store that owns this authority.
    #[must_use]
    pub const fn store_revision(&self) -> u64 {
        match self {
            Self::WholeBlob(authority) => authority.store_revision(),
            Self::HeadCanonical(authority) => authority.store_revision(),
        }
    }
}

/// Body-free atomic observation used to authorize one session resume attempt.
///
/// The session authority, catalog projection, and raw machine-lifecycle row
/// are observed from one backend snapshot. The carrier deliberately preserves
/// absence and malformed or unsupported lifecycle rows instead of deriving a
/// lifecycle verdict in the store. Session materialization remains a separate
/// operation; callers that need a race-free body bracket that load with equal
/// before and after observations.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeSessionResumeObservation {
    runtime_id: LogicalRuntimeId,
    session_authority: Option<RuntimeSessionAuthority>,
    catalog_entry: Option<RuntimeSessionCatalogEntry>,
    lifecycle: MachineLifecycleObservation,
}

impl RuntimeSessionResumeObservation {
    /// Construct the exact body-free observation captured by a custom store
    /// while it holds one backend snapshot or lock.
    ///
    /// This constructor does not combine independent reads or mint authority.
    /// The caller must already own the atomic backend observation represented
    /// by these values. Session and catalog identities are checked here so a
    /// custom backend cannot accidentally pair rows from different sessions.
    pub fn from_store_snapshot(
        runtime_id: LogicalRuntimeId,
        session_authority: Option<RuntimeSessionAuthority>,
        catalog_entry: Option<RuntimeSessionCatalogEntry>,
        lifecycle: MachineLifecycleObservation,
    ) -> Result<Self, RuntimeStoreError> {
        let expected_session_id = runtime_id.session_id();
        if let Some(authority) = session_authority.as_ref()
            && Some(authority.session_id()) != expected_session_id.as_ref()
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "resume observation paired session authority from another runtime"
                    .to_string(),
            });
        }
        if let Some(entry) = catalog_entry.as_ref()
            && Some(entry.session_id()) != expected_session_id.as_ref()
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "resume observation paired catalog entry from another runtime".to_string(),
            });
        }
        Ok(Self {
            runtime_id,
            session_authority,
            catalog_entry,
            lifecycle,
        })
    }

    pub(crate) fn new(
        runtime_id: LogicalRuntimeId,
        session_authority: Option<RuntimeSessionAuthority>,
        catalog_entry: Option<RuntimeSessionCatalogEntry>,
        lifecycle: MachineLifecycleObservation,
    ) -> Self {
        Self {
            runtime_id,
            session_authority,
            catalog_entry,
            lifecycle,
        }
    }

    #[must_use]
    pub fn runtime_id(&self) -> &LogicalRuntimeId {
        &self.runtime_id
    }

    #[must_use]
    pub fn session_authority(&self) -> Option<&RuntimeSessionAuthority> {
        self.session_authority.as_ref()
    }

    #[must_use]
    pub fn session_store_revision(&self) -> Option<u64> {
        self.session_authority
            .as_ref()
            .map(RuntimeSessionAuthority::store_revision)
    }

    #[must_use]
    pub fn catalog_entry(&self) -> Option<&RuntimeSessionCatalogEntry> {
        self.catalog_entry.as_ref()
    }

    #[must_use]
    pub fn lifecycle(&self) -> &MachineLifecycleObservation {
        &self.lifecycle
    }

    /// Exact content version of the physical lifecycle row, when present.
    #[must_use]
    pub fn lifecycle_row_version(&self) -> Option<&MachineLifecycleObservationVersion> {
        self.lifecycle.version()
    }

    /// Machine-owned runtime generation, only when the lifecycle row decodes.
    #[must_use]
    pub fn runtime_generation(&self) -> Option<u64> {
        match &self.lifecycle {
            MachineLifecycleObservation::Decoded { record, .. } => {
                record.binding().runtime_generation()
            }
            MachineLifecycleObservation::Missing
            | MachineLifecycleObservation::Unsupported { .. }
            | MachineLifecycleObservation::Malformed { .. } => None,
        }
    }

    /// Machine-owned runtime epoch, only when the lifecycle row decodes.
    #[must_use]
    pub fn runtime_epoch_id(&self) -> Option<&str> {
        match &self.lifecycle {
            MachineLifecycleObservation::Decoded { record, .. } => {
                record.binding().runtime_epoch_id()
            }
            MachineLifecycleObservation::Missing
            | MachineLifecycleObservation::Unsupported { .. }
            | MachineLifecycleObservation::Malformed { .. } => None,
        }
    }
}

/// Store-owned source for one durable-tail recovery pass.
///
/// The public recovery API accepts only a session identity. A backend that
/// owns both runtime authority and canonical session rows constructs this
/// opaque carrier from one transactional snapshot after checking the exact
/// retained boundary head, current physical head, and both complete
/// materializations. Callers cannot substitute a hand-authored Session or
/// head row.
#[derive(Debug, Clone)]
pub struct PreparedDurableTailRecoverySource {
    runtime_authority: RuntimeSessionAuthority,
    provisional_authority: Option<HeadCanonicalProvisionalTailAuthority>,
    provisional_target_applied: bool,
    committed_session: Arc<meerkat_core::Session>,
    physical_head: meerkat_core::session_store::SessionHead,
    physical_head_cas_token: String,
    physical_session: Arc<meerkat_core::Session>,
}

impl PreparedDurableTailRecoverySource {
    pub(crate) fn new(
        runtime_authority: RuntimeSessionAuthority,
        provisional_authority: Option<HeadCanonicalProvisionalTailAuthority>,
        committed_materialization: meerkat_core::VerifiedSessionHeadMaterialization,
        physical_materialization: meerkat_core::VerifiedSessionHeadMaterialization,
    ) -> Result<Self, RuntimeStoreError> {
        let committed_session = Arc::clone(committed_materialization.session());
        let physical_head = physical_materialization.head().clone();
        let physical_session = Arc::clone(physical_materialization.session());
        let runtime_id = runtime_authority.session_id().to_string();
        let conflict = |detail: String| RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: runtime_id.clone(),
            detail,
        };
        if runtime_authority.profile() != RuntimeSessionPersistenceProfile::HeadCanonicalV1 {
            return Err(conflict(
                "durable-tail source requires head-canonical session ownership".to_string(),
            ));
        }
        let committed_authority = runtime_authority
            .head_canonical()
            .ok_or_else(|| conflict("runtime authority is not HeadCanonical".to_string()))?;
        let boundary_head = committed_authority.boundary_head();
        if committed_materialization.head() != boundary_head {
            return Err(conflict(
                "verified committed materialization belongs to a different retained boundary head"
                    .to_string(),
            ));
        }
        let boundary_row_prefix = boundary_head.message_row_prefix.as_ref().ok_or_else(|| {
            conflict("runtime boundary has no exact message-row prefix authority".to_string())
        })?;
        if physical_materialization
            .exact_row_prefix_at(boundary_head.message_count)
            .as_ref()
            != Some(boundary_row_prefix)
        {
            return Err(conflict(
                "physical recovery materialization does not retain the runtime boundary's exact row prefix"
                    .to_string(),
            ));
        }
        if committed_session.id() != runtime_authority.session_id()
            || physical_session.id() != runtime_authority.session_id()
            || &physical_head.id != runtime_authority.session_id()
        {
            return Err(conflict(
                "durable-tail source identities do not all match runtime authority".to_string(),
            ));
        }
        let committed_revision = committed_session
            .transcript_content_digest()
            .map_err(|error| conflict(format!("committed transcript is invalid: {error}")))?;
        let committed_metadata_matches = boundary_head
            .matches_session_metadata(&committed_session)
            .map_err(|error| {
                conflict(format!(
                    "committed recovery metadata identity is invalid: {error}"
                ))
            })?;
        if committed_session.messages().len() as u64 != boundary_head.message_count
            || committed_revision != boundary_head.head_revision
            || committed_session.version() != boundary_head.version
            || committed_session.created_at() != boundary_head.created_at
            || committed_session.updated_at() != boundary_head.updated_at
            || committed_session.total_usage() != boundary_head.usage
            || !committed_metadata_matches
        {
            return Err(conflict(format!(
                "committed recovery materialization differs from the exact retained boundary envelope \
                     (message_count={}, revision={}, version={}, created_at={}, updated_at={}, usage={}, metadata={})",
                committed_session.messages().len() as u64 == boundary_head.message_count,
                committed_revision == boundary_head.head_revision,
                committed_session.version() == boundary_head.version,
                committed_session.created_at() == boundary_head.created_at,
                committed_session.updated_at() == boundary_head.updated_at,
                committed_session.total_usage() == boundary_head.usage,
                committed_metadata_matches,
            )));
        }
        let physical_revision = physical_session
            .transcript_content_digest()
            .map_err(|error| conflict(format!("physical transcript is invalid: {error}")))?;
        let physical_metadata_matches = physical_head
            .matches_session_metadata(&physical_session)
            .map_err(|error| {
            conflict(format!(
                "physical recovery metadata identity is invalid: {error}"
            ))
        })?;
        if physical_session.messages().len() as u64 != physical_head.message_count
            || physical_revision != physical_head.head_revision
            || physical_session.version() != physical_head.version
            || physical_session.created_at() != physical_head.created_at
            || physical_session.updated_at() != physical_head.updated_at
            || physical_session.total_usage() != physical_head.usage
            || !physical_metadata_matches
        {
            return Err(conflict(
                "physical recovery materialization differs from the exact current canonical envelope"
                    .to_string(),
            ));
        }
        let Some(physical_row_prefix) = physical_head.message_row_prefix.as_ref() else {
            return Err(conflict(
                "physical recovery head has no exact message-row prefix authority".to_string(),
            ));
        };
        if physical_row_prefix.row_count() != physical_head.message_count {
            return Err(conflict(
                "physical recovery head message count and exact row prefix differ".to_string(),
            ));
        }
        let physical_head_cas_token = meerkat_core::session_head_cas_token(&physical_head)
            .map_err(|error| {
                conflict(format!("physical recovery head token is invalid: {error}"))
            })?;
        if committed_authority.committed_head_token()
            != meerkat_core::session_head_cas_token(boundary_head)
                .map_err(|error| conflict(format!("committed head token is invalid: {error}")))?
        {
            return Err(conflict(
                "committed runtime authority token differs from its retained boundary head"
                    .to_string(),
            ));
        }
        let provisional_target_applied = match (
            &provisional_authority,
            physical_head == *boundary_head,
        ) {
            (None, true) => false,
            (None, false) => {
                return Err(conflict(
                    "newer physical head has no store-issued provisional authority".to_string(),
                ));
            }
            (Some(provisional), aligned) => {
                let first_provisional_revision = committed_authority
                    .store_revision()
                    .checked_add(1)
                    .ok_or_else(|| {
                        conflict("HeadCanonical store revision exhausted".to_string())
                    })?;
                let target_applied = provisional.physical_head_token() == physical_head_cas_token;
                if provisional.authority_version() != HeadCanonicalProvisionalTailAuthority::VERSION
                    || provisional.session_id() != runtime_authority.session_id()
                    || provisional.base_store_revision() != committed_authority.store_revision()
                    || provisional.base_committed_head_token()
                        != committed_authority.committed_head_token()
                    || (aligned
                        && (physical_head_cas_token != committed_authority.committed_head_token()
                            || provisional.physical_store_revision() != first_provisional_revision))
                    || (!aligned
                        && !target_applied
                        && provisional.physical_store_revision() <= first_provisional_revision)
                {
                    return Err(conflict(
                        "provisional authority does not bind the exact committed parent and physical head"
                        .to_string(),
                    ));
                }
                target_applied
            }
        };
        Ok(Self {
            runtime_authority,
            provisional_authority,
            provisional_target_applied,
            committed_session,
            physical_head,
            physical_head_cas_token,
            physical_session,
        })
    }

    pub(crate) fn runtime_authority(&self) -> &RuntimeSessionAuthority {
        &self.runtime_authority
    }

    pub(crate) fn committed_session(&self) -> &Arc<meerkat_core::Session> {
        &self.committed_session
    }

    pub(crate) fn provisional_authority(&self) -> Option<&HeadCanonicalProvisionalTailAuthority> {
        self.provisional_authority.as_ref()
    }

    pub(crate) const fn provisional_target_applied(&self) -> bool {
        self.provisional_target_applied
    }

    pub(crate) fn physical_head(&self) -> &meerkat_core::session_store::SessionHead {
        &self.physical_head
    }

    pub(crate) fn physical_head_cas_token(&self) -> &str {
        &self.physical_head_cas_token
    }

    pub(crate) fn physical_session(&self) -> &Arc<meerkat_core::Session> {
        &self.physical_session
    }
}

/// One exact durable receipt row admitted to recovery classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRecoveryReceiptSource {
    receipt: RunBoundaryReceipt,
    exact_row_token: String,
}

impl PreparedRecoveryReceiptSource {
    pub(crate) fn from_serialized_row(bytes: &[u8]) -> Result<Self, RuntimeStoreError> {
        let receipt = serde_json::from_slice(bytes).map_err(|error| {
            RuntimeStoreError::ReadFailed(format!("invalid durable recovery receipt row: {error}"))
        })?;
        Ok(Self {
            receipt,
            exact_row_token: format!("receipt-row-sha256:{:x}", Sha256::digest(bytes)),
        })
    }

    pub(crate) fn receipt(&self) -> &RunBoundaryReceipt {
        &self.receipt
    }

    pub(crate) fn exact_row_token(&self) -> &str {
        &self.exact_row_token
    }
}

/// Sealed one-time enrichment of a supported-floor digestless receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRecoveryReceiptDigestEnrichment {
    original_receipt: RunBoundaryReceipt,
    original_exact_row_token: String,
    derived_conversation_digest: String,
}

impl PreparedRecoveryReceiptDigestEnrichment {
    pub(crate) fn new(
        source: &PreparedRecoveryReceiptSource,
        derived_conversation_digest: String,
    ) -> Result<Self, RuntimeStoreError> {
        if source.receipt.conversation_digest.is_some() || derived_conversation_digest.is_empty() {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: source.receipt.run_id.to_string(),
                detail: "receipt enrichment must replace exactly one missing digest".to_string(),
            });
        }
        Ok(Self {
            original_receipt: source.receipt.clone(),
            original_exact_row_token: source.exact_row_token.clone(),
            derived_conversation_digest,
        })
    }

    pub(crate) fn original_receipt(&self) -> &RunBoundaryReceipt {
        &self.original_receipt
    }

    pub(crate) fn original_exact_row_token(&self) -> &str {
        &self.original_exact_row_token
    }

    pub(crate) fn derived_conversation_digest(&self) -> &str {
        &self.derived_conversation_digest
    }

    pub(crate) fn enriched_receipt(&self) -> RunBoundaryReceipt {
        let mut receipt = self.original_receipt.clone();
        receipt.conversation_digest = Some(self.derived_conversation_digest.clone());
        receipt
    }
}

/// Whether a prepared boundary was newly written or proven from durable store
/// authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparedRuntimeSessionCommitOutcome {
    /// This call installed the boundary.
    Applied,
    /// All durable authority and receipt witnesses already matched exactly.
    AlreadyAppliedExact,
    /// The exact released 0.8.10 receipt and every still-observable committed
    /// effect matched, and the v1 -> v2 migration marker authorized minting the
    /// first current request witness. The released schema did not retain prior
    /// CAS preconditions, so this is deliberately not reported as an exact
    /// retry of the original request.
    AlreadyAppliedReleasedEquivalent,
}

/// Exact convergence result for a machine-authorized recovery boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryCommitStatus {
    /// This invocation installed the recovery boundary.
    Committed,
    /// A prior invocation installed byte-exact evidence and receipt state.
    AlreadyCommittedExact,
}

/// Result of an atomic prepared session-boundary commit.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedRuntimeSessionCommitResult {
    profile: RuntimeSessionPersistenceProfile,
    outcome: PreparedRuntimeSessionCommitOutcome,
    recovery_status: Option<RecoveryCommitStatus>,
    downstream_projection_required: bool,
    authority: Option<RuntimeSessionAuthority>,
}

impl PreparedRuntimeSessionCommitResult {
    /// Construct the result of a boundary that committed session authority.
    #[must_use]
    pub fn committed(authority: RuntimeSessionAuthority) -> Self {
        let profile = authority.profile();
        Self {
            profile,
            outcome: PreparedRuntimeSessionCommitOutcome::Applied,
            recovery_status: None,
            // RuntimeStore is the sole full-body authority for WholeBlob and
            // owns the small catalog projection for both profiles. No boundary
            // requires a downstream SessionStore body mirror.
            downstream_projection_required: false,
            authority: Some(authority),
        }
    }

    /// Construct the result of a receipt/input-only boundary.
    #[must_use]
    pub const fn receipt_only(profile: RuntimeSessionPersistenceProfile) -> Self {
        Self {
            profile,
            outcome: PreparedRuntimeSessionCommitOutcome::Applied,
            recovery_status: None,
            downstream_projection_required: false,
            authority: None,
        }
    }

    /// Construct the exact result of an atomic recovery boundary.
    #[must_use]
    pub fn recovery(authority: RuntimeSessionAuthority, status: RecoveryCommitStatus) -> Self {
        let mut result = Self::committed(authority);
        result.recovery_status = Some(status);
        if status == RecoveryCommitStatus::AlreadyCommittedExact {
            result.outcome = PreparedRuntimeSessionCommitOutcome::AlreadyAppliedExact;
        }
        result
    }

    /// Reclassify a result after the store proved an exact durable retry.
    #[must_use]
    pub fn already_applied_exact(mut self) -> Self {
        self.outcome = PreparedRuntimeSessionCommitOutcome::AlreadyAppliedExact;
        self
    }

    /// Reclassify a result after one migration-authorized released-boundary
    /// adoption. Subsequent retries of the newly witnessed request are exact.
    #[must_use]
    pub fn already_applied_released_equivalent(mut self) -> Self {
        self.outcome = PreparedRuntimeSessionCommitOutcome::AlreadyAppliedReleasedEquivalent;
        self
    }

    /// Representation that became authoritative in the atomic boundary.
    #[must_use]
    pub const fn profile(&self) -> RuntimeSessionPersistenceProfile {
        self.profile
    }

    /// Whether this invocation installed the boundary, proved an exact retry,
    /// or adopted an effect-equivalent released boundary.
    #[must_use]
    pub const fn outcome(&self) -> PreparedRuntimeSessionCommitOutcome {
        self.outcome
    }

    /// Exact recovery convergence, when this was a recovery boundary.
    #[must_use]
    pub const fn recovery_status(&self) -> Option<RecoveryCommitStatus> {
        self.recovery_status
    }

    /// Whether the caller must publish a separate compatibility projection
    /// after this commit.
    #[must_use]
    pub const fn downstream_projection_required(&self) -> bool {
        self.downstream_projection_required
    }

    /// Exact session authority committed in this boundary, or `None` when the
    /// successful boundary carried receipt/input state only.
    #[must_use]
    pub fn authority(&self) -> Option<&RuntimeSessionAuthority> {
        self.authority.as_ref()
    }
}

/// Errors from RuntimeStore operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeStoreError {
    /// Write failed.
    #[error("Store write failed: {0}")]
    WriteFailed(String),
    /// Read failed.
    #[error("Store read failed: {0}")]
    ReadFailed(String),
    /// The explicit session-store key does not match the serialized session.
    #[error("Session store key mismatch: expected {expected}, actual {actual}")]
    SessionKeyMismatch {
        expected: meerkat_core::types::SessionId,
        actual: meerkat_core::types::SessionId,
    },
    /// Not found.
    #[error("Not found: {0}")]
    NotFound(String),
    /// Operation is not supported by this store implementation.
    #[error("Unsupported store operation: {0}")]
    Unsupported(String),
    /// Direct-member semantic authority is malformed before durable admission.
    #[error("Invalid direct-member incarnation: {reason}")]
    InvalidDirectMemberIncarnation { reason: String },
    /// A non-whole-blob store declared a profile but did not implement the
    /// prepared boundary method required to commit that representation.
    #[error("runtime store profile '{profile}' must override commit_prepared_session_boundary")]
    PreparedSessionBoundaryRequiresOverride {
        profile: RuntimeSessionPersistenceProfile,
    },
    /// Recovery needs a backend that can CAS both runtime authority and the
    /// independently observed physical session head in one transaction.
    #[error(
        "runtime store profile '{profile}' cannot atomically CAS the physical session head for prepared recovery"
    )]
    PreparedRecoveryRequiresAtomicPhysicalHeadCas {
        profile: RuntimeSessionPersistenceProfile,
    },
    /// A head-canonical store encountered durable whole-blob state that has
    /// not completed its explicit, observable profile-activation conversion.
    ///
    /// Ordinary boundary application must never perform this conversion
    /// implicitly: it can be O(document) and may take long enough to require
    /// deploy-facing progress reporting. Open the store through its explicit
    /// activation seam and retry only after that seam reports completion.
    #[error(
        "head-canonical profile activation is required for runtime '{runtime_id}' (state: {state})"
    )]
    HeadCanonicalActivationRequired {
        /// Logical runtime whose frozen predecessor is not activated.
        runtime_id: String,
        /// Durable activation state (`not_started`, `in_progress`, or a
        /// backend-specific refusal detail).
        state: String,
    },
    /// Persisted session authority conflicts with the configured profile,
    /// checkpoint, canonical head, frozen legacy BLOB, or mutation shape.
    #[error("session persistence authority conflict for runtime '{runtime_id}': {detail}")]
    SessionPersistenceAuthorityConflict { runtime_id: String, detail: String },
    /// A detached producer attempted to persist an ops snapshot after the
    /// matching epoch was atomically retired by unregister.
    #[error("Ops lifecycle epoch {epoch_id} for runtime {runtime_id} is retired")]
    OpsLifecycleEpochRetired {
        runtime_id: String,
        epoch_id: meerkat_core::RuntimeEpochId,
    },
    /// An unregister-finalization commit may have become durable, but the
    /// backend could not authoritatively classify its outcome.
    ///
    /// Callers must retry the idempotent atomic finalization and must not
    /// publish a compensating lifecycle rollback for this error.
    #[error("Unregister finalization outcome is unknown: {0}")]
    UnregisterFinalizationOutcomeUnknown(String),
    /// Runtime snapshot CAS rejected a stale transcript rewrite.
    #[error("Transcript revision conflict: expected {expected}, actual {actual}")]
    TranscriptRevisionConflict { expected: String, actual: String },
    /// An atomic boundary commit carried a session snapshot that was already
    /// superseded by the durable append-only head. Callers must observe this
    /// as a failed commit rather than mistaking a no-op for publication.
    #[error("Session snapshot for runtime '{runtime_id}' was superseded by the durable head")]
    SessionSnapshotSuperseded { runtime_id: String },
    /// The requested exact input-state batch CAS has an invalid row/key shape.
    #[error("Invalid input-state batch compare-and-swap: {reason}")]
    InvalidInputStateBatchCas { reason: String },
    /// The maintained idempotency-key index cannot prove a unique answer while
    /// a source input row's key identity is unindexable.
    ///
    /// This is durable corruption evidence, not an authoritative miss and not
    /// a transient read failure. Callers must fail closed until the named row
    /// is repaired or quarantined through an operator-authorized workflow.
    #[error(
        "input idempotency index for runtime '{runtime_id}' cannot prove key '{key}' while \
         input row '{evidence_input_id}' is unindexable: {reason}"
    )]
    InputIdempotencyIndexUncertain {
        runtime_id: String,
        key: String,
        evidence_input_id: String,
        reason: String,
    },
    /// A lifecycle record was observed exactly, but replacing it would risk
    /// lowering or fabricating durable runtime fencing authority.
    ///
    /// This is a permanent reconciliation result for the observed row, not a
    /// transport retry. Callers should project RepairBlocked while retaining
    /// the evidence digest for operator repair.
    #[error("Machine lifecycle repair is blocked: {detail}")]
    MachineLifecycleRepairBlocked {
        evidence_digest: Option<String>,
        detail: String,
    },
    /// The file's schema ledger records a version newer than this binary
    /// supports: a newer binary migrated the file and this one must refuse
    /// it (typed, health-visible refusal — never a crash loop).
    #[error(
        "schema for domain '{domain}' is from the future: file has version {found}, \
         this binary supports up to {supported}"
    )]
    SchemaFromTheFuture {
        domain: String,
        found: i64,
        supported: i64,
    },
    /// The exclusive maintenance fence is held for this database; storage is
    /// under offline maintenance.
    #[error("maintenance fence is held for '{path}'; storage is under offline maintenance")]
    MaintenanceFenceHeld { path: String },
    /// An input-state update carried an expected prior row version that no
    /// longer matches the stored row. The whole atomic boundary fails stale;
    /// nothing is written.
    #[error(
        "Input row version conflict for input '{input_id}': the stored row changed since it was observed"
    )]
    InputRowVersionConflict { input_id: String },
    /// The complete set of nonterminal input rows changed after recovery
    /// classified it. The whole recovery boundary fails stale; nothing is
    /// written.
    #[error(
        "Recovery input-set conflict for runtime '{runtime_id}': the nonterminal input set changed since it was observed"
    )]
    RecoveryInputSetConflict { runtime_id: String },
    /// A machine-lifecycle commit carried an expected prior row version that
    /// no longer matches the stored row. The whole atomic boundary fails
    /// stale; nothing is written.
    #[error(
        "Machine lifecycle version conflict for runtime '{runtime_id}': the stored row changed since it was observed"
    )]
    MachineLifecycleVersionConflict { runtime_id: String },
    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Classify whether `candidate` may become the durable direct-member semantic
/// high-water for one member session.
///
/// The record deliberately contains no runtime bearer token. Equal semantic
/// authority may be rebound after a runtime epoch replacement; only a newer
/// generation/fence pair for the same Mob member identity may supersede it.
pub(crate) fn direct_member_high_water_accepts(
    current: &meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation,
    candidate: &meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation,
) -> bool {
    current == candidate
        || (current.mob_id == candidate.mob_id
            && current.agent_identity == candidate.agent_identity
            && (candidate.generation, candidate.fence_token)
                > (current.generation, current.fence_token))
}

pub(crate) fn validate_direct_member_high_water_candidate(
    member_session_id: &str,
    candidate: &meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation,
) -> Result<(), RuntimeStoreError> {
    if member_session_id.is_empty()
        || candidate.mob_id.is_empty()
        || candidate.agent_identity.is_empty()
        || candidate.fence_token == 0
    {
        return Err(RuntimeStoreError::InvalidDirectMemberIncarnation {
            reason: "member session, mob, and agent identities must be non-empty and fence token must be nonzero"
                .to_string(),
        });
    }
    Ok(())
}

/// Transactional updater for the runtime-owned OAuth login-flow payload snapshot.
pub type AuthOAuthFlowSnapshotUpdate<'a> =
    dyn FnMut(Option<&[u8]>) -> Result<Vec<u8>, RuntimeStoreError> + 'a;

/// Describes a serialized session snapshot for boundary and snapshot-only commits.
#[derive(Debug, Clone)]
pub struct SerializedSessionSnapshot {
    /// Immutable serialized session snapshot (opaque to RuntimeStore).
    ///
    /// The shared owner is part of the WholeBlob cost contract: a prepared
    /// boundary must carry the one materialized document through the atomic
    /// store verb without allocating and copying a second full buffer.
    pub session_snapshot: std::sync::Arc<Vec<u8>>,
}

fn recovery_class_name(
    class: crate::meerkat_machine::dsl::DurableTailRecoveryClass,
) -> &'static str {
    use crate::meerkat_machine::dsl::DurableTailRecoveryClass;
    match class {
        DurableTailRecoveryClass::CompletedCandidate => "completed_candidate",
        DurableTailRecoveryClass::InterruptedRepairableCandidate => {
            "interrupted_repairable_candidate"
        }
        DurableTailRecoveryClass::Ambiguous => "ambiguous",
    }
}

fn recovery_class_from_name(
    name: &str,
) -> Result<crate::meerkat_machine::dsl::DurableTailRecoveryClass, RuntimeStoreError> {
    use crate::meerkat_machine::dsl::DurableTailRecoveryClass;
    match name {
        "completed_candidate" => Ok(DurableTailRecoveryClass::CompletedCandidate),
        "interrupted_repairable_candidate" => {
            Ok(DurableTailRecoveryClass::InterruptedRepairableCandidate)
        }
        "ambiguous" => Ok(DurableTailRecoveryClass::Ambiguous),
        other => Err(RuntimeStoreError::ReadFailed(format!(
            "unknown committed recovery class '{other}'"
        ))),
    }
}

fn recovery_disposition_name(
    disposition: crate::meerkat_machine::dsl::DurableTailRecoveryDisposition,
) -> &'static str {
    use crate::meerkat_machine::dsl::DurableTailRecoveryDisposition;
    match disposition {
        DurableTailRecoveryDisposition::RefuseRecovery => "refuse_recovery",
        DurableTailRecoveryDisposition::CommitCompleted => "commit_completed",
        DurableTailRecoveryDisposition::RepairAndCommitInterrupted => {
            "repair_and_commit_interrupted"
        }
        DurableTailRecoveryDisposition::CommitCompletedRetainInputs => {
            "commit_completed_retain_inputs"
        }
        DurableTailRecoveryDisposition::HoldIntact => "hold_intact",
    }
}

fn recovery_disposition_from_name(
    name: &str,
) -> Result<crate::meerkat_machine::dsl::DurableTailRecoveryDisposition, RuntimeStoreError> {
    use crate::meerkat_machine::dsl::DurableTailRecoveryDisposition;
    match name {
        "refuse_recovery" => Ok(DurableTailRecoveryDisposition::RefuseRecovery),
        "commit_completed" => Ok(DurableTailRecoveryDisposition::CommitCompleted),
        "repair_and_commit_interrupted" => {
            Ok(DurableTailRecoveryDisposition::RepairAndCommitInterrupted)
        }
        "commit_completed_retain_inputs" => {
            Ok(DurableTailRecoveryDisposition::CommitCompletedRetainInputs)
        }
        "hold_intact" => Ok(DurableTailRecoveryDisposition::HoldIntact),
        other => Err(RuntimeStoreError::ReadFailed(format!(
            "unknown committed recovery disposition '{other}'"
        ))),
    }
}

fn recovery_hash_part(hasher: &mut Sha256, label: &str, bytes: &[u8]) {
    hasher.update((label.len() as u64).to_be_bytes());
    hasher.update(label.as_bytes());
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn lifecycle_expected_version_token(
    lifecycle: &MachineLifecycleCommit,
) -> Result<String, RuntimeStoreError> {
    match lifecycle.expected_version() {
        Some(MachineLifecycleExpectedVersion::Missing) => Ok("missing".to_string()),
        Some(MachineLifecycleExpectedVersion::Version(version)) => {
            Ok(format!("version:{}", version.as_str()))
        }
        None => Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: "prepared-recovery".to_string(),
            detail: "recovery lifecycle commit is not fenced on an exact observed row".to_string(),
        }),
    }
}

fn recovery_sha256_token(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn is_canonical_sha256_token(token: &str) -> bool {
    let Some(hex) = token.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Exact stored input row resolved through the durable idempotency-key index.
#[derive(Debug, Clone)]
pub struct ExactInputStateObservation {
    state: StoredInputState,
    exact_row_digest: String,
}

impl ExactInputStateObservation {
    /// Bind a decoded state to the exact bytes the store resolved.
    pub fn from_exact_stored_row(
        state: StoredInputState,
        exact_row_digest: String,
    ) -> Result<Self, RuntimeStoreError> {
        if !is_canonical_sha256_token(&exact_row_digest) {
            return Err(RuntimeStoreError::ReadFailed(format!(
                "input {} has a malformed exact-row digest",
                state.state.input_id
            )));
        }
        Ok(Self {
            state,
            exact_row_digest,
        })
    }

    /// Decoded stored input state.
    #[must_use]
    pub fn state(&self) -> &StoredInputState {
        &self.state
    }

    /// Exact digest of the backend row bytes observed with the lookup.
    #[must_use]
    pub fn exact_row_digest(&self) -> &str {
        &self.exact_row_digest
    }

    /// Consume the observation into decoded state and exact row digest.
    #[must_use]
    pub fn into_parts(self) -> (StoredInputState, String) {
        (self.state, self.exact_row_digest)
    }
}

/// Store-owned monotonic revision of one logical runtime's input-row set.
///
/// The value is opaque to recovery callers. A store mints it from its own
/// transactionally maintained generation and MUST advance that generation for
/// every insert, update, or delete of an input row for the runtime. Revision
/// zero is the canonical generation for a runtime that has never owned an
/// input row; it is still a real absence fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecoveryInputSetRevision(u64);

impl RecoveryInputSetRevision {
    /// Mint a revision from a store-owned monotonic generation.
    #[must_use]
    pub fn from_store_generation(generation: u64) -> Self {
        Self(generation)
    }

    /// Return the opaque generation for an exact store-side comparison.
    #[must_use]
    pub fn store_generation(self) -> u64 {
        self.0
    }
}

/// Exact, store-owned observation of every nonterminal input row that can
/// affect durable-tail recovery for one logical runtime.
///
/// Runtime-store implementations construct this value from an authoritative,
/// complete read of their persisted nonterminal-input set. Every row token
/// must be the canonical `sha256:<lowercase hex>` digest of the exact stored
/// row representation that the backend will compare in its atomic recovery
/// commit. An empty row vector is not an unproved absence: it produces a
/// domain-separated absence token scoped to `runtime_id`.
///
/// The exact set token is durable evidence of what was classified. A
/// recovery-capable backend MUST also compare [`Self::input_set_revision`]
/// against its current store-owned generation inside the transaction that
/// applies recovery. A row inserted, removed, terminalized, reopened, or
/// byte-modified after this observation must make that transaction fail with
/// [`RuntimeStoreError::RecoveryInputSetConflict`] without rescanning the set.
#[derive(Debug, Clone)]
pub struct PreparedRecoveryInputSnapshot {
    runtime_id: LogicalRuntimeId,
    input_set_revision: RecoveryInputSetRevision,
    rows: Vec<(StoredInputState, String)>,
    exact_set_token: String,
}

impl PreparedRecoveryInputSnapshot {
    /// Seal an authoritative complete set of exact nonterminal input rows.
    ///
    /// The constructor canonicalizes row order by [`InputId`], rejects
    /// duplicates, terminal rows, and non-canonical exact-row tokens, then
    /// hashes the runtime id, row count, and every `(input_id, row_token)`
    /// pair with length framing under `meerkat.recovery-input-set.v1`.
    ///
    /// This constructor validates representation, not completeness. The
    /// [`RuntimeStore`] implementation owns the obligation to select all and
    /// only persisted nonterminal rows for the supplied runtime and to observe
    /// `input_set_revision` in the same backend snapshot as those rows.
    pub fn from_exact_nonterminal_rows(
        runtime_id: LogicalRuntimeId,
        input_set_revision: RecoveryInputSetRevision,
        mut rows: Vec<(StoredInputState, String)>,
    ) -> Result<Self, RuntimeStoreError> {
        if runtime_id.0.is_empty() {
            return Err(RuntimeStoreError::ReadFailed(
                "recovery input snapshot has an empty logical runtime id".to_string(),
            ));
        }
        rows.sort_by(|(left, _), (right, _)| {
            left.state
                .input_id
                .to_string()
                .cmp(&right.state.input_id.to_string())
        });
        for (index, (state, row_token)) in rows.iter().enumerate() {
            if !input_state_is_recovery_nonterminal(state) {
                return Err(RuntimeStoreError::ReadFailed(format!(
                    "recovery input snapshot includes terminal input {}",
                    state.state.input_id
                )));
            }
            if !is_canonical_sha256_token(row_token) {
                return Err(RuntimeStoreError::ReadFailed(format!(
                    "recovery input snapshot row {} has a malformed exact-row token",
                    state.state.input_id
                )));
            }
            if index > 0 && rows[index - 1].0.state.input_id == state.state.input_id {
                return Err(RuntimeStoreError::ReadFailed(format!(
                    "recovery input snapshot repeats input {}",
                    state.state.input_id
                )));
            }
        }

        let mut hasher = Sha256::new();
        recovery_hash_part(&mut hasher, "domain", b"meerkat.recovery-input-set.v1");
        recovery_hash_part(&mut hasher, "runtime_id", runtime_id.0.as_bytes());
        recovery_hash_part(
            &mut hasher,
            "nonterminal_row_count",
            &(rows.len() as u64).to_be_bytes(),
        );
        for (state, row_token) in &rows {
            recovery_hash_part(
                &mut hasher,
                "input_id",
                state.state.input_id.to_string().as_bytes(),
            );
            recovery_hash_part(&mut hasher, "exact_row_token", row_token.as_bytes());
        }
        let exact_set_token = format!("sha256:{:x}", hasher.finalize());
        Ok(Self {
            runtime_id,
            input_set_revision,
            rows,
            exact_set_token,
        })
    }

    /// Logical runtime whose complete nonterminal set was observed.
    #[must_use]
    pub fn runtime_id(&self) -> &LogicalRuntimeId {
        &self.runtime_id
    }

    /// Store-owned input-set revision observed with the exact rows.
    #[must_use]
    pub fn input_set_revision(&self) -> RecoveryInputSetRevision {
        self.input_set_revision
    }

    /// Exact set/absence token sealed into prepared recovery evidence.
    #[must_use]
    pub fn exact_set_token(&self) -> &str {
        &self.exact_set_token
    }

    /// Consume the snapshot into canonical rows, store revision, and exact
    /// set/absence token.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        Vec<(StoredInputState, String)>,
        RecoveryInputSetRevision,
        String,
    ) {
        (self.rows, self.input_set_revision, self.exact_set_token)
    }
}

/// Exact predecessor authority for deleting one recovery-discarded input row.
///
/// Construction is crate-owned by the generated recovery path. Public store
/// implementations can inspect the values but callers cannot turn omission
/// from a target image into delete authority.
#[derive(Debug, Clone)]
pub struct PreparedRecoveryInputDelete {
    input_id: InputId,
    expected_row_digest: String,
}

impl PreparedRecoveryInputDelete {
    pub(crate) fn from_exact_observation(
        input_id: InputId,
        expected_row_digest: String,
    ) -> Result<Self, RuntimeStoreError> {
        if !is_canonical_sha256_token(&expected_row_digest) {
            return Err(RuntimeStoreError::InvalidInputStateBatchCas {
                reason: format!(
                    "recovery delete for input {input_id} has a malformed predecessor digest"
                ),
            });
        }
        Ok(Self {
            input_id,
            expected_row_digest,
        })
    }

    /// Input row removed by the machine-authorized recovery disposition.
    #[must_use]
    pub fn input_id(&self) -> &InputId {
        &self.input_id
    }

    /// Exact digest of the predecessor bytes the delete must match.
    #[must_use]
    pub fn expected_row_digest(&self) -> &str {
        &self.expected_row_digest
    }
}

/// One machine-authorized mutation in a cold-recovery input boundary.
#[derive(Debug, Clone)]
pub enum RecoveryInputStateMutation {
    /// Retained input row normalized to its recovered machine image.
    Upsert(InputStatePersistenceRecord),
    /// Ephemeral/discarded input row removed under exact predecessor authority.
    Delete(PreparedRecoveryInputDelete),
}

impl RecoveryInputStateMutation {
    /// Prepare an exact delete from the row digest returned by the recovery
    /// snapshot. Crate-owned because only the generated recovery classifier
    /// can authorize the discard disposition.
    pub(crate) fn delete(
        input_id: InputId,
        expected_row_digest: String,
    ) -> Result<Self, RuntimeStoreError> {
        PreparedRecoveryInputDelete::from_exact_observation(input_id, expected_row_digest)
            .map(Self::Delete)
    }
}

/// One recovery input mutation paired with the exact serialized target and
/// predecessor digest that authorize it.
///
/// `InputStatePersistenceRecord` intentionally does not implement equality:
/// equality here is defined by the exact durable bytes and CAS predecessor
/// sealed into the recovery witness.
#[derive(Debug, Clone)]
struct PreparedRecoveryInputUpdate {
    record: InputStatePersistenceRecord,
    input_id: InputId,
    expected_row_digest: String,
    target_bytes: Vec<u8>,
}

impl PartialEq for PreparedRecoveryInputUpdate {
    fn eq(&self, other: &Self) -> bool {
        self.input_id == other.input_id
            && self.expected_row_digest == other.expected_row_digest
            && self.target_bytes == other.target_bytes
    }
}

impl Eq for PreparedRecoveryInputUpdate {}

impl PreparedRecoveryInputUpdate {
    fn seal(record: InputStatePersistenceRecord) -> Result<Self, String> {
        let input_id = record.as_stored().state.input_id.clone();
        let expected_row_digest = record
            .expected_row_digest()
            .ok_or_else(|| {
                format!("recovery input {input_id} is not fenced on an exact predecessor row")
            })?
            .to_string();
        if !is_canonical_sha256_token(&expected_row_digest) {
            return Err(format!(
                "recovery input {input_id} has a malformed predecessor digest"
            ));
        }
        let target_bytes = serde_json::to_vec(record.as_stored()).map_err(|error| {
            format!("failed to encode exact recovery input target {input_id}: {error}")
        })?;
        Ok(Self {
            record,
            input_id,
            expected_row_digest,
            target_bytes,
        })
    }

    fn decode(
        input_id: InputId,
        expected_row_digest: String,
        target_bytes: Vec<u8>,
    ) -> Result<Self, String> {
        if !is_canonical_sha256_token(&expected_row_digest) {
            return Err(format!(
                "recovery input {input_id} has a malformed predecessor digest"
            ));
        }
        if target_bytes.is_empty() {
            return Err(format!(
                "recovery input {input_id} has an empty serialized target"
            ));
        }
        let bundle: StoredInputState = serde_json::from_slice(&target_bytes)
            .map_err(|error| format!("recovery input {input_id} target is invalid: {error}"))?;
        if bundle.state.input_id != input_id {
            return Err(format!(
                "recovery input target {} differs from sealed input {input_id}",
                bundle.state.input_id
            ));
        }
        let canonical_target_bytes = serde_json::to_vec(&bundle).map_err(|error| {
            format!("failed to canonicalize recovery input target {input_id}: {error}")
        })?;
        if canonical_target_bytes != target_bytes {
            return Err(format!(
                "recovery input {input_id} target is not in its canonical serialized form"
            ));
        }
        let record = InputStatePersistenceRecord::from_machine_snapshot(bundle)
            .map_err(|error| {
                format!("recovery input {input_id} target is not machine-authorized: {error}")
            })?
            .with_expected_row_digest(expected_row_digest.clone());
        Ok(Self {
            record,
            input_id,
            expected_row_digest,
            target_bytes,
        })
    }
}

#[derive(Debug)]
pub(crate) enum PreparedRecoveryInputStateMutation {
    Upsert {
        replacement: StoredInputState,
        expected_row_digest: String,
    },
    Delete {
        input_id: InputId,
        expected_row_digest: String,
    },
}

impl PreparedRecoveryInputStateMutation {
    pub(crate) fn input_id(&self) -> &InputId {
        match self {
            Self::Upsert { replacement, .. } => &replacement.state.input_id,
            Self::Delete { input_id, .. } => input_id,
        }
    }

    pub(crate) fn expected_row_digest(&self) -> &str {
        match self {
            Self::Upsert {
                expected_row_digest,
                ..
            }
            | Self::Delete {
                expected_row_digest,
                ..
            } => expected_row_digest,
        }
    }
}

/// Prepare an unbounded recovery input mutation set in canonical key order.
///
/// Recovery is scoped by the store-owned input-set revision rather than the
/// ordinary directed-terminal batch limit. Every target still carries an
/// exact predecessor-row digest, and duplicate identities are rejected.
pub(crate) fn prepare_recovery_input_state_mutations(
    mutations: &[RecoveryInputStateMutation],
) -> Result<Vec<PreparedRecoveryInputStateMutation>, RuntimeStoreError> {
    let mut prepared = mutations
        .iter()
        .cloned()
        .map(|mutation| match mutation {
            RecoveryInputStateMutation::Upsert(record) => {
                let update = PreparedRecoveryInputUpdate::seal(record)
                    .map_err(|reason| RuntimeStoreError::InvalidInputStateBatchCas { reason })?;
                Ok(PreparedRecoveryInputStateMutation::Upsert {
                    replacement: update.record.clone_stored(),
                    expected_row_digest: update.expected_row_digest,
                })
            }
            RecoveryInputStateMutation::Delete(delete) => {
                if !is_canonical_sha256_token(&delete.expected_row_digest) {
                    return Err(RuntimeStoreError::InvalidInputStateBatchCas {
                        reason: format!(
                            "recovery delete for input {} has a malformed predecessor digest",
                            delete.input_id
                        ),
                    });
                }
                Ok(PreparedRecoveryInputStateMutation::Delete {
                    input_id: delete.input_id,
                    expected_row_digest: delete.expected_row_digest,
                })
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    prepared.sort_by(|left, right| {
        left.input_id()
            .to_string()
            .cmp(&right.input_id().to_string())
    });
    if prepared
        .windows(2)
        .any(|pair| pair[0].input_id() == pair[1].input_id())
    {
        return Err(RuntimeStoreError::InvalidInputStateBatchCas {
            reason: "recovery input mutations must have unique input ids".to_string(),
        });
    }
    Ok(prepared)
}

fn validate_recovery_input_update_order(
    input_updates: &[PreparedRecoveryInputUpdate],
) -> Result<(), String> {
    if input_updates
        .windows(2)
        .any(|window| window[0].input_id.0 >= window[1].input_id.0)
    {
        return Err(
            "recovery input updates must have unique input ids in canonical order".to_string(),
        );
    }
    Ok(())
}

/// Exact store-issued authority transition sealed into one recovery witness.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PreparedRecoverySessionAuthority {
    WholeBlob {
        base_store_revision: u64,
        base_blob_sha256: String,
        provisional_candidate_blob_sha256: String,
        provisional_candidate_sequence: u64,
        recovered_blob_sha256: String,
    },
    HeadCanonical {
        committed_store_revision: u64,
        committed_head_token: String,
        physical_store_revision: u64,
        physical_head_token: String,
        recovered_head_token: String,
    },
}

/// Machine-authorized recovery evidence sealed to one exact recovered
/// document, receipt, lifecycle target, complete predecessor nonterminal
/// input-set/absence token, and canonically ordered input-update set.
///
/// Fields are private and construction is crate-only. Store implementations
/// may inspect the paired values but cannot mint a recovery classification or
/// replace any one proof independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRecoveryEvidence {
    session_id: meerkat_core::types::SessionId,
    candidate_id: String,
    candidate_run_id: RunId,
    class: crate::meerkat_machine::dsl::DurableTailRecoveryClass,
    disposition: crate::meerkat_machine::dsl::DurableTailRecoveryDisposition,
    session_authority: PreparedRecoverySessionAuthority,
    receipt_digest_enrichments: Vec<PreparedRecoveryReceiptDigestEnrichment>,
    predecessor_nonterminal_input_set_revision: RecoveryInputSetRevision,
    predecessor_nonterminal_input_set_token: String,
    input_updates: Vec<PreparedRecoveryInputUpdate>,
    lifecycle_target_token: String,
    lifecycle_target_bytes: Vec<u8>,
    exact_witness: String,
}

impl PreparedRecoveryEvidence {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn seal_head_canonical(
        recovered: &meerkat_core::Session,
        document: &BoundSessionCommit,
        session_id: meerkat_core::types::SessionId,
        candidate_id: String,
        candidate_run_id: RunId,
        class: crate::meerkat_machine::dsl::DurableTailRecoveryClass,
        disposition: crate::meerkat_machine::dsl::DurableTailRecoveryDisposition,
        committed_store_revision: u64,
        committed_head_token: String,
        physical_store_revision: u64,
        physical_head_token: String,
        recovered_head_token: String,
        receipt_digest_enrichments: Vec<PreparedRecoveryReceiptDigestEnrichment>,
        predecessor_nonterminal_input_set_revision: RecoveryInputSetRevision,
        predecessor_nonterminal_input_set_token: String,
        input_updates: Vec<InputStatePersistenceRecord>,
        receipt: &RunBoundaryReceipt,
        lifecycle: &MachineLifecycleCommit,
    ) -> Result<Self, RuntimeStoreError> {
        let conflict = |detail: String| RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: session_id.to_string(),
            detail,
        };
        if candidate_id.is_empty()
            || committed_store_revision == 0
            || physical_store_revision <= committed_store_revision
            || committed_head_token.is_empty()
            || physical_head_token.is_empty()
            || recovered_head_token.is_empty()
            || committed_head_token == physical_head_token
            || !is_canonical_sha256_token(&predecessor_nonterminal_input_set_token)
        {
            return Err(conflict(
                "prepared recovery contains an invalid store-issued authority transition"
                    .to_string(),
            ));
        }
        let valid_disposition = matches!(
            (class, disposition),
            (
                crate::meerkat_machine::dsl::DurableTailRecoveryClass::CompletedCandidate,
                crate::meerkat_machine::dsl::DurableTailRecoveryDisposition::CommitCompleted
                    | crate::meerkat_machine::dsl::DurableTailRecoveryDisposition::CommitCompletedRetainInputs
            ) | (
                crate::meerkat_machine::dsl::DurableTailRecoveryClass::InterruptedRepairableCandidate,
                crate::meerkat_machine::dsl::DurableTailRecoveryDisposition::RepairAndCommitInterrupted
            )
        );
        if !valid_disposition {
            return Err(conflict(format!(
                "recovery class {} cannot realize disposition {}",
                recovery_class_name(class),
                recovery_disposition_name(disposition)
            )));
        }

        if recovered.id() != &session_id {
            return Err(conflict(format!(
                "prepared recovery document belongs to {}, not {session_id}",
                recovered.id()
            )));
        }
        let head_boundary = document.head_canonical().ok_or_else(|| {
            conflict("prepared recovery has no sealed head-canonical mutation".to_string())
        })?;
        let successor_head = head_boundary.mutation().successor_head();
        let recovered_message_count = u64::try_from(recovered.messages().len()).map_err(|_| {
            conflict("recovered document message count exceeds u64 authority".to_string())
        })?;
        let conversation_digest = recovered.transcript_content_digest().map_err(|error| {
            conflict(format!(
                "failed to derive recovered conversation digest: {error}"
            ))
        })?;
        let metadata_matches =
            successor_head
                .matches_session_metadata(recovered)
                .map_err(|error| {
                    conflict(format!(
                        "failed to compare recovered document metadata authority: {error}"
                    ))
                })?;
        if &successor_head.id != recovered.id()
            || successor_head.version != recovered.version()
            || successor_head.head_revision != conversation_digest
            || successor_head.message_count != recovered_message_count
            || successor_head.created_at != recovered.created_at()
            || successor_head.updated_at != recovered.updated_at()
            || successor_head.usage != recovered.total_usage()
            || !metadata_matches
        {
            return Err(conflict(
                "prepared recovered head differs from the exact recovered document".to_string(),
            ));
        }
        let derived_recovered_head_token = meerkat_core::session_head_cas_token(successor_head)
            .map_err(|error| {
                conflict(format!(
                    "failed to derive recovered HeadCanonical token: {error}"
                ))
            })?;
        if derived_recovered_head_token != recovered_head_token {
            return Err(conflict(
                "prepared recovered head differs from the sealed successor token".to_string(),
            ));
        }
        if receipt.run_id != candidate_run_id || receipt.message_count != recovered.messages().len()
        {
            return Err(conflict(
                "recovery receipt does not bind the candidate run and exact message count"
                    .to_string(),
            ));
        }
        if receipt.conversation_digest.as_deref() != Some(conversation_digest.as_str()) {
            return Err(conflict(
                "recovery receipt does not bind the exact recovered conversation".to_string(),
            ));
        }
        let mut previous_enrichment_sequence = None;
        for enrichment in &receipt_digest_enrichments {
            let original = enrichment.original_receipt();
            if original.run_id != candidate_run_id
                || original.conversation_digest.is_some()
                || previous_enrichment_sequence
                    .is_some_and(|previous| original.sequence <= previous)
                || original.message_count > recovered.messages().len()
            {
                return Err(conflict(
                    "prepared recovery receipt enrichment has an invalid run, sequence, count, or pre-migration shape"
                        .to_string(),
                ));
            }
            let derived = recovered
                .transcript_prefix_digest(original.message_count)
                .map_err(|error| {
                    conflict(format!(
                        "failed to verify recovery receipt enrichment prefix: {error}"
                    ))
                })?;
            if derived != enrichment.derived_conversation_digest()
                || enrichment.original_exact_row_token().is_empty()
            {
                return Err(conflict(
                    "prepared recovery receipt enrichment differs from the exact recovered transcript prefix"
                        .to_string(),
                ));
            }
            previous_enrichment_sequence = Some(original.sequence);
        }

        let input_updates = input_updates
            .into_iter()
            .map(PreparedRecoveryInputUpdate::seal)
            .collect::<Result<Vec<_>, _>>()
            .map_err(&conflict)?;
        validate_recovery_input_update_order(&input_updates).map_err(&conflict)?;
        let lifecycle_target_bytes = lifecycle.store_record().encode()?;
        let lifecycle_target_token = recovery_sha256_token(&lifecycle_target_bytes);
        // The expected version is a first-apply fence, not outcome identity:
        // after a successful commit the exact lifecycle target has a new row
        // version. Require the fence to exist, but do not bake that transient
        // predecessor token into the durable retry witness.
        let _ = lifecycle_expected_version_token(lifecycle)?;
        let session_authority = PreparedRecoverySessionAuthority::HeadCanonical {
            committed_store_revision,
            committed_head_token,
            physical_store_revision,
            physical_head_token,
            recovered_head_token,
        };

        let mut evidence = Self {
            session_id,
            candidate_id,
            candidate_run_id,
            class,
            disposition,
            session_authority,
            receipt_digest_enrichments,
            predecessor_nonterminal_input_set_revision,
            predecessor_nonterminal_input_set_token,
            input_updates,
            lifecycle_target_token,
            lifecycle_target_bytes,
            exact_witness: String::new(),
        };
        evidence.exact_witness = evidence.compute_exact_witness(receipt).map_err(|error| {
            RuntimeStoreError::WriteFailed(format!(
                "failed to encode exact recovery witness: {error}"
            ))
        })?;
        evidence.verify_head_canonical_boundary(document, receipt)?;
        Ok(evidence)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn seal_whole_blob(
        recovered: &meerkat_core::Session,
        repaired_document: Option<&BoundSessionCommit>,
        session_id: meerkat_core::types::SessionId,
        candidate_id: String,
        candidate_run_id: RunId,
        class: crate::meerkat_machine::dsl::DurableTailRecoveryClass,
        disposition: crate::meerkat_machine::dsl::DurableTailRecoveryDisposition,
        base_store_revision: u64,
        base_blob_sha256: String,
        provisional_candidate_blob_sha256: String,
        provisional_candidate_sequence: u64,
        recovered_blob_sha256: String,
        receipt_digest_enrichments: Vec<PreparedRecoveryReceiptDigestEnrichment>,
        predecessor_nonterminal_input_set_revision: RecoveryInputSetRevision,
        predecessor_nonterminal_input_set_token: String,
        input_updates: Vec<InputStatePersistenceRecord>,
        receipt: &RunBoundaryReceipt,
        lifecycle: &MachineLifecycleCommit,
    ) -> Result<Self, RuntimeStoreError> {
        let conflict = |detail: String| RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: session_id.to_string(),
            detail,
        };
        if candidate_id.is_empty()
            || base_store_revision == 0
            || base_blob_sha256.is_empty()
            || provisional_candidate_blob_sha256.is_empty()
            || provisional_candidate_sequence == 0
            || recovered_blob_sha256.is_empty()
            || !is_canonical_sha256_token(&predecessor_nonterminal_input_set_token)
        {
            return Err(conflict(
                "prepared WholeBlob recovery contains an invalid store-issued authority transition"
                    .to_string(),
            ));
        }
        if recovered.id() != &session_id {
            return Err(conflict(format!(
                "prepared recovery document belongs to {}, not {session_id}",
                recovered.id()
            )));
        }
        let valid_disposition = matches!(
            (class, disposition),
            (
                crate::meerkat_machine::dsl::DurableTailRecoveryClass::CompletedCandidate,
                crate::meerkat_machine::dsl::DurableTailRecoveryDisposition::CommitCompleted
                    | crate::meerkat_machine::dsl::DurableTailRecoveryDisposition::CommitCompletedRetainInputs
            ) | (
                crate::meerkat_machine::dsl::DurableTailRecoveryClass::InterruptedRepairableCandidate,
                crate::meerkat_machine::dsl::DurableTailRecoveryDisposition::RepairAndCommitInterrupted
            )
        );
        if !valid_disposition {
            return Err(conflict(format!(
                "recovery class {} cannot realize disposition {}",
                recovery_class_name(class),
                recovery_disposition_name(disposition)
            )));
        }
        match class {
            crate::meerkat_machine::dsl::DurableTailRecoveryClass::CompletedCandidate => {
                if recovered_blob_sha256 != provisional_candidate_blob_sha256
                    || repaired_document.is_some()
                {
                    return Err(conflict(
                        "completed WholeBlob recovery must promote the exact provisional candidate without a repaired artifact"
                            .to_string(),
                    ));
                }
            }
            crate::meerkat_machine::dsl::DurableTailRecoveryClass::InterruptedRepairableCandidate => {
                if recovered_blob_sha256 == provisional_candidate_blob_sha256 {
                    return Err(conflict(
                        "interrupted WholeBlob recovery must install a distinct repaired artifact"
                            .to_string(),
                    ));
                }
                let document = repaired_document.ok_or_else(|| {
                    conflict(
                        "interrupted WholeBlob recovery has no sealed repaired artifact"
                            .to_string(),
                    )
                })?;
                if document.head_canonical().is_some() {
                    return Err(conflict(
                        "WholeBlob recovery unexpectedly carries a HeadCanonical mutation"
                            .to_string(),
                    ));
                }
                let artifact = document.whole_blob_artifact().map_err(|error| {
                    conflict(format!(
                        "failed to materialize recovered WholeBlob artifact: {error}"
                    ))
                })?;
                if artifact.row_sha256_token() != recovered_blob_sha256 {
                    return Err(conflict(
                        "recovered WholeBlob bytes differ from the sealed successor digest"
                            .to_string(),
                    ));
                }
            }
            crate::meerkat_machine::dsl::DurableTailRecoveryClass::Ambiguous => {
                return Err(conflict(
                    "ambiguous WholeBlob recovery cannot be sealed".to_string(),
                ));
            }
        }
        if receipt.run_id != candidate_run_id || receipt.message_count != recovered.messages().len()
        {
            return Err(conflict(
                "recovery receipt does not bind the candidate run and exact message count"
                    .to_string(),
            ));
        }
        let conversation_digest = recovered.transcript_content_digest().map_err(|error| {
            conflict(format!(
                "failed to derive recovered conversation digest: {error}"
            ))
        })?;
        if receipt.conversation_digest.as_deref() != Some(conversation_digest.as_str()) {
            return Err(conflict(
                "recovery receipt does not bind the exact recovered conversation".to_string(),
            ));
        }
        let mut previous_enrichment_sequence = None;
        for enrichment in &receipt_digest_enrichments {
            let original = enrichment.original_receipt();
            if original.run_id != candidate_run_id
                || original.conversation_digest.is_some()
                || previous_enrichment_sequence
                    .is_some_and(|previous| original.sequence <= previous)
                || original.message_count > recovered.messages().len()
            {
                return Err(conflict(
                    "prepared recovery receipt enrichment has an invalid run, sequence, count, or pre-migration shape"
                        .to_string(),
                ));
            }
            let derived = recovered
                .transcript_prefix_digest(original.message_count)
                .map_err(|error| {
                    conflict(format!(
                        "failed to verify recovery receipt enrichment prefix: {error}"
                    ))
                })?;
            if derived != enrichment.derived_conversation_digest()
                || enrichment.original_exact_row_token().is_empty()
            {
                return Err(conflict(
                    "prepared recovery receipt enrichment differs from the exact recovered transcript prefix"
                        .to_string(),
                ));
            }
            previous_enrichment_sequence = Some(original.sequence);
        }
        let input_updates = input_updates
            .into_iter()
            .map(PreparedRecoveryInputUpdate::seal)
            .collect::<Result<Vec<_>, _>>()
            .map_err(&conflict)?;
        validate_recovery_input_update_order(&input_updates).map_err(&conflict)?;
        let lifecycle_target_bytes = lifecycle.store_record().encode()?;
        let lifecycle_target_token = recovery_sha256_token(&lifecycle_target_bytes);
        let _ = lifecycle_expected_version_token(lifecycle)?;
        let mut evidence = Self {
            session_id,
            candidate_id,
            candidate_run_id,
            class,
            disposition,
            session_authority: PreparedRecoverySessionAuthority::WholeBlob {
                base_store_revision,
                base_blob_sha256,
                provisional_candidate_blob_sha256,
                provisional_candidate_sequence,
                recovered_blob_sha256,
            },
            receipt_digest_enrichments,
            predecessor_nonterminal_input_set_revision,
            predecessor_nonterminal_input_set_token,
            input_updates,
            lifecycle_target_token,
            lifecycle_target_bytes,
            exact_witness: String::new(),
        };
        evidence.exact_witness = evidence.compute_exact_witness(receipt).map_err(|error| {
            RuntimeStoreError::WriteFailed(format!(
                "failed to encode exact recovery witness: {error}"
            ))
        })?;
        Ok(evidence)
    }

    fn compute_exact_witness(
        &self,
        receipt: &RunBoundaryReceipt,
    ) -> Result<String, serde_json::Error> {
        let receipt_json = serde_json::to_vec(receipt)?;
        let mut hasher = Sha256::new();
        recovery_hash_part(
            &mut hasher,
            "domain",
            b"meerkat.prepared-recovery-evidence.v6",
        );
        recovery_hash_part(
            &mut hasher,
            "session_id",
            self.session_id.to_string().as_bytes(),
        );
        recovery_hash_part(&mut hasher, "candidate_id", self.candidate_id.as_bytes());
        recovery_hash_part(
            &mut hasher,
            "candidate_run_id",
            self.candidate_run_id.to_string().as_bytes(),
        );
        recovery_hash_part(
            &mut hasher,
            "class",
            recovery_class_name(self.class).as_bytes(),
        );
        recovery_hash_part(
            &mut hasher,
            "disposition",
            recovery_disposition_name(self.disposition).as_bytes(),
        );
        match &self.session_authority {
            PreparedRecoverySessionAuthority::WholeBlob {
                base_store_revision,
                base_blob_sha256,
                provisional_candidate_blob_sha256,
                provisional_candidate_sequence,
                recovered_blob_sha256,
            } => {
                recovery_hash_part(&mut hasher, "profile", b"whole_blob_v1");
                recovery_hash_part(
                    &mut hasher,
                    "base_store_revision",
                    &base_store_revision.to_be_bytes(),
                );
                recovery_hash_part(&mut hasher, "base_blob_sha256", base_blob_sha256.as_bytes());
                recovery_hash_part(
                    &mut hasher,
                    "provisional_candidate_blob_sha256",
                    provisional_candidate_blob_sha256.as_bytes(),
                );
                recovery_hash_part(
                    &mut hasher,
                    "provisional_candidate_sequence",
                    &provisional_candidate_sequence.to_be_bytes(),
                );
                recovery_hash_part(
                    &mut hasher,
                    "recovered_blob_sha256",
                    recovered_blob_sha256.as_bytes(),
                );
            }
            PreparedRecoverySessionAuthority::HeadCanonical {
                committed_store_revision,
                committed_head_token,
                physical_store_revision,
                physical_head_token,
                recovered_head_token,
            } => {
                recovery_hash_part(&mut hasher, "profile", b"head_canonical_v1");
                recovery_hash_part(
                    &mut hasher,
                    "committed_store_revision",
                    &committed_store_revision.to_be_bytes(),
                );
                recovery_hash_part(
                    &mut hasher,
                    "committed_head_token",
                    committed_head_token.as_bytes(),
                );
                recovery_hash_part(
                    &mut hasher,
                    "physical_store_revision",
                    &physical_store_revision.to_be_bytes(),
                );
                recovery_hash_part(
                    &mut hasher,
                    "physical_head_token",
                    physical_head_token.as_bytes(),
                );
                recovery_hash_part(
                    &mut hasher,
                    "recovered_head_token",
                    recovered_head_token.as_bytes(),
                );
            }
        }
        recovery_hash_part(
            &mut hasher,
            "receipt_digest_enrichment_count",
            &(self.receipt_digest_enrichments.len() as u64).to_be_bytes(),
        );
        for enrichment in &self.receipt_digest_enrichments {
            let original_json = serde_json::to_vec(enrichment.original_receipt())?;
            recovery_hash_part(
                &mut hasher,
                "receipt_digest_enrichment_original",
                &original_json,
            );
            recovery_hash_part(
                &mut hasher,
                "receipt_digest_enrichment_original_token",
                enrichment.original_exact_row_token().as_bytes(),
            );
            recovery_hash_part(
                &mut hasher,
                "receipt_digest_enrichment_derived_digest",
                enrichment.derived_conversation_digest().as_bytes(),
            );
        }
        recovery_hash_part(
            &mut hasher,
            "predecessor_nonterminal_input_set_revision",
            &self
                .predecessor_nonterminal_input_set_revision
                .store_generation()
                .to_be_bytes(),
        );
        recovery_hash_part(
            &mut hasher,
            "predecessor_nonterminal_input_set_token",
            self.predecessor_nonterminal_input_set_token.as_bytes(),
        );
        recovery_hash_part(
            &mut hasher,
            "input_update_count",
            &(self.input_updates.len() as u64).to_be_bytes(),
        );
        for input_update in &self.input_updates {
            recovery_hash_part(
                &mut hasher,
                "input_update_id",
                input_update.input_id.to_string().as_bytes(),
            );
            recovery_hash_part(
                &mut hasher,
                "input_update_expected_row_digest",
                input_update.expected_row_digest.as_bytes(),
            );
            recovery_hash_part(
                &mut hasher,
                "input_update_target",
                &input_update.target_bytes,
            );
        }
        recovery_hash_part(&mut hasher, "receipt", &receipt_json);
        recovery_hash_part(
            &mut hasher,
            "lifecycle_target_token",
            self.lifecycle_target_token.as_bytes(),
        );
        recovery_hash_part(
            &mut hasher,
            "lifecycle_target",
            &self.lifecycle_target_bytes,
        );
        Ok(format!("sha256:{:x}", hasher.finalize()))
    }

    pub(crate) fn verify_head_canonical_boundary(
        &self,
        document: &BoundSessionCommit,
        receipt: &RunBoundaryReceipt,
    ) -> Result<(), RuntimeStoreError> {
        let conflict = |detail: String| RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: self.session_id.to_string(),
            detail,
        };
        let boundary = document.head_canonical().ok_or_else(|| {
            conflict("prepared recovery has no sealed head-canonical mutation".to_string())
        })?;
        let PreparedRecoverySessionAuthority::HeadCanonical {
            physical_head_token,
            recovered_head_token,
            ..
        } = &self.session_authority
        else {
            return Err(conflict(
                "WholeBlob recovery evidence cannot authorize a HeadCanonical mutation".to_string(),
            ));
        };
        let mutation = boundary.mutation();
        if mutation.session_id() != &self.session_id
            || mutation.predecessor_head_token() != Some(physical_head_token.as_str())
        {
            return Err(conflict(
                "prepared recovery head mutation differs from sealed source/successor authority"
                    .to_string(),
            ));
        }
        let successor = mutation.successor_head();
        let successor_token = meerkat_core::session_head_cas_token(successor).map_err(|error| {
            conflict(format!(
                "prepared recovery successor token is invalid: {error}"
            ))
        })?;
        let receipt_count = u64::try_from(receipt.message_count).map_err(|_| {
            conflict("recovery receipt message count does not fit head authority".to_string())
        })?;
        if successor_token != *recovered_head_token
            || successor.message_count != receipt_count
            || receipt.conversation_digest.as_deref() != Some(successor.head_revision.as_str())
        {
            return Err(conflict(
                "prepared recovery head does not bind the receipt's exact transcript".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn verify_input_updates(
        &self,
        input_updates: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        let conflict = |detail: String| RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: self.session_id.to_string(),
            detail,
        };
        let input_updates = input_updates
            .iter()
            .cloned()
            .map(PreparedRecoveryInputUpdate::seal)
            .collect::<Result<Vec<_>, _>>()
            .map_err(&conflict)?;
        validate_recovery_input_update_order(&input_updates).map_err(&conflict)?;
        if input_updates != self.input_updates {
            return Err(conflict(
                "recovery input effects differ from sealed evidence".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn verify_request_effects(
        &self,
        receipt: &RunBoundaryReceipt,
        lifecycle: &MachineLifecycleCommit,
    ) -> Result<(), RuntimeStoreError> {
        let conflict = |detail: String| RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: self.session_id.to_string(),
            detail,
        };
        // The predecessor version remains a first-apply fence. The target
        // bytes, not that transient predecessor token, define retry identity.
        let _ = lifecycle_expected_version_token(lifecycle)?;
        let lifecycle_target_bytes = lifecycle.store_record().encode()?;
        let lifecycle_target_token = recovery_sha256_token(&lifecycle_target_bytes);
        if lifecycle_target_token != self.lifecycle_target_token
            || lifecycle_target_bytes != self.lifecycle_target_bytes
        {
            return Err(conflict(
                "recovery lifecycle target differs from sealed evidence".to_string(),
            ));
        }
        let exact_witness = self.compute_exact_witness(receipt).map_err(|error| {
            RuntimeStoreError::WriteFailed(format!(
                "failed to re-encode exact recovery witness: {error}"
            ))
        })?;
        if exact_witness != self.exact_witness {
            return Err(conflict(
                "recovery receipt or sealed effects differ from exact evidence".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn cloned_input_updates(&self) -> Vec<InputStatePersistenceRecord> {
        self.input_updates
            .iter()
            .map(|input_update| input_update.record.clone())
            .collect()
    }

    pub(crate) fn session_id(&self) -> &meerkat_core::types::SessionId {
        &self.session_id
    }

    pub(crate) fn candidate_id(&self) -> &str {
        &self.candidate_id
    }

    pub(crate) fn candidate_run_id(&self) -> &RunId {
        &self.candidate_run_id
    }

    pub(crate) fn disposition(
        &self,
    ) -> crate::meerkat_machine::dsl::DurableTailRecoveryDisposition {
        self.disposition
    }

    pub(crate) fn head_canonical_authority_transition(
        &self,
    ) -> Option<(u64, &str, u64, &str, &str)> {
        match &self.session_authority {
            PreparedRecoverySessionAuthority::HeadCanonical {
                committed_store_revision,
                committed_head_token,
                physical_store_revision,
                physical_head_token,
                recovered_head_token,
            } => Some((
                *committed_store_revision,
                committed_head_token,
                *physical_store_revision,
                physical_head_token,
                recovered_head_token,
            )),
            PreparedRecoverySessionAuthority::WholeBlob { .. } => None,
        }
    }

    pub(crate) fn whole_blob_authority_transition(&self) -> Option<(u64, &str, &str, u64, &str)> {
        match &self.session_authority {
            PreparedRecoverySessionAuthority::WholeBlob {
                base_store_revision,
                base_blob_sha256,
                provisional_candidate_blob_sha256,
                provisional_candidate_sequence,
                recovered_blob_sha256,
            } => Some((
                *base_store_revision,
                base_blob_sha256,
                provisional_candidate_blob_sha256,
                *provisional_candidate_sequence,
                recovered_blob_sha256,
            )),
            PreparedRecoverySessionAuthority::HeadCanonical { .. } => None,
        }
    }

    pub(crate) fn receipt_digest_enrichments(&self) -> &[PreparedRecoveryReceiptDigestEnrichment] {
        &self.receipt_digest_enrichments
    }

    pub(crate) fn predecessor_nonterminal_input_set_token(&self) -> &str {
        &self.predecessor_nonterminal_input_set_token
    }

    pub(crate) fn predecessor_nonterminal_input_set_revision(&self) -> RecoveryInputSetRevision {
        self.predecessor_nonterminal_input_set_revision
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CommittedRecoveryReceiptDigestEnrichmentWire {
    original_receipt: RunBoundaryReceipt,
    original_exact_row_token: String,
    derived_conversation_digest: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CommittedRecoveryInputUpdateWire {
    input_id: InputId,
    expected_row_digest: String,
    target_bytes: Vec<u8>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "profile", rename_all = "snake_case", deny_unknown_fields)]
enum CommittedRecoverySessionAuthorityWire {
    WholeBlobV1 {
        base_store_revision: u64,
        base_blob_sha256: String,
        provisional_candidate_blob_sha256: String,
        provisional_candidate_sequence: u64,
        recovered_blob_sha256: String,
    },
    HeadCanonicalV1 {
        committed_store_revision: u64,
        committed_head_token: String,
        physical_store_revision: u64,
        physical_head_token: String,
        recovered_head_token: String,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CommittedRecoveryBoundaryWire {
    version: u16,
    session_id: meerkat_core::types::SessionId,
    candidate_id: String,
    candidate_run_id: RunId,
    class: String,
    disposition: String,
    session_authority: CommittedRecoverySessionAuthorityWire,
    receipt_digest_enrichments: Vec<CommittedRecoveryReceiptDigestEnrichmentWire>,
    predecessor_nonterminal_input_set_revision: u64,
    predecessor_nonterminal_input_set_token: String,
    input_updates: Vec<CommittedRecoveryInputUpdateWire>,
    lifecycle_target_token: String,
    lifecycle_target_bytes: Vec<u8>,
    exact_witness: String,
    receipt: RunBoundaryReceipt,
}

/// Durable exact-retry witness for one recovery candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedRecoveryBoundary {
    evidence: PreparedRecoveryEvidence,
    receipt: RunBoundaryReceipt,
}

impl CommittedRecoveryBoundary {
    const VERSION: u16 = 6;

    pub(crate) fn from_prepared(
        evidence: &PreparedRecoveryEvidence,
        receipt: &RunBoundaryReceipt,
    ) -> Self {
        Self {
            evidence: evidence.clone(),
            receipt: receipt.clone(),
        }
    }

    pub(crate) fn evidence(&self) -> &PreparedRecoveryEvidence {
        &self.evidence
    }

    pub(crate) fn receipt(&self) -> &RunBoundaryReceipt {
        &self.receipt
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, RuntimeStoreError> {
        serde_json::to_vec(&CommittedRecoveryBoundaryWire {
            version: Self::VERSION,
            session_id: self.evidence.session_id.clone(),
            candidate_id: self.evidence.candidate_id.clone(),
            candidate_run_id: self.evidence.candidate_run_id.clone(),
            class: recovery_class_name(self.evidence.class).to_string(),
            disposition: recovery_disposition_name(self.evidence.disposition).to_string(),
            session_authority: match &self.evidence.session_authority {
                PreparedRecoverySessionAuthority::WholeBlob {
                    base_store_revision,
                    base_blob_sha256,
                    provisional_candidate_blob_sha256,
                    provisional_candidate_sequence,
                    recovered_blob_sha256,
                } => CommittedRecoverySessionAuthorityWire::WholeBlobV1 {
                    base_store_revision: *base_store_revision,
                    base_blob_sha256: base_blob_sha256.clone(),
                    provisional_candidate_blob_sha256: provisional_candidate_blob_sha256.clone(),
                    provisional_candidate_sequence: *provisional_candidate_sequence,
                    recovered_blob_sha256: recovered_blob_sha256.clone(),
                },
                PreparedRecoverySessionAuthority::HeadCanonical {
                    committed_store_revision,
                    committed_head_token,
                    physical_store_revision,
                    physical_head_token,
                    recovered_head_token,
                } => CommittedRecoverySessionAuthorityWire::HeadCanonicalV1 {
                    committed_store_revision: *committed_store_revision,
                    committed_head_token: committed_head_token.clone(),
                    physical_store_revision: *physical_store_revision,
                    physical_head_token: physical_head_token.clone(),
                    recovered_head_token: recovered_head_token.clone(),
                },
            },
            receipt_digest_enrichments: self
                .evidence
                .receipt_digest_enrichments
                .iter()
                .map(|enrichment| CommittedRecoveryReceiptDigestEnrichmentWire {
                    original_receipt: enrichment.original_receipt.clone(),
                    original_exact_row_token: enrichment.original_exact_row_token.clone(),
                    derived_conversation_digest: enrichment.derived_conversation_digest.clone(),
                })
                .collect(),
            predecessor_nonterminal_input_set_revision: self
                .evidence
                .predecessor_nonterminal_input_set_revision
                .store_generation(),
            predecessor_nonterminal_input_set_token: self
                .evidence
                .predecessor_nonterminal_input_set_token
                .clone(),
            input_updates: self
                .evidence
                .input_updates
                .iter()
                .map(|input_update| CommittedRecoveryInputUpdateWire {
                    input_id: input_update.input_id.clone(),
                    expected_row_digest: input_update.expected_row_digest.clone(),
                    target_bytes: input_update.target_bytes.clone(),
                })
                .collect(),
            lifecycle_target_token: self.evidence.lifecycle_target_token.clone(),
            lifecycle_target_bytes: self.evidence.lifecycle_target_bytes.clone(),
            exact_witness: self.evidence.exact_witness.clone(),
            receipt: self.receipt.clone(),
        })
        .map_err(|error| {
            RuntimeStoreError::WriteFailed(format!(
                "failed to encode committed recovery boundary: {error}"
            ))
        })
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, RuntimeStoreError> {
        let wire: CommittedRecoveryBoundaryWire =
            serde_json::from_slice(bytes).map_err(|error| {
                RuntimeStoreError::ReadFailed(format!(
                    "invalid committed recovery boundary: {error}"
                ))
            })?;
        if wire.version != Self::VERSION {
            return Err(RuntimeStoreError::ReadFailed(format!(
                "unsupported committed recovery boundary version {}",
                wire.version
            )));
        }
        let CommittedRecoveryBoundaryWire {
            version: _,
            session_id,
            candidate_id,
            candidate_run_id,
            class,
            disposition,
            session_authority: session_authority_wire,
            receipt_digest_enrichments: receipt_digest_enrichment_wires,
            predecessor_nonterminal_input_set_revision,
            predecessor_nonterminal_input_set_token,
            input_updates: input_update_wires,
            lifecycle_target_token,
            lifecycle_target_bytes,
            exact_witness,
            receipt,
        } = wire;
        let session_authority = match session_authority_wire {
            CommittedRecoverySessionAuthorityWire::WholeBlobV1 {
                base_store_revision,
                base_blob_sha256,
                provisional_candidate_blob_sha256,
                provisional_candidate_sequence,
                recovered_blob_sha256,
            } if base_store_revision != 0
                && !base_blob_sha256.is_empty()
                && !provisional_candidate_blob_sha256.is_empty()
                && provisional_candidate_sequence != 0
                && !recovered_blob_sha256.is_empty() =>
            {
                PreparedRecoverySessionAuthority::WholeBlob {
                    base_store_revision,
                    base_blob_sha256,
                    provisional_candidate_blob_sha256,
                    provisional_candidate_sequence,
                    recovered_blob_sha256,
                }
            }
            CommittedRecoverySessionAuthorityWire::HeadCanonicalV1 {
                committed_store_revision,
                committed_head_token,
                physical_store_revision,
                physical_head_token,
                recovered_head_token,
            } if committed_store_revision != 0
                && physical_store_revision > committed_store_revision
                && !committed_head_token.is_empty()
                && !physical_head_token.is_empty()
                && !recovered_head_token.is_empty()
                && committed_head_token != physical_head_token =>
            {
                PreparedRecoverySessionAuthority::HeadCanonical {
                    committed_store_revision,
                    committed_head_token,
                    physical_store_revision,
                    physical_head_token,
                    recovered_head_token,
                }
            }
            _ => {
                return Err(RuntimeStoreError::ReadFailed(
                    "committed recovery boundary contains an invalid store authority transition"
                        .to_string(),
                ));
            }
        };
        if candidate_id.is_empty()
            || !is_canonical_sha256_token(&predecessor_nonterminal_input_set_token)
            || lifecycle_target_bytes.is_empty()
            || !is_canonical_sha256_token(&lifecycle_target_token)
            || !is_canonical_sha256_token(&exact_witness)
        {
            return Err(RuntimeStoreError::ReadFailed(
                "committed recovery boundary contains an empty exact identity".to_string(),
            ));
        }
        if recovery_sha256_token(&lifecycle_target_bytes) != lifecycle_target_token {
            return Err(RuntimeStoreError::ReadFailed(
                "committed recovery lifecycle target token does not match its exact bytes"
                    .to_string(),
            ));
        }
        let lifecycle_target_snapshot =
            decode_machine_lifecycle_store_record(&lifecycle_target_bytes).map_err(|error| {
                RuntimeStoreError::ReadFailed(format!(
                    "committed recovery lifecycle target is invalid: {error}"
                ))
            })?;
        let canonical_lifecycle_target_bytes =
            MachineLifecycleStoreRecord::from_snapshot(&lifecycle_target_snapshot)
                .encode()
                .map_err(|error| {
                    RuntimeStoreError::ReadFailed(format!(
                        "failed to canonicalize committed recovery lifecycle target: {error}"
                    ))
                })?;
        if canonical_lifecycle_target_bytes != lifecycle_target_bytes {
            return Err(RuntimeStoreError::ReadFailed(
                "committed recovery lifecycle target is not in canonical serialized form"
                    .to_string(),
            ));
        }
        if receipt.run_id != candidate_run_id {
            return Err(RuntimeStoreError::ReadFailed(
                "committed recovery receipt run differs from candidate run".to_string(),
            ));
        }
        let mut previous_enrichment_sequence = None;
        let mut receipt_digest_enrichments =
            Vec::with_capacity(receipt_digest_enrichment_wires.len());
        for enrichment in receipt_digest_enrichment_wires {
            if enrichment.original_receipt.run_id != candidate_run_id
                || enrichment.original_receipt.conversation_digest.is_some()
                || previous_enrichment_sequence
                    .is_some_and(|previous| enrichment.original_receipt.sequence <= previous)
                || enrichment.original_exact_row_token.is_empty()
                || enrichment.derived_conversation_digest.is_empty()
            {
                return Err(RuntimeStoreError::ReadFailed(
                    "committed recovery receipt enrichment is malformed".to_string(),
                ));
            }
            previous_enrichment_sequence = Some(enrichment.original_receipt.sequence);
            receipt_digest_enrichments.push(PreparedRecoveryReceiptDigestEnrichment {
                original_receipt: enrichment.original_receipt,
                original_exact_row_token: enrichment.original_exact_row_token,
                derived_conversation_digest: enrichment.derived_conversation_digest,
            });
        }
        let input_updates = input_update_wires
            .into_iter()
            .map(|input_update| {
                PreparedRecoveryInputUpdate::decode(
                    input_update.input_id,
                    input_update.expected_row_digest,
                    input_update.target_bytes,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|detail| {
                RuntimeStoreError::ReadFailed(format!(
                    "committed recovery input update is malformed: {detail}"
                ))
            })?;
        validate_recovery_input_update_order(&input_updates).map_err(|detail| {
            RuntimeStoreError::ReadFailed(format!(
                "committed recovery input update order is malformed: {detail}"
            ))
        })?;
        let class = recovery_class_from_name(&class)?;
        let disposition = recovery_disposition_from_name(&disposition)?;
        let mut evidence = PreparedRecoveryEvidence {
            session_id,
            candidate_id,
            candidate_run_id,
            class,
            disposition,
            session_authority,
            receipt_digest_enrichments,
            predecessor_nonterminal_input_set_revision:
                RecoveryInputSetRevision::from_store_generation(
                    predecessor_nonterminal_input_set_revision,
                ),
            predecessor_nonterminal_input_set_token,
            input_updates,
            lifecycle_target_token,
            lifecycle_target_bytes,
            exact_witness: String::new(),
        };
        let derived_exact_witness = evidence.compute_exact_witness(&receipt).map_err(|error| {
            RuntimeStoreError::ReadFailed(format!(
                "failed to verify committed recovery exact witness: {error}"
            ))
        })?;
        if derived_exact_witness != exact_witness {
            return Err(RuntimeStoreError::ReadFailed(
                "committed recovery exact witness does not match its serialized effects"
                    .to_string(),
            ));
        }
        evidence.exact_witness = exact_witness;
        Ok(Self { evidence, receipt })
    }
}

/// Kind of atomic boundary carried by [`PreparedRuntimeSessionCommit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparedRuntimeSessionCommitKind {
    /// Session-control snapshot without a run receipt.
    SnapshotOnly,
    /// Successfully applied run boundary.
    Success,
    /// Direct service-turn terminal with machine lifecycle authority.
    ServiceTurnTerminal,
    /// Failed-but-applied run boundary with machine lifecycle authority.
    MachineTerminal,
    /// Machine-authorized durable-tail recovery with exact physical-head CAS.
    Recovery,
}

#[derive(Debug, Clone)]
pub(crate) enum PreparedRuntimeSessionCommitPayload {
    SnapshotOnly {
        session: BoundSessionCommit,
    },
    Success {
        session: Option<BoundSessionCommit>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    },
    PromoteWholeBlobSuccess {
        promotion: PreparedWholeBlobProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    },
    PromoteHeadCanonicalSuccess {
        promotion: PreparedHeadCanonicalProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    },
    ServiceTurnTerminal {
        session: BoundSessionCommit,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        session_store_key: meerkat_core::types::SessionId,
    },
    PromoteWholeBlobServiceTurnTerminal {
        promotion: PreparedWholeBlobProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        session_store_key: meerkat_core::types::SessionId,
    },
    PromoteHeadCanonicalServiceTurnTerminal {
        promotion: PreparedHeadCanonicalProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        session_store_key: meerkat_core::types::SessionId,
    },
    MachineTerminal {
        session: BoundSessionCommit,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    },
    PromoteWholeBlobMachineTerminal {
        promotion: PreparedWholeBlobProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    },
    PromoteHeadCanonicalMachineTerminal {
        promotion: PreparedHeadCanonicalProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    },
    Recovery {
        session: BoundSessionCommit,
        evidence: PreparedRecoveryEvidence,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    },
    PromoteWholeBlobRecovery {
        promotion: PreparedWholeBlobRecoveryPromotion,
        evidence: PreparedRecoveryEvidence,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    },
}

/// Opaque, valid-by-construction request for one runtime-owned session
/// boundary.
///
/// The constructors prevent receipt, lifecycle, and session-key values from
/// being combined into a boundary shape that no
/// [`RuntimeStore`] verb can commit. The sealed session remains typed and lazy
/// until the selected persistence profile consumes it.
#[derive(Debug, Clone)]
pub struct PreparedRuntimeSessionCommit {
    payload: PreparedRuntimeSessionCommitPayload,
}

impl PreparedRuntimeSessionCommit {
    fn validate_whole_blob_promotion_binding(
        promotion: &PreparedWholeBlobProvisionalPromotion,
        receipt: &RunBoundaryReceipt,
        session_store_key: &meerkat_core::types::SessionId,
    ) -> Result<(), RuntimeStoreError> {
        if promotion.authority().run_id() != &receipt.run_id {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: promotion.authority().session_id().to_string(),
                detail: "WholeBlob promotion receipt run differs from provisional authority"
                    .to_string(),
            });
        }
        if receipt.conversation_digest.as_deref() != Some(promotion.conversation_digest.as_str())
            || u64::try_from(receipt.message_count).ok() != Some(promotion.message_count)
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: promotion.authority().session_id().to_string(),
                detail: "WholeBlob final receipt differs from checkpoint candidate count/digest"
                    .to_string(),
            });
        }
        if promotion.authority().session_id() != session_store_key {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: promotion.authority().session_id().clone(),
                actual: session_store_key.clone(),
            });
        }
        Ok(())
    }

    fn validate_head_canonical_promotion_binding(
        promotion: &PreparedHeadCanonicalProvisionalPromotion,
        receipt: &RunBoundaryReceipt,
        session_store_key: &meerkat_core::types::SessionId,
    ) -> Result<(), RuntimeStoreError> {
        if promotion.authority().run_id() != &receipt.run_id {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: promotion.authority().session_id().to_string(),
                detail: "HeadCanonical promotion receipt run differs from provisional authority"
                    .to_string(),
            });
        }
        if promotion.authority().session_id() != session_store_key {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: promotion.authority().session_id().clone(),
                actual: session_store_key.clone(),
            });
        }
        if receipt.conversation_digest.as_deref()
            != Some(promotion.checkpoint().conversation_digest())
            || u64::try_from(receipt.message_count).ok()
                != Some(promotion.checkpoint().message_count())
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: promotion.authority().session_id().to_string(),
                detail:
                    "HeadCanonical promotion terminal receipt differs from checkpoint digest/count"
                        .to_string(),
            });
        }
        Ok(())
    }

    /// Prepare a session-control snapshot without a run receipt.
    #[must_use]
    pub fn snapshot_only(session: BoundSessionCommit) -> Self {
        Self {
            payload: PreparedRuntimeSessionCommitPayload::SnapshotOnly { session },
        }
    }

    /// Prepare a successful run boundary.
    #[must_use]
    pub fn success(
        session: Option<BoundSessionCommit>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Self {
        Self {
            payload: PreparedRuntimeSessionCommitPayload::Success {
                session,
                receipt,
                input_updates,
                session_store_key,
            },
        }
    }

    /// Prepare a successful final boundary that promotes an already-written
    /// WholeBlob candidate without carrying or materializing its Session body.
    pub fn promote_whole_blob_success(
        promotion: PreparedWholeBlobProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<Self, RuntimeStoreError> {
        Self::validate_whole_blob_promotion_binding(&promotion, &receipt, &session_store_key)?;
        Ok(Self {
            payload: PreparedRuntimeSessionCommitPayload::PromoteWholeBlobSuccess {
                promotion,
                receipt,
                input_updates,
                session_store_key,
            },
        })
    }

    /// Prepare a successful final boundary that promotes an already-applied
    /// HeadCanonical physical checkpoint without reapplying its rows.
    pub fn promote_head_canonical_success(
        promotion: PreparedHeadCanonicalProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<Self, RuntimeStoreError> {
        Self::validate_head_canonical_promotion_binding(&promotion, &receipt, &session_store_key)?;
        Ok(Self {
            payload: PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalSuccess {
                promotion,
                receipt,
                input_updates,
                session_store_key,
            },
        })
    }

    /// Prepare a failed-but-applied run boundary.
    #[must_use]
    pub fn machine_terminal(
        session: BoundSessionCommit,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Self {
        Self {
            payload: PreparedRuntimeSessionCommitPayload::MachineTerminal {
                session,
                receipt,
                machine_lifecycle,
                input_updates,
                session_store_key,
            },
        }
    }

    /// Prepare a failed-but-applied final boundary that promotes the exact
    /// store-owned WholeBlob candidate.
    pub fn promote_whole_blob_machine_terminal(
        promotion: PreparedWholeBlobProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<Self, RuntimeStoreError> {
        Self::validate_whole_blob_promotion_binding(&promotion, &receipt, &session_store_key)?;
        Ok(Self {
            payload: PreparedRuntimeSessionCommitPayload::PromoteWholeBlobMachineTerminal {
                promotion,
                receipt,
                machine_lifecycle,
                input_updates,
                session_store_key,
            },
        })
    }

    /// Prepare a failed-but-applied boundary that promotes the exact applied
    /// HeadCanonical provisional checkpoint.
    pub fn promote_head_canonical_machine_terminal(
        promotion: PreparedHeadCanonicalProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<Self, RuntimeStoreError> {
        Self::validate_head_canonical_promotion_binding(&promotion, &receipt, &session_store_key)?;
        Ok(Self {
            payload: PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalMachineTerminal {
                promotion,
                receipt,
                machine_lifecycle,
                input_updates,
                session_store_key,
            },
        })
    }

    /// Prepare a direct service-turn terminal boundary.
    #[must_use]
    pub fn service_turn_terminal(
        session: BoundSessionCommit,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Self {
        Self {
            payload: PreparedRuntimeSessionCommitPayload::ServiceTurnTerminal {
                session,
                receipt,
                machine_lifecycle,
                session_store_key,
            },
        }
    }

    /// Prepare a direct service-turn terminal boundary that promotes the exact
    /// store-owned WholeBlob candidate.
    pub fn promote_whole_blob_service_turn_terminal(
        promotion: PreparedWholeBlobProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<Self, RuntimeStoreError> {
        Self::validate_whole_blob_promotion_binding(&promotion, &receipt, &session_store_key)?;
        Ok(Self {
            payload: PreparedRuntimeSessionCommitPayload::PromoteWholeBlobServiceTurnTerminal {
                promotion,
                receipt,
                machine_lifecycle,
                session_store_key,
            },
        })
    }

    /// Prepare a service-turn terminal boundary that promotes the exact
    /// applied HeadCanonical provisional checkpoint.
    pub fn promote_head_canonical_service_turn_terminal(
        promotion: PreparedHeadCanonicalProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<Self, RuntimeStoreError> {
        Self::validate_head_canonical_promotion_binding(&promotion, &receipt, &session_store_key)?;
        Ok(Self {
            payload: PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalServiceTurnTerminal {
                promotion,
                receipt,
                machine_lifecycle,
                session_store_key,
            },
        })
    }

    /// Prepare the only boundary allowed to realize machine-authorized
    /// durable-tail recovery.
    ///
    /// Crate-only because the recovery classifier and generated machine must
    /// seal [`PreparedRecoveryEvidence`]; a caller cannot select a recovery
    /// disposition or physical-head proof.
    pub(crate) fn machine_terminal_recovery(
        session: BoundSessionCommit,
        evidence: PreparedRecoveryEvidence,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<Self, RuntimeStoreError> {
        if &session_store_key != evidence.session_id() {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: evidence.session_id().clone(),
                actual: session_store_key,
            });
        }
        if &receipt.run_id != evidence.candidate_run_id() {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: evidence.session_id().to_string(),
                detail: "recovery receipt run differs from sealed candidate run".to_string(),
            });
        }
        evidence.verify_head_canonical_boundary(&session, &receipt)?;
        evidence.verify_request_effects(&receipt, &machine_lifecycle)?;
        let input_updates = evidence.cloned_input_updates();
        Ok(Self {
            payload: PreparedRuntimeSessionCommitPayload::Recovery {
                session,
                evidence,
                receipt,
                machine_lifecycle,
                input_updates,
                session_store_key,
            },
        })
    }

    /// Prepare a WholeBlob recovery without routing the recovered document
    /// through the ordinary whole-document boundary.
    ///
    /// Completed candidates become metadata-only promotions. Interrupted
    /// candidates retain the one already-materialized repaired artifact.
    pub(crate) fn machine_terminal_whole_blob_recovery(
        repaired_document: Option<BoundSessionCommit>,
        evidence: PreparedRecoveryEvidence,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<Self, RuntimeStoreError> {
        if &session_store_key != evidence.session_id() {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: evidence.session_id().clone(),
                actual: session_store_key,
            });
        }
        if &receipt.run_id != evidence.candidate_run_id() {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: evidence.session_id().to_string(),
                detail: "recovery receipt run differs from sealed candidate run".to_string(),
            });
        }
        evidence.verify_request_effects(&receipt, &machine_lifecycle)?;
        let input_updates = evidence.cloned_input_updates();
        evidence.verify_input_updates(&input_updates)?;
        let promotion =
            PreparedWholeBlobRecoveryPromotion::prepare(repaired_document.as_ref(), &evidence)?;
        Ok(Self {
            payload: PreparedRuntimeSessionCommitPayload::PromoteWholeBlobRecovery {
                promotion,
                evidence,
                receipt,
                machine_lifecycle,
                input_updates,
                session_store_key,
            },
        })
    }

    /// Boundary shape selected by the constructor.
    #[must_use]
    pub fn kind(&self) -> PreparedRuntimeSessionCommitKind {
        match &self.payload {
            PreparedRuntimeSessionCommitPayload::SnapshotOnly { .. } => {
                PreparedRuntimeSessionCommitKind::SnapshotOnly
            }
            PreparedRuntimeSessionCommitPayload::Success { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobSuccess { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalSuccess { .. } => {
                PreparedRuntimeSessionCommitKind::Success
            }
            PreparedRuntimeSessionCommitPayload::ServiceTurnTerminal { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobServiceTurnTerminal { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalServiceTurnTerminal {
                ..
            } => PreparedRuntimeSessionCommitKind::ServiceTurnTerminal,
            PreparedRuntimeSessionCommitPayload::MachineTerminal { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobMachineTerminal { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalMachineTerminal { .. } => {
                PreparedRuntimeSessionCommitKind::MachineTerminal
            }
            PreparedRuntimeSessionCommitPayload::Recovery { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobRecovery { .. } => {
                PreparedRuntimeSessionCommitKind::Recovery
            }
        }
    }

    /// Prepared session document, when this boundary carries one.
    #[must_use]
    pub fn session(&self) -> Option<&BoundSessionCommit> {
        match &self.payload {
            PreparedRuntimeSessionCommitPayload::SnapshotOnly { session, .. }
            | PreparedRuntimeSessionCommitPayload::ServiceTurnTerminal { session, .. }
            | PreparedRuntimeSessionCommitPayload::MachineTerminal { session, .. }
            | PreparedRuntimeSessionCommitPayload::Recovery { session, .. } => Some(session),
            PreparedRuntimeSessionCommitPayload::Success { session, .. } => session.as_ref(),
            PreparedRuntimeSessionCommitPayload::PromoteWholeBlobSuccess { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobServiceTurnTerminal { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobMachineTerminal { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalSuccess { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalServiceTurnTerminal {
                ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalMachineTerminal { .. } => {
                None
            }
            PreparedRuntimeSessionCommitPayload::PromoteWholeBlobRecovery { .. } => None,
        }
    }

    /// Boundary receipt, absent only for snapshot-only commits.
    #[must_use]
    pub fn receipt(&self) -> Option<&RunBoundaryReceipt> {
        match &self.payload {
            PreparedRuntimeSessionCommitPayload::SnapshotOnly { .. } => None,
            PreparedRuntimeSessionCommitPayload::Success { receipt, .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobSuccess { receipt, .. }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalSuccess {
                receipt, ..
            }
            | PreparedRuntimeSessionCommitPayload::ServiceTurnTerminal { receipt, .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobServiceTurnTerminal {
                receipt,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalServiceTurnTerminal {
                receipt,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::MachineTerminal { receipt, .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobMachineTerminal {
                receipt,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalMachineTerminal {
                receipt,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::Recovery { receipt, .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobRecovery { receipt, .. } => {
                Some(receipt)
            }
        }
    }

    /// Input-state mutations committed with a run boundary.
    #[must_use]
    pub fn input_updates(&self) -> Option<&[InputStatePersistenceRecord]> {
        match &self.payload {
            PreparedRuntimeSessionCommitPayload::SnapshotOnly { .. } => None,
            PreparedRuntimeSessionCommitPayload::ServiceTurnTerminal { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobServiceTurnTerminal { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalServiceTurnTerminal {
                ..
            } => Some(&[]),
            PreparedRuntimeSessionCommitPayload::Success { input_updates, .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobSuccess {
                input_updates, ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalSuccess {
                input_updates,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::MachineTerminal { input_updates, .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobMachineTerminal {
                input_updates,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalMachineTerminal {
                input_updates,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::Recovery { input_updates, .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobRecovery {
                input_updates, ..
            } => Some(input_updates),
        }
    }

    /// Explicit SessionStore identity carried by a run boundary.
    #[must_use]
    pub fn session_store_key(&self) -> Option<&meerkat_core::types::SessionId> {
        match &self.payload {
            PreparedRuntimeSessionCommitPayload::SnapshotOnly { .. } => None,
            PreparedRuntimeSessionCommitPayload::Success {
                session_store_key, ..
            } => session_store_key.as_ref(),
            PreparedRuntimeSessionCommitPayload::PromoteWholeBlobSuccess {
                session_store_key,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalSuccess {
                session_store_key,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::ServiceTurnTerminal {
                session_store_key, ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobServiceTurnTerminal {
                session_store_key,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalServiceTurnTerminal {
                session_store_key,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::MachineTerminal {
                session_store_key, ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobMachineTerminal {
                session_store_key,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalMachineTerminal {
                session_store_key,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::Recovery {
                session_store_key, ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobRecovery {
                session_store_key,
                ..
            } => Some(session_store_key),
        }
    }

    /// Machine lifecycle authority, present only for a machine-terminal commit.
    #[must_use]
    pub fn machine_lifecycle(&self) -> Option<&MachineLifecycleCommit> {
        match &self.payload {
            PreparedRuntimeSessionCommitPayload::ServiceTurnTerminal {
                machine_lifecycle, ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobServiceTurnTerminal {
                machine_lifecycle,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalServiceTurnTerminal {
                machine_lifecycle,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::MachineTerminal {
                machine_lifecycle, ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobMachineTerminal {
                machine_lifecycle,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalMachineTerminal {
                machine_lifecycle,
                ..
            }
            | PreparedRuntimeSessionCommitPayload::Recovery {
                machine_lifecycle, ..
            }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobRecovery {
                machine_lifecycle,
                ..
            } => Some(machine_lifecycle),
            PreparedRuntimeSessionCommitPayload::SnapshotOnly { .. }
            | PreparedRuntimeSessionCommitPayload::Success { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobSuccess { .. }
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalSuccess { .. } => None,
        }
    }

    pub(crate) fn into_payload(self) -> PreparedRuntimeSessionCommitPayload {
        self.payload
    }
}

/// Store-internal exact pairing produced only from a sealed typed WholeBlob
/// boundary. Backends consume the typed Session for guards and compaction
/// intents while writing the already-materialized shared bytes and authority.
/// This prevents a prepared boundary from reparsing its own JSON.
#[derive(Debug, Clone)]
pub(crate) struct PreparedWholeBlobSnapshot {
    session: std::sync::Arc<meerkat_core::Session>,
    serialized: SerializedSessionSnapshot,
    blob_sha256: String,
}

impl PreparedWholeBlobSnapshot {
    #[must_use]
    pub(crate) fn session(&self) -> &meerkat_core::Session {
        self.session.as_ref()
    }

    #[must_use]
    pub(crate) fn blob_sha256(&self) -> &str {
        &self.blob_sha256
    }

    #[must_use]
    pub(crate) fn into_parts(
        self,
    ) -> (
        std::sync::Arc<meerkat_core::Session>,
        SerializedSessionSnapshot,
        String,
    ) {
        (self.session, self.serialized, self.blob_sha256)
    }
}

fn prepared_whole_blob_snapshot(
    session: &BoundSessionCommit,
) -> Result<PreparedWholeBlobSnapshot, RuntimeStoreError> {
    let typed_session = session.session_arc_cloned().ok_or_else(|| {
        RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: "<untyped-whole-blob-boundary>".to_string(),
            detail: "prepared WholeBlob boundary requires a sealed typed Session".to_string(),
        }
    })?;
    let artifact = session.whole_blob_artifact().map_err(|error| {
        RuntimeStoreError::WriteFailed(format!(
            "failed to materialize whole-blob session boundary: {error}"
        ))
    })?;
    Ok(PreparedWholeBlobSnapshot {
        session: typed_session,
        serialized: whole_blob_serialized_snapshot(artifact.bytes_arc()),
        blob_sha256: artifact.row_sha256_token().to_string(),
    })
}

fn parsed_whole_blob_snapshot(
    serialized: SerializedSessionSnapshot,
) -> Result<PreparedWholeBlobSnapshot, RuntimeStoreError> {
    let session = std::sync::Arc::new(
        meerkat_core::Session::from_persisted_bytes(serialized.session_snapshot.as_ref()).map_err(
            |error| {
                RuntimeStoreError::WriteFailed(format!(
                    "whole-blob snapshot is not a valid Session payload: {error}"
                ))
            },
        )?,
    );
    let blob_sha256 = format!(
        "row-sha256:{:x}",
        sha2::Sha256::digest(serialized.session_snapshot.as_ref())
    );
    Ok(PreparedWholeBlobSnapshot {
        session,
        serialized,
        blob_sha256,
    })
}

fn whole_blob_serialized_snapshot(
    session_snapshot: std::sync::Arc<Vec<u8>>,
) -> SerializedSessionSnapshot {
    SerializedSessionSnapshot { session_snapshot }
}

/// Opaque generated runtime-delivery authority persisted by a
/// [`RuntimeStore`].
///
/// Stores compare the mechanical revision and retain the bytes exactly. They
/// do not interpret delivery lifecycle, sequence assignment, or cursor
/// semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDeliveryAuthorityRecord {
    revision: u64,
    state_json: Vec<u8>,
}

impl RuntimeDeliveryAuthorityRecord {
    #[doc(hidden)]
    pub fn from_parts(revision: u64, state_json: Vec<u8>) -> Self {
        Self {
            revision,
            state_json,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn state_json(&self) -> &[u8] {
        &self.state_json
    }
}

/// Opaque runtime-inbox row committed alongside generated delivery authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDeliveryStoreRecord {
    delivery_id: String,
    sequence: u64,
    submission_json: Vec<u8>,
}

impl RuntimeDeliveryStoreRecord {
    #[doc(hidden)]
    pub fn from_parts(
        delivery_id: impl Into<String>,
        sequence: u64,
        submission_json: Vec<u8>,
    ) -> Self {
        Self {
            delivery_id: delivery_id.into(),
            sequence,
            submission_json,
        }
    }

    pub fn delivery_id(&self) -> &str {
        &self.delivery_id
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn submission_json(&self) -> &[u8] {
        &self.submission_json
    }
}

/// Mechanical compare-and-swap result for runtime-delivery authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeDeliveryAuthorityCasOutcome {
    Applied(RuntimeDeliveryAuthorityRecord),
    Conflict(Option<RuntimeDeliveryAuthorityRecord>),
}

fn validated_compaction_projection_intents(
    session: &meerkat_core::Session,
) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeStoreError> {
    session
        .validated_compaction_projection_intents()
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))
}

/// Clear one finalized compaction intent from the ordinary session document.
///
/// Physical currentness remains store-owned; mutating this domain payload
/// neither consumes nor mints persistence authority.
pub(crate) fn complete_compaction_projection_intent(
    session: &mut meerkat_core::Session,
    projection: &meerkat_core::CompactionProjectionId,
) -> Result<(), RuntimeStoreError> {
    session
        .complete_compaction_projection_intent(projection)
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    Ok(())
}

/// Runtime binding facts selected by generated MeerkatMachine authority.
///
/// RuntimeStore implementations persist and read these facts as part of a
/// machine lifecycle snapshot. The commit token that writes these facts stays
/// crate-private so compatibility callers cannot mint replacement lifecycle
/// truth.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachineLifecycleBindingFacts {
    agent_runtime_id: Option<String>,
    fence_token: Option<u64>,
    runtime_generation: Option<u64>,
    runtime_epoch_id: Option<String>,
}

/// Durable identity receipt for the last completed supervisor revoke.
///
/// This is not a live supervisor binding and carries no route address. It is
/// only the identity/key/epoch witness needed to authorize an exact duplicate
/// revoke response after a cold restart; the current authenticated request
/// supplies its current route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokedSupervisorReceipt {
    peer_id: String,
    signing_public_key: String,
    epoch: u64,
}

/// Durable current supervisor binding used to authenticate terminal retry
/// traffic after a cold runtime restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorBindingReceipt {
    name: String,
    peer_id: String,
    address: String,
    signing_public_key: String,
    epoch: u64,
}

/// Durable in-flight supervisor revocation receipt.
///
/// This is the closed-world hand-off between generated machine authority and
/// the concrete router mutation.  It deliberately retains the complete prior
/// route so a cold runtime can authenticate an exact retry and re-materialize
/// the generated remove obligation without resurrecting a live binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorRevocationPendingReceipt {
    name: String,
    peer_id: String,
    address: String,
    signing_public_key: String,
    epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorRotationPersistencePhase {
    PreviousRevokePending,
    NextPublishPending,
    Completed,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorRotationRejection {
    OperationConflict,
    NotBound,
    SenderMismatch,
    TargetEpochNotAdvanced,
    InvalidTarget,
    UnsupportedProtocolVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorRotationReceipt {
    operation_id: meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId,
    phase: SupervisorRotationPersistencePhase,
    rejection: Option<SupervisorRotationRejection>,
    previous: SupervisorBindingReceipt,
    next: SupervisorBindingReceipt,
}

impl SupervisorRotationReceipt {
    pub(crate) fn new(
        operation_id: meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId,
        phase: SupervisorRotationPersistencePhase,
        rejection: Option<SupervisorRotationRejection>,
        previous: SupervisorBindingReceipt,
        next: SupervisorBindingReceipt,
    ) -> Self {
        Self {
            operation_id,
            phase,
            rejection,
            previous,
            next,
        }
    }

    pub fn operation_id(
        &self,
    ) -> meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId {
        self.operation_id
    }

    pub fn phase(&self) -> SupervisorRotationPersistencePhase {
        self.phase
    }

    pub fn rejection(&self) -> Option<SupervisorRotationRejection> {
        self.rejection
    }

    pub fn previous(&self) -> &SupervisorBindingReceipt {
        &self.previous
    }

    pub fn next(&self) -> &SupervisorBindingReceipt {
        &self.next
    }
}

impl SupervisorBindingReceipt {
    pub(crate) fn new(
        name: String,
        peer_id: String,
        address: String,
        signing_public_key: String,
        epoch: u64,
    ) -> Self {
        Self {
            name,
            peer_id,
            address,
            signing_public_key,
            epoch,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn signing_public_key(&self) -> &str {
        &self.signing_public_key
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl RevokedSupervisorReceipt {
    pub(crate) fn new(peer_id: String, signing_public_key: String, epoch: u64) -> Self {
        Self {
            peer_id,
            signing_public_key,
            epoch,
        }
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    pub fn signing_public_key(&self) -> &str {
        &self.signing_public_key
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl SupervisorRevocationPendingReceipt {
    pub(crate) fn new(
        name: String,
        peer_id: String,
        address: String,
        signing_public_key: String,
        epoch: u64,
    ) -> Self {
        Self {
            name,
            peer_id,
            address,
            signing_public_key,
            epoch,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn signing_public_key(&self) -> &str {
        &self.signing_public_key
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// Closed durable supervisor authority state. Each variant owns one complete
/// recovery shape; terminal rotation receipts retain their exact operation and
/// participant descriptors for idempotent submission and later observation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SupervisorAuthoritySnapshot {
    #[default]
    UnboundNoReceipt,
    Bound(SupervisorBindingReceipt),
    RevocationPending(SupervisorRevocationPendingReceipt),
    RotationOperation(SupervisorRotationReceipt),
    RevokedReceipt(RevokedSupervisorReceipt),
    WithRotationHistory {
        current: Box<SupervisorAuthoritySnapshot>,
        terminal_receipts: std::collections::BTreeMap<
            meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId,
            SupervisorRotationReceipt,
        >,
    },
}

impl MachineLifecycleBindingFacts {
    pub(crate) fn new(
        agent_runtime_id: Option<String>,
        fence_token: Option<u64>,
        runtime_generation: Option<u64>,
        runtime_epoch_id: Option<String>,
    ) -> Self {
        Self {
            agent_runtime_id,
            fence_token,
            runtime_generation,
            runtime_epoch_id,
        }
    }

    pub fn agent_runtime_id(&self) -> Option<&str> {
        self.agent_runtime_id.as_deref()
    }

    pub fn fence_token(&self) -> Option<u64> {
        self.fence_token
    }

    pub fn runtime_generation(&self) -> Option<u64> {
        self.runtime_generation
    }

    pub fn runtime_epoch_id(&self) -> Option<&str> {
        self.runtime_epoch_id.as_deref()
    }
}

/// Exact content version of one observed machine-lifecycle row.
///
/// The version is the domain-prefixed SHA-256 digest of the raw stored bytes,
/// not a decoded projection. It therefore remains a valid target-local CAS
/// witness for unsupported and malformed rows as well as current records.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MachineLifecycleObservationVersion(String);

impl MachineLifecycleObservationVersion {
    /// Derive the exact target-local compare token for opaque stored bytes.
    ///
    /// Custom [`RuntimeStore`] implementations use the same constructor for
    /// both observations and successful CAS receipts; no decoded lifecycle
    /// shape is allowed to stand in for the physical row version.
    pub fn from_raw_record(bytes: &[u8]) -> Self {
        Self(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Independently observed run-binding atoms.
///
/// A torn row may contain exactly one side of this pair. The store preserves
/// that shape; the generated reconciler, not the decoder, decides whether it
/// can be normalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineLifecyclePreRunPhase {
    Idle,
    Attached,
    Retired,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachineLifecycleRunFacts {
    current_run_id: Option<RunId>,
    pre_run_phase: Option<MachineLifecyclePreRunPhase>,
}

impl MachineLifecycleRunFacts {
    pub(crate) fn new(
        current_run_id: Option<RunId>,
        pre_run_phase: Option<MachineLifecyclePreRunPhase>,
    ) -> Self {
        Self {
            current_run_id,
            pre_run_phase,
        }
    }

    #[must_use]
    pub fn current_run_id(&self) -> Option<&RunId> {
        self.current_run_id.as_ref()
    }

    #[must_use]
    pub fn pre_run_phase(&self) -> Option<MachineLifecyclePreRunPhase> {
        self.pre_run_phase
    }
}

/// Decoded runtime-lifecycle observation.
///
/// The lifecycle phase, four binding atoms, and two run atoms remain
/// independently optional. This type deliberately represents partial tuples
/// such as `current_run_id = Some` with `pre_run_phase = None` instead of
/// rejecting them as an impossible transition shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedMachineLifecycleObservation {
    record_version: u16,
    runtime_state: Option<RuntimeState>,
    binding: MachineLifecycleBindingFacts,
    run: MachineLifecycleRunFacts,
    supervisor_authority: SupervisorAuthoritySnapshot,
    unregister_progress: Option<MachineUnregisterProgressSnapshot>,
    live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage,
}

impl DecodedMachineLifecycleObservation {
    #[must_use]
    pub fn record_version(&self) -> u16 {
        self.record_version
    }

    #[must_use]
    pub fn runtime_state(&self) -> Option<RuntimeState> {
        self.runtime_state
    }

    #[must_use]
    pub fn binding(&self) -> &MachineLifecycleBindingFacts {
        &self.binding
    }

    #[must_use]
    pub fn run(&self) -> &MachineLifecycleRunFacts {
        &self.run
    }

    #[must_use]
    pub fn supervisor_authority(&self) -> &SupervisorAuthoritySnapshot {
        &self.supervisor_authority
    }

    #[must_use]
    pub fn unregister_progress(&self) -> Option<&MachineUnregisterProgressSnapshot> {
        self.unregister_progress.as_ref()
    }

    pub(crate) fn live_bridge_recovery(&self) -> &crate::live_execution::LiveBridgeRecoveryImage {
        &self.live_bridge_recovery
    }
}

/// Lossless classification of one physical machine-lifecycle row.
///
/// Transport failures remain [`RuntimeStoreError`] values. Every successfully
/// read row is classified without collapsing unsupported or corrupt bytes into
/// absence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineLifecycleObservation {
    Missing,
    Decoded {
        record: DecodedMachineLifecycleObservation,
        version: MachineLifecycleObservationVersion,
    },
    Unsupported {
        record_version: u64,
        evidence_digest: String,
        version: MachineLifecycleObservationVersion,
    },
    Malformed {
        record_version: Option<u64>,
        evidence_digest: String,
        version: MachineLifecycleObservationVersion,
        detail: String,
    },
}

impl MachineLifecycleObservation {
    /// Losslessly classify one successfully read physical lifecycle row.
    ///
    /// This is the canonical adapter seam for custom stores. Transport
    /// failure stays an outer [`RuntimeStoreError`]; every byte sequence read
    /// successfully becomes Decoded, Unsupported, or Malformed here.
    #[must_use]
    pub fn from_raw_record(bytes: &[u8]) -> Self {
        classify_machine_lifecycle_record(bytes)
    }

    #[must_use]
    pub fn version(&self) -> Option<&MachineLifecycleObservationVersion> {
        match self {
            Self::Missing => None,
            Self::Decoded { version, .. }
            | Self::Unsupported { version, .. }
            | Self::Malformed { version, .. } => Some(version),
        }
    }

    #[must_use]
    pub fn evidence_digest(&self) -> Option<&str> {
        match self {
            Self::Unsupported {
                evidence_digest, ..
            }
            | Self::Malformed {
                evidence_digest, ..
            } => Some(evidence_digest),
            Self::Missing | Self::Decoded { .. } => None,
        }
    }
}

/// Target-local precondition for lifecycle normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineLifecycleExpectedVersion {
    Missing,
    Version(MachineLifecycleObservationVersion),
}

impl MachineLifecycleObservation {
    /// Exact target-local precondition represented by this observation.
    ///
    /// Missing is a first-class compare value. Every present row, including
    /// unsupported and malformed bytes, is compared by its raw-content
    /// version rather than by a decoded projection.
    #[must_use]
    pub fn expected_version(&self) -> MachineLifecycleExpectedVersion {
        self.version()
            .map_or(MachineLifecycleExpectedVersion::Missing, |version| {
                MachineLifecycleExpectedVersion::Version(version.clone())
            })
    }
}

/// Result of executing one synchronous target write under an external fence.
///
/// The fence is deliberately runtime-generic. A caller may back it with a
/// lease, process-incarnation lock, or another authority source without the
/// runtime store depending on that owner's domain types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStoreWriteFenceOutcome {
    /// The fence was current and invoked the supplied operation exactly once.
    Applied,
    /// Durable authority was superseded. The operation was not invoked.
    Conflict { reason: String },
    /// Authority could not be checked temporarily. The operation was not
    /// invoked and the caller should retry after re-observation.
    Backoff { reason: String },
}

/// Synchronous authority guard for a RuntimeStore target write.
///
/// Implementations MUST retain their authority serialization guard for the
/// full duration of `operation`, invoke it exactly once only when authority is
/// current, and never invoke it for Conflict or Backoff. The operation is
/// synchronous by design so built-in stores can call it inside their own lock
/// or transaction immediately before the target write. Time-bounded authority
/// must be evaluated using the authority store's own clock while that guard is
/// held, never a caller-supplied observation timestamp. Once a successful
/// operation returns, the fence must return Applied without a new fallible
/// boundary. Implementations must not re-enter the same RuntimeStore from this
/// callback.
pub trait RuntimeStoreWriteFence: Send + Sync {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError>;
}

pub(crate) fn execute_runtime_store_write_fence(
    write_fence: &dyn RuntimeStoreWriteFence,
    operation: impl FnOnce() -> Result<(), RuntimeStoreError>,
) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
    let invoked = std::cell::Cell::new(false);
    let operation_result = std::cell::RefCell::new(None);
    let checked_operation = || {
        invoked.set(true);
        let result = operation();
        *operation_result.borrow_mut() = Some(result.clone());
        result
    };
    let outcome = write_fence.execute_if_current(Box::new(checked_operation))?;
    if let Some(Err(error)) = operation_result.borrow_mut().take() {
        return Err(error);
    }
    let shape_is_valid = matches!(
        (&outcome, invoked.get()),
        (RuntimeStoreWriteFenceOutcome::Applied, true)
            | (
                RuntimeStoreWriteFenceOutcome::Conflict { .. }
                    | RuntimeStoreWriteFenceOutcome::Backoff { .. },
                false,
            )
    );
    if !shape_is_valid {
        return Err(RuntimeStoreError::Internal(
            "runtime write fence returned an outcome inconsistent with operation execution"
                .to_string(),
        ));
    }
    Ok(outcome)
}

/// Result of a target-local lifecycle CAS performed under an external fence.
///
/// Applied and AlreadyExact carry the exact decoded row used to construct the
/// fresh process-local registration. Callers never receive or construct a
/// MachineLifecycleCommit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FencedMachineLifecycleCasOutcome {
    Applied {
        record: DecodedMachineLifecycleObservation,
        version: MachineLifecycleObservationVersion,
    },
    AlreadyExact {
        record: DecodedMachineLifecycleObservation,
        version: MachineLifecycleObservationVersion,
    },
    Conflict {
        current: MachineLifecycleObservation,
    },
    FenceConflict {
        reason: String,
    },
    FenceBackoff {
        reason: String,
    },
}

/// Result of a target-local lifecycle compare-and-swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineLifecycleCasOutcome {
    Applied {
        version: MachineLifecycleObservationVersion,
    },
    Conflict {
        current: MachineLifecycleObservation,
    },
}

/// Durable read-back shape for machine-owned lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineLifecycleSnapshot {
    runtime_state: RuntimeState,
    binding: MachineLifecycleBindingFacts,
    run: MachineLifecycleRunFacts,
    supervisor_authority: SupervisorAuthoritySnapshot,
    unregister_progress: Option<MachineUnregisterProgressSnapshot>,
    live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage,
}

/// Durable generated unregister-saga progress needed to resume an interrupted
/// Draining epoch without reconstructing missing producer outcomes in shell
/// code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineUnregisterProgressSnapshot {
    runtime_loop_drain_pending: bool,
    comms_drain_exit_pending: bool,
    completion_waiter_drain_pending: bool,
    runtime_loop_forced_abort: bool,
    comms_drain_forced_abort: bool,
}

impl MachineUnregisterProgressSnapshot {
    pub(crate) fn new(
        runtime_loop_drain_pending: bool,
        comms_drain_exit_pending: bool,
        completion_waiter_drain_pending: bool,
        runtime_loop_forced_abort: bool,
        comms_drain_forced_abort: bool,
    ) -> Self {
        Self {
            runtime_loop_drain_pending,
            comms_drain_exit_pending,
            completion_waiter_drain_pending,
            runtime_loop_forced_abort,
            comms_drain_forced_abort,
        }
    }

    pub(crate) fn runtime_loop_drain_pending(&self) -> bool {
        self.runtime_loop_drain_pending
    }

    pub(crate) fn comms_drain_exit_pending(&self) -> bool {
        self.comms_drain_exit_pending
    }

    pub(crate) fn completion_waiter_drain_pending(&self) -> bool {
        self.completion_waiter_drain_pending
    }

    pub(crate) fn runtime_loop_forced_abort(&self) -> bool {
        self.runtime_loop_forced_abort
    }

    pub(crate) fn comms_drain_forced_abort(&self) -> bool {
        self.comms_drain_forced_abort
    }
}

impl MachineLifecycleSnapshot {
    pub(crate) fn new(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
    ) -> Self {
        Self::new_with_unregister_progress(runtime_state, binding, supervisor_authority, None)
    }

    pub(crate) fn new_with_unregister_progress(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
        unregister_progress: Option<MachineUnregisterProgressSnapshot>,
    ) -> Self {
        Self::new_with_run_and_unregister_progress(
            runtime_state,
            binding,
            MachineLifecycleRunFacts::default(),
            supervisor_authority,
            unregister_progress,
        )
    }

    pub(crate) fn new_with_run_and_unregister_progress(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        run: MachineLifecycleRunFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
        unregister_progress: Option<MachineUnregisterProgressSnapshot>,
    ) -> Self {
        Self::new_with_run_unregister_progress_and_live_bridge(
            runtime_state,
            binding,
            run,
            supervisor_authority,
            unregister_progress,
            crate::live_execution::LiveBridgeRecoveryImage::default(),
        )
    }

    pub(crate) fn new_with_run_unregister_progress_and_live_bridge(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        run: MachineLifecycleRunFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
        unregister_progress: Option<MachineUnregisterProgressSnapshot>,
        live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage,
    ) -> Self {
        Self {
            runtime_state,
            binding,
            run,
            supervisor_authority,
            unregister_progress,
            live_bridge_recovery,
        }
    }

    /// Runtime state selected by the owning MeerkatMachine transition.
    pub fn runtime_state(&self) -> RuntimeState {
        self.runtime_state
    }

    /// Runtime binding facts selected by the owning MeerkatMachine transition.
    pub fn binding(&self) -> &MachineLifecycleBindingFacts {
        &self.binding
    }

    /// Independently persisted run-binding atoms.
    pub fn run(&self) -> &MachineLifecycleRunFacts {
        &self.run
    }

    pub fn supervisor_authority(&self) -> &SupervisorAuthoritySnapshot {
        &self.supervisor_authority
    }

    pub fn unregister_progress(&self) -> Option<&MachineUnregisterProgressSnapshot> {
        self.unregister_progress.as_ref()
    }

    pub(crate) fn live_bridge_recovery(&self) -> &crate::live_execution::LiveBridgeRecoveryImage {
        &self.live_bridge_recovery
    }
}

#[allow(
    clippy::option_option,
    reason = "serde distinguishes missing from explicit null"
)]
fn deserialize_present_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer).map(Some)
}

#[allow(
    clippy::option_option,
    reason = "serde distinguishes missing from explicit null"
)]
fn require_present_nullable<T>(
    value: Option<Option<T>>,
    field: &str,
) -> Result<Option<T>, RuntimeStoreError> {
    value.ok_or_else(|| {
        RuntimeStoreError::ReadFailed(format!(
            "machine lifecycle field {field} is required (explicit null is allowed)"
        ))
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleBindingFactsStoreWire {
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes missing from explicit null"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    agent_runtime_id: Option<Option<String>>,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes missing from explicit null"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    fence_token: Option<Option<u64>>,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes missing from explicit null"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    runtime_generation: Option<Option<u64>>,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes missing from explicit null"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    runtime_epoch_id: Option<Option<String>>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleBindingFactsStoreWireV1 {
    agent_runtime_id: Option<String>,
    fence_token: Option<u64>,
    runtime_generation: Option<u64>,
    runtime_epoch_id: Option<String>,
}

impl From<&MachineLifecycleBindingFacts> for MachineLifecycleBindingFactsStoreWire {
    fn from(binding: &MachineLifecycleBindingFacts) -> Self {
        Self {
            agent_runtime_id: Some(binding.agent_runtime_id().map(ToOwned::to_owned)),
            fence_token: Some(binding.fence_token()),
            runtime_generation: Some(binding.runtime_generation()),
            runtime_epoch_id: Some(binding.runtime_epoch_id().map(ToOwned::to_owned)),
        }
    }
}

impl TryFrom<MachineLifecycleBindingFactsStoreWire> for MachineLifecycleBindingFacts {
    type Error = RuntimeStoreError;

    fn try_from(binding: MachineLifecycleBindingFactsStoreWire) -> Result<Self, Self::Error> {
        Ok(Self::new(
            require_present_nullable(binding.agent_runtime_id, "binding.agent_runtime_id")?,
            require_present_nullable(binding.fence_token, "binding.fence_token")?,
            require_present_nullable(binding.runtime_generation, "binding.runtime_generation")?,
            require_present_nullable(binding.runtime_epoch_id, "binding.runtime_epoch_id")?,
        ))
    }
}

impl From<MachineLifecycleBindingFactsStoreWireV1> for MachineLifecycleBindingFacts {
    fn from(binding: MachineLifecycleBindingFactsStoreWireV1) -> Self {
        Self::new(
            binding.agent_runtime_id,
            binding.fence_token,
            binding.runtime_generation,
            binding.runtime_epoch_id,
        )
    }
}

#[derive(serde::Serialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleSnapshotStoreWire {
    record_version: u16,
    runtime_state: RuntimeState,
    binding: MachineLifecycleBindingFactsStoreWire,
    current_run_id: Option<RunId>,
    pre_run_phase: Option<MachineLifecyclePreRunPhase>,
    supervisor_authority: SupervisorAuthoritySnapshotStoreWire,
    unregister_progress: Option<MachineUnregisterProgressSnapshotStoreWire>,
    live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleObservationStoreWireV5 {
    record_version: u16,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes a missing phase from an explicitly absent observed phase"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    runtime_state: Option<Option<RuntimeState>>,
    binding: MachineLifecycleBindingFactsStoreWire,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes a missing run id from an explicitly absent run id"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    current_run_id: Option<Option<RunId>>,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes a missing pre-run phase from an explicitly absent phase"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    pre_run_phase: Option<Option<MachineLifecyclePreRunPhase>>,
    supervisor_authority: SupervisorAuthoritySnapshotStoreWire,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes a missing v5 field from explicit null progress"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    unregister_progress: Option<Option<MachineUnregisterProgressSnapshotStoreWire>>,
    live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleObservationStoreWireV4 {
    record_version: u16,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes a missing phase from an explicitly absent observed phase"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    runtime_state: Option<Option<RuntimeState>>,
    binding: MachineLifecycleBindingFactsStoreWire,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes a missing run id from an explicitly absent run id"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    current_run_id: Option<Option<RunId>>,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes a missing pre-run phase from an explicitly absent phase"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    pre_run_phase: Option<Option<MachineLifecyclePreRunPhase>>,
    supervisor_authority: SupervisorAuthoritySnapshotStoreWire,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes a missing v4 field from explicit null progress"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    unregister_progress: Option<Option<MachineUnregisterProgressSnapshotStoreWire>>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleSnapshotStoreWireV3 {
    record_version: u16,
    runtime_state: RuntimeState,
    binding: MachineLifecycleBindingFactsStoreWire,
    supervisor_authority: SupervisorAuthoritySnapshotStoreWire,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes a missing v3 field from explicit null progress"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    unregister_progress: Option<Option<MachineUnregisterProgressSnapshotStoreWire>>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleSnapshotStoreWireV2 {
    record_version: u16,
    runtime_state: RuntimeState,
    binding: MachineLifecycleBindingFactsStoreWire,
    supervisor_authority: SupervisorAuthoritySnapshotStoreWire,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineUnregisterProgressSnapshotStoreWire {
    runtime_loop_drain_pending: bool,
    comms_drain_exit_pending: bool,
    completion_waiter_drain_pending: bool,
    runtime_loop_forced_abort: bool,
    comms_drain_forced_abort: bool,
}

impl From<&MachineUnregisterProgressSnapshot> for MachineUnregisterProgressSnapshotStoreWire {
    fn from(snapshot: &MachineUnregisterProgressSnapshot) -> Self {
        Self {
            runtime_loop_drain_pending: snapshot.runtime_loop_drain_pending(),
            comms_drain_exit_pending: snapshot.comms_drain_exit_pending(),
            completion_waiter_drain_pending: snapshot.completion_waiter_drain_pending(),
            runtime_loop_forced_abort: snapshot.runtime_loop_forced_abort(),
            comms_drain_forced_abort: snapshot.comms_drain_forced_abort(),
        }
    }
}

impl From<MachineUnregisterProgressSnapshotStoreWire> for MachineUnregisterProgressSnapshot {
    fn from(snapshot: MachineUnregisterProgressSnapshotStoreWire) -> Self {
        Self::new(
            snapshot.runtime_loop_drain_pending,
            snapshot.comms_drain_exit_pending,
            snapshot.completion_waiter_drain_pending,
            snapshot.runtime_loop_forced_abort,
            snapshot.comms_drain_forced_abort,
        )
    }
}

/// Exact pre-supervisor-authority lifecycle shape. Version 1 is decoded only
/// through this migration carrier so a missing authority on a current record
/// cannot be confused with legacy data.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleSnapshotStoreWireV1 {
    record_version: u16,
    runtime_state: RuntimeState,
    binding: MachineLifecycleBindingFactsStoreWireV1,
}

#[derive(serde::Deserialize)]
struct MachineLifecycleSnapshotStoreVersionProbe {
    record_version: u16,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SupervisorAuthoritySnapshotStoreWire {
    #[default]
    UnboundNoReceipt,
    Bound {
        binding: SupervisorBindingReceiptStoreWire,
    },
    RevocationPending {
        pending: SupervisorRevocationPendingReceiptStoreWire,
    },
    RotationOperation {
        rotation: SupervisorRotationReceiptStoreWire,
    },
    RevokedReceipt {
        receipt: RevokedSupervisorReceiptStoreWire,
    },
    WithRotationHistory {
        current: Box<SupervisorAuthoritySnapshotStoreWire>,
        terminal_receipts: Vec<SupervisorRotationReceiptStoreWire>,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SupervisorBindingReceiptStoreWire {
    name: String,
    peer_id: String,
    address: String,
    signing_public_key: String,
    epoch: u64,
}

impl From<&SupervisorBindingReceipt> for SupervisorBindingReceiptStoreWire {
    fn from(receipt: &SupervisorBindingReceipt) -> Self {
        Self {
            name: receipt.name().to_owned(),
            peer_id: receipt.peer_id().to_owned(),
            address: receipt.address().to_owned(),
            signing_public_key: receipt.signing_public_key().to_owned(),
            epoch: receipt.epoch(),
        }
    }
}

impl From<SupervisorBindingReceiptStoreWire> for SupervisorBindingReceipt {
    fn from(receipt: SupervisorBindingReceiptStoreWire) -> Self {
        Self::new(
            receipt.name,
            receipt.peer_id,
            receipt.address,
            receipt.signing_public_key,
            receipt.epoch,
        )
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokedSupervisorReceiptStoreWire {
    peer_id: String,
    signing_public_key: String,
    epoch: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SupervisorRevocationPendingReceiptStoreWire {
    name: String,
    peer_id: String,
    address: String,
    signing_public_key: String,
    epoch: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SupervisorRotationReceiptStoreWire {
    operation_id: meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId,
    phase: SupervisorRotationPersistencePhase,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes missing from explicit null"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    rejection: Option<Option<SupervisorRotationRejection>>,
    previous: SupervisorBindingReceiptStoreWire,
    next: SupervisorBindingReceiptStoreWire,
}

impl From<&SupervisorRotationReceipt> for SupervisorRotationReceiptStoreWire {
    fn from(receipt: &SupervisorRotationReceipt) -> Self {
        Self {
            operation_id: receipt.operation_id(),
            phase: receipt.phase(),
            rejection: Some(receipt.rejection()),
            previous: receipt.previous().into(),
            next: receipt.next().into(),
        }
    }
}

impl TryFrom<SupervisorRotationReceiptStoreWire> for SupervisorRotationReceipt {
    type Error = RuntimeStoreError;

    fn try_from(receipt: SupervisorRotationReceiptStoreWire) -> Result<Self, Self::Error> {
        Ok(Self::new(
            receipt.operation_id,
            receipt.phase,
            require_present_nullable(receipt.rejection, "supervisor_authority.rotation.rejection")?,
            receipt.previous.into(),
            receipt.next.into(),
        ))
    }
}

impl From<&SupervisorRevocationPendingReceipt> for SupervisorRevocationPendingReceiptStoreWire {
    fn from(receipt: &SupervisorRevocationPendingReceipt) -> Self {
        Self {
            name: receipt.name().to_owned(),
            peer_id: receipt.peer_id().to_owned(),
            address: receipt.address().to_owned(),
            signing_public_key: receipt.signing_public_key().to_owned(),
            epoch: receipt.epoch(),
        }
    }
}

impl From<SupervisorRevocationPendingReceiptStoreWire> for SupervisorRevocationPendingReceipt {
    fn from(receipt: SupervisorRevocationPendingReceiptStoreWire) -> Self {
        Self::new(
            receipt.name,
            receipt.peer_id,
            receipt.address,
            receipt.signing_public_key,
            receipt.epoch,
        )
    }
}

impl From<&RevokedSupervisorReceipt> for RevokedSupervisorReceiptStoreWire {
    fn from(receipt: &RevokedSupervisorReceipt) -> Self {
        Self {
            peer_id: receipt.peer_id().to_owned(),
            signing_public_key: receipt.signing_public_key().to_owned(),
            epoch: receipt.epoch(),
        }
    }
}

impl From<RevokedSupervisorReceiptStoreWire> for RevokedSupervisorReceipt {
    fn from(receipt: RevokedSupervisorReceiptStoreWire) -> Self {
        Self::new(receipt.peer_id, receipt.signing_public_key, receipt.epoch)
    }
}

impl From<&SupervisorAuthoritySnapshot> for SupervisorAuthoritySnapshotStoreWire {
    fn from(snapshot: &SupervisorAuthoritySnapshot) -> Self {
        match snapshot {
            SupervisorAuthoritySnapshot::UnboundNoReceipt => Self::UnboundNoReceipt,
            SupervisorAuthoritySnapshot::Bound(binding) => Self::Bound {
                binding: binding.into(),
            },
            SupervisorAuthoritySnapshot::RevocationPending(pending) => Self::RevocationPending {
                pending: pending.into(),
            },
            SupervisorAuthoritySnapshot::RotationOperation(rotation) => Self::RotationOperation {
                rotation: rotation.into(),
            },
            SupervisorAuthoritySnapshot::RevokedReceipt(receipt) => Self::RevokedReceipt {
                receipt: receipt.into(),
            },
            SupervisorAuthoritySnapshot::WithRotationHistory {
                current,
                terminal_receipts,
            } => Self::WithRotationHistory {
                current: Box::new(current.as_ref().into()),
                terminal_receipts: terminal_receipts.values().map(Into::into).collect(),
            },
        }
    }
}

fn supervisor_authority_read_error(
    context: &str,
    detail: impl std::fmt::Display,
) -> RuntimeStoreError {
    RuntimeStoreError::ReadFailed(format!("{context}: {detail}"))
}

fn validate_supervisor_descriptor(
    name: &str,
    peer_id: &str,
    address: &str,
    signing_public_key: &str,
    context: &str,
) -> Result<(), RuntimeStoreError> {
    let pubkey = crate::comms_drain::decode_supervisor_signing_public_key(signing_public_key)
        .map_err(|error| supervisor_authority_read_error(context, error))?;
    let spec = meerkat_contracts::wire::supervisor_bridge::BridgePeerSpec {
        name: name.to_owned(),
        peer_id: peer_id.to_owned(),
        address: address.to_owned(),
        pubkey,
    };
    meerkat_core::comms::TrustedPeerDescriptor::try_from(&spec)
        .map(|_| ())
        .map_err(|error| supervisor_authority_read_error(context, error))
}

fn validate_supervisor_binding_receipt(
    receipt: &SupervisorBindingReceipt,
    context: &str,
) -> Result<(), RuntimeStoreError> {
    validate_supervisor_descriptor(
        receipt.name(),
        receipt.peer_id(),
        receipt.address(),
        receipt.signing_public_key(),
        context,
    )
}

fn validate_revoked_supervisor_receipt(
    receipt: &RevokedSupervisorReceipt,
    context: &str,
) -> Result<(), RuntimeStoreError> {
    let pubkey =
        crate::comms_drain::decode_supervisor_signing_public_key(receipt.signing_public_key())
            .map_err(|error| supervisor_authority_read_error(context, error))?;
    if pubkey.iter().all(|byte| *byte == 0) {
        return Err(supervisor_authority_read_error(
            context,
            "supervisor signing public key must be non-zero",
        ));
    }
    let peer_id = meerkat_core::comms::PeerId::parse(receipt.peer_id())
        .map_err(|error| supervisor_authority_read_error(context, error))?;
    let derived = meerkat_core::comms::PeerId::from_ed25519_pubkey(&pubkey);
    if peer_id != derived {
        return Err(supervisor_authority_read_error(
            context,
            format!("peer id {peer_id} does not match signing-key-derived id {derived}"),
        ));
    }
    Ok(())
}

fn validate_supervisor_rotation_receipt(
    receipt: &SupervisorRotationReceipt,
    terminal_history: bool,
) -> Result<(), RuntimeStoreError> {
    let operation_id = receipt.operation_id();
    if operation_id.as_uuid().is_nil() {
        return Err(supervisor_authority_read_error(
            "supervisor rotation operation",
            "operation id must not be the nil UUID",
        ));
    }
    validate_supervisor_binding_receipt(
        receipt.previous(),
        &format!("supervisor rotation {operation_id} previous authority is invalid"),
    )?;

    let rejection_matches = matches!(
        (receipt.phase(), receipt.rejection()),
        (
            SupervisorRotationPersistencePhase::PreviousRevokePending
                | SupervisorRotationPersistencePhase::NextPublishPending
                | SupervisorRotationPersistencePhase::Completed,
            None
        ) | (SupervisorRotationPersistencePhase::Rejected, Some(_))
    );
    if !rejection_matches {
        return Err(supervisor_authority_read_error(
            "supervisor rotation operation",
            format!("{operation_id} has inconsistent rejection state"),
        ));
    }
    if terminal_history
        && !matches!(
            receipt.phase(),
            SupervisorRotationPersistencePhase::Completed
                | SupervisorRotationPersistencePhase::Rejected
        )
    {
        return Err(supervisor_authority_read_error(
            "supervisor rotation history",
            format!("{operation_id} is not terminal"),
        ));
    }

    match receipt.phase() {
        SupervisorRotationPersistencePhase::PreviousRevokePending
        | SupervisorRotationPersistencePhase::NextPublishPending => {
            validate_supervisor_binding_receipt(
                receipt.next(),
                &format!("supervisor rotation {operation_id} target is invalid"),
            )?;
            if receipt.next().epoch() <= receipt.previous().epoch() {
                return Err(supervisor_authority_read_error(
                    "supervisor rotation operation",
                    format!(
                        "{operation_id} target epoch {} does not advance previous epoch {}",
                        receipt.next().epoch(),
                        receipt.previous().epoch()
                    ),
                ));
            }
        }
        SupervisorRotationPersistencePhase::Completed => {
            validate_supervisor_binding_receipt(
                receipt.next(),
                &format!("supervisor rotation {operation_id} target is invalid"),
            )?;
            // A legacy member may already have the exact target installed
            // before the operation protocol assigns an id. Its adoption
            // receipt is Completed with an exact previous == next witness.
            let exact_current_adoption = receipt.previous() == receipt.next();
            if !exact_current_adoption && receipt.next().epoch() <= receipt.previous().epoch() {
                return Err(supervisor_authority_read_error(
                    "supervisor rotation operation",
                    format!(
                        "{operation_id} completed target epoch {} does not advance previous epoch {}",
                        receipt.next().epoch(),
                        receipt.previous().epoch()
                    ),
                ));
            }
        }
        SupervisorRotationPersistencePhase::Rejected => {
            let Some(rejection) = receipt.rejection() else {
                return Err(supervisor_authority_read_error(
                    "supervisor rotation operation",
                    format!("{operation_id} rejected without a rejection class"),
                ));
            };
            match rejection {
                SupervisorRotationRejection::InvalidTarget
                | SupervisorRotationRejection::UnsupportedProtocolVersion => {
                    // These two rejection classes retain the undecodable target
                    // fields as raw evidence. They are deliberately exempt from
                    // target descriptor validation and epoch comparison.
                }
                SupervisorRotationRejection::TargetEpochNotAdvanced => {
                    validate_supervisor_binding_receipt(
                        receipt.next(),
                        &format!("supervisor rotation {operation_id} rejected target is invalid"),
                    )?;
                    if receipt.next().epoch() > receipt.previous().epoch() {
                        return Err(supervisor_authority_read_error(
                            "supervisor rotation operation",
                            format!(
                                "{operation_id} rejected as non-advancing but target epoch {} advances previous epoch {}",
                                receipt.next().epoch(),
                                receipt.previous().epoch()
                            ),
                        ));
                    }
                }
                SupervisorRotationRejection::OperationConflict
                | SupervisorRotationRejection::NotBound
                | SupervisorRotationRejection::SenderMismatch => {
                    return Err(supervisor_authority_read_error(
                        "supervisor rotation operation",
                        format!(
                            "{operation_id} transient rejection {rejection:?} must not be persisted as a durable receipt"
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

type SupervisorEpochKeyIndex = std::collections::BTreeMap<u64, [u8; 32]>;

fn record_supervisor_epoch_key(
    epochs: &mut SupervisorEpochKeyIndex,
    epoch: u64,
    signing_public_key: &str,
    context: &str,
) -> Result<(), RuntimeStoreError> {
    let key = crate::comms_drain::decode_supervisor_signing_public_key(signing_public_key)
        .map_err(|error| supervisor_authority_read_error(context, error))?;
    if let Some(existing) = epochs.get(&epoch) {
        if existing != &key {
            return Err(supervisor_authority_read_error(
                context,
                format!("epoch {epoch} is bound to conflicting supervisor signing keys"),
            ));
        }
    } else {
        epochs.insert(epoch, key);
    }
    Ok(())
}

fn record_supervisor_binding_epoch(
    epochs: &mut SupervisorEpochKeyIndex,
    receipt: &SupervisorBindingReceipt,
    context: &str,
) -> Result<(), RuntimeStoreError> {
    record_supervisor_epoch_key(
        epochs,
        receipt.epoch(),
        receipt.signing_public_key(),
        context,
    )
}

fn record_rotation_authoritative_epochs(
    epochs: &mut SupervisorEpochKeyIndex,
    receipt: &SupervisorRotationReceipt,
    context: &str,
) -> Result<(), RuntimeStoreError> {
    record_supervisor_binding_epoch(epochs, receipt.previous(), context)?;
    if matches!(
        receipt.phase(),
        SupervisorRotationPersistencePhase::PreviousRevokePending
            | SupervisorRotationPersistencePhase::NextPublishPending
            | SupervisorRotationPersistencePhase::Completed
    ) {
        record_supervisor_binding_epoch(epochs, receipt.next(), context)?;
    }
    Ok(())
}

fn record_current_authoritative_epochs(
    epochs: &mut SupervisorEpochKeyIndex,
    current: &SupervisorAuthoritySnapshot,
) -> Result<(), RuntimeStoreError> {
    match current {
        SupervisorAuthoritySnapshot::UnboundNoReceipt => Ok(()),
        SupervisorAuthoritySnapshot::Bound(binding) => {
            record_supervisor_binding_epoch(epochs, binding, "current supervisor authority")
        }
        SupervisorAuthoritySnapshot::RevocationPending(pending) => record_supervisor_epoch_key(
            epochs,
            pending.epoch(),
            pending.signing_public_key(),
            "current pending supervisor revocation authority",
        ),
        SupervisorAuthoritySnapshot::RotationOperation(rotation) => {
            record_rotation_authoritative_epochs(
                epochs,
                rotation,
                "current supervisor rotation authority",
            )
        }
        SupervisorAuthoritySnapshot::RevokedReceipt(receipt) => record_supervisor_epoch_key(
            epochs,
            receipt.epoch(),
            receipt.signing_public_key(),
            "current revoked supervisor authority",
        ),
        SupervisorAuthoritySnapshot::WithRotationHistory { .. } => {
            Err(RuntimeStoreError::ReadFailed(
                "nested supervisor rotation history is not canonical".to_string(),
            ))
        }
    }
}

fn current_supervisor_epoch(current: &SupervisorAuthoritySnapshot) -> Option<u64> {
    match current {
        SupervisorAuthoritySnapshot::UnboundNoReceipt => None,
        SupervisorAuthoritySnapshot::Bound(binding) => Some(binding.epoch()),
        SupervisorAuthoritySnapshot::RevocationPending(pending) => Some(pending.epoch()),
        SupervisorAuthoritySnapshot::RotationOperation(rotation) => Some(match rotation.phase() {
            SupervisorRotationPersistencePhase::PreviousRevokePending
            | SupervisorRotationPersistencePhase::Rejected => rotation.previous().epoch(),
            SupervisorRotationPersistencePhase::NextPublishPending
            | SupervisorRotationPersistencePhase::Completed => rotation.next().epoch(),
        }),
        SupervisorAuthoritySnapshot::RevokedReceipt(receipt) => Some(receipt.epoch()),
        SupervisorAuthoritySnapshot::WithRotationHistory { .. } => None,
    }
}

fn terminal_rotation_authority_epoch(receipt: &SupervisorRotationReceipt) -> u64 {
    match receipt.phase() {
        SupervisorRotationPersistencePhase::Completed => receipt.next().epoch(),
        SupervisorRotationPersistencePhase::Rejected => receipt.previous().epoch(),
        SupervisorRotationPersistencePhase::PreviousRevokePending
        | SupervisorRotationPersistencePhase::NextPublishPending => receipt.previous().epoch(),
    }
}

fn validate_supervisor_rotation_history_coherence(
    current: &SupervisorAuthoritySnapshot,
    terminal_receipts: &std::collections::BTreeMap<
        meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId,
        SupervisorRotationReceipt,
    >,
) -> Result<(), RuntimeStoreError> {
    let Some(current_epoch) = current_supervisor_epoch(current) else {
        return Err(RuntimeStoreError::ReadFailed(
            "supervisor rotation history requires a current authority epoch".to_string(),
        ));
    };

    let mut epochs = SupervisorEpochKeyIndex::new();
    record_current_authoritative_epochs(&mut epochs, current)?;
    let mut history_high_water = 0;
    for receipt in terminal_receipts.values() {
        record_rotation_authoritative_epochs(
            &mut epochs,
            receipt,
            "supervisor rotation history authority",
        )?;
        history_high_water = history_high_water.max(terminal_rotation_authority_epoch(receipt));
    }
    if current_epoch < history_high_water {
        return Err(RuntimeStoreError::ReadFailed(format!(
            "current supervisor epoch {current_epoch} is below terminal rotation history high-water {history_high_water}"
        )));
    }
    Ok(())
}

fn validate_supervisor_authority_snapshot(
    snapshot: &SupervisorAuthoritySnapshot,
) -> Result<(), RuntimeStoreError> {
    match snapshot {
        SupervisorAuthoritySnapshot::UnboundNoReceipt => Ok(()),
        SupervisorAuthoritySnapshot::Bound(binding) => {
            validate_supervisor_binding_receipt(binding, "bound supervisor is invalid")
        }
        SupervisorAuthoritySnapshot::RevocationPending(pending) => validate_supervisor_descriptor(
            pending.name(),
            pending.peer_id(),
            pending.address(),
            pending.signing_public_key(),
            "pending supervisor revocation authority is invalid",
        ),
        SupervisorAuthoritySnapshot::RotationOperation(rotation) => {
            validate_supervisor_rotation_receipt(rotation, false)
        }
        SupervisorAuthoritySnapshot::RevokedReceipt(receipt) => {
            validate_revoked_supervisor_receipt(receipt, "revoked supervisor receipt is invalid")
        }
        SupervisorAuthoritySnapshot::WithRotationHistory {
            current,
            terminal_receipts,
        } => {
            if matches!(
                current.as_ref(),
                SupervisorAuthoritySnapshot::WithRotationHistory { .. }
            ) {
                return Err(RuntimeStoreError::ReadFailed(
                    "nested supervisor rotation history is not canonical".to_string(),
                ));
            }
            if terminal_receipts.is_empty() {
                return Err(RuntimeStoreError::ReadFailed(
                    "empty supervisor rotation history wrapper is not canonical".to_string(),
                ));
            }
            validate_supervisor_authority_snapshot(current)?;
            for (operation_id, receipt) in terminal_receipts {
                if operation_id != &receipt.operation_id() {
                    return Err(RuntimeStoreError::ReadFailed(format!(
                        "supervisor rotation history key {operation_id} does not match receipt id {}",
                        receipt.operation_id()
                    )));
                }
                validate_supervisor_rotation_receipt(receipt, true)?;
            }
            if let SupervisorAuthoritySnapshot::RotationOperation(active) = current.as_ref()
                && terminal_receipts.contains_key(&active.operation_id())
            {
                return Err(RuntimeStoreError::ReadFailed(
                    "active supervisor rotation is duplicated in terminal history".to_string(),
                ));
            }
            validate_supervisor_rotation_history_coherence(current, terminal_receipts)
        }
    }
}

impl TryFrom<SupervisorAuthoritySnapshotStoreWire> for SupervisorAuthoritySnapshot {
    type Error = RuntimeStoreError;

    fn try_from(snapshot: SupervisorAuthoritySnapshotStoreWire) -> Result<Self, Self::Error> {
        match snapshot {
            SupervisorAuthoritySnapshotStoreWire::UnboundNoReceipt => Ok(Self::UnboundNoReceipt),
            SupervisorAuthoritySnapshotStoreWire::Bound { binding } => {
                let binding = binding.into();
                validate_supervisor_binding_receipt(&binding, "bound supervisor is invalid")?;
                Ok(Self::Bound(binding))
            }
            SupervisorAuthoritySnapshotStoreWire::RevocationPending { pending } => {
                let pending: SupervisorRevocationPendingReceipt = pending.into();
                validate_supervisor_descriptor(
                    pending.name(),
                    pending.peer_id(),
                    pending.address(),
                    pending.signing_public_key(),
                    "pending supervisor revocation authority is invalid",
                )?;
                Ok(Self::RevocationPending(pending))
            }
            SupervisorAuthoritySnapshotStoreWire::RotationOperation { rotation } => {
                let receipt: SupervisorRotationReceipt = rotation.try_into()?;
                validate_supervisor_rotation_receipt(&receipt, false)?;
                Ok(Self::RotationOperation(receipt))
            }
            SupervisorAuthoritySnapshotStoreWire::RevokedReceipt { receipt } => {
                let receipt = receipt.into();
                validate_revoked_supervisor_receipt(
                    &receipt,
                    "revoked supervisor receipt is invalid",
                )?;
                Ok(Self::RevokedReceipt(receipt))
            }
            SupervisorAuthoritySnapshotStoreWire::WithRotationHistory {
                current,
                terminal_receipts,
            } => {
                if terminal_receipts.is_empty() {
                    return Err(RuntimeStoreError::ReadFailed(
                        "empty supervisor rotation history wrapper is not canonical".to_string(),
                    ));
                }
                let current = Self::try_from(*current)?;
                if matches!(current, Self::WithRotationHistory { .. }) {
                    return Err(RuntimeStoreError::ReadFailed(
                        "nested supervisor rotation history is not canonical".to_string(),
                    ));
                }
                let mut receipts = std::collections::BTreeMap::new();
                for wire in terminal_receipts {
                    let receipt: SupervisorRotationReceipt = wire.try_into()?;
                    validate_supervisor_rotation_receipt(&receipt, true)?;
                    if receipts.insert(receipt.operation_id(), receipt).is_some() {
                        return Err(RuntimeStoreError::ReadFailed(
                            "supervisor rotation history contains a duplicate operation id"
                                .to_string(),
                        ));
                    }
                }
                if let Self::RotationOperation(active) = &current
                    && receipts.contains_key(&active.operation_id())
                {
                    return Err(RuntimeStoreError::ReadFailed(
                        "active supervisor rotation is duplicated in terminal history".to_string(),
                    ));
                }
                let snapshot = Self::WithRotationHistory {
                    current: Box::new(current),
                    terminal_receipts: receipts,
                };
                validate_supervisor_authority_snapshot(&snapshot)?;
                Ok(snapshot)
            }
        }
    }
}

impl From<&MachineLifecycleSnapshot> for MachineLifecycleSnapshotStoreWire {
    fn from(snapshot: &MachineLifecycleSnapshot) -> Self {
        Self {
            record_version: MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            runtime_state: snapshot.runtime_state(),
            binding: snapshot.binding().into(),
            current_run_id: snapshot.run().current_run_id().cloned(),
            pre_run_phase: snapshot.run().pre_run_phase(),
            supervisor_authority: snapshot.supervisor_authority().into(),
            unregister_progress: snapshot.unregister_progress().map(Into::into),
            live_bridge_recovery: snapshot.live_bridge_recovery().clone(),
        }
    }
}

fn validate_unregister_progress_snapshot(
    progress: Option<&MachineUnregisterProgressSnapshot>,
) -> Result<(), RuntimeStoreError> {
    if let Some(progress) = progress {
        if progress.runtime_loop_drain_pending() && progress.runtime_loop_forced_abort() {
            return Err(RuntimeStoreError::ReadFailed(
                "unregister runtime-loop forced disposition cannot precede obligation closure"
                    .into(),
            ));
        }
        if progress.comms_drain_exit_pending() && progress.comms_drain_forced_abort() {
            return Err(RuntimeStoreError::ReadFailed(
                "unregister comms-drain forced disposition cannot precede obligation closure"
                    .into(),
            ));
        }
    }
    Ok(())
}

impl TryFrom<MachineLifecycleSnapshotStoreWireV3> for MachineLifecycleSnapshot {
    type Error = RuntimeStoreError;

    fn try_from(record: MachineLifecycleSnapshotStoreWireV3) -> Result<Self, Self::Error> {
        if record.record_version != UNREGISTER_MACHINE_LIFECYCLE_STORE_RECORD_VERSION {
            return Err(RuntimeStoreError::ReadFailed(format!(
                "unsupported machine lifecycle store record version {}",
                record.record_version
            )));
        }
        let unregister_progress =
            require_present_nullable(record.unregister_progress, "unregister_progress")?
                .map(Into::into);
        validate_unregister_progress_snapshot(unregister_progress.as_ref())?;
        Ok(Self::new_with_unregister_progress(
            record.runtime_state,
            record.binding.try_into()?,
            record.supervisor_authority.try_into()?,
            unregister_progress,
        ))
    }
}

fn decode_machine_lifecycle_observation_v4(
    bytes: &[u8],
) -> Result<DecodedMachineLifecycleObservation, RuntimeStoreError> {
    let record = serde_json::from_slice::<MachineLifecycleObservationStoreWireV4>(bytes)
        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
    if record.record_version != RUN_MACHINE_LIFECYCLE_STORE_RECORD_VERSION {
        return Err(RuntimeStoreError::ReadFailed(format!(
            "unsupported machine lifecycle store record version {}",
            record.record_version
        )));
    }
    let runtime_state = require_present_nullable(record.runtime_state, "runtime_state")?;
    let current_run_id = require_present_nullable(record.current_run_id, "current_run_id")?;
    let pre_run_phase = require_present_nullable(record.pre_run_phase, "pre_run_phase")?;
    let unregister_progress =
        require_present_nullable(record.unregister_progress, "unregister_progress")?
            .map(Into::into);
    validate_unregister_progress_snapshot(unregister_progress.as_ref())?;
    Ok(DecodedMachineLifecycleObservation {
        record_version: record.record_version,
        runtime_state,
        binding: record.binding.try_into()?,
        run: MachineLifecycleRunFacts::new(current_run_id, pre_run_phase),
        supervisor_authority: record.supervisor_authority.try_into()?,
        unregister_progress,
        live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage::default(),
    })
}

fn decode_machine_lifecycle_observation_v5(
    bytes: &[u8],
) -> Result<DecodedMachineLifecycleObservation, RuntimeStoreError> {
    let record = serde_json::from_slice::<MachineLifecycleObservationStoreWireV5>(bytes)
        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
    if record.record_version != MACHINE_LIFECYCLE_STORE_RECORD_VERSION {
        return Err(RuntimeStoreError::ReadFailed(format!(
            "unsupported machine lifecycle store record version {}",
            record.record_version
        )));
    }
    let runtime_state = require_present_nullable(record.runtime_state, "runtime_state")?;
    let current_run_id = require_present_nullable(record.current_run_id, "current_run_id")?;
    let pre_run_phase = require_present_nullable(record.pre_run_phase, "pre_run_phase")?;
    let unregister_progress =
        require_present_nullable(record.unregister_progress, "unregister_progress")?
            .map(Into::into);
    validate_unregister_progress_snapshot(unregister_progress.as_ref())?;
    record
        .live_bridge_recovery
        .validate_bound()
        .map_err(RuntimeStoreError::ReadFailed)?;
    Ok(DecodedMachineLifecycleObservation {
        record_version: record.record_version,
        runtime_state,
        binding: record.binding.try_into()?,
        run: MachineLifecycleRunFacts::new(current_run_id, pre_run_phase),
        supervisor_authority: record.supervisor_authority.try_into()?,
        unregister_progress,
        live_bridge_recovery: record.live_bridge_recovery,
    })
}

fn decoded_machine_lifecycle_from_snapshot(
    record_version: u16,
    snapshot: MachineLifecycleSnapshot,
) -> DecodedMachineLifecycleObservation {
    DecodedMachineLifecycleObservation {
        record_version,
        runtime_state: Some(snapshot.runtime_state),
        binding: snapshot.binding,
        run: snapshot.run,
        supervisor_authority: snapshot.supervisor_authority,
        unregister_progress: snapshot.unregister_progress,
        live_bridge_recovery: snapshot.live_bridge_recovery,
    }
}

fn decode_machine_lifecycle_store_record(
    bytes: &[u8],
) -> Result<MachineLifecycleSnapshot, RuntimeStoreError> {
    let version = serde_json::from_slice::<MachineLifecycleSnapshotStoreVersionProbe>(bytes)
        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
    match version.record_version {
        LEGACY_MACHINE_LIFECYCLE_STORE_RECORD_VERSION => {
            let record = serde_json::from_slice::<MachineLifecycleSnapshotStoreWireV1>(bytes)
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
            if record.record_version != LEGACY_MACHINE_LIFECYCLE_STORE_RECORD_VERSION {
                return Err(RuntimeStoreError::ReadFailed(format!(
                    "unsupported machine lifecycle store record version {}",
                    record.record_version
                )));
            }
            Ok(MachineLifecycleSnapshot::new(
                record.runtime_state,
                record.binding.into(),
                SupervisorAuthoritySnapshot::UnboundNoReceipt,
            ))
        }
        SUPERVISOR_MACHINE_LIFECYCLE_STORE_RECORD_VERSION => {
            let record = serde_json::from_slice::<MachineLifecycleSnapshotStoreWireV2>(bytes)
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
            if record.record_version != SUPERVISOR_MACHINE_LIFECYCLE_STORE_RECORD_VERSION {
                return Err(RuntimeStoreError::ReadFailed(format!(
                    "unsupported machine lifecycle store record version {}",
                    record.record_version
                )));
            }
            Ok(MachineLifecycleSnapshot::new(
                record.runtime_state,
                record.binding.try_into()?,
                record.supervisor_authority.try_into()?,
            ))
        }
        UNREGISTER_MACHINE_LIFECYCLE_STORE_RECORD_VERSION => {
            let record = serde_json::from_slice::<MachineLifecycleSnapshotStoreWireV3>(bytes)
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
            MachineLifecycleSnapshot::try_from(record)
        }
        RUN_MACHINE_LIFECYCLE_STORE_RECORD_VERSION => {
            let record = decode_machine_lifecycle_observation_v4(bytes)?;
            let runtime_state = record.runtime_state.ok_or_else(|| {
                RuntimeStoreError::ReadFailed(
                    "machine lifecycle runtime_state cannot be null for strict recovery".into(),
                )
            })?;
            Ok(
                MachineLifecycleSnapshot::new_with_run_and_unregister_progress(
                    runtime_state,
                    record.binding,
                    record.run,
                    record.supervisor_authority,
                    record.unregister_progress,
                ),
            )
        }
        MACHINE_LIFECYCLE_STORE_RECORD_VERSION => {
            let record = decode_machine_lifecycle_observation_v5(bytes)?;
            let runtime_state = record.runtime_state.ok_or_else(|| {
                RuntimeStoreError::ReadFailed(
                    "machine lifecycle runtime_state cannot be null for strict recovery".into(),
                )
            })?;
            Ok(
                MachineLifecycleSnapshot::new_with_run_unregister_progress_and_live_bridge(
                    runtime_state,
                    record.binding,
                    record.run,
                    record.supervisor_authority,
                    record.unregister_progress,
                    record.live_bridge_recovery,
                ),
            )
        }
        unsupported => Err(RuntimeStoreError::ReadFailed(format!(
            "unsupported machine lifecycle store record version {unsupported}"
        ))),
    }
}

#[derive(serde::Deserialize)]
struct MachineLifecycleRawVersionProbe {
    record_version: u64,
}

fn machine_lifecycle_record_version(bytes: &[u8]) -> Result<u64, String> {
    serde_json::from_slice::<MachineLifecycleRawVersionProbe>(bytes)
        .map(|probe| probe.record_version)
        .map_err(|error| {
            format!("machine lifecycle record_version is not uniquely readable: {error}")
        })
}

fn classify_machine_lifecycle_record(bytes: &[u8]) -> MachineLifecycleObservation {
    let version = MachineLifecycleObservationVersion::from_raw_record(bytes);
    let evidence_digest = version.as_str().to_owned();
    let record_version = match machine_lifecycle_record_version(bytes) {
        Ok(record_version) => record_version,
        Err(detail) => {
            return MachineLifecycleObservation::Malformed {
                record_version: None,
                evidence_digest,
                version,
                detail,
            };
        }
    };

    let supported = [
        u64::from(LEGACY_MACHINE_LIFECYCLE_STORE_RECORD_VERSION),
        u64::from(SUPERVISOR_MACHINE_LIFECYCLE_STORE_RECORD_VERSION),
        u64::from(UNREGISTER_MACHINE_LIFECYCLE_STORE_RECORD_VERSION),
        u64::from(RUN_MACHINE_LIFECYCLE_STORE_RECORD_VERSION),
        u64::from(MACHINE_LIFECYCLE_STORE_RECORD_VERSION),
    ];
    if !supported.contains(&record_version) {
        return MachineLifecycleObservation::Unsupported {
            record_version,
            evidence_digest,
            version,
        };
    }

    let decoded = if record_version == u64::from(MACHINE_LIFECYCLE_STORE_RECORD_VERSION) {
        decode_machine_lifecycle_observation_v5(bytes)
    } else if record_version == u64::from(RUN_MACHINE_LIFECYCLE_STORE_RECORD_VERSION) {
        decode_machine_lifecycle_observation_v4(bytes)
    } else {
        decode_machine_lifecycle_store_record(bytes).map(|snapshot| {
            decoded_machine_lifecycle_from_snapshot(record_version as u16, snapshot)
        })
    };
    match decoded {
        Ok(record) => MachineLifecycleObservation::Decoded { record, version },
        Err(error) => MachineLifecycleObservation::Malformed {
            record_version: Some(record_version),
            evidence_digest,
            version,
            detail: error.to_string(),
        },
    }
}

#[cfg(test)]
pub(crate) async fn assert_input_idempotency_final_image_contract(store: &dyn RuntimeStore) {
    fn state_with_key(input_id: InputId, key: &str) -> StoredInputState {
        let mut state = StoredInputState::new_accepted(input_id);
        state.state.idempotency_key = Some(IdempotencyKey::new(key));
        state
    }

    fn record(state: StoredInputState) -> InputStatePersistenceRecord {
        InputStatePersistenceRecord::from_machine_snapshot(state).unwrap()
    }

    let runtime_id =
        LogicalRuntimeId::new(format!("idempotency-final-image-{}", uuid::Uuid::now_v7()));
    let left_id = InputId::new();
    let right_id = InputId::new();
    let left = state_with_key(left_id.clone(), "left-key");
    let right = state_with_key(right_id.clone(), "right-key");
    store
        .persist_input_states_atomically(
            &runtime_id,
            &[record(left.clone()), record(right.clone())],
        )
        .await
        .unwrap();

    let swapped_left = state_with_key(left_id.clone(), "right-key");
    let swapped_right = state_with_key(right_id.clone(), "left-key");
    store
        .persist_input_states_atomically(
            &runtime_id,
            &[record(swapped_left.clone()), record(swapped_right.clone())],
        )
        .await
        .expect("complete-final-image persistence must permit a key swap");
    let left_key_owner = store
        .load_input_state_by_idempotency_key(&runtime_id, &IdempotencyKey::new("left-key"))
        .await
        .unwrap()
        .expect("left key after swap");
    let right_key_owner = store
        .load_input_state_by_idempotency_key(&runtime_id, &IdempotencyKey::new("right-key"))
        .await
        .unwrap()
        .expect("right key after swap");
    assert_eq!(left_key_owner.state().state.input_id, right_id);
    assert_eq!(right_key_owner.state().state.input_id, left_id);

    assert_eq!(
        store
            .compare_and_swap_input_states_atomically(
                &runtime_id,
                &[swapped_left, swapped_right],
                &[record(left), record(right)],
            )
            .await
            .unwrap(),
        InputStateBatchCasOutcome::Swapped,
        "complete-final-image CAS must permit the reverse key swap"
    );
}

#[cfg(test)]
pub(crate) fn pending_terminal_owner_fixture(
    input_id: InputId,
    published: bool,
) -> (StoredInputState, InputStatePersistenceRecord) {
    use crate::input_state::{
        InputState, InputStateSeed, InteractionTerminalBatchKey, InteractionTerminalCandidate,
        InteractionTerminalOutbox, InteractionTerminalOutboxPhase, InteractionTerminalPublication,
        interaction_terminal_payload_digest,
    };

    let candidate = InteractionTerminalCandidate::RuntimeTerminated {
        reason: "indexed terminal recovery fixture".to_string(),
    };
    let recipients = vec![input_id.clone()];
    let candidate_digest = interaction_terminal_payload_digest(&candidate).unwrap();
    let completion_input_ids_digest = interaction_terminal_payload_digest(&recipients).unwrap();
    let phase = if published {
        InteractionTerminalOutboxPhase::Published {
            finalization_failed: false,
            publication: InteractionTerminalPublication {
                terminal_seq: 1,
                payload_digest: "published-payload".to_string(),
            },
        }
    } else {
        InteractionTerminalOutboxPhase::Candidate
    };
    let outbox = InteractionTerminalOutbox {
        interaction_id: meerkat_core::interaction::InteractionId(input_id.0),
        input_id: input_id.clone(),
        batch_ordinal: 0,
        batch_key: InteractionTerminalBatchKey::RuntimeTermination {
            candidate_owner_input_id: input_id.clone(),
        },
        owner_session_id: meerkat_core::types::SessionId::new(),
        owner_agent_runtime_id: Some("indexed-runtime".to_string()),
        owner_fence_token: Some(1),
        owner_runtime_generation: Some(1),
        owner_runtime_epoch_id: Some("indexed-epoch".to_string()),
        candidate_owner_input_id: input_id.clone(),
        candidate: (!published).then_some(candidate),
        candidate_digest,
        completion_input_ids: (!published).then_some(recipients),
        completion_input_ids_digest,
        phase,
    };
    outbox.validate().unwrap();
    let directed_input = crate::mob_adapter::create_tracked_flow_step_input(
        "fixture-step",
        meerkat_core::types::ContentInput::Text("fixture-directed-input".to_string()),
        "fixture-run",
        None,
        &input_id.to_string(),
    )
    .unwrap();
    let mut state = InputState::new_accepted(input_id);
    state.directed_run_started_attribution =
        crate::input_state::DirectedRunStartedAttribution::from_input(&directed_input).unwrap();
    state.interaction_terminal_outbox = Some(outbox);
    let stored = StoredInputState {
        state,
        seed: InputStateSeed::new_accepted(),
    };
    let record = InputStatePersistenceRecord::from_machine_snapshot(stored.clone()).unwrap();
    (stored, record)
}

#[cfg(test)]
pub(crate) async fn assert_pending_terminal_owner_index_contract(store: &dyn RuntimeStore) {
    let runtime_id =
        LogicalRuntimeId::new(format!("pending-terminal-index-{}", uuid::Uuid::now_v7()));
    let mut ids = [InputId::new(), InputId::new(), InputId::new()];
    ids.sort_by_key(|input_id| input_id.0);
    let fixtures = ids
        .iter()
        .cloned()
        .map(|input_id| pending_terminal_owner_fixture(input_id, false))
        .collect::<Vec<_>>();
    for (_, record) in &fixtures {
        store
            .persist_input_state(&runtime_id, record)
            .await
            .unwrap();
    }

    let first_page = store
        .load_pending_terminal_owner_ids_page(&runtime_id, None, 2)
        .await
        .unwrap();
    assert_eq!(first_page, ids[..2]);
    let second_page = store
        .load_pending_terminal_owner_ids_page(&runtime_id, first_page.last(), 2)
        .await
        .unwrap();
    assert_eq!(second_page, ids[2..]);

    let (_, published) = pending_terminal_owner_fixture(ids[1].clone(), true);
    assert_eq!(
        store
            .compare_and_swap_input_states_atomically(
                &runtime_id,
                std::slice::from_ref(&fixtures[1].0),
                std::slice::from_ref(&published),
            )
            .await
            .unwrap(),
        InputStateBatchCasOutcome::Swapped
    );
    assert_eq!(
        store
            .load_pending_terminal_owner_ids_page(&runtime_id, None, 3)
            .await
            .unwrap(),
        vec![ids[0].clone(), ids[2].clone()]
    );
}

fn replacement_repair_blocked(
    evidence_digest: Option<String>,
    detail: impl Into<String>,
) -> RuntimeStoreError {
    RuntimeStoreError::MachineLifecycleRepairBlocked {
        evidence_digest,
        detail: detail.into(),
    }
}

/// Validate whether an exact lifecycle observation may be normalized.
///
/// Binding, fence, generation, and run atoms describe the dead process that
/// authored the observed row; they are not a durable high-water authority and
/// may be cleared by an exact-version cold-normalization CAS. Unsupported and
/// malformed rows remain fail-closed because this slice cannot prove their
/// custody fields safe to preserve.
fn validate_machine_lifecycle_replacement(
    current: &MachineLifecycleObservation,
    _current_raw: Option<&[u8]>,
    _replacement: &MachineLifecycleSnapshot,
) -> Result<(), RuntimeStoreError> {
    match current {
        MachineLifecycleObservation::Missing | MachineLifecycleObservation::Decoded { .. } => {
            Ok(())
        }
        MachineLifecycleObservation::Unsupported {
            evidence_digest,
            record_version,
            ..
        } => Err(replacement_repair_blocked(
            Some(evidence_digest.clone()),
            format!(
                "unsupported lifecycle record version {record_version} cannot prove fencing semantics"
            ),
        )),
        MachineLifecycleObservation::Malformed {
            evidence_digest,
            detail,
            ..
        } => Err(replacement_repair_blocked(
            Some(evidence_digest.clone()),
            format!("malformed lifecycle evidence is not reclaimable: {detail}"),
        )),
    }
}

struct PreparedMachineLifecycleReplacement {
    snapshot: MachineLifecycleSnapshot,
    bytes: Vec<u8>,
    version: MachineLifecycleObservationVersion,
}

impl PreparedMachineLifecycleReplacement {
    /// A runtime-authority normalization owns only lifecycle, binding, and run
    /// atoms. Preserve independent supervisor and unregister custody from the
    /// exact decoded row rather than copying it through the reconciler.
    fn preserve_observed_custody(
        mut self,
        current: &MachineLifecycleObservation,
    ) -> Result<Self, RuntimeStoreError> {
        if let MachineLifecycleObservation::Decoded { record, .. } = current {
            self.snapshot.supervisor_authority = record.supervisor_authority().clone();
            self.snapshot.unregister_progress = record.unregister_progress().cloned();
            self.bytes = MachineLifecycleStoreRecord::from_snapshot(&self.snapshot).encode()?;
            self.version = MachineLifecycleObservationVersion::from_raw_record(&self.bytes);
        }
        Ok(self)
    }
}

fn prepare_machine_lifecycle_replacement(
    commit: MachineLifecycleCommit,
) -> Result<PreparedMachineLifecycleReplacement, RuntimeStoreError> {
    let bytes = commit.store_record().encode()?;
    let version = MachineLifecycleObservationVersion::from_raw_record(&bytes);
    Ok(PreparedMachineLifecycleReplacement {
        snapshot: commit.into_snapshot(),
        bytes,
        version,
    })
}

fn decoded_prepared_machine_lifecycle_replacement(
    replacement: &PreparedMachineLifecycleReplacement,
) -> Result<DecodedMachineLifecycleObservation, RuntimeStoreError> {
    match classify_machine_lifecycle_record(&replacement.bytes) {
        MachineLifecycleObservation::Decoded { record, .. } => Ok(record),
        other => Err(RuntimeStoreError::Internal(format!(
            "machine-authorized lifecycle replacement did not decode: {other:?}"
        ))),
    }
}

/// Load the last persisted runtime-state projection from a generated lifecycle
/// record.
///
/// This is a projection of [`MachineLifecycleCommit`] authority. Store
/// implementations provide only opaque record bytes; the runtime crate owns the
/// decoding and rejects compatibility rows that are not machine lifecycle
/// records.
pub async fn load_runtime_state(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<Option<RuntimeState>, RuntimeStoreError> {
    Ok(load_machine_lifecycle(store, runtime_id)
        .await?
        .map(|snapshot| snapshot.runtime_state()))
}

pub(crate) async fn load_machine_lifecycle(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<Option<MachineLifecycleSnapshot>, RuntimeStoreError> {
    store
        .load_machine_lifecycle_record(runtime_id)
        .await?
        .map(|bytes| decode_machine_lifecycle_store_record(&bytes))
        .transpose()
}

/// Declared durable store record for generated machine lifecycle truth.
///
/// Stores receive this record from [`MachineLifecycleCommit`] and may persist
/// its encoded form. Loading must decode this exact record shape; compatibility
/// runtime-state projections are not lifecycle authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineLifecycleStoreRecord {
    snapshot: MachineLifecycleSnapshot,
}

impl MachineLifecycleStoreRecord {
    pub(crate) fn from_snapshot(snapshot: &MachineLifecycleSnapshot) -> Self {
        Self {
            snapshot: snapshot.clone(),
        }
    }

    /// Runtime state carried by this exact machine-authorized store record.
    ///
    /// Custom stores use this bounded fact to advance an existing session
    /// catalog projection in the same transaction as the lifecycle row.
    #[must_use]
    pub fn runtime_state(&self) -> RuntimeState {
        self.snapshot.runtime_state()
    }

    pub fn encode(&self) -> Result<Vec<u8>, RuntimeStoreError> {
        validate_supervisor_authority_snapshot(self.snapshot.supervisor_authority())
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        validate_unregister_progress_snapshot(self.snapshot.unregister_progress())
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let wire = MachineLifecycleSnapshotStoreWire::from(&self.snapshot);
        serde_json::to_vec(&wire).map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))
    }
}

/// Machine-owned lifecycle commit token.
///
/// This token has no public constructor. RuntimeStore implementors can persist
/// the selected state and binding facts, but callers outside the machine/driver
/// commit path cannot select arbitrary lifecycle truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineLifecycleCommit {
    snapshot: MachineLifecycleSnapshot,
    /// When set, the store must verify the CURRENT persisted lifecycle row
    /// still matches this exact version inside the same transaction that
    /// writes the commit, and fail the whole boundary with
    /// [`RuntimeStoreError::MachineLifecycleVersionConflict`] otherwise.
    /// `None` preserves the historical last-writer semantics of the live
    /// driver commit path, whose exclusive in-process authority already
    /// serializes writers.
    expected_version: Option<MachineLifecycleExpectedVersion>,
}

impl MachineLifecycleCommit {
    #[cfg(test)]
    pub(crate) fn new_with_binding(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
    ) -> Self {
        Self::new_with_binding_and_unregister_progress(
            runtime_state,
            binding,
            supervisor_authority,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_binding_and_unregister_progress(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
        unregister_progress: Option<MachineUnregisterProgressSnapshot>,
    ) -> Self {
        Self::new_with_binding_run_and_unregister_progress(
            runtime_state,
            binding,
            MachineLifecycleRunFacts::default(),
            supervisor_authority,
            unregister_progress,
        )
    }

    pub(crate) fn new_with_binding_unregister_progress_and_live_bridge(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
        unregister_progress: Option<MachineUnregisterProgressSnapshot>,
        live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage,
    ) -> Self {
        Self::new_with_binding_run_unregister_progress_and_live_bridge(
            runtime_state,
            binding,
            MachineLifecycleRunFacts::default(),
            supervisor_authority,
            unregister_progress,
            live_bridge_recovery,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_binding_run_and_unregister_progress(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        run: MachineLifecycleRunFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
        unregister_progress: Option<MachineUnregisterProgressSnapshot>,
    ) -> Self {
        Self::new_with_binding_run_unregister_progress_and_live_bridge(
            runtime_state,
            binding,
            run,
            supervisor_authority,
            unregister_progress,
            crate::live_execution::LiveBridgeRecoveryImage::default(),
        )
    }

    pub(crate) fn new_with_binding_run_unregister_progress_and_live_bridge(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        run: MachineLifecycleRunFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
        unregister_progress: Option<MachineUnregisterProgressSnapshot>,
        live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage,
    ) -> Self {
        Self {
            snapshot: MachineLifecycleSnapshot::new_with_run_unregister_progress_and_live_bridge(
                runtime_state,
                binding,
                run,
                supervisor_authority,
                unregister_progress,
                live_bridge_recovery,
            ),
            expected_version: None,
        }
    }

    /// Fence this commit on the exact lifecycle row version it was derived
    /// from. Used by cold recovery: between observing the persisted row and
    /// committing the recovered boundary another process may register or
    /// advance the runtime, and a blind upsert would stomp its truth.
    pub(crate) fn with_expected_version(
        mut self,
        expected: MachineLifecycleExpectedVersion,
    ) -> Self {
        self.expected_version = Some(expected);
        self
    }

    /// Runtime state selected by the owning MeerkatMachine transition.
    pub fn runtime_state(&self) -> RuntimeState {
        self.snapshot.runtime_state()
    }

    /// Durable lifecycle snapshot selected by the owning MeerkatMachine transition.
    pub fn snapshot(&self) -> &MachineLifecycleSnapshot {
        &self.snapshot
    }

    /// Durable record selected by the owning MeerkatMachine transition.
    pub fn store_record(&self) -> MachineLifecycleStoreRecord {
        MachineLifecycleStoreRecord::from_snapshot(&self.snapshot)
    }

    /// Exact prior row version this commit is fenced on, when the producer
    /// demanded compare-and-swap semantics. Store implementations MUST
    /// enforce it inside the same transaction that writes the commit.
    pub fn expected_version(&self) -> Option<&MachineLifecycleExpectedVersion> {
        self.expected_version.as_ref()
    }

    pub(crate) fn into_snapshot(self) -> MachineLifecycleSnapshot {
        self.snapshot
    }
}

/// Machine-authorized final-unregister persistence token.
///
/// The token bundles terminal lifecycle truth with the exact authorized input
/// snapshot. It has no public constructor and can only be minted by consuming
/// the private-field delete witness derived from the generated
/// `DeleteSnapshot` unregister verdict.
#[derive(Debug, Clone)]
pub struct UnregisterFinalizationCommit {
    machine_lifecycle: MachineLifecycleCommit,
    input_states: Vec<InputStatePersistenceRecord>,
    retired_ops_epoch: meerkat_core::RuntimeEpochId,
}

impl UnregisterFinalizationCommit {
    pub(crate) fn new(
        machine_lifecycle: MachineLifecycleCommit,
        input_states: Vec<InputStatePersistenceRecord>,
        retired_ops_epoch: meerkat_core::RuntimeEpochId,
        _authority: crate::meerkat_machine::DeleteOpsFinalizationAuthority,
    ) -> Self {
        Self {
            machine_lifecycle,
            input_states,
            retired_ops_epoch,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        MachineLifecycleSnapshot,
        Vec<InputStatePersistenceRecord>,
        meerkat_core::RuntimeEpochId,
    ) {
        (
            self.machine_lifecycle.into_snapshot(),
            self.input_states,
            self.retired_ops_epoch,
        )
    }

    /// Opaque encoded lifecycle record selected by final-unregister machine
    /// authority. External stores can persist this without gaining a way to
    /// construct or alter the authority token.
    pub fn lifecycle_store_record(&self) -> MachineLifecycleStoreRecord {
        self.machine_lifecycle.store_record()
    }

    /// Authorized input-state rows that must commit in the same transaction.
    pub fn input_states(&self) -> &[InputStatePersistenceRecord] {
        &self.input_states
    }

    /// Exact ops epoch retired by this finalization transaction.
    pub fn retired_ops_epoch(&self) -> &meerkat_core::RuntimeEpochId {
        &self.retired_ops_epoch
    }
}

/// One durable input row observed by [`RuntimeStore::load_input_states`].
#[derive(Debug, Clone)]
pub enum InputStateRow {
    /// The row decoded under this binary's persisted contract.
    Decoded(Box<StoredInputState>),
    /// The row's persisted bytes no longer decode. The row stays on disk
    /// untouched (forensics), and is reported typed so one damaged row does
    /// not make the runtime's other durable inputs unreadable.
    Corrupt {
        /// Row key as stored (the row's JSON no longer parses, so the typed
        /// `InputId` cannot be recovered from it).
        input_id: String,
        /// Decode failure detail.
        detail: String,
    },
}

/// Recovery projection of [`RuntimeStore::load_input_states`]: corrupt rows
/// are reported loudly and skipped so one damaged row cannot make the whole
/// runtime unrecoverable (the v0.8.7 failure mode). The damaged rows stay on
/// disk untouched for forensics.
pub async fn load_input_states_for_recovery(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<Vec<StoredInputState>, RuntimeStoreError> {
    let mut states = Vec::new();
    for row in store.load_input_states(runtime_id).await? {
        match row {
            InputStateRow::Decoded(state) => states.push(*state),
            InputStateRow::Corrupt { input_id, detail } => {
                tracing::error!(
                    runtime_id = %runtime_id.0,
                    input_id = %input_id,
                    detail = %detail,
                    "durable input row no longer decodes; recovering the runtime's remaining inputs without it"
                );
            }
        }
    }
    Ok(states)
}

/// Atomic persistence interface for runtime state.
///
/// Implementations:
/// - `InMemoryRuntimeStore` — in-memory, no durability (ephemeral/testing)
/// - `SqliteRuntimeStore` — SQLite-backed durable runtime state
///
/// A store may contain many logical runtime ids, but each id is controlled by
/// one live `MeerkatMachine` authority. Store transactions provide durable
/// atomicity; they are not a distributed lease for two machines concurrently
/// controlling the same logical runtime.
///
/// Every operation that mutates more than one input row evaluates the unique
/// idempotency-key constraint against the batch's complete final image. Target
/// rows relinquish their old keys as one logical set before any target claims
/// its successor key, so a valid key swap is accepted regardless of mutation
/// order. A duplicate final claim or a claim held by a row outside the mutation
/// set rejects the entire operation without exposing any sibling effect.
///
/// This object-safe carrier is implemented only by real persistence backends.
/// Its methods have the same contracts as the corresponding forwarding
/// methods on [`RuntimeStore`]. Every method is required: profile-specific
/// capability refusals are explicit backend behavior, never inherited
/// defaults.
///
/// This is an implementor seam. Operational callers use [`RuntimeStore`] so a
/// decorator's intentional per-operation overrides remain observable.
#[doc(hidden)]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait RuntimeSessionAuthorityOps: Send + Sync {
    fn session_persistence_profile(&self) -> RuntimeSessionPersistenceProfile;

    fn session_boundary_authority_read_cost(&self) -> RuntimeSessionAuthorityReadCost;

    /// Consume one exact store-verified physical activation proof and align
    /// the matching runtime HeadCanonical authority atomically.
    async fn activate_head_canonical_runtime_authority(
        &self,
        authority: meerkat_core::VerifiedHeadCanonicalAuthority,
    ) -> Result<HeadCanonicalRuntimeAuthorityActivation, RuntimeStoreError>;

    async fn commit_prepared_session_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        request: PreparedRuntimeSessionCommit,
    ) -> Result<PreparedRuntimeSessionCommitResult, RuntimeStoreError>;

    async fn load_session_boundary_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeSessionAuthority>, RuntimeStoreError>;

    async fn load_session_resume_observation(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RuntimeSessionResumeObservation, RuntimeStoreError>;

    async fn load_whole_blob_store_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<WholeBlobStoreAuthority>, RuntimeStoreError>;

    async fn load_committed_whole_blob_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<CommittedWholeBlobSnapshot>, RuntimeStoreError>;

    async fn commit_prepared_whole_blob_snapshot_cas(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobSnapshotCas,
    ) -> Result<WholeBlobSnapshotCasOutcome, RuntimeStoreError>;

    async fn delete_runtime_session_catalog_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError>;

    async fn load_runtime_session_catalog_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeSessionCatalogEntry>, RuntimeStoreError>;

    async fn list_runtime_session_catalog_entries(
        &self,
        filter: meerkat_core::SessionFilter,
    ) -> Result<Vec<RuntimeSessionCatalogEntry>, RuntimeStoreError>;

    async fn write_prepared_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobProvisionalTail,
    ) -> Result<WholeBlobProvisionalTailAuthority, RuntimeStoreError>;

    async fn load_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<CommittedWholeBlobProvisionalTail>, RuntimeStoreError>;

    async fn discard_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &WholeBlobProvisionalTailAuthority,
    ) -> Result<bool, RuntimeStoreError>;

    async fn write_prepared_head_canonical_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedHeadCanonicalProvisionalTail,
    ) -> Result<HeadCanonicalProvisionalTailAuthority, RuntimeStoreError>;

    async fn load_head_canonical_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<HeadCanonicalProvisionalTailAuthority>, RuntimeStoreError>;

    async fn discard_head_canonical_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &HeadCanonicalProvisionalTailAuthority,
    ) -> Result<bool, RuntimeStoreError>;

    async fn load_durable_tail_recovery_source(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<PreparedDurableTailRecoverySource>, RuntimeStoreError>;

    async fn load_durable_tail_recovery_receipts(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
    ) -> Result<Vec<PreparedRecoveryReceiptSource>, RuntimeStoreError>;

    async fn load_committed_recovery_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate_id: &str,
    ) -> Result<Option<CommittedRecoveryBoundary>, RuntimeStoreError>;
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait RuntimeStore: Send + Sync {
    /// Required carrier for the complete store-owned session-authority seam.
    ///
    /// Decorators forward this one accessor. A fault-injection decorator may
    /// still override the individual forwarding method it intentionally
    /// perturbs. Omitting the carrier is therefore a compile error instead of
    /// a runtime `Unsupported` surprise.
    #[doc(hidden)]
    fn session_authority_ops(&self) -> &dyn RuntimeSessionAuthorityOps;

    /// Durable session representation owned by this store.
    ///
    /// Every backend carrier must choose explicitly. `WholeBlobV1`
    /// materializes and writes the accumulated session document at each
    /// boundary, so its ordinary persistence cost is O(document).
    /// `HeadCanonicalV1` commits the prepared head/suffix mutation and small
    /// runtime authority incrementally. Every profile must implement
    /// [`RuntimeSessionAuthorityOps::commit_prepared_session_boundary`]
    /// directly; there is no checkpoint-derived or whole-blob compatibility
    /// bridge.
    fn session_persistence_profile(&self) -> RuntimeSessionPersistenceProfile {
        self.session_authority_ops().session_persistence_profile()
    }

    /// Declared cost of [`Self::load_session_boundary_authority`].
    ///
    /// The carrier default is deliberately unsupported. Backends opt in only
    /// after maintaining authority separately from the document body.
    fn session_boundary_authority_read_cost(&self) -> RuntimeSessionAuthorityReadCost {
        self.session_authority_ops()
            .session_boundary_authority_read_cost()
    }

    /// Consume one exact physical HeadCanonical activation proof at the
    /// backend construction boundary.
    async fn activate_head_canonical_runtime_authority(
        &self,
        authority: meerkat_core::VerifiedHeadCanonicalAuthority,
    ) -> Result<HeadCanonicalRuntimeAuthorityActivation, RuntimeStoreError> {
        self.session_authority_ops()
            .activate_head_canonical_runtime_authority(authority)
            .await
    }

    /// Commit one valid-by-construction prepared session boundary.
    ///
    /// Every backend carrier must override this operation. Only the backend
    /// can allocate the next physical revision and atomically bind it to the
    /// exact body/head, catalog projection, receipts, input rows, and lifecycle
    /// effects. A generic implementation cannot honestly mint store-issued
    /// authority.
    async fn commit_prepared_session_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        request: PreparedRuntimeSessionCommit,
    ) -> Result<PreparedRuntimeSessionCommitResult, RuntimeStoreError> {
        self.session_authority_ops()
            .commit_prepared_session_boundary(runtime_id, request)
            .await
    }

    /// Load the versioned session authority for a runtime.
    ///
    /// Implementations may expose this only as a bounded authority-row read.
    /// There is intentionally no default fallback through
    /// [`Self::load_session_snapshot`]: callers poll this seam during
    /// reconciliation, so parsing a WholeBlob body here would turn degraded
    /// operation into an invisible O(document) loop.
    async fn load_session_boundary_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeSessionAuthority>, RuntimeStoreError> {
        self.session_authority_ops()
            .load_session_boundary_authority(runtime_id)
            .await
    }

    /// Atomically observe the body-free authority and lifecycle facts needed
    /// to authorize a session resume attempt.
    ///
    /// Backends must read the session authority, catalog projection, and raw
    /// lifecycle row under one snapshot. This method never materializes the
    /// accumulated Session body.
    async fn load_session_resume_observation(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RuntimeSessionResumeObservation, RuntimeStoreError> {
        self.session_authority_ops()
            .load_session_resume_observation(runtime_id)
            .await
    }

    /// Observe only the fixed-size store-issued WholeBlob identity.
    async fn load_whole_blob_store_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<WholeBlobStoreAuthority>, RuntimeStoreError> {
        self.session_authority_ops()
            .load_whole_blob_store_authority(runtime_id)
            .await
    }

    /// Atomically pair the WholeBlob body with its store-issued identity.
    ///
    /// This is the source for resume/rewrite payload verification. Polling
    /// callers must use [`Self::load_whole_blob_store_authority`] instead.
    async fn load_committed_whole_blob_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<CommittedWholeBlobSnapshot>, RuntimeStoreError> {
        self.session_authority_ops()
            .load_committed_whole_blob_snapshot(runtime_id)
            .await
    }

    /// Commit one typed WholeBlob successor only while its exact store-issued
    /// predecessor remains current.
    ///
    /// Implementations compare only [`WholeBlobStoreAuthority`]. They must not
    /// derive currentness from Session checkpoint metadata or reread/compare a
    /// whole document.
    async fn commit_prepared_whole_blob_snapshot_cas(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobSnapshotCas,
    ) -> Result<WholeBlobSnapshotCasOutcome, RuntimeStoreError> {
        self.session_authority_ops()
            .commit_prepared_whole_blob_snapshot_cas(runtime_id, prepared)
            .await
    }

    /// Delete one exact runtime's catalog projection.
    async fn delete_runtime_session_catalog_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        self.session_authority_ops()
            .delete_runtime_session_catalog_entry(runtime_id)
            .await
    }

    /// Load one bounded, body-free runtime session catalog entry.
    async fn load_runtime_session_catalog_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeSessionCatalogEntry>, RuntimeStoreError> {
        self.session_authority_ops()
            .load_runtime_session_catalog_entry(runtime_id)
            .await
    }

    /// List body-free catalog entries in deterministic updated-descending,
    /// session-id-ascending order.
    async fn list_runtime_session_catalog_entries(
        &self,
        filter: meerkat_core::SessionFilter,
    ) -> Result<Vec<RuntimeSessionCatalogEntry>, RuntimeStoreError> {
        self.session_authority_ops()
            .list_runtime_session_catalog_entries(filter)
            .await
    }

    /// Write one typed provisional WholeBlob candidate exactly once.
    async fn write_prepared_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobProvisionalTail,
    ) -> Result<WholeBlobProvisionalTailAuthority, RuntimeStoreError> {
        self.session_authority_ops()
            .write_prepared_whole_blob_provisional_tail(runtime_id, prepared)
            .await
    }

    /// Atomically load one provisional authority and its candidate body.
    async fn load_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<CommittedWholeBlobProvisionalTail>, RuntimeStoreError> {
        self.session_authority_ops()
            .load_whole_blob_provisional_tail(runtime_id)
            .await
    }

    /// Discard only the exact provisional candidate named by `expected`.
    async fn discard_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &WholeBlobProvisionalTailAuthority,
    ) -> Result<bool, RuntimeStoreError> {
        self.session_authority_ops()
            .discard_whole_blob_provisional_tail(runtime_id, expected)
            .await
    }

    /// Persist one exact HeadCanonical provisional intent before the physical
    /// SessionStore CAS it authorizes.
    async fn write_prepared_head_canonical_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedHeadCanonicalProvisionalTail,
    ) -> Result<HeadCanonicalProvisionalTailAuthority, RuntimeStoreError> {
        self.session_authority_ops()
            .write_prepared_head_canonical_provisional_tail(runtime_id, prepared)
            .await
    }

    /// Load only the fixed-size HeadCanonical provisional authority.
    async fn load_head_canonical_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<HeadCanonicalProvisionalTailAuthority>, RuntimeStoreError> {
        self.session_authority_ops()
            .load_head_canonical_provisional_tail(runtime_id)
            .await
    }

    /// Discard only the exact HeadCanonical provisional authority supplied.
    async fn discard_head_canonical_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &HeadCanonicalProvisionalTailAuthority,
    ) -> Result<bool, RuntimeStoreError> {
        self.session_authority_ops()
            .discard_head_canonical_provisional_tail(runtime_id, expected)
            .await
    }

    /// Load one store-owned durable-tail source from a single verified
    /// authority/physical-head snapshot.
    ///
    /// Only a backend that atomically owns runtime authority and canonical
    /// session rows can implement this. The default refuses instead of
    /// accepting caller-supplied session/head facts.
    async fn load_durable_tail_recovery_source(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<PreparedDurableTailRecoverySource>, RuntimeStoreError> {
        self.session_authority_ops()
            .load_durable_tail_recovery_source(runtime_id)
            .await
    }

    /// Load every exact original receipt row for one store-derived recovery
    /// candidate run, ordered by boundary sequence.
    ///
    /// The row token in each opaque result lets a supported-floor missing
    /// conversation digest be enriched in the same transaction as recovery.
    async fn load_durable_tail_recovery_receipts(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
    ) -> Result<Vec<PreparedRecoveryReceiptSource>, RuntimeStoreError> {
        self.session_authority_ops()
            .load_durable_tail_recovery_receipts(runtime_id, run_id)
            .await
    }

    /// Load the durable exact-retry witness for one recovery candidate.
    ///
    /// Only a backend that owns runtime authority and the physical session
    /// head in one atomic resource may implement this. The generic WholeBlob
    /// profile has no way to recheck an external SessionStore row and therefore
    /// refuses rather than presenting a partial commit as converged recovery.
    async fn load_committed_recovery_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate_id: &str,
    ) -> Result<Option<CommittedRecoveryBoundary>, RuntimeStoreError> {
        self.session_authority_ops()
            .load_committed_recovery_boundary(runtime_id, candidate_id)
            .await
    }

    /// Whether [`RuntimeStore::atomic_apply`] durably records typed compaction
    /// projection intents in the same boundary as the session rewrite.
    /// Unknown/custom stores fail closed by default.
    fn supports_compaction_projection_outbox(&self) -> bool {
        false
    }

    /// Stable key for process-local auth/OAuth authority reuse across reopened
    /// handles for the same durable store.
    fn auth_authority_key(&self) -> Option<String> {
        None
    }

    /// Load the exact generated runtime-delivery authority record.
    async fn load_runtime_delivery_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeDeliveryAuthorityRecord>, RuntimeStoreError> {
        let _ = runtime_id;
        Err(RuntimeStoreError::Unsupported(
            "load_runtime_delivery_authority".into(),
        ))
    }

    /// Load one durable runtime-delivery inbox row by stable identity.
    async fn load_runtime_delivery_record(
        &self,
        runtime_id: &LogicalRuntimeId,
        delivery_id: &str,
    ) -> Result<Option<RuntimeDeliveryStoreRecord>, RuntimeStoreError> {
        let _ = (runtime_id, delivery_id);
        Err(RuntimeStoreError::Unsupported(
            "load_runtime_delivery_record".into(),
        ))
    }

    /// Compare-and-swap generated delivery authority and optionally insert one
    /// inbox row in the same atomic boundary.
    ///
    /// `expected_revision = None` means the authority row must be absent.
    /// Stores enforce only exact CAS, row uniqueness, and atomicity; the
    /// generated machine decides sequence allocation and application order.
    async fn compare_and_swap_runtime_delivery_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: Option<u64>,
        replacement: RuntimeDeliveryAuthorityRecord,
        inserted_delivery: Option<RuntimeDeliveryStoreRecord>,
    ) -> Result<RuntimeDeliveryAuthorityCasOutcome, RuntimeStoreError> {
        let _ = (
            runtime_id,
            expected_revision,
            replacement,
            inserted_delivery,
        );
        Err(RuntimeStoreError::Unsupported(
            "compare_and_swap_runtime_delivery_authority".into(),
        ))
    }

    /// List every generated delivery-authority record in this store, keyed by
    /// the runtime that owns it.
    ///
    /// This is the only cross-runtime delivery read. Every other delivery verb
    /// is scoped to one `runtime_id`, which makes a caller that wants a
    /// store-wide answer invent a candidate runtime set from somewhere else -
    /// and any such set can be wrong in the direction that matters: a runtime
    /// holding committed-but-undrained rows whose originating jobs have aged
    /// out of the caller's window is invisible, which is exactly the field
    /// symptom this exists to make visible.
    ///
    /// Deliberately uncapped. A limit here would restore the same class of
    /// miss one layer down. The pending arithmetic is NOT done here: how many
    /// rows are undrained is a fact of the generated delivery machine, and the
    /// store retains that machine's state without interpreting it.
    async fn list_runtime_delivery_authorities(
        &self,
    ) -> Result<Vec<(LogicalRuntimeId, RuntimeDeliveryAuthorityRecord)>, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "list_runtime_delivery_authorities".into(),
        ))
    }

    /// List durable inbox rows in generated sequence order.
    async fn list_runtime_delivery_records(
        &self,
        runtime_id: &LogicalRuntimeId,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<RuntimeDeliveryStoreRecord>, RuntimeStoreError> {
        let _ = (runtime_id, after_sequence, limit);
        Err(RuntimeStoreError::Unsupported(
            "list_runtime_delivery_records".into(),
        ))
    }

    /// Persist the runtime-owned OAuth login-flow payload snapshot.
    ///
    /// The AuthMachine owns admission/consume semantics; this payload snapshot
    /// carries the PKCE verifier and device-code correlation data needed to
    /// rehydrate active flows after a persistent runtime process restart.
    fn persist_auth_oauth_flow_snapshot(
        &self,
        snapshot_json: &[u8],
    ) -> Result<(), RuntimeStoreError> {
        let _ = snapshot_json;
        Err(RuntimeStoreError::Unsupported(
            "persist_auth_oauth_flow_snapshot".into(),
        ))
    }

    /// Load the runtime-owned OAuth login-flow payload snapshot, if present.
    fn load_auth_oauth_flow_snapshot(&self) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "load_auth_oauth_flow_snapshot".into(),
        ))
    }

    /// Atomically update the runtime-owned OAuth login-flow payload snapshot.
    ///
    /// Stores that support OAuth snapshots must override this with a lock,
    /// transaction, or compare-and-swap boundary. A load/compute/persist
    /// fallback is not safe for admission, capacity, or consume claims.
    fn update_auth_oauth_flow_snapshot(
        &self,
        _update: &mut AuthOAuthFlowSnapshotUpdate<'_>,
    ) -> Result<(), RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "update_auth_oauth_flow_snapshot".into(),
        ))
    }

    /// Atomically persist a session snapshot that is not a run boundary.
    ///
    /// Session-control snapshots update durable session authority without
    /// producing a [`RunBoundaryReceipt`].
    async fn commit_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SerializedSessionSnapshot,
    ) -> Result<(), RuntimeStoreError>;

    /// Commit one valid-by-construction WholeBlob transcript-rewrite boundary.
    ///
    /// Implementations compare the exact current authority with
    /// `boundary.expected_authority()` and write the already-materialized
    /// successor bytes once. If the exact successor authority is already
    /// current, return it without another physical write. Any other current
    /// authority conflicts. Stores must not decode the Session or reconstruct
    /// rewrite semantics; those proofs are sealed before this mechanical CAS.
    /// Exact successor compaction intents must match already-committed,
    /// non-finalized outbox rows inside the same lock or transaction.
    async fn commit_prepared_whole_blob_rewrite_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        boundary: PreparedWholeBlobRewriteStoreParts,
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError>;

    /// Atomically persist session delta + receipt + input state updates.
    ///
    /// All writes MUST commit in a single atomic operation.
    /// If `session_store_key` is `Some`, validates that the snapshot belongs
    /// to that session and, for stores that physically share a `SessionStore`
    /// table, writes that table in the same transaction. Runtime snapshot
    /// authority remains keyed only by `runtime_id`; `session_store_key` must
    /// not create a raw session UUID runtime alias.
    /// Compaction intents must be inserted as pending outbox rows in this same
    /// boundary. An intent whose exact outbox identity is already finalized is
    /// a stale snapshot replay and must be rejected without mutating any part
    /// of the boundary.
    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<SerializedSessionSnapshot>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<(), RuntimeStoreError>;

    /// Load exact compaction projection intents committed by atomic_apply but
    /// not yet acknowledged as finalized by the memory store.
    async fn load_pending_compaction_projections(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeStoreError> {
        let _ = runtime_id;
        Err(RuntimeStoreError::Unsupported(
            "load_pending_compaction_projections".to_string(),
        ))
    }

    /// Idempotently acknowledge post-commit memory finalization.
    ///
    /// The acknowledgement and removal of this exact intent from the
    /// authoritative persisted session snapshot MUST occur in one atomic
    /// boundary. The finalized outbox row remains as a tombstone so later
    /// snapshot writes can reject stale metadata replay.
    async fn mark_compaction_projection_finalized(
        &self,
        runtime_id: &LogicalRuntimeId,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), RuntimeStoreError> {
        let _ = (runtime_id, projection);
        Err(RuntimeStoreError::Unsupported(
            "mark_compaction_projection_finalized".to_string(),
        ))
    }

    /// Atomically persist a failed-but-applied runtime turn.
    ///
    /// This is the machine-terminal counterpart to [`Self::atomic_apply`]:
    /// the mutated session snapshot, boundary receipt, generated machine
    /// lifecycle record, and input/outbox state must become visible in one
    /// transaction. Implementations must never compose this from separate
    /// `atomic_apply` and `commit_machine_lifecycle` calls.
    async fn atomic_apply_with_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SerializedSessionSnapshot,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<(), RuntimeStoreError> {
        let _ = (
            runtime_id,
            session_delta,
            receipt,
            machine_lifecycle,
            input_updates,
            session_store_key,
        );
        Err(RuntimeStoreError::Unsupported(
            "atomic_apply_with_machine_lifecycle".to_string(),
        ))
    }

    /// Load all input states for a runtime, one row outcome per stored row.
    ///
    /// A row whose persisted bytes no longer decode under this binary's
    /// contract is surfaced as [`InputStateRow::Corrupt`] instead of failing
    /// the whole load: one damaged row must not make every other durable
    /// input unreadable. The store never drops or rewrites the damaged row;
    /// the caller owns the per-row skip/fail policy
    /// ([`RuntimeStore::load_input_states_strict`] is the fail-on-any
    /// projection).
    async fn load_input_states(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<InputStateRow>, RuntimeStoreError>;

    /// Strict projection of [`RuntimeStore::load_input_states`]: every row
    /// must decode; the first corrupt row fails the whole load with its row
    /// identity in the typed error.
    async fn load_input_states_strict(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<StoredInputState>, RuntimeStoreError> {
        let mut states = Vec::new();
        for row in self.load_input_states(runtime_id).await? {
            match row {
                InputStateRow::Decoded(state) => states.push(*state),
                InputStateRow::Corrupt { input_id, detail } => {
                    return Err(RuntimeStoreError::ReadFailed(format!(
                        "input state row `{input_id}` failed to decode: {detail}"
                    )));
                }
            }
        }
        Ok(states)
    }

    /// Load a specific boundary receipt.
    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError>;

    /// Load every durably committed boundary receipt for one run, in
    /// ascending sequence order.
    ///
    /// Recovery reads these to (a) derive the next boundary sequence for a
    /// recovered commit (an interrupted tool loop can already have committed
    /// `BoundaryContinue` receipts before losing only its final boundary)
    /// and (b) recover the exact contributing input identities the run
    /// already bound durably. The default probes ascending sequences through
    /// [`RuntimeStore::load_boundary_receipt`]; backends with range reads
    /// should override it.
    async fn load_committed_boundary_receipts(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
    ) -> Result<Vec<RunBoundaryReceipt>, RuntimeStoreError> {
        // Receipt sequences are minted densely from 1 by the generated
        // machine; a gap therefore terminates the probe. The cap is a
        // corruption backstop, far above any real per-run boundary count.
        const PROBE_CAP: u64 = 100_000;
        let mut receipts = Vec::new();
        for sequence in 1..=PROBE_CAP {
            match self
                .load_boundary_receipt(runtime_id, run_id, sequence)
                .await?
            {
                Some(receipt) => receipts.push(receipt),
                None => return Ok(receipts),
            }
        }
        Err(RuntimeStoreError::ReadFailed(format!(
            "run {run_id} has more than {PROBE_CAP} boundary receipts; refusing to probe further"
        )))
    }

    /// Load one authoritative snapshot of all and only nonterminal input-state
    /// rows, with the exact domain-prefixed SHA-256 digest of each row's
    /// stored bytes and a set/absence token over the complete ordered set.
    ///
    /// Each row digest is a target-local compare token: recovery carries it
    /// back on fenced [`InputStatePersistenceRecord`]s. The snapshot's set
    /// token additionally fences inserts, removals, and terminality changes,
    /// including the empty-set case where there are no per-row tokens to CAS.
    ///
    /// Implementations that apply recovery MUST recompute the snapshot from
    /// the same complete, runtime-scoped nonterminal index/set inside the
    /// transaction that writes the boundary. A different set token fails the
    /// whole boundary with [`RuntimeStoreError::RecoveryInputSetConflict`];
    /// implementations MUST also enforce every
    /// [`InputStatePersistenceRecord::expected_row_digest`] in that
    /// transaction, failing with
    /// [`RuntimeStoreError::InputRowVersionConflict`] on mismatch.
    ///
    /// There is deliberately no compatibility derivation from decoded rows:
    /// reserializing a bundle proves only the current serializer's canonical
    /// representation, not the exact bytes the backend observed and will CAS.
    /// A backend must override this method only when it can return and enforce
    /// tokens for its actual stored-row representation. Wrappers and custom
    /// stores that cannot do so fail closed, and durable-tail recovery maps
    /// this typed absence of fencing capability to `Unfenceable`.
    async fn load_input_states_with_versions(
        &self,
        _runtime_id: &LogicalRuntimeId,
    ) -> Result<PreparedRecoveryInputSnapshot, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "load_input_states_with_versions requires exact stored-row and complete-set tokens"
                .to_string(),
        ))
    }

    /// Load the latest committed whole-blob session snapshot for a runtime.
    ///
    /// Compatibility-only. A head-canonical implementation must return
    /// [`RuntimeStoreError::SessionPersistenceAuthorityConflict`] once a
    /// canonical authority row exists; returning a frozen migration BLOB would
    /// resurrect its predecessor as current truth.
    async fn load_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<std::sync::Arc<Vec<u8>>>, RuntimeStoreError>;

    /// Remove the latest committed session snapshot for a runtime.
    ///
    /// This is used only as a fail-closed quarantine path when transcript
    /// rewrite audit failure makes the runtime snapshot itself invalid recovery
    /// authority and the service cannot restore the previous snapshot. An
    /// ordinary downstream compatibility-projection failure must retain the
    /// already-committed runtime snapshot for retry.
    /// Head-canonical stores must refuse this whole-document mutation.
    async fn clear_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError>;

    /// Replace the latest committed session snapshot only if it still matches
    /// `expected_current`.
    ///
    /// Used by fail-closed recovery when a rejected transcript-rewrite snapshot
    /// must be restored to its prior audited value. Implementations must compare
    /// and write atomically so recovery cannot overwrite newer runtime authority.
    /// Head-canonical stores must refuse this whole-document mutation.
    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, RuntimeStoreError>;

    /// Remove the latest committed session snapshot only if it still matches
    /// `expected_current`.
    ///
    /// This is the conditional variant of the fail-closed quarantine path.
    /// Head-canonical stores must refuse this whole-document mutation.
    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, RuntimeStoreError>;

    /// Report whether the runtime-projection fallback for `runtime_id` is
    /// quarantined.
    ///
    /// This is a durable single-owner fact: when
    /// [`clear_session_snapshot_if_current`](Self::clear_session_snapshot_if_current)
    /// matches and DELETEs a rejected runtime snapshot, the same atomic boundary
    /// records the quarantine marker. A subsequent live snapshot write clears it.
    /// Recovery reads this to decide whether a store-only projection may stand in
    /// for an absent runtime snapshot. The default is fail-safe (`false`): stores
    /// that cannot record the marker durably never claim a snapshot is
    /// quarantined.
    async fn is_runtime_projection_quarantined(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<bool, RuntimeStoreError> {
        let _ = runtime_id;
        Ok(false)
    }

    /// Persist a single input state (for durable-before-ack).
    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &InputStatePersistenceRecord,
    ) -> Result<(), RuntimeStoreError>;

    /// Atomically persist a batch of machine-authorized input shell updates.
    /// Used by per-input terminal outboxes so an N-input batch can never
    /// expose a mixed provisional/finalized or finalized/published phase.
    /// Idempotency-key ownership follows the trait's complete-final-image
    /// contract, including valid swaps between rows in this batch.
    async fn persist_input_states_atomically(
        &self,
        _runtime_id: &LogicalRuntimeId,
        states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        if states.is_empty() {
            return Ok(());
        }
        Err(RuntimeStoreError::Unsupported(
            "persist_input_states_atomically".to_string(),
        ))
    }

    /// Durable realization profile for
    /// [`Self::compare_and_swap_input_states_atomically`].
    fn input_state_batch_cas_implementation_profile(
        &self,
    ) -> InputStateBatchCasImplementationProfile {
        InputStateBatchCasImplementationProfile::Unsupported
    }

    /// Atomically replace an exact set of input-state rows only when every
    /// currently persisted row is byte-identical to its expected
    /// [`StoredInputState`] serialization.
    ///
    /// Expected and replacement batches must contain the same unique keys and
    /// at most [`MAX_INPUT_STATE_BATCH_CAS`] rows. If every current row already
    /// equals its replacement, implementations return
    /// [`InputStateBatchCasOutcome::Swapped`] without rewriting it; this makes
    /// a committed store-first transaction retryable after caller
    /// cancellation or acknowledgement loss. Missing rows, mixed
    /// expected/replacement images, and any other changed durable rows return
    /// [`InputStateBatchCasOutcome::Stale`] without writing a replacement.
    /// Implementations must hold one lock/transaction across the complete
    /// comparison and write set. Replacement idempotency-key ownership follows
    /// the trait's complete-final-image contract, including valid swaps between
    /// rows in this batch.
    async fn compare_and_swap_input_states_atomically(
        &self,
        _runtime_id: &LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
    ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
        let prepared = prepare_input_state_batch_cas(expected, replacements)?;
        if prepared.is_empty() {
            return Ok(InputStateBatchCasOutcome::Swapped);
        }
        Err(RuntimeStoreError::Unsupported(
            "compare_and_swap_input_states_atomically".to_string(),
        ))
    }

    /// Atomically replace an exact input-state batch while an external
    /// authority fence is held across the target write.
    ///
    /// Implementations must compare the target rows first, then retain both
    /// the target transaction and the external authority guard until every
    /// replacement is committed. This is the cold-registration recovery seam:
    /// a process whose lease expires or is superseded must never overwrite
    /// input work recovered by its successor.
    async fn compare_and_swap_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
        write_fence: std::sync::Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedInputStateBatchCasOutcome, RuntimeStoreError> {
        let prepared = prepare_input_state_batch_cas(expected, replacements)?;
        if prepared.is_empty() {
            return Ok(FencedInputStateBatchCasOutcome::Swapped);
        }
        let _ = (runtime_id, write_fence);
        Err(RuntimeStoreError::Unsupported(
            "compare_and_swap_input_states_atomically_with_fence".to_string(),
        ))
    }

    /// Atomically publish machine-normalized recovery input rows only while
    /// the exact store-owned input-set revision observed with the source rows
    /// remains current.
    ///
    /// Unlike ordinary bounded input CAS, this seam has no total-row cap and
    /// MUST compare `expected_revision` even when `replacements` is empty. The
    /// empty case is the absence-fence path: a concurrent first insert must
    /// make it stale. Each replacement additionally carries the exact
    /// predecessor-row digest returned in
    /// [`Self::load_input_states_with_versions`].
    async fn compare_and_swap_recovery_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: RecoveryInputSetRevision,
        mutations: &[RecoveryInputStateMutation],
    ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
        let _ = prepare_recovery_input_state_mutations(mutations)?;
        let _ = (runtime_id, expected_revision);
        Err(RuntimeStoreError::Unsupported(
            "compare_and_swap_recovery_input_states_atomically".to_string(),
        ))
    }

    /// Revision-fenced recovery input publication while an external runtime
    /// authority fence is held across the target transaction.
    ///
    /// Implementations MUST execute the external fence even for an empty
    /// replacement set, after comparing the store-owned revision and before
    /// committing. This prevents a zero-row bootstrap from bypassing either
    /// the absence witness or its runtime-authority lease.
    async fn compare_and_swap_recovery_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: RecoveryInputSetRevision,
        mutations: &[RecoveryInputStateMutation],
        write_fence: std::sync::Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedInputStateBatchCasOutcome, RuntimeStoreError> {
        let _ = prepare_recovery_input_state_mutations(mutations)?;
        let _ = (runtime_id, expected_revision, write_fence);
        Err(RuntimeStoreError::Unsupported(
            "compare_and_swap_recovery_input_states_atomically_with_fence".to_string(),
        ))
    }

    /// Load a single input state.
    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeStoreError>;

    /// Resolve one historical or live input through the store-owned
    /// idempotency-key index.
    ///
    /// Implementations MUST maintain a unique `(runtime_id, key) -> input_id`
    /// mapping atomically with every input-row insert, update, and delete.
    /// Presence and absence are authoritative only when the store proves, in
    /// the same backend snapshot as the keyed lookup, that every source row for
    /// the runtime has an unambiguous indexable key identity. A corrupt or
    /// otherwise unindexable row must return
    /// [`RuntimeStoreError::InputIdempotencyIndexUncertain`] for both hits and
    /// misses; it must never be treated as absence or ignored behind another
    /// indexed owner. A full input-history scan is not a conforming
    /// implementation. The returned digest binds the exact stored row bytes
    /// observed with the index lookup.
    async fn load_input_state_by_idempotency_key(
        &self,
        _runtime_id: &LogicalRuntimeId,
        _key: &IdempotencyKey,
    ) -> Result<Option<ExactInputStateObservation>, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "load_input_state_by_idempotency_key requires an exact maintained index".to_string(),
        ))
    }

    /// Load an exact bounded set of input rows from one backend snapshot.
    ///
    /// Results have exactly the request's cardinality and order; a missing key
    /// occupies its corresponding `None` slot. Duplicate keys and batches
    /// larger than [`MAX_INPUT_STATE_BATCH_CAS`] are rejected. Implementations
    /// must perform one bounded backend read rather than repeatedly
    /// materializing a whole-blob ledger.
    async fn load_input_states_by_ids(
        &self,
        _runtime_id: &LogicalRuntimeId,
        input_ids: &[InputId],
    ) -> Result<Vec<Option<StoredInputState>>, RuntimeStoreError> {
        validate_input_state_batch_read_ids(input_ids)?;
        if input_ids.is_empty() {
            return Ok(Vec::new());
        }
        Err(RuntimeStoreError::Unsupported(
            "load_input_states_by_ids".to_string(),
        ))
    }

    /// Discover canonical owner ids for unfinished terminal work.
    ///
    /// Results are strictly ordered by [`InputId`], contain only ids greater
    /// than the stable exclusive `after` cursor, and contain at most `limit`
    /// entries. Implementations must maintain a store-owned index
    /// transactionally with input-state writes; scanning or decoding the
    /// accumulated input ledger inside this method violates the contract.
    ///
    /// The result is discovery only. Callers must hydrate and validate each
    /// owner's exact declared recipient batch through
    /// [`Self::load_input_states_by_ids`].
    async fn load_pending_terminal_owner_ids_page(
        &self,
        _runtime_id: &LogicalRuntimeId,
        after: Option<&InputId>,
        limit: usize,
    ) -> Result<Vec<InputId>, RuntimeStoreError> {
        validate_pending_terminal_owner_page(after, limit, &[])?;
        Err(RuntimeStoreError::Unsupported(
            "load_pending_terminal_owner_ids_page".to_string(),
        ))
    }

    /// Observe one physical machine-lifecycle row without collapsing corrupt
    /// or future-version bytes into absence.
    ///
    /// This is the recovery/reconciliation read surface. Custom stores must
    /// implement it explicitly; the default is capability-unavailable rather
    /// than inferring a total observation from the older strict decoder.
    async fn observe_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<MachineLifecycleObservation, RuntimeStoreError> {
        let _ = runtime_id;
        Err(RuntimeStoreError::Unsupported(
            "observe_machine_lifecycle".to_string(),
        ))
    }

    /// Replace exactly one machine-lifecycle row when it is absent or still
    /// has the observed raw-content version.
    ///
    /// Built-in stores atomically compare the raw-content version and publish
    /// the machine-authorized replacement. Binding, generation, fence, and
    /// run atoms belong to the dead process that wrote the observed row; they
    /// are not durable high-waters and may be cleared by an exact-version
    /// cold-normalization CAS. The caller retains the prior raw digest for
    /// output-only diagnostics. Conflicts are ordinary level-triggered
    /// re-observation; unsupported or malformed bytes return
    /// [`RuntimeStoreError::MachineLifecycleRepairBlocked`].
    ///
    /// When a runtime session catalog entry already exists, an applied CAS
    /// must advance its runtime-state projection in the same atomic operation.
    async fn compare_and_swap_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: MachineLifecycleExpectedVersion,
        replacement: MachineLifecycleCommit,
    ) -> Result<MachineLifecycleCasOutcome, RuntimeStoreError> {
        let _ = (runtime_id, expected, replacement);
        Err(RuntimeStoreError::Unsupported(
            "compare_and_swap_machine_lifecycle".to_string(),
        ))
    }

    /// Replace exactly one lifecycle row while an external authority fence is
    /// held across the target write.
    ///
    /// This is the conditional-registration store seam. Built-in stores call
    /// `write_fence` inside the row lock/transaction after the exact raw-row
    /// comparison and immediately before publication. Custom stores must opt
    /// in explicitly; the default is capability-unavailable rather than an
    /// unfenced fallback to compare_and_swap_machine_lifecycle. An applied
    /// fence must also advance an existing runtime session catalog entry to
    /// the replacement state in that same operation. This includes an
    /// already-exact lifecycle row: the applied fence heals a stale catalog
    /// projection before returning [`FencedMachineLifecycleCasOutcome::AlreadyExact`].
    async fn compare_and_swap_machine_lifecycle_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: MachineLifecycleExpectedVersion,
        replacement: MachineLifecycleCommit,
        write_fence: std::sync::Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedMachineLifecycleCasOutcome, RuntimeStoreError> {
        let _ = (runtime_id, expected, replacement, write_fence);
        Err(RuntimeStoreError::Unsupported(
            "compare_and_swap_machine_lifecycle_with_fence".to_string(),
        ))
    }

    /// Load the last persisted machine lifecycle record bytes, if any.
    ///
    /// Implementations return only the opaque bytes previously obtained from
    /// [`MachineLifecycleCommit::store_record`]. The runtime crate decodes
    /// these bytes through `load_runtime_state` or internal recovery helpers;
    /// stores must not promote compatibility rows or bare runtime states into
    /// lifecycle authority.
    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError>;

    /// Atomically commit machine-owned lifecycle state changes.
    ///
    /// Writes runtime state, generated runtime binding facts, and all input
    /// state updates in a single atomic operation. `MachineLifecycleCommit` has
    /// no public constructor, so this cannot be used by compatibility callers
    /// to pick runtime truth. If a runtime session catalog entry exists, its
    /// runtime-state projection must advance to [`MachineLifecycleCommit::runtime_state`]
    /// in the same operation.
    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: MachineLifecycleCommit,
        input_states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError>;

    /// Atomically publish final unregister lifecycle truth and retire the
    /// matching ops-lifecycle epoch.
    ///
    /// The lifecycle record, input-state updates, and ops snapshot deletion
    /// MUST commit in one store transaction (or one indivisible in-memory
    /// critical section). A terminal lifecycle record with the old ops epoch
    /// still present is forbidden: recovery would otherwise resurrect stale
    /// operation/cursor authority after unregister. The commit also carries
    /// the exact retired ops epoch; implementations MUST atomically retain a
    /// durable deletion-wins fence for it, and every later
    /// `persist_ops_lifecycle` for that epoch must return
    /// [`RuntimeStoreError::OpsLifecycleEpochRetired`] rather than recreate the
    /// row. Implementations must also be idempotent so retry after a process
    /// crash following commit converges on the same terminal lifecycle with no
    /// ops snapshot and the same epoch fence.
    ///
    /// `Ok(())` means the whole finalization is visible. Every error except
    /// [`RuntimeStoreError::UnregisterFinalizationOutcomeUnknown`] MUST mean
    /// none of it is visible. A backend with an ambiguous commit
    /// acknowledgement must first resolve that ambiguity internally by
    /// reading its transaction authority. It may use the typed unknown error
    /// only when it cannot prove either the exact final state or the exact
    /// pre-transaction state; callers then retry without a durable rollback.
    /// The opaque token also proves the generated `DeleteSnapshot` verdict and
    /// bundles the exact lifecycle and input rows selected by the machine.
    ///
    /// The returned future is also a cancellation boundary: after it is
    /// dropped, no mutation from that invocation may become visible later.
    /// An implementation may leave the prior pair untouched or finish the
    /// entire atomic commit before cancellation is observable, but it must not
    /// detach a background write that can cross a same-runtime-ID replacement.
    /// If a runtime session catalog entry exists, its runtime-state projection
    /// is part of this same atomic finalization and is selected from
    /// [`UnregisterFinalizationCommit::lifecycle_store_record`].
    async fn commit_unregister_finalization(
        &self,
        runtime_id: &LogicalRuntimeId,
        finalization: UnregisterFinalizationCommit,
    ) -> Result<(), RuntimeStoreError> {
        let _ = (runtime_id, finalization);
        Err(RuntimeStoreError::Unsupported(
            "commit_unregister_finalization".into(),
        ))
    }

    /// Atomically initialize the ops lifecycle row if it is absent and return
    /// the canonical durable snapshot.
    ///
    /// The absence check, optional insert, and canonical read MUST share one
    /// store transaction (or one indivisible in-memory critical section).
    /// Concurrent initializer calls for the same runtime must therefore all
    /// observe the same epoch: exactly one candidate may become durable and
    /// every losing caller receives that winner's snapshot. The machine's
    /// stable registration transaction separately spans this store call
    /// through map publication/removal; this method is not a distributed
    /// machine lease. Implementations must also reject a candidate whose epoch
    /// is already covered by the unregister deletion-wins fence.
    ///
    /// Cancellation may leave the candidate as the canonical empty row: no
    /// bindings escape before this await completes, and the next registrar
    /// adopts the returned durable epoch. A cancelled invocation must never
    /// overwrite a row that was already present.
    ///
    /// There is intentionally no load-then-persist default. Custom stores
    /// that support durable ops lifecycle state must implement this atomic
    /// boundary or fail closed with [`RuntimeStoreError::Unsupported`].
    async fn initialize_ops_lifecycle_if_absent(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate: &crate::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<crate::ops_lifecycle::PersistedOpsSnapshot, RuntimeStoreError> {
        let _ = (runtime_id, candidate);
        Err(RuntimeStoreError::Unsupported(
            "initialize_ops_lifecycle_if_absent".into(),
        ))
    }

    /// Persist a snapshot of the ops lifecycle registry state.
    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        snapshot: &crate::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        let _ = (runtime_id, snapshot);
        Err(RuntimeStoreError::Unsupported(
            "persist_ops_lifecycle".into(),
        ))
    }

    /// Load a previously persisted ops lifecycle snapshot.
    async fn load_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<crate::ops_lifecycle::PersistedOpsSnapshot>, RuntimeStoreError> {
        let _ = runtime_id;
        Err(RuntimeStoreError::Unsupported("load_ops_lifecycle".into()))
    }

    /// Delete a previously persisted ops lifecycle snapshot.
    async fn delete_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        let _ = runtime_id;
        Err(RuntimeStoreError::Unsupported(
            "delete_ops_lifecycle".into(),
        ))
    }

    // -----------------------------------------------------------------------
    // Direct peer-member semantic high-water
    // -----------------------------------------------------------------------

    /// Atomically admit one semantic incarnation or return the current durable
    /// high-water for this exact member session.
    ///
    /// Implementations must compare and update inside one transaction. The
    /// returned value is always the canonical durable high-water after the
    /// operation. No runtime/session bearer token is stored at this boundary.
    async fn admit_direct_member_incarnation_high_water(
        &self,
        member_session_id: &str,
        candidate: &meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation,
    ) -> Result<
        meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation,
        RuntimeStoreError,
    > {
        let _ = (member_session_id, candidate);
        Err(RuntimeStoreError::Unsupported(
            "admit_direct_member_incarnation_high_water".into(),
        ))
    }

    // -----------------------------------------------------------------------
    // Mob host binding rows (`runtime_mob_host_bindings`, multi-host mobs R8)
    // -----------------------------------------------------------------------
    //
    // Raw record-JSON accessors only: the TYPED record and the
    // transition-derived persistence authorities live mob-side
    // (`meerkat-mob/src/runtime/host_actor.rs`); this store never interprets
    // the blob. CAS compares the full serialized record, mirroring the
    // `mob_runtime_supervisors` mechanics.

    /// Load the persisted host-binding record blob for `mob_id`, if any.
    async fn load_mob_host_binding(
        &self,
        mob_id: &str,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        let _ = mob_id;
        Err(RuntimeStoreError::Unsupported(
            "load_mob_host_binding".into(),
        ))
    }

    /// List every persisted host-binding row (boot recovery).
    async fn list_mob_host_bindings(&self) -> Result<Vec<(String, Vec<u8>)>, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "list_mob_host_bindings".into(),
        ))
    }

    /// Insert the host-binding row for `mob_id` iff absent. Returns whether
    /// the row was inserted.
    async fn put_mob_host_binding_if_absent(
        &self,
        mob_id: &str,
        record_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let _ = (mob_id, record_json);
        Err(RuntimeStoreError::Unsupported(
            "put_mob_host_binding_if_absent".into(),
        ))
    }

    /// Replace the host-binding row for `mob_id` iff the stored blob equals
    /// `expected_json`. Returns whether the swap applied.
    async fn compare_and_put_mob_host_binding(
        &self,
        mob_id: &str,
        expected_json: &[u8],
        next_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let _ = (mob_id, expected_json, next_json);
        Err(RuntimeStoreError::Unsupported(
            "compare_and_put_mob_host_binding".into(),
        ))
    }

    /// Delete the host-binding row for `mob_id` iff the stored blob equals
    /// `expected_json`. Returns whether a row was deleted.
    async fn delete_mob_host_binding(
        &self,
        mob_id: &str,
        expected_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let _ = (mob_id, expected_json);
        Err(RuntimeStoreError::Unsupported(
            "delete_mob_host_binding".into(),
        ))
    }

    /// Load the durable receipt for an already-completed host revocation.
    ///
    /// The blob is deliberately separate from `runtime_mob_host_bindings`:
    /// boot recovery must never mistake a revoke retry receipt for a live
    /// binding or revive the materialized-member rows that the revoke
    /// removed. The typed receipt and its transition witness live mob-side;
    /// this store treats it as opaque bytes.
    async fn load_mob_host_revocation(
        &self,
        mob_id: &str,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        let _ = mob_id;
        Err(RuntimeStoreError::Unsupported(
            "load_mob_host_revocation".into(),
        ))
    }

    /// List durable host-revocation receipts for boot recovery of exact
    /// reply-loss retries. Receipts are not bindings and carry no member
    /// revival rows.
    async fn list_mob_host_revocations(&self) -> Result<Vec<(String, Vec<u8>)>, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "list_mob_host_revocations".into(),
        ))
    }

    /// Atomically delete the expected active binding and publish its revoke
    /// receipt. Returns `false` when the expected binding did not match; in
    /// that case neither write is visible.
    ///
    /// This is the durable terminal boundary for host revocation. A crash
    /// before it leaves the binding retryable; a crash after it leaves no
    /// binding/member rows to revive and an exact receipt to replay.
    async fn revoke_mob_host_binding(
        &self,
        mob_id: &str,
        expected_binding_json: &[u8],
        receipt_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let _ = (mob_id, expected_binding_json, receipt_json);
        Err(RuntimeStoreError::Unsupported(
            "revoke_mob_host_binding".into(),
        ))
    }
}

pub use memory::InMemoryRuntimeStore;
#[cfg(feature = "sqlite-store")]
pub use sqlite::SqliteRuntimeStore;

#[cfg(test)]
mod store_authority_record_tests {
    use super::*;
    use meerkat_core::session_store::PreparedHeadCanonicalMutation;
    use meerkat_core::types::{Message, UserMessage};

    fn row_digest(byte: char) -> String {
        format!("row-sha256:{}", byte.to_string().repeat(64))
    }

    fn head_record() -> (
        meerkat_core::types::SessionId,
        meerkat_core::session_store::SessionHead,
        String,
    ) {
        let mut session = meerkat_core::Session::new();
        session.push(Message::User(UserMessage::text("canonical head")));
        let session_id = session.id().clone();
        let mutation =
            PreparedHeadCanonicalMutation::prepare(&session, None).expect("prepare canonical head");
        (
            session_id,
            mutation.successor_head().clone(),
            mutation.successor_head_token().to_string(),
        )
    }

    #[test]
    fn borrowed_whole_blob_provisional_prepare_encodes_once_and_retains_no_session() {
        let mut session = meerkat_core::Session::new();
        session.push(Message::User(UserMessage::text("candidate")));
        let session_id = session.id().clone();
        let base = WholeBlobStoreAuthority::issued(session_id.clone(), 7, row_digest('b'))
            .expect("valid base authority");
        let prepared =
            PreparedWholeBlobProvisionalTail::prepare_from_session(base, RunId::new(), 1, &session)
                .expect("prepare borrowed WholeBlob candidate");
        assert_eq!(
            prepared.whole_blob_encode_count(),
            1,
            "borrowed preparation must stream the Session exactly once"
        );
        let retained = prepared.clone();
        drop(prepared);
        drop(session);
        assert_eq!(
            retained.whole_blob_encode_count(),
            1,
            "cloning the bounded carrier must share bytes, not re-encode"
        );

        let (authority, artifact, digest, message_count, catalog, intents) = retained.into_parts();
        assert_eq!(authority.session_id(), &session_id);
        assert_eq!(
            authority.candidate_blob_sha256(),
            artifact.row_sha256_token()
        );
        assert_eq!(catalog.session_id(), &session_id);
        assert_eq!(catalog.message_count(), 1);
        assert_eq!(message_count, 1);
        assert!(!digest.is_empty());
        assert!(intents.is_empty());
        let decoded = meerkat_core::Session::from_persisted_bytes(artifact.bytes())
            .expect("carrier bytes remain independently usable after the Session is dropped");
        assert_eq!(decoded.id(), &session_id);
        assert_eq!(decoded.messages().len(), 1);
    }

    #[test]
    fn committed_whole_blob_decode_installs_store_owned_rewrite_lineage() {
        let mut session = meerkat_core::Session::new();
        session.push(Message::User(UserMessage::text("original")));
        let artifact = session
            .to_persisted_artifact()
            .expect("serialize WholeBlob document");
        let authority = WholeBlobStoreAuthority::issued(
            session.id().clone(),
            1,
            artifact.row_sha256_token().to_string(),
        )
        .expect("issue exact WholeBlob authority");
        let committed = CommittedWholeBlobSnapshot::new(artifact.bytes_arc(), authority)
            .expect("decode store-owned WholeBlob document");

        let mut decoded = committed.session().clone();
        let parent_revision = decoded.transcript_revision().expect("read parent revision");
        decoded
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("edited"))],
                meerkat_core::TranscriptRewriteReason::new("test"),
                Some("runtime-store-test".to_string()),
                Some(parent_revision),
            )
            .expect("store-owned WholeBlob decode must carry exact rewrite lineage");
    }

    #[test]
    fn whole_blob_store_record_constructor_validates_every_fixed_field() {
        let session_id = meerkat_core::types::SessionId::new();
        let valid = WholeBlobStoreAuthority::from_store_record(
            WholeBlobStoreAuthority::VERSION,
            session_id.clone(),
            7,
            row_digest('a'),
        )
        .expect("valid WholeBlob record");
        assert_eq!(valid.authority_version(), WholeBlobStoreAuthority::VERSION);
        assert_eq!(valid.session_id(), &session_id);
        assert_eq!(valid.store_revision(), 7);
        assert_eq!(valid.blob_sha256(), row_digest('a'));

        for (version, revision, digest) in [
            (WholeBlobStoreAuthority::VERSION + 1, 7, row_digest('a')),
            (WholeBlobStoreAuthority::VERSION, 0, row_digest('a')),
            (WholeBlobStoreAuthority::VERSION, 7, String::new()),
            (
                WholeBlobStoreAuthority::VERSION,
                7,
                format!("sha256:{}", "a".repeat(64)),
            ),
            (WholeBlobStoreAuthority::VERSION, 7, row_digest('A')),
            (
                WholeBlobStoreAuthority::VERSION,
                7,
                format!("row-sha256:{}", "a".repeat(63)),
            ),
        ] {
            assert!(
                WholeBlobStoreAuthority::from_store_record(
                    version,
                    session_id.clone(),
                    revision,
                    digest,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn head_canonical_store_record_constructor_validates_every_bound_fact() {
        let (session_id, head, token) = head_record();
        let valid = HeadCanonicalStoreAuthority::from_store_record(
            HeadCanonicalStoreAuthority::VERSION,
            session_id.clone(),
            11,
            head.clone(),
            token.clone(),
        )
        .expect("valid HeadCanonical record");
        assert_eq!(
            valid.authority_version(),
            HeadCanonicalStoreAuthority::VERSION
        );
        assert_eq!(valid.session_id(), &session_id);
        assert_eq!(valid.store_revision(), 11);
        assert_eq!(valid.boundary_head(), &head);
        assert_eq!(valid.committed_head_token(), token);

        assert!(
            HeadCanonicalStoreAuthority::from_store_record(
                HeadCanonicalStoreAuthority::VERSION + 1,
                session_id.clone(),
                11,
                head.clone(),
                token.clone(),
            )
            .is_err()
        );
        assert!(
            HeadCanonicalStoreAuthority::from_store_record(
                HeadCanonicalStoreAuthority::VERSION,
                session_id.clone(),
                0,
                head.clone(),
                token.clone(),
            )
            .is_err()
        );
        assert!(
            HeadCanonicalStoreAuthority::from_store_record(
                HeadCanonicalStoreAuthority::VERSION,
                session_id.clone(),
                11,
                head.clone(),
                String::new(),
            )
            .is_err()
        );
        assert!(
            HeadCanonicalStoreAuthority::from_store_record(
                HeadCanonicalStoreAuthority::VERSION,
                session_id.clone(),
                11,
                head.clone(),
                "head-cas:different".to_string(),
            )
            .is_err()
        );

        let mut wrong_session = head.clone();
        wrong_session.id = meerkat_core::types::SessionId::new();
        assert!(
            HeadCanonicalStoreAuthority::from_store_record(
                HeadCanonicalStoreAuthority::VERSION,
                session_id.clone(),
                11,
                wrong_session,
                token.clone(),
            )
            .is_err()
        );

        let mut missing_row_prefix = head.clone();
        missing_row_prefix.message_row_prefix = None;
        assert!(
            HeadCanonicalStoreAuthority::from_store_record(
                HeadCanonicalStoreAuthority::VERSION,
                session_id.clone(),
                11,
                missing_row_prefix,
                token.clone(),
            )
            .is_err()
        );

        let mut wrong_row_count = head.clone();
        wrong_row_count.message_count = wrong_row_count.message_count.saturating_add(1);
        assert!(
            HeadCanonicalStoreAuthority::from_store_record(
                HeadCanonicalStoreAuthority::VERSION,
                session_id.clone(),
                11,
                wrong_row_count,
                token.clone(),
            )
            .is_err()
        );

        let mut wrong_rewrite_count = head;
        wrong_rewrite_count.rewrite_count = wrong_rewrite_count.rewrite_count.saturating_add(1);
        assert!(
            HeadCanonicalStoreAuthority::from_store_record(
                HeadCanonicalStoreAuthority::VERSION,
                session_id,
                11,
                wrong_rewrite_count,
                token,
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod runtime_session_catalog_entry_tests {
    use super::*;
    use meerkat_core::session_store::PreparedHeadCanonicalMutation;
    use meerkat_core::types::{Message, UserMessage};

    fn labeled_session() -> meerkat_core::Session {
        let mut session = meerkat_core::Session::new();
        session.push(Message::User(UserMessage::text(
            "transcript body must not enter the catalog",
        )));
        session.set_metadata(
            RuntimeSessionCatalogEntry::SESSION_LABELS_KEY,
            serde_json::json!({
                "owner": "operations",
                "tier": "production"
            }),
        );
        session
    }

    #[test]
    fn public_session_projection_is_validated_and_body_free() {
        let session = labeled_session();
        let entry = RuntimeSessionCatalogEntry::from_session(
            &session,
            RuntimeSessionPersistenceProfile::WholeBlobV1,
            Some(RuntimeState::Idle),
        )
        .expect("typed Session projects to bounded catalog metadata");

        assert_eq!(entry.session_id(), session.id());
        assert_eq!(
            entry.persistence_profile(),
            RuntimeSessionPersistenceProfile::WholeBlobV1
        );
        assert_eq!(entry.created_at(), session.created_at());
        assert_eq!(entry.updated_at(), session.updated_at());
        assert_eq!(entry.message_count(), session.messages().len());
        assert_eq!(entry.total_tokens(), session.total_tokens());
        assert_eq!(
            entry.labels(),
            &BTreeMap::from([
                ("owner".to_string(), "operations".to_string()),
                ("tier".to_string(), "production".to_string()),
            ])
        );
        assert_eq!(entry.runtime_state(), Some(RuntimeState::Idle));

        let encoded = serde_json::to_string(&entry).expect("catalog entry serializes");
        assert!(
            !encoded.contains("transcript body must not enter the catalog"),
            "catalog projection must never carry transcript body data"
        );
    }

    #[test]
    fn public_head_projection_matches_session_catalog_facts() {
        let session = labeled_session();
        let mutation =
            PreparedHeadCanonicalMutation::prepare(&session, None).expect("prepare canonical head");
        let from_session = RuntimeSessionCatalogEntry::from_session(
            &session,
            RuntimeSessionPersistenceProfile::HeadCanonicalV1,
            None,
        )
        .expect("Session catalog projection");
        let from_head = RuntimeSessionCatalogEntry::from_head(
            mutation.successor_head(),
            RuntimeSessionPersistenceProfile::HeadCanonicalV1,
            None,
        )
        .expect("SessionHead catalog projection");

        assert_eq!(from_head, from_session);
    }

    #[test]
    fn public_catalog_projections_reject_malformed_labels() {
        let mut session = labeled_session();
        session.set_metadata(
            RuntimeSessionCatalogEntry::SESSION_LABELS_KEY,
            serde_json::json!(["not", "a", "label", "map"]),
        );

        assert!(matches!(
            RuntimeSessionCatalogEntry::from_session(
                &session,
                RuntimeSessionPersistenceProfile::WholeBlobV1,
                None,
            ),
            Err(RuntimeStoreError::WriteFailed(detail))
                if detail.contains("malformed catalog labels")
        ));

        let mutation =
            PreparedHeadCanonicalMutation::prepare(&session, None).expect("prepare canonical head");
        assert!(matches!(
            RuntimeSessionCatalogEntry::from_head(
                mutation.successor_head(),
                RuntimeSessionPersistenceProfile::HeadCanonicalV1,
                None,
            ),
            Err(RuntimeStoreError::WriteFailed(detail))
                if detail.contains("malformed catalog labels")
        ));
    }
}

#[cfg(test)]
mod lifecycle_record_compatibility_tests {
    use super::*;

    fn durable_live_bridge_evidence() -> crate::live_execution::LiveBridgeRecoveryImage {
        serde_json::from_value(serde_json::json!({
            "operations": [
                {
                    "operation_id": "op-in-flight",
                    "channel_id": "channel-in-flight",
                    "interaction_id": "interaction-in-flight",
                    "provider_turn_ref": "turn-in-flight",
                    "provider_delegation_ref": "delegation-in-flight",
                    "provider_call_ref": "call-in-flight",
                    "source_agent_identity": "executor-in-flight",
                    "canonical_context_revision": "context-in-flight",
                    "request_digest": "sha256:request-in-flight",
                    "phase": "execution_running",
                    "terminal": null,
                    "result_digest": null,
                    "cancellation_reason": "restart",
                    "submission_output_kind": null,
                    "submission_digest": null,
                    "submission_state": null,
                    "current_for_channel": false,
                    "channel_revoked": true
                },
                {
                    "operation_id": "op-ambiguous",
                    "channel_id": "channel-ambiguous",
                    "interaction_id": "interaction-ambiguous",
                    "provider_turn_ref": "turn-ambiguous",
                    "provider_delegation_ref": "delegation-ambiguous",
                    "provider_call_ref": "call-ambiguous",
                    "source_agent_identity": "executor-ambiguous",
                    "canonical_context_revision": "context-ambiguous",
                    "request_digest": "sha256:request-ambiguous",
                    "phase": "execution_terminal",
                    "terminal": "completed",
                    "result_digest": "sha256:result-ambiguous",
                    "cancellation_reason": "channel_close",
                    "submission_output_kind": "success",
                    "submission_digest": "sha256:submission-ambiguous",
                    "submission_state": "submission_ambiguous",
                    "current_for_channel": false,
                    "channel_revoked": true
                }
            ]
        }))
        .expect("valid durable live bridge test image")
    }

    fn operation_id(
        value: u128,
    ) -> meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId {
        meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId::from_uuid(
            uuid::Uuid::from_u128(value),
        )
    }

    fn binding(seed: u8, name: &str, epoch: u64) -> SupervisorBindingReceipt {
        let pubkey = [seed; 32];
        SupervisorBindingReceipt::new(
            name.to_string(),
            meerkat_core::comms::PeerId::from_ed25519_pubkey(&pubkey).as_str(),
            format!("inproc://{name}"),
            crate::comms_drain::encode_supervisor_signing_public_key(pubkey),
            epoch,
        )
    }

    fn rotation(
        operation_id: meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId,
        phase: SupervisorRotationPersistencePhase,
        rejection: Option<SupervisorRotationRejection>,
        previous: SupervisorBindingReceipt,
        next: SupervisorBindingReceipt,
    ) -> SupervisorRotationReceipt {
        SupervisorRotationReceipt::new(operation_id, phase, rejection, previous, next)
    }

    fn snapshot(authority: SupervisorAuthoritySnapshot) -> MachineLifecycleSnapshot {
        MachineLifecycleSnapshot::new(
            RuntimeState::Idle,
            MachineLifecycleBindingFacts::new(None, None, None, None),
            authority,
        )
    }

    fn encode_snapshot(snapshot: &MachineLifecycleSnapshot) -> Vec<u8> {
        MachineLifecycleStoreRecord::from_snapshot(snapshot)
            .encode()
            .expect("encode lifecycle snapshot")
    }

    fn encode_unvalidated_snapshot(snapshot: &MachineLifecycleSnapshot) -> Vec<u8> {
        serde_json::to_vec(&MachineLifecycleSnapshotStoreWire::from(snapshot))
            .expect("serialize deliberately corrupt lifecycle snapshot")
    }

    fn encoded_value(snapshot: &MachineLifecycleSnapshot) -> serde_json::Value {
        serde_json::from_slice(&encode_snapshot(snapshot)).expect("decode encoded snapshot as JSON")
    }

    fn assert_decode_fails(value: serde_json::Value) {
        let bytes = serde_json::to_vec(&value).expect("serialize corrupt lifecycle record");
        assert!(
            decode_machine_lifecycle_store_record(&bytes).is_err(),
            "corrupt lifecycle record must fail closed: {value}"
        );
    }

    #[test]
    fn version_one_record_without_supervisor_authority_migrates_explicitly_to_unbound() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "record_version": LEGACY_MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            "runtime_state": RuntimeState::Retired,
            "binding": {
                "agent_runtime_id": "rt:session:legacy-v1",
                "fence_token": 19,
                "runtime_generation": 4,
                "runtime_epoch_id": "epoch-legacy-v1"
            }
        }))
        .expect("serialize legacy v1 lifecycle record");

        let decoded = decode_machine_lifecycle_store_record(&bytes)
            .expect("valid v1 record without the additive field must decode");
        assert_eq!(decoded.runtime_state(), RuntimeState::Retired);
        assert_eq!(
            decoded.supervisor_authority(),
            &SupervisorAuthoritySnapshot::UnboundNoReceipt
        );
    }

    #[test]
    fn current_record_requires_supervisor_authority_and_unregister_progress_presence() {
        assert_decode_fails(serde_json::json!({
            "record_version": MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            "runtime_state": RuntimeState::Idle,
            "binding": {
                "agent_runtime_id": null,
                "fence_token": null,
                "runtime_generation": null,
                "runtime_epoch_id": null
            },
            "unregister_progress": null
        }));
    }

    #[test]
    fn current_nullable_fields_require_presence_but_accept_explicit_null() {
        let unbound = snapshot(SupervisorAuthoritySnapshot::UnboundNoReceipt);
        let encoded = encoded_value(&unbound);
        assert_eq!(
            decode_machine_lifecycle_store_record(
                &serde_json::to_vec(&encoded).expect("serialize valid current record")
            )
            .expect("explicit-null current binding fields must decode"),
            unbound
        );
        let mut missing_progress = encoded.clone();
        missing_progress
            .as_object_mut()
            .expect("lifecycle record object")
            .remove("unregister_progress");
        assert_decode_fails(missing_progress);

        for field in [
            "agent_runtime_id",
            "fence_token",
            "runtime_generation",
            "runtime_epoch_id",
        ] {
            let mut partial = encoded.clone();
            partial["binding"]
                .as_object_mut()
                .expect("binding object")
                .remove(field);
            assert_decode_fails(partial);
        }
        for field in ["current_run_id", "pre_run_phase"] {
            let mut partial = encoded.clone();
            partial
                .as_object_mut()
                .expect("lifecycle record object")
                .remove(field);
            assert_decode_fails(partial);
        }

        let completed = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(101),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(30, "required-null-previous", 4),
            binding(31, "required-null-next", 5),
        )));
        let mut missing_rejection = encoded_value(&completed);
        assert!(missing_rejection["supervisor_authority"]["rotation"]["rejection"].is_null());
        missing_rejection["supervisor_authority"]["rotation"]
            .as_object_mut()
            .expect("rotation object")
            .remove("rejection");
        assert_decode_fails(missing_rejection);
    }

    #[test]
    fn lossless_observation_preserves_partial_run_pair_and_nullable_lifecycle() {
        let mut value = encoded_value(&snapshot(SupervisorAuthoritySnapshot::UnboundNoReceipt));
        let run_id = RunId::new();
        value["runtime_state"] = serde_json::Value::Null;
        value["current_run_id"] = serde_json::to_value(&run_id).expect("serialize run id");
        value["pre_run_phase"] = serde_json::Value::Null;
        let bytes = serde_json::to_vec(&value).expect("serialize partial lifecycle row");

        let MachineLifecycleObservation::Decoded { record, version } =
            classify_machine_lifecycle_record(&bytes)
        else {
            panic!("explicitly nullable partial runtime tuple must remain decoded");
        };
        assert_eq!(
            record.record_version(),
            MACHINE_LIFECYCLE_STORE_RECORD_VERSION
        );
        assert_eq!(record.runtime_state(), None);
        assert_eq!(record.run().current_run_id(), Some(&run_id));
        assert_eq!(record.run().pre_run_phase(), None);
        assert_eq!(
            version.as_str(),
            format!("sha256:{:x}", Sha256::digest(&bytes))
        );
        assert!(decode_machine_lifecycle_store_record(&bytes).is_err());
    }

    #[test]
    fn lifecycle_observation_distinguishes_unsupported_and_malformed_raw_rows() {
        let unsupported = br#"{"record_version":99,"opaque":"future"}"#;
        assert!(matches!(
            classify_machine_lifecycle_record(unsupported),
            MachineLifecycleObservation::Unsupported {
                record_version: 99,
                ..
            }
        ));

        let malformed = br#"{"record_version":4,"binding":"torn"}"#;
        assert!(matches!(
            classify_machine_lifecycle_record(malformed),
            MachineLifecycleObservation::Malformed {
                record_version: Some(4),
                ..
            }
        ));

        let undecodable = b"not-json";
        assert!(matches!(
            classify_machine_lifecycle_record(undecodable),
            MachineLifecycleObservation::Malformed {
                record_version: None,
                ..
            }
        ));
    }

    #[test]
    fn version_three_unregister_record_migrates_without_run_binding() {
        let expected = snapshot(SupervisorAuthoritySnapshot::UnboundNoReceipt);
        let mut value = encoded_value(&expected);
        value["record_version"] =
            serde_json::json!(UNREGISTER_MACHINE_LIFECYCLE_STORE_RECORD_VERSION);
        value
            .as_object_mut()
            .expect("lifecycle record object")
            .remove("current_run_id");
        value
            .as_object_mut()
            .expect("lifecycle record object")
            .remove("pre_run_phase");
        let bytes = serde_json::to_vec(&value).expect("serialize v3 row");
        let decoded = decode_machine_lifecycle_store_record(&bytes).expect("decode v3 row");
        assert_eq!(decoded, expected);
        assert_eq!(decoded.run(), &MachineLifecycleRunFacts::default());
    }

    #[test]
    fn completed_unregister_v5_round_trip_preserves_durable_live_bridge_evidence() {
        let live_bridge_recovery = durable_live_bridge_evidence();
        let snapshot = MachineLifecycleSnapshot::new_with_run_unregister_progress_and_live_bridge(
            RuntimeState::Retired,
            MachineLifecycleBindingFacts::default(),
            MachineLifecycleRunFacts::default(),
            SupervisorAuthoritySnapshot::UnboundNoReceipt,
            None,
            live_bridge_recovery.clone(),
        );

        let decoded = decode_machine_lifecycle_store_record(&encode_snapshot(&snapshot))
            .expect("decode completed unregister v5 row");

        assert_eq!(decoded.runtime_state(), RuntimeState::Retired);
        assert_eq!(decoded.binding(), &MachineLifecycleBindingFacts::default());
        assert_eq!(decoded.unregister_progress(), None);
        assert_eq!(decoded.live_bridge_recovery(), &live_bridge_recovery);
    }

    #[test]
    fn version_four_completed_unregister_remains_compatible_with_empty_bridge_image() {
        let expected = snapshot(SupervisorAuthoritySnapshot::UnboundNoReceipt);
        let mut value = encoded_value(&expected);
        value["record_version"] = serde_json::json!(RUN_MACHINE_LIFECYCLE_STORE_RECORD_VERSION);
        value
            .as_object_mut()
            .expect("lifecycle record object")
            .remove("live_bridge_recovery");
        let bytes = serde_json::to_vec(&value).expect("serialize v4 completed unregister row");

        let decoded = decode_machine_lifecycle_store_record(&bytes)
            .expect("decode v4 completed unregister row");

        assert_eq!(decoded, expected);
        assert_eq!(
            decoded.live_bridge_recovery(),
            &crate::live_execution::LiveBridgeRecoveryImage::default()
        );
    }

    #[test]
    fn version_two_supervisor_record_migrates_with_no_unregister_progress() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "record_version": SUPERVISOR_MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            "runtime_state": RuntimeState::Retired,
            "binding": {
                "agent_runtime_id": "rt:session:legacy-v2",
                "fence_token": 23,
                "runtime_generation": 5,
                "runtime_epoch_id": "epoch-legacy-v2"
            },
            "supervisor_authority": { "kind": "unbound_no_receipt" }
        }))
        .expect("serialize v2 lifecycle record");

        let decoded = decode_machine_lifecycle_store_record(&bytes)
            .expect("valid v2 supervisor record must migrate");
        assert_eq!(decoded.runtime_state(), RuntimeState::Retired);
        assert_eq!(decoded.unregister_progress(), None);
    }

    #[test]
    fn current_unregister_progress_rejects_forced_disposition_before_feedback() {
        let mut value = encoded_value(&snapshot(SupervisorAuthoritySnapshot::UnboundNoReceipt));
        value["unregister_progress"] = serde_json::json!({
            "runtime_loop_drain_pending": true,
            "comms_drain_exit_pending": false,
            "completion_waiter_drain_pending": true,
            "runtime_loop_forced_abort": true,
            "comms_drain_forced_abort": false
        });
        assert_decode_fails(value);
    }

    #[test]
    fn version_one_migration_rejects_current_authority_fields() {
        assert_decode_fails(serde_json::json!({
            "record_version": LEGACY_MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            "runtime_state": RuntimeState::Idle,
            "binding": {
                "agent_runtime_id": null,
                "fence_token": null,
                "runtime_generation": null,
                "runtime_epoch_id": null
            },
            "supervisor_authority": { "kind": "unbound_no_receipt" }
        }));
    }

    #[test]
    fn mixed_or_unknown_supervisor_authority_fields_fail_closed() {
        let current = binding(1, "current-supervisor", 7);
        let mut value = encoded_value(&snapshot(SupervisorAuthoritySnapshot::Bound(current)));
        value["supervisor_authority"]["rotation"] = serde_json::json!({});
        assert_decode_fails(value);
    }

    #[test]
    fn completed_rotation_operation_receipt_round_trips_for_cold_observation() {
        let snapshot = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(1),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(1, "previous-supervisor", 7),
            binding(2, "next-supervisor", 8),
        )));

        let encoded = encode_snapshot(&snapshot);
        let decoded = decode_machine_lifecycle_store_record(&encoded)
            .expect("decode completed rotation receipt");

        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn exact_current_completed_adoption_round_trips_but_other_equal_epoch_completion_fails() {
        let current = binding(3, "already-rotated-supervisor", 9);
        let adoption = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(2),
            SupervisorRotationPersistencePhase::Completed,
            None,
            current.clone(),
            current,
        )));
        assert_eq!(
            decode_machine_lifecycle_store_record(&encode_snapshot(&adoption))
                .expect("exact-current legacy adoption receipt must decode"),
            adoption
        );

        let non_advancing = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(3),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(3, "previous-supervisor", 9),
            binding(4, "different-supervisor", 9),
        )));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&non_advancing))
                .is_err()
        );
    }

    #[test]
    fn malformed_rotation_descriptors_epochs_and_operation_ids_fail_closed() {
        let invalid_previous = SupervisorBindingReceipt::new(
            String::new(),
            "not-a-uuid".to_string(),
            "not-an-address".to_string(),
            "not-a-key".to_string(),
            1,
        );
        let invalid_previous_receipt =
            snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
                operation_id(4),
                SupervisorRotationPersistencePhase::Rejected,
                Some(SupervisorRotationRejection::InvalidTarget),
                invalid_previous,
                binding(5, "raw-target", 2),
            )));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(
                &invalid_previous_receipt,
            ))
            .is_err()
        );

        let invalid_next = SupervisorBindingReceipt::new(
            "invalid-target".to_string(),
            "not-a-uuid".to_string(),
            "not-an-address".to_string(),
            "not-a-key".to_string(),
            2,
        );
        let invalid_completed_target =
            snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
                operation_id(5),
                SupervisorRotationPersistencePhase::Completed,
                None,
                binding(6, "previous-supervisor", 1),
                invalid_next,
            )));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(
                &invalid_completed_target,
            ))
            .is_err()
        );

        let mut invalid_id = encoded_value(&snapshot(
            SupervisorAuthoritySnapshot::RotationOperation(rotation(
                operation_id(6),
                SupervisorRotationPersistencePhase::PreviousRevokePending,
                None,
                binding(7, "previous-supervisor", 1),
                binding(8, "next-supervisor", 2),
            )),
        ));
        invalid_id["supervisor_authority"]["rotation"]["operation_id"] =
            serde_json::json!("not-a-uuid");
        assert_decode_fails(invalid_id);

        let nil_id = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(0),
            SupervisorRotationPersistencePhase::PreviousRevokePending,
            None,
            binding(7, "previous-supervisor", 1),
            binding(8, "next-supervisor", 2),
        )));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&nil_id)).is_err()
        );

        let non_advancing_pending =
            snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
                operation_id(13),
                SupervisorRotationPersistencePhase::PreviousRevokePending,
                None,
                binding(7, "previous-supervisor", 4),
                binding(8, "next-supervisor", 4),
            )));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(
                &non_advancing_pending,
            ))
            .is_err()
        );
    }

    #[test]
    fn rejected_invalid_or_unsupported_target_preserves_raw_evidence() {
        for (id, rejection) in [
            (7, SupervisorRotationRejection::InvalidTarget),
            (14, SupervisorRotationRejection::UnsupportedProtocolVersion),
        ] {
            let raw_invalid_target = SupervisorBindingReceipt::new(
                "".to_string(),
                "not-a-peer-id".to_string(),
                "not-an-address".to_string(),
                "not-a-signing-key".to_string(),
                0,
            );
            let snapshot = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
                operation_id(id),
                SupervisorRotationPersistencePhase::Rejected,
                Some(rejection),
                binding(9, "retained-supervisor", 11),
                raw_invalid_target,
            )));
            assert_eq!(
                decode_machine_lifecycle_store_record(&encode_snapshot(&snapshot))
                    .expect("rejected raw target evidence must remain durable"),
                snapshot
            );
        }
    }

    #[test]
    fn only_raw_target_rejections_are_durable_and_epoch_rejection_must_be_genuine() {
        for (id, rejection) in [
            (102, SupervisorRotationRejection::OperationConflict),
            (103, SupervisorRotationRejection::NotBound),
            (104, SupervisorRotationRejection::SenderMismatch),
        ] {
            let impossible = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
                operation_id(id),
                SupervisorRotationPersistencePhase::Rejected,
                Some(rejection),
                binding(32, "retained-supervisor", 7),
                binding(33, "requested-supervisor", 8),
            )));
            assert!(
                MachineLifecycleStoreRecord::from_snapshot(&impossible)
                    .encode()
                    .is_err()
            );
            assert!(
                decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&impossible))
                    .is_err()
            );
        }

        let advancing = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(105),
            SupervisorRotationPersistencePhase::Rejected,
            Some(SupervisorRotationRejection::TargetEpochNotAdvanced),
            binding(34, "retained-supervisor", 9),
            binding(35, "advancing-target", 10),
        )));
        assert!(
            MachineLifecycleStoreRecord::from_snapshot(&advancing)
                .encode()
                .is_err()
        );
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&advancing))
                .is_err()
        );

        let non_advancing = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(106),
            SupervisorRotationPersistencePhase::Rejected,
            Some(SupervisorRotationRejection::TargetEpochNotAdvanced),
            binding(36, "retained-supervisor", 11),
            binding(37, "non-advancing-target", 11),
        )));
        assert_eq!(
            decode_machine_lifecycle_store_record(&encode_snapshot(&non_advancing))
                .expect("genuine target-epoch rejection must remain durable"),
            non_advancing
        );
    }

    #[test]
    fn malformed_current_authority_variants_fail_closed() {
        let malformed = SupervisorBindingReceipt::new(
            String::new(),
            "not-a-peer-id".to_string(),
            "not-an-address".to_string(),
            "not-a-signing-key".to_string(),
            1,
        );
        let bound = snapshot(SupervisorAuthoritySnapshot::Bound(malformed.clone()));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&bound)).is_err()
        );

        let pending = snapshot(SupervisorAuthoritySnapshot::RevocationPending(
            SupervisorRevocationPendingReceipt::new(
                malformed.name().to_owned(),
                malformed.peer_id().to_owned(),
                malformed.address().to_owned(),
                malformed.signing_public_key().to_owned(),
                malformed.epoch(),
            ),
        ));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&pending)).is_err()
        );

        let revoked = snapshot(SupervisorAuthoritySnapshot::RevokedReceipt(
            RevokedSupervisorReceipt::new(
                malformed.peer_id().to_owned(),
                malformed.signing_public_key().to_owned(),
                malformed.epoch(),
            ),
        ));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&revoked)).is_err()
        );
    }

    #[test]
    fn partial_and_nonterminal_history_records_fail_closed() {
        let receipt = rotation(
            operation_id(8),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(10, "history-previous", 1),
            binding(11, "history-next", 2),
        );
        let history = std::collections::BTreeMap::from([(receipt.operation_id(), receipt)]);
        let snapshot = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                12,
                "current-supervisor",
                3,
            ))),
            terminal_receipts: history,
        });

        let mut partial = encoded_value(&snapshot);
        partial["supervisor_authority"]["terminal_receipts"][0]
            .as_object_mut()
            .expect("history receipt object")
            .remove("next");
        assert_decode_fails(partial);

        let mut nonterminal = encoded_value(&snapshot);
        nonterminal["supervisor_authority"]["terminal_receipts"][0]["phase"] =
            serde_json::json!("next_publish_pending");
        assert_decode_fails(nonterminal);
    }

    #[test]
    fn duplicate_nested_and_active_history_conflicts_fail_closed() {
        let history_receipt = rotation(
            operation_id(9),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(13, "history-previous", 1),
            binding(14, "history-next", 2),
        );
        let history = std::collections::BTreeMap::from([(
            history_receipt.operation_id(),
            history_receipt.clone(),
        )]);
        let wrapper = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                15,
                "current-supervisor",
                3,
            ))),
            terminal_receipts: history,
        });

        let mut duplicate = encoded_value(&wrapper);
        let receipt = duplicate["supervisor_authority"]["terminal_receipts"][0].clone();
        duplicate["supervisor_authority"]["terminal_receipts"]
            .as_array_mut()
            .expect("history receipt array")
            .push(receipt);
        assert_decode_fails(duplicate);

        let mut nested = encoded_value(&wrapper);
        let nested_current = nested["supervisor_authority"].clone();
        nested["supervisor_authority"]["current"] = nested_current;
        assert_decode_fails(nested);

        let active_conflict = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::RotationOperation(
                history_receipt.clone(),
            )),
            terminal_receipts: std::collections::BTreeMap::from([(
                history_receipt.operation_id(),
                history_receipt,
            )]),
        });
        assert!(
            MachineLifecycleStoreRecord::from_snapshot(&active_conflict)
                .encode()
                .is_err()
        );

        let empty_history = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                20,
                "current-supervisor",
                4,
            ))),
            terminal_receipts: std::collections::BTreeMap::new(),
        });
        assert!(
            MachineLifecycleStoreRecord::from_snapshot(&empty_history)
                .encode()
                .is_err()
        );
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&empty_history))
                .is_err()
        );

        let mismatched_key = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                21,
                "current-supervisor",
                4,
            ))),
            terminal_receipts: std::collections::BTreeMap::from([(
                operation_id(99),
                rotation(
                    operation_id(98),
                    SupervisorRotationPersistencePhase::Completed,
                    None,
                    binding(22, "history-previous", 2),
                    binding(23, "history-next", 3),
                ),
            )]),
        });
        assert!(
            MachineLifecycleStoreRecord::from_snapshot(&mismatched_key)
                .encode()
                .is_err()
        );
    }

    #[test]
    fn history_current_epoch_and_same_epoch_identity_must_cohere() {
        let previous = binding(38, "history-previous", 12);
        let next = binding(39, "history-next", 13);
        let completed = rotation(
            operation_id(107),
            SupervisorRotationPersistencePhase::Completed,
            None,
            previous.clone(),
            next.clone(),
        );
        let history =
            std::collections::BTreeMap::from([(completed.operation_id(), completed.clone())]);

        let stale_current = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                38,
                "refreshed-history-previous",
                12,
            ))),
            terminal_receipts: history.clone(),
        });
        assert!(
            MachineLifecycleStoreRecord::from_snapshot(&stale_current)
                .encode()
                .is_err()
        );
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&stale_current))
                .is_err()
        );

        let conflicting_current = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                40,
                "conflicting-current",
                13,
            ))),
            terminal_receipts: history.clone(),
        });
        assert!(
            MachineLifecycleStoreRecord::from_snapshot(&conflicting_current)
                .encode()
                .is_err()
        );
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(
                &conflicting_current,
            ))
            .is_err()
        );

        let route_refreshed_current = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                39,
                "route-refreshed-history-next",
                13,
            ))),
            terminal_receipts: history,
        });
        assert_eq!(
            decode_machine_lifecycle_store_record(&encode_snapshot(&route_refreshed_current))
                .expect("same identity may refresh route metadata within one epoch"),
            route_refreshed_current
        );
    }

    #[test]
    fn terminal_history_survives_later_rotation_and_recovery() {
        let first = rotation(
            operation_id(10),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(16, "first-supervisor", 1),
            binding(17, "second-supervisor", 2),
        );
        let rejected = rotation(
            operation_id(11),
            SupervisorRotationPersistencePhase::Rejected,
            Some(SupervisorRotationRejection::TargetEpochNotAdvanced),
            binding(17, "second-supervisor", 2),
            binding(18, "rejected-supervisor", 2),
        );
        let later = rotation(
            operation_id(12),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(17, "second-supervisor", 2),
            binding(19, "current-supervisor", 3),
        );
        let snapshot = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::RotationOperation(later)),
            terminal_receipts: std::collections::BTreeMap::from([
                (first.operation_id(), first),
                (rejected.operation_id(), rejected),
            ]),
        });

        let decoded = decode_machine_lifecycle_store_record(&encode_snapshot(&snapshot))
            .expect("later rotation and old terminal history must recover together");
        assert_eq!(decoded, snapshot);
    }
}
