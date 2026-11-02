//! One realm over a host store: the fence, the lease, the scopes and the
//! commit engine every host store adapter of the process shares.
//!
//! A process opens ONE [`SpiRealm`] per realm: it acquires the fence,
//! renews it, keeps the local lease clock, lists every head once, and owns
//! the per-scope journal state. The runtime adapter
//! ([`super::SpiRuntimeStore`]) and the mob adapter (in meerkat-mob) both
//! commit through it, so the second adapter never fences out the first.
//!
//! # Concurrency
//!
//! There is no realm-wide lock on the commit path. Each scope has its own
//! async mutex ([`ScopeCell`]); a call locks the cells of the scopes it
//! touches, in [`ScopeId`] order, and holds them across its host commit, so
//! no other call observes a scope's state before the host acknowledged it.
//! Adapters add a realm gate on top (shared for single-scope calls,
//! exclusive for cross-scope reads); the realm itself never serializes two
//! scopes' commits.
//!
//! # Fencing
//!
//! The realm keeps the writer rules of [`meerkat_core::host_store`]:
//!
//! - Open acquires with [`AcquireMode::Exclusive`] by default: a live holder
//!   answers [`SpiError::Held`] with what remains of its lease. After an
//!   acquire that superseded a live holder (a takeover), the open waits out
//!   [`FenceGrant::superseded_lease`](meerkat_core::host_store::FenceGrant)
//!   before the realm serves anything.
//! - A background task starts a renew once per the host's renew interval
//!   ([`HostStore::fence_timing`]), on a fixed period, and a renew slower
//!   than the interval counts as failed, so renew starts stay one interval
//!   apart. A fenced out answer poisons the realm; any other failed renew,
//!   or a window seen closed, STOPS the writer, and a stopped writer never
//!   resumes on a later renew: it re-acquires the fence (exclusive; its own
//!   still-recorded lease is superseded), resumes only when a fresh listing
//!   shows no batch it did not write, and renews once with the new token
//!   before it does. Another live holder, or a foreign batch, poisons the
//!   realm. Renews and re-acquires never overlap, and a re-acquire runs on
//!   a task of its own, so a dropped caller never leaves a granted token
//!   unrecorded.
//! - **Self-fence on the local clock:** a mutation runs only while the last
//!   renew did not fail and the last successful acquire or renew *started*
//!   less than the lease minus the host-declared margin ago
//!   ([`FenceTiming::self_fence_window`]). The realm is the process's
//!   [`SelfFence`]: the run loop reads the same window right before each
//!   model call and each tool dispatch. Otherwise the call re-acquires
//!   first, and refuses when that fails; a dispatch waits for that
//!   re-acquire for at most [`SpiRealm::dispatch_wait_bound`] (one
//!   self-fence window plus the re-acquire's own timeouts,
//!   [`SelfFence::dispatch_window`]). In the steady state this is a local
//!   clock read: no store I/O is added to a round.
//!   Every per-round commit of a run (commits A and B) is such a mutation,
//!   so a writer past its window dispatches no tool and makes no model call
//!   after its next commit point.
//!
//! # Failure
//!
//! A batch that is refused, or whose outcome stays unknown after the retry
//! budget, poisons every scope it touched: the process may hold state the
//! host does not, so those scopes refuse every later call until the process
//! reopens the realm. A fence loss poisons the whole realm. An unknown
//! outcome is always settled (by retrying the same batch, or by reading the
//! head's commit id) before a scope's next version is written, and a
//! poisoned scope writes no next version at all. A batch whose outcome stays
//! unknown is remembered as such, so a later re-acquire does not read it as
//! another writer's.
//!
//! # Cancellation
//!
//! An adapter call changes memory before its batch commits, and its future
//! can be dropped at any await. Adapters hold their locked scopes in an
//! [`ArmedScopes`]: dropped while armed (the in-memory change has started
//! and its batch is not handed off), it poisons them. The commit runs on a
//! task that owns the scopes, so once handed off it lands and is adopted,
//! or poisons, whether or not the caller still waits. A body counts as
//! loaded only once the adapter installed it in memory.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use meerkat_core::host_store::{
    AcquireMode, CommitBatch, CommitId, FenceTiming, FenceToken, HostStore, HostStoreError,
    MAX_BATCH_BYTES, MAX_ROW_BYTES, Row, RowIndex, ScopeKind, ScopeName, ScopeRef, ScopeUpdate,
    ScopeVersion, SelfFence, SelfFenceWindow,
};
use std::sync::atomic::AtomicBool;
use tokio::sync::{Mutex, OwnedMutexGuard};

use super::SpiError;
use super::codec::{
    self, CHUNK_TAG, ChunkHeadV1, ChunkRef, FORMAT_V1, PinnedV1, RowOp, ScopeHeadV1, WindowV1,
};

/// A row entry value larger than this goes to a chunk scope.
pub const INLINE_VALUE_LIMIT: usize = 256 * 1024;

/// A pinned (head) value larger than this goes to a chunk scope.
pub const PINNED_INLINE_LIMIT: usize = 32 * 1024;

/// A head is kept below this; larger inline pinned values move to chunks.
pub const HEAD_LIMIT: usize = 256 * 1024;

/// The most head and row bytes the journal puts in one batch, leaving the
/// rest of [`MAX_BATCH_BYTES`] as margin.
pub const BATCH_BUDGET: usize = MAX_BATCH_BYTES - 512 * 1024;

/// Chunk rows written per batch.
pub const CHUNK_ROWS_PER_BATCH: usize = 7;

/// A checkpoint (the host store reference's re-base) is written once the rows
/// appended since the last one pass
/// `max(CHECKPOINT_FACTOR x live bytes, CHECKPOINT_MIN_BYTES)`: the
/// scope's next commit that carries its full image appends a reset and every
/// live row entry, and drops every earlier row, in one batch. A scope's
/// visible rows therefore stay within that threshold plus its last commit's
/// rows. After a checkpoint the counter starts at the image's own bytes, so
/// at least three times the live bytes are appended before the next one: a
/// checkpoint adds at most about a third to the row bytes a scope writes.
pub const CHECKPOINT_FACTOR: u64 = 4;

/// The least row bytes appended between two checkpoints (see
/// [`CHECKPOINT_FACTOR`]). It bounds the superseded rows of a scope whose
/// live entries are small but rewritten whole on every commit (a runtime's
/// ops lifecycle snapshot is rewritten on every terminal operation
/// transition): such a scope keeps at most this many row bytes plus its last
/// commit's. 4 MiB before 0.9.7; the format is unchanged, and every release
/// reads a scope checkpointed at either value.
pub const CHECKPOINT_MIN_BYTES: u64 = 256 * 1024;

/// Rows read per host call when loading.
pub(crate) const READ_PAGE_ROWS: usize = 16;

/// The most heads one `list_heads` call asks for, when the realm opens and
/// when a stopped writer re-acquires. A listing visits every scope the realm
/// holds, archived sessions and chunk scopes included, and a store's cost is
/// often per call, so a page is large: the calls of a listing are
/// `ceil(scopes / LIST_PAGE_HEADS)` per kind, at least one. A store may
/// answer fewer heads with `next` set, and the listing reads on from there.
/// A page of typical runtime heads (a few KiB each) is a few MiB; each head
/// stays under [`HEAD_LIMIT`]. 256 before 0.9.7; no stored format depends on
/// it.
pub(crate) const LIST_PAGE_HEADS: usize = 2048;

/// Values at least this large keep their allocation in the stamp, so an
/// unchanged value (the same allocation) is recognized without hashing.
const SHARED_STAMP_MIN: usize = 64 * 1024;

/// The scope kinds a boot lists.
const LISTED_KINDS: [ScopeKind; 4] = [
    ScopeKind::Session,
    ScopeKind::Identity,
    ScopeKind::Mob,
    ScopeKind::Record,
];

/// How a realm opens and how long it retries. The fence timing (renew
/// interval, lease, self-fence window) is the host's own
/// ([`HostStore::fence_timing`]).
#[derive(Debug, Clone)]
pub struct SpiRealmConfig {
    /// How the open acquires the fence.
    pub acquire: AcquireMode,
    /// How long one renew may take before it counts as failed. The realm
    /// holds renews to at most the host's renew interval: a slower
    /// renew would push the next renew's start more than one interval past
    /// the last.
    pub renew_timeout: Duration,
    /// How long an unknown or unavailable commit is retried before the
    /// scopes it touched are poisoned.
    pub commit_retry_budget: Duration,
}

impl Default for SpiRealmConfig {
    fn default() -> Self {
        Self {
            acquire: AcquireMode::Exclusive,
            renew_timeout: Duration::from_secs(5),
            commit_retry_budget: Duration::from_secs(30),
        }
    }
}

/// A family of journaled scopes: its tag (the scope name prefix), its kind,
/// and the entry groups it keeps in the head (pinned: readable at boot
/// without loading the body).
///
/// A tag names one family in a realm. A scope's name starts with its
/// family's tag and a `:`, so a tag never contains `:`. Core never uses a
/// tag that starts with `x` or `mx`: a layer above Core that keeps scopes of
/// its own in the realm ([`SpiRealm::claim`], [`SpiRealm::commit`]) names its
/// families with those prefixes, and no family a later Core release adds can
/// take one of its tags.
#[derive(Debug, Clone, Copy)]
pub struct ScopeFamily {
    pub tag: &'static str,
    pub kind: ScopeKind,
    pub pinned: &'static [&'static str],
}

impl ScopeFamily {
    /// The group of an entry key: the text before its first `/`.
    #[must_use]
    pub fn group(entry_key: &str) -> &str {
        entry_key
            .split_once('/')
            .map_or(entry_key, |(group, _)| group)
    }

    /// Whether the family keeps this entry in the head.
    #[must_use]
    pub fn is_pinned(&self, entry_key: &str) -> bool {
        self.pinned.contains(&Self::group(entry_key))
    }
}

impl PartialEq for ScopeFamily {
    fn eq(&self, other: &Self) -> bool {
        self.tag == other.tag
    }
}

impl Eq for ScopeFamily {}

impl std::hash::Hash for ScopeFamily {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.tag.hash(state);
    }
}

impl PartialOrd for ScopeFamily {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScopeFamily {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.tag.cmp(other.tag)
    }
}

/// The logical identity of a journaled scope: its family and its key. The
/// host name of its current incarnation is kept in its [`ScopeState`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopeId {
    pub family: ScopeFamily,
    pub key: String,
}

impl ScopeId {
    #[must_use]
    pub fn new(family: ScopeFamily, key: impl Into<String>) -> Self {
        Self {
            family,
            key: key.into(),
        }
    }
}

impl std::fmt::Display for ScopeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.family.tag, self.key)
    }
}

/// One scope's lock and journal state.
pub type ScopeCell = Mutex<ScopeState>;

/// A locked scope.
pub type ScopeGuard = OwnedMutexGuard<ScopeState>;

#[derive(Debug, Clone)]
enum Pinned {
    Inline(Arc<Vec<u8>>),
    Chunk(ChunkRef),
}

