//! Compatibility adapters bridging legacy Meta traits to identity-first contracts.
//!
//! - [`DiscoveryRosterAdapter`]: `Discovery` → `RosterProvider` (CONTRACT-08, REQ-27, REQ-28)
//! - [`EdgeDiscoveryTopologyAdapter`]: `EdgeDiscovery` → `TopologyProvider` (CONTRACT-09, REQ-29)
//! - [`ContinuitySessionStoreAdapter`]: `ContinuityStore` → `SessionStore` (CONTRACT-10)
//! - [`SessionHookCustomizerAdapter`]: `SessionHook` → `AgentCustomizer` (CONTRACT-11, REQ-30)

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;

use super::contracts::{
    AgentCustomizer, RosterProvider, SessionSnapshotMatchCandidate, TopologyProvider,
};
use super::types::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, CustomizerError,
    DurableAgentSpec, ManagedPeerEdge, RosterContext, RosterError, TopologyContext, TopologyError,
};
use crate::mob_handle_runtime::{SessionCreatedContext, SessionHook};
use crate::types::AgentDiscoverySpec;
use crate::unified_runtime::edge_types::{Discovery, EdgeDiscovery};

// ---------------------------------------------------------------------------
// CONTRACT-08 / REQ-27 / REQ-28: Discovery → RosterProvider
// ---------------------------------------------------------------------------

/// Adapts a legacy `Discovery` trait impl into a `RosterProvider`.
///
/// In-process mutable roster: the desired-identity list for hosts where the
/// roster is operator-driven rather than app-provided (the identity-first
/// console gateway: seeded from init params, extended by
/// `meta/ensure_member`, shrunk by identity deletion). `roster()` returns a
/// snapshot; mutations take effect on the next reconcile
/// (`restore_flow` / `meta/reconcile_identity` / the Broken-identity
/// repair task).
#[derive(Default)]
pub struct MutableRosterProvider {
    roster: std::sync::RwLock<Vec<DurableAgentSpec>>,
}

impl MutableRosterProvider {
    pub fn new(initial: Vec<DurableAgentSpec>) -> Self {
        Self {
            roster: std::sync::RwLock::new(initial),
        }
    }

    /// Insert or replace (by identity) a desired spec.
    pub fn upsert(&self, spec: DurableAgentSpec) {
        let mut roster = self
            .roster
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match roster
            .iter_mut()
            .find(|entry| entry.identity == spec.identity)
        {
            Some(entry) => *entry = spec,
            None => roster.push(spec),
        }
    }

    /// Remove an identity from the desired roster. Returns whether it was
    /// present. Removal does NOT retire the live identity — reconcile owns
    /// convergence.
    pub fn remove(&self, identity: &AgentIdentity) -> bool {
        let mut roster = self
            .roster
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = roster.len();
        roster.retain(|entry| &entry.identity != identity);
        roster.len() != before
    }

    pub fn snapshot(&self) -> Vec<DurableAgentSpec> {
        self.roster
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl RosterProvider for MutableRosterProvider {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(self.snapshot())
    }
}

/// Maps `AgentDiscoverySpec` to `DurableAgentSpec` per REQ-27:
/// - `meerkat_id` → `identity` (parsed as `AgentIdentity`)
/// - `profile` → `profile`
/// - `labels` → `labels`
/// - `context` → `context`
/// - `additional_instructions` → `additional_instructions`
/// - `resume_session_id` → ignored
/// - `addressability` → `Addressable`
/// - `display_name` → `None`
pub struct DiscoveryRosterAdapter {
    inner: Box<dyn Discovery>,
}

impl DiscoveryRosterAdapter {
    pub fn new(discovery: impl Discovery + 'static) -> Self {
        Self {
            inner: Box::new(discovery),
        }
    }
}

/// Convert an `AgentDiscoverySpec` to a `DurableAgentSpec` per REQ-27.
pub fn agent_discovery_to_durable(
    spec: &AgentDiscoverySpec,
) -> Result<DurableAgentSpec, RosterError> {
    let identity = AgentIdentity::parse(&spec.meerkat_id)
        .map_err(|e| RosterError::Io(format!("invalid meerkat_id: {e}")))?;
    Ok(DurableAgentSpec {
        identity,
        profile: meerkat_mob::ProfileName::from(spec.profile.as_str()),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: spec.labels.clone().unwrap_or_default(),
        context: spec.context.clone(),
        additional_instructions: spec.additional_instructions.clone(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    })
}

#[async_trait]
impl RosterProvider for DiscoveryRosterAdapter {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        let specs = self.inner.discover(serde_json::Value::Null).await;
        specs.iter().map(agent_discovery_to_durable).collect()
    }
}

// ---------------------------------------------------------------------------
// CONTRACT-09 / REQ-29: EdgeDiscovery → TopologyProvider
// ---------------------------------------------------------------------------

/// Adapts a legacy `EdgeDiscovery` trait impl into a `TopologyProvider`.
///
/// Parses `DesiredPeerEdge` endpoint strings as `AgentIdentity` to produce
/// `ManagedPeerEdge` instances.
pub struct EdgeDiscoveryTopologyAdapter {
    inner: Box<dyn EdgeDiscovery>,
}

impl EdgeDiscoveryTopologyAdapter {
    pub fn new(edge_discovery: impl EdgeDiscovery + 'static) -> Self {
        Self {
            inner: Box::new(edge_discovery),
        }
    }
}

#[async_trait]
impl TopologyProvider for EdgeDiscoveryTopologyAdapter {
    async fn compute_edges(
        &self,
        _target_identities: &[AgentIdentity],
        context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        // Project the roster context to EdgeMemberView so legacy EdgeDiscovery
        // impls see real member identities/labels instead of an empty vec.
        let member_views: Vec<crate::unified_runtime::edge_types::EdgeMemberView> = context
            .roster
            .iter()
            .map(|spec| crate::unified_runtime::edge_types::EdgeMemberView {
                agent_identity: spec.identity.as_str().to_string(),
                role: spec.profile.as_str().to_string(),
                wired_to: std::collections::BTreeSet::new(),
                labels: spec.labels.clone(),
            })
            .collect();

        let desired_edges = self.inner.discover_edges(member_views).await;
        let mut edges = Vec::with_capacity(desired_edges.len());
        for edge in &desired_edges {
            let (a_str, b_str) = edge.endpoints();
            let a = AgentIdentity::parse(a_str)
                .map_err(|e| TopologyError::InvalidEdge(format!("endpoint {a_str:?}: {e}")))?;
            let b = AgentIdentity::parse(b_str)
                .map_err(|e| TopologyError::InvalidEdge(format!("endpoint {b_str:?}: {e}")))?;
            let managed = ManagedPeerEdge::new(a, b)
                .map_err(|e| TopologyError::InvalidEdge(format!("{e}")))?;
            edges.push(managed);
        }
        Ok(edges)
    }
}

// ---------------------------------------------------------------------------
// CONTRACT-10: ContinuityStore → SessionStore adapter
// ---------------------------------------------------------------------------

/// Runtime state for a session, used by the adapter to resolve identity/fencing.
///
/// Public because publishing a cursor is the documented embedder seam: the
/// identity runtime does it through `MobSessionBridge`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRuntimeState {
    pub identity: AgentIdentity,
    pub generation: super::types::ContinuityGeneration,
    pub fencing_token: super::types::FencingToken,
    pub checkpoint_version: super::types::CheckpointVersion,
}

/// Pre-registration delta writes, held in process memory.
///
/// `PersistentSessionService` routes EVERY boundary save through the
/// incremental branch once the capability is `Some`, and member creation
/// provably saves before the bridge publishes the owning identity (the same
/// window `pending_unregistered` covers for whole-blob bytes). Refusing those
/// writes would break member creation on every identity gateway, so they are
/// parked here instead — in a plain in-memory `MemoryStore`, which is itself
/// a conforming `IncrementalSessionStore`, so parked reads answer the
/// service's `verify_incremental_projection_continuity` preflight and head
/// CAS view exactly as the durable channel would.
///
/// Two invariants: parked state is NEVER durable (nothing reaches the
/// continuity store until a cursor exists), and registration flushes it under
/// the real cursor or fails the registration typed — a parked write is never
/// silently dropped.
struct ParkedDeltas {
    store: meerkat_store::MemoryStore,
    sessions: Mutex<HashMap<String, ParkedFootprint>>,
}

/// What a session has actually parked under its routing marker.
///
/// The marker alone cannot answer the only question
/// [`ParkedDeltas::purge`] must never get wrong — "would dropping this
/// drop a write?" — because a marker is published by every parked
/// mutation, including ones that carry no rows. The footprint answers it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ParkedFootprint {
    /// Messages accepted into the parked strand rows. Counted at the write,
    /// after the parked store accepted it, so it can never overstate what is
    /// held.
    rows: u64,
}

impl ParkedDeltas {
    fn new() -> Self {
        Self {
            store: meerkat_store::MemoryStore::new(),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn is_parked(&self, session_id: &meerkat_core::types::SessionId) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&session_id.to_string())
    }

    /// Read-only view of the parked store.
    ///
    /// Reads are safe to hand out; WRITES are not, and none of this layer's
    /// write paths go through here. Parking a write and publishing its
    /// footprint are one operation ([`Self::park_append`],
    /// [`Self::park_rewrite`], [`Self::park_head`]) precisely so the
    /// footprint cannot drift from what the store holds — and the footprint
    /// is the only thing standing between [`ParkedFlush::Empty`] and a purge
    /// over real rows.
    fn reads(&self) -> &meerkat_store::MemoryStore {
        &self.store
    }

    /// Publish (or extend) a session's parked footprint. `rows` is the number
    /// of transcript messages this mutation just parked. Private: callers
    /// park through the three `park_*` verbs, which mark as part of the
    /// write.
    fn mark_parked(&self, session_id: &meerkat_core::types::SessionId, rows: u64) {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let footprint = sessions.entry(session_id.to_string()).or_default();
        footprint.rows = footprint.rows.saturating_add(rows);
    }

    /// Park an append and count exactly the rows the store accepted.
    ///
    /// Marking AFTER the store accepts is deliberate: a refused append parks
    /// nothing, so it must not publish a footprint (or a routing marker) for
    /// rows that do not exist.
    async fn park_append(
        &self,
        session_id: &meerkat_core::types::SessionId,
        strand: &meerkat_core::session_store::TranscriptStrandId,
        base_seq: u64,
        messages: &[meerkat_core::Message],
    ) -> Result<(), meerkat_store::SessionStoreError> {
        meerkat_core::session_store::IncrementalSessionStore::append_messages(
            &self.store,
            session_id,
            strand,
            base_seq,
            messages,
        )
        .await?;
        self.mark_parked(session_id, messages.len() as u64);
        Ok(())
    }

    async fn park_rewrite(
        &self,
        session_id: &meerkat_core::types::SessionId,
        record: &meerkat_core::TranscriptRewriteRecord,
        expected: meerkat_core::session_store::SessionHeadCas,
    ) -> Result<meerkat_core::session_store::SessionHead, meerkat_store::SessionStoreError> {
        let head = meerkat_core::session_store::IncrementalSessionStore::commit_rewrite(
            &self.store,
            session_id,
            record,
            expected,
        )
        .await?;
        self.mark_parked(session_id, record.revision_body.messages.len() as u64);
        Ok(head)
    }

    async fn park_head(
        &self,
        head: &meerkat_core::session_store::SessionHead,
        expected: meerkat_core::session_store::SessionHeadCas,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        meerkat_core::session_store::IncrementalSessionStore::save_head(
            &self.store,
            head,
            expected,
        )
        .await?;
        // A head adopts rows; it parks none of its own. The marker still has
        // to exist, so the zero-row mark is the point of this call.
        self.mark_parked(&head.id, 0);
        Ok(())
    }

    /// What this session holds in the parked store, or `None` when it is not
    /// parked at all.
    fn footprint(&self, session_id: &meerkat_core::types::SessionId) -> Option<ParkedFootprint> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&session_id.to_string())
            .copied()
    }

    fn unmark(&self, session_id: &meerkat_core::types::SessionId) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&session_id.to_string())
            .is_some()
    }

    /// Drop every parked row for a session and clear its routing marker.
    ///
    /// Only ever legal once the parked state has been ADOPTED (replayed
    /// durably) or proven empty — see [`ParkedFlush`]. Calling it on a
    /// still-unadopted footprint is the one way this layer can lose a write.
    async fn purge(&self, session_id: &meerkat_core::types::SessionId) {
        self.unmark(session_id);
        let _ = meerkat::SessionStore::delete(&self.store, session_id).await;
    }
}

/// The outcome of replaying a session's parked state at registration.
///
/// Deliberately not `Option<CheckpointVersion>`: "no version was committed"
/// and "there was nothing to commit" are different facts, and conflating
/// them is what let a purge run over unadopted rows.
enum ParkedFlush {
    /// The parked document was replayed into the durable channel; this is
    /// the checkpoint version it committed at.
    Adopted(super::types::CheckpointVersion),
    /// A routing marker existed but nothing had been parked under it, so
    /// there is nothing to replay and clearing the marker drops no write.
    Empty,
}

// ---------------------------------------------------------------------------
// Whole-document accounting
// ---------------------------------------------------------------------------

/// Why the adapter performed a WHOLE-DOCUMENT pass over a session.
///
/// The continuity layer's O(delta) claim is a claim about the ordinary turn:
/// a head-canonical steady-state save must not serialize, hash or copy the
/// whole document. It is not a claim that whole-document work never happens —
/// migration, recovery replay and removal compare-tokens are all inherently
/// O(document). Every such pass therefore names itself here,
/// and the counters behind [`ContinuitySessionStoreAdapter::whole_document_passes`]
/// make the distinction observable instead of asserted.
///
/// UPSTREAM BOUNDARY, stated honestly: meerkat's own `PersistentSessionService`
/// mints a whole-document RUNTIME SNAPSHOT per turn boundary — that is its
/// published contract (it records the bytes into
/// `meerkat_core::checkpoint::global_session_encode_bytes`). Those bytes are
/// produced above this adapter, are not Meta continuity cost, and are not
/// counted here. Equally, they must not contaminate this path: nothing below
/// reuses that snapshot as an excuse to re-encode the document for storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WholeDocumentPass {
    /// The session's canonical durable representation IS the whole blob, so
    /// the whole blob is what gets written.
    BlobCanonicalPersist,
    /// Registration replaying a parked creation-window document onto a
    /// session that already carries a durable head (recovery reconciliation).
    ParkedFlushAdoption,
    /// A whole-document compare token for a removal or projection CAS. The
    /// token is `sha256(serialize(document))` inside meerkat, so the pass is
    /// counted even though this adapter never holds the bytes.
    ProjectionCompareToken,
}

impl WholeDocumentPass {
    fn label(self) -> &'static str {
        match self {
            Self::BlobCanonicalPersist => "blob_canonical_persist",
            Self::ParkedFlushAdoption => "parked_flush_adoption",
            Self::ProjectionCompareToken => "projection_compare_token",
        }
    }
}

// ---------------------------------------------------------------------------
// Session lifecycle authority
// ---------------------------------------------------------------------------

/// Positive lifecycle authority for one session id, derived from durable
/// EVIDENCE.
///
/// This replaced "absent from a process-local durably-written set means
/// creation window", which inferred a lifecycle fact from the absence of a
/// process-local mark. A freshly started process observing a session that is
/// durable on disk starts with that set empty, so a cold-restored session was
/// indistinguishable from a never-written one: delete it, let a stale actor
/// save late, and the save parked as a "creation-window" write that the next
/// `register_session` flushed back — resurrecting the deleted document.
///
/// Every state here is established positively: by probing the substrate, by a
/// durable write this adapter completed, or by a removal this adapter
/// performed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionLifecycle {
    /// No durable document has ever been observed for this session id. The
    /// genuine pre-registration creation window, and the ONLY state in which
    /// a pre-registration write may park.
    CreationWindow,
    /// A durable document exists: probed on the continuity substrate, or
    /// written by this adapter (and acknowledged).
    DurableObserved,
    /// A durable document existed and was explicitly removed — `delete`,
    /// `delete_if_current_revision`, or reset abandonment. Sticky within the
    /// process until a genuinely new durable write recreates the session.
    Removed,
}

// ---------------------------------------------------------------------------
// Persistence capability selection
// ---------------------------------------------------------------------------

/// Which durable representation a write for one session must use.
///
/// Resolved BEFORE anything is serialized: a head-canonical session takes the
/// delta/head/CAS path and never encodes its document, a blob-canonical
/// session takes the whole-document path, and any state this adapter cannot
/// name is a typed refusal rather than a silent degradation to whole-blob
/// persistence.
enum PersistenceCapability {
    /// The substrate's delta channel is this session's canonical durable
    /// representation.
    HeadCanonical(Box<HeadCanonicalWrite>),
    /// A REGISTERED session with no durable representation at all on an
    /// incremental-capable substrate: this write BIRTHS the head-canonical
    /// representation (strand rows from 0 + the initial head under
    /// `SessionHeadCas::Create`). Head birth lived in the 0.8.10 service's
    /// incremental-channel drive; meerkat 0.8.11 removed the service's
    /// store writes, so the adapter owns it - without this, every new
    /// session would persist as O(document) whole-blob rows and the
    /// head-canonical steady state (O(delta) appends) would never engage.
    HeadCanonicalBirth(Box<HeadCanonicalBirthWrite>),
    /// The whole document is the unit of persistence: the substrate
    /// advertises no delta channel, the session is unregistered (park /
    /// supervisor discipline), or parked pre-registration state owns it.
    BlobCanonical,
}

/// Everything a head-canonical write needs, captured at capability
/// resolution: the channel, the stored head row it CAS-compares against, and
/// the registered continuity cursor state it writes under.
struct HeadCanonicalWrite {
    channel: Arc<dyn super::contracts::ContinuityIncrementalSessions>,
    stored: meerkat_core::session_store::SessionHead,
    state: SessionRuntimeState,
}

/// Everything a head-canonical BIRTH needs: the channel and the registered
/// continuity cursor state. There is no stored head to CAS against - the
/// birth head commits under `SessionHeadCas::Create`.
struct HeadCanonicalBirthWrite {
    channel: Arc<dyn super::contracts::ContinuityIncrementalSessions>,
    state: SessionRuntimeState,
}

/// Where a head-canonical document write lands on the strand graph.
#[derive(Clone, Debug, PartialEq, Eq)]
enum HeadCanonicalShape {
    /// The incoming transcript extends (or equals) the persisted head strand:
    /// append the tail to the SAME strand and re-point the head. The
    /// steady-state turn — O(delta) rows plus one small head row.
    PlainAppend,
    /// It does not: the write opens a fresh `rebase:` strand carrying the
    /// live transcript, which the immediately following head write adopts.
    /// Still entirely on the delta/head/CAS path — no blob is ever encoded —
    /// but it is a representation rebase, not an ordinary turn.
    Rebase(meerkat_core::session_store::TranscriptStrandId),
}

/// Where an incoming document lands on a head-canonical session, decided from
/// the SMALL head row alone.
///
/// This is the same rule meerkat-store applies in
/// `write_head_canonical_session_in_txn`: the incoming transcript is a plain
/// append exactly when its first `head.message_count` messages digest to
/// `head.head_revision`. `Session::transcript_prefix_digest` answers that from
/// the session's own boundary ring on an ordinary turn, so the steady-state
/// decision costs O(delta), not O(document).
/// Equality for the exact-resave noop: strict head equality, loosened by
/// EXACTLY two facts that are zero durable change by construction.
///
/// (a) `updated_at`: a timestamp-only difference re-mints nothing durable —
/// the noop's own charter ("would re-mint a checkpoint version for zero
/// durable change"). Every content-bearing field (messages, usage, every
/// other metadata value, prefixes, counts) stays byte-strict.
///
/// (b) The ORDER of `session_tool_visibility_state_v1`'s Allow/Deny arrays:
/// upstream projects `ToolNameSet` (a HashSet) through serde, so the same
/// visibility fact re-stamps as a differently-ordered array every boot —
/// per-process hash order, frozen into the session metadata. Equality on
/// that field is SET equality by the type's own semantics; comparing the
/// arrays order-sensitively made every zero-turn boot rewrite the head of
/// any session carrying a multi-tool filter (downstream app domain:security, the
/// boot-2 exactly-once violation: same content, same length, shuffled
/// bytes, checkpoint churn every boot). Only these two arrays are
/// canonicalized; no other array in the document is touched (arrays are
/// order-bearing everywhere else). Filed upstream: durable bytes minted
/// from HashSet iteration.
fn head_equal_for_exact_resave(
    adopted: &meerkat_core::session_store::SessionHead,
    stored: &meerkat_core::session_store::SessionHead,
) -> bool {
    if adopted == stored {
        return true;
    }
    let mut adopted = adopted.clone();
    let mut stored = stored.clone();
    stored.updated_at = adopted.updated_at;
    canonicalize_tool_visibility_order(&mut adopted.metadata);
    canonicalize_tool_visibility_order(&mut stored.metadata);
    adopted == stored
}

/// Sort the set-semantics tool arrays inside
/// `session_tool_visibility_state_v1` (see [`head_equal_for_exact_resave`]).
fn canonicalize_tool_visibility_order(metadata: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(state) = metadata.get_mut("session_tool_visibility_state_v1") else {
        return;
    };
    for filter_key in ["active_filter", "staged_filter"] {
        let Some(filter) = state.get_mut(filter_key) else {
            continue;
        };
        for tag in ["Allow", "Deny"] {
            if let Some(serde_json::Value::Array(names)) = filter.get_mut(tag) {
                names.sort_by(|a, b| {
                    a.as_str()
                        .unwrap_or_default()
                        .cmp(b.as_str().unwrap_or_default())
                });
            }
        }
    }
}

fn head_canonical_shape(
    session: &meerkat_core::Session,
    stored: &meerkat_core::session_store::SessionHead,
) -> Result<HeadCanonicalShape, meerkat_store::SessionStoreError> {
    let live = session.messages();
    let prev_count = usize::try_from(stored.message_count)
        .map_err(|_| meerkat_store::SessionStoreError::Corrupted(session.id().clone()))?;
    if live.len() >= prev_count {
        // When nothing was appended the stored prefix IS the whole
        // transcript. Its full content digest is byte-identical to the
        // prefix digest at that length, and unlike an unwitnessed prefix
        // digest it seeds the session's retained midstate, so the successor
        // head minted from this session does not hash the transcript again.
        let prefix = if live.len() == prev_count {
            session.transcript_content_digest()
        } else {
            session.transcript_prefix_digest(prev_count)
        }
        .map_err(meerkat_store::SessionStoreError::from)?;
        if prefix == stored.head_revision {
            return Ok(HeadCanonicalShape::PlainAppend);
        }
    }
    // Byte-identical to `transcript_messages_digest(live)`, served from the
    // session's retained midstate when it covers the live buffer.
    let live_digest = session
        .transcript_content_digest()
        .map_err(meerkat_store::SessionStoreError::from)?;
    Ok(HeadCanonicalShape::Rebase(
        meerkat_core::session_store::TranscriptStrandId::rebase(&live_digest),
    ))
}

/// Adapts a `ContinuityStore` to the Meerkat `SessionStore` interface.
///
/// On the external-authoritative path, this is the session persistence layer.
/// No separate local SQLite is created under scratch_dir — the ContinuityStore
/// is the single authoritative session truth.
///
/// The adapter maintains a session→identity registry populated by the runtime
/// during restore/activate. This maps SessionId to the owning identity's
/// continuity parameters (identity, generation, fencing_token).
pub struct ContinuitySessionStoreAdapter {
    store: Arc<dyn super::contracts::ContinuityStore>,
    /// The substrate's session-delta channel, resolved once at construction
    /// (the capability is a property of the store, not of a call).
    incremental: Option<Arc<dyn super::contracts::ContinuityIncrementalSessions>>,
    /// Delta writes that arrive before the owning identity is registered.
    /// Nothing here is durable; `register_session` flushes it.
    parked_deltas: ParkedDeltas,
    /// Per-session monotonic version counter to satisfy CAS on repeated saves.
    versions: Mutex<HashMap<String, AtomicU64>>,
    /// Session→identity mapping, populated by the runtime.
    session_registry: Mutex<HashMap<String, SessionRuntimeState>>,
    /// Session saves that arrive before the bridge can publish the owning
    /// identity. These are flushed immediately when the session is registered.
    /// Admission is gated by [`Self::ensure_unregistered_park_allowed`]: only
    /// sessions with no durable history in this process may park here.
    pending_unregistered: Mutex<HashMap<String, Vec<u8>>>,
    /// Sessions that were explicitly unregistered. Later writes from those
    /// actors must fail closed instead of becoming pre-registration pending
    /// snapshots for a future session with the same id.
    unregistered_sessions: Mutex<HashSet<String>>,
    /// Sessions whose persistence authority is temporarily quiesced while the
    /// identity runtime rotates an external fencing grant. Unlike permanent
    /// unregistration, suspension preserves the registry/version state so a
    /// successful publication can resume the same session at the new token.
    suspended_sessions: Mutex<HashSet<String>>,
    /// Sessions deliberately superseded by a committed destructive reset.
    /// Their old physical member is already quiescing, so archive projection
    /// writes are acknowledged without crossing into the replacement
    /// identity generation. The exact snapshot is CAS-deleted before this
    /// marker is published and ordinary unregistration clears it after the
    /// retained Mob cleanup anchor reaches structural absence.
    superseded_sessions: Mutex<HashSet<String>>,
    /// Positive lifecycle authority per session id (see [`SessionLifecycle`]).
    ///
    /// An entry is published by durable EVIDENCE only: a substrate probe
    /// ([`Self::resolve_session_lifecycle`]), an acknowledged durable write
    /// ([`Self::mark_session_durably_written`]), or an explicit removal
    /// ([`Self::mark_session_removed`]). An ABSENT entry means "not yet
    /// determined" and forces the probe — it never means "creation window",
    /// which is precisely the inference that let a cold-restored, deleted
    /// session be resurrected by a parked late save.
    ///
    /// `route_delta_write` is evaluated independently per call, so an
    /// `append_messages` that routed `Durable` can be followed by a
    /// `save_head` that routes `Park` if the session unregistered in between.
    /// That parks the adopting head in memory while the strand rows are
    /// already durable: on the next open the head still names the previous
    /// revision and the reader serves an OLDER transcript than what is
    /// persisted. Every durable verb marks through
    /// [`Self::mark_session_durably_written`] AFTER the write is
    /// acknowledged; every park site — the delta route and the four
    /// whole-blob save verbs — asks
    /// [`Self::ensure_unregistered_park_allowed`].
    session_lifecycle: Mutex<HashMap<String, SessionLifecycle>>,
    /// Per-session serialization for registry, pending, version, and durable
    /// load/guard/write transitions. Weak values let inactive locks be reclaimed
    /// without ever creating a second lock while an operation or waiter exists.
    session_locks: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    /// Whole-document passes this adapter has performed (see
    /// [`WholeDocumentPass`]). Per-adapter, not process-global, so a test can
    /// observe exactly its own adapter without racing the rest of the suite.
    whole_document_passes: AtomicU64,
    /// Bytes produced by adapter-side whole-document ENCODES. A subset of
    /// [`Self::whole_document_passes`]: the compare-token pass serializes
    /// inside meerkat and its byte count is not observable here.
    whole_document_encode_bytes: AtomicU64,
    /// First head-canonical read declined for want of an incremental channel.
    ///
    /// A blob-only store has `incremental == None` from construction, so the
    /// decline fires on EVERY read of every session — 6,504 WARN lines in one
    /// observed production boot. The fault signal ("a head-canonical
    /// deployment lost its delta channel") only needs to be loud once per
    /// adapter; repeats carry no new information and drown real warnings.
    no_incremental_channel_warned: std::sync::atomic::AtomicBool,
}

impl ContinuitySessionStoreAdapter {
    pub fn new(store: Arc<dyn super::contracts::ContinuityStore>) -> Self {
        let incremental = store.as_incremental_sessions();
        Self {
            store,
            incremental,
            parked_deltas: ParkedDeltas::new(),
            versions: Mutex::new(HashMap::new()),
            session_registry: Mutex::new(HashMap::new()),
            pending_unregistered: Mutex::new(HashMap::new()),
            unregistered_sessions: Mutex::new(HashSet::new()),
            suspended_sessions: Mutex::new(HashSet::new()),
            superseded_sessions: Mutex::new(HashSet::new()),
            session_lifecycle: Mutex::new(HashMap::new()),
            session_locks: Mutex::new(HashMap::new()),
            whole_document_passes: AtomicU64::new(0),
            whole_document_encode_bytes: AtomicU64::new(0),
            no_incremental_channel_warned: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The concrete type of the continuity store this adapter persists to.
    #[must_use]
    pub fn continuity_store_type_name(&self) -> &'static str {
        self.store.store_type_name()
    }

    /// Whole-document passes this adapter has performed since construction
    /// (see `WholeDocumentPass`).
    ///
    /// The O(delta) continuity contract, made observable: a head-canonical
    /// STEADY-STATE save must not move this counter. Every value it can take
    /// names a migration, recovery, removal or blob-canonical persist —
    /// never an ordinary turn on a head-canonical session.
    #[must_use]
    pub fn whole_document_passes(&self) -> u64 {
        self.whole_document_passes.load(Ordering::Relaxed)
    }

    /// Bytes this adapter produced by encoding whole session documents.
    #[must_use]
    pub fn whole_document_encode_bytes(&self) -> u64 {
        self.whole_document_encode_bytes.load(Ordering::Relaxed)
    }

    fn record_whole_document_pass(
        &self,
        session_id: &meerkat_core::types::SessionId,
        pass: WholeDocumentPass,
        bytes: u64,
    ) {
        self.whole_document_passes.fetch_add(1, Ordering::Relaxed);
        self.whole_document_encode_bytes
            .fetch_add(bytes, Ordering::Relaxed);
        tracing::debug!(
            session_id = %session_id,
            pass = pass.label(),
            bytes,
            "continuity adapter performed a whole-document pass"
        );
    }

    /// The ONE adapter-side whole-session encode.
    ///
    /// Every caller names the reason it is legal — migration, recovery,
    /// or genuinely blob-canonical persistence. There is
    /// deliberately no reason value for "ordinary turn": a head-canonical
    /// save that reached here would be the silent degradation this seam
    /// exists to prevent, so the head-canonical paths refuse typed instead of
    /// calling it.
    fn encode_whole_document(
        &self,
        session: &meerkat_core::Session,
        pass: WholeDocumentPass,
    ) -> Result<Vec<u8>, meerkat_store::SessionStoreError> {
        let data = session
            .to_persisted_bytes()
            .map_err(|e| meerkat_store::SessionStoreError::Serialization(e.to_string()))?;
        self.record_whole_document_pass(session.id(), pass, data.len() as u64);
        Ok(data)
    }

    /// The whole-document compare token used by the removal and
    /// projection-CAS paths.
    ///
    /// `session_projection_cas_token` is `sha256(serialize(document))`, so
    /// this is an O(document) pass even though the bytes never surface here.
    /// It is counted for exactly that reason: the release claim is about the
    /// ordinary turn, and an uncounted whole-document pass on a save path
    /// would make the claim unverifiable.
    fn projection_compare_token(
        &self,
        session: &meerkat_core::Session,
    ) -> Result<String, meerkat_store::SessionStoreError> {
        let token = meerkat_core::session_store::session_projection_cas_token(session)?;
        self.record_whole_document_pass(session.id(), WholeDocumentPass::ProjectionCompareToken, 0);
        Ok(token)
    }

    fn session_lock(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Arc<tokio::sync::Mutex<()>> {
        const PRUNE_THRESHOLD: usize = 1_024;

        let key = session_id.to_string();
        let mut locks = self
            .session_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
            return lock;
        }
        if locks.len() >= PRUNE_THRESHOLD {
            locks.retain(|_, lock| lock.strong_count() > 0);
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(key, Arc::downgrade(&lock));
        lock
    }

    async fn lock_session(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        self.session_lock(session_id).lock_owned().await
    }

    /// Register a session with its owning identity's runtime state.
    ///
    /// Called by the runtime during restore/activate to wire real
    /// identity/generation/fencing data into the adapter. Public because
    /// cursor publication is the documented embedder/conformance seam.
    #[allow(dead_code)]
    pub async fn register_session(
        &self,
        session_id: &meerkat_core::types::SessionId,
        state: SessionRuntimeState,
    ) -> Result<super::types::CheckpointVersion, meerkat_store::SessionStoreError> {
        let _guard = self.lock_session(session_id).await;
        let session_key = session_id.to_string();
        let checkpoint_version = state.checkpoint_version.get();
        if let Some(existing) = self.lookup_session(&session_key) {
            if existing.identity != state.identity || existing.generation != state.generation {
                return Err(meerkat_store::SessionStoreError::Internal(format!(
                    "session ownership conflict for {session_id}: registered owner {}/generation {} cannot be replaced by {}/generation {}",
                    existing.identity, existing.generation, state.identity, state.generation
                )));
            }
            if state.fencing_token < existing.fencing_token {
                return Err(meerkat_store::SessionStoreError::Internal(format!(
                    "session ownership conflict for {session_id}: fencing token {} cannot regress to {}",
                    existing.fencing_token, state.fencing_token
                )));
            }
        }
        let was_unregistered = self
            .unregistered_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&session_key);
        let was_suspended = self
            .suspended_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&session_key);
        let was_superseded = self
            .superseded_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&session_key);
        let previous_registry = {
            let mut registry = self
                .session_registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.insert(session_key.clone(), state.clone())
        };
        {
            let mut versions = self
                .versions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let counter = versions
                .entry(session_key.clone())
                .or_insert_with(|| AtomicU64::new(checkpoint_version));
            counter.fetch_max(checkpoint_version, Ordering::Relaxed);
        }