/// What the journal keeps of one committed row entry.
#[derive(Debug, Clone)]
struct Stamp {
    digest: [u8; 32],
    len: u64,
    /// The chunk holding the value, when it is not inline.
    chunk: Option<ChunkRef>,
    /// The committed allocation of a large value (see [`SHARED_STAMP_MIN`]).
    shared: Option<Arc<Vec<u8>>>,
    /// The bytes the entry takes in a row.
    op_bytes: u64,
}

impl Stamp {
    fn matches(&self, value: &Arc<Vec<u8>>) -> bool {
        if let Some(shared) = &self.shared
            && Arc::ptr_eq(shared, value)
        {
            return true;
        }
        self.len == value.len() as u64 && self.digest == codec::sha256(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Body {
    /// Only the head is known (a listed scope not opened yet).
    Unloaded,
    /// The rows are folded into `rows` (or the scope does not exist yet).
    Loaded,
}

/// One scope's journal state: what the host holds for it, as far as this
/// process committed or loaded it.
#[derive(Debug)]
pub struct ScopeState {
    id: ScopeId,
    name: Option<ScopeName>,
    version: Option<ScopeVersion>,
    pinned: BTreeMap<String, Pinned>,
    rows: BTreeMap<String, Stamp>,
    window: WindowV1,
    live_bytes: u64,
    since_checkpoint: u64,
    /// Chunks referenced from rows of the window (spills), by row index.
    row_chunks: Vec<(u64, ChunkRef)>,
    body: Body,
    poison: Option<String>,
}

impl ScopeState {
    fn empty(id: ScopeId) -> Self {
        Self {
            id,
            name: None,
            version: None,
            pinned: BTreeMap::new(),
            rows: BTreeMap::new(),
            window: WindowV1::default(),
            live_bytes: 0,
            since_checkpoint: 0,
            row_chunks: Vec::new(),
            body: Body::Loaded,
            poison: None,
        }
    }

    fn from_head(
        id: ScopeId,
        name: ScopeName,
        version: ScopeVersion,
        head: &ScopeHeadV1,
    ) -> Result<Self, SpiError> {
        let mut pinned = BTreeMap::new();
        for (key, value) in &head.pin {
            let entry = match value {
                PinnedV1::Utf8(text) => Pinned::Inline(Arc::new(text.as_bytes().to_vec())),
                PinnedV1::Hex(text) => Pinned::Inline(Arc::new(codec::unhex(text)?)),
                PinnedV1::Chunk(chunk) => Pinned::Chunk(ChunkRef::from_wire(chunk)?),
            };
            pinned.insert(key.clone(), entry);
        }
        Ok(Self {
            id,
            name: Some(name),
            version: Some(version),
            pinned,
            rows: BTreeMap::new(),
            window: head.w,
            live_bytes: head.lb,
            since_checkpoint: head.sb,
            row_chunks: Vec::new(),
            body: if head.w.s == head.w.e {
                Body::Loaded
            } else {
                Body::Unloaded
            },
            poison: None,
        })
    }

    #[must_use]
    pub fn id(&self) -> &ScopeId {
        &self.id
    }

    /// Whether the host holds an incarnation of this scope.
    #[must_use]
    pub fn exists(&self) -> bool {
        self.name.is_some()
    }

    /// Whether the rows are folded (or there are none).
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.body == Body::Loaded
    }

    /// The poison cause, when this scope is poisoned.
    #[must_use]
    pub fn poison(&self) -> Option<&str> {
        self.poison.as_deref()
    }

    /// Whether the next commit should carry the scope's full image, so the
    /// journal writes a checkpoint (rows since the last checkpoint pass
    /// `max(4 x live bytes, 256 KiB)`: [`CHECKPOINT_FACTOR`],
    /// [`CHECKPOINT_MIN_BYTES`]).
    #[must_use]
    pub fn wants_full_image(&self) -> bool {
        self.since_checkpoint
            > (CHECKPOINT_FACTOR.saturating_mul(self.live_bytes)).max(CHECKPOINT_MIN_BYTES)
    }

    /// The committed entry keys under `group` (the entry `group` itself and
    /// every `group/...`), pinned and row entries alike.
    #[must_use]
    pub fn entry_keys_in_group(&self, group: &str) -> Vec<String> {
        let prefix = format!("{group}/");
        self.pinned
            .keys()
            .chain(self.rows.keys())
            .filter(|key| key.as_str() == group || key.starts_with(&prefix))
            .cloned()
            .collect()
    }

    /// The adapter installed the body [`SpiRealm::load_body`] answered: the
    /// scope may now commit.
    pub fn mark_loaded(&mut self) {
        self.body = Body::Loaded;
    }

    /// Forget the folded rows (the body is dropped from memory). The next
    /// commit needs a [`SpiRealm::load_body`] first.
    pub fn unload(&mut self) {
        if self.window.s != self.window.e {
            self.rows.clear();
            self.row_chunks.clear();
            self.body = Body::Unloaded;
        }
    }

    fn chunk_refs(&self) -> Vec<ChunkRef> {
        let mut refs: Vec<ChunkRef> = self
            .pinned
            .values()
            .filter_map(|pinned| match pinned {
                Pinned::Chunk(chunk) => Some(chunk.clone()),
                Pinned::Inline(_) => None,
            })
            .collect();
        refs.extend(self.rows.values().filter_map(|stamp| stamp.chunk.clone()));
        refs.extend(self.row_chunks.iter().map(|(_, chunk)| chunk.clone()));
        refs
    }

    fn head(
        &self,
        pinned: &BTreeMap<String, Pinned>,
        window: WindowV1,
        live: u64,
        since: u64,
    ) -> Result<Vec<u8>, SpiError> {
        let pin = pinned
            .iter()
            .map(|(key, value)| {
                let wire = match value {
                    Pinned::Inline(bytes) => codec::pin_inline(bytes),
                    Pinned::Chunk(chunk) => PinnedV1::Chunk(chunk.to_wire()),
                };
                (key.clone(), wire)
            })
            .collect();
        ScopeHeadV1 {
            f: FORMAT_V1,
            fam: self.id.family.tag.to_string(),
            key: self.id.key.clone(),
            pin,
            w: window,
            lb: live,
            sb: since,
        }
        .encode()
    }
}

/// What one call changes in one locked scope.
#[derive(Debug, Clone)]
pub enum ScopeDelta {
    /// These entries are set (`Some`) or removed (`None`); the rest stay.
    Changes(Vec<(String, Option<Arc<Vec<u8>>>)>),
    /// The scope's complete image: every entry not in it is removed. Lets
    /// the journal write a checkpoint ([`ScopeState::wants_full_image`]).
    Full(BTreeMap<String, Arc<Vec<u8>>>),
}

impl ScopeDelta {
    /// Whether the delta names no entry at all (a full image never counts
    /// as empty: it may remove every entry).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Changes(changes) if changes.is_empty())
    }
}

/// A listed head not yet claimed by an adapter.
#[derive(Debug, Clone)]
struct ListedHead {
    name: ScopeName,
    version: ScopeVersion,
    head: ScopeHeadV1,
}

#[derive(Debug, Clone)]
struct ChunkMeta {
    version: ScopeVersion,
    owner: String,
}

/// A chunk to write before the batch that references it.
#[derive(Debug, Clone)]
struct PendingChunk {
    chunk: ChunkRef,
    owner: ScopeName,
    bytes: Arc<Vec<u8>>,
}

/// The planned update of one scope.
struct ScopePlan {
    index: usize,
    family_tag: &'static str,
    key: String,
    old_window: WindowV1,
    old_since: u64,
    scope: ScopeRef,
    previous: Option<ScopeVersion>,
    next: ScopeVersion,
    delete: bool,
    pinned_after: BTreeMap<String, Pinned>,
    rows: Vec<Row>,
    checkpoint: bool,
    row_changes: Vec<(String, Option<Stamp>)>,
    live_after: u64,
    new_chunks: Vec<PendingChunk>,
    dead_chunks: Vec<ChunkRef>,
    spill: Option<ChunkRef>,
}

impl ScopePlan {
    fn row_bytes(&self) -> usize {
        self.rows.iter().map(Row::len).sum()
    }
}

/// The shared realm handle. See the module documentation.
pub struct SpiRealm {
    host: Arc<dyn HostStore>,
    config: SpiRealmConfig,
    timing: FenceTiming,
    /// How long after the start of the last successful acquire or renew
    /// this writer may write (the lease minus the self-fence margin).
    self_fence_window: Duration,
    /// The token this writer commits with; a re-acquire replaces it.
    fence: AtomicU64,
    /// What the open waited out of a superseded holder's lease.
    superseded_lease: Duration,
    writer: String,
    counter: AtomicU64,
    lease_start: StdMutex<Instant>,
    /// Set when a renew failed or the window was seen closed; cleared only
    /// by a fresh fenced acquire, never by a renew.
    stopped: AtomicBool,
    /// Commits hold it shared; a re-acquire holds it exclusive, so no batch
    /// carrying the old token is in flight when the token changes.
    fence_gate: tokio::sync::RwLock<()>,
    /// Serializes re-acquires.
    renew_gate: Mutex<()>,
    /// The re-acquire [`Self::ensure_lease`] runs on a task of its own, while
    /// one is in flight: every caller of a stopped writer waits on that one
    /// attempt, so waits that give up during an outage leave one attempt
    /// behind them, not one each.
    reacquire_in_flight: StdMutex<Option<SharedReacquire>>,
    /// The version of every visible scope as this writer last saw or wrote
    /// it: a re-acquire compares a fresh listing with it to prove no other
    /// writer committed while this one was stopped.
    known_heads: StdMutex<HashMap<(ScopeKind, String), ScopeVersion>>,
    /// Scopes a batch with an unknown outcome touched, with every version
    /// (`None`: absent) each may be at: the batch may or may not have
    /// landed. A re-acquire accepts any of them; a later landed batch
    /// on the scope settles it.
    uncertain: StdMutex<HashMap<(ScopeKind, String), Vec<Option<ScopeVersion>>>>,
    realm_poison: StdMutex<Option<String>>,
    poisoned_scopes: AtomicUsize,
    cells: StdMutex<HashMap<ScopeId, Arc<ScopeCell>>>,
    listed: StdMutex<HashMap<String, Vec<ListedHead>>>,
    /// Families claimed so far, with the refusal of a claim that failed.
    claimed: StdMutex<HashMap<&'static str, Result<(), String>>>,
    chunks: StdMutex<HashMap<String, ChunkMeta>>,
    /// Per-realm adapter state, one value per type ([`SpiRealm::extension`]).
    extensions: Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    /// The renew timeout in force: the configured one, held to the renew
    /// interval.
    renew_timeout: Duration,
    /// This realm, for the work it hands to tasks of its own.
    me: std::sync::Weak<SpiRealm>,
}

impl std::fmt::Debug for SpiRealm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpiRealm")
            .field("fence", &self.fence())
            .field("writer", &self.writer)
            .field("poisoned", &self.realm_poisoned())
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl SelfFence for SpiRealm {
    fn window(&self) -> SelfFenceWindow {
        SelfFenceWindow {
            elapsed: self.lease_started().elapsed(),
            window: self.self_fence_window,
            renew_failed: self.stopped.load(Ordering::Acquire),
        }
    }

    /// While the window is open, the reading as it is: a clock read, no
    /// store I/O. Otherwise the dispatch waits for the re-acquire a write
    /// would wait for ([`SpiRealm::ensure_lease`]: it joins the re-acquire in
    /// flight, or starts one on a task of its own when none is, so the realm
    /// runs one at a time and giving up on it leaves that one running), for
    /// at most [`SpiRealm::dispatch_wait_bound`], and reads the window again:
    /// open only when a re-acquire resumed the writer. A
    /// re-acquire that fails without proving another writer (the store
    /// refused an acquire, or the renew after it failed) is tried again
    /// after one renew interval, as the realm's own renewal does, until the
    /// bound; another writer (the realm is poisoned) ends the wait at once.
    async fn dispatch_window(&self) -> SelfFenceWindow {
        let window = SelfFence::window(self);
        if window.is_open() {
            return window;
        }
        let bound = self.dispatch_wait_bound();
        let retry_after = self.timing.renew_interval();
        let mut last_error = None;
        let reacquired = tokio::time::timeout(bound, async {
            loop {
                match self.ensure_lease().await {
                    Ok(()) => return,
                    Err(error @ SpiError::RealmPoisoned(_)) => {
                        last_error = Some(error);
                        return;
                    }
                    Err(error) => last_error = Some(error),
                }
                tokio::time::sleep(retry_after).await;
            }
        })
        .await;
        // The window read after the wait decides, not the attempts' outcome:
        // another attempt may have resumed the writer meanwhile, or a new
        // stop may have closed the window again since.
        let window = SelfFence::window(self);
        if !window.is_open() {
            match (reacquired, last_error) {
                (_, Some(error @ SpiError::RealmPoisoned(_))) => tracing::warn!(
                    %error,
                    "host store writer's fence window is closed and another writer holds the \
                     realm; the dispatch does not proceed"
                ),
                (Ok(()), _) => tracing::warn!(
                    "host store writer's fence window closed again right after its re-acquire; \
                     the dispatch does not proceed"
                ),
                (Err(_), Some(error)) => tracing::warn!(
                    %error,
                    ?bound,
                    "host store writer's fence window stayed closed: no re-acquire restored it \
                     within the dispatch wait bound; the dispatch does not proceed"
                ),
                (Err(_), None) => tracing::warn!(
                    ?bound,
                    "host store writer's fence window stayed closed: its re-acquire did not \
                     finish within the dispatch wait bound; the dispatch does not proceed"
                ),
            }
        }
        window
    }

    /// The realm lost its fence: it is poisoned (another writer's holder or
    /// batch, a fenced-out renew or commit, or a break of the writer's own
    /// rules), and this writer never resumes.
    fn lost(&self) -> bool {
        self.realm_poisoned().is_some()
    }
}

/// How long a dispatch waits, at most, for the re-acquire of a writer whose
/// fence window closed ([`SelfFence::dispatch_window`]): the re-acquire's
/// own bounded steps, its two acquires (the exclusive one, then the takeover
/// of the writer's own lease, each under `acquire_timeout`) and its renew
/// (under `renew_timeout`), plus one self-fence window for its steps that
/// have no timeout of their own (the drain of the commits in flight and the
/// listing of every head).
///
/// The bound keeps the run loop from waiting on a store that does not
/// answer; fence safety does not depend on it, since a dispatch proceeds
/// only on an open window. Right after a renew stop the store still records
/// this writer's lease, so a re-acquire takes both acquires; under the tail
/// latency that stopped the writer they are slow too, and a bound of one
/// self-fence window alone cuts such a re-acquire off.
pub(crate) fn dispatch_wait_bound(
    self_fence_window: Duration,
    acquire_timeout: Duration,
    renew_timeout: Duration,
) -> Duration {
    self_fence_window
        .saturating_add(acquire_timeout.saturating_mul(2))
        .saturating_add(renew_timeout)
}

/// The re-acquire in flight, shared by every caller that waits on it.
type SharedReacquire =
    futures::future::Shared<futures::future::BoxFuture<'static, Result<(), SpiError>>>;

/// Clears the realm's re-acquire slot when the re-acquire task ends, however
/// it ends.
struct ClearReacquireInFlight(Arc<SpiRealm>);

impl Drop for ClearReacquireInFlight {
    fn drop(&mut self) {
        *lock(&self.0.reacquire_in_flight) = None;
    }
}

fn lock<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn next_version(previous: Option<ScopeVersion>) -> Result<ScopeVersion, SpiError> {
    match previous {
        None => Ok(ScopeVersion::FIRST),
        Some(previous) => previous
            .next()
            .ok_or_else(|| SpiError::Corrupt("a scope version overflowed".into())),
    }
}

fn op_bytes(key: &str, value_len: usize) -> u64 {
    (1 + 4 + key.len() + 4 + value_len) as u64
}

impl SpiRealm {
    /// Acquire the realm's fence, start renewing it, wait out a superseded
    /// holder's lease, and list every head (after the acquire, before
    /// the first commit). Chunks whose owner no longer exists are deleted.
    ///
    /// # Errors
    /// [`SpiError::Held`] when an exclusive acquire meets a live holder; the
    /// host retries after `remaining`, or takes over.
    pub async fn open(
        host: Arc<dyn HostStore>,
        config: SpiRealmConfig,
    ) -> Result<Arc<Self>, SpiError> {
        let timing = host.fence_timing();
        let self_fence_window = timing.self_fence_window();
        let started = Instant::now();
        let grant = match host.acquire_fence(config.acquire).await {
            Ok(grant) => grant,
            Err(HostStoreError::Held { holder, remaining }) => {
                return Err(SpiError::Held { holder, remaining });
            }
            Err(error) => return Err(SpiError::Host(error)),
        };
        let renew_timeout = config.renew_timeout.min(timing.renew_interval());
        let realm = Arc::new_cyclic(|me| Self {
            host,
            timing,
            self_fence_window,
            fence: AtomicU64::new(grant.token.get()),
            superseded_lease: grant.superseded_lease,
            writer: uuid::Uuid::now_v7().simple().to_string(),
            counter: AtomicU64::new(0),
            lease_start: StdMutex::new(started),
            stopped: AtomicBool::new(false),
            fence_gate: tokio::sync::RwLock::new(()),
            renew_gate: Mutex::new(()),
            reacquire_in_flight: StdMutex::new(None),
            known_heads: StdMutex::new(HashMap::new()),
            uncertain: StdMutex::new(HashMap::new()),
            realm_poison: StdMutex::new(None),
            poisoned_scopes: AtomicUsize::new(0),
            cells: StdMutex::new(HashMap::new()),
            listed: StdMutex::new(HashMap::new()),
            claimed: StdMutex::new(HashMap::new()),
            chunks: StdMutex::new(HashMap::new()),
            extensions: Mutex::new(HashMap::new()),
            config,
            renew_timeout,
            me: me.clone(),
        });
        realm.spawn_renewal();
        if !grant.superseded_lease.is_zero() {
            // The superseded holder stops writing the self-fence margin
            // before its recorded lease ends; nothing here serves until that
            // lease is over.
            tokio::time::sleep(grant.superseded_lease).await;
        }
        realm.list_all().await?;
        realm.sweep_ownerless_chunks().await;
        Ok(realm)
    }

    /// The fence token this realm commits with now.
    #[must_use]
    pub fn fence(&self) -> FenceToken {
        FenceToken::new(self.fence.load(Ordering::Acquire))
    }

    /// The options this realm was opened with.
    #[must_use]
    pub fn config(&self) -> &SpiRealmConfig {
        &self.config
    }

    /// The host's fence timing.
    #[must_use]
    pub fn timing(&self) -> FenceTiming {
        self.timing
    }

    /// How long a dispatch waits, at most, for this writer's re-acquire once
    /// its fence window closed: the re-acquire's own bounded steps, its two
    /// acquires (the exclusive one, then the takeover of this writer's own
    /// lease, each under [`SpiRealmConfig::renew_timeout`]) and its renew
    /// (held to one renew interval), plus one self-fence window for its
    /// steps that have no timeout of their own (the drain of the commits in
    /// flight and the listing of every head). 35 s at the default timing and
    /// configuration.
    #[must_use]
    pub fn dispatch_wait_bound(&self) -> Duration {
        dispatch_wait_bound(
            self.self_fence_window,
            self.config.renew_timeout,
            self.renew_timeout,
        )
    }

    /// What remained of the superseded holder's recorded lease, which the
    /// open waited out before it served anything; zero when the acquire
    /// superseded no live holder.
    #[must_use]
    pub fn superseded_lease(&self) -> Duration {
        self.superseded_lease
    }

    /// The realm poison cause, when the fence was lost.
    #[must_use]
    pub fn realm_poisoned(&self) -> Option<String> {
        lock(&self.realm_poison).clone()
    }

    /// Whether any scope is poisoned.
    #[must_use]
    pub fn has_poisoned_scopes(&self) -> bool {
        self.poisoned_scopes.load(Ordering::Acquire) > 0
    }

    /// Refuse when the realm is poisoned.
    pub fn check_realm(&self) -> Result<(), SpiError> {
        match self.realm_poisoned() {
            Some(cause) => Err(SpiError::RealmPoisoned(cause)),
            None => Ok(()),
        }
    }

    /// Refuse a cross-scope read while the realm or any scope is poisoned:
    /// a poisoned scope's memory may be ahead of the host.
    pub fn check_cross_scope_read(&self) -> Result<(), SpiError> {
        self.check_realm()?;
        if self.has_poisoned_scopes() {
            return Err(SpiError::RealmPoisoned(
                "a scope is poisoned; cross-scope reads refuse until the realm is reopened".into(),
            ));
        }
        Ok(())
    }

    /// Refuse a call on a poisoned scope (or realm).
    pub fn check_scope(&self, state: &ScopeState) -> Result<(), SpiError> {
        self.check_realm()?;
        match &state.poison {
            Some(cause) => Err(SpiError::ScopePoisoned {
                scope: state.id.to_string(),
                cause: cause.clone(),
            }),
            None => Ok(()),
        }
    }

    fn lease_started(&self) -> Instant {
        *lock(&self.lease_start)
    }

    /// A renew that started at `started` succeeded. It extends the window
    /// only while the writer has not stopped: a stopped writer resumes on a
    /// fresh acquire alone.
    fn renewed(&self, started: Instant) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let mut slot = lock(&self.lease_start);
        if started.duration_since(*slot) >= self.self_fence_window {
            // The window closed before this renew started: an acquire made
            // meanwhile may have missed the last renew. Stop instead.
            drop(slot);
            self.stop("the self-fence window closed before a renew");
            return;
        }
        if started > *slot {
            *slot = started;
        }
    }