        let pending = {
            let pending = self
                .pending_unregistered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pending.get(&session_id.to_string()).cloned()
        };
        let mut effective_checkpoint_version = self.current_version(session_id);
        let restore_markers = |adapter: &Self| {
            adapter.restore_registration_state(session_id, previous_registry.clone());
            if was_unregistered {
                adapter
                    .unregistered_sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(session_key.clone());
            }
            if was_suspended {
                adapter
                    .suspended_sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(session_key.clone());
            }
            if was_superseded {
                adapter
                    .superseded_sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(session_key.clone());
            }
        };
        if let Some(data) = pending {
            let flush_result = self
                .save_registered_snapshot(session_id, data, state.clone())
                .await;
            match flush_result {
                Ok(version) => {
                    self.pending_unregistered
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&session_id.to_string());
                    effective_checkpoint_version = version;
                }
                Err(err) => {
                    // The provider may have committed the attempted version
                    // and lost only its acknowledgement. Restore registry and
                    // marker state, but never rewind the monotonic allocator.
                    restore_markers(self);
                    return Err(err);
                }
            }
        }
        // Parked delta writes flush under the now-real cursor. Same
        // discipline as the pending-bytes flush: on failure the registration
        // is refused, registry and markers are restored, and the parked
        // state is RETAINED so a retry can replay it (never dropped).
        //
        // The purge is INSIDE the success arms on purpose. Dropping parked
        // rows is only ever legal once they have been adopted durably (or
        // proven empty); purging on any other outcome would delete the
        // writes this layer exists to protect.
        if self.parked_deltas.is_parked(session_id) {
            match self.flush_parked_deltas(session_id, &state).await {
                Ok(ParkedFlush::Adopted(version)) => {
                    effective_checkpoint_version = version;
                    self.parked_deltas.purge(session_id).await;
                }
                Ok(ParkedFlush::Empty) => {
                    self.parked_deltas.purge(session_id).await;
                }
                Err(err) => {
                    restore_markers(self);
                    return Err(err);
                }
            }
        }
        Ok(effective_checkpoint_version)
    }

    /// Replay parked delta state into the durable channel under `state`'s
    /// cursor.
    ///
    /// The realistic parked shape is a creation-window save: one root strand
    /// plus a `Create` head, replayed as `append_messages` + `save_head`.
    /// Three deliberate refusals:
    ///
    /// - parked rewrites (structurally impossible before the first turn) fail
    ///   the flush typed rather than being silently dropped;
    /// - parked ROWS with no adopting head — the service appends and adopts
    ///   under two separate locks, so a registration can land between them —
    ///   are not an adoptable document: replaying them blind would guess a
    ///   head, and dropping them would delete a write in exactly the window
    ///   this layer exists to protect. The flush fails typed instead, which
    ///   keeps the rows AND the routing marker (the caller restores the
    ///   registry), so the still-parked `save_head` completes the document
    ///   and a retried registration adopts the whole thing;
    /// - when the session ALREADY has a durable head (a registration that
    ///   raced a restore), the parked document is replayed through the
    ///   whole-document verb, whose head-canonical compat write owns the
    ///   plain-append-vs-rebase reconciliation rule — never a blind
    ///   `Create` that would CAS-conflict.
    async fn flush_parked_deltas(
        &self,
        session_id: &meerkat_core::types::SessionId,
        state: &SessionRuntimeState,
    ) -> Result<ParkedFlush, meerkat_store::SessionStoreError> {
        let Some(incremental) = self.incremental.as_ref() else {
            return Err(meerkat_store::SessionStoreError::Internal(format!(
                "session {session_id} parked incremental writes but the continuity substrate \
                 advertises no delta channel"
            )));
        };
        let parked = self.parked_deltas.reads();
        let Some(head) =
            meerkat_core::session_store::IncrementalSessionStore::load_head(parked, session_id)
                .await?
        else {
            let parked_rows = self
                .parked_deltas
                .footprint(session_id)
                .unwrap_or_default()
                .rows;
            if parked_rows == 0 {
                // A routing marker with nothing under it (an empty append is
                // how the service seeds a fresh session). Nothing to adopt,
                // and clearing it drops no write.
                //
                // The footprint is the load-bearing fact here, so it is
                // cross-examined against the one parked shape it does not
                // count: a rewrite. A rewrite is structurally impossible in
                // this arm (the parked store refuses `commit_rewrite`
                // without a head, and a head would have been loaded above),
                // so observing one means the footprint and the store have
                // diverged — refuse and RETAIN rather than purge on a
                // counter that has just been proven wrong.
                let parked_rewrites =
                    meerkat_core::session_store::IncrementalSessionStore::load_rewrites(
                        parked, session_id,
                    )
                    .await?;
                if !parked_rewrites.is_empty() {
                    return Err(meerkat_store::SessionStoreError::Internal(format!(
                        "session {session_id} reports an empty parked footprint but the parked \
                         store holds {} rewrite record(s) and no head; refusing the registration \
                         instead of purging state the footprint cannot account for",
                        parked_rewrites.len()
                    )));
                }
                return Ok(ParkedFlush::Empty);
            }
            tracing::warn!(
                session_id = %session_id,
                parked_rows,
                "registration refused: parked delta rows have no adopting head yet; \
                 retaining them for the retry"
            );
            return Err(meerkat_store::SessionStoreError::Internal(format!(
                "session {session_id} parked {parked_rows} delta message row(s) that no parked \
                 head adopts yet; refusing the registration instead of dropping them — the \
                 parked state is retained, so retry once the adopting head write lands"
            )));
        };
        if head.rewrite_count > 0
            || !meerkat_core::session_store::IncrementalSessionStore::load_rewrites(
                parked, session_id,
            )
            .await?
            .is_empty()
        {
            return Err(meerkat_store::SessionStoreError::Internal(format!(
                "session {session_id} parked a transcript rewrite before its owning identity was \
                 registered; refusing to flush an unauditable rewrite chain"
            )));
        }
        let messages = meerkat_core::session_store::IncrementalSessionStore::load_messages(
            parked,
            session_id,
            &head.strand,
            0..head.message_count,
        )
        .await?;

        if incremental.load_canonical_head(session_id).await?.is_some() {
            // RECOVERY RECONCILIATION, not an ordinary turn: a registration
            // that raced a restore has a parked creation-window document AND
            // a durable head. The whole-document verb owns the
            // plain-append-vs-rebase rule for that collision, so the encode
            // is legal here and named as such.
            let session = head.clone().into_session(messages)?;
            let data =
                self.encode_whole_document(&session, WholeDocumentPass::ParkedFlushAdoption)?;
            let version = self
                .save_registered_snapshot(session_id, data, state.clone())
                .await?;
            return Ok(ParkedFlush::Adopted(version));
        }

        let append_cursor = self.write_cursor(session_id, state);
        incremental
            .append_messages(&append_cursor, session_id, &head.strand, 0, &messages)
            .await?;
        let head_cursor = self.write_cursor(session_id, state);
        let committed = head_cursor.checkpoint_version;
        incremental
            .save_head(
                &head_cursor,
                &head,
                meerkat_core::session_store::SessionHeadCas::Create,
            )
            .await?;
        // The flushed rows and adopting head ARE durable writes: without
        // this mark, a session whose only durable history came from the
        // registration flush could later park a write and be resurrected by
        // a replayed flush — the exact vector the guard exists to refuse.
        self.mark_session_durably_written(session_id);
        Ok(ParkedFlush::Adopted(committed))
    }

    /// Resolve which durable representation a write for `id` must use —
    /// BEFORE anything is serialized.
    ///
    /// The probe is [`super::contracts::ContinuityIncrementalSessions::load_canonical_head`]:
    /// a small indexed point read that never synthesizes a head from a blob,
    /// so `Some` genuinely means "the delta channel IS this session's
    /// canonical representation". The three refusals below are the contract's
    /// fail-loud clause: a head-canonical session must never silently degrade
    /// to whole-document persistence, because that write would be O(document)
    /// AND would leave the durable head row describing a transcript the blob
    /// no longer agrees with.
    async fn resolve_persistence_capability(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<PersistenceCapability, meerkat_store::SessionStoreError> {
        let Some(channel) = self.incremental.as_ref() else {
            // No delta channel at all (the JSON-RPC gateway store): the whole
            // document is honestly the unit of persistence here.
            return Ok(PersistenceCapability::BlobCanonical);
        };
        let parked = self.parked_deltas.is_parked(id);
        let stored = channel.load_canonical_head(id).await?;
        match (parked, stored) {
            // Parked pre-registration state owns the creation window: the
            // registration flush downstream replays it.
            (true, None) => Ok(PersistenceCapability::BlobCanonical),
            // No persisted canonical head. For a REGISTERED session, the
            // SYNTHESIZING read decides: a legacy/imported blob synthesizes
            // a head and converts on this very write (the 0.8.10
            // lazy-migration dance - the store migrates the blob inside the
            // first delta write's transaction), while a truly fresh session
            // BIRTHS the head-canonical representation. Unregistered
            // sessions keep the whole-document park discipline (the
            // supervisor's sessions are never identity-registered).
            (false, None) => {
                let Some(state) = self.lookup_session(&id.to_string()) else {
                    return Ok(PersistenceCapability::BlobCanonical);
                };
                match channel.load_head(id).await? {
                    Some(stored) => Ok(PersistenceCapability::HeadCanonical(Box::new(
                        HeadCanonicalWrite {
                            channel: Arc::clone(channel),
                            stored,
                            state,
                        },
                    ))),
                    None => Ok(PersistenceCapability::HeadCanonicalBirth(Box::new(
                        HeadCanonicalBirthWrite {
                            channel: Arc::clone(channel),
                            state,
                        },
                    ))),
                }
            }
            (true, Some(_)) => Err(meerkat_store::SessionStoreError::Internal(format!(
                "session {id} holds parked pre-registration delta state while the continuity \
                 substrate already serves a durable head row; refusing to persist against a \
                 representation this adapter cannot name (parked state is only ever legal for a \
                 session with no durable document)"
            ))),
            (false, Some(stored)) => {
                let Some(state) = self.lookup_session(&id.to_string()) else {
                    // Parked repair (task #56 corpus, a production member): an
                    // EXPLICITLY unregistered session whose continuity
                    // record still binds it hydrates write authority from
                    // the DURABLE record - identity, generation, and fencing
                    // token are the same facts registration would carry, so
                    // the substrate write runs under the identity's current
                    // fence. Only the projection doors can reach this arm
                    // with an explicitly-unregistered session (their
                    // parked-repair admission); never-registered sessions
                    // (foreign writers, the supervisor plane) and sessions
                    // whose record rotated away keep the refusal below.
                    if self.session_was_unregistered(id) {
                        let bound =
                            self.store
                                .resolve_record_by_session(id)
                                .await
                                .map_err(|e| {
                                    meerkat_store::SessionStoreError::Internal(format!(
                                        "continuity record lookup for parked repair of {id}: {e}"
                                    ))
                                })?;
                        if let Some((record, fencing_token, fence_current)) = bound
                            && record.session_id == *id
                        {
                            // The write cursor presents from the per-session
                            // counter, which unregistration cleared: re-seed
                            // it at the SUBSTRATE's fence-current version so
                            // the repair writes present current + 1, exactly
                            // as a registered resume would.
                            {
                                let mut versions = self
                                    .versions
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                versions
                                    .entry(id.to_string())
                                    .or_insert_with(|| AtomicU64::new(fence_current.get()))
                                    .fetch_max(fence_current.get(), Ordering::Relaxed);
                            }
                            tracing::info!(
                                session_id = %id,
                                identity = %record.identity,
                                generation = record.generation.get(),
                                fence_current = fence_current.get(),
                                "parked repair write authority hydrated from the durable \
                                 continuity record (no in-memory registration)"
                            );
                            return Ok(PersistenceCapability::HeadCanonical(Box::new(
                                HeadCanonicalWrite {
                                    channel: Arc::clone(channel),
                                    stored,
                                    state: SessionRuntimeState {
                                        identity: record.identity,
                                        generation: record.generation,
                                        fencing_token,
                                        checkpoint_version: fence_current,
                                    },
                                },
                            )));
                        }
                    }
                    // The head row just read IS positive durable evidence.
                    // Publishing it here keeps this refusal on the same
                    // lifecycle footing as the blob parking guard: without
                    // it, the session's lifecycle stays unobserved and a
                    // later probe could reinterpret the same durable
                    // document as a creation window.
                    self.mark_session_durably_observed(id);
                    // BACKSTOP, not the primary contract. The actionable
                    // refusal for a recoverable owner now happens earlier, as an
                    // attach postcondition: a reconciled occupant is registered
                    // from the caller's authoritative continuity record before
                    // anything commits a runtime boundary, and a registration
                    // failure surfaces there as a typed retryable outcome.
                    //
                    // Reaching HERE means the owner is not merely unregistered
                    // but unrecoverable from this side - a foreign writer, the
                    // supervisor plane, or a record that rotated away - so there
                    // is no caller action that would make the write legal. The
                    // message says which, because "Store write failed" sent me
                    // chasing a persistence bug for three edits.
                    //
                    // The variant stays Internal because SessionStoreError is
                    // Meerkat-owned and Meta cannot add an
                    // unregistered-owner/incompatible-continuity variant to it.
                    return Err(meerkat_store::SessionStoreError::Internal(format!(
                        "refusing to degrade head-canonical session {id} to whole-document \
                         persistence: its owning identity has no authoritative registration and \
                         none can be reconciled from here (the session was never registered in \
                         this process, rather than explicitly unregistered for parked repair, so \
                         this is a foreign or rotated-away writer). This is a deliberate refusal, \
                         not a store failure: no retry of the write will succeed. The owner must \
                         be registered by the authority holding its continuity record before any \
                         runtime boundary commit."
                    )));
                };
                Ok(PersistenceCapability::HeadCanonical(Box::new(
                    HeadCanonicalWrite {
                        channel: Arc::clone(channel),
                        stored,
                        state,
                    },
                )))
            }
        }
    }

    /// BIRTH the head-canonical representation for a registered session
    /// with no durable state: the live transcript lands as root-strand rows
    /// (base 0) and the initial head commits under
    /// [`SessionHeadCas::Create`], so a concurrent birth loses the CAS
    /// instead of forking representation. Every row is written from these
    /// exact instances, so the head minted by `from_session` commits to the
    /// same bytes (the Rebase-arm reasoning in
    /// [`Self::write_head_canonical_document`]).
    async fn birth_head_canonical_document(
        &self,
        session: &meerkat_core::Session,
        write: &HeadCanonicalBirthWrite,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let id = session.id();
        let live = session.messages();
        let strand = meerkat_core::session_store::TranscriptStrandId::root();
        if !live.is_empty() {
            write
                .channel
                .append_messages(&self.write_cursor(id, &write.state), id, &strand, 0, live)
                .await?;
        }
        let head = meerkat_core::session_store::SessionHead::from_session(session, strand, 0)?;
        write
            .channel
            .save_head(
                &self.write_cursor(id, &write.state),
                &head,
                meerkat_core::session_store::SessionHeadCas::Create,
            )
            .await?;
        self.mark_session_durably_written(id);
        Ok(())
    }

    /// Persist an incoming document on the head-canonical path: delta rows
    /// through the delta channel, one small head row through CAS.
    ///
    /// This is the ONLY write path for a head-canonical session and it never
    /// encodes the whole document. What crosses the wire is exactly the
    /// appended messages, the head metadata, and the CAS token — the
    /// bounded checkpoint evidence the contract permits.
    async fn write_head_canonical_document(
        &self,
        session: &meerkat_core::Session,
        write: &HeadCanonicalWrite,
        shape: HeadCanonicalShape,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let id = session.id();
        let live = session.messages();
        let token = match meerkat_core::session_store::session_head_cas_token(&write.stored) {
            Ok(token) => token,
            Err(refusal)
                if write.stored.version
                    == super::contracts::RELEASED_0810_SESSION_ENVELOPE_VERSION =>
            {
                // A released 0.8.10 head with retained rewrites structurally
                // cannot authorize a current mutation (its rewrite-generation
                // authority predates the compact graph/rewrite-prefix
                // carriers), so every ordinary arm below is unreachable for
                // it - the 17/17 fleet boot failure at the first projected
                // boundary. The sanctioned adoption replaces the released
                // representation wholesale, authorized by the in-store
                // import proof instead of by the released head; genuine
                // divergence from the imported reading refuses typed inside
                // the store.
                tracing::info!(
                    session_id = %id,
                    refusal = %refusal,
                    "released head cannot authorize a current mutation; taking the sanctioned \
                     adoption lane"
                );
                write
                    .channel
                    .adopt_released_head_document(&self.write_cursor(id, &write.state), session)
                    .await?;
                self.mark_session_durably_written(id);
                return Ok(());
            }
            Err(refusal) => return Err(refusal),
        };
        let adopted = match shape {
            HeadCanonicalShape::PlainAppend => {
                let prev_count = usize::try_from(write.stored.message_count)
                    .map_err(|_| meerkat_store::SessionStoreError::Corrupted(id.clone()))?;
                let appended = &live[prev_count..];
                if !appended.is_empty() {
                    write
                        .channel
                        .append_messages(
                            &self.write_cursor(id, &write.state),
                            id,
                            &write.stored.strand,
                            write.stored.message_count,
                            appended,
                        )
                        .await?;
                }
                let strand = write.stored.strand.clone();
                // The successor head must commit to the EXACT durable row
                // bytes. Rows 0..prev_count keep the serialization they were
                // written with, which need not equal re-encoding the same
                // typed Messages today, so the stored commitment is EXTENDED
                // by only the appended rows' bytes - mirrors meerkat-core
                // `SessionHead::from_session_with_proved_inline_storage_authority`,
                // the published seam for "a retained runtime boundary whose
                // exact row bytes may use an older representation than
                // reserializing the same typed Messages today". Re-minting
                // via `from_session` breaks `SessionHead::into_session`'s
                // byte-exact prefix verification on the next cold
                // materialization.
                match write.stored.message_row_prefix.clone() {
                    Some(prefix) => {
                        let appended_serialized = appended
                            .iter()
                            .map(|message| {
                                serde_json::to_vec(message)
                                    .map_err(meerkat_store::SessionStoreError::from)
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        let proved = prefix.extend_serialized_rows(&appended_serialized)?;
                        meerkat_core::session_store::SessionHead::from_session_with_proved_inline_storage_authority(
                            session,
                            strand,
                            write.stored.rewrite_prefix.clone(),
                            proved,
                        )?
                    }
                    None => {
                        // A pre-0.8.11 head whose row identity was never
                        // proved stays unproved: inventing a commitment the
                        // stored rows may not satisfy would corrupt the next
                        // materialization instead of leaving it on the
                        // explicit full-verification conversion lane.
                        let mut head = meerkat_core::session_store::SessionHead::from_session(
                            session,
                            strand,
                            write.stored.rewrite_count,
                        )?;
                        head.message_row_prefix = None;
                        head.row_lineage_anchor = None;
                        head
                    }
                }
            }
            HeadCanonicalShape::Rebase(rebased) => {
                write
                    .channel
                    .append_messages(&self.write_cursor(id, &write.state), id, &rebased, 0, live)
                    .await?;
                // A fresh strand: every row was just written from these
                // exact instances, so the minted commitment matches the
                // durable bytes.
                meerkat_core::session_store::SessionHead::from_session(
                    session,
                    rebased,
                    write.stored.rewrite_count,
                )?
            }
        };
        // Exact-resave noop (the head-path mirror of the whole-blob
        // byte-equality probe): a document identical to the stored head
        // appended nothing above and would re-mint a checkpoint version for
        // zero durable change. The store-side probe additionally requires
        // the registered fence to be the CURRENT write authority — exactly
        // like the blob probe's `continuity.fencing_token = ?` predicate —
        // so a fenced-out writer's exact bytes never mask the stale-fence
        // refusal `save_head` below must surface.
        if head_equal_for_exact_resave(&adopted, &write.stored)
            && write
                .channel
                .session_head_matches_current(
                    &write.state.identity,
                    id,
                    write.state.generation,
                    write.state.fencing_token,
                    &write.stored,
                )
                .await?
        {
            self.mark_session_durably_written(id);
            return Ok(());
        }
        write
            .channel
            .save_head(
                &self.write_cursor(id, &write.state),
                &adopted,
                meerkat_core::session_store::SessionHeadCas::IfToken(token),
            )
            .await?;
        self.mark_session_durably_written(id);
        Ok(())
    }

    /// The head-canonical plain save.
    ///
    /// STEADY STATE (the ordinary turn): the incoming document extends the
    /// persisted head strand and carries no inline retained history. Those
    /// are exactly the two early-return arms of
    /// `head_canonical_plain_save_guard`, and both are decidable from the
    /// SMALL head row — the guard's `previous_revision` is
    /// `stored.head_revision` and its `prev_len` is `stored.message_count`,
    /// because the slim materialization it would compare against is derived
    /// from this very row (`SessionHead::into_session` fails closed
    /// otherwise). Proving them here is what keeps an ordinary turn off both
    /// the O(document) previous-materialization and the O(document) encode.
    ///
    /// Anything else — a transcript that does not extend the head strand, or
    /// a document still carrying its retained history inline — is not an
    /// ordinary turn. Those materialize the predecessor and run meerkat's own
    /// published guard, so this store mirror's accept/reject boundary stays
    /// identical to the session service's, and then rebase onto a fresh
    /// strand. Still no blob: what a rebase writes is strand rows plus a head.
    ///
    /// HONEST LIMIT on the inline-history arm: an in-memory session that has
    /// just compacted carries its retained bodies inline until its next
    /// resume, and the published guard's commit-ledger check needs the stored
    /// commits. The meta substrate contract exposes them only through
    /// `load_canonical_previous`, so those saves still pay one O(document)
    /// row materialization — the same cost this path paid unconditionally
    /// before. They never pay an O(document) ENCODE, and a session resumed
    /// from head rows (the steady state) pays neither.
    async fn save_head_canonical_document(
        &self,
        session: &meerkat_core::Session,
        write: HeadCanonicalWrite,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let id = session.id();
        session.validate_transcript_history_state().map_err(|err| {
            meerkat_store::SessionStoreError::InvalidTranscriptRewrite {
                id: id.clone(),
                reason: format!("incoming transcript history state is malformed: {err}"),
            }
        })?;
        // Metadata key probe, not a parse: parsing the graph value is
        // O(retained history) and the steady state has none.
        let carries_inline_history = session
            .metadata()
            .contains_key(meerkat_core::SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
        let shape = head_canonical_shape(session, &write.stored)?;
        if carries_inline_history || shape != HeadCanonicalShape::PlainAppend {
            // Single substrate snapshot over head, rows and the rewrite
            // ledger: a guard fed a document from one instant and a commit
            // list from another would be deciding about a state that never
            // existed. A snapshot that has moved since capability resolution
            // fails closed at the head CAS below, never silently.
            let Some((previous_slim, stored_commits)) =
                write.channel.load_canonical_previous(id).await?
            else {
                return Err(meerkat_store::SessionStoreError::Internal(format!(
                    "session {id} advertised a durable head row but the continuity substrate \
                     could not materialize its head-canonical predecessor; refusing to fall back \
                     to whole-document persistence"
                )));
            };
            meerkat_core::session_store::head_canonical_plain_save_guard(
                session,
                &previous_slim,
                &stored_commits,
            )?;
        }
        self.write_head_canonical_document(session, &write, shape)
            .await
    }

    /// Record + adopt a transcript rewrite on a head-canonical session.
    ///
    /// `commit_rewrite` writes the commit row plus the new strand's base
    /// rows; the following `save_head` adopts it. Both are delta/head/CAS
    /// verbs — the whole document is never encoded.
    async fn save_head_canonical_transcript_rewrite(
        &self,
        session: &meerkat_core::Session,
        commit: &meerkat_core::TranscriptRewriteCommit,
        write: HeadCanonicalWrite,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let id = session.id();
        let incoming_revision = session
            .transcript_content_digest()
            .map_err(meerkat_store::SessionStoreError::from)?;
        if incoming_revision != commit.revision {
            return Err(meerkat_store::SessionStoreError::InvalidTranscriptRewrite {
                id: id.clone(),
                reason: format!(
                    "incoming current transcript digest {incoming_revision} does not match \
                     commit revision {}",
                    commit.revision
                ),
            });
        }
        let record = rewrite_record_from_session_bodies(session, commit)?;
        let token = meerkat_core::session_store::session_head_cas_token(&write.stored)?;
        let next = write
            .channel
            .commit_rewrite(
                &self.write_cursor(id, &write.state),
                id,
                &record,
                meerkat_core::session_store::SessionHeadCas::IfToken(token.clone()),
            )
            .await?;
        // Adopt with the incoming session's envelope. `commit_rewrite` does
        // not advance the head, so the pre-commit token is still current.
        let adopted = meerkat_core::session_store::SessionHead::from_session(
            session,
            next.strand.clone(),
            next.rewrite_count,
        )?;
        write
            .channel
            .save_head(
                &self.write_cursor(id, &write.state),
                &adopted,
                meerkat_core::session_store::SessionHeadCas::IfToken(token),
            )
            .await?;
        self.mark_session_durably_written(id);
        Ok(())
    }

    /// Mint the next continuity write cursor for a registered session: the
    /// registered identity/generation/fence plus a version from the adapter's
    /// ONE allocator — the same allocator whole-blob saves mint from, so the
    /// durable checkpoint stream has a single writer.
    fn write_cursor(
        &self,
        session_id: &meerkat_core::types::SessionId,
        state: &SessionRuntimeState,
    ) -> super::contracts::ContinuityWriteCursor {
        super::contracts::ContinuityWriteCursor {
            identity: state.identity.clone(),
            generation: state.generation,
            checkpoint_version: super::types::CheckpointVersion::new(
                self.next_version(&session_id.to_string()),
            ),
            fencing_token: state.fencing_token,
        }
    }

    /// Drop every in-process trace of a session: its registry entry, its
    /// pending pre-registration bytes, its version counter, its markers —
    /// and its parked delta rows.
    ///
    /// The purge is INSIDE this function, not an obligation left to each
    /// call site. It used to be the latter, with the routing marker cleared
    /// here and the rows "reclaimed by the async purge at each call site":
    /// four of the six call sites never purged, so every session forgotten
    /// through a delete path leaked its parked transcript into the process's
    /// parked `MemoryStore` for the lifetime of the process. Clearing the
    /// marker without the rows is not a state anyone wants — it is
    /// unreachable-but-retained memory — so the two are no longer separable.
    ///
    /// Dropping parked rows here is safe precisely because every caller is
    /// abandoning the session (delete, unregister, retire), not registering
    /// it. The one place parked rows must survive is the registration flush,
    /// which never calls this: it purges only after a durable adoption.
    ///
    /// It deliberately does NOT touch [`SessionLifecycle`] authority.
    /// "Forgotten" and "removed" are different facts: unregistration forgets
    /// a session whose durable document is still there, while `delete`
    /// removes it. Only the removal verbs publish
    /// [`Self::mark_session_removed`], so a later park decision is made
    /// against what is actually durable, not against what this process
    /// happens to still remember.
    async fn forget_session(&self, session_id: &meerkat_core::types::SessionId) {
        let key = session_id.to_string();
        // Routing first: a forgotten session must stop being served from the
        // parked view immediately. `purge` clears the marker and the rows
        // together.
        self.parked_deltas.purge(session_id).await;
        self.session_registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key);
        self.pending_unregistered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key);
        self.versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key);
        self.suspended_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key);
    }

    /// Quiesce persistence for one registered session. Acquiring the existing
    /// per-session lock is the drain barrier: every save admitted before this
    /// call completes first, and every later mutation observes `Suspended`.
    #[cfg(test)]
    pub(crate) async fn suspend_session(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), super::types::ContinuityStoreError> {
        let _guard = self.lock_session(session_id).await;
        self.suspended_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.to_string());
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn unregister_session(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), super::types::ContinuityStoreError> {
        let _guard = self.lock_session(session_id).await;
        self.forget_session(session_id).await;
        self.superseded_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&session_id.to_string());
        self.unregistered_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.to_string());
        Ok(())
    }

    /// Permanently abandon a reset-superseded session after the replacement
    /// continuity generation has committed.
    ///
    /// Reset changes the identity-keyed continuity head before the old Mob
    /// member can be retired. Leaving the old snapshot behind makes Meerkat's
    /// archive path try to save that stale generation through the new fence,
    /// which correctly fails and retains a `Retiring` cleanup anchor. Delete
    /// the exact old snapshot under its document CAS first, then publish a
    /// superseded tombstone. The exact Mob retirement retry may acknowledge
    /// its generated terminal archive write without crossing the replacement
    /// fence; ordinary unregistration clears the tombstone only after the
    /// physical roster anchor is structurally absent.
    #[cfg(test)]
    pub(crate) async fn abandon_superseded_session(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let _guard = self.lock_session(session_id).await;
        let session_key = session_id.to_string();

        // Quiesce every later projection write before observing and deleting
        // the old document. Keep suspension in place on failure so a retry
        // cannot race a stale runtime save.
        self.suspended_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_key.clone());

        if let Some(session) = self.load_persisted_session(session_id).await? {
            let current_revision = self.projection_compare_token(&session)?;
            let deleted = self
                .store
                .delete_session_snapshot_if_current_revision(session_id, &current_revision)
                .await
                .map_err(|error| {
                    meerkat_store::SessionStoreError::Internal(format!(
                        "continuity abandon superseded session: {error}"
                    ))
                })?;
            if !deleted {
                return Err(meerkat_store::SessionStoreError::Internal(format!(
                    "continuity abandon did not delete superseded session snapshot {session_id}"
                )));
            }
        }

        self.forget_session(session_id).await;
        // Reset abandonment IS a removal: the old document is gone and must
        // never come back through a parked replay, even after the superseded
        // tombstone is cleared by ordinary unregistration.
        self.mark_session_removed(session_id);
        self.superseded_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_key);
        Ok(())
    }

    fn session_was_superseded(&self, session_id: &meerkat_core::types::SessionId) -> bool {
        self.superseded_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&session_id.to_string())
    }

    fn session_was_unregistered(&self, session_id: &meerkat_core::types::SessionId) -> bool {
        self.unregistered_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&session_id.to_string())
    }

    fn session_was_suspended(&self, session_id: &meerkat_core::types::SessionId) -> bool {
        self.suspended_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&session_id.to_string())
    }

    fn ensure_session_mutation_allowed(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        if self.session_was_unregistered(session_id) {
            return Err(meerkat_store::SessionStoreError::Internal(format!(
                "session {session_id} was unregistered from identity runtime state"
            )));
        }
        if self.session_was_suspended(session_id) {
            return Err(meerkat_store::SessionStoreError::Internal(format!(
                "session {session_id} persistence is suspended during identity authority rotation"
            )));
        }
        Ok(())
    }

    /// The PROJECTION doors' variant of
    /// [`Self::ensure_session_mutation_allowed`] (task #56 corpus finding,
    /// a production member): `save_authoritative_projection*` and
    /// `save_transcript_rewrite` carry STORE-ISSUED committed runtime
    /// authority by trait contract, and the durable-tear reconciliation must
    /// be able to repair a PARKED member's durable head - a member that
    /// holds no identity-runtime registration by design. A blanket
    /// unregistered refusal deadlocks repair against registration: the
    /// member cannot register until its torn row resumes, and the row
    /// cannot be repaired until the member registers.
    ///
    /// The refusal keeps protecting what it actually protects. A session
    /// whose durable document was REMOVED (delete/reset/rotation) or that
    /// never had one (creation window) still refuses - a projection there
    /// would resurrect a removed session or mint durable state for a dead
    /// incarnation - and suspension (identity authority rotation in flight)
    /// still refuses everything. Only the durable-observed, non-superseded
    /// current head - the parked-repair shape - is admitted, loudly.
    async fn ensure_projection_repair_allowed(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        if self.session_was_suspended(session_id) {
            return Err(meerkat_store::SessionStoreError::Internal(format!(
                "session {session_id} persistence is suspended during identity authority rotation"
            )));
        }
        if !self.session_was_unregistered(session_id) {
            return Ok(());
        }
        match self.resolve_session_lifecycle(session_id).await? {
            SessionLifecycle::DurableObserved => {
                // The archive wall stays up: an ARCHIVED durable document
                // (readable, lifecycle terminal present) keeps the
                // registration requirement - a raw spawn/resume bypass must
                // not extend an archive through the projection doors (the
                // recorded refusal in identity_first_head_canonical_resume).
                // A live head admits the repair; an UNREADABLE head - the
                // torn row this seam exists to fix - admits it too, because
                // archives read fine and a failing read is precisely the
                // tear under repair.
                if let Ok(Some(durable)) = self.load_persisted_session(session_id).await
                    && durable.lifecycle_terminal().is_some()
                {
                    return Err(meerkat_store::SessionStoreError::Internal(format!(
                        "session {session_id} was unregistered from identity runtime state \
                         and its durable document is terminal (archived); a projection here \
                         requires identity registration"
                    )));
                }
                tracing::info!(
                    session_id = %session_id,
                    "unregistered-session projection admitted as durable-head repair \
                     (parked member: committed runtime authority projecting into the \
                     current durable row)"
                );
                Ok(())
            }
            SessionLifecycle::CreationWindow => {
                Err(meerkat_store::SessionStoreError::Internal(format!(
                    "session {session_id} was unregistered from identity runtime state and \
                     has no durable document; refusing to mint durable state for a dead \
                     incarnation"
                )))
            }
            SessionLifecycle::Removed => Err(meerkat_store::SessionStoreError::Internal(format!(
                "session {session_id} was unregistered from identity runtime state and \
                     its durable document was removed; refusing to resurrect it"
            ))),
        }
    }

    /// Get the next checkpoint version for a session, starting at 1.
    fn next_version(&self, session_id: &str) -> u64 {
        let mut map = self
            .versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let counter = map
            .entry(session_id.to_string())
            .or_insert_with(|| AtomicU64::new(0));
        counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn restore_registration_state(
        &self,
        session_id: &meerkat_core::types::SessionId,
        previous_registry: Option<SessionRuntimeState>,
    ) {
        let key = session_id.to_string();
        {
            let mut registry = self
                .session_registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match previous_registry {
                Some(state) => {
                    registry.insert(key, state);
                }
                None => {
                    registry.remove(&key);
                }
            }
        }
    }

    fn current_version(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> super::types::CheckpointVersion {
        let map = self
            .versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let version = map
            .get(&session_id.to_string())
            .map(|counter| counter.load(Ordering::Relaxed))
            .unwrap_or(0);
        super::types::CheckpointVersion::new(version)
    }

    /// Look up the runtime state for a session.
    fn lookup_session(&self, session_id: &str) -> Option<SessionRuntimeState> {
        let registry = self
            .session_registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry.get(session_id).cloned()
    }

    /// The durable document, decoded. Head-first: a head-canonical session
    /// materializes straight from head+rows, so the guard/CAS/delete paths
    /// that need only the document skip the substrate's
    /// serialize-the-slim-materialization + parse-it-back round trip that
    /// the whole-snapshot read verb has to perform. Identical value either
    /// way — the verb serializes exactly this materialization.
    async fn load_persisted_session(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::Session>, meerkat_store::SessionStoreError> {
        if let Some(session) = self.load_head_canonical_session(id).await? {
            // Representation+count trace (task #61, a downstream app cold-mint
            // 839-vs-851): names which durable representation each load
            // served, so a mint-vs-save-guard split is attributable from a
            // debug log instead of a store dump.
            tracing::debug!(
                session_id = %id,
                representation = "head_canonical",
                message_count = session.messages().len(),
                "durable session load"
            );
            return Ok(Some(session));
        }
        let fallback = self
            .load_persisted_session_with_bytes(id)
            .await?
            .map(|(session, _)| session);
        if let Some(session) = fallback.as_ref() {
            tracing::debug!(
                session_id = %id,
                representation = "whole_blob_fallback",
                message_count = session.messages().len(),
                "durable session load"
            );
        }
        // Falling back is the ordinary path for every blob-canonical session,
        // so it is not worth a log line by itself. What IS worth one: a
        // durable head row existing for this session while the caller is
        // handed the whole-snapshot view (or nothing) — the reader may be
        // acting on a document OLDER than the committed head, the advisory's
        // stale-read shape. The probe is one indexed point-read on a path
        // already paying a full blob read + parse, and it is diagnostic only:
        // a probe failure must not fail the load.
        if let Some(incremental) = self.incremental.as_ref() {
            match incremental.load_canonical_head(id).await {
                Ok(Some(_)) => tracing::warn!(
                    session_id = %id,
                    served_blob = fallback.is_some(),
                    "head-canonical read declined while a durable head row exists; \
                     serving the whole-snapshot fallback"
                ),
                Ok(None) => {}
                Err(error) => tracing::debug!(
                    session_id = %id,
                    %error,
                    "head-row probe failed during whole-snapshot fallback"
                ),
            }
        }
        Ok(fallback)
    }

    /// Head-first materialization for head-canonical sessions: one row-parse
    /// pass over the head-covered messages, no whole-document serialize +
    /// reparse round trip, and no inline transcript-history metadata (so the
    /// decode-time retained-history validation storm never triggers).
    /// `Ok(None)` means the session is still blob-canonical and the caller
    /// must take the legacy path, which keeps H3's byte custody intact.
    ///
    /// The head and its rows are read through the substrate's single-snapshot
    /// [`super::contracts::ContinuityIncrementalSessions::load_canonical_session`],
    /// never as `load_canonical_head` + `load_messages`: this replaced the
    /// whole-snapshot verb's ONE transactional substrate read, and it has to
    /// stay one, or a concurrent delta write between the two halves would
    /// materialize a torn document.
    async fn load_head_canonical_session(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::Session>, meerkat_store::SessionStoreError> {
        let Some(incremental) = self.incremental.as_ref() else {
            // A head-canonical deployment losing its delta channel is a real
            // fault: every head-canonical session on it becomes unreadable
            // through this path. But a blob-only store has no channel from
            // construction and hits this on EVERY read, so the fault is loud
            // exactly once per adapter and debug thereafter — one line keeps
            // the signal, repeats only drown other warnings.
            if self
                .no_incremental_channel_warned
                .swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                tracing::debug!(
                    session_id = %id,
                    gate = "no_incremental_channel",
                    "head-canonical read declined"
                );
            } else {
                tracing::warn!(
                    session_id = %id,
                    gate = "no_incremental_channel",
                    "head-canonical read declined (further declines log at debug)"
                );
            }
            return Ok(None);
        };
        if self.parked_deltas.is_parked(id) {
            // Pre-registration state is not durable authority. A legitimate
            // window in every member creation, so debug.
            tracing::debug!(session_id = %id, gate = "parked", "head-canonical read declined");
            return Ok(None);
        }
        let Some(session) = incremental.load_canonical_session(id).await? else {
            // Blob-canonical sessions land here on every load; legitimate.
            tracing::debug!(session_id = %id, gate = "no_head_row", "head-canonical read declined");
            return Ok(None);
        };
        if session.id() != id {
            return Err(meerkat_store::SessionStoreError::Serialization(format!(
                "continuity head row {id} materializes session {}",
                session.id()
            )));
        }
        Ok(Some(session))
    }

    /// Load the durable snapshot, returning both the decoded document and the
    /// exact raw bytes. Lazy checkpoint adoption stamps a legacy document by
    /// taking byte custody of the source BLOB, so the decoded form alone is
    /// not enough.
    async fn load_persisted_session_with_bytes(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<(meerkat_core::Session, Vec<u8>)>, meerkat_store::SessionStoreError> {
        let snapshot = self.store.load_session_snapshot(id).await.map_err(|e| {
            meerkat_store::SessionStoreError::Internal(format!("continuity load: {e}"))
        })?;
        match snapshot {
            Some(snap) => {
                // Paired decode: adopts the digest midstates the producer
                // recorded for these exact bytes, so resume-side guards and
                // checkpoint verifications stay O(delta) instead of paying
                // an O(document) reseed per read (flat-curve boundary).
                let (session, data) = match meerkat_core::Session::from_persisted_bytes(&snap.data)
                {
                    Ok(session) => (session, snap.data),
                    Err(decode_error) => {
                        // Released 0.8.10 envelope (v2): interpretable only
                        // through the explicit one-time importer. This is the
                        // one path EVERY external whole-blob store traverses,
                        // so the import-on-load lives here rather than in
                        // each store (production field finding: an external corpus
                        // of v2 rows was otherwise unreadable at turn-time
                        // resume).
                        self.import_released_snapshot_on_load(id, snap.data, &decode_error)
                            .await?
                    }
                };
                if session.id() != id {
                    return Err(meerkat_store::SessionStoreError::Serialization(format!(
                        "continuity snapshot key {id} contains session {}",
                        session.id()
                    )));
                }
                Ok(Some((session, data)))
            }
            None => Ok(None),
        }
    }

    /// One-time import of a released 0.8.10 session envelope observed on the
    /// continuity load path, with durable current-format adoption.
    ///
    /// Per the banked 0.8.11 import contract: the public core importer is the
    /// sole boundary allowed to interpret released evidence; the non-Clone
    /// receipt is consumed by the adoption; the source blob SHA is re-proved
    /// against the exact bytes read; nothing ever mints the retired
    /// vocabulary; every proof failure fails closed (the original decode
    /// refusal is surfaced, never a healed reading).
    ///
    /// External stores expose no transaction, so the same-transaction
    /// requirement maps to this adapter's own write discipline: a REGISTERED
    /// session adopts durably through the registered-cursor CAS write
    /// (version advance under the same generation/fence — the write every
    /// later read observes, making the second load take the already-current
    /// path). A not-yet-registered session serves the imported document
    /// without adoption; the durable conversion lands with registration or
    /// the first registered write, exactly like every other pre-registration
    /// write on this adapter.
    async fn import_released_snapshot_on_load(
        &self,
        id: &meerkat_core::types::SessionId,
        source: Vec<u8>,
        decode_error: &serde_json::Error,
    ) -> Result<(meerkat_core::Session, Vec<u8>), meerkat_store::SessionStoreError> {
        use sha2::Digest as _;

        let imported = meerkat_core::import_released_0810_session(&source).map_err(|import| {
            meerkat_store::SessionStoreError::Serialization(format!(
                "continuity snapshot {id} decodes neither as a current document \
                 ({decode_error}) nor as a released 0.8.10 envelope ({import})"
            ))
        })?;
        let (session, receipt) = imported.into_parts();
        let observed_sha256: [u8; 32] = sha2::Sha256::digest(&source).into();
        if receipt.source_document_sha256() != &observed_sha256 {
            return Err(meerkat_store::SessionStoreError::Serialization(format!(
                "continuity snapshot {id} changed during exact released-0.8.10 import"
            )));
        }
        if receipt.session_id() != id {
            return Err(meerkat_store::SessionStoreError::Serialization(format!(
                "continuity snapshot key {id} contains released session {}",
                receipt.session_id()
            )));
        }
        let data = self.encode_whole_document(&session, WholeDocumentPass::BlobCanonicalPersist)?;
        match self.lookup_session(&id.to_string()) {
            Some(state) => {
                // The receipt is consumed by this durable adoption: the CAS
                // write below is the store-authorized conversion, after which
                // every load decodes current bytes directly.
                drop(receipt);
                self.save_registered_snapshot(id, data.clone(), state)
                    .await?;
                tracing::info!(
                    session_id = %id,
                    source_bytes = source.len(),
                    current_bytes = data.len(),
                    "released 0.8.10 session envelope imported and durably adopted on load"
                );
            }
            None => {
                tracing::info!(
                    session_id = %id,
                    "released 0.8.10 session envelope imported for an unregistered read; \
                     durable adoption follows the first registered write"
                );
            }
        }
        Ok((session, data))
    }

    async fn load_previous_session_for_save(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::Session>, meerkat_store::SessionStoreError> {
        if let Some(session) = self.load_persisted_session(id).await? {
            // Companion to the load_persisted_session representation trace
            // (task #61): this is the document the save guard compares the
            // incoming session against, so a mint that materialized from a
            // DIFFERENT representation shows up as a count split between
            // these two debug lines.
            tracing::debug!(
                session_id = %id,
                lane = "save_guard_previous",
                message_count = session.messages().len(),
                "previous session resolved for save"
            );
            return Ok(Some(session));
        }
        let pending = self
            .pending_unregistered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id.to_string())
            .cloned();
        pending
            .map(|data| {
                meerkat_core::Session::from_persisted_bytes(&data)
                    .map_err(|e| meerkat_store::SessionStoreError::Serialization(e.to_string()))
            })
            .transpose()
    }

    /// The lifecycle state this adapter has already established for a
    /// session, or `None` when it has observed nothing yet.
    fn recorded_lifecycle(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<SessionLifecycle> {
        self.session_lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&session_id.to_string())
            .copied()
    }

    fn publish_lifecycle(
        &self,
        session_id: &meerkat_core::types::SessionId,
        state: SessionLifecycle,
    ) {
        self.session_lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.to_string(), state);
    }

    /// Record that a durable write for this session has been ACKNOWLEDGED.
    ///
    /// Call sites must invoke this only after the substrate returned `Ok`.
    /// Marking before the await published a durability claim the write might
    /// never make good on: a failed durable write left the session
    /// `DurableObserved`, and its perfectly legal creation-window park was
    /// refused afterwards.
    fn mark_session_durably_written(&self, session_id: &meerkat_core::types::SessionId) {
        self.publish_lifecycle(session_id, SessionLifecycle::DurableObserved);
    }

    /// The projection body, under the caller-held session lock (see
    /// [`Self::save_authoritative_projection`]).
    async fn save_authoritative_projection_locked(
        &self,
        session: &meerkat_core::Session,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        // Capability before serialization, same rule as `save`. The
        // authoritative projection asserts that the incoming document IS the
        // truth (the service's rollback repair), so there is no save guard —
        // exactly as meerkat-store's head-canonical path — but it still lands
        // as delta rows plus a small head, never as a blob.
        match self.resolve_persistence_capability(session.id()).await? {
            PersistenceCapability::HeadCanonical(write) => {
                let shape = head_canonical_shape(session, &write.stored)?;
                return self
                    .write_head_canonical_document(session, &write, shape)
                    .await;
            }
            PersistenceCapability::HeadCanonicalBirth(write) => {
                return self.birth_head_canonical_document(session, &write).await;
            }
            PersistenceCapability::BlobCanonical => {}
        }
        let data = self.encode_whole_document(session, WholeDocumentPass::BlobCanonicalPersist)?;
        let sid_str = session.id().to_string();
        match self.lookup_session(&sid_str) {
            Some(state) => {
                self.save_registered_snapshot(session.id(), data, state)
                    .await?;
            }
            None => {
                self.ensure_unregistered_park_allowed(session.id()).await?;
                let mut pending = self
                    .pending_unregistered
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                pending.insert(sid_str, data);
            }
        }
        Ok(())
    }

    /// Absorb a projection write refused because the identity's continuity
    /// record has ADVANCED PAST this session (the LIVE reset/reprofile
    /// shape).
    ///
    /// Live reset replaces the identity's record (new generation, new
    /// session) FIRST and retires the superseded runtime through deferred
    /// cleanup debt afterwards — deliberately, per the reset contract: the
    /// old bridge projection is rollback authority until the replacement
    /// commits, and reset must not wait on a hung old retire
    /// (`reset_records_exact_cleanup_debt_without_waiting_for_hung_old_retire`).
    /// In that window the OLD runtime session is still live, and any
    /// boundary it commits fails its durable projection with the store's
    /// cursor refusal — which, propagated, fails the committing verb,
    /// escalates the runtime to repair-blocked retention, wedges the
    /// deferred retire behind it, and blows the gateway's bounded shutdown
    /// horizon (the PR #304 CI wedge).
    ///
    /// A record that names a NEWER binding for the same identity IS the
    /// supersede fact, discovered lazily under the identity fence — the
    /// same fact `abandon_superseded_session` records eagerly. The absorbed
    /// write takes the exact semantics the superseded-session pins already
    /// establish (terminal writes drop without parking). Deliberately NO
    /// persistent supersede mark is set: a reset that FAILS after the
    /// record replacement rolls the record back, after which this session's
    /// cursor enforces cleanly again — a lingering mark would silently drop
    /// its writes forever. Every other projection failure propagates
    /// fail-closed exactly as before.
    async fn absorb_projection_superseded_by_identity_advance(
        &self,
        session_id: &meerkat_core::types::SessionId,
        result: Result<(), meerkat_store::SessionStoreError>,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let Err(error) = result else {
            return result;
        };
        let Some(state) = self.lookup_session(&session_id.to_string()) else {
            return Err(error);
        };
        let resolved = match self
            .store
            .resolve_many(std::slice::from_ref(&state.identity))
            .await
        {
            Ok(resolved) => resolved,
            Err(resolve_error) => {
                tracing::warn!(
                    session_id = %session_id,
                    identity = %state.identity,
                    error = %resolve_error,
                    "projection supersede probe could not resolve the identity record; \
                     surfacing the original projection failure"
                );
                return Err(error);
            }
        };
        let advanced_past_this_session = match resolved.get(&state.identity) {
            Some(super::types::ContinuityResolveState::Ready { record }) => {
                record.session_id != *session_id || record.generation.get() > state.generation.get()
            }
            _ => false,
        };
        if !advanced_past_this_session {
            return Err(error);
        }
        tracing::info!(
            session_id = %session_id,
            identity = %state.identity,
            refused = %error,
            "durable projection dropped: the identity's continuity record has advanced past \
             this session (live reset supersede); the superseded runtime retires through the \
             reset cleanup debt"
        );
        Ok(())
    }

    /// Record positive durable-evidence authority from an OBSERVATION — a
    /// head row read back from the substrate — as opposed to a write this
    /// process acknowledged.
    ///
    /// `or_insert`, not `insert`: evidence established while the observing
    /// read was in flight (an acknowledged write, a removal) is newer than
    /// the read and must win, exactly as the probe in
    /// [`Self::resolve_session_lifecycle`] yields.
    fn mark_session_durably_observed(&self, session_id: &meerkat_core::types::SessionId) {
        self.session_lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(session_id.to_string())
            .or_insert(SessionLifecycle::DurableObserved);
    }

    /// Record that this session's durable document was explicitly removed.
    ///
    /// This is the deletion marker whose absence made a cold-restored session
    /// resurrectable: without it, a delete left no trace, the next
    /// unregistered save read as a creation-window park, and the following
    /// registration flushed the removed document back into existence.
    fn mark_session_removed(&self, session_id: &meerkat_core::types::SessionId) {
        self.publish_lifecycle(session_id, SessionLifecycle::Removed);
    }

    /// Does a durable document exist for this session on the substrate?
    ///
    /// Cheap probe first (the indexed head row), whole-snapshot lookup only
    /// when there is no head row. O(document) in the blob case — deliberately
    /// accepted, because this runs only on the pre-registration park path,
    /// never on a head-canonical steady-state turn.
    async fn durable_document_exists(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, meerkat_store::SessionStoreError> {
        if let Some(incremental) = self.incremental.as_ref()
            && incremental.load_canonical_head(session_id).await?.is_some()
        {
            return Ok(true);
        }
        let snapshot = self
            .store
            .load_session_snapshot(session_id)
            .await
            .map_err(|e| {
                meerkat_store::SessionStoreError::Internal(format!(
                    "continuity durable-evidence probe: {e}"
                ))
            })?;
        Ok(snapshot.is_some())
    }

    /// Positive lifecycle authority for one session (see [`SessionLifecycle`]).
    ///
    /// Established state wins; otherwise the substrate is probed exactly once
    /// and the verdict recorded. Only this function may turn "nothing
    /// observed" into `CreationWindow`, and only against durable evidence
    /// that no document exists.
    async fn resolve_session_lifecycle(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionLifecycle, meerkat_store::SessionStoreError> {
        if let Some(state) = self.recorded_lifecycle(session_id) {
            return Ok(state);
        }
        let probed = if self.durable_document_exists(session_id).await? {
            SessionLifecycle::DurableObserved
        } else {
            SessionLifecycle::CreationWindow
        };
        let mut states = self
            .session_lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // `or_insert`, not `insert`: a durable write or a removal that landed
        // while the probe was in flight is newer evidence than the probe.
        Ok(*states.entry(session_id.to_string()).or_insert(probed))
    }

    /// The parking guard: a write may park only inside the genuine creation
    /// window.
    ///
    /// Parking is correct exactly for a session with NO durable document —
    /// the pre-registration window member creation depends on. A session that
    /// reaches a park site with durable state (no registry entry, no
    /// unregistered/suspended/superseded marker) was dropped by a delete or
    /// reset path, or belongs to another actor: parking its write
    /// acknowledges bytes that either die with the process (acknowledged
    /// write loss) or flush durably at the next `register_session` and
    /// resurrect the removed document. ONE helper for the delta route and
    /// every whole-blob save verb, so the five park sites cannot drift and
    /// operators see one refusal vocabulary.
    async fn ensure_unregistered_park_allowed(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        match self.resolve_session_lifecycle(session_id).await? {
            SessionLifecycle::CreationWindow => Ok(()),
            SessionLifecycle::DurableObserved => {
                Err(meerkat_store::SessionStoreError::Internal(format!(
                    "session {session_id} is no longer registered but has durable continuity \
                     state (written in this process, or observed on the substrate at restore); \
                     refusing to park a later write, which would strand those rows behind an \
                     unadvanced head or resurrect a removed session on the next flush"
                )))
            }
            SessionLifecycle::Removed => Err(meerkat_store::SessionStoreError::Internal(format!(
                "session {session_id} is no longer registered and its durable continuity \
                     document was removed (unregister, delete, or reset); refusing to park a \
                     later write, which would resurrect a removed session on the next flush"
            ))),
        }
    }

    /// The whole-document plain save, for sessions whose canonical durable
    /// representation genuinely IS the blob (no delta channel, or a session
    /// that has never taken a delta write).
    ///
    /// Serializing up front is correct HERE and only here: the blob is what
    /// gets written, and the exact-bytes match probe below is what turns a
    /// no-op resave into a no-op without loading and reparsing the previous
    /// document.
    async fn save_blob_canonical_document(
        &self,
        session: &meerkat_core::Session,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let snapshot = Arc::new(super::types::SessionSnapshot {
            data: self.encode_whole_document(session, WholeDocumentPass::BlobCanonicalPersist)?,
        });
        let sid_str = session.id().to_string();
        let state = self.lookup_session(&sid_str);
        if let Some(state) = state.as_ref() {
            let candidate = SessionSnapshotMatchCandidate {
                identity: state.identity.clone(),
                session_id: session.id().clone(),
                generation: state.generation,
                checkpoint_version: self.current_version(session.id()),
                fencing_token: state.fencing_token,
                snapshot: snapshot.clone(),
            };
            let matches = self
                .store
                .session_snapshot_matches_current(candidate)
                .await
                .map_err(|e| {
                    meerkat_store::SessionStoreError::Internal(format!(
                        "continuity snapshot match: {e}"
                    ))
                })?;
            if matches {
                meerkat_core::session_store::append_only_save_guard(session, Some(session))?;
                return Ok(());
            }
        }

        let previous = self.load_previous_session_for_save(session.id()).await?;
        // Meerkat 0.8.2's PersistentSessionService owns projection rollback
        // and invokes the authoritative-projection CAS seam when its
        // generated classifier permits repair. This adapter remains a strict
        // store; it must not run a second recovery classifier.
        meerkat_core::session_store::append_only_save_guard(session, previous.as_ref())?;
        let snapshot = Arc::try_unwrap(snapshot).unwrap_or_else(|snapshot| (*snapshot).clone());
        let data = snapshot.data;

        // Use real identity/generation/fencing from the runtime registry.
        match state {
            Some(state) => {
                self.save_registered_snapshot(session.id(), data, state)
                    .await?;
            }
            None => {
                // PersistentSessionService can save during member creation
                // before the bridge call returns. Hold that first snapshot
                // until the identity runtime registers the real owner; never
                // checkpoint under a synthetic `_session:*` identity. Only
                // for that creation window, though: a session with durable
                // state and no registry entry was removed (delete/reset) or
                // belongs to another actor, and parking its snapshot would
                // acknowledge a write that can only be lost with the process
                // or resurrected by the next registration flush.
                self.ensure_unregistered_park_allowed(session.id()).await?;
                tracing::warn!(
                    session_id = %sid_str,
                    "ContinuitySessionStoreAdapter: delaying save until runtime state is registered"
                );
                let mut pending = self
                    .pending_unregistered
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                pending.insert(sid_str, data);
            }
        }
        Ok(())
    }

    async fn save_registered_snapshot(
        &self,
        session_id: &meerkat_core::types::SessionId,
        data: Vec<u8>,
        state: SessionRuntimeState,
    ) -> Result<super::types::CheckpointVersion, meerkat_store::SessionStoreError> {
        let version = self.next_version(&session_id.to_string());
        let checkpoint_version = super::types::CheckpointVersion::new(version);
        let snapshot = super::types::SessionSnapshot { data };
        self.store
            .save_session_snapshot_owned(
                state.identity,
                session_id.clone(),
                state.generation,
                checkpoint_version,
                state.fencing_token,
                snapshot,
            )
            .await
            .map_err(|e| {
                meerkat_store::SessionStoreError::Internal(format!("continuity save: {e}"))
            })?;
        self.mark_session_durably_written(session_id);
        Ok(checkpoint_version)
    }
}

#[async_trait]
impl meerkat::SessionStore for ContinuitySessionStoreAdapter {
    async fn save(
        &self,
        session: &meerkat_core::Session,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let _guard = self.lock_session(session.id()).await;
        if self.session_was_superseded(session.id()) {
            // The old runtime has already been quiesced by the first retire
            // attempt. Meerkat's retry still realizes its generated Archived
            // document action before removing the retained roster anchor, but
            // reset deliberately discarded this document when it committed
            // the replacement generation. Acknowledge that terminal write
            // without crossing the new identity fence.
            return Ok(());
        }
        self.ensure_session_mutation_allowed(session.id())?;

        // CAPABILITY FIRST, SERIALIZATION SECOND. This used to be the other
        // way round: every save opened with `Session::to_persisted_bytes()`
        // and only then asked whether the substrate could take the write
        // incrementally. Continuity WRITE VOLUME was O(delta) while the path
        // stayed O(document) in serialization, hashing, allocation and
        // copies — which made the O(delta) claim materially wider than the
        // truth. The head-canonical branch below never encodes a document.
        match self.resolve_persistence_capability(session.id()).await? {
            PersistenceCapability::HeadCanonical(write) => {
                self.save_head_canonical_document(session, *write).await
            }
            PersistenceCapability::HeadCanonicalBirth(write) => {
                self.birth_head_canonical_document(session, &write).await
            }
            PersistenceCapability::BlobCanonical => {
                self.save_blob_canonical_document(session).await
            }
        }
    }

    async fn save_transcript_rewrite(
        &self,
        session: &meerkat_core::Session,
        commit: &meerkat_core::TranscriptRewriteCommit,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let _guard = self.lock_session(session.id()).await;
        if self.session_was_superseded(session.id()) {
            return Ok(());
        }
        self.ensure_projection_repair_allowed(session.id()).await?;
        // Head-canonical sessions keep retained history out-of-line, so a
        // rewrite is `commit_rewrite -> adopt`, not a whole-document write:
        // routing it through the whole-document verb would rebase the live
        // strand and silently drop the commit record. Mirrors meerkat-store's
        // own head-canonical `save_transcript_rewrite`.
        match self.resolve_persistence_capability(session.id()).await? {
            PersistenceCapability::HeadCanonical(write) => {
                return self
                    .save_head_canonical_transcript_rewrite(session, commit, *write)
                    .await;
            }
            // A rewrite as a session's FIRST persistence has no head to
            // rewrite; it lands whole-document below (guarded against the
            // absent predecessor) rather than fabricating a birth-plus-
            // rewrite composite.
            PersistenceCapability::HeadCanonicalBirth(_) => {}
            PersistenceCapability::BlobCanonical => {}
        }
        let previous = self.load_previous_session_for_save(session.id()).await?;
        meerkat_core::session_store::transcript_rewrite_save_guard(
            session,
            previous.as_ref(),
            commit,
        )?;
        let data = self.encode_whole_document(session, WholeDocumentPass::BlobCanonicalPersist)?;
        let sid_str = session.id().to_string();

        match self.lookup_session(&sid_str) {
            Some(state) => {
                self.save_registered_snapshot(session.id(), data, state)
                    .await?;
            }
            None => {
                self.ensure_unregistered_park_allowed(session.id()).await?;
                let mut pending = self
                    .pending_unregistered
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                pending.insert(sid_str, data);
            }
        }
        Ok(())
    }

    async fn save_authoritative_projection(
        &self,
        session: &meerkat_core::Session,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let _guard = self.lock_session(session.id()).await;
        if self.session_was_superseded(session.id()) {
            return Ok(());
        }
        self.ensure_projection_repair_allowed(session.id()).await?;
        let result = self.save_authoritative_projection_locked(session).await;
        self.absorb_projection_superseded_by_identity_advance(session.id(), result)
            .await
    }

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &meerkat_core::Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let _guard = self.lock_session(session.id()).await;
        if self.session_was_superseded(session.id()) {
            return Ok(());
        }
        self.ensure_projection_repair_allowed(session.id()).await?;
        // A recovery/repair verb, not an ordinary turn: the compare token IS
        // `sha256(serialize(previous document))`, so this path is
        // O(document) by the CAS contract itself. What it must NOT do is add
        // a second whole-document pass by encoding the INCOMING document for
        // a session the substrate stores as head + rows.
        match self.resolve_persistence_capability(session.id()).await? {
            PersistenceCapability::HeadCanonical(write) => {
                let id = session.id();
                let Some((previous_slim, _stored_commits)) =
                    write.channel.load_canonical_previous(id).await?
                else {
                    return Err(meerkat_store::SessionStoreError::Internal(format!(
                        "session {id} advertised a durable head row but the continuity substrate \
                         could not materialize its head-canonical predecessor; refusing to fall \
                         back to whole-document persistence"
                    )));
                };
                meerkat_core::session_store::authoritative_projection_current_revision_guard(
                    session,
                    Some(&previous_slim),
                    expected_current_revision.as_deref(),
                )?;
                let shape = head_canonical_shape(session, &write.stored)?;
                return self
                    .write_head_canonical_document(session, &write, shape)
                    .await;
            }
            PersistenceCapability::HeadCanonicalBirth(write) => {
                // No durable representation exists: the CAS predicate must
                // expect exactly that, then the write births head-canonical.
                meerkat_core::session_store::authoritative_projection_current_revision_guard(
                    session,
                    None,
                    expected_current_revision.as_deref(),
                )?;
                return self.birth_head_canonical_document(session, &write).await;
            }
            PersistenceCapability::BlobCanonical => {}
        }
        // This is a compare-and-set against the visible durable projection.
        // Pre-registration pending bytes are not a durable current revision
        // and must not influence the expected `None`/revision predicate.
        let previous = self.load_persisted_session(session.id()).await?;
        meerkat_core::session_store::authoritative_projection_current_revision_guard(
            session,
            previous.as_ref(),
            expected_current_revision.as_deref(),
        )?;
        let data = self.encode_whole_document(session, WholeDocumentPass::BlobCanonicalPersist)?;
        let sid_str = session.id().to_string();
        match self.lookup_session(&sid_str) {
            Some(state) => {
                self.save_registered_snapshot(session.id(), data, state)
                    .await?;
                Ok(())
            }
            None => {
                self.ensure_unregistered_park_allowed(session.id()).await?;
                let mut pending = self
                    .pending_unregistered
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                pending.insert(sid_str, data);
                Ok(())
            }
        }
    }

    async fn load(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::Session>, meerkat_store::SessionStoreError> {
        let _guard = self.lock_session(id).await;
        if self.session_was_superseded(id) {
            return Ok(None);
        }
        // Head-canonical sessions materialize straight from head+rows.
        if let Some(session) = self.load_head_canonical_session(id).await? {
            return Ok(Some(session));
        }
        // Registration is observed realization state, not transcript history.
        // Missing durable bytes stay missing so upstream can classify them;
        // never synthesize an empty session.
        Ok(self
            .load_persisted_session_with_bytes(id)
            .await?
            .map(|(session, _)| session))
    }

    async fn list(
        &self,
        _filter: meerkat_store::SessionFilter,
    ) -> Result<Vec<meerkat_core::SessionMeta>, meerkat_store::SessionStoreError> {
        // Listing is not supported through the continuity store adapter.
        // The continuity model is identity-keyed, not session-list-keyed.
        Ok(Vec::new())
    }

    async fn delete(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let _guard = self.lock_session(id).await;
        if self.session_was_superseded(id) {
            return Ok(());
        }
        self.ensure_session_mutation_allowed(id)?;
        let Some(session) = self.load_persisted_session(id).await? else {
            self.forget_session(id).await;
            // `delete` is unconditional: once it reports Ok the session is
            // gone, whether or not a durable row was there to remove. The
            // marker is what stops a later stale save from parking as a
            // "creation-window" write and being flushed back at the next
            // registration.
            self.mark_session_removed(id);
            return Ok(());
        };
        let current_revision = self.projection_compare_token(&session)?;
        let deleted = self
            .store
            .delete_session_snapshot_if_current_revision(id, &current_revision)
            .await
            .map_err(|e| {
                meerkat_store::SessionStoreError::Internal(format!("continuity delete: {e}"))
            })?;
        if !deleted {
            return Err(meerkat_store::SessionStoreError::Internal(format!(
                "continuity delete did not remove session snapshot {id}"
            )));
        }
        self.forget_session(id).await;
        self.mark_session_removed(id);
        Ok(())
    }

    async fn delete_if_current_revision(
        &self,
        id: &meerkat_core::types::SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, meerkat_store::SessionStoreError> {
        let _guard = self.lock_session(id).await;
        if self.session_was_superseded(id) {
            return Ok(false);
        }
        self.ensure_session_mutation_allowed(id)?;
        let Some(session) = self.load_persisted_session(id).await? else {
            self.forget_session(id).await;
            return Ok(false);
        };
        let current_revision = self.projection_compare_token(&session)?;
        if current_revision != expected_current_revision {
            return Ok(false);
        }
        let deleted = self
            .store
            .delete_session_snapshot_if_current_revision(id, expected_current_revision)
            .await
            .map_err(|e| {
                meerkat_store::SessionStoreError::Internal(format!(
                    "continuity delete_if_current_revision: {e}"
                ))
            })?;
        if deleted {
            self.forget_session(id).await;
            // Conditional verb: the removal marker is published only for the
            // outcome that actually removed the durable document.
            self.mark_session_removed(id);
        }
        Ok(deleted)
    }

    /// Genuine forwarding of the incremental capability (M4b): `Some` exactly
    /// when the continuity substrate advertises a session-delta channel
    /// ([`super::contracts::ContinuityStore::as_incremental_sessions`]). The
    /// returned wrapper preserves the adapter's registration and fencing
    /// discipline — every delta mutation carries the registered continuity
    /// cursor, is refused for suspended/unregistered/superseded sessions,
    /// parks when the owning identity has not been published yet, and
    /// serializes on the same per-session lock as the whole-blob paths. The
    /// bundled `LocalContinuityStore` advertises the channel; the JSON-RPC
    /// `GatewayContinuityStore` (whole-snapshot wire verbs only) does not, so
    /// external-authoritative launches keep the truthful H2 whole-blob
    /// report.
    fn as_incremental(
        self: Arc<Self>,
    ) -> Option<Arc<dyn meerkat_core::session_store::IncrementalSessionStore>> {
        let inner = self.incremental.clone()?;
        Some(Arc::new(ContinuityIncrementalSessionStore {
            adapter: self,
            inner,
        }))
    }
}

// ---------------------------------------------------------------------------
// Incremental forwarding wrapper (M4b)
// ---------------------------------------------------------------------------

/// The incremental session-store view the adapter returns when its
/// substrate advertises [`super::contracts::ContinuityStore::as_incremental_sessions`].
///
/// Split of responsibilities:
/// - the substrate's channel owns the durable delta contract (append
///   contiguity/idempotency, rewrite verification, head CAS) **and** the
///   continuity write discipline (fence CAS + version monotonicity per
///   mutation) — that is the advertised capability's documented obligation;
/// - this wrapper owns the adapter-side session lifecycle: delta mutations
///   are admitted only for sessions registered with the identity runtime and
///   are refused while a session is suspended (authority rotation),
///   unregistered, or superseded — under the same per-session lock the
///   whole-blob save paths serialize on (H3's lazy adoption included).
///
/// Whole-document `SessionStore` verbs delegate to the adapter unchanged, so
/// mixed consumers observe one behavior.
pub struct ContinuityIncrementalSessionStore {
    adapter: Arc<ContinuitySessionStoreAdapter>,
    inner: Arc<dyn super::contracts::ContinuityIncrementalSessions>,
}

/// Rebuild the durable rewrite record from the incoming document's retained
/// bodies (the port of meerkat-store's `rewrite_record_from_session_bodies`).
fn rewrite_record_from_session_bodies(
    session: &meerkat_core::Session,
    commit: &meerkat_core::TranscriptRewriteCommit,
) -> Result<meerkat_core::TranscriptRewriteRecord, meerkat_store::SessionStoreError> {
    let parent_body = session
        .transcript_revision_body(&commit.parent_revision)?
        .ok_or_else(
            || meerkat_store::SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!(
                    "incoming rewrite omitted parent revision body {}",
                    commit.parent_revision
                ),
            },
        )?;
    let revision_body = session
        .transcript_revision_body(&commit.revision)?
        .ok_or_else(
            || meerkat_store::SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!(
                    "incoming rewrite omitted new revision body {}",
                    commit.revision
                ),
            },
        )?;
    meerkat_core::TranscriptRewriteRecord::new(commit.clone(), parent_body, revision_body).map_err(
        |err| meerkat_store::SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("transcript rewrite record failed validation: {err}"),
        },
    )
}

/// Where one delta mutation is routed.
enum DeltaRoute {
    /// Registered: write through to the substrate under this cursor.
    Durable(super::contracts::ContinuityWriteCursor),
    /// Not registered yet: park in process memory, write nothing durable.
    Park,
    /// Superseded by a committed continuity reset: acknowledge the terminal
    /// projection and write nothing. Reset already discarded this document
    /// when it committed the replacement generation, and Meerkat's retire
    /// still realizes its generated Archived document action afterwards.
    SupersededNoOp,
}

impl ContinuityIncrementalSessionStore {
    /// Lifecycle gate + cursor resolution for one delta mutation.
    ///
    /// Superseded / unregistered / suspended sessions are refused typed
    /// exactly as before. A session whose owning identity has not been
    /// published yet is PARKED rather than refused: creation-time saves
    /// provably arrive in that window, and the service fails closed on
    /// projection errors, so refusing them would break member creation on
    /// every identity gateway.
    ///
    /// Routing does NOT publish durability: the caller marks the session
    /// durably-observed only after the substrate acknowledges the write. This
    /// used to mark on the `Durable` branch, before the await — so a write
    /// that failed at the substrate still left the session claiming durable
    /// state, and its perfectly legal creation-window park was refused
    /// afterwards.
    async fn route_delta_write(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<DeltaRoute, meerkat_store::SessionStoreError> {
        if self.adapter.session_was_superseded(id) {
            // SAFE BECAUSE OF THE ORDERING, not merely by convention:
            // `abandon_superseded_session` deletes the durable snapshot under
            // an exact-CAS and only inserts into `superseded_sessions` AFTER
            // that delete succeeds, all while holding this session's lock. So
            // a session observed as superseded provably has no durable rows
            // left for a skipped write to strand. (If supersession were ever
            // marked BEFORE the delete, acknowledging a head write here would
            // leave already-written strand rows adopted by nothing and the
            // next read would serve an older transcript — silent data loss.
            // Keep the marking after the delete.)
            //
            // Every sibling write verb on this adapter already acknowledges a
            // post-abandon terminal write as a no-op (see `save`,
            // `save_transcript`, `save_transcript_rewrite`, and the CAS save
            // above). The incremental verbs must agree: reset's retire path
            // abandons the superseded session and THEN retires the physical
            // member, and Meerkat's retire/archive issues one more save. A
            // typed refusal there aborts identity-authority release, so the
            // gateway retains provider grants and reports
            // runtime_cleanup_completed=false.
            return Ok(DeltaRoute::SupersededNoOp);
        }
        self.adapter.ensure_session_mutation_allowed(id)?;
        match self.adapter.lookup_session(&id.to_string()) {
            Some(state) => Ok(DeltaRoute::Durable(self.adapter.write_cursor(id, &state))),
            None => {
                // The delta-specific harm behind the shared parking guard:
                // once any delta of this session is durable, parking a later
                // one (in particular the adopting head) leaves the durable
                // strand rows adopted by nothing, and the next open serves an
                // older transcript than what is persisted. Refuse so the
                // caller learns its write did not land, instead of the loss
                // surfacing as a reboot-time "transcript went backwards".
                self.adapter.ensure_unregistered_park_allowed(id).await?;
                Ok(DeltaRoute::Park)
            }
        }
    }

    /// Read-only view of the parked store. Parked WRITES go through
    /// [`ParkedDeltas`]'s `park_*` verbs, which publish the footprint as part
    /// of the write.
    fn parked(&self) -> &meerkat_store::MemoryStore {
        self.adapter.parked_deltas.reads()
    }

    /// Reads follow writes: a session whose deltas are parked must read back
    /// through the parked view, or the service's continuity preflight and
    /// head CAS view would disagree with what it just wrote.
    fn reads_parked(&self, id: &meerkat_core::types::SessionId) -> bool {
        self.adapter.parked_deltas.is_parked(id)
    }
}

#[async_trait]
impl meerkat::SessionStore for ContinuityIncrementalSessionStore {
    async fn save(
        &self,
        session: &meerkat_core::Session,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        self.adapter.save(session).await
    }

    async fn save_transcript_rewrite(
        &self,
        session: &meerkat_core::Session,
        commit: &meerkat_core::TranscriptRewriteCommit,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        self.adapter.save_transcript_rewrite(session, commit).await
    }

    async fn save_authoritative_projection(
        &self,
        session: &meerkat_core::Session,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        self.adapter.save_authoritative_projection(session).await
    }

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &meerkat_core::Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        self.adapter
            .save_authoritative_projection_if_current_revision(session, expected_current_revision)
            .await
    }

    async fn load(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::Session>, meerkat_store::SessionStoreError> {
        self.adapter.load(id).await
    }

    async fn list(
        &self,
        filter: meerkat_store::SessionFilter,
    ) -> Result<Vec<meerkat_core::SessionMeta>, meerkat_store::SessionStoreError> {
        self.adapter.list(filter).await
    }

    async fn delete(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        self.adapter.delete(id).await
    }

    async fn delete_if_current_revision(
        &self,
        id: &meerkat_core::types::SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, meerkat_store::SessionStoreError> {
        self.adapter
            .delete_if_current_revision(id, expected_current_revision)
            .await
    }

    fn as_incremental(
        self: Arc<Self>,
    ) -> Option<Arc<dyn meerkat_core::session_store::IncrementalSessionStore>> {
        Some(self)
    }
}

#[async_trait]
impl meerkat_core::session_store::IncrementalSessionStore for ContinuityIncrementalSessionStore {
    /// meerkat 0.8.22 store-wide format-door activation (NEW in 0.8.22;
    /// required, no default). This wrapper REFUSES it, typed, and the
    /// refusal is the honest answer rather than a shortfall we hid.
    ///
    /// The contract is a census, not a loop: the backend must enumerate its
    /// PHYSICAL session identities, cross and verify each one, and return the
    /// complete result from ONE write transaction or equivalent snapshot -
    /// and it must NOT derive that census from `SessionStore::list`, which is
    /// a filtered metadata projection.
    ///
    /// This wrapper cannot satisfy that. Its substrate seam
    /// ([`super::contracts::ContinuityIncrementalSessions`]) has no
    /// store-wide census verb at all, and the outer
    /// [`super::contracts::ContinuityStore`] has no physical session
    /// enumeration either (`resolve_many` is identity-keyed,
    /// `load_session_snapshot` is id-keyed). The only shapes available here
    /// are the two the contract forbids: the adapter's `list` projection, or
    /// an enumeration composed with N separate per-id
    /// `cross_head_canonical_authority` calls - which together span N+1
    /// snapshots and can therefore report a census of a store state that
    /// never existed at any instant. That is exactly the tearing this repo already refuses to
    /// commit for a single document (see `load_canonical_session`'s
    /// single-snapshot obligation in `contracts.rs`), widened store-wide.
    ///
    /// Why not `NotApplicable`: that variant asserts "this store's durable
    /// profile is not HeadCanonical", and here that would be false in the one
    /// direction that matters. The continuity substrate does hold physical
    /// head-canonical rows (`continuity_session_heads`, operator-gated by
    /// `HEAD_CANONICAL_CONTINUITY_SCHEMA_VERSION`), and this very impl
    /// forwards a REAL `AlreadyCurrent` crossing for a named session below.
    /// A store that answers `AlreadyCurrent` per session while answering "not
    /// HeadCanonical" store-wide is contradicting itself, and it would
    /// mis-diagnose the failure for whoever hit it: meerkat's
    /// `activate_external_head_canonical_store` turns `NotApplicable` into
    /// "profile mismatch", pointing the reader at operator configuration
    /// instead of at the missing census seam.
    ///
    /// Reachability, stated exactly, because the comfortable version of this
    /// sentence is false. meerkat calls this verb from
    /// `activate_external_head_canonical_store`, which runs inside its
    /// `open_realm_bundle_with_provider`. meta does not go through that
    /// function: `storage_provider::open_provider_meerkat_stores` calls
    /// `RealmStorageProvider::open` directly and keeps only the runtime,
    /// workgraph and jobs slots, DISCARDING the meerkat-level session store.
    /// So this refusal fires only for an embedder that composes this store
    /// through meerkat's own realm-bundle seam, and for that consumer it is
    /// a fail-closed backstop naming the seam that is missing.
    ///
    /// It is deliberately NOT claimed to be the guard for meta's own
    /// provider pairing, because it is not one. `unified_runtime::builder`
    /// pairs a provider-supplied runtime store with this session store
    /// (through `SessionStoreBackedRuntimeStore`), and that runtime store's
    /// persistence profile is whatever the provider declares - meta's own
    /// `!= WholeBlobV1` gates in `mob_handle_runtime` exist precisely because
    /// a HeadCanonical inner store is contemplated there. A composite
    /// provider declaring HeadCanonical would therefore start with NO
    /// store-wide activation performed by anyone: meta's provider path
    /// takes no census and asks for none. Closing that needs BOTH the census
    /// verb below and an activation call on the provider path, and the second
    /// half is currently not expressible: meerkat published
    /// `enforce_fail_closed_durability` from its post-`open()` obligation set
    /// but kept `activate_external_head_canonical_store` private, so an
    /// external composer of `RealmStorageProvider` cannot discharge it
    /// without re-deriving meerkat's dedup and token-alignment invariants.
    /// That is an upstream ask reported with this port, not something already
    /// tracked, and not something a port may smuggle in - pretending this
    /// `Err` already covers it would be the lie.
    ///
    /// The census verb, when a HeadCanonical pairing is actually wanted, is
    /// one transaction on `ContinuityIncrementalSessions` implemented by
    /// `LocalContinuityStore`; the SQL already exists as
    /// `storage_doctor.rs`'s `census_head_canonical_sessions`.
    async fn activate_head_canonical_store(
        &self,
    ) -> Result<
        meerkat_core::session_store::HeadCanonicalStoreActivation,
        meerkat_store::SessionStoreError,
    > {
        Err(meerkat_store::SessionStoreError::Internal(
            "continuity incremental session store cannot activate a HeadCanonical store: the \
             continuity substrate exposes no one-snapshot physical session census \
             (ContinuityIncrementalSessions has no census verb, ContinuityStore has no physical \
             session enumeration), and the activation contract forbids deriving the census from \
             SessionStore::list"
                .to_string(),
        ))
    }

    async fn append_messages(
        &self,
        id: &meerkat_core::types::SessionId,
        strand: &meerkat_core::session_store::TranscriptStrandId,
        base_seq: u64,
        messages: &[meerkat_core::Message],
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let _guard = self.adapter.lock_session(id).await;
        match self.route_delta_write(id).await? {
            DeltaRoute::Durable(cursor) => {
                self.inner
                    .append_messages(&cursor, id, strand, base_seq, messages)
                    .await?;
                // AFTER the acknowledgement, never before: a refused append
                // is not durable state and must not lock the session out of
                // its creation window.
                self.adapter.mark_session_durably_written(id);
                Ok(())
            }
            DeltaRoute::Park => {
                self.adapter
                    .parked_deltas
                    .park_append(id, strand, base_seq, messages)
                    .await
            }
            DeltaRoute::SupersededNoOp => Ok(()),
        }
    }

    async fn commit_rewrite(
        &self,
        id: &meerkat_core::types::SessionId,
        record: &meerkat_core::TranscriptRewriteRecord,
        expected: meerkat_core::session_store::SessionHeadCas,
    ) -> Result<meerkat_core::session_store::SessionHead, meerkat_store::SessionStoreError> {
        let _guard = self.adapter.lock_session(id).await;
        match self.route_delta_write(id).await? {
            DeltaRoute::Durable(cursor) => {
                let head = self
                    .inner
                    .commit_rewrite(&cursor, id, record, expected)
                    .await?;
                self.adapter.mark_session_durably_written(id);
                Ok(head)
            }
            DeltaRoute::Park => {
                self.adapter
                    .parked_deltas
                    .park_rewrite(id, record, expected)
                    .await
            }
            // A rewrite COMMIT is not a terminal projection: it carries new
            // authority and must not be silently dropped. Superseded stays
            // refused here, typed, exactly as M4b intended.
            DeltaRoute::SupersededNoOp => Err(meerkat_store::SessionStoreError::Internal(format!(
                "session {id} was superseded by a committed continuity reset; \
                     rewrite commits are refused"
            ))),
        }
    }

    async fn save_head(
        &self,
        head: &meerkat_core::session_store::SessionHead,
        expected: meerkat_core::session_store::SessionHeadCas,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let _guard = self.adapter.lock_session(&head.id).await;
        match self.route_delta_write(&head.id).await? {
            DeltaRoute::Durable(cursor) => {
                self.inner.save_head(&cursor, head, expected).await?;
                self.adapter.mark_session_durably_written(&head.id);
                Ok(())
            }
            DeltaRoute::Park => self.adapter.parked_deltas.park_head(head, expected).await,
            DeltaRoute::SupersededNoOp => Ok(()),
        }
    }

    async fn load_head(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::session_store::SessionHead>, meerkat_store::SessionStoreError>
    {
        if self.adapter.session_was_superseded(id) {
            return Ok(None);
        }
        if self.reads_parked(id) {
            return meerkat_core::session_store::IncrementalSessionStore::load_head(
                self.parked(),
                id,
            )
            .await;
        }
        self.inner.load_head(id).await
    }

    /// meerkat 0.8.21 format-door crossing (required, no default). This
    /// wrapper routes, never converts: the store that OWNS the row performs
    /// the single-transaction conversion + reverification. Superseded
    /// sessions present as absent on every read verb here, and the crossing
    /// contract names absence `NotApplicable`.
    async fn cross_head_canonical_authority(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<
        meerkat_core::session_store::HeadCanonicalAuthorityCrossing,
        meerkat_store::SessionStoreError,
    > {
        let _guard = self.adapter.lock_session(id).await;
        if self.adapter.session_was_superseded(id) {
            return Ok(meerkat_core::session_store::HeadCanonicalAuthorityCrossing::NotApplicable);
        }
        // Deliberately NOT routed through the parked overlay: the park holds
        // creation-window deltas, never physical format authority. The
        // crossing classifies and converts the exact stored head in the
        // owning store, and activation runs before any park can exist.
        self.inner.cross_head_canonical_authority(id).await
    }

    async fn load_messages(
        &self,
        id: &meerkat_core::types::SessionId,
        strand: &meerkat_core::session_store::TranscriptStrandId,
        range: std::ops::Range<u64>,
    ) -> Result<Vec<meerkat_core::Message>, meerkat_store::SessionStoreError> {
        if self.adapter.session_was_superseded(id) {
            return Ok(Vec::new());
        }
        if self.reads_parked(id) {
            return meerkat_core::session_store::IncrementalSessionStore::load_messages(
                self.parked(),
                id,
                strand,
                range,
            )
            .await;
        }
        self.inner.load_messages(id, strand, range).await
    }

    async fn load_rewrites(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<Vec<meerkat_core::TranscriptRewriteRecord>, meerkat_store::SessionStoreError> {
        if self.adapter.session_was_superseded(id) {
            return Ok(Vec::new());
        }
        if self.reads_parked(id) {
            return meerkat_core::session_store::IncrementalSessionStore::load_rewrites(
                self.parked(),
                id,
            )
            .await;
        }
        self.inner.load_rewrites(id).await
    }
}

// ---------------------------------------------------------------------------
// CONTRACT-11 / REQ-30: SessionHook → AgentCustomizer adapter
// ---------------------------------------------------------------------------

/// Adapts a legacy `SessionHook` to the `AgentCustomizer` trait.
///
/// Constructs a synthetic `CreateSessionRequest` from the `AgentBuildDraft`,
/// lets the hook mutate it, and writes supported mutations back. Unsupported
/// field mutations (e.g., `resume_session`) are detected and logged as warnings.
pub struct SessionHookCustomizerAdapter {
    hook: Arc<dyn SessionHook>,
}

impl SessionHookCustomizerAdapter {
    pub fn new(hook: Arc<dyn SessionHook>) -> Self {
        Self { hook }
    }
}

#[async_trait]
impl AgentCustomizer for SessionHookCustomizerAdapter {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        // Build a synthetic CreateSessionRequest from the draft
        let mut req = meerkat_core::service::CreateSessionRequest {
            model: draft.model.clone().unwrap_or_default(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            // Meerkat 0.7: per-request system prompt is the typed tri-state
            // override; a draft prompt maps to an explicit `Set`.
            system_prompt: match draft.system_prompt.clone() {
                Some(prompt) => meerkat_core::config::SystemPromptOverride::Set(prompt),
                None => meerkat_core::config::SystemPromptOverride::Inherit,
            },
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            build: None,
            labels: if draft.labels.is_empty() {
                None
            } else {
                Some(draft.labels.clone())
            },
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
            // Meerkat 0.7.12: typed per-request ambient injection (upstream
            // ask #1). This synthetic request only carries build-draft state;
            // memory injection rides its own path, so it stays empty here.
            injected_context: Vec::new(),
        };

        // Snapshot "before" state for unsupported-mutation detection (REQ-30).
        // Supported: model, system_prompt, labels — written back to draft.
        // Unsupported: everything else — warn if hook mutated them.
        let prompt_before = req.prompt.clone();
        let max_tokens_before = req.max_tokens;
        let event_tx_was_some = req.event_tx.is_some();
        let initial_turn_before = req.initial_turn;
        let build_before_is_none = req.build.is_none();

        self.hook
            .before_create(&mut req)
            .await
            .map_err(|e| CustomizerError::BuildFailed(format!("session hook: {e}")))?;

        // Detect unsupported mutations by comparing before/after (REQ-30).
        // Warn for each mutated field — mutations are NOT applied to the draft.
        let mut unsupported_mutations: Vec<&str> = Vec::new();

        if req.prompt != prompt_before {
            unsupported_mutations.push("prompt");
        }
        if req.max_tokens != max_tokens_before {
            unsupported_mutations.push("max_tokens");
        }
        if req.event_tx.is_some() != event_tx_was_some {
            unsupported_mutations.push("event_tx");
        }
        if req.initial_turn != initial_turn_before {
            unsupported_mutations.push("initial_turn");
        }
        // Meerkat 0.7 removed the flat `render_metadata` / `skill_references`
        // request fields; both now live only on the typed
        // `build.initial_turn_metadata` carrier, which the `build` mutation
        // detection below already covers.
        if let Some(ref build) = req.build {
            if build_before_is_none {
                // Hook created a build block — any build.* is unsupported
                unsupported_mutations.push("build");
                if build.resume_session.is_some() {
                    unsupported_mutations.push("build.resume_session");
                }
            } else if build.resume_session.is_some() {
                // build existed before but hook added resume_session
                unsupported_mutations.push("build.resume_session");
            }
        }

        if !unsupported_mutations.is_empty() {
            tracing::warn!(
                identity = %spec.identity,
                fields = ?unsupported_mutations,
                "SessionHook mutated unsupported CreateSessionRequest fields — \
                 these mutations are NOT applied in the identity-first model. \
                 Migrate to AgentCustomizer."
            );
        }

        // Apply supported mutations back to the draft.
        //
        // NOTE: `additional_instructions` is part of the AgentCustomizer mutation
        // surface (REQ-30), but `CreateSessionRequest` does not expose it as a
        // field. Legacy hooks therefore cannot modify additional_instructions —
        // the draft's existing value from the DurableAgentSpec passes through
        // untouched. Native `AgentCustomizer` impls CAN mutate it directly.
        if !req.model.is_empty() {
            draft.model = Some(req.model);
        }
        draft.system_prompt = req.system_prompt.as_set_prompt().map(ToString::to_string);
        draft.labels = req.labels.unwrap_or_default();

        Ok(())
    }

    async fn after_create(
        &self,
        _identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        self.hook.after_create(session_id, context).await;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
    use std::time::Duration;

    use serde_json::json;

    use super::super::contracts::ContinuityStore;
    use super::super::local_store::LocalContinuityStore;
    use super::super::types::{
        AgentIdentity, AgentRuntimeId, CheckpointVersion, ContinuityGeneration, ContinuityRecord,
        ContinuityResolveState, ContinuityStoreError, FencingToken, SessionSnapshot,
    };
    use super::*;

    struct FailSaveContinuityStore {
        inner: Arc<LocalContinuityStore>,
        fail_save: AtomicBool,
        commit_then_fail_save: AtomicBool,
        fail_delete_once: AtomicBool,
        block_next_save: AtomicBool,
        save_entered: tokio::sync::Semaphore,
        release_save: tokio::sync::Semaphore,
    }

    impl FailSaveContinuityStore {
        fn new(inner: Arc<LocalContinuityStore>) -> Self {
            Self {
                inner,
                fail_save: AtomicBool::new(false),
                commit_then_fail_save: AtomicBool::new(false),
                fail_delete_once: AtomicBool::new(false),
                block_next_save: AtomicBool::new(false),
                save_entered: tokio::sync::Semaphore::new(0),
                release_save: tokio::sync::Semaphore::new(0),
            }
        }

        fn fail_saves(&self, fail: bool) {
            self.fail_save.store(fail, AtomicOrdering::SeqCst);
        }

        fn commit_then_fail_next_save(&self) {
            self.commit_then_fail_save
                .store(true, AtomicOrdering::SeqCst);
        }

        fn fail_next_delete(&self) {
            self.fail_delete_once.store(true, AtomicOrdering::SeqCst);
        }

        fn block_one_save(&self) {
            self.block_next_save.store(true, AtomicOrdering::SeqCst);
        }

        async fn wait_for_blocked_save(&self) {
            self.save_entered
                .acquire()
                .await
                .expect("save-entered semaphore remains open")
                .forget();
        }

        fn release_blocked_save(&self) {
            self.release_save.add_permits(1);
        }
    }

    struct ConcurrentLoadStore {
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
        rendezvous: tokio::sync::Barrier,
    }

    impl ConcurrentLoadStore {
        fn new(expected_concurrent_loads: usize) -> Self {
            Self {
                in_flight: AtomicUsize::new(0),
                max_in_flight: AtomicUsize::new(0),
                rendezvous: tokio::sync::Barrier::new(expected_concurrent_loads),
            }
        }
    }

    #[async_trait]
    impl ContinuityStore for FailSaveContinuityStore {
        // Explicit answer required by the trait: silence here used to
        // assert an authoritative "no record" while meaning "never
        // taught to answer".
        async fn resolve_record_by_session(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<
            Option<(
                crate::identity_first::ContinuityRecord,
                crate::identity_first::FencingToken,
                crate::identity_first::CheckpointVersion,
            )>,
            crate::identity_first::ContinuityStoreError,
        > {
            Ok(None)
        }

        async fn resolve_many(
            &self,
            identities: &[AgentIdentity],
        ) -> Result<
            std::collections::BTreeMap<AgentIdentity, ContinuityResolveState>,
            ContinuityStoreError,
        > {
            self.inner.resolve_many(identities).await
        }

        async fn load_session_snapshot(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<Option<SessionSnapshot>, ContinuityStoreError> {
            self.inner.load_session_snapshot(session_id).await
        }

        async fn delete_session_snapshot_if_current_revision(
            &self,
            session_id: &meerkat_core::types::SessionId,
            expected_current_revision: &str,
        ) -> Result<bool, ContinuityStoreError> {
            if self.fail_delete_once.swap(false, AtomicOrdering::SeqCst) {
                return Err(ContinuityStoreError::Io(
                    "synthetic superseded snapshot delete failure".to_string(),
                ));
            }
            self.inner
                .delete_session_snapshot_if_current_revision(session_id, expected_current_revision)
                .await
        }

        async fn save_session_snapshot(
            &self,
            identity: &AgentIdentity,
            session_id: &meerkat_core::types::SessionId,
            generation: ContinuityGeneration,
            version: CheckpointVersion,
            fencing_token: FencingToken,
            snapshot: &SessionSnapshot,
        ) -> Result<(), ContinuityStoreError> {
            if self.block_next_save.swap(false, AtomicOrdering::SeqCst) {
                self.save_entered.add_permits(1);
                self.release_save
                    .acquire()
                    .await
                    .expect("release-save semaphore remains open")
                    .forget();
            }
            if self.fail_save.load(AtomicOrdering::SeqCst) {
                return Err(ContinuityStoreError::Io("forced save failure".to_string()));
            }
            self.inner
                .save_session_snapshot(
                    identity,
                    session_id,
                    generation,
                    version,
                    fencing_token,
                    snapshot,
                )
                .await?;
            if self
                .commit_then_fail_save
                .swap(false, AtomicOrdering::SeqCst)
            {
                return Err(ContinuityStoreError::Io(
                    "synthetic lost save acknowledgement".to_string(),
                ));
            }
            Ok(())
        }

        async fn upsert_continuity_record(
            &self,
            record: &ContinuityRecord,
            fencing_token: FencingToken,
        ) -> Result<(), ContinuityStoreError> {
            self.inner
                .upsert_continuity_record(record, fencing_token)
                .await
        }

        async fn delete_continuity_record(
            &self,
            identity: &AgentIdentity,
            fencing_token: FencingToken,
        ) -> Result<(), ContinuityStoreError> {
            self.inner
                .delete_continuity_record(identity, fencing_token)
                .await
        }
        /// Test double: deliberately whole-snapshot only.
        fn as_incremental_sessions(
            &self,
        ) -> Option<
            std::sync::Arc<dyn crate::identity_first::contracts::ContinuityIncrementalSessions>,
        > {
            None
        }
    }

    #[async_trait]
    impl ContinuityStore for ConcurrentLoadStore {
        // The trait requires an explicit answer: a double that stayed
        // silent used to assert an authoritative "no record" while
        // meaning "I was never taught to answer", which is how a fix
        // goes inert with every test still green.
        async fn resolve_record_by_session(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<
            Option<(
                crate::identity_first::ContinuityRecord,
                crate::identity_first::FencingToken,
                crate::identity_first::CheckpointVersion,
            )>,
            crate::identity_first::ContinuityStoreError,
        > {
            Ok(None)
        }

        async fn resolve_many(
            &self,
            identities: &[AgentIdentity],
        ) -> Result<
            std::collections::BTreeMap<AgentIdentity, ContinuityResolveState>,
            ContinuityStoreError,
        > {
            Ok(identities
                .iter()
                .cloned()
                .map(|identity| (identity, ContinuityResolveState::Uninitialized))
                .collect())
        }

        async fn load_session_snapshot(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<Option<SessionSnapshot>, ContinuityStoreError> {
            let now = self.in_flight.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            self.max_in_flight.fetch_max(now, AtomicOrdering::SeqCst);
            self.rendezvous.wait().await;
            self.in_flight.fetch_sub(1, AtomicOrdering::SeqCst);
            Ok(None)
        }

        async fn save_session_snapshot(
            &self,
            _identity: &AgentIdentity,
            _session_id: &meerkat_core::types::SessionId,
            _generation: ContinuityGeneration,
            _version: CheckpointVersion,
            _fencing_token: FencingToken,
            _snapshot: &SessionSnapshot,
        ) -> Result<(), ContinuityStoreError> {
            Ok(())
        }

        async fn upsert_continuity_record(
            &self,
            _record: &ContinuityRecord,
            _fencing_token: FencingToken,
        ) -> Result<(), ContinuityStoreError> {
            Ok(())
        }

        async fn delete_continuity_record(
            &self,
            _identity: &AgentIdentity,
            _fencing_token: FencingToken,
        ) -> Result<(), ContinuityStoreError> {
            Ok(())
        }
        /// Test double: deliberately whole-snapshot only.
        fn as_incremental_sessions(
            &self,
        ) -> Option<
            std::sync::Arc<dyn crate::identity_first::contracts::ContinuityIncrementalSessions>,
        > {
            None
        }
    }

    /// Production release-critical regression (2026-07-31): a released 0.8.10
    /// session envelope (v2) held by a continuity store must import exactly
    /// once on the adapter load path - the seam every external whole-blob
    /// store traverses - and adopt durably, so the second load takes the
    /// already-current path. The fixture keeps the frozen 0.8.10 envelope
    /// shape with synthetic content (tests/fixtures/README.md); current code
    /// cannot and must not mint it.
    #[tokio::test]
    async fn released_v2_snapshot_imports_once_on_adapter_load() {
        const RELEASED: &[u8] =
            include_bytes!("../../tests/fixtures/v0_8_10_released_session.json");
        let raw: serde_json::Value = serde_json::from_slice(RELEASED).expect("fixture JSON");
        let session_id =
            meerkat_core::types::SessionId::parse(raw["id"].as_str().expect("fixture id"))
                .expect("fixture session id");
        // The released envelope is exactly what the current decoder refuses.
        assert!(meerkat_core::Session::from_persisted_bytes(RELEASED).is_err());

        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("continuity.sqlite3");
        let store = Arc::new(LocalContinuityStore::open(&db_path).expect("store"));
        let identity = AgentIdentity::parse("agent:released-import").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:released-import:0")
                .expect("runtime id"),
            session_id: session_id.clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(1),
        };
        let fencing_token = FencingToken::new(3);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        // Seed the released bytes exactly as a 0.8.10-era deployment left
        // them: a raw durable row, never routed through current encoders.
        {
            let conn = rusqlite::Connection::open(&db_path).expect("seed connection");
            conn.execute(
                "INSERT INTO session_snapshots \
                 (session_id, identity, generation, checkpoint_version, fencing_token, data) \
                 VALUES (?1, ?2, 0, 1, 3, ?3)",
                rusqlite::params![session_id.to_string(), identity.to_string(), RELEASED],
            )
            .expect("seed released snapshot row");
        }

        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        adapter
            .register_session(
                &session_id,
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: ContinuityGeneration::new(0),
                    fencing_token,
                    checkpoint_version: CheckpointVersion::new(1),
                },
            )
            .await
            .expect("register");

        let cursor_of = |resolved: &std::collections::BTreeMap<
            AgentIdentity,
            ContinuityResolveState,
        >| match resolved.get(&identity) {
            Some(ContinuityResolveState::Ready { record }) => record.checkpoint_version,
            other => panic!("expected a ready continuity record, got {other:?}"),
        };

        // First load: imports the released envelope and durably adopts it.
        let loaded = meerkat::SessionStore::load(&adapter, &session_id)
            .await
            .expect("first load imports the released envelope")
            .expect("session present");
        assert_eq!(loaded.id(), &session_id);
        assert!(
            !loaded.messages().is_empty(),
            "the released transcript resumes with its content"
        );
        let after_first = cursor_of(
            &store
                .resolve_many(std::slice::from_ref(&identity))
                .await
                .expect("resolve after import"),
        );
        assert!(
            after_first > CheckpointVersion::new(1),
            "the import must adopt durably under an advanced registered cursor, got \
             {after_first:?}"
        );

        // Second load: the already-current path - same content, no new
        // adoption write.
        let reloaded = meerkat::SessionStore::load(&adapter, &session_id)
            .await
            .expect("second load")
            .expect("session present");
        assert_eq!(reloaded.id(), &session_id);
        assert_eq!(reloaded.messages().len(), loaded.messages().len());
        let after_second = cursor_of(
            &store
                .resolve_many(std::slice::from_ref(&identity))
                .await
                .expect("resolve after second load"),
        );
        assert_eq!(
            after_second, after_first,
            "a second load must take the already-current path and adopt nothing"
        );
    }

    /// The FIRST birth, pinned explicitly (lead requirement 2026-07-31): a
    /// registered session's first save on an incremental-capable substrate
    /// CREATES the head-canonical representation - one canonical head row
    /// (committed under the create-CAS) whose byte-exact commitment the
    /// store's own materialization satisfies (the document round-trips),
    /// with NO whole-blob row written. This is the birth half of the
    /// O(delta) contract; the steady state is pinned by the idle/encode
    /// gates and `identity_first_head_canonical_resume`.
    #[tokio::test]
    async fn first_registered_save_births_head_canonical() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("continuity.sqlite3");
        let store = Arc::new(LocalContinuityStore::open(&db_path).expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let mut session = meerkat_core::Session::new();
        session.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("first turn"),
        ));
        let identity = AgentIdentity::parse("agent:first-birth").expect("identity");
        let token = FencingToken::new(1);
        store
            .upsert_continuity_record(
                &ContinuityRecord {
                    identity: identity.clone(),
                    agent_runtime_id: AgentRuntimeId::parse("rt:agent:first-birth:0")
                        .expect("runtime id"),
                    session_id: session.id().clone(),
                    generation: ContinuityGeneration::new(0),
                    checkpoint_version: CheckpointVersion::new(0),
                },
                token,
            )
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: ContinuityGeneration::new(0),
                    fencing_token: token,
                    checkpoint_version: CheckpointVersion::new(0),
                },
            )
            .await
            .expect("register");

        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("the first registered save must birth head-canonical");

        let conn = rusqlite::Connection::open(&db_path).expect("probe connection");
        let (heads, head_count): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(message_count), -1) \
                 FROM continuity_session_heads WHERE session_id = ?1",
                rusqlite::params![session.id().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("head census");
        assert_eq!(heads, 1, "birth must create exactly one canonical head row");
        assert_eq!(
            head_count,
            session.messages().len() as i64,
            "the born head must describe the live transcript"
        );
        let blobs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_snapshots WHERE session_id = ?1",
                rusqlite::params![session.id().to_string()],
                |row| row.get(0),
            )
            .expect("blob census");
        assert_eq!(blobs, 0, "birth must not write a whole-blob row");

        // Byte-exact commitment: the store's own materialization (which
        // verifies the head's row-prefix commitment via
        // `SessionHead::into_session`) round-trips the document.
        let loaded = meerkat::SessionStore::load(&adapter, session.id())
            .await
            .expect("load after birth")
            .expect("born session present");
        assert_eq!(
            loaded.messages(),
            session.messages(),
            "the born representation must materialize the exact transcript"
        );
    }

    /// Released 0.8.10 ZERO-REWRITE history (the universal mob-supervisor
    /// shape: a transcript graph with one revision and zero commits) is
    /// REFUSED TYPED by the shipping 0.8.11 strict importer, through our
    /// adapter load path, with the durable row left untouched - a load
    /// error scoped to that one session, never a crash or a store-wide
    /// failure. The importer follow-ups that would have accepted this shape
    /// (d6cafd405 and successors) deliberately did NOT ship; this test pins
    /// the shipping contract so a later "fix" cannot silently turn the
    /// refusal into a panic or an adoption.
    ///
    /// PROVENANCE: the fixture keeps the RELEASED-MINTED envelope of a real
    /// 0.8.10 mob-supervisor snapshot (key order, spelling, and the omitted
    /// empty `commits` key, a spelling a writer-synthesized fixture gets
    /// wrong); only its free text is synthetic, with the transcript revision
    /// digest recomputed by the released recomputation
    /// (tests/fixtures/README.md). It is never re-synthesized by the pinned
    /// writer - a self-minted fixture silently passes writer-drift bugs.
    #[tokio::test]
    async fn released_zero_rewrite_history_refuses_typed_on_adapter_load() {
        const RELEASED: &[u8] =
            include_bytes!("../../tests/fixtures/v0_8_10_zero_rewrite_supervisor_session.json");
        let raw: serde_json::Value = serde_json::from_slice(RELEASED).expect("fixture JSON");
        let session_id =
            meerkat_core::types::SessionId::parse(raw["id"].as_str().expect("fixture id"))
                .expect("fixture session id");
        // The released envelope is exactly what the current decoder refuses,
        // so the load below reaches the one-time importer.
        assert!(meerkat_core::Session::from_persisted_bytes(RELEASED).is_err());

        let identity = AgentIdentity::parse("agent:zero-rewrite-strict").expect("identity");
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("continuity.sqlite3");
        let store = Arc::new(LocalContinuityStore::open(&db_path).expect("store"));
        // Seed the released bytes exactly as the 0.8.10 deployment left
        // them: a raw durable row, never routed through current encoders.
        {
            let conn = rusqlite::Connection::open(&db_path).expect("seed connection");
            conn.execute(
                "INSERT INTO session_snapshots \
                 (session_id, identity, generation, checkpoint_version, fencing_token, data) \
                 VALUES (?1, ?2, 0, 1, 3, ?3)",
                rusqlite::params![session_id.to_string(), identity.to_string(), RELEASED],
            )
            .expect("seed released snapshot row");
        }
        store
            .upsert_continuity_record(
                &ContinuityRecord {
                    identity: identity.clone(),
                    agent_runtime_id: AgentRuntimeId::parse("rt:agent:zero-rewrite-strict:0")
                        .expect("runtime id"),
                    session_id: session_id.clone(),
                    generation: ContinuityGeneration::new(0),
                    checkpoint_version: CheckpointVersion::new(1),
                },
                FencingToken::new(3),
            )
            .await
            .expect("seed record");
        let adapter = ContinuitySessionStoreAdapter::new(store);
        adapter
            .register_session(
                &session_id,
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: ContinuityGeneration::new(0),
                    fencing_token: FencingToken::new(3),
                    checkpoint_version: CheckpointVersion::new(1),
                },
            )
            .await
            .expect("register");

        // The strict importer refuses the zero-rewrite graph as a TYPED load
        // error for this session - not a panic, not an adoption.
        let error = meerkat::SessionStore::load(&adapter, &session_id)
            .await
            .expect_err("the shipping strict importer must refuse a zero-rewrite graph");
        assert!(
            error.to_string().contains("import"),
            "the refusal must surface as the typed import failure: {error}"
        );

        // Fail-closed means UNTOUCHED: the durable row still holds the exact
        // released bytes (no partial adoption, no rewrite, no deletion), and
        // an unrelated session on the same store still operates.
        {
            let conn = rusqlite::Connection::open(&db_path).expect("probe connection");
            let preserved: Vec<u8> = conn
                .query_row(
                    "SELECT data FROM session_snapshots WHERE session_id = ?1",
                    rusqlite::params![session_id.to_string()],
                    |row| row.get(0),
                )
                .expect("released row preserved");
            assert_eq!(
                preserved, RELEASED,
                "the refusal must leave the released bytes byte-identical"
            );
        }
        let healthy = meerkat_core::Session::new();
        meerkat::SessionStore::save(&adapter, &healthy)
            .await
            .expect("an unrelated session must keep working on the same store");
    }

    /// Released strand rows in the FROZEN 0.8.10 wire shapes, with a head
    /// commitment minted by the pin's own released digest recomputation, so
    /// the import proof chain is exercised end to end.
    ///
    /// FIXTURE PROVENANCE (documented schema-level synthesis, lead-authorized
    /// 2026-08-01): the row bytes follow the frozen released decoders in
    /// meerkat import_0810.rs but were NOT minted by a released binary - the
    /// released realms committed under tests/fixtures carry no
    /// rewrite-carrying heads (rewrite_count is 0 in all four), and the
    /// failing fleet shape is exactly a rewrite-carrying released head.
    /// Replace with a real released-binary capture if one becomes available.
    fn seed_released_rewrite_carrying_head(
        db_path: &std::path::Path,
        session_id: &meerkat_core::types::SessionId,
        identity: &AgentIdentity,
    ) -> Vec<Vec<u8>> {
        let rows: Vec<Vec<u8>> = vec![
            br#"{"role":"system","content":"released prompt","mutation_kind":"explicit_build","created_at":"2026-06-01T00:00:00Z"}"#.to_vec(),
            br#"{"role":"user","content":"released question","created_at":"2026-06-01T00:00:01Z"}"#.to_vec(),
        ];
        let head_revision = meerkat_core::released_0810_transcript_serialized_rows_digest(&rows)
            .expect("released digest recomputation");
        let head_json = serde_json::json!({
            "id": session_id.to_string(),
            "version": 2u32,
            "strand": "root",
            "head_revision": head_revision,
            "message_count": 2u64,
            "rewrite_count": 2u64,
            "created_at": {"secs_since_epoch": 1_780_000_000u64, "nanos_since_epoch": 0u32},
            "updated_at": {"secs_since_epoch": 1_780_000_000u64, "nanos_since_epoch": 0u32},
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_tokens": null,
                "cache_read_tokens": null
            },
            "metadata": {},
        });
        let conn = rusqlite::Connection::open(db_path).expect("seed connection");
        // The released deployment's file already carries the head-canonical
        // tables and the ledger stamp (the downstream app's closure's ledger row is
        // meta-continuity=2); a freshly opened store creates them lazily on
        // the first delta write, so the seed converges the schema itself.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS continuity_session_heads (
                session_id     TEXT PRIMARY KEY,
                identity       TEXT NOT NULL,
                generation     INTEGER NOT NULL,
                checkpoint_version INTEGER NOT NULL,
                fencing_token  INTEGER NOT NULL,
                head_revision  TEXT NOT NULL,
                message_count  INTEGER NOT NULL,
                rewrite_count  INTEGER NOT NULL,
                head_json      BLOB NOT NULL,
                cas_token      TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS continuity_strand_messages (
                session_id     TEXT NOT NULL,
                strand         TEXT NOT NULL,
                seq            INTEGER NOT NULL,
                message_json   BLOB NOT NULL,
                identity       TEXT NOT NULL,
                generation     INTEGER NOT NULL,
                created_at_ms  INTEGER NOT NULL,
                PRIMARY KEY (session_id, strand, seq)
            );
            CREATE TABLE IF NOT EXISTS continuity_session_rewrites (
                session_id     TEXT NOT NULL,
                rewrite_idx    INTEGER NOT NULL,
                parent_strand  TEXT NOT NULL,
                parent_len     INTEGER NOT NULL,
                strand         TEXT NOT NULL,
                strand_len     INTEGER NOT NULL,
                commit_json    BLOB NOT NULL,
                identity       TEXT NOT NULL,
                generation     INTEGER NOT NULL,
                created_at_ms  INTEGER NOT NULL,
                PRIMARY KEY (session_id, rewrite_idx)
            );
            INSERT INTO meerkat_schema (domain, version) VALUES ('meta-continuity', 2)
              ON CONFLICT(domain) DO UPDATE SET version = MAX(version, 2);",
        )
        .expect("converge released head-canonical schema for the seed");
        conn.execute(
            "INSERT INTO continuity_session_heads \
             (session_id, identity, generation, checkpoint_version, fencing_token, \
              head_revision, message_count, rewrite_count, head_json, cas_token) \
             VALUES (?1, ?2, 0, 1, 3, ?3, 2, 2, ?4, 'released-cas-token')",
            rusqlite::params![
                session_id.to_string(),
                identity.to_string(),
                head_revision,
                serde_json::to_vec(&head_json).expect("head json"),
            ],
        )
        .expect("seed released head row");
        for (seq, row) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO continuity_strand_messages \
                 (session_id, strand, seq, message_json, identity, generation, created_at_ms) \
                 VALUES (?1, 'root', ?2, ?3, ?4, 0, 0)",
                rusqlite::params![
                    session_id.to_string(),
                    seq as i64,
                    row,
                    identity.to_string()
                ],
            )
            .expect("seed released strand row");
        }
        rows
    }

    async fn register_released_adoption_session(
        store: &Arc<LocalContinuityStore>,
        adapter: &ContinuitySessionStoreAdapter,
        session_id: &meerkat_core::types::SessionId,
        identity: &AgentIdentity,
        runtime_id: &str,
    ) {
        store
            .upsert_continuity_record(
                &ContinuityRecord {
                    identity: identity.clone(),
                    agent_runtime_id: AgentRuntimeId::parse(runtime_id).expect("runtime id"),
                    session_id: session_id.clone(),
                    generation: ContinuityGeneration::new(0),
                    checkpoint_version: CheckpointVersion::new(1),
                },
                FencingToken::new(3),
            )
            .await
            .expect("seed record");
        adapter
            .register_session(
                session_id,
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: ContinuityGeneration::new(0),
                    fencing_token: FencingToken::new(3),
                    checkpoint_version: CheckpointVersion::new(1),
                },
            )
            .await
            .expect("register");
    }

    /// The PR #304 CI wedge, pinned at the adapter seam: LIVE reset replaces
    /// the identity's continuity record (new generation, new session, new
    /// fence) FIRST and retires the superseded runtime through deferred
    /// cleanup debt - deliberately, per the reset contract. In that window
    /// the superseded session's runtime still commits boundaries, and its
    /// durable projection used to propagate the cursor refusal ("continuity
    /// record not found"), failing the committing verb, escalating the
    /// runtime to repair-blocked retention, wedging the deferred retire
    /// behind it, and blowing the gateway's bounded shutdown horizon. The
    /// projection now drops with the superseded-terminal-write semantics -
    /// and with NO persistent mark, so a reset ROLLBACK re-enforces the same
    /// session's writes cleanly.
    #[tokio::test]
    async fn live_reset_superseded_projection_drops_instead_of_wedging() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:live-reset").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:live-reset:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        store
            .upsert_continuity_record(&record, FencingToken::new(3))
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: ContinuityGeneration::new(0),
                    fencing_token: FencingToken::new(3),
                    checkpoint_version: CheckpointVersion::new(0),
                },
            )
            .await
            .expect("register");
        let mut doc = session.clone();
        doc.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("turn 1"),
        ));
        meerkat::SessionStore::save(&adapter, &doc)
            .await
            .expect("initial save");

        // THE LIVE RESET SHAPE: the identity's record advances (generation,
        // session, fence) while the old session stays registered and its
        // runtime keeps committing.
        let new_session = meerkat_core::Session::new();
        store
            .upsert_continuity_record(
                &ContinuityRecord {
                    identity: identity.clone(),
                    agent_runtime_id: AgentRuntimeId::parse("rt:agent:live-reset:1")
                        .expect("runtime id"),
                    session_id: new_session.id().clone(),
                    generation: ContinuityGeneration::new(1),
                    checkpoint_version: CheckpointVersion::new(0),
                },
                FencingToken::new(4),
            )
            .await
            .expect("reset-shape record replacement");

        let mut post_reset = doc.clone();
        post_reset.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("in-flight turn tail"),
        ));
        meerkat::SessionStore::save_authoritative_projection(&adapter, &post_reset)
            .await
            .expect(
                "a projection superseded by a live reset must DROP, not fail the committing \
                 verb into repair-blocked retention (the gateway shutdown wedge)",
            );
        let durable = meerkat::SessionStore::load(&adapter, session.id())
            .await
            .expect("post-drop load")
            .expect("post-drop document");
        assert_eq!(
            durable.messages().len(),
            1,
            "the dropped projection must not have written"
        );

        // NO POISON: the drop is scoped to the superseded session only. The
        // reset's REPLACEMENT session (the live generation) registers and
        // writes normally - a wedged-or-poisoned adapter would refuse here.
        // (A same-session "rollback" is not a store-level operation: durable
        // head rows are generation-stamped and only the reset machinery's own
        // compensation may migrate them; that flow is exercised end-to-end by
        // the gateway reset tests.)
        adapter
            .register_session(
                new_session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: ContinuityGeneration::new(1),
                    fencing_token: FencingToken::new(4),
                    checkpoint_version: CheckpointVersion::new(0),
                },
            )
            .await
            .expect("replacement-session registration at the live generation");
        let mut replacement_doc = new_session.clone();
        replacement_doc.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("first turn on the replacement"),
        ));
        meerkat::SessionStore::save_authoritative_projection(&adapter, &replacement_doc)
            .await
            .expect("the live generation's projection must enforce and write after the drop");
        let durable = meerkat::SessionStore::load(&adapter, new_session.id())
            .await
            .expect("replacement load")
            .expect("replacement document");
        assert_eq!(
            durable.messages().len(),
            1,
            "the replacement projection must land durably (drop poisoned nothing)"
        );
        let old = meerkat::SessionStore::load(&adapter, session.id())
            .await
            .expect("superseded load")
            .expect("superseded document");
        assert_eq!(
            old.messages().len(),
            1,
            "the superseded session's durable state stays exactly pre-reset"
        );
    }

    /// A boot-2 exactly-once violation seen in the field, pinned on head rows
    /// of that shape with synthetic content
    /// (fixtures/security_idempotency/, sha256 2f2c8443...,
    /// domain:security 019fae11-4e87-...): two consecutive boots of the same
    /// binary wrote two byte-different heads for an unchanged document. The
    /// diffs are exactly `updated_at` plus the ORDER of the tool-visibility
    /// Allow arrays (upstream projects `ToolNameSet`, a HashSet, through
    /// serde - per-process hash order). Strict equality must SEE the drift
    /// (else this fixture rotted) while the exact-resave equality must
    /// recognize it as zero durable change - the pin that stops the
    /// per-boot head rewrite.
    #[test]
    fn downstream_security_boot_drift_is_zero_durable_change() {
        const BUNDLE: &[u8] = include_bytes!(
            "../../tests/fixtures/security_idempotency/security-head-evolution.json"
        );
        let bundle: serde_json::Value = serde_json::from_slice(BUNDLE).expect("bundle JSON");
        let head_of = |state: &str| -> meerkat_core::session_store::SessionHead {
            use base64::Engine as _;
            let table = &bundle[state]["continuity_session_heads"];
            let columns: Vec<&str> = table["columns"]
                .as_array()
                .expect("bundle columns")
                .iter()
                .map(|c| c.as_str().expect("bundle column"))
                .collect();
            let row = table["rows"][0].as_array().expect("bundle row");
            let head_json = base64::engine::general_purpose::STANDARD
                .decode(
                    row[columns
                        .iter()
                        .position(|c| *c == "head_json")
                        .expect("head_json column")]["b64"]
                        .as_str()
                        .expect("head_json b64"),
                )
                .expect("head_json base64");
            serde_json::from_slice(&head_json).expect("bundle head deserializes")
        };
        let boot1 = head_of("post_boot1_fresh");
        let boot2 = head_of("post_boot2");
        assert_ne!(
            boot1, boot2,
            "the fleet drift must be visible to strict equality; if the two rows became \
             identical, the fixture (or upstream's stamp) changed and this pin needs re-deriving"
        );
        assert!(
            head_equal_for_exact_resave(&boot1, &boot2),
            "the exact-resave equality must recognize the fleet's boot drift as zero durable \
             change (updated_at + visibility set order only)"
        );
        assert!(
            head_equal_for_exact_resave(&boot2, &boot1),
            "zero-durable-change recognition must be symmetric"
        );
    }

    /// The downstream app's class-3 binding failure, pinned: a released 0.8.10 head
    /// that RETAINS REWRITES cannot authorize a current mutation
    /// (`session_head_cas_token` refuses "rewritten current head has no
    /// compact graph-prefix authority" - the rewrite-generation authority
    /// predates the compact graph/rewrite-prefix carriers), so before the
    /// adoption lane the FIRST projected boundary write at boot failed
    /// 17/17: the head-lane import made the corpus readable and the
    /// write-back made it a dead end. The released realms in the R1 lane all
    /// carry rewrite_count 0, which is why that lane stayed green across
    /// this failure class.
    #[tokio::test]
    async fn released_rewrite_carrying_head_adopts_on_first_projected_write() {
        let identity = AgentIdentity::parse("agent:released-adoption").expect("identity");
        let session_id = meerkat_core::types::SessionId::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("continuity.sqlite3");
        let store = Arc::new(LocalContinuityStore::open(&db_path).expect("store"));
        seed_released_rewrite_carrying_head(&db_path, &session_id, &identity);
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        register_released_adoption_session(
            &store,
            &adapter,
            &session_id,
            &identity,
            "rt:agent:released-adoption:0",
        )
        .await;

        // Read side (the class-2 lane, already green): the released head
        // imports on load.
        let imported = meerkat::SessionStore::load(&adapter, &session_id)
            .await
            .expect("import-on-load")
            .expect("imported document");
        assert_eq!(imported.messages().len(), 2);

        // The boot shape: the first projected boundary writes the imported
        // reading back. Before the adoption lane this refused with the fleet
        // error; now the released representation is adopted wholesale.
        meerkat::SessionStore::save_authoritative_projection(&adapter, &imported)
            .await
            .expect("the first projected write must ADOPT the released head, not refuse");
        let adopted_head =
            super::super::contracts::ContinuityIncrementalSessions::load_canonical_head(
                store.as_ref(),
                &session_id,
            )
            .await
            .expect("adopted head read")
            .expect("adopted head present");
        assert_ne!(
            adopted_head.version,
            super::super::contracts::RELEASED_0810_SESSION_ENVELOPE_VERSION,
            "adoption must leave a CURRENT head, not the released one"
        );
        assert_eq!(
            adopted_head.rewrite_count, 0,
            "the imported reading carries no retained history; the adopted head starts a \
             current lineage"
        );
        let readable = meerkat::SessionStore::load(&adapter, &session_id)
            .await
            .expect("post-adoption load")
            .expect("post-adoption document");
        assert_eq!(
            readable.messages().len(),
            imported.messages().len(),
            "adoption must preserve the imported document"
        );

        // The ordinary arms own the session from here: a plain append works
        // and no longer routes through adoption.
        let mut extended = readable.clone();
        extended.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("post-adoption turn"),
        ));
        meerkat::SessionStore::save(&adapter, &extended)
            .await
            .expect("ordinary append after adoption");

        // Divergence direction pin: a document that is NOT a successor of
        // the imported reading refuses typed inside the adoption lane, and
        // the released representation survives untouched.
        let fork_id = meerkat_core::types::SessionId::new();
        let fork_identity = AgentIdentity::parse("agent:released-adoption-fork").expect("identity");
        seed_released_rewrite_carrying_head(&db_path, &fork_id, &fork_identity);
        register_released_adoption_session(
            &store,
            &adapter,
            &fork_id,
            &fork_identity,
            "rt:agent:released-adoption-fork:0",
        )
        .await;
        let imported_fork = meerkat::SessionStore::load(&adapter, &fork_id)
            .await
            .expect("fork import-on-load")
            .expect("fork imported document");
        let mut doc: serde_json::Value = serde_json::from_slice(
            &imported_fork
                .to_persisted_bytes()
                .expect("fork document bytes"),
        )
        .expect("fork document JSON");
        doc["messages"][1]["content"] = serde_json::Value::String("FORKED".to_string());
        let fork = meerkat_core::Session::from_persisted_bytes(
            &serde_json::to_vec(&doc).expect("fork bytes"),
        )
        .expect("fork decodes");
        let refused = meerkat::SessionStore::save_authoritative_projection(&adapter, &fork)
            .await
            .expect_err("a non-successor of the imported reading must refuse typed");
        assert!(
            refused.to_string().contains("not a continuation"),
            "the adoption lane's refusal must be the boundary guard's continuity violation: \
             {refused}"
        );
        let preserved = meerkat::SessionStore::load(&adapter, &fork_id)
            .await
            .expect("post-refusal load")
            .expect("post-refusal document");
        assert_eq!(
            preserved.messages().len(),
            2,
            "the refused adoption must leave the released document untouched"
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_parallelizes_different_sessions() {
        let store = Arc::new(ConcurrentLoadStore::new(2));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let first = meerkat_core::Session::new();
        let second = meerkat_core::Session::new();
        assert_ne!(first.id(), second.id());

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            let (first_result, second_result) = tokio::join!(
                meerkat::SessionStore::save(&adapter, &first),
                meerkat::SessionStore::save(&adapter, &second),
            );
            first_result.expect("first save");
            second_result.expect("second save");
        })
        .await
        .expect("different session IDs must not share one global save lock");

        assert_eq!(
            store.max_in_flight.load(AtomicOrdering::SeqCst),
            2,
            "both independent session loads should overlap"
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_exact_resave_is_a_noop() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:exact-resave").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:exact-resave:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(3);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");

        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("initial save");
        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("exact resave");

        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .expect("resolve");
        let ContinuityResolveState::Ready { record } = resolved.get(&identity).expect("record")
        else {
            panic!("expected ready record");
        };
        assert_eq!(
            record.checkpoint_version,
            CheckpointVersion::new(1),
            "an exact durable resave must not manufacture a new checkpoint"
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_abandons_only_superseded_snapshot() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let identity = AgentIdentity::parse("agent:reset-abandon").expect("identity");
        let old_session = meerkat_core::Session::new();
        let old_record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:reset-abandon:0")
                .expect("old runtime id"),
            session_id: old_session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let old_fence = FencingToken::new(3);
        store
            .upsert_continuity_record(&old_record, old_fence)
            .await
            .expect("seed old record");
        adapter
            .register_session(
                old_session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: old_record.generation,
                    fencing_token: old_fence,
                    checkpoint_version: old_record.checkpoint_version,
                },
            )
            .await
            .expect("register old session");
        meerkat::SessionStore::save(&adapter, &old_session)
            .await
            .expect("persist old session");

        let replacement_session = meerkat_core::types::SessionId::new();
        let replacement_record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:reset-abandon:1")
                .expect("replacement runtime id"),
            session_id: replacement_session.clone(),
            generation: ContinuityGeneration::new(1),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let replacement_fence = FencingToken::new(4);
        store
            .upsert_continuity_record(&replacement_record, replacement_fence)
            .await
            .expect("commit replacement record");

        adapter
            .abandon_superseded_session(old_session.id())
            .await
            .expect("abandon old projection");
        assert!(
            store
                .load_session_snapshot(old_session.id())
                .await
                .expect("load old snapshot")
                .is_none(),
            "the exact superseded snapshot must be CAS-deleted"
        );
        assert_eq!(
            store
                .resolve_many(std::slice::from_ref(&identity))
                .await
                .expect("resolve replacement")
                .get(&identity),
            Some(&ContinuityResolveState::Ready {
                record: replacement_record
            }),
            "abandonment must not disturb the replacement continuity head"
        );

        // Meerkat's exact retained retire retry realizes an Archived document
        // write before removing its roster anchor. The superseded tombstone
        // acknowledges that write without recreating the old snapshot.
        meerkat::SessionStore::save_authoritative_projection(&adapter, &old_session)
            .await
            .expect("terminal superseded projection is acknowledged");
        assert!(
            store
                .load_session_snapshot(old_session.id())
                .await
                .expect("reload old snapshot")
                .is_none()
        );

        adapter
            .unregister_session(old_session.id())
            .await
            .expect("finalize old session authority");
        assert!(
            meerkat::SessionStore::save(&adapter, &old_session)
                .await
                .is_err(),
            "late writes must fail closed after structural member absence"
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_retries_failed_superseded_snapshot_cas() {
        let inner = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let store = Arc::new(FailSaveContinuityStore::new(inner.clone()));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let identity = AgentIdentity::parse("agent:reset-abandon-retry").expect("identity");
        let session = meerkat_core::Session::new();
        let old_record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:reset-abandon-retry:0")
                .expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let old_fence = FencingToken::new(7);
        store
            .upsert_continuity_record(&old_record, old_fence)
            .await
            .expect("seed old record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: old_record.generation,
                    fencing_token: old_fence,
                    checkpoint_version: old_record.checkpoint_version,
                },
            )
            .await
            .expect("register old session");
        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("persist old session");
        let replacement = ContinuityRecord {
            identity,
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:reset-abandon-retry:1")
                .expect("replacement runtime id"),
            session_id: meerkat_core::types::SessionId::new(),
            generation: ContinuityGeneration::new(1),
            checkpoint_version: CheckpointVersion::new(0),
        };
        store
            .upsert_continuity_record(&replacement, FencingToken::new(8))
            .await
            .expect("commit replacement");

        store.fail_next_delete();
        assert!(
            adapter
                .abandon_superseded_session(session.id())
                .await
                .is_err(),
            "the injected CAS failure must remain visible"
        );
        assert!(adapter.session_was_suspended(session.id()));
        assert!(
            inner
                .load_session_snapshot(session.id())
                .await
                .expect("snapshot after failed abandon")
                .is_some()
        );

        adapter
            .abandon_superseded_session(session.id())
            .await
            .expect("retry exact CAS abandon");
        assert!(adapter.session_was_superseded(session.id()));
        assert!(
            inner
                .load_session_snapshot(session.id())
                .await
                .expect("snapshot after retry")
                .is_none()
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_exact_bytes_do_not_mask_a_newer_fence() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:exact-stale-fence").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:exact-stale-fence:0")
                .expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(3);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");
        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("initial save");

        store
            .upsert_continuity_record(&record, FencingToken::new(4))
            .await
            .expect("advance durable fence");
        let error = meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect_err("the stale registered fence must still be rejected");
        assert!(
            error.to_string().contains("stale fencing token"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_seeds_registered_checkpoint_version() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:restored").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:restored:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(2),
            checkpoint_version: CheckpointVersion::new(5),
        };
        let fencing_token = FencingToken::new(9);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");

        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");

        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("save should advance from restored checkpoint");
        let effective_version = adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("post-save register should report advanced version");
        assert_eq!(effective_version, CheckpointVersion::new(6));
        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .expect("resolve");
        let ContinuityResolveState::Ready { record } = resolved.get(&identity).expect("record")
        else {
            panic!("expected ready record");
        };
        assert_eq!(record.checkpoint_version, CheckpointVersion::new(6));
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_flushes_pending_save_under_registered_identity() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:fresh").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:fresh:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(3);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");

        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("unregistered save should be delayed, not written under fallback identity");
        assert!(
            store
                .load_session_snapshot(session.id())
                .await
                .expect("load before register")
                .is_none(),
            "unregistered save must not be visible in continuity store"
        );

        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register flushes pending");

        assert!(
            store
                .load_session_snapshot(session.id())
                .await
                .expect("load after register")
                .is_some(),
            "pending save should flush under the registered identity"
        );
        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .expect("resolve");
        let ContinuityResolveState::Ready { record } = resolved.get(&identity).expect("record")
        else {
            panic!("expected ready record");
        };
        assert_eq!(record.checkpoint_version, CheckpointVersion::new(1));
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_rejects_saves_after_unregister() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:retired").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:retired:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(9);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");

        adapter
            .unregister_session(session.id())
            .await
            .expect("unregister");
        let err = meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect_err("post-unregister save must fail closed");
        assert!(
            err.to_string().contains("was unregistered"),
            "unexpected error: {err}"
        );
        assert!(
            store
                .load_session_snapshot(session.id())
                .await
                .expect("load")
                .is_none(),
            "post-unregister save must not be queued as pending"
        );

        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("registering the same id later should not flush stale pending data");
        assert!(
            store
                .load_session_snapshot(session.id())
                .await
                .expect("load after re-register")
                .is_none(),
            "stale post-unregister save must not flush on a later registration"
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_suspension_blocks_every_mutation_until_reregister() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let mut session = meerkat_core::Session::new();
        session.append_external_user_content(meerkat_core::ContentInput::Text(
            "before rotation".to_string(),
        ));
        let identity = AgentIdentity::parse("agent:suspended").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:suspended:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let old_token = FencingToken::new(7);
        store
            .upsert_continuity_record(&record, old_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: record.generation,
                    fencing_token: old_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");
        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("seed snapshot");

        let parent_revision = session.transcript_revision().expect("parent revision");
        let mut rewritten = session.clone();
        let rewrite_commit = rewritten
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![meerkat_core::Message::User(
                    meerkat_core::UserMessage::text("rewritten".to_string()),
                )],
                meerkat_core::TranscriptRewriteReason::new("rotation-test"),
                Some("meta-test".to_string()),
                Some(parent_revision),
            )
            .expect("rewrite commit");

        adapter
            .suspend_session(session.id())
            .await
            .expect("suspend");

        let save_error = meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect_err("ordinary save must fail while suspended");
        let rewrite_error =
            meerkat::SessionStore::save_transcript_rewrite(&adapter, &rewritten, &rewrite_commit)
                .await
                .expect_err("transcript rewrite must fail while suspended");
        let projection_error =
            meerkat::SessionStore::save_authoritative_projection(&adapter, &session)
                .await
                .expect_err("authoritative projection must fail while suspended");
        let projection_cas_error =
            meerkat::SessionStore::save_authoritative_projection_if_current_revision(
                &adapter, &session, None,
            )
            .await
            .expect_err("authoritative projection CAS must fail while suspended");
        let delete_error = meerkat::SessionStore::delete(&adapter, session.id())
            .await
            .expect_err("delete must fail while suspended");
        let delete_cas_error = meerkat::SessionStore::delete_if_current_revision(
            &adapter,
            session.id(),
            "row-sha256:any",
        )
        .await
        .expect_err("delete CAS must fail while suspended");

        for error in [
            save_error,
            rewrite_error,
            projection_error,
            projection_cas_error,
            delete_error,
            delete_cas_error,
        ] {
            assert!(
                error.to_string().contains("persistence is suspended"),
                "unexpected suspension error: {error}"
            );
        }

        let new_token = FencingToken::new(8);
        store
            .upsert_continuity_record(&record, new_token)
            .await
            .expect("publish replacement authority");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token: new_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("re-register replacement authority");
        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("writes resume only after exact replacement registration");
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_suspension_drains_admitted_save() {
        let inner = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let store = Arc::new(FailSaveContinuityStore::new(inner.clone()));
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(store.clone()));
        let session = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:suspend-drain").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:suspend-drain:0")
                .expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(17);
        inner
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");

        store.block_one_save();
        let save_adapter = adapter.clone();
        let save_session = session.clone();
        let save_task = tokio::spawn(async move {
            meerkat::SessionStore::save(save_adapter.as_ref(), &save_session).await
        });
        store.wait_for_blocked_save().await;

        let suspend_adapter = adapter.clone();
        let session_id = session.id().clone();
        let mut suspend_task =
            tokio::spawn(async move { suspend_adapter.suspend_session(&session_id).await });
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut suspend_task)
                .await
                .is_err(),
            "suspension must wait for the already-admitted save to leave the session lock"
        );

        store.release_blocked_save();
        save_task
            .await
            .expect("save task joins")
            .expect("admitted save completes before suspension");
        suspend_task
            .await
            .expect("suspend task joins")
            .expect("suspension completes after drain");

        let error = meerkat::SessionStore::save(adapter.as_ref(), &session)
            .await
            .expect_err("later saves must see the suspension barrier");
        assert!(error.to_string().contains("persistence is suspended"));
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_register_keeps_pending_snapshot_on_flush_failure() {
        let inner = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let fail_store = Arc::new(FailSaveContinuityStore::new(inner.clone()));
        let adapter = ContinuitySessionStoreAdapter::new(fail_store.clone());
        let mut session = meerkat_core::Session::new();
        session.set_metadata("pending", json!(true));
        let identity = AgentIdentity::parse("agent:pending-fail").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:pending-fail:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(14);
        inner
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");

        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("pending save");
        fail_store.fail_saves(true);
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect_err("forced pending flush failure");
        assert!(
            meerkat::SessionStore::load(&adapter, session.id())
                .await
                .expect("load after failed register")
                .is_none(),
            "failed register must not leave a synthetic registered session"
        );

        fail_store.fail_saves(false);
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("retry register should flush preserved pending snapshot");
        let loaded = meerkat::SessionStore::load(&adapter, session.id())
            .await
            .expect("load after retry")
            .expect("snapshot");
        assert_eq!(loaded.metadata().get("pending"), Some(&json!(true)));
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_lost_ack_consumes_checkpoint_version() {
        let inner = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let fail_store = Arc::new(FailSaveContinuityStore::new(inner.clone()));
        let adapter = ContinuitySessionStoreAdapter::new(fail_store.clone());
        let session = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:lost-ack").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:lost-ack:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(23);
        inner
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("queue pre-registration snapshot");

        let state = SessionRuntimeState {
            identity: identity.clone(),
            generation: record.generation,
            fencing_token,
            checkpoint_version: record.checkpoint_version,
        };
        fail_store.commit_then_fail_next_save();
        adapter
            .register_session(session.id(), state.clone())
            .await
            .expect_err("first flush commits version 1 but loses its acknowledgement");

        let effective = adapter
            .register_session(session.id(), state)
            .await
            .expect("retry must allocate a fresh checkpoint version");
        assert_eq!(effective, CheckpointVersion::new(2));
        let resolved = inner
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .expect("resolve");
        let ContinuityResolveState::Ready { record } = resolved.get(&identity).expect("record")
        else {
            panic!("expected ready record");
        };
        assert_eq!(record.checkpoint_version, CheckpointVersion::new(2));
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_rejects_owner_generation_and_fence_regression() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store);
        let session = meerkat_core::Session::new();
        let first = SessionRuntimeState {
            identity: AgentIdentity::parse("agent:owner-a").expect("identity"),
            generation: ContinuityGeneration::new(4),
            fencing_token: FencingToken::new(10),
            checkpoint_version: CheckpointVersion::new(7),
        };
        adapter
            .register_session(session.id(), first.clone())
            .await
            .expect("initial registration");

        let foreign_owner = SessionRuntimeState {
            identity: AgentIdentity::parse("agent:owner-b").expect("identity"),
            ..first.clone()
        };
        adapter
            .register_session(session.id(), foreign_owner)
            .await
            .expect_err("a session id cannot be rebound to another identity");

        let foreign_generation = SessionRuntimeState {
            generation: ContinuityGeneration::new(5),
            ..first.clone()
        };
        adapter
            .register_session(session.id(), foreign_generation)
            .await
            .expect_err("a session id cannot be rebound to another generation");

        let greater = SessionRuntimeState {
            fencing_token: FencingToken::new(11),
            ..first.clone()
        };
        adapter
            .register_session(session.id(), greater.clone())
            .await
            .expect("a monotonic fence may replace the prior write authority");
        adapter
            .suspend_session(session.id())
            .await
            .expect("suspend replacement authority");

        let regressed = SessionRuntimeState {
            fencing_token: FencingToken::new(9),
            ..first.clone()
        };
        adapter
            .register_session(session.id(), regressed)
            .await
            .expect_err("suspension must never authorize fence regression");
        adapter
            .register_session(session.id(), greater.clone())
            .await
            .expect("the same current fence resumes suspended persistence");
        assert_eq!(
            adapter.lookup_session(&session.id().to_string()),
            Some(greater),
            "rejected registrations must not replace the owner"
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_delete_if_current_revision_removes_matching_snapshot()
    {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:quarantine").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:quarantine:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(4);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");
        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("save snapshot");

        let stale_revision = "row-sha256:not-current".to_string();
        assert!(
            !meerkat::SessionStore::delete_if_current_revision(
                &adapter,
                session.id(),
                &stale_revision
            )
            .await
            .expect("stale delete should be clean"),
            "stale revision must not delete"
        );
        assert!(
            store
                .load_session_snapshot(session.id())
                .await
                .expect("load after stale")
                .is_some(),
            "stale CAS delete must leave snapshot in place"
        );

        let current_revision =
            meerkat_core::session_store::session_projection_cas_token(&session).expect("revision");
        assert!(
            meerkat::SessionStore::delete_if_current_revision(
                &adapter,
                session.id(),
                &current_revision
            )
            .await
            .expect("matching delete should succeed"),
            "matching revision should delete"
        );
        assert!(
            store
                .load_session_snapshot(session.id())
                .await
                .expect("load after delete")
                .is_none(),
            "matching CAS delete must remove the continuity snapshot"
        );
        assert!(
            meerkat::SessionStore::load(&adapter, session.id())
                .await
                .expect("adapter load after delete")
                .is_none(),
            "adapter must not synthesize a session after successful CAS delete"
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_save_rejects_transcript_shrink() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let mut session = meerkat_core::Session::new();
        session.append_external_user_content(meerkat_core::ContentInput::Text("first".to_string()));
        session
            .append_external_user_content(meerkat_core::ContentInput::Text("second".to_string()));
        let identity = AgentIdentity::parse("agent:append-only").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:append-only:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(12);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");
        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("initial save");

        let mut stale = meerkat_core::Session::with_id(session.id().clone());
        stale.append_external_user_content(meerkat_core::ContentInput::Text("first".to_string()));
        let err = meerkat::SessionStore::save(&adapter, &stale)
            .await
            .expect_err("plain save must reject transcript shrink");
        assert!(
            err.to_string().contains("transcript")
                || err.to_string().contains("monotonicity")
                || err.to_string().contains("continuity"),
            "unexpected shrink error: {err}"
        );
    }

    /// Projection rollback is owned by Meerkat's PersistentSessionService.
    /// The raw store adapter remains append-only and must not independently
    /// reinterpret a stamped longer row as permission to fabricate rollback.
    #[tokio::test]
    #[allow(deprecated)] // Exercises compatibility with legacy stamped rows.
    async fn raw_save_rejects_stamped_checkpoint_residue_rollback() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let mut authority = meerkat_core::Session::new();
        authority
            .append_external_user_content(meerkat_core::ContentInput::Text("first".to_string()));
        authority
            .append_external_user_content(meerkat_core::ContentInput::Text("second".to_string()));
        let identity = AgentIdentity::parse("agent:torn-shutdown").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:torn-shutdown:0")
                .expect("runtime id"),
            session_id: authority.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(21);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                authority.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");
        meerkat::SessionStore::save(&adapter, &authority)
            .await
            .expect("boundary save of the authority");

        // The intra-turn checkpointer persists a head strictly ahead of the
        // boundary commit; the host dies before the boundary lands. (0.8.11:
        // the retired runtime-checkpoint provenance stamp no longer exists;
        // the rollback guard under test is provenance-independent.)
        let mut stamped_head = authority.clone();
        stamped_head
            .append_external_user_content(meerkat_core::ContentInput::Text("mid-turn".to_string()));
        meerkat::SessionStore::save(&adapter, &stamped_head)
            .await
            .expect("checkpointer save of the stamped head");

        // The authoritative Meerkat service may classify and drive this
        // repair through its CAS seam. A direct adapter call may not.
        let error = meerkat::SessionStore::save(&adapter, &authority)
            .await
            .expect_err("raw save must reject transcript rollback");
        assert!(
            error.to_string().contains("transcript")
                || error.to_string().contains("monotonicity")
                || error.to_string().contains("continuation"),
            "unexpected rollback error: {error}"
        );
        let loaded = meerkat::SessionStore::load(&adapter, authority.id())
            .await
            .expect("load")
            .expect("session");
        assert_eq!(
            loaded.messages().len(),
            stamped_head.messages().len(),
            "rejected raw rollback must leave the durable stamped row unchanged"
        );
    }

    /// The rollback requires BOTH observations: a head that extends the
    /// authority but does NOT carry the checkpointer's provenance stamp is
    /// out-of-band divergence and must keep failing closed.
    #[tokio::test]
    async fn save_keeps_rejecting_unstamped_longer_heads() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let mut authority = meerkat_core::Session::new();
        authority
            .append_external_user_content(meerkat_core::ContentInput::Text("first".to_string()));
        let identity = AgentIdentity::parse("agent:unstamped").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:unstamped:0").expect("runtime id"),
            session_id: authority.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(22);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                authority.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");
        meerkat::SessionStore::save(&adapter, &authority)
            .await
            .expect("initial save");
        let mut unstamped_head = authority.clone();
        unstamped_head
            .append_external_user_content(meerkat_core::ContentInput::Text("tail".to_string()));
        meerkat::SessionStore::save(&adapter, &unstamped_head)
            .await
            .expect("longer head appends fine");

        meerkat::SessionStore::save(&adapter, &authority)
            .await
            .expect_err("unstamped longer head must keep failing closed");
    }

    /// A stamped head whose content FORKS from the incoming authority (not a
    /// faithful continuation) resolves `RejectDivergent` — stamp alone never
    /// authorizes the rollback.
    #[tokio::test]
    #[allow(deprecated)] // Exercises compatibility with legacy stamped rows.
    async fn save_keeps_rejecting_stamped_forked_heads() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let mut base = meerkat_core::Session::new();
        base.append_external_user_content(meerkat_core::ContentInput::Text("first".to_string()));
        let identity = AgentIdentity::parse("agent:forked").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:forked:0").expect("runtime id"),
            session_id: base.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(23);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                base.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");
        let mut stamped_fork = base.clone();
        stamped_fork
            .append_external_user_content(meerkat_core::ContentInput::Text("forked".to_string()));
        meerkat::SessionStore::save(&adapter, &stamped_fork)
            .await
            .expect("seed the stamped head");

        // A DIVERGENT authority (same length as base + different tail) is a
        // content fork relative to the persisted head, not its prefix.
        let mut diverged = base.clone();
        diverged
            .append_external_user_content(meerkat_core::ContentInput::Text("other".to_string()));
        diverged
            .append_external_user_content(meerkat_core::ContentInput::Text("branch".to_string()));
        meerkat::SessionStore::save(&adapter, &diverged)
            .await
            .expect_err("stamped but forked head must keep failing closed");
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_saves_transcript_rewrite() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let mut session = meerkat_core::Session::new();
        session.append_external_user_content(meerkat_core::ContentInput::Text("first".to_string()));
        session
            .append_external_user_content(meerkat_core::ContentInput::Text("second".to_string()));
        let identity = AgentIdentity::parse("agent:rewrite").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:rewrite:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(13);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");
        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("initial save");

        let parent_revision = session.transcript_revision().expect("parent revision");
        let mut rewritten = session.clone();
        let commit = rewritten
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![meerkat_core::Message::User(
                    meerkat_core::UserMessage::text("compacted first".to_string()),
                )],
                meerkat_core::TranscriptRewriteReason::new("test"),
                Some("meta-test".to_string()),
                Some(parent_revision),
            )
            .expect("rewrite commit");

        meerkat::SessionStore::save_transcript_rewrite(&adapter, &rewritten, &commit)
            .await
            .expect("rewrite save should be supported");
        let loaded = meerkat::SessionStore::load(&adapter, session.id())
            .await
            .expect("load rewritten")
            .expect("rewritten session");
        assert_eq!(loaded.messages().len(), rewritten.messages().len());
        assert_eq!(
            loaded.transcript_revision().expect("loaded revision"),
            commit.revision
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_delete_removes_current_snapshot() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:delete").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:delete:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(7);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");
        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("save snapshot");

        meerkat::SessionStore::delete(&adapter, session.id())
            .await
            .expect("delete should remove current snapshot");
        assert!(
            store
                .load_session_snapshot(session.id())
                .await
                .expect("load after delete")
                .is_none(),
            "delete must not be a successful no-op"
        );
        assert!(
            meerkat::SessionStore::load(&adapter, session.id())
                .await
                .expect("adapter load after delete")
                .is_none(),
            "adapter must forget registry state after delete"
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_queues_unregistered_authoritative_projection() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();

        meerkat::SessionStore::save_authoritative_projection(&adapter, &session)
            .await
            .expect("create-time authoritative projection should queue before registration");
        assert!(
            meerkat::SessionStore::load(&adapter, session.id())
                .await
                .expect("load")
                .is_none(),
            "pending authoritative projection must stay invisible until registration"
        );

        let identity = AgentIdentity::parse("agent:queued").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:queued:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(7);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register flushes pending authoritative projection");
        assert!(
            meerkat::SessionStore::load(&adapter, session.id())
                .await
                .expect("load after register")
                .is_some(),
            "registration must flush the pending authoritative projection"
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_delete_forgets_registered_session_without_snapshot() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:delete-empty").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:delete-empty:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(11);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");
        assert!(
            meerkat::SessionStore::load(&adapter, session.id())
                .await
                .expect("load before delete")
                .is_none(),
            "registration alone must not fabricate a session document"
        );

        meerkat::SessionStore::delete(&adapter, session.id())
            .await
            .expect("delete with no persisted snapshot should be idempotent");
        assert!(
            meerkat::SessionStore::load(&adapter, session.id())
                .await
                .expect("load after delete")
                .is_none(),
            "delete must forget registry state when no persisted row exists"
        );
    }

    /// Register `session` under `identity_name`, land one durable whole-blob
    /// save, then delete it. This is the exact state the parking guard exists
    /// for: no registry entry, no unregistered/suspended/superseded marker
    /// (delete is not unregister), durable history in this process.
    async fn register_save_delete(
        store: &Arc<LocalContinuityStore>,
        adapter: &ContinuitySessionStoreAdapter,
        session: &meerkat_core::Session,
        identity_name: &str,
    ) -> AgentIdentity {
        let identity = AgentIdentity::parse(identity_name).expect("identity");
        let fencing_token = FencingToken::new(1);
        store
            .upsert_continuity_record(
                &ContinuityRecord {
                    identity: identity.clone(),
                    agent_runtime_id: AgentRuntimeId::parse(&format!("rt:{identity_name}:0"))
                        .expect("runtime id"),
                    session_id: session.id().clone(),
                    generation: ContinuityGeneration::new(0),
                    checkpoint_version: CheckpointVersion::new(0),
                },
                fencing_token,
            )
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: ContinuityGeneration::new(0),
                    fencing_token,
                    checkpoint_version: CheckpointVersion::new(0),
                },
            )
            .await
            .expect("register");
        meerkat::SessionStore::save(adapter, session)
            .await
            .expect("durable save");
        meerkat::SessionStore::delete(adapter, session.id())
            .await
            .expect("delete");
        identity
    }

    fn parked_pending_bytes(
        adapter: &ContinuitySessionStoreAdapter,
        id: &meerkat_core::types::SessionId,
    ) -> bool {
        adapter
            .pending_unregistered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&id.to_string())
    }

    /// `delete` drops the registry entry without stamping the unregistered
    /// marker, so before the guard a whole-blob save after delete parked
    /// silently and acknowledged Ok — an acknowledged write that dies with
    /// the process, or resurrects the deleted document at the next
    /// registration flush. Same refusal `route_delta_write` already has.
    #[tokio::test]
    async fn whole_blob_save_after_delete_is_refused_not_parked() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        register_save_delete(&store, &adapter, &session, "agent:park-guard-save").await;

        let refused = meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect_err("a whole-blob save after delete must refuse, not park");
        assert!(
            refused.to_string().contains("refusing to park"),
            "the refusal must speak the parking guard's vocabulary: {refused}"
        );
        assert!(
            !parked_pending_bytes(&adapter, session.id()),
            "a refused save must leave nothing parked"
        );
    }

    #[tokio::test]
    async fn transcript_rewrite_after_delete_is_refused_not_parked() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let mut session = meerkat_core::Session::new();
        session.append_external_user_content(meerkat_core::ContentInput::Text("first".to_string()));
        session
            .append_external_user_content(meerkat_core::ContentInput::Text("second".to_string()));
        register_save_delete(&store, &adapter, &session, "agent:park-guard-rewrite").await;

        let parent_revision = session.transcript_revision().expect("parent revision");
        let mut rewritten = session.clone();
        let commit = rewritten
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![meerkat_core::Message::User(
                    meerkat_core::UserMessage::text("compacted first".to_string()),
                )],
                meerkat_core::TranscriptRewriteReason::new("test"),
                Some("meta-test".to_string()),
                Some(parent_revision),
            )
            .expect("rewrite commit");

        // For this shape the missing-previous rewrite guard fires before the
        // parking arm (the deleted document is gone), and the parking guard
        // backstops the arm itself. Either refusal is fine; what must never
        // happen is Ok with bytes parked.
        meerkat::SessionStore::save_transcript_rewrite(&adapter, &rewritten, &commit)
            .await
            .expect_err("a transcript rewrite after delete must refuse, not park");
        assert!(
            !parked_pending_bytes(&adapter, rewritten.id()),
            "a refused rewrite must leave nothing parked"
        );
    }

    #[tokio::test]
    async fn authoritative_projection_after_delete_is_refused_not_parked() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        register_save_delete(&store, &adapter, &session, "agent:park-guard-projection").await;

        let refused = meerkat::SessionStore::save_authoritative_projection(&adapter, &session)
            .await
            .expect_err("an authoritative projection after delete must refuse, not park");
        assert!(
            refused.to_string().contains("refusing to park"),
            "the refusal must speak the parking guard's vocabulary: {refused}"
        );
        assert!(
            !parked_pending_bytes(&adapter, session.id()),
            "a refused projection must leave nothing parked"
        );
    }

    #[tokio::test]
    async fn authoritative_projection_cas_after_delete_is_refused_not_parked() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        register_save_delete(
            &store,
            &adapter,
            &session,
            "agent:park-guard-projection-cas",
        )
        .await;

        // Post-delete there is no durable row, so `None` is the matching
        // expectation: the CAS guard passes and the write reaches the parking
        // arm, which must refuse.
        let refused = meerkat::SessionStore::save_authoritative_projection_if_current_revision(
            &adapter, &session, None,
        )
        .await
        .expect_err("a projection CAS after delete must refuse, not park");
        assert!(
            refused.to_string().contains("refusing to park"),
            "the refusal must speak the parking guard's vocabulary: {refused}"
        );
        assert!(
            !parked_pending_bytes(&adapter, session.id()),
            "a refused projection CAS must leave nothing parked"
        );
    }

    /// The guard must not reach into the creation window: a session that has
    /// never written durably in this process still parks its
    /// pre-registration saves (member creation depends on it).
    #[tokio::test]
    async fn never_durable_unregistered_saves_still_park() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();

        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("creation-window save must park, not fail");
        assert!(
            parked_pending_bytes(&adapter, session.id()),
            "the creation-window save must be parked for the registration flush"
        );
        meerkat::SessionStore::save_authoritative_projection(&adapter, &session)
            .await
            .expect("creation-window authoritative projection must park, not fail");
        meerkat::SessionStore::save_authoritative_projection_if_current_revision(
            &adapter, &session, None,
        )
        .await
        .expect("creation-window projection CAS must park, not fail");
        assert!(
            parked_pending_bytes(&adapter, session.id()),
            "the creation window must keep parking after every whole-blob verb"
        );
    }

    /// The resurrection vector, pinned end to end: save durably, delete,
    /// attempt another save (refused, nothing parked), re-register the same
    /// session id. Registration must find nothing to flush — the deleted
    /// document stays deleted — and the id keeps working for genuinely new
    /// durable writes.
    #[tokio::test]
    async fn register_after_delete_does_not_resurrect_refused_bytes() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let session = meerkat_core::Session::new();
        let identity =
            register_save_delete(&store, &adapter, &session, "agent:park-guard-register").await;

        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect_err("the post-delete save must refuse");
        assert!(
            !parked_pending_bytes(&adapter, session.id()),
            "the refused save must not queue pre-registration bytes"
        );

        // Re-register at the durable record's real cursor (delete removed the
        // snapshot, not the continuity record).
        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .expect("resolve");
        let ContinuityResolveState::Ready { record } = resolved.get(&identity).expect("record")
        else {
            panic!("expected ready record");
        };
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: record.generation,
                    fencing_token: FencingToken::new(1),
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("re-registration after delete must not be bricked");
        assert!(
            meerkat::SessionStore::load(&adapter, session.id())
                .await
                .expect("load after re-register")
                .is_none(),
            "re-registration must not resurrect the deleted document"
        );

        // The id is not bricked: a registered save lands durably again.
        meerkat::SessionStore::save(&adapter, &session)
            .await
            .expect("a registered save after re-registration lands durably");
        assert!(
            meerkat::SessionStore::load(&adapter, session.id())
                .await
                .expect("load after registered save")
                .is_some(),
            "the re-registered save must be durable, not parked"
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_projection_cas_ignores_pending_bytes() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let mut session = meerkat_core::Session::new();
        session.set_metadata("projection", json!("first-pending"));
        meerkat::SessionStore::save_authoritative_projection_if_current_revision(
            &adapter, &session, None,
        )
        .await
        .expect("first missing-row CAS");

        session.set_metadata("projection", json!("latest-pending"));
        meerkat::SessionStore::save_authoritative_projection_if_current_revision(
            &adapter, &session, None,
        )
        .await
        .expect("pending bytes are not a visible durable revision");

        let identity = AgentIdentity::parse("agent:projection-pending").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:projection-pending:0")
                .expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(31);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("flush latest pending projection");
        let loaded = meerkat::SessionStore::load(&adapter, session.id())
            .await
            .expect("load")
            .expect("snapshot");
        assert_eq!(
            loaded.metadata().get("projection"),
            Some(&json!("latest-pending"))
        );
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_rejects_snapshot_with_foreign_embedded_id() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let requested = meerkat_core::Session::new();
        let foreign = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:foreign-snapshot-id").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:foreign-snapshot-id:0")
                .expect("runtime id"),
            session_id: requested.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(41);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        let bytes = serde_json::to_vec(&foreign).expect("serialize foreign session");
        store
            .save_session_snapshot(
                &identity,
                requested.id(),
                record.generation,
                CheckpointVersion::new(1),
                fencing_token,
                &SessionSnapshot { data: bytes },
            )
            .await
            .expect("seed corrupt keyed snapshot");

        let error = meerkat::SessionStore::load(&adapter, requested.id())
            .await
            .expect_err("embedded foreign session id must be explicit corruption");
        assert!(error.to_string().contains("contains session"));
    }

    #[tokio::test]
    async fn continuity_session_store_adapter_authoritative_projection_cas_guards_rewrites() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = ContinuitySessionStoreAdapter::new(store.clone());
        let mut session = meerkat_core::Session::new();
        let identity = AgentIdentity::parse("agent:projection").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:projection:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        let fencing_token = FencingToken::new(5);
        store
            .upsert_continuity_record(&record, fencing_token)
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: record.generation,
                    fencing_token,
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register");

        meerkat::SessionStore::save_authoritative_projection_if_current_revision(
            &adapter, &session, None,
        )
        .await
        .expect("initial projection should accept missing current revision");
        let original_revision =
            meerkat_core::session_store::session_projection_cas_token(&session).expect("revision");

        let mut stale_rewrite = session.clone();
        stale_rewrite.set_metadata("projection", json!("stale"));
        let stale_error = meerkat::SessionStore::save_authoritative_projection_if_current_revision(
            &adapter,
            &stale_rewrite,
            Some("row-sha256:not-current".to_string()),
        )
        .await
        .expect_err("stale CAS projection must reject");
        assert!(
            stale_error.to_string().contains("not a continuation"),
            "unexpected stale error: {stale_error}"
        );

        let loaded = meerkat::SessionStore::load(&adapter, session.id())
            .await
            .expect("load")
            .expect("snapshot");
        assert_eq!(
            meerkat_core::session_store::session_projection_cas_token(&loaded).expect("revision"),
            original_revision,
            "stale authoritative projection must leave stored row unchanged"
        );

        session.set_metadata("projection", json!("current"));
        meerkat::SessionStore::save_authoritative_projection_if_current_revision(
            &adapter,
            &session,
            Some(original_revision),
        )
        .await
        .expect("matching CAS projection should save");

        let loaded = meerkat::SessionStore::load(&adapter, session.id())
            .await
            .expect("load after save")
            .expect("snapshot after save");
        assert_eq!(loaded.metadata().get("projection"), Some(&json!("current")));
    }

    /// A cursor-recording delta channel over `meerkat_store::MemoryStore`:
    /// proves the wrapper presents the registered continuity cursor with
    /// every mutation without needing a SQLite substrate.
    struct RecordingIncrementalSessions {
        inner: Arc<meerkat_store::MemoryStore>,
        cursors: Mutex<Vec<super::super::contracts::ContinuityWriteCursor>>,
        /// Injected substrate failure: while set, every DURABLE mutation is
        /// refused before it touches `inner`. Drives the flush-failure path
        /// of `register_session`.
        refuse_writes: AtomicBool,
    }

    impl RecordingIncrementalSessions {
        fn refuse_writes(&self, refuse: bool) {
            self.refuse_writes.store(refuse, AtomicOrdering::SeqCst);
        }

        fn injected_failure(&self) -> Option<meerkat_core::SessionStoreError> {
            self.refuse_writes
                .load(AtomicOrdering::SeqCst)
                .then(|| meerkat_core::SessionStoreError::Internal("injected substrate".into()))
        }

        fn new(inner: Arc<meerkat_store::MemoryStore>) -> Self {
            Self {
                inner,
                refuse_writes: AtomicBool::new(false),
                cursors: Mutex::new(Vec::new()),
            }
        }

        fn record(&self, cursor: &super::super::contracts::ContinuityWriteCursor) {
            self.cursors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(cursor.clone());
        }

        fn recorded(&self) -> Vec<super::super::contracts::ContinuityWriteCursor> {
            self.cursors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait]
    impl super::super::contracts::ContinuityIncrementalSessions for RecordingIncrementalSessions {
        async fn append_messages(
            &self,
            cursor: &super::super::contracts::ContinuityWriteCursor,
            id: &meerkat_core::types::SessionId,
            strand: &meerkat_core::session_store::TranscriptStrandId,
            base_seq: u64,
            messages: &[meerkat_core::Message],
        ) -> Result<(), meerkat_core::SessionStoreError> {
            self.record(cursor);
            if let Some(failure) = self.injected_failure() {
                return Err(failure);
            }
            meerkat_core::session_store::IncrementalSessionStore::append_messages(
                self.inner.as_ref(),
                id,
                strand,
                base_seq,
                messages,
            )
            .await
        }

        async fn commit_rewrite(
            &self,
            cursor: &super::super::contracts::ContinuityWriteCursor,
            id: &meerkat_core::types::SessionId,
            record: &meerkat_core::TranscriptRewriteRecord,
            expected: meerkat_core::session_store::SessionHeadCas,
        ) -> Result<meerkat_core::session_store::SessionHead, meerkat_core::SessionStoreError>
        {
            self.record(cursor);
            if let Some(failure) = self.injected_failure() {
                return Err(failure);
            }
            meerkat_core::session_store::IncrementalSessionStore::commit_rewrite(
                self.inner.as_ref(),
                id,
                record,
                expected,
            )
            .await
        }

        async fn save_head(
            &self,
            cursor: &super::super::contracts::ContinuityWriteCursor,
            head: &meerkat_core::session_store::SessionHead,
            expected: meerkat_core::session_store::SessionHeadCas,
        ) -> Result<(), meerkat_core::SessionStoreError> {
            self.record(cursor);
            if let Some(failure) = self.injected_failure() {
                return Err(failure);
            }
            meerkat_core::session_store::IncrementalSessionStore::save_head(
                self.inner.as_ref(),
                head,
                expected,
            )
            .await
        }

        async fn load_head(
            &self,
            id: &meerkat_core::types::SessionId,
        ) -> Result<Option<meerkat_core::session_store::SessionHead>, meerkat_core::SessionStoreError>
        {
            meerkat_core::session_store::IncrementalSessionStore::load_head(self.inner.as_ref(), id)
                .await
        }

        async fn load_canonical_head(
            &self,
            id: &meerkat_core::types::SessionId,
        ) -> Result<Option<meerkat_core::session_store::SessionHead>, meerkat_core::SessionStoreError>
        {
            meerkat_core::session_store::IncrementalSessionStore::load_head(self.inner.as_ref(), id)
                .await
        }

        /// The double's substrate is a `MemoryStore` behind one `RwLock`, so
        /// `load` IS the single-snapshot head+rows materialization the
        /// contract asks for — never a head read composed with a separate
        /// rows read.
        async fn load_canonical_session(
            &self,
            id: &meerkat_core::types::SessionId,
        ) -> Result<Option<meerkat_core::Session>, meerkat_core::SessionStoreError> {
            if meerkat_core::session_store::IncrementalSessionStore::load_head(
                self.inner.as_ref(),
                id,
            )
            .await?
            .is_none()
            {
                return Ok(None);
            }
            meerkat::SessionStore::load(self.inner.as_ref(), id).await
        }

        async fn load_canonical_previous(
            &self,
            id: &meerkat_core::types::SessionId,
        ) -> Result<
            Option<(
                meerkat_core::Session,
                Vec<meerkat_core::TranscriptRewriteCommit>,
            )>,
            meerkat_core::SessionStoreError,
        > {
            let Some(session) = self.load_canonical_session(id).await? else {
                return Ok(None);
            };
            let adopted = meerkat_core::session_store::IncrementalSessionStore::load_rewrites(
                self.inner.as_ref(),
                id,
            )
            .await?
            .into_iter()
            .map(|record| record.commit)
            .collect();
            Ok(Some((session, adopted)))
        }

        async fn load_messages(
            &self,
            id: &meerkat_core::types::SessionId,
            strand: &meerkat_core::session_store::TranscriptStrandId,
            range: std::ops::Range<u64>,
        ) -> Result<Vec<meerkat_core::Message>, meerkat_core::SessionStoreError> {
            meerkat_core::session_store::IncrementalSessionStore::load_messages(
                self.inner.as_ref(),
                id,
                strand,
                range,
            )
            .await
        }

        async fn load_rewrites(
            &self,
            id: &meerkat_core::types::SessionId,
        ) -> Result<Vec<meerkat_core::TranscriptRewriteRecord>, meerkat_core::SessionStoreError>
        {
            meerkat_core::session_store::IncrementalSessionStore::load_rewrites(
                self.inner.as_ref(),
                id,
            )
            .await
        }
    }

    /// A continuity store advertising the M4b incremental capability: the
    /// whole-blob verbs delegate to an in-memory `LocalContinuityStore`,
    /// the delta channel to a cursor-recording `meerkat_store::MemoryStore`.
    struct IncrementalCapableStore {
        inner: Arc<LocalContinuityStore>,
        incremental: Arc<RecordingIncrementalSessions>,
    }

    impl IncrementalCapableStore {
        fn new() -> Self {
            Self {
                inner: Arc::new(LocalContinuityStore::in_memory().expect("store")),
                incremental: Arc::new(RecordingIncrementalSessions::new(Arc::new(
                    meerkat_store::MemoryStore::new(),
                ))),
            }
        }
    }

    #[async_trait]
    impl ContinuityStore for IncrementalCapableStore {
        // Explicit answer required by the trait: silence here used to
        // assert an authoritative "no record" while meaning "never
        // taught to answer".
        async fn resolve_record_by_session(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<
            Option<(
                crate::identity_first::ContinuityRecord,
                crate::identity_first::FencingToken,
                crate::identity_first::CheckpointVersion,
            )>,
            crate::identity_first::ContinuityStoreError,
        > {
            Ok(None)
        }

        async fn resolve_many(
            &self,
            identities: &[AgentIdentity],
        ) -> Result<
            std::collections::BTreeMap<AgentIdentity, ContinuityResolveState>,
            ContinuityStoreError,
        > {
            self.inner.resolve_many(identities).await
        }

        async fn load_session_snapshot(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<Option<SessionSnapshot>, ContinuityStoreError> {
            self.inner.load_session_snapshot(session_id).await
        }

        async fn save_session_snapshot(
            &self,
            identity: &AgentIdentity,
            session_id: &meerkat_core::types::SessionId,
            generation: ContinuityGeneration,
            version: CheckpointVersion,
            fencing_token: FencingToken,
            snapshot: &SessionSnapshot,
        ) -> Result<(), ContinuityStoreError> {
            self.inner
                .save_session_snapshot(
                    identity,
                    session_id,
                    generation,
                    version,
                    fencing_token,
                    snapshot,
                )
                .await
        }

        async fn upsert_continuity_record(
            &self,
            record: &ContinuityRecord,
            fencing_token: FencingToken,
        ) -> Result<(), ContinuityStoreError> {
            self.inner
                .upsert_continuity_record(record, fencing_token)
                .await
        }

        async fn delete_continuity_record(
            &self,
            identity: &AgentIdentity,
            fencing_token: FencingToken,
        ) -> Result<(), ContinuityStoreError> {
            self.inner
                .delete_continuity_record(identity, fencing_token)
                .await
        }

        fn as_incremental_sessions(
            &self,
        ) -> Option<Arc<dyn super::super::contracts::ContinuityIncrementalSessions>> {
            Some(self.incremental.clone())
        }
    }

    /// A continuity store that deliberately declines the delta channel (the
    /// `GatewayContinuityStore` shape: whole-snapshot verbs only).
    struct WholeSnapshotOnlyStore {
        inner: Arc<LocalContinuityStore>,
    }

    #[async_trait]
    impl ContinuityStore for WholeSnapshotOnlyStore {
        // The trait requires an explicit answer: a double that stayed
        // silent used to assert an authoritative "no record" while
        // meaning "I was never taught to answer", which is how a fix
        // goes inert with every test still green.
        async fn resolve_record_by_session(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<
            Option<(
                crate::identity_first::ContinuityRecord,
                crate::identity_first::FencingToken,
                crate::identity_first::CheckpointVersion,
            )>,
            crate::identity_first::ContinuityStoreError,
        > {
            Ok(None)
        }

        async fn resolve_many(
            &self,
            identities: &[AgentIdentity],
        ) -> Result<
            std::collections::BTreeMap<AgentIdentity, ContinuityResolveState>,
            ContinuityStoreError,
        > {
            self.inner.resolve_many(identities).await
        }

        async fn load_session_snapshot(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<Option<SessionSnapshot>, ContinuityStoreError> {
            self.inner.load_session_snapshot(session_id).await
        }

        async fn save_session_snapshot(
            &self,
            identity: &AgentIdentity,
            session_id: &meerkat_core::types::SessionId,
            generation: ContinuityGeneration,
            version: CheckpointVersion,
            fencing_token: FencingToken,
            snapshot: &SessionSnapshot,
        ) -> Result<(), ContinuityStoreError> {
            self.inner
                .save_session_snapshot(
                    identity,
                    session_id,
                    generation,
                    version,
                    fencing_token,
                    snapshot,
                )
                .await
        }

        async fn upsert_continuity_record(
            &self,
            record: &ContinuityRecord,
            fencing_token: FencingToken,
        ) -> Result<(), ContinuityStoreError> {
            self.inner
                .upsert_continuity_record(record, fencing_token)
                .await
        }

        async fn delete_continuity_record(
            &self,
            identity: &AgentIdentity,
            fencing_token: FencingToken,
        ) -> Result<(), ContinuityStoreError> {
            self.inner
                .delete_continuity_record(identity, fencing_token)
                .await
        }
        /// Test double: deliberately whole-snapshot only.
        fn as_incremental_sessions(
            &self,
        ) -> Option<
            std::sync::Arc<dyn crate::identity_first::contracts::ContinuityIncrementalSessions>,
        > {
            None
        }
    }

    /// M4b: the adapter genuinely forwards `as_incremental` — `Some` when the
    /// substrate advertises the channel (the bundled `LocalContinuityStore`
    /// now does), `None` when it declines (the wire-verb gateway store).
    #[tokio::test]
    async fn adapter_forwards_incremental_capability_only_when_substrate_advertises() {
        let bundled = Arc::new(ContinuitySessionStoreAdapter::new(Arc::new(
            LocalContinuityStore::in_memory().expect("store"),
        )));
        assert!(
            meerkat::SessionStore::as_incremental(bundled).is_some(),
            "the bundled LocalContinuityStore ships the delta channel (M4b)"
        );

        let declining = Arc::new(ContinuitySessionStoreAdapter::new(Arc::new(
            WholeSnapshotOnlyStore {
                inner: Arc::new(LocalContinuityStore::in_memory().expect("store")),
            },
        )));
        assert!(
            meerkat::SessionStore::as_incremental(declining).is_none(),
            "a whole-snapshot-only substrate must not surface a delta channel"
        );

        let capable = Arc::new(ContinuitySessionStoreAdapter::new(Arc::new(
            IncrementalCapableStore::new(),
        )));
        assert!(
            meerkat::SessionStore::as_incremental(capable).is_some(),
            "an advertising substrate must surface through the adapter"
        );
    }

    /// meerkat 0.8.22 store-wide activation: this wrapper must REFUSE, never
    /// answer a census it did not take. The rationale otherwise lives only in
    /// a doc comment, and a doc comment is not a guard - this pins it as
    /// behavior so a later "just return `NotApplicable`" edit fails here.
    ///
    /// `NotApplicable` is the specific wrong answer being fenced out: it
    /// asserts "this store's durable profile is not HeadCanonical" while the
    /// same impl forwards genuine per-session crossings, and meerkat's
    /// activation caller converts it into an operator profile mismatch,
    /// pointing a reader at configuration instead of at the missing census
    /// seam.
    #[tokio::test]
    async fn store_wide_head_canonical_activation_is_refused_not_faked() {
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(Arc::new(
            LocalContinuityStore::in_memory().expect("store"),
        )));
        let incremental = meerkat::SessionStore::as_incremental(adapter)
            .expect("the bundled LocalContinuityStore ships the delta channel");

        let error = incremental
            .activate_head_canonical_store()
            .await
            .expect_err("the continuity substrate exposes no one-snapshot physical census");
        assert!(
            matches!(error, meerkat_store::SessionStoreError::Internal(_)),
            "store-wide activation must refuse, never report a census it did not take: {error}"
        );
    }

    /// M4b: incremental mutations preserve the adapter's lifecycle
    /// discipline — parked (never durable) before registration, flushed
    /// under the real cursor at registration, refused while suspended.
    #[tokio::test]
    async fn incremental_mutations_park_before_registration_and_flush_on_register() {
        let store = Arc::new(IncrementalCapableStore::new());
        let channel = store.incremental.clone();
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(
            store.clone() as Arc<dyn ContinuityStore>
        ));
        let incremental = meerkat::SessionStore::as_incremental(adapter.clone())
            .expect("advertising substrate forwards");

        let session = meerkat_core::Session::new();
        let root = meerkat_core::session_store::TranscriptStrandId::root();
        let message =
            meerkat_core::Message::User(meerkat_core::UserMessage::text("delta turn".to_string()));
        let mut document = session.clone();
        document.push(message.clone());
        let head =
            meerkat_core::session_store::SessionHead::from_session(&document, root.clone(), 0)
                .expect("head from session");

        // Pre-registration (the member-creation window): parked, not refused.
        incremental
            .append_messages(session.id(), &root, 0, std::slice::from_ref(&message))
            .await
            .expect("pre-registration delta writes must park, not fail");
        incremental
            .save_head(&head, meerkat_core::session_store::SessionHeadCas::Create)
            .await
            .expect("pre-registration head writes must park, not fail");
        assert!(
            channel.recorded().is_empty(),
            "parked writes must reach nothing durable"
        );
        // The parked view answers the service's continuity preflight.
        let parked_head = incremental
            .load_head(session.id())
            .await
            .expect("parked load_head")
            .expect("parked head is visible to the preflight");
        assert_eq!(parked_head.head_revision, head.head_revision);
        assert_eq!(
            incremental
                .load_messages(session.id(), &root, 0..1)
                .await
                .expect("parked rows")
                .len(),
            1
        );

        let identity = AgentIdentity::parse("agent:incremental").expect("identity");
        let record = ContinuityRecord {
            identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::parse("rt:agent:incremental:0").expect("runtime id"),
            session_id: session.id().clone(),
            generation: ContinuityGeneration::new(0),
            checkpoint_version: CheckpointVersion::new(0),
        };
        store
            .upsert_continuity_record(&record, FencingToken::new(1))
            .await
            .expect("seed record");
        let registered_version = adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: record.generation,
                    fencing_token: FencingToken::new(1),
                    checkpoint_version: record.checkpoint_version,
                },
            )
            .await
            .expect("register flushes the parked deltas");

        let cursors = channel.recorded();
        assert_eq!(
            cursors.len(),
            2,
            "the flush replays one append + one head save under the real cursor"
        );
        assert!(
            cursors.iter().all(|cursor| cursor.identity == identity
                && cursor.fencing_token == FencingToken::new(1)),
            "every flushed mutation carries the registered continuity cursor"
        );
        // Exact accounting, not just monotonicity: under M4b the initial
        // document is TWO delta mutations, each minting one version from the
        // session's single allocator — append = 1, head adoption = 2 — so
        // registration reports the creation-window document at checkpoint
        // version exactly 2. This is the writer behind the identity status
        // (and the Python e2e reset bound in test_reset_advances_generation)
        // reading 2 right after a member is materialized: it is the
        // initial-document flush, not reset's retire cleanup, which only
        // no-ops against the superseded OLD session.
        assert_eq!(
            (
                cursors[0].checkpoint_version.get(),
                cursors[1].checkpoint_version.get()
            ),
            (1, 2),
            "the creation-window flush mints append at 1 and head adoption at 2"
        );
        assert_eq!(
            registered_version.get(),
            2,
            "registration must report the flushed document's committed version"
        );
        let durable_rows = incremental
            .load_messages(session.id(), &root, 0..1)
            .await
            .expect("read back after flush");
        assert_eq!(
            durable_rows.len(),
            1,
            "the flushed delta row must be durable in the channel"
        );

        // Registered writes go straight through with a fresh cursor.
        let second = meerkat_core::Message::User(meerkat_core::UserMessage::text(
            "second delta turn".to_string(),
        ));
        incremental
            .append_messages(session.id(), &root, 1, std::slice::from_ref(&second))
            .await
            .expect("registered delta writes delegate to the substrate channel");
        assert_eq!(channel.recorded().len(), 3);

        adapter
            .suspend_session(session.id())
            .await
            .expect("suspend");
        let suspended = incremental
            .append_messages(session.id(), &root, 2, std::slice::from_ref(&message))
            .await
            .expect_err("suspended sessions must refuse delta writes");
        assert!(
            suspended.to_string().contains("suspended"),
            "the refusal must name the suspension: {suspended}"
        );
    }

    /// Seed a continuity record so `register_session` has a real cursor to
    /// flush parked state under.
    async fn seed_incremental_record(
        store: &IncrementalCapableStore,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
    ) -> SessionRuntimeState {
        store
            .upsert_continuity_record(
                &ContinuityRecord {
                    identity: identity.clone(),
                    agent_runtime_id: AgentRuntimeId::parse(&format!("rt:{identity}:0"))
                        .expect("runtime id"),
                    session_id: session_id.clone(),
                    generation: ContinuityGeneration::new(0),
                    checkpoint_version: CheckpointVersion::new(0),
                },
                FencingToken::new(1),
            )
            .await
            .expect("seed record");
        SessionRuntimeState {
            identity: identity.clone(),
            generation: ContinuityGeneration::new(0),
            fencing_token: FencingToken::new(1),
            checkpoint_version: CheckpointVersion::new(0),
        }
    }

    /// DATA-LOSS PIN: the service appends rows and adopts them with a head
    /// under two separate locks, so a registration can land between the two.
    /// The parked rows are NOT an adoptable document at that instant —
    /// and dropping them is the one outcome the parking layer must never
    /// produce. Registration is refused typed, rows and routing marker are
    /// retained, the registry is restored, and the retry (after the head
    /// write lands) adopts the whole document.
    #[tokio::test]
    async fn registration_never_purges_parked_rows_that_no_head_adopts_yet() {
        let store = Arc::new(IncrementalCapableStore::new());
        let channel = store.incremental.clone();
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(
            store.clone() as Arc<dyn ContinuityStore>
        ));
        let incremental = meerkat::SessionStore::as_incremental(adapter.clone())
            .expect("advertising substrate forwards");

        let session = meerkat_core::Session::new();
        let root = meerkat_core::session_store::TranscriptStrandId::root();
        let message = meerkat_core::Message::User(meerkat_core::UserMessage::text(
            "creation-window turn".to_string(),
        ));
        let mut document = session.clone();
        document.push(message.clone());
        let head =
            meerkat_core::session_store::SessionHead::from_session(&document, root.clone(), 0)
                .expect("head from session");

        // The creation-window append parks. The adopting head has NOT been
        // written yet — this is the interleaving window.
        incremental
            .append_messages(session.id(), &root, 0, std::slice::from_ref(&message))
            .await
            .expect("pre-registration append must park");

        let identity = AgentIdentity::parse("agent:racing").expect("identity");
        let state = seed_incremental_record(&store, &identity, session.id()).await;
        let refused = adapter
            .register_session(session.id(), state.clone())
            .await
            .expect_err("an unadoptable parked state must refuse the registration");
        assert!(
            refused.to_string().contains("no parked head adopts"),
            "the refusal must name the unadoptable parked state: {refused}"
        );

        // Nothing dropped: rows and routing marker survive, registry restored.
        assert!(
            adapter.parked_deltas.is_parked(session.id()),
            "the routing marker must survive a refused registration"
        );
        assert_eq!(
            adapter
                .parked_deltas
                .footprint(session.id())
                .expect("footprint")
                .rows,
            1,
            "the parked footprint must still account for the parked row"
        );
        assert!(
            adapter.lookup_session(&session.id().to_string()).is_none(),
            "a refused registration must restore the registry entry"
        );
        assert!(
            channel.recorded().is_empty(),
            "an unadoptable parked state must reach nothing durable"
        );

        // The still-parked head lands, completing the parked document. This
        // is also where a purge would be caught red-handed: adopting a
        // 1-message head over the 0 rows a purge would have left behind is
        // exactly what `validate_save_head_transition` refuses.
        incremental
            .save_head(&head, meerkat_core::session_store::SessionHeadCas::Create)
            .await
            .expect("the retained rows must still be there for the head to adopt");
        assert_eq!(
            incremental
                .load_messages(session.id(), &root, 0..1)
                .await
                .expect("parked rows")
                .len(),
            1,
            "the row parked before the refused registration must be the row the head adopts"
        );

        // ...and the retry adopts everything.
        let committed = adapter
            .register_session(session.id(), state)
            .await
            .expect("the retry must flush the now-adoptable parked document");
        assert!(committed.get() > 0, "the flush must commit a version");
        assert!(
            !adapter.parked_deltas.is_parked(session.id()),
            "an adopted parked state is purged"
        );
        assert_eq!(
            incremental
                .load_messages(session.id(), &root, 0..1)
                .await
                .expect("durable rows")
                .len(),
            1,
            "the retained row must reach the durable channel on the retry"
        );
    }

    /// MANDATORY PIN (flush-failure-restores-registry): when the parked
    /// flush FAILS at the substrate, registration is refused, the registry
    /// entry and lifecycle markers are restored to their pre-registration
    /// values, the parked rows survive, and a retry after the substrate
    /// recovers adopts them.
    #[tokio::test]
    async fn failed_parked_flush_restores_registry_and_markers_and_retries_clean() {
        let store = Arc::new(IncrementalCapableStore::new());
        let channel = store.incremental.clone();
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(
            store.clone() as Arc<dyn ContinuityStore>
        ));
        let incremental = meerkat::SessionStore::as_incremental(adapter.clone())
            .expect("advertising substrate forwards");

        let session = meerkat_core::Session::new();
        let root = meerkat_core::session_store::TranscriptStrandId::root();
        let message = meerkat_core::Message::User(meerkat_core::UserMessage::text(
            "flush failure turn".to_string(),
        ));
        let mut document = session.clone();
        document.push(message.clone());
        let head =
            meerkat_core::session_store::SessionHead::from_session(&document, root.clone(), 0)
                .expect("head from session");

        // A COMPLETE parked document: rows plus the head that adopts them.
        incremental
            .append_messages(session.id(), &root, 0, std::slice::from_ref(&message))
            .await
            .expect("pre-registration append must park");
        incremental
            .save_head(&head, meerkat_core::session_store::SessionHeadCas::Create)
            .await
            .expect("pre-registration head must park");

        // Suspend first, so the pin covers marker restoration too, not just
        // the registry entry.
        adapter
            .suspend_session(session.id())
            .await
            .expect("suspend");

        let identity = AgentIdentity::parse("agent:flushfail").expect("identity");
        let state = seed_incremental_record(&store, &identity, session.id()).await;
        channel.refuse_writes(true);
        let refused = adapter
            .register_session(session.id(), state.clone())
            .await
            .expect_err("a failing parked flush must refuse the registration");
        assert!(
            refused.to_string().contains("injected substrate"),
            "the substrate failure must ride out typed: {refused}"
        );

        // Registry restored.
        assert!(
            adapter.lookup_session(&session.id().to_string()).is_none(),
            "a failed flush must restore the registry entry"
        );
        // Markers restored.
        let still_suspended = adapter
            .ensure_session_mutation_allowed(session.id())
            .expect_err("the suspension marker must be restored");
        assert!(
            still_suspended.to_string().contains("suspended"),
            "the restored marker must be the suspension: {still_suspended}"
        );
        // Parked rows retained, nothing durable.
        assert!(
            adapter.parked_deltas.is_parked(session.id()),
            "a failed flush must retain the routing marker"
        );
        assert_eq!(
            incremental
                .load_messages(session.id(), &root, 0..1)
                .await
                .expect("parked rows")
                .len(),
            1,
            "a failed flush must retain the parked rows for the retry"
        );
        assert!(
            meerkat_core::session_store::IncrementalSessionStore::load_head(
                channel.inner.as_ref(),
                session.id()
            )
            .await
            .expect("durable head")
            .is_none(),
            "a failed flush must leave the durable channel empty"
        );

        // Retry once the substrate recovers.
        channel.refuse_writes(false);
        adapter
            .register_session(session.id(), state)
            .await
            .expect("the retry must flush the retained parked document");
        assert!(
            !adapter.parked_deltas.is_parked(session.id()),
            "an adopted parked state is purged"
        );
        assert_eq!(
            incremental
                .load_messages(session.id(), &root, 0..1)
                .await
                .expect("durable rows")
                .len(),
            1,
            "the retained row must reach the durable channel on the retry"
        );
        assert_eq!(
            meerkat_core::session_store::IncrementalSessionStore::load_head(
                channel.inner.as_ref(),
                session.id()
            )
            .await
            .expect("durable head")
            .expect("durable head after retry")
            .head_revision,
            head.head_revision,
            "the retry must adopt the parked head"
        );
    }

    /// A transcript rewrite on a HEAD-CANONICAL session must be recorded as a
    /// commit + adoption, not flattened into a whole-document rebase (which
    /// would silently drop the rewrite record and the retained bodies).
    #[tokio::test]
    async fn transcript_rewrite_on_a_head_canonical_session_records_the_commit() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(
            store.clone() as Arc<dyn ContinuityStore>
        ));
        let incremental = meerkat::SessionStore::as_incremental(adapter.clone())
            .expect("the bundled store advertises the delta channel");

        let mut session = meerkat_core::Session::new();
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("one".to_string()),
        ));
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("two".to_string()),
        ));
        let identity = AgentIdentity::parse("agent:rewrite").expect("identity");
        store
            .upsert_continuity_record(
                &ContinuityRecord {
                    identity: identity.clone(),
                    agent_runtime_id: AgentRuntimeId::parse("rt:agent:rewrite:0")
                        .expect("runtime id"),
                    session_id: session.id().clone(),
                    generation: ContinuityGeneration::new(0),
                    checkpoint_version: CheckpointVersion::new(0),
                },
                FencingToken::new(1),
            )
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: ContinuityGeneration::new(0),
                    fencing_token: FencingToken::new(1),
                    checkpoint_version: CheckpointVersion::new(0),
                },
            )
            .await
            .expect("register");

        let root = meerkat_core::session_store::TranscriptStrandId::root();
        incremental
            .append_messages(session.id(), &root, 0, session.messages())
            .await
            .expect("seed rows");
        let head =
            meerkat_core::session_store::SessionHead::from_session(&session, root.clone(), 0)
                .expect("head");
        incremental
            .save_head(&head, meerkat_core::session_store::SessionHeadCas::Create)
            .await
            .expect("adopt head");

        let mut rewritten = session.clone();
        let commit = rewritten
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                vec![meerkat_core::Message::User(
                    meerkat_core::UserMessage::text("[summary]".to_string()),
                )],
                meerkat_core::TranscriptRewriteReason::new("compaction"),
                Some("adapter-test".to_string()),
                None,
            )
            .expect("commit rewrite");

        meerkat::SessionStore::save_transcript_rewrite(adapter.as_ref(), &rewritten, &commit)
            .await
            .expect("head-canonical rewrite must be admitted");

        let records = incremental
            .load_rewrites(session.id())
            .await
            .expect("load rewrites");
        assert_eq!(
            records.len(),
            1,
            "the rewrite must be recorded and adopted, not flattened into a rebase"
        );
        assert_eq!(records[0].commit.revision, commit.revision);
        let loaded = meerkat::SessionStore::load(adapter.as_ref(), session.id())
            .await
            .expect("load")
            .expect("session");
        assert_eq!(
            loaded.messages(),
            rewritten.messages(),
            "the adopted head must serve the rewritten transcript"
        );
    }

    /// A superseded session must never PARK a delta write — parking a write
    /// for an abandoned generation would resurrect it at the next
    /// registration. That is the hazard; a typed refusal was merely one way
    /// to avoid it, and the wrong one.
    ///
    /// Reset's retire path abandons the superseded session and THEN retires
    /// the physical member, and Meerkat's retire/archive issues one more
    /// save. Refusing it aborts identity-authority release, so the gateway
    /// retains provider grants and reports runtime_cleanup_completed=false
    /// (reproduced by test_real_gateway_reset_reprofile_materializes_shell_tools).
    /// Every sibling verb on this adapter — `save`, `save_transcript`,
    /// `save_transcript_rewrite`, the CAS save, and `load_head` — already
    /// acknowledges a post-abandon terminal write as a no-op, so the
    /// incremental verbs agree.
    ///
    /// This pins BOTH halves: the terminal projection is acknowledged and
    /// reaches nothing (durable or parked), while a rewrite COMMIT, which
    /// carries new authority and must never be silently dropped, stays
    /// refused typed.
    #[tokio::test]
    async fn superseded_sessions_drop_terminal_writes_without_parking_them() {
        let store = Arc::new(IncrementalCapableStore::new());
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(
            store.clone() as Arc<dyn ContinuityStore>
        ));
        let incremental = meerkat::SessionStore::as_incremental(adapter.clone())
            .expect("advertising substrate forwards");
        let session = meerkat_core::Session::new();
        adapter
            .superseded_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session.id().to_string());

        let root = meerkat_core::session_store::TranscriptStrandId::root();
        let message =
            meerkat_core::Message::User(meerkat_core::UserMessage::text("orphan".to_string()));
        incremental
            .append_messages(session.id(), &root, 0, std::slice::from_ref(&message))
            .await
            .expect("a post-abandon terminal write must be acknowledged, not refused");

        // The actual hazard: nothing may be parked, or the next registration
        // would resurrect the abandoned generation.
        assert!(
            meerkat_core::session_store::IncrementalSessionStore::load_head(
                adapter.parked_deltas.reads(),
                session.id(),
            )
            .await
            .expect("parked head probe")
            .is_none(),
            "a superseded terminal write must not park anything"
        );

        // ... and nothing may have reached the substrate either.
        assert!(
            meerkat_core::session_store::IncrementalSessionStore::load_head(
                incremental.as_ref(),
                session.id(),
            )
            .await
            .expect("substrate head probe")
            .is_none(),
            "a superseded terminal write must not reach the substrate"
        );
    }

    /// The OTHER half of the superseded-write contract, driven directly: a
    /// rewrite COMMIT against a superseded session takes the typed REFUSAL
    /// branch (`commit_rewrite`'s `DeltaRoute::SupersededNoOp` arm) — it is
    /// not acknowledged, not parked, and writes nothing.
    ///
    /// The sibling test pins the no-op half (append/save_head are terminal
    /// projections and are acknowledged). A rewrite commit is NOT terminal:
    /// it carries new authority. Acknowledging it would fabricate a durable
    /// commit that never happened; parking it would resurrect the abandoned
    /// generation at the next registration. Refusal must stay typed.
    ///
    /// The fixture reproduces the real post-reset shape rather than a bare
    /// marker: a registered session commits a durable head-canonical
    /// document, then `abandon_superseded_session` (reset's own verb)
    /// deletes it under exact CAS and publishes the superseded tombstone.
    /// The rewrite commit arrives afterwards — a compaction racing a reset.
    #[tokio::test]
    async fn superseded_sessions_refuse_rewrite_commits_and_write_nothing() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let raw_channel = store
            .as_incremental_sessions()
            .expect("the bundled store advertises the delta channel");
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(
            store.clone() as Arc<dyn ContinuityStore>
        ));
        let incremental = meerkat::SessionStore::as_incremental(adapter.clone())
            .expect("adapter forwards the delta channel");

        // A registered session with a durable head-canonical document.
        let mut session = meerkat_core::Session::new();
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("one".to_string()),
        ));
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("two".to_string()),
        ));
        let identity = AgentIdentity::parse("agent:superseded-rewrite").expect("identity");
        store
            .upsert_continuity_record(
                &ContinuityRecord {
                    identity: identity.clone(),
                    agent_runtime_id: AgentRuntimeId::parse("rt:agent:superseded-rewrite:0")
                        .expect("runtime id"),
                    session_id: session.id().clone(),
                    generation: ContinuityGeneration::new(0),
                    checkpoint_version: CheckpointVersion::new(0),
                },
                FencingToken::new(1),
            )
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: ContinuityGeneration::new(0),
                    fencing_token: FencingToken::new(1),
                    checkpoint_version: CheckpointVersion::new(0),
                },
            )
            .await
            .expect("register");
        let root = meerkat_core::session_store::TranscriptStrandId::root();
        incremental
            .append_messages(session.id(), &root, 0, session.messages())
            .await
            .expect("seed rows");
        let head =
            meerkat_core::session_store::SessionHead::from_session(&session, root.clone(), 0)
                .expect("head");
        incremental
            .save_head(&head, meerkat_core::session_store::SessionHeadCas::Create)
            .await
            .expect("adopt head");
        let token = meerkat_core::session_store::session_head_cas_token(&head).expect("head token");

        // The rewrite a late caller will try to commit.
        let mut rewritten = session.clone();
        let commit = rewritten
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                vec![meerkat_core::Message::User(
                    meerkat_core::UserMessage::text("[summary]".to_string()),
                )],
                meerkat_core::TranscriptRewriteReason::new("compaction"),
                Some("superseded-rewrite-test".to_string()),
                None,
            )
            .expect("commit rewrite");
        let record =
            rewrite_record_from_session_bodies(&rewritten, &commit).expect("rewrite record");

        // Reset commits the replacement generation and abandons this session.
        adapter
            .abandon_superseded_session(session.id())
            .await
            .expect("abandon superseded session");
        assert!(adapter.session_was_superseded(session.id()));
        assert!(
            raw_channel
                .load_canonical_head(session.id())
                .await
                .expect("post-abandon head probe")
                .is_none(),
            "abandon must have deleted the durable document under exact CAS"
        );

        // The late rewrite commit takes the documented refusal branch.
        let refused = incremental
            .commit_rewrite(
                session.id(),
                &record,
                meerkat_core::session_store::SessionHeadCas::IfToken(token),
            )
            .await
            .expect_err("a rewrite commit against a superseded session must be refused");
        assert!(
            refused.to_string().contains("rewrite commits are refused"),
            "the refusal must be the superseded rewrite branch, not a downstream \
             CAS/park failure: {refused}"
        );

        // ...and it must not have written: nothing parked (the resurrection
        // vector the parking layer exists to prevent)...
        assert!(
            !adapter.parked_deltas.is_parked(session.id()),
            "a refused rewrite commit must not park anything"
        );
        // ...and nothing durable — no head resurrected, no rewrite record
        // materialized.
        assert!(
            raw_channel
                .load_canonical_head(session.id())
                .await
                .expect("post-refusal head probe")
                .is_none(),
            "a refused rewrite commit must not resurrect the durable head"
        );
        assert!(
            raw_channel
                .load_rewrites(session.id())
                .await
                .expect("post-refusal rewrite probe")
                .is_empty(),
            "a refused rewrite commit must not record the rewrite durably"
        );
    }

    /// UNVERIFIED-PURGE PIN (N2): `ParkedFlush::Empty` is the one purge that
    /// is NOT preceded by a durable adoption — it drops parked state on the
    /// strength of the footprint alone. Nothing reached it, so the footprint
    /// it trusts was never exercised at the decision point.
    ///
    /// The reachable shape is the service seeding a fresh session with an
    /// empty append: a routing marker with a zero-row footprint and no
    /// parked head. This pins the whole arm — that it is REACHED (marker
    /// present, footprint zero, no parked head), that it clears the marker
    /// rather than leaving the session stranded in the parked view forever,
    /// that it reaches nothing durable, and that the session is fully usable
    /// afterwards.
    #[tokio::test]
    async fn registration_over_an_empty_parked_marker_clears_it_and_keeps_the_session_usable() {
        let store = Arc::new(IncrementalCapableStore::new());
        let channel = store.incremental.clone();
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(
            store.clone() as Arc<dyn ContinuityStore>
        ));
        let incremental = meerkat::SessionStore::as_incremental(adapter.clone())
            .expect("advertising substrate forwards");

        let session = meerkat_core::Session::new();
        let root = meerkat_core::session_store::TranscriptStrandId::root();

        // How the service seeds a fresh session before its identity is
        // published: an append that carries nothing.
        incremental
            .append_messages(session.id(), &root, 0, &[])
            .await
            .expect("an empty pre-registration append must park, not fail");

        // Pin that this is really the Empty arm's pre-state and not one of
        // the other two.
        assert!(
            adapter.parked_deltas.is_parked(session.id()),
            "an empty append still publishes the routing marker"
        );
        assert_eq!(
            adapter
                .parked_deltas
                .footprint(session.id())
                .expect("footprint")
                .rows,
            0,
            "an empty append parks zero rows"
        );
        assert!(
            meerkat_core::session_store::IncrementalSessionStore::load_head(
                adapter.parked_deltas.reads(),
                session.id(),
            )
            .await
            .expect("parked head probe")
            .is_none(),
            "the Empty arm is only reachable with no parked head"
        );

        let identity = AgentIdentity::parse("agent:empty-marker").expect("identity");
        let state = seed_incremental_record(&store, &identity, session.id()).await;
        adapter
            .register_session(session.id(), state)
            .await
            .expect("an empty parked marker must not refuse the registration");

        assert!(
            !adapter.parked_deltas.is_parked(session.id()),
            "the empty marker must be cleared, or every later read for this session \
             is served from a parked view that will never hold anything"
        );
        assert!(
            channel.recorded().is_empty(),
            "an empty parked marker replays nothing durable"
        );

        // And the session works: writes now route straight to the substrate.
        let message = meerkat_core::Message::User(meerkat_core::UserMessage::text(
            "first real turn".to_string(),
        ));
        let mut document = session.clone();
        document.push(message.clone());
        let head =
            meerkat_core::session_store::SessionHead::from_session(&document, root.clone(), 0)
                .expect("head from session");
        incremental
            .append_messages(session.id(), &root, 0, std::slice::from_ref(&message))
            .await
            .expect("registered append");
        incremental
            .save_head(&head, meerkat_core::session_store::SessionHeadCas::Create)
            .await
            .expect("registered head save");
        assert_eq!(
            channel.recorded().len(),
            2,
            "post-registration writes must reach the durable channel"
        );
    }

    /// LEAK PIN (N3): `forget_session` used to clear only the routing
    /// marker, leaving the rows to "the async purge at each call site" —
    /// which four of the six call sites never performed. Every session
    /// deleted while its writes were parked leaked its whole transcript into
    /// the process's parked store for the lifetime of the process.
    ///
    /// Delete is the leaking path with the widest blast radius, because it
    /// is also the path that reports success.
    #[tokio::test]
    async fn deleting_a_parked_session_reclaims_its_parked_rows() {
        let store = Arc::new(IncrementalCapableStore::new());
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(
            store.clone() as Arc<dyn ContinuityStore>
        ));
        let incremental = meerkat::SessionStore::as_incremental(adapter.clone())
            .expect("advertising substrate forwards");

        let session = meerkat_core::Session::new();
        let root = meerkat_core::session_store::TranscriptStrandId::root();
        let message = meerkat_core::Message::User(meerkat_core::UserMessage::text(
            "parked transcript".to_string(),
        ));
        let mut document = session.clone();
        document.push(message.clone());
        let head =
            meerkat_core::session_store::SessionHead::from_session(&document, root.clone(), 0)
                .expect("head from session");

        incremental
            .append_messages(session.id(), &root, 0, std::slice::from_ref(&message))
            .await
            .expect("pre-registration append parks");
        incremental
            .save_head(&head, meerkat_core::session_store::SessionHeadCas::Create)
            .await
            .expect("pre-registration head parks");
        assert!(
            meerkat_core::session_store::IncrementalSessionStore::load_head(
                adapter.parked_deltas.reads(),
                session.id(),
            )
            .await
            .expect("parked head probe")
            .is_some(),
            "the parked document must exist before the delete"
        );

        // The delete path for a session with no durable document: it forgets
        // the session and reports success.
        meerkat::SessionStore::delete(adapter.as_ref(), session.id())
            .await
            .expect("deleting a session with no durable document succeeds");

        assert!(
            !adapter.parked_deltas.is_parked(session.id()),
            "delete must clear the routing marker"
        );
        assert!(
            adapter.parked_deltas.footprint(session.id()).is_none(),
            "delete must clear the footprint"
        );
        assert!(
            meerkat_core::session_store::IncrementalSessionStore::load_head(
                adapter.parked_deltas.reads(),
                session.id(),
            )
            .await
            .expect("parked head probe")
            .is_none(),
            "delete must reclaim the parked ROWS, not merely stop routing to them — \
             an unmarked-but-retained parked document is unreachable memory held \
             for the lifetime of the process"
        );
    }

    /// TORN-READ PIN (N4): the head-first load replaced the whole-snapshot
    /// verb's ONE transactional substrate read with a `load_canonical_head`
    /// followed by an independent `load_messages`. Two reads, two snapshots:
    /// a concurrent delta write landing between them yields a head that
    /// describes a transcript the rows no longer are.
    ///
    /// The substrate double below makes that interleaving deterministic —
    /// its composed halves disagree by construction, while its
    /// single-snapshot `load_canonical_session` is self-consistent. A loader
    /// that still composes the halves surfaces the torn pair; one that takes
    /// the single snapshot cannot.
    #[tokio::test]
    async fn head_canonical_load_takes_one_substrate_snapshot() {
        struct TearingSubstrate {
            /// What a single-snapshot read observes: the settled document.
            settled: meerkat_core::Session,
            /// What the head half of a composed read observes: a head from
            /// BEFORE a concurrent rewrite shortened the strand.
            stale_head: meerkat_core::session_store::SessionHead,
        }

        #[async_trait]
        impl super::super::contracts::ContinuityIncrementalSessions for TearingSubstrate {
            async fn append_messages(
                &self,
                _cursor: &super::super::contracts::ContinuityWriteCursor,
                _id: &meerkat_core::types::SessionId,
                _strand: &meerkat_core::session_store::TranscriptStrandId,
                _base_seq: u64,
                _messages: &[meerkat_core::Message],
            ) -> Result<(), meerkat_core::SessionStoreError> {
                Ok(())
            }

            async fn commit_rewrite(
                &self,
                _cursor: &super::super::contracts::ContinuityWriteCursor,
                _id: &meerkat_core::types::SessionId,
                _record: &meerkat_core::TranscriptRewriteRecord,
                _expected: meerkat_core::session_store::SessionHeadCas,
            ) -> Result<meerkat_core::session_store::SessionHead, meerkat_core::SessionStoreError>
            {
                Ok(self.stale_head.clone())
            }

            async fn save_head(
                &self,
                _cursor: &super::super::contracts::ContinuityWriteCursor,
                _head: &meerkat_core::session_store::SessionHead,
                _expected: meerkat_core::session_store::SessionHeadCas,
            ) -> Result<(), meerkat_core::SessionStoreError> {
                Ok(())
            }

            async fn load_head(
                &self,
                _id: &meerkat_core::types::SessionId,
            ) -> Result<
                Option<meerkat_core::session_store::SessionHead>,
                meerkat_core::SessionStoreError,
            > {
                Ok(Some(self.stale_head.clone()))
            }

            /// Snapshot A — taken before the concurrent write.
            async fn load_canonical_head(
                &self,
                _id: &meerkat_core::types::SessionId,
            ) -> Result<
                Option<meerkat_core::session_store::SessionHead>,
                meerkat_core::SessionStoreError,
            > {
                Ok(Some(self.stale_head.clone()))
            }

            /// ONE snapshot over head and rows: internally consistent, always.
            async fn load_canonical_session(
                &self,
                _id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_core::Session>, meerkat_core::SessionStoreError>
            {
                Ok(Some(self.settled.clone()))
            }

            async fn load_canonical_previous(
                &self,
                _id: &meerkat_core::types::SessionId,
            ) -> Result<
                Option<(
                    meerkat_core::Session,
                    Vec<meerkat_core::TranscriptRewriteCommit>,
                )>,
                meerkat_core::SessionStoreError,
            > {
                Ok(Some((self.settled.clone(), Vec::new())))
            }

            /// Snapshot B — taken after it.
            async fn load_messages(
                &self,
                _id: &meerkat_core::types::SessionId,
                _strand: &meerkat_core::session_store::TranscriptStrandId,
                _range: std::ops::Range<u64>,
            ) -> Result<Vec<meerkat_core::Message>, meerkat_core::SessionStoreError> {
                Ok(self.settled.messages().to_vec())
            }

            async fn load_rewrites(
                &self,
                _id: &meerkat_core::types::SessionId,
            ) -> Result<Vec<meerkat_core::TranscriptRewriteRecord>, meerkat_core::SessionStoreError>
            {
                Ok(Vec::new())
            }
        }

        struct TearingStore {
            inner: Arc<LocalContinuityStore>,
            channel: Arc<TearingSubstrate>,
        }

        #[async_trait]
        impl ContinuityStore for TearingStore {
            // The trait requires an explicit answer: a double that stayed
            // silent used to assert an authoritative "no record" while
            // meaning "I was never taught to answer", which is how a fix
            // goes inert with every test still green.
            async fn resolve_record_by_session(
                &self,
                _session_id: &meerkat_core::types::SessionId,
            ) -> Result<
                Option<(
                    crate::identity_first::ContinuityRecord,
                    crate::identity_first::FencingToken,
                    crate::identity_first::CheckpointVersion,
                )>,
                crate::identity_first::ContinuityStoreError,
            > {
                Ok(None)
            }

            async fn resolve_many(
                &self,
                identities: &[AgentIdentity],
            ) -> Result<
                std::collections::BTreeMap<AgentIdentity, ContinuityResolveState>,
                ContinuityStoreError,
            > {
                self.inner.resolve_many(identities).await
            }

            async fn load_session_snapshot(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<SessionSnapshot>, ContinuityStoreError> {
                self.inner.load_session_snapshot(session_id).await
            }

            async fn save_session_snapshot(
                &self,
                identity: &AgentIdentity,
                session_id: &meerkat_core::types::SessionId,
                generation: ContinuityGeneration,
                checkpoint_version: CheckpointVersion,
                fencing_token: FencingToken,
                snapshot: &SessionSnapshot,
            ) -> Result<(), ContinuityStoreError> {
                self.inner
                    .save_session_snapshot(
                        identity,
                        session_id,
                        generation,
                        checkpoint_version,
                        fencing_token,
                        snapshot,
                    )
                    .await
            }

            async fn upsert_continuity_record(
                &self,
                record: &ContinuityRecord,
                fencing_token: FencingToken,
            ) -> Result<(), ContinuityStoreError> {
                self.inner
                    .upsert_continuity_record(record, fencing_token)
                    .await
            }

            async fn delete_continuity_record(
                &self,
                identity: &AgentIdentity,
                fencing_token: FencingToken,
            ) -> Result<(), ContinuityStoreError> {
                self.inner
                    .delete_continuity_record(identity, fencing_token)
                    .await
            }

            fn as_incremental_sessions(
                &self,
            ) -> Option<Arc<dyn super::super::contracts::ContinuityIncrementalSessions>>
            {
                Some(self.channel.clone())
            }
        }

        let root = meerkat_core::session_store::TranscriptStrandId::root();
        // The settled document the substrate actually holds: one message.
        let mut settled = meerkat_core::Session::new();
        settled.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("settled turn".to_string()),
        ));
        // The head a composed read would pick up from the earlier snapshot:
        // same session, but describing a two-message transcript.
        let mut earlier = settled.clone();
        earlier.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("since rewritten away".to_string()),
        ));
        let stale_head =
            meerkat_core::session_store::SessionHead::from_session(&earlier, root.clone(), 0)
                .expect("head from session");
        assert_ne!(
            stale_head.message_count,
            settled.messages().len() as u64,
            "the double must actually model a torn pair"
        );

        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(Arc::new(TearingStore {
            inner: Arc::new(LocalContinuityStore::in_memory().expect("store")),
            channel: Arc::new(TearingSubstrate {
                settled: settled.clone(),
                stale_head,
            }),
        })));

        let loaded = meerkat::SessionStore::load(adapter.as_ref(), settled.id())
            .await
            .expect(
                "the head-first load must take ONE substrate snapshot; composing an \
                 independent head read with an independent rows read surfaces the torn pair",
            )
            .expect("the session is head-canonical");
        assert_eq!(
            loaded.messages().len(),
            settled.messages().len(),
            "the loaded document must be the substrate's single-snapshot materialization"
        );
    }

    // -----------------------------------------------------------------
    // Lifecycle authority (the resurrection class)
    // -----------------------------------------------------------------

    /// Seed a continuity record and register `session` on `adapter`, then
    /// land one durable whole-blob save. Returns the identity so a later,
    /// COLD adapter can re-register against the same record.
    async fn seed_durable_blob_session(
        store: &Arc<LocalContinuityStore>,
        adapter: &ContinuitySessionStoreAdapter,
        session: &meerkat_core::Session,
        identity_name: &str,
    ) -> AgentIdentity {
        let identity = AgentIdentity::parse(identity_name).expect("identity");
        store
            .upsert_continuity_record(
                &ContinuityRecord {
                    identity: identity.clone(),
                    agent_runtime_id: AgentRuntimeId::parse(&format!("rt:{identity_name}:0"))
                        .expect("runtime id"),
                    session_id: session.id().clone(),
                    generation: ContinuityGeneration::new(0),
                    checkpoint_version: CheckpointVersion::new(0),
                },
                FencingToken::new(1),
            )
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity: identity.clone(),
                    generation: ContinuityGeneration::new(0),
                    fencing_token: FencingToken::new(1),
                    checkpoint_version: CheckpointVersion::new(0),
                },
            )
            .await
            .expect("register");
        meerkat::SessionStore::save(adapter, session)
            .await
            .expect("durable save");
        identity
    }

    /// RESURRECTION PIN (cold restore). The parking guard used to key off a
    /// process-local "durably written in THIS process" set. That set starts
    /// empty, and neither registration nor loading pre-existing durable data
    /// populated it — so a restarted process could not tell a cold-restored
    /// session from a never-written one.
    ///
    /// The vector, end to end: a second adapter (the restart) restores an
    /// existing durable session, deletes it, and then a stale actor's late
    /// save arrives. Under the old guard that save read as a creation-window
    /// write and PARKED, and the next `register_session` flushed the parked
    /// bytes — resurrecting the deleted document. The existing park-guard
    /// tests all missed this because their helper writes through the adapter
    /// first, which populated the process-local set.
    #[tokio::test]
    async fn cold_restored_session_deleted_then_late_save_cannot_be_resurrected() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let session = meerkat_core::Session::new();

        // Process A writes the durable document and goes away.
        let identity = {
            let first = ContinuitySessionStoreAdapter::new(store.clone());
            seed_durable_blob_session(&store, &first, &session, "agent:cold-restore").await
        };

        // Process B: a brand-new adapter over the SAME store. Nothing has
        // ever been written through this instance — its process-local view is
        // exactly as empty as it is after a restart.
        let cold = ContinuitySessionStoreAdapter::new(store.clone());
        assert_eq!(
            cold.recorded_lifecycle(session.id()),
            None,
            "the cold adapter must start with no lifecycle authority at all"
        );
        assert!(
            meerkat::SessionStore::load(&cold, session.id())
                .await
                .expect("cold restore load")
                .is_some(),
            "the durable document must be visible to the cold adapter"
        );

        meerkat::SessionStore::delete(&cold, session.id())
            .await
            .expect("the cold adapter deletes the restored session");
        assert_eq!(
            cold.recorded_lifecycle(session.id()),
            Some(SessionLifecycle::Removed),
            "delete must leave an explicit removal marker, not merely forget the session"
        );

        // The stale actor's late save.
        let refused = meerkat::SessionStore::save(&cold, &session)
            .await
            .expect_err("a late save for a deleted session must refuse, not park");
        assert!(
            refused.to_string().contains("refusing to park"),
            "the refusal must speak the parking guard's vocabulary: {refused}"
        );
        assert!(
            !parked_pending_bytes(&cold, session.id()),
            "a refused late save must leave nothing for a registration to flush"
        );

        // And registration must not resurrect the document.
        let resolved = store
            .resolve_many(std::slice::from_ref(&identity))
            .await
            .expect("resolve");
        let ContinuityResolveState::Ready { record } = resolved.get(&identity).expect("record")
        else {
            panic!("expected ready record");
        };
        cold.register_session(
            session.id(),
            SessionRuntimeState {
                identity: identity.clone(),
                generation: record.generation,
                fencing_token: FencingToken::new(1),
                checkpoint_version: record.checkpoint_version,
            },
        )
        .await
        .expect("re-registration after delete must not be bricked");
        assert!(
            meerkat::SessionStore::load(&cold, session.id())
                .await
                .expect("load after re-register")
                .is_none(),
            "the deleted document must stay deleted"
        );
    }

    /// The other half of positive lifecycle authority: durable evidence with
    /// no removal. A cold adapter that has written nothing must still refuse
    /// to park a write for a session that ALREADY has a durable document —
    /// those bytes belong to a session someone else owns, and parking them
    /// would either lose the write with the process or flush it over the
    /// durable document at the next registration.
    #[tokio::test]
    async fn cold_restored_durable_session_refuses_an_unregistered_late_save() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let session = meerkat_core::Session::new();
        {
            let first = ContinuitySessionStoreAdapter::new(store.clone());
            seed_durable_blob_session(&store, &first, &session, "agent:cold-durable").await;
        }

        let cold = ContinuitySessionStoreAdapter::new(store.clone());
        let refused = meerkat::SessionStore::save(&cold, &session)
            .await
            .expect_err("an unregistered save over durable state must refuse, not park");
        // Two fail-closed vocabularies are legal here, decided by the
        // durable representation the seed produced: a blob-canonical
        // session refuses through the parking guard; a head-canonical one
        // (the registered seed births heads since the 0.8.11 repin) refuses
        // through the capability rule's registered-owner requirement. Both
        // refuse rather than park, which is the property under test.
        let text = refused.to_string();
        assert!(
            text.contains("refusing to park")
                || (text.contains("refusing to degrade head-canonical session")
                    && text.contains("whole-document persistence")),
            "the refusal must speak a fail-closed guard's vocabulary: {refused}"
        );
        assert_eq!(
            cold.recorded_lifecycle(session.id()),
            Some(SessionLifecycle::DurableObserved),
            "the durable-evidence probe must publish positive authority, \
             not leave the session inferring a creation window from absence"
        );
        assert!(
            !parked_pending_bytes(&cold, session.id()),
            "a refused save must leave nothing parked"
        );
    }

    /// MARK-AFTER-WRITE PIN. `route_delta_write` used to mark the session
    /// durably-observed on the `Durable` branch — BEFORE awaiting the
    /// substrate. A write the substrate refused therefore published a
    /// durability claim it never made good on, and the session's perfectly
    /// legal creation-window park was refused afterwards.
    #[tokio::test]
    async fn a_failed_durable_delta_write_does_not_mark_the_session_durably_observed() {
        let store = Arc::new(IncrementalCapableStore::new());
        let channel = store.incremental.clone();
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(
            store.clone() as Arc<dyn ContinuityStore>
        ));
        let incremental = meerkat::SessionStore::as_incremental(adapter.clone())
            .expect("advertising substrate forwards");

        let session = meerkat_core::Session::new();
        let root = meerkat_core::session_store::TranscriptStrandId::root();
        let identity = AgentIdentity::parse("agent:failed-delta").expect("identity");
        let state = seed_incremental_record(&store, &identity, session.id()).await;
        adapter
            .register_session(session.id(), state)
            .await
            .expect("register");

        channel.refuse_writes(true);
        let message =
            meerkat_core::Message::User(meerkat_core::UserMessage::text("never lands".to_string()));
        let failed = incremental
            .append_messages(session.id(), &root, 0, std::slice::from_ref(&message))
            .await
            .expect_err("the substrate refuses this write");
        assert!(
            failed.to_string().contains("injected substrate"),
            "the substrate failure must ride out typed: {failed}"
        );
        channel.refuse_writes(false);

        assert_eq!(
            adapter.recorded_lifecycle(session.id()),
            None,
            "a refused durable write must publish no lifecycle authority at all"
        );

        // Behavioural half: the bridge drops the registry entry (a teardown
        // that is not an unregistration), and the session must still be
        // inside its creation window, because nothing durable ever landed.
        adapter
            .session_registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&session.id().to_string());
        meerkat::SessionStore::save(adapter.as_ref(), &session)
            .await
            .expect("a session whose only durable write FAILED is still in its creation window");
        assert!(
            parked_pending_bytes(&adapter, session.id()),
            "the creation-window save must park for the registration flush"
        );
    }

    /// The creation window must survive the move to positive authority: a
    /// session with no durable evidence anywhere still parks, on both the
    /// whole-blob verb and the delta channel. Member creation depends on it.
    #[tokio::test]
    async fn creation_window_without_durable_evidence_still_parks() {
        let store = Arc::new(IncrementalCapableStore::new());
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(
            store.clone() as Arc<dyn ContinuityStore>
        ));
        let incremental = meerkat::SessionStore::as_incremental(adapter.clone())
            .expect("advertising substrate forwards");

        let session = meerkat_core::Session::new();
        meerkat::SessionStore::save(adapter.as_ref(), &session)
            .await
            .expect("creation-window save must park, not fail");
        assert!(
            parked_pending_bytes(&adapter, session.id()),
            "the creation-window save must be parked for the registration flush"
        );
        assert_eq!(
            adapter.recorded_lifecycle(session.id()),
            Some(SessionLifecycle::CreationWindow),
            "the creation window must be established POSITIVELY, by a durable-evidence \
             probe that found nothing — never inferred from an empty process-local set"
        );

        let root = meerkat_core::session_store::TranscriptStrandId::root();
        let message = meerkat_core::Message::User(meerkat_core::UserMessage::text(
            "creation-window delta".to_string(),
        ));
        incremental
            .append_messages(session.id(), &root, 0, std::slice::from_ref(&message))
            .await
            .expect("creation-window delta writes must park, not fail");
        assert!(
            adapter.parked_deltas.is_parked(session.id()),
            "the creation-window delta must be parked"
        );
    }

    // -----------------------------------------------------------------
    // O(delta) persistence (the whole-document class)
    // -----------------------------------------------------------------

    /// Register `session`, then seed the head-canonical representation
    /// through the delta channel: rows on the root strand plus the head that
    /// adopts them. Returns the incremental view for later assertions.
    async fn seed_head_canonical_session(
        store: &Arc<LocalContinuityStore>,
        adapter: &Arc<ContinuitySessionStoreAdapter>,
        session: &meerkat_core::Session,
        identity_name: &str,
    ) -> Arc<dyn meerkat_core::session_store::IncrementalSessionStore> {
        let identity = AgentIdentity::parse(identity_name).expect("identity");
        store
            .upsert_continuity_record(
                &ContinuityRecord {
                    identity: identity.clone(),
                    agent_runtime_id: AgentRuntimeId::parse(&format!("rt:{identity_name}:0"))
                        .expect("runtime id"),
                    session_id: session.id().clone(),
                    generation: ContinuityGeneration::new(0),
                    checkpoint_version: CheckpointVersion::new(0),
                },
                FencingToken::new(1),
            )
            .await
            .expect("seed record");
        adapter
            .register_session(
                session.id(),
                SessionRuntimeState {
                    identity,
                    generation: ContinuityGeneration::new(0),
                    fencing_token: FencingToken::new(1),
                    checkpoint_version: CheckpointVersion::new(0),
                },
            )
            .await
            .expect("register");

        let incremental = meerkat::SessionStore::as_incremental(adapter.clone())
            .expect("the bundled store advertises the delta channel");
        let root = meerkat_core::session_store::TranscriptStrandId::root();
        incremental
            .append_messages(session.id(), &root, 0, session.messages())
            .await
            .expect("seed rows");
        let head = meerkat_core::session_store::SessionHead::from_session(session, root, 0)
            .expect("head from session");
        incremental
            .save_head(&head, meerkat_core::session_store::SessionHeadCas::Create)
            .await
            .expect("adopt head");
        incremental
    }

    /// O(delta) PIN. The ordinary turn on a head-canonical session used to
    /// open with `Session::to_persisted_bytes()` — a full `serde_json::to_vec`
    /// of the whole document — and only THEN ask whether the substrate could
    /// take the write incrementally. Write volume was O(delta); the path was
    /// O(document) in serialization, hashing, allocation and copies.
    ///
    /// The honest observation is the adapter's own whole-document counter,
    /// not `meerkat_core::checkpoint::global_session_encode_bytes()`: that
    /// counter is fed by exactly two meerkat sites (the core executor's
    /// boundary snapshot and `PersistentSessionService`'s checkpoint mint),
    /// NOT by `Session::to_persisted_bytes`, so asserting on it here would be
    /// vacuous — it cannot move whichever way this adapter behaves. Those
    /// meerkat-side runtime snapshots are whole-document by meerkat's own
    /// published contract, are produced above this adapter, and are not
    /// Meta continuity cost.
    #[tokio::test]
    async fn head_canonical_steady_state_save_performs_no_whole_document_pass() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(
            store.clone() as Arc<dyn ContinuityStore>
        ));

        let mut session = meerkat_core::Session::new();
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("one".to_string()),
        ));
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("two".to_string()),
        ));
        let incremental =
            seed_head_canonical_session(&store, &adapter, &session, "agent:odelta").await;
        let root = meerkat_core::session_store::TranscriptStrandId::root();
        assert_eq!(
            adapter.whole_document_passes(),
            0,
            "seeding a head-canonical session through the delta channel must encode nothing"
        );

        // The ordinary turn: one appended message, saved through the
        // whole-document `SessionStore::save` verb.
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("three".to_string()),
        ));
        meerkat::SessionStore::save(adapter.as_ref(), &session)
            .await
            .expect("head-canonical steady-state save");

        assert_eq!(
            adapter.whole_document_passes(),
            0,
            "a head-canonical STEADY-STATE save must not serialize, hash or compare-token \
             the whole document"
        );
        assert_eq!(
            adapter.whole_document_encode_bytes(),
            0,
            "and it must not produce a single whole-document byte"
        );

        // It genuinely persisted, as delta rows on the SAME strand plus one
        // small head — not as a rebase and not as a blob.
        let head = incremental
            .load_head(session.id())
            .await
            .expect("load head")
            .expect("head-canonical head row");
        assert_eq!(
            head.strand, root,
            "a plain append must not open a new strand"
        );
        assert_eq!(
            head.message_count, 3,
            "the head must adopt the appended row"
        );
        assert_eq!(
            incremental
                .load_messages(session.id(), &root, 0..3)
                .await
                .expect("durable rows")
                .len(),
            3,
            "the appended message must be durable"
        );
        let loaded = meerkat::SessionStore::load(adapter.as_ref(), session.id())
            .await
            .expect("load")
            .expect("session");
        assert_eq!(
            loaded.messages(),
            session.messages(),
            "the head-canonical read must serve exactly what the delta save wrote"
        );
    }

    /// FAIL-LOUD PIN (contract clause 4/5). A head-canonical session whose
    /// owning identity is not registered has no legal write path: the delta
    /// channel needs a continuity cursor, and degrading to whole-blob
    /// persistence would both re-impose the O(document) cost and leave the
    /// durable head row describing a transcript the blob no longer agrees
    /// with. It must refuse typed, before serializing anything.
    #[tokio::test]
    async fn head_canonical_save_without_a_registered_owner_refuses_instead_of_writing_a_blob() {
        let store = Arc::new(LocalContinuityStore::in_memory().expect("store"));
        let adapter = Arc::new(ContinuitySessionStoreAdapter::new(
            store.clone() as Arc<dyn ContinuityStore>
        ));

        let mut session = meerkat_core::Session::new();
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("one".to_string()),
        ));
        seed_head_canonical_session(&store, &adapter, &session, "agent:headless-owner").await;

        // The bridge tears the registry entry down without unregistering.
        adapter
            .session_registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&session.id().to_string());

        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("two".to_string()),
        ));
        let refused = meerkat::SessionStore::save(adapter.as_ref(), &session)
            .await
            .expect_err("a head-canonical save with no registered owner must refuse");
        // Both SIDES of the degradation, not one phrase. The refusal is only
        // actionable if it says what representation is being left and what it
        // would be replaced by, and asserting the pair keeps this test
        // discriminating if either half is ever dropped.
        let refused_text = refused.to_string();
        assert!(
            refused_text.contains("refusing to degrade head-canonical session")
                && refused_text.contains("whole-document persistence"),
            "the refusal must name the degradation it is preventing, both sides of it: {refused}"
        );
        assert_eq!(
            adapter.whole_document_passes(),
            0,
            "capability selection happens BEFORE serialization, so a refused \
             head-canonical save must not have encoded anything"
        );
        assert!(
            !parked_pending_bytes(&adapter, session.id()),
            "a refused save must leave nothing parked"
        );
    }
}