    fn stop(&self, cause: &str) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            tracing::warn!(%cause, "host store writer stopped; it resumes only on a fresh acquire");
        }
    }

    /// Whether this writer may write: it has not stopped (no failed renew
    /// and no closed window since its last acquire), and its last successful
    /// acquire or renew started less than the self-fence window ago (the
    /// lease minus the host-declared margin,
    /// [`FenceTiming::self_fence_window`]). A local clock read only, no
    /// store I/O.
    #[must_use]
    pub fn lease_is_live(&self) -> bool {
        SelfFence::window(self).is_open()
    }

    /// Make sure this writer may write before a mutation: a no-op while its
    /// window is open, otherwise a fresh fenced acquire (a stopped
    /// holder re-acquires; it never resumes on a later renew).
    ///
    /// # Errors
    /// [`SpiError::LeaseLapsed`] when the acquire fails or times out
    /// (nothing is poisoned: the caller has not mutated anything yet), and
    /// [`SpiError::RealmPoisoned`] when another writer holds the realm or
    /// committed while this one was stopped.
    pub async fn ensure_lease(&self) -> Result<(), SpiError> {
        self.check_realm()?;
        if self.lease_is_live() {
            return Ok(());
        }
        self.stop("the self-fence window closed");
        self.shared_reacquire()?.await
    }

    /// The re-acquire every caller of a stopped writer waits on: the one in
    /// flight, or a new one when none is. It runs on a task of its own: a
    /// caller dropped after the host granted a token must not leave that
    /// token unrecorded, or the next renew would answer fenced out. The task
    /// clears the slot as it ends, so the next caller of a writer still
    /// stopped starts the next attempt.
    fn shared_reacquire(&self) -> Result<SharedReacquire, SpiError> {
        use futures::FutureExt as _;
        let mut slot = lock(&self.reacquire_in_flight);
        if let Some(attempt) = slot.as_ref() {
            return Ok(attempt.clone());
        }
        let Some(realm) = self.me.upgrade() else {
            return Err(SpiError::LeaseLapsed("the realm is closing".into()));
        };
        let task = tokio::spawn(async move {
            let _clear = ClearReacquireInFlight(Arc::clone(&realm));
            realm.reacquire().await
        });
        let attempt = async move {
            match task.await {
                Ok(outcome) => outcome,
                Err(error) => Err(SpiError::LeaseLapsed(format!(
                    "the re-acquire task failed: {error}"
                ))),
            }
        }
        .boxed()
        .shared();
        *slot = Some(attempt.clone());
        Ok(attempt)
    }

    /// Acquire the fence again after a stop, and resume only when no other
    /// writer committed in between: a fresh listing (which, after the
    /// acquire, shows every batch committed before it) must match every
    /// version this writer last saw or wrote.
    async fn reacquire(&self) -> Result<(), SpiError> {
        let _serial = self.renew_gate.lock().await;
        self.reacquire_serialized().await
    }

    /// [`Self::reacquire`] under the renew gate, which the caller holds.
    async fn reacquire_serialized(&self) -> Result<(), SpiError> {
        self.check_realm()?;
        if self.lease_is_live() {
            return Ok(());
        }
        // No batch with the old token may be in flight.
        let _no_commits = self.fence_gate.write().await;
        let previous = self.fence();
        let started = Instant::now();
        let acquired = tokio::time::timeout(
            self.config.renew_timeout,
            self.host.acquire_fence(AcquireMode::Exclusive),
        )
        .await;
        let grant = match acquired {
            Ok(Ok(grant)) => grant,
            Ok(Err(HostStoreError::Held { holder, .. })) if holder == previous => {
                // This writer's own lease is still recorded and nobody
                // acquired since: supersede it, with nothing to wait out.
                match tokio::time::timeout(
                    self.config.renew_timeout,
                    self.host.acquire_fence(AcquireMode::Takeover),
                )
                .await
                {
                    Ok(Ok(grant)) => grant,
                    Ok(Err(error)) => {
                        return Err(SpiError::LeaseLapsed(format!(
                            "the writer stopped and a re-acquire failed: {error}"
                        )));
                    }
                    Err(_) => {
                        return Err(SpiError::LeaseLapsed(
                            "the writer stopped and a re-acquire timed out".into(),
                        ));
                    }
                }
            }
            Ok(Err(HostStoreError::Held { holder, .. })) => {
                return Err(self.poison_realm(format!(
                    "fenced out while stopped: token {holder} holds the realm"
                )));
            }
            Ok(Err(error)) => {
                return Err(SpiError::LeaseLapsed(format!(
                    "the writer stopped and a re-acquire failed: {error}"
                )));
            }
            Err(_) => {
                return Err(SpiError::LeaseLapsed(
                    "the writer stopped and a re-acquire timed out".into(),
                ));
            }
        };
        let listed = self.list_versions().await?;
        if !self.listing_matches(&listed) {
            return Err(self.poison_realm(
                "another writer committed while this one was stopped; reopen the realm".into(),
            ));
        }
        self.fence.store(grant.token.get(), Ordering::Release);
        *lock(&self.known_heads) = listed;
        lock(&self.uncertain).clear();
        // The listing may have outlasted the window counted from the
        // acquire: renew once with the new token and count the window
        // from that renew's start.
        let renew_started = Instant::now();
        match tokio::time::timeout(self.renew_timeout, self.host.renew_fence(grant.token)).await {
            Ok(Ok(())) => {}
            Ok(Err(HostStoreError::FencedOut { current })) => {
                return Err(self.poison_realm(format!(
                    "fenced out right after a re-acquire: token {current} superseded token {}",
                    grant.token
                )));
            }
            Ok(Err(error)) => {
                return Err(SpiError::LeaseLapsed(format!(
                    "the renew after a re-acquire failed: {error}"
                )));
            }
            Err(_) => {
                return Err(SpiError::LeaseLapsed(
                    "the renew after a re-acquire timed out".into(),
                ));
            }
        }
        *lock(&self.lease_start) = renew_started.max(started);
        self.stopped.store(false, Ordering::Release);
        tracing::info!(token = %grant.token, "host store writer re-acquired the fence and resumes");
        Ok(())
    }

    /// Whether a fresh listing shows only batches this writer wrote: every
    /// scope at the version this writer last saw or wrote, or, for a scope a
    /// batch with an unknown outcome touched, at any version that batch
    /// leaves possible.
    fn listing_matches(&self, listed: &HashMap<(ScopeKind, String), ScopeVersion>) -> bool {
        let known = lock(&self.known_heads);
        let uncertain = lock(&self.uncertain);
        let keys: HashSet<&(ScopeKind, String)> = known
            .keys()
            .chain(listed.keys())
            .chain(uncertain.keys())
            .collect();
        keys.into_iter().all(|key| {
            let seen = listed.get(key).copied();
            match uncertain.get(key) {
                Some(possible) => possible.contains(&seen) || seen == known.get(key).copied(),
                None => seen == known.get(key).copied(),
            }
        })
    }

    /// The version of every visible scope, from one listing.
    async fn list_versions(&self) -> Result<HashMap<(ScopeKind, String), ScopeVersion>, SpiError> {
        let mut versions = HashMap::new();
        for kind in LISTED_KINDS {
            let mut after: Option<ScopeName> = None;
            loop {
                let page = self
                    .host
                    .list_heads(kind, after.as_ref(), LIST_PAGE_HEADS)
                    .await?;
                for scope in page.heads {
                    versions.insert((kind, scope.name.as_str().to_string()), scope.head.version);
                }
                match page.next {
                    Some(next) => after = Some(next),
                    None => break,
                }
            }
        }
        Ok(versions)
    }

    /// Record what a landed batch did to the scope versions this writer
    /// knows.
    fn record_landed(&self, batch: &CommitBatch) {
        let mut known = lock(&self.known_heads);
        let mut uncertain = lock(&self.uncertain);
        for update in batch.updates() {
            let key = (update.scope.kind, update.scope.name.as_str().to_string());
            uncertain.remove(&key);
            if update.delete {
                known.remove(&key);
            } else {
                known.insert(key, update.next);
            }
        }
    }

    /// Record a batch whose outcome stays unknown: each scope it touched
    /// may be at its previous or its next version. Its scopes are
    /// poisoned; this keeps a re-acquire from reading the batch as
    /// another writer's.
    fn record_unsettled(&self, batch: &CommitBatch) {
        let mut uncertain = lock(&self.uncertain);
        for update in batch.updates() {
            let key = (update.scope.kind, update.scope.name.as_str().to_string());
            let possible = uncertain.entry(key).or_default();
            let after = (!update.delete).then_some(update.next);
            for version in [update.previous, after] {
                if !possible.contains(&version) {
                    possible.push(version);
                }
            }
        }
    }

    /// Commit `batch` ([`Self::commit_settled`]) and record what it did to
    /// the scope versions this writer knows, landed or unknown.
    async fn commit_recorded(&self, batch: &CommitBatch) -> Result<(), SpiError> {
        match self.commit_settled(batch).await {
            Ok(()) => {
                self.record_landed(batch);
                Ok(())
            }
            Err(error) => {
                self.record_unsettled(batch);
                Err(error)
            }
        }
    }

    fn poison_realm(&self, cause: String) -> SpiError {
        let mut slot = lock(&self.realm_poison);
        let cause = slot.get_or_insert(cause).clone();
        tracing::error!(%cause, "host store realm poisoned; every call refuses until it is reopened");
        SpiError::RealmPoisoned(cause)
    }

    /// Poison one scope: it refuses every call until the realm reopens.
    /// For adapters whose memory may now differ from the host for it.
    pub fn poison(&self, state: &mut ScopeState, cause: &str) {
        self.poison_scope(state, cause);
    }

    /// Poison the whole realm (an adapter rule was broken and the affected
    /// scope could not be singled out).
    pub fn poison_all(&self, cause: String) -> SpiError {
        self.poison_realm(cause)
    }

    fn poison_scope(&self, state: &mut ScopeState, cause: &str) {
        if state.poison.is_none() {
            state.poison = Some(cause.to_string());
            self.poisoned_scopes.fetch_add(1, Ordering::AcqRel);
            tracing::error!(scope = %state.id, %cause, "host store scope poisoned; it refuses until the realm is reopened");
        }
    }

    /// Renew once per renew interval while the writer runs. A failed renew
    /// stops the writer at once; a stopped writer is renewed no more and
    /// tries a fresh acquire at each tick instead.
    fn spawn_renewal(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let interval = self.timing.renew_interval();
        tokio::spawn(async move {
            let mut ticks =
                tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticks.tick().await;
                let Some(realm) = weak.upgrade() else {
                    return;
                };
                if realm.realm_poisoned().is_some() {
                    return;
                }
                // Renews and re-acquires never overlap: a renew still in
                // flight with an old token cannot meet the new one.
                let _serial = realm.renew_gate.lock().await;
                if realm.stopped.load(Ordering::Acquire) || !realm.lease_is_live() {
                    realm.stop("the self-fence window closed");
                    if let Err(error) = realm.reacquire_serialized().await {
                        tracing::warn!(%error, "host store re-acquire failed; writes stay stopped");
                    }
                    continue;
                }
                let started = Instant::now();
                let token = realm.fence();
                match tokio::time::timeout(realm.renew_timeout, realm.host.renew_fence(token)).await
                {
                    Ok(Ok(())) => realm.renewed(started),
                    Ok(Err(HostStoreError::FencedOut { current }))
                        if current != token && current == realm.fence() =>
                    {
                        // A stale renew of this writer's own superseded
                        // token: the current token is ours.
                    }
                    Ok(Err(HostStoreError::FencedOut { current })) => {
                        realm.poison_realm(format!(
                            "fenced out: token {current} superseded token {token}"
                        ));
                        return;
                    }
                    Ok(Err(error)) => {
                        realm.stop(&format!("a renew failed: {error}"));
                    }
                    Err(_) => {
                        realm.stop("a renew timed out");
                    }
                }
            }
        });
    }

    fn next_token(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn commit_id(&self) -> Result<CommitId, SpiError> {
        CommitId::try_new(format!("{}-{}", self.writer, self.next_token())).map_err(SpiError::Host)
    }

    fn incarnation(&self) -> String {
        format!("{}.{}", self.writer, self.next_token())
    }

    async fn list_all(&self) -> Result<(), SpiError> {
        let mut listed: HashMap<String, Vec<ListedHead>> = HashMap::new();
        let mut chunks: HashMap<String, ChunkMeta> = HashMap::new();
        let mut known: HashMap<(ScopeKind, String), ScopeVersion> = HashMap::new();
        for kind in LISTED_KINDS {
            let mut after: Option<ScopeName> = None;
            loop {
                let page = self
                    .host
                    .list_heads(kind, after.as_ref(), LIST_PAGE_HEADS)
                    .await?;
                for scope in page.heads {
                    known.insert((kind, scope.name.as_str().to_string()), scope.head.version);
                    let Some(tag) = codec::name_tag(&scope.name).map(str::to_string) else {
                        // Not a name this journal writes: leave it alone.
                        continue;
                    };
                    if tag == CHUNK_TAG {
                        if kind != ScopeKind::Record {
                            return Err(SpiError::Corrupt(format!(
                                "chunk scope {} has kind {kind}",
                                scope.name
                            )));
                        }
                        let head = ChunkHeadV1::decode(&scope.head.head)?;
                        chunks.insert(
                            scope.name.as_str().to_string(),
                            ChunkMeta {
                                version: scope.head.version,
                                owner: head.owner,
                            },
                        );
                        continue;
                    }
                    let head = ScopeHeadV1::decode(&scope.head.head)?;
                    if head.fam != tag || !codec::name_encodes(&scope.name, &tag, &head.key) {
                        return Err(SpiError::Corrupt(format!(
                            "scope {} does not encode its head's family {} and key",
                            scope.name, head.fam
                        )));
                    }
                    listed.entry(tag).or_default().push(ListedHead {
                        name: scope.name,
                        version: scope.head.version,
                        head,
                    });
                }
                match page.next {
                    Some(next) => after = Some(next),
                    None => break,
                }
            }
        }
        *lock(&self.listed) = listed;
        *lock(&self.chunks) = chunks;
        *lock(&self.known_heads) = known;
        Ok(())
    }

    /// Delete chunks whose owner is not a listed scope: garbage of a crash
    /// between a chunk and the batch that would have referenced it, or of a
    /// deleted owner. Best effort; a failure leaves garbage, not corruption.
    async fn sweep_ownerless_chunks(&self) {
        let owners: HashSet<String> = lock(&self.listed)
            .values()
            .flatten()
            .map(|listed| listed.name.as_str().to_string())
            .collect();
        let doomed: Vec<(String, ScopeVersion)> = lock(&self.chunks)
            .iter()
            .filter(|(_, meta)| !owners.contains(&meta.owner))
            .map(|(name, meta)| (name.clone(), meta.version))
            .collect();
        for group in doomed.chunks(64) {
            if let Err(error) = self.delete_chunks(group).await {
                tracing::warn!(%error, "could not delete orphaned host store chunks");
                return;
            }
        }
    }

    async fn delete_chunks(&self, chunks: &[(String, ScopeVersion)]) -> Result<(), SpiError> {
        if chunks.is_empty() {
            return Ok(());
        }
        let mut updates = Vec::with_capacity(chunks.len());
        for (name, version) in chunks {
            updates.push(ScopeUpdate {
                scope: ScopeRef::new(
                    ScopeKind::Record,
                    ScopeName::try_new(name.clone()).map_err(SpiError::Host)?,
                ),
                previous: Some(*version),
                next: next_version(Some(*version))?,
                head: None,
                append: Vec::new(),
                truncate_after: None,
                drop_before: None,
                delete: true,
            });
        }
        let _commits = self.fence_gate.read().await;
        let batch = CommitBatch::try_new(self.commit_id()?, self.fence(), updates)?;
        self.commit_recorded(&batch).await?;
        let mut table = lock(&self.chunks);
        for (name, _) in chunks {
            table.remove(name);
        }
        Ok(())
    }

    /// Claim the listed scopes of `family`: register their cells (head
    /// only, bodies unloaded) and answer them in key order. A family is
    /// claimed once per realm: a later claim answers the family's registered
    /// cells, or the first claim's refusal again.
    ///
    /// # Errors
    /// [`SpiError::Corrupt`] when two live incarnations claim one key.
    pub fn claim(&self, family: ScopeFamily) -> Result<Vec<Arc<ScopeCell>>, SpiError> {
        let mut claimed = lock(&self.claimed);
        match claimed.get(family.tag) {
            Some(Ok(())) => return Ok(self.cells_of(family)),
            Some(Err(cause)) => return Err(SpiError::Corrupt(cause.clone())),
            None => {}
        }
        let outcome = self.claim_listed(family);
        claimed.insert(
            family.tag,
            outcome.as_ref().map(|_| ()).map_err(ToString::to_string),
        );
        outcome
    }

    fn cells_of(&self, family: ScopeFamily) -> Vec<Arc<ScopeCell>> {
        let cells = lock(&self.cells);
        let mut found: Vec<(&ScopeId, &Arc<ScopeCell>)> =
            cells.iter().filter(|(id, _)| id.family == family).collect();
        found.sort_by_key(|(id, _)| *id);
        found
            .into_iter()
            .map(|(_, cell)| Arc::clone(cell))
            .collect()
    }

    fn claim_listed(&self, family: ScopeFamily) -> Result<Vec<Arc<ScopeCell>>, SpiError> {
        let listed = lock(&self.listed).remove(family.tag).unwrap_or_default();
        let mut cells = lock(&self.cells);
        let mut claimed = BTreeMap::new();
        for entry in listed {
            if entry.head.fam != family.tag {
                return Err(SpiError::Corrupt(format!(
                    "scope {} names family {}",
                    entry.name, entry.head.fam
                )));
            }
            let id = ScopeId::new(family, entry.head.key.clone());
            if cells.contains_key(&id) || claimed.contains_key(&id) {
                return Err(SpiError::Corrupt(format!(
                    "two live incarnations of {id} exist"
                )));
            }
            let state = ScopeState::from_head(id.clone(), entry.name, entry.version, &entry.head)?;
            claimed.insert(id, Arc::new(Mutex::new(state)));
        }
        let mut out = Vec::with_capacity(claimed.len());
        for (id, cell) in claimed {
            cells.insert(id, Arc::clone(&cell));
            out.push(cell);
        }
        Ok(out)
    }

    /// The realm's one value of type `T`, created by `init` on first use:
    /// per-realm adapter state (such as the mob adapter's registry of open
    /// storages) without a process-wide registry. A failed `init` stores
    /// nothing. Hold the realm only weakly from `T`: the realm keeps `T`.
    pub async fn extension<T, E, F, Fut>(&self, init: F) -> Result<Arc<T>, E>
    where
        T: Any + Send + Sync,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let mut slots = self.extensions.lock().await;
        if let Some(existing) = slots.get(&TypeId::of::<T>())
            && let Ok(found) = Arc::clone(existing).downcast::<T>()
        {
            return Ok(found);
        }
        let value = Arc::new(init().await?);
        slots.insert(
            TypeId::of::<T>(),
            Arc::clone(&value) as Arc<dyn Any + Send + Sync>,
        );
        Ok(value)
    }

    /// The cell of a scope, created empty (no incarnation) when unknown.
    #[must_use]
    pub fn cell(&self, id: &ScopeId) -> Arc<ScopeCell> {
        let mut cells = lock(&self.cells);
        Arc::clone(
            cells
                .entry(id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(ScopeState::empty(id.clone())))),
        )
    }

    /// The ids of every registered scope of a family, in key order.
    #[must_use]
    pub fn ids_of(&self, family: ScopeFamily) -> Vec<ScopeId> {
        let mut ids: Vec<ScopeId> = lock(&self.cells)
            .keys()
            .filter(|id| id.family == family)
            .cloned()
            .collect();
        ids.sort();
        ids
    }

    /// Lock several scopes in [`ScopeId`] order (deadlock free), once each.
    pub async fn lock_scopes(&self, ids: &[ScopeId]) -> Vec<ScopeGuard> {
        let unique: BTreeSet<&ScopeId> = ids.iter().collect();
        let mut guards = Vec::with_capacity(unique.len());
        for id in unique {
            guards.push(self.cell(id).lock_owned().await);
        }
        guards
    }

    /// The pinned values of a scope, chunked ones read and verified.
    pub async fn pinned_values(
        &self,
        state: &ScopeState,
    ) -> Result<BTreeMap<String, Arc<Vec<u8>>>, SpiError> {
        let mut out = BTreeMap::new();
        for (key, pinned) in &state.pinned {
            let value = match pinned {
                Pinned::Inline(bytes) => Arc::clone(bytes),
                Pinned::Chunk(chunk) => Arc::new(self.read_chunk(chunk).await?),
            };
            out.insert(key.clone(), value);
        }
        Ok(out)
    }

    /// Fold the scope's rows into its row entries (a body loads when
    /// Core opens the scope). A chunked entry's value is read once the
    /// whole window is folded, and only for an entry that survives it: a
    /// commit that replaces or removes a chunked entry deletes the chunk in
    /// the same batch, while the superseded row stays in the window until a
    /// checkpoint drops it. Unreferenced chunks the scope owns are deleted
    /// afterwards, best effort. The scope counts as loaded only once the
    /// adapter has installed the answered values in its memory and called
    /// [`ScopeState::mark_loaded`], so a call dropped in between leaves it
    /// unloaded and the next call loads it again.
    pub async fn load_body(
        &self,
        state: &mut ScopeState,
    ) -> Result<BTreeMap<String, Arc<Vec<u8>>>, SpiError> {
        self.check_scope(state)?;
        let Some(name) = state.name.clone() else {
            state.body = Body::Loaded;
            return Ok(BTreeMap::new());
        };
        let scope = ScopeRef::new(state.id.family.kind, name.clone());
        let mut values: BTreeMap<String, Arc<Vec<u8>>> = BTreeMap::new();
        let mut stamps: BTreeMap<String, Stamp> = BTreeMap::new();
        let mut row_chunks = Vec::new();
        let mut index = state.window.s;
        while index < state.window.e {
            let want = usize::try_from(state.window.e - index)
                .unwrap_or(READ_PAGE_ROWS)
                .min(READ_PAGE_ROWS);
            let rows = self
                .host
                .read_rows(&scope, RowIndex::new(index), want)
                .await?;
            if rows.is_empty() {
                return Err(SpiError::Corrupt(format!(
                    "{scope} rows end at {index}, before its head's window end {}",
                    state.window.e
                )));
            }
            for row in &rows {
                for op in codec::decode_row(row.as_bytes())? {
                    match op {
                        RowOp::Spill(chunk) => {
                            let packed = self.read_chunk(&chunk).await?;
                            for inner in codec::unpack_rows(&packed)? {
                                for op in codec::decode_row(&inner)? {
                                    if matches!(op, RowOp::Spill(_)) {
                                        return Err(SpiError::Corrupt(format!(
                                            "{scope} nests a spill inside a spill"
                                        )));
                                    }
                                    Self::fold(op, &mut values, &mut stamps)?;
                                }
                            }
                            row_chunks.push((index, chunk));
                        }
                        op => Self::fold(op, &mut values, &mut stamps)?,
                    }
                }
                index += 1;
            }
        }
        for (key, stamp) in &mut stamps {
            if let Some(chunk) = &stamp.chunk {
                let value = Arc::new(self.read_chunk(chunk).await?);
                stamp.shared = Some(Arc::clone(&value));
                values.insert(key.clone(), value);
            }
        }
        state.rows = stamps;
        state.row_chunks = row_chunks;
        self.collect_owned_garbage(state).await;
        Ok(values)
    }

    /// Fold one row op into the entries. A chunked entry gets its stamp
    /// here; [`Self::load_body`] reads its value after the fold.
    fn fold(
        op: RowOp,
        values: &mut BTreeMap<String, Arc<Vec<u8>>>,
        stamps: &mut BTreeMap<String, Stamp>,
    ) -> Result<(), SpiError> {
        match op {
            RowOp::Delete(key) => {
                values.remove(&key);
                stamps.remove(&key);
            }
            RowOp::Put(key, value) => {
                let stamp = Stamp {
                    digest: codec::sha256(&value),
                    len: value.len() as u64,
                    chunk: None,
                    shared: (value.len() >= SHARED_STAMP_MIN).then(|| Arc::clone(&value)),
                    op_bytes: op_bytes(&key, value.len()),
                };
                stamps.insert(key.clone(), stamp);
                values.insert(key, value);
            }
            RowOp::PutChunk(key, chunk) => {
                let stamp = Stamp {
                    digest: chunk.sha256,
                    len: chunk.len,
                    op_bytes: op_bytes(&key, chunk.name.as_str().len() + 40),
                    chunk: Some(chunk),
                    shared: None,
                };
                values.remove(&key);
                stamps.insert(key, stamp);
            }
            RowOp::Reset => {
                values.clear();
                stamps.clear();
            }
            RowOp::Spill(_) => {
                return Err(SpiError::Corrupt("an unexpected spill".into()));
            }
        }
        Ok(())
    }

    /// Delete chunks this scope owns but no longer references (a crash
    /// between their write and the referencing batch). Best effort. The
    /// deletes run on a task of their own, which the caller awaits: a caller
    /// dropped meanwhile leaves no landed delete unrecorded.
    async fn collect_owned_garbage(&self, state: &ScopeState) {
        let Some(name) = state.name.as_ref() else {
            return;
        };
        let referenced: HashSet<String> = state
            .chunk_refs()
            .into_iter()
            .map(|chunk| chunk.name.as_str().to_string())
            .collect();
        let garbage: Vec<(String, ScopeVersion)> = lock(&self.chunks)
            .iter()
            .filter(|(chunk, meta)| meta.owner == name.as_str() && !referenced.contains(*chunk))
            .map(|(chunk, meta)| (chunk.clone(), meta.version))
            .collect();
        if garbage.is_empty() {
            return;
        }
        let Some(realm) = self.me.upgrade() else {
            return;
        };
        let scope = state.id.to_string();
        let deletes = tokio::spawn(async move {
            for group in garbage.chunks(64) {
                if let Err(error) = realm.delete_chunks(group).await {
                    tracing::warn!(%scope, %error, "could not delete unreferenced host store chunks");
                    return;
                }
            }
        });
        if let Err(error) = deletes.await {
            tracing::warn!(%error, "the host store chunk delete task failed");
        }
    }

    async fn read_chunk(&self, chunk: &ChunkRef) -> Result<Vec<u8>, SpiError> {
        let scope = ScopeRef::new(ScopeKind::Record, chunk.name.clone());
        let head = match self.host.load_head(&scope).await? {
            Some(head) => ChunkHeadV1::decode(&head.head)?,
            None => {
                return Err(SpiError::Corrupt(format!(
                    "chunk {} is referenced but missing",
                    chunk.name
                )));
            }
        };
        if !head.done || head.len != chunk.len || codec::unhex32(&head.sha)? != chunk.sha256 {
            return Err(SpiError::Corrupt(format!(
                "chunk {} is incomplete or differs from its reference",
                chunk.name
            )));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(head.len).unwrap_or(0));
        let mut index = 0_u64;
        while index < head.n {
            let want = usize::try_from(head.n - index)
                .unwrap_or(READ_PAGE_ROWS)
                .min(READ_PAGE_ROWS);
            let rows = self
                .host
                .read_rows(&scope, RowIndex::new(index), want)
                .await?;
            if rows.is_empty() {
                return Err(SpiError::Corrupt(format!(
                    "chunk {} ends before its {} rows",
                    chunk.name, head.n
                )));
            }
            for row in &rows {
                bytes.extend_from_slice(row.as_bytes());
            }
            index += rows.len() as u64;
        }
        if bytes.len() as u64 != chunk.len || codec::sha256(&bytes) != chunk.sha256 {
            return Err(SpiError::Corrupt(format!(
                "chunk {} does not match its digest",
                chunk.name
            )));
        }
        Ok(bytes)
    }

    /// Commit one call's changes to its locked scopes as ONE batch (plus the
    /// chunk batches that must land first), before the call answers.
    ///
    /// # Errors
    /// The memory side already changed, so every failure poisons what may
    /// now differ from the host: a failed or unknown batch poisons the
    /// scopes it would have changed (not the ones only locked to read), a
    /// failure before any batch was sent poisons every scope whose delta
    /// names an entry, and a fence loss poisons the realm.
    pub async fn commit(
        &self,
        scopes: &mut [&mut ScopeState],
        deltas: Vec<ScopeDelta>,
    ) -> Result<(), SpiError> {
        if scopes.len() != deltas.len() {
            return Err(SpiError::Misuse(
                "a commit pairs every locked scope with one delta".into(),
            ));
        }
        let changed: Vec<usize> = deltas
            .iter()
            .enumerate()
            .filter(|(_, delta)| !delta.is_empty())
            .map(|(index, _)| index)
            .collect();
        let mut plans = match self.plan_all(scopes, deltas) {
            Ok(plans) => plans,
            Err(error) => {
                // Memory already changed and no batch was sent: every scope
                // with a change may now differ from the host.
                let cause = error.to_string();
                for index in changed {
                    self.poison_scope(&mut *scopes[index], &cause);
                }
                return Err(error);
            }
        };
        if plans.is_empty() {
            return Ok(());
        }
        // The memory side already changed: from here on, a failure poisons.
        let outcome = self.write_plans(&mut plans).await;
        match outcome {
            Ok(()) => {
                for plan in plans {
                    let state = &mut *scopes[plan.index];
                    self.adopt(state, plan);
                }
                Ok(())
            }
            Err(error) => {
                let cause = error.to_string();
                for plan in &plans {
                    self.poison_scope(&mut *scopes[plan.index], &cause);
                }
                if matches!(error, SpiError::Host(HostStoreError::FencedOut { .. })) {
                    return Err(self.poison_realm(cause));
                }
                Err(error)
            }
        }
    }

    fn plan_all(
        &self,
        scopes: &[&mut ScopeState],
        deltas: Vec<ScopeDelta>,
    ) -> Result<Vec<ScopePlan>, SpiError> {
        self.check_realm()?;
        for state in scopes {
            self.check_scope(state)?;
            if state.body != Body::Loaded {
                return Err(SpiError::Misuse(format!(
                    "{} is committed before its body was loaded",
                    state.id
                )));
            }
        }
        let mut plans = Vec::new();
        for (index, (state, delta)) in scopes.iter().zip(deltas).enumerate() {
            if let Some(plan) = self.plan(index, state, delta)? {
                plans.push(plan);
            }
        }
        Ok(plans)
    }

    async fn write_plans(&self, plans: &mut [ScopePlan]) -> Result<(), SpiError> {
        // Self-fence again: the call may have outlived the window.
        self.ensure_lease().await?;
        // Every batch of this call carries one token.
        let _commits = self.fence_gate.read().await;
        self.fit_batch(plans)?;
        for plan in &*plans {
            for chunk in &plan.new_chunks {
                self.write_chunk(chunk).await?;
            }
        }
        let mut updates = Vec::new();
        let mut dead: BTreeMap<String, ScopeVersion> = BTreeMap::new();
        {
            let table = lock(&self.chunks);
            for plan in &*plans {
                for chunk in &plan.dead_chunks {
                    if let Some(meta) = table.get(chunk.name.as_str()) {
                        dead.insert(chunk.name.as_str().to_string(), meta.version);
                    }
                }
            }
        }
        for plan in &*plans {
            updates.push(if plan.delete {
                ScopeUpdate {
                    scope: plan.scope.clone(),
                    previous: plan.previous,
                    next: plan.next,
                    head: None,
                    append: Vec::new(),
                    truncate_after: None,
                    drop_before: None,
                    delete: true,
                }
            } else {
                ScopeUpdate {
                    scope: plan.scope.clone(),
                    previous: plan.previous,
                    next: plan.next,
                    head: Some(plan_head_of(plan)?),
                    append: plan.rows.clone(),
                    truncate_after: None,
                    drop_before: plan
                        .checkpoint
                        .then(|| RowIndex::new(plan_window_of(plan).s)),
                    delete: false,
                }
            });
        }
        for (name, version) in &dead {
            updates.push(ScopeUpdate {
                scope: ScopeRef::new(
                    ScopeKind::Record,
                    ScopeName::try_new(name.clone()).map_err(SpiError::Host)?,
                ),
                previous: Some(*version),
                next: next_version(Some(*version))?,
                head: None,
                append: Vec::new(),
                truncate_after: None,
                drop_before: None,
                delete: true,
            });
        }
        let bytes = updates
            .iter()
            .map(ScopeUpdate::payload_bytes)
            .fold(0, usize::saturating_add);
        if bytes > MAX_BATCH_BYTES {
            let heads = updates
                .iter()
                .map(|update| update.head.as_ref().map_or(0, Vec::len))
                .fold(0, usize::saturating_add);
            return Err(SpiError::TooLarge {
                head_bytes: heads,
                row_bytes: bytes.saturating_sub(heads),
                budget: MAX_BATCH_BYTES,
            });
        }
        let batch = CommitBatch::try_new(self.commit_id()?, self.fence(), updates)?;
        self.commit_recorded(&batch).await?;
        let mut table = lock(&self.chunks);
        for name in dead.keys() {
            table.remove(name);
        }
        Ok(())
    }

    /// Commit `batch`, settling an unknown or unavailable outcome by
    /// retrying the same batch (same commit id) and by reading the first
    /// scope's head, within the retry budget.
    async fn commit_settled(&self, batch: &CommitBatch) -> Result<(), SpiError> {
        let deadline = Instant::now() + self.config.commit_retry_budget;
        let mut attempt: u32 = 0;
        loop {
            let error = match self.host.commit(batch).await {
                Ok(()) => return Ok(()),
                Err(HostStoreError::Indeterminate(detail)) => {
                    if self.landed(batch).await {
                        return Ok(());
                    }
                    HostStoreError::Indeterminate(detail)
                }
                Err(HostStoreError::Unavailable(detail)) => HostStoreError::Unavailable(detail),
                Err(other) => return Err(SpiError::Host(other)),
            };
            attempt = attempt.saturating_add(1);
            let backoff = Duration::from_millis(50_u64.saturating_mul(1 << attempt.min(6)));
            if Instant::now() + backoff >= deadline {
                return Err(SpiError::Host(error));
            }
            tokio::time::sleep(backoff).await;
        }
    }

    /// Whether `batch` landed, read from its first update's scope: the head
    /// names the batch's commit id, or a deleted scope is gone.
    async fn landed(&self, batch: &CommitBatch) -> bool {
        let Some(update) = batch.updates().first() else {
            return false;
        };
        match self.host.load_head(&update.scope).await {
            Ok(Some(head)) => !update.delete && head.commit_id == *batch.commit_id(),
            Ok(None) => update.delete,
            Err(_) => false,
        }
    }

    async fn write_chunk(&self, pending: &PendingChunk) -> Result<(), SpiError> {
        let scope = ScopeRef::new(ScopeKind::Record, pending.chunk.name.clone());
        let pieces: Vec<&[u8]> = if pending.bytes.is_empty() {
            Vec::new()
        } else {
            pending.bytes.chunks(MAX_ROW_BYTES).collect()
        };
        let total = pieces.len() as u64;
        let groups: Vec<&[&[u8]]> = if pieces.is_empty() {
            vec![&[][..]]
        } else {
            pieces.chunks(CHUNK_ROWS_PER_BATCH).collect()
        };
        let last_group = groups.len().saturating_sub(1);
        let mut previous: Option<ScopeVersion> = None;
        let mut head = ChunkHeadV1 {
            f: FORMAT_V1,
            owner: pending.owner.as_str().to_string(),
            len: pending.chunk.len,
            sha: codec::hex(&pending.chunk.sha256),
            n: total,
            done: false,
        };
        for (index, group) in groups.into_iter().enumerate() {
            head.done = index == last_group;
            let mut append = Vec::with_capacity(group.len());
            for piece in group {
                append.push(Row::try_new(piece.to_vec())?);
            }
            let next = next_version(previous)?;
            let update = ScopeUpdate {
                scope: scope.clone(),
                previous,
                next,
                head: Some(head.encode()?),
                append,
                truncate_after: None,
                drop_before: None,
                delete: false,
            };
            let batch = CommitBatch::try_new(self.commit_id()?, self.fence(), vec![update])?;
            self.commit_recorded(&batch).await?;
            previous = Some(next);
            lock(&self.chunks).insert(
                pending.chunk.name.as_str().to_string(),
                ChunkMeta {
                    version: next,
                    owner: pending.owner.as_str().to_string(),
                },
            );
        }
        Ok(())
    }

    fn new_chunk(&self, owner: &ScopeName, value: &Arc<Vec<u8>>) -> Result<PendingChunk, SpiError> {
        let name = codec::scope_name(CHUNK_TAG, owner.as_str(), &self.incarnation())?;
        Ok(PendingChunk {
            chunk: ChunkRef {
                name,
                len: value.len() as u64,
                sha256: codec::sha256(value),
            },
            owner: owner.clone(),
            bytes: Arc::clone(value),
        })
    }

    /// Plan one scope's update, or `None` when nothing changes.
    fn plan(
        &self,
        index: usize,
        state: &ScopeState,
        delta: ScopeDelta,
    ) -> Result<Option<ScopePlan>, SpiError> {
        let family = state.id.family;
        // Resolve the delta into explicit changes, and whether it is a full
        // image eligible for a checkpoint.
        let (changes, full) = match delta {
            ScopeDelta::Changes(changes) => (changes, None),
            ScopeDelta::Full(image) => {
                let mut changes: Vec<(String, Option<Arc<Vec<u8>>>)> = state
                    .pinned
                    .keys()
                    .chain(state.rows.keys())
                    .filter(|key| !image.contains_key(*key))
                    .map(|key| (key.clone(), None))
                    .collect();
                changes.extend(
                    image
                        .iter()
                        .map(|(key, value)| (key.clone(), Some(Arc::clone(value)))),
                );
                (changes, Some(image))
            }
        };
        let mut pinned_after = state.pinned.clone();
        let mut row_changes: Vec<(String, Option<Arc<Vec<u8>>>)> = Vec::new();
        let mut changed_pins = false;
        for (key, value) in changes {
            if family.is_pinned(&key) {
                match value {
                    None => {
                        if pinned_after.remove(&key).is_some() {
                            changed_pins = true;
                        }
                    }
                    Some(value) => {
                        let same = match state.pinned.get(&key) {
                            Some(Pinned::Inline(old)) => old.as_slice() == value.as_slice(),
                            Some(Pinned::Chunk(chunk)) => {
                                chunk.len == value.len() as u64
                                    && chunk.sha256 == codec::sha256(&value)
                            }
                            None => false,
                        };
                        if !same {
                            pinned_after.insert(key, Pinned::Inline(value));
                            changed_pins = true;
                        }
                    }
                }
            } else {
                match (&value, state.rows.get(&key)) {
                    (None, None) => {}
                    (Some(value), Some(stamp)) if stamp.matches(value) => {}
                    _ => row_changes.push((key, value)),
                }
            }
        }
        let rows_after_empty = {
            let removed: BTreeSet<&String> = row_changes
                .iter()
                .filter(|(_, value)| value.is_none())
                .map(|(key, _)| key)
                .collect();
            let added = row_changes.iter().any(|(_, value)| value.is_some());
            !added && state.rows.keys().all(|key| removed.contains(key))
        };
        let delete = rows_after_empty && pinned_after.is_empty();
        let checkpoint = full.is_some() && state.wants_full_image() && !delete;
        if !changed_pins && row_changes.is_empty() && !checkpoint {
            return Ok(None);
        }
        if delete && state.name.is_none() {
            // Nothing exists and nothing remains.
            return Ok(None);
        }
        let name = match &state.name {
            Some(name) => name.clone(),
            None => codec::scope_name(family.tag, &state.id.key, &self.incarnation())?,
        };
        let scope = ScopeRef::new(family.kind, name.clone());
        let previous = state.version;
        let next = next_version(previous)?;
        if delete {
            return Ok(Some(ScopePlan {
                index,
                family_tag: family.tag,
                key: state.id.key.clone(),
                old_window: state.window,
                old_since: state.since_checkpoint,
                scope,
                previous,
                next,
                delete: true,
                pinned_after: BTreeMap::new(),
                rows: Vec::new(),
                checkpoint: false,
                row_changes: Vec::new(),
                live_after: 0,
                new_chunks: Vec::new(),
                dead_chunks: state.chunk_refs(),
                spill: None,
            }));
        }
        let mut new_chunks = Vec::new();
        let mut dead_chunks = Vec::new();
        // Pinned values: large ones move to chunks; replaced chunks die.
        for pinned in pinned_after.values_mut() {
            if let Pinned::Inline(value) = pinned
                && value.len() > PINNED_INLINE_LIMIT
            {
                let pending = self.new_chunk(&name, value)?;
                *pinned = Pinned::Chunk(pending.chunk.clone());
                new_chunks.push(pending);
            }
        }
        for (key, old) in &state.pinned {
            if let Pinned::Chunk(chunk) = old {
                let kept =
                    matches!(pinned_after.get(key), Some(Pinned::Chunk(now)) if now == chunk);
                if !kept {
                    dead_chunks.push(chunk.clone());
                }
            }
        }
        // Keep the head bounded: move the largest inline pins to chunks.
        loop {
            let size = state.head(&pinned_after, state.window, 0, 0)?.len();
            if size <= HEAD_LIMIT {
                break;
            }
            let largest = pinned_after
                .iter()
                .filter_map(|(key, pinned)| match pinned {
                    Pinned::Inline(value) => Some((key.clone(), value.len())),
                    Pinned::Chunk(_) => None,
                })
                .max_by_key(|(_, len)| *len);
            let Some((key, _)) = largest else {
                return Err(SpiError::Misuse(format!(
                    "{} has a head of {size} bytes even with every pinned value chunked",
                    state.id
                )));
            };
            if let Some(Pinned::Inline(value)) = pinned_after.get(&key).cloned() {
                let pending = self.new_chunk(&name, &value)?;
                pinned_after.insert(key, Pinned::Chunk(pending.chunk.clone()));
                new_chunks.push(pending);
            }
        }
        // Row entries.
        let mut stamps_after: Vec<(String, Option<Stamp>)> = Vec::new();
        let mut ops: Vec<RowOp> = Vec::new();
        let mut live_after = state.live_bytes;
        let checkpoint_image: Option<BTreeMap<String, Arc<Vec<u8>>>> = if checkpoint {
            full.map(|image| {
                image
                    .into_iter()
                    .filter(|(key, _)| !family.is_pinned(key))
                    .collect()
            })
        } else {
            None
        };
        let stamp_for = |key: &str, value: &Arc<Vec<u8>>, chunk: Option<ChunkRef>| -> Stamp {
            let op_len = match &chunk {
                Some(chunk) => op_bytes(key, chunk.name.as_str().len() + 40),
                None => op_bytes(key, value.len()),
            };
            Stamp {
                digest: chunk
                    .as_ref()
                    .map_or_else(|| codec::sha256(value), |chunk| chunk.sha256),
                len: value.len() as u64,
                chunk,
                shared: (value.len() >= SHARED_STAMP_MIN).then(|| Arc::clone(value)),
                op_bytes: op_len,
            }
        };
        // A large value equal to a chunk this plan retires takes that chunk
        // over instead of uploading the bytes again: a boundary
        // promotion moves the provisional tail's body to the session entry
        // unchanged in one commit.
        let mut chunk_for = |value: &Arc<Vec<u8>>,
                             retiring: &mut HashMap<([u8; 32], u64), Vec<ChunkRef>>|
         -> Result<ChunkRef, SpiError> {
            let digest = codec::sha256(value);
            if let Some(chunk) = retiring
                .get_mut(&(digest, value.len() as u64))
                .and_then(Vec::pop)
            {
                return Ok(chunk);
            }
            let pending = self.new_chunk(&name, value)?;
            let chunk = pending.chunk.clone();
            new_chunks.push(pending);
            Ok(chunk)
        };
        let mut retiring: HashMap<([u8; 32], u64), Vec<ChunkRef>> = HashMap::new();
        if let Some(image) = checkpoint_image {
            // A checkpoint: RESET, then every live row entry. Unchanged
            // chunked values keep their chunk; every earlier spill dies.
            ops.push(RowOp::Reset);
            live_after = 0;
            let kept: HashMap<&String, ChunkRef> = image
                .iter()
                .filter_map(|(key, value)| {
                    state
                        .rows
                        .get(key)
                        .filter(|stamp| stamp.chunk.is_some() && stamp.matches(value))
                        .and_then(|stamp| stamp.chunk.clone())
                        .map(|chunk| (key, chunk))
                })
                .collect();
            let kept_names: HashSet<&str> =
                kept.values().map(|chunk| chunk.name.as_str()).collect();
            for stamp in state.rows.values() {
                if let Some(chunk) = &stamp.chunk
                    && !kept_names.contains(chunk.name.as_str())
                {
                    retiring
                        .entry((chunk.sha256, chunk.len))
                        .or_default()
                        .push(chunk.clone());
                }
            }
            for (key, value) in &image {
                let stamp = if let Some(chunk) = kept.get(key) {
                    ops.push(RowOp::PutChunk(key.clone(), chunk.clone()));
                    stamp_for(key, value, Some(chunk.clone()))
                } else if value.len() > INLINE_VALUE_LIMIT {
                    let chunk = chunk_for(value, &mut retiring)?;
                    ops.push(RowOp::PutChunk(key.clone(), chunk.clone()));
                    stamp_for(key, value, Some(chunk))
                } else {
                    ops.push(RowOp::Put(key.clone(), Arc::clone(value)));
                    stamp_for(key, value, None)
                };
                live_after += stamp.op_bytes;
                stamps_after.push((key.clone(), Some(stamp)));
            }
            dead_chunks.extend(state.row_chunks.iter().map(|(_, chunk)| chunk.clone()));
        } else {
            for (key, _) in &row_changes {
                if let Some(chunk) = state.rows.get(key).and_then(|stamp| stamp.chunk.as_ref()) {
                    retiring
                        .entry((chunk.sha256, chunk.len))
                        .or_default()
                        .push(chunk.clone());
                }
            }
            for (key, value) in row_changes {
                if let Some(old) = state.rows.get(&key) {
                    live_after = live_after.saturating_sub(old.op_bytes);
                }
                match value {
                    None => {
                        ops.push(RowOp::Delete(key.clone()));
                        stamps_after.push((key, None));
                    }
                    Some(value) => {
                        let stamp = if value.len() > INLINE_VALUE_LIMIT {
                            let chunk = chunk_for(&value, &mut retiring)?;
                            ops.push(RowOp::PutChunk(key.clone(), chunk.clone()));
                            stamp_for(&key, &value, Some(chunk))
                        } else {
                            ops.push(RowOp::Put(key.clone(), Arc::clone(&value)));
                            stamp_for(&key, &value, None)
                        };
                        live_after += stamp.op_bytes;
                        stamps_after.push((key, Some(stamp)));
                    }
                }
            }
        }
        dead_chunks.extend(retiring.into_values().flatten());
        let rows = codec::encode_rows(&ops)?;
        Ok(Some(ScopePlan {
            index,
            family_tag: family.tag,
            key: state.id.key.clone(),
            old_window: state.window,
            old_since: state.since_checkpoint,
            scope,
            previous,
            next,
            delete: false,
            pinned_after,
            rows,
            checkpoint,
            row_changes: stamps_after,
            live_after,
            new_chunks,
            dead_chunks,
            spill: None,
        }))
    }

    /// Keep the main batch within [`BATCH_BUDGET`]: spill the largest
    /// scopes' rows into chunks, each replaced by one spill row.
    fn fit_batch(&self, plans: &mut [ScopePlan]) -> Result<(), SpiError> {
        loop {
            let heads: usize = plans
                .iter()
                .filter(|plan| !plan.delete)
                .map(|plan| plan_head_of(plan).map_or(HEAD_LIMIT, |head| head.len()))
                .sum();
            let rows: usize = plans.iter().map(ScopePlan::row_bytes).sum();
            if heads + rows <= BATCH_BUDGET {
                return Ok(());
            }
            let Some(largest) = plans
                .iter_mut()
                .filter(|plan| plan.spill.is_none() && plan.row_bytes() > MAX_ROW_BYTES / 4)
                .max_by_key(|plan| plan.row_bytes())
            else {
                return Err(SpiError::TooLarge {
                    head_bytes: heads,
                    row_bytes: rows,
                    budget: BATCH_BUDGET,
                });
            };
            let packed = Arc::new(codec::pack_rows(&largest.rows));
            let owner = largest.scope.name.clone();
            let pending = self.new_chunk(&owner, &packed)?;
            let spill = pending.chunk.clone();
            largest.rows = codec::encode_rows(&[RowOp::Spill(spill.clone())])?;
            largest.new_chunks.push(pending);
            largest.spill = Some(spill);
        }
    }

    fn adopt(&self, state: &mut ScopeState, plan: ScopePlan) {
        if plan.delete {
            let id = state.id.clone();
            *state = ScopeState::empty(id);
            return;
        }
        let window = plan_window_of(&plan);
        state.name = Some(plan.scope.name.clone());
        state.version = Some(plan.next);
        state.pinned = plan.pinned_after;
        if plan.checkpoint {
            state.rows.clear();
            state.row_chunks.clear();
            state.since_checkpoint = plan.rows.iter().map(|row| row.len() as u64).sum();
        } else {
            state.since_checkpoint += plan.rows.iter().map(|row| row.len() as u64).sum::<u64>();
        }
        for (key, stamp) in plan.row_changes {
            match stamp {
                Some(stamp) => {
                    state.rows.insert(key, stamp);
                }
                None => {
                    state.rows.remove(&key);
                }
            }
        }
        if let Some(spill) = plan.spill {
            state.row_chunks.push((window.e.saturating_sub(1), spill));
        }
        state.live_bytes = plan.live_after;
        state.window = window;
        state.body = Body::Loaded;
    }
}

/// The locked scopes of one adapter call that changes memory.
///
/// A call mutates memory first and commits to the host after; its future can
/// be dropped at any await in between (a cancelled task, an aborted drain).
/// Armed, this guard poisons every scope it holds when it is dropped, since
/// memory may then hold a change the host never got. The commit runs on a
/// task that owns the scopes ([`ArmedScopes::commit_detached`]), so a caller
/// dropped after the hand-off still lands its batch and adopts it, or
/// poisons, exactly as if it had waited. An adapter gate held with the
/// scopes ([`ArmedScopes::hold`]) stays held until the commit settles.
pub struct ArmedScopes {
    realm: Arc<SpiRealm>,
    guards: Vec<ScopeGuard>,
    armed: bool,
    gate: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
}

impl std::fmt::Debug for ArmedScopes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArmedScopes")
            .field("scopes", &self.guards.len())
            .field("armed", &self.armed)
            .field("gate", &self.gate.is_some())
            .finish()
    }
}

impl ArmedScopes {
    /// Wrap locked scopes, disarmed.
    #[must_use]
    pub fn new(realm: Arc<SpiRealm>, guards: Vec<ScopeGuard>) -> Self {
        Self {
            realm,
            guards,
            armed: false,
            gate: None,
        }
    }

    /// Keep an adapter's gate held for as long as these scopes are, so a
    /// commit handed off by a dropped caller still runs under it.
    #[must_use]
    pub fn hold(mut self, gate: tokio::sync::OwnedRwLockReadGuard<()>) -> Self {
        self.gate = Some(gate);
        self
    }

    /// From now on a drop poisons every held scope: call it right before the
    /// in-memory change starts.
    pub fn arm(&mut self) {
        self.armed = true;
    }

    /// The change settled (nothing changed, or its commit adopted or
    /// poisoned): a drop no longer poisons.
    pub fn disarm(&mut self) {
        self.armed = false;
    }

    #[must_use]
    pub fn guards(&self) -> &[ScopeGuard] {
        &self.guards
    }

    pub fn guards_mut(&mut self) -> &mut [ScopeGuard] {
        &mut self.guards
    }

    /// Commit `deltas` (each paired with the index of its scope in
    /// [`Self::guards`]) as one batch ([`SpiRealm::commit`], which poisons
    /// what a failed batch may have changed), then disarm.
    ///
    /// # Errors
    /// The commit's failure. A delta naming a scope these do not hold is
    /// [`SpiError::Misuse`] and leaves the scopes armed, so their drop
    /// poisons them all.
    pub async fn commit_in_place(
        &mut self,
        deltas: Vec<(usize, ScopeDelta)>,
    ) -> Result<(), SpiError> {
        let mut by_index: BTreeMap<usize, ScopeDelta> = deltas.into_iter().collect();
        let mut states: Vec<&mut ScopeState> = Vec::with_capacity(by_index.len());
        let mut ordered = Vec::with_capacity(by_index.len());
        for (index, guard) in self.guards.iter_mut().enumerate() {
            if let Some(delta) = by_index.remove(&index) {
                states.push(&mut **guard);
                ordered.push(delta);
            }
        }
        if !by_index.is_empty() {
            return Err(SpiError::Misuse(
                "a commit names a scope it does not hold".to_string(),
            ));
        }
        let outcome = self.realm.commit(&mut states, ordered).await;
        self.armed = false;
        outcome
    }

    /// [`Self::commit_in_place`] on a task that owns these scopes (and the
    /// held gate) until the commit settles; answers them back once it
    /// landed.
    ///
    /// # Errors
    /// The commit's failure; the scopes are released after the realm
    /// poisoned what the batch may have changed.
    pub async fn commit_detached(self, deltas: Vec<(usize, ScopeDelta)>) -> Result<Self, SpiError> {
        let task = tokio::spawn(async move {
            let mut scopes = self;
            let outcome = scopes.commit_in_place(deltas).await;
            (scopes, outcome)
        });
        match task.await {
            Ok((scopes, Ok(()))) => Ok(scopes),
            Ok((scopes, Err(error))) => {
                drop(scopes);
                Err(error)
            }
            // The task panicked or its runtime shut down: it dropped its
            // armed scopes, which poisoned them.
            Err(error) => Err(SpiError::Misuse(format!(
                "the host commit task failed: {error}"
            ))),
        }
    }
}

impl Drop for ArmedScopes {
    fn drop(&mut self) {
        if self.armed {
            let cause = "a call was dropped while it changed this scope; memory may hold a change the host never got";
            for guard in &mut self.guards {
                self.realm.poison(guard, cause);
            }
        }
    }
}

/// The scope's window after the plan: rows append at the old end; a
/// checkpoint drops every row before its first one.
fn plan_window_of(plan: &ScopePlan) -> WindowV1 {
    let old = plan.old_window;
    let appended = plan.rows.len() as u64;
    if plan.checkpoint {
        WindowV1 {
            s: old.e,
            e: old.e + appended,
        }
    } else {
        WindowV1 {
            s: old.s,
            e: old.e + appended,
        }
    }
}

fn plan_head_of(plan: &ScopePlan) -> Result<Vec<u8>, SpiError> {
    let window = plan_window_of(plan);
    let since = if plan.checkpoint {
        plan.rows.iter().map(|row| row.len() as u64).sum()
    } else {
        plan.old_since + plan.rows.iter().map(|row| row.len() as u64).sum::<u64>()
    };
    let pin = plan
        .pinned_after
        .iter()
        .map(|(key, value)| {
            let wire = match value {
                Pinned::Inline(bytes) => codec::pin_inline(bytes),
                Pinned::Chunk(chunk) => PinnedV1::Chunk(chunk.to_wire()),
            };
            (key.clone(), wire)
        })
        .collect();
    ScopeHeadV1 {
        f: FORMAT_V1,
        fam: plan.family_tag.to_string(),
        key: plan.key.clone(),
        pin,
        w: window,
        lb: plan.live_after,
        sb: since,
    }
    .encode()
}
