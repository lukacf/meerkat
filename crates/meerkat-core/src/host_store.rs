//! The host store SPI: a small storage interface a host implements so that
//! Core can keep a realm's durable state in the host's own database.
//!
//! Core stays the only writer of a realm. It builds its own sealed stores
//! over this interface: it assigns every scope version and commit id, keeps
//! the heads it has open in memory, and loads a scope only when it opens it
//! (at start, on resume and on failover), never before a commit. A host
//! store therefore keeps opaque bytes and enforces the obligations below; it
//! never interprets a payload.
//!
//! # Scopes
//!
//! The store holds **scopes**. Each scope has a [`ScopeKind`], a
//! [`ScopeName`] unique among the scopes of that kind in the realm, a
//! versioned head (opaque bytes) and an ordered list of rows (opaque bytes,
//! each at most [`MAX_ROW_BYTES`]). Every committed batch moves each scope it
//! touches from one version to the next, and may write several scopes at
//! once: rebinding an identity or spawning a member touches more than one.
//!
//! Rows have stable indices. A scope's visible rows are the indices from its
//! first visible row up to its end: appended rows take the next indices,
//! [`ScopeUpdate::truncate_after`] removes rows at the end, and
//! [`ScopeUpdate::drop_before`] removes rows at the start (log pruning)
//! without renumbering the rest.
//!
//! # Obligations
//!
//! These are the whole contract. Each lists the ways a store may meet it,
//! including the ways an append only store (one without updates or
//! transactions, such as BigQuery) meets it.
//!
//! 1. **All or nothing.** A load shows a batch completely, in every scope it
//!    touches, or not at all. Valid ways: a transaction; rows tagged with the
//!    commit id plus a commit marker row, where loads ignore rows that have
//!    no marker; a pending stream batch commit.
//! 2. **Durable once acknowledged.** After [`HostStore::commit`] answers
//!    `Ok`, the batch survives a writer crash, and every later load shows it,
//!    from this writer or any later one, unless obligation 5 hides it.
//! 3. **Idempotent retry.** A commit id sent twice acts once. A store may
//!    refuse the duplicate or collapse it at load; answering `Ok` to a retry
//!    of a batch that already landed is the usual way. Core never reuses a
//!    commit id for different content, and never commits a scope's next
//!    version while an earlier commit touching that scope has an unknown
//!    outcome: it settles the unknown first, by retrying the same batch or
//!    by loading the head.
//! 4. **One version chain per scope.** Core never sends two batches for one
//!    version of a scope, so no atomic precheck is required. If two exist
//!    anyway (a fenced out writer, a bug), every load must resolve them the
//!    same way. Valid ways: refuse the second batch at commit
//!    ([`HostStoreError::Conflict`]); fail the load with a typed conflict;
//!    pick one by a fixed rule, such as the lowest commit id, decided per
//!    batch, so that a batch that loses in one scope is invisible in all of
//!    them. Whatever the rule, a batch carrying a lower fence token never
//!    beats one carrying a higher token at the same version.
//! 5. **Fencing.** After a token is acquired, a batch carrying a lower token
//!    whose commit point comes later never becomes visible, and every batch
//!    whose commit point comes before the acquire stays visible, whether or
//!    not its writer saw the acknowledgement: a load after the acquire shows
//!    it. Valid ways: refuse the batch at commit
//!    ([`HostStoreError::FencedOut`]); at load, ignore every batch whose
//!    token is below the highest token acquired before its commit point.
//!    The commit point is the store's own server assigned commit order (for
//!    example its marker's commit time), never a writer's clock; commit
//!    points that tie are ordered by a fixed rule, such as the commit id.
//! 6. **Logical removal.** Truncate, drop and delete take effect for loads
//!    together with their batch. Physical cleanup may come later or never;
//!    tombstones are fine.
//! 7. **Order.** Rows load in commit order.
//! 8. **Independence.** No realm wide lock, pointer, sequence or
//!    transaction on the commit path, and scopes commit concurrently. The
//!    fence is acquired and renewed off the hot path; commits only carry the
//!    token.
//! 9. **Listing.** A scope is listed once its creating batch is visible, and
//!    not after its deletion.
//!
//! **Limits:** a batch carries at most [`MAX_BATCH_BYTES`] of heads and
//! rows, and a row at most [`MAX_ROW_BYTES`]. Core splits larger content
//! into chunks it keeps in the store itself, as rows or record scopes, and
//! commits them in batches before the batch that references them; an
//! orphaned chunk is garbage, not corruption.
//!
//! A store meets obligations 4 and 5 either at commit, answering
//! [`HostStoreError::Conflict`] or [`HostStoreError::FencedOut`], or at
//! load, by the fixed rules above. Core treats both the same way: the
//! fenced or conflicting batch is not part of the realm.
//!
//! # Fence leases
//!
//! The store records a lease for the holder of the highest token: it lasts
//! [`FenceTiming::lease`] from the holder's last acquire or renew, as the
//! store's own clock saw it. A holder counts as live while its recorded
//! lease lasts. The host sizes the lease and the renew interval through
//! [`HostStore::fence_timing`]; the defaults are [`FENCE_LEASE_SECS`] and
//! [`FENCE_RENEW_INTERVAL_SECS`]. A shorter lease shortens the wait at a
//! restart and at a takeover; a longer interval renews less often.
//!
//! The host also declares the longest one commit takes on its backend
//! ([`FenceTiming::max_commit_latency`], [`FENCE_MAX_COMMIT_LATENCY_SECS`]
//! by default). A fenced write that meets a commit in flight on the
//! authority it checks waits that long before it reports the write as
//! backed off, so a host whose commits are slower declares a longer bound
//! instead of failing those writes.
//!
//! # Writer rules (Core's side)
//!
//! - A writer acquires with [`AcquireMode::Exclusive`]; a live holder makes
//!   that answer [`HostStoreError::Held`] with what remains of its lease, so
//!   two processes never fence each other back and forth. Apart from Core
//!   taking over its own recorded lease (a stopped holder re-acquires, and
//!   its exclusive acquire answers Held with its own token), only an
//!   explicit takeover uses [`AcquireMode::Takeover`].
//! - A holder renews at least once per renew interval and stops writing as
//!   soon as a renew fails.
//! - **A holder that stopped re-acquires; it never resumes on a later
//!   renew.** Once a renew failed or its window closed, an acquire made
//!   meanwhile may have missed its last renew (the store's visibility lag)
//!   and seen its lease lapse; a renew that then succeeds proves nothing.
//!   It resumes only after a fresh fenced acquire, which acquires are
//!   ordered against: a token higher than any acquired before. This rule is
//!   Core's, not the store's: a store answers a renew of the current token
//!   even when the holder's recorded lease lapsed, and refuses only a token
//!   that was superseded or that it never issued
//!   ([`HostStore::renew_fence`]).
//! - A holder fences itself on its own clock: it makes no model call and
//!   dispatches nothing unless its last successful acquire or renew started
//!   less than [`FenceTiming::self_fence_window`] ago, the lease minus the
//!   host-declared self-fence margin ([`FenceTiming::self_fence_margin`],
//!   one renew interval unless the host declares another). This takes no
//!   store I/O per round, and there is no fence check on the per round
//!   path. Core exposes the window through [`SelfFence`], so the run loop
//!   checks it right before each model call and each tool dispatch.
//! - **The host's obligation on the margin:** it is at least one renew
//!   interval plus twice the store's visibility lag (the time from the
//!   start of a write until every acquire and renew sees it). A takeover
//!   can miss the renews started in the lag before it, so the lease it
//!   waits out ([`FenceGrant::superseded_lease`]) can come from a renew an
//!   interval or more older than the old holder's last one; and a renew
//!   started in the lag after the takeover can still confirm the old
//!   token. With no visibility lag, the default margin of one renew
//!   interval is enough.
//! - After an acquire that supersedes a live holder, the new writer waits
//!   out what remained of that holder's recorded lease
//!   ([`FenceGrant::superseded_lease`]) before it makes a model call or
//!   dispatches anything. By then the old holder has fenced itself: the
//!   store started that lease no earlier than the holder started the
//!   acquire or renew it came from, and the holder stops the self-fence
//!   margin before the lease ends, which covers the renews the takeover
//!   could not see.
//! - The bulk load of heads ([`HostStore::list_heads`]) runs after the
//!   acquire and before the first commit, so it observes one cut.

use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};

/// The most bytes one batch carries: the heads and rows of every scope it
/// writes, together.
pub const MAX_BATCH_BYTES: usize = 8 * 1024 * 1024;

/// The most bytes one row carries.
pub const MAX_ROW_BYTES: usize = 1024 * 1024;

/// The most bytes of a scope name.
pub const MAX_SCOPE_NAME_BYTES: usize = 512;

/// The most bytes of a commit id.
pub const MAX_COMMIT_ID_BYTES: usize = 128;

/// How often a fence holder renews its token by default, in seconds. A host
/// sizes its own interval through [`HostStore::fence_timing`].
pub const FENCE_RENEW_INTERVAL_SECS: u64 = 10;

/// How long a fence holder counts as live after its last acquire or renew
/// by default, in seconds. A host sizes its own lease through
/// [`HostStore::fence_timing`].
pub const FENCE_LEASE_SECS: u64 = 30;

/// The fewest renew intervals a lease lasts, so that one late renew does not
/// make a live holder fence itself. A failed renew stops the holder at once
/// (it acquires again before it resumes).
pub const FENCE_MIN_LEASE_RENEWALS: u32 = 3;

/// The longest one commit takes by default, in seconds. A host declares its
/// own bound through [`HostStore::fence_timing`]
/// ([`FenceTiming::try_with_max_commit_latency`]).
pub const FENCE_MAX_COMMIT_LATENCY_SECS: u64 = 2;

const _: () =
    assert!(FENCE_LEASE_SECS >= FENCE_RENEW_INTERVAL_SECS * FENCE_MIN_LEASE_RENEWALS as u64);

/// How a realm's fence is renewed and how long a holder stays live, as the
/// host sizes them for its backend and its rollout (see the module
/// documentation on fence leases). The defaults are
/// [`FENCE_RENEW_INTERVAL_SECS`] and [`FENCE_LEASE_SECS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FenceTiming {
    renew_interval: Duration,
    lease: Duration,
    self_fence_margin: Duration,
    max_commit_latency: Duration,
}

impl FenceTiming {
    /// The shortest renew interval a host may choose.
    pub const MIN_RENEW_INTERVAL: Duration = Duration::from_secs(1);

    /// The longest lease a host may choose. A takeover may wait this long.
    pub const MAX_LEASE: Duration = Duration::from_secs(60 * 60);

    /// A timing that renews every `renew_interval` and keeps a holder live
    /// for `lease` after its last acquire or renew. The interval is at least
    /// [`Self::MIN_RENEW_INTERVAL`], and the lease lasts at least
    /// [`FENCE_MIN_LEASE_RENEWALS`] intervals and at most
    /// [`Self::MAX_LEASE`].
    pub fn try_new(renew_interval: Duration, lease: Duration) -> Result<Self, HostStoreError> {
        Self::try_with_margin(renew_interval, lease, renew_interval)
    }

    /// A timing as [`Self::try_new`] with a host-declared self-fence margin:
    /// a holder writes only while its last successful acquire or renew
    /// started less than `lease - self_fence_margin` ago. The margin is at
    /// least one renew interval and leaves a window of at least two; the
    /// host's obligation is a margin of at least one renew interval plus
    /// twice the store's visibility lag (see the writer rules in the module
    /// documentation).
    pub fn try_with_margin(
        renew_interval: Duration,
        lease: Duration,
        self_fence_margin: Duration,
    ) -> Result<Self, HostStoreError> {
        if renew_interval < Self::MIN_RENEW_INTERVAL {
            return Err(HostStoreError::Invalid(format!(
                "a fence renew interval of {renew_interval:?} is shorter than {:?}",
                Self::MIN_RENEW_INTERVAL
            )));
        }
        let shortest_lease = renew_interval.saturating_mul(FENCE_MIN_LEASE_RENEWALS);
        if lease < shortest_lease || lease > Self::MAX_LEASE {
            return Err(HostStoreError::Invalid(format!(
                "a fence lease of {lease:?} lies outside {shortest_lease:?} to {:?}",
                Self::MAX_LEASE
            )));
        }
        // At least one renew interval (a store with a visibility lag needs
        // more, see the writer rules), and a window of at least two renew
        // intervals, so a holder that renews on time never closes its own
        // window between two renews.
        let longest_margin = lease.saturating_sub(renew_interval.saturating_mul(2));
        if self_fence_margin < renew_interval || self_fence_margin > longest_margin {
            return Err(HostStoreError::Invalid(format!(
                "a self-fence margin of {self_fence_margin:?} lies outside {renew_interval:?} to \
                 {longest_margin:?} (one renew interval to the lease minus two renew intervals)"
            )));
        }
        Ok(Self {
            renew_interval,
            lease,
            self_fence_margin,
            max_commit_latency: Duration::from_secs(FENCE_MAX_COMMIT_LATENCY_SECS),
        })
    }

    /// This timing with a host-declared longest commit: how long one
    /// [`HostStore::commit`] can take on the host's backend. A fenced write
    /// that meets a commit in flight on the authority it checks waits this
    /// long before it reports the write as backed off. The bound is longer
    /// than zero and at most the lease.
    pub fn try_with_max_commit_latency(
        self,
        max_commit_latency: Duration,
    ) -> Result<Self, HostStoreError> {
        if max_commit_latency.is_zero() || max_commit_latency > self.lease {
            return Err(HostStoreError::Invalid(format!(
                "a longest commit of {max_commit_latency:?} is not longer than zero and at most \
                 the lease of {:?}",
                self.lease
            )));
        }
        Ok(Self {
            max_commit_latency,
            ..self
        })
    }

    /// How often a holder renews.
    pub const fn renew_interval(self) -> Duration {
        self.renew_interval
    }

    /// How long a holder counts as live after its last acquire or renew.
    pub const fn lease(self) -> Duration {
        self.lease
    }

    /// The host-declared self-fence margin: one renew interval unless the
    /// host declared another ([`Self::try_with_margin`]).
    pub const fn self_fence_margin(self) -> Duration {
        self.self_fence_margin
    }

    /// How long after the start of its last successful acquire or renew a
    /// holder may still make a model call or dispatch, on its own clock: the
    /// lease minus the self-fence margin.
    pub const fn self_fence_window(self) -> Duration {
        self.lease.saturating_sub(self.self_fence_margin)
    }

    /// The host-declared longest commit: [`FENCE_MAX_COMMIT_LATENCY_SECS`]
    /// unless the host declared another
    /// ([`Self::try_with_max_commit_latency`]).
    pub const fn max_commit_latency(self) -> Duration {
        self.max_commit_latency
    }
}

impl Default for FenceTiming {
    fn default() -> Self {
        Self {
            renew_interval: Duration::from_secs(FENCE_RENEW_INTERVAL_SECS),
            lease: Duration::from_secs(FENCE_LEASE_SECS),
            self_fence_margin: Duration::from_secs(FENCE_RENEW_INTERVAL_SECS),
            max_commit_latency: Duration::from_secs(FENCE_MAX_COMMIT_LATENCY_SECS),
        }
    }
}

/// A writer's self-fence on its own clock (the writer rules in the module
/// documentation). Core's host store realm implements it; the run loop asks
/// it right before each model call and each tool dispatch, not only before
/// commits.
pub trait SelfFence: Send + Sync {
    /// The writer's self-fence window now. A local clock read; no store I/O.
    fn window(&self) -> SelfFenceWindow;
}

/// One reading of a writer's self-fence window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfFenceWindow {
    /// Time since the start of the last successful acquire or renew.
    pub elapsed: Duration,
    /// [`FenceTiming::self_fence_window`]: the lease minus the margin.
    pub window: Duration,
    /// Whether a renew failed after that acquire or renew.
    pub renew_failed: bool,
}

impl SelfFenceWindow {
    /// Whether the writer may still make a model call or dispatch: no renew
    /// failed, and the last successful acquire or renew started less than
    /// the window ago.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.renew_failed && self.elapsed < self.window
    }
}

/// What an acquire answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FenceGrant {
    /// The new token, higher than any acquired before it in the realm.
    pub token: FenceToken,
    /// What remained, at the acquire, of the lease of the holder this
    /// acquire superseded, as the store recorded that lease; zero when no
    /// holder was live. The new holder waits this long before it makes a
    /// model call or dispatches anything.
    pub superseded_lease: Duration,
}

/// How [`HostStore::acquire_fence`] treats a live holder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AcquireMode {
    /// Refuse with [`HostStoreError::Held`] while another holder's lease is
    /// live.
    Exclusive,
    /// Supersede any holder, live or not.
    Takeover,
}

/// What a scope holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScopeKind {
    /// One runtime's state: its head is the runtime state, its rows the
    /// transcript.
    Session,
    /// One durable identity and what it is bound to.
    Identity,
    /// One mob: its head is the mob's state, its rows the mob's event log.
    Mob,
    /// A realm wide record, such as one runtime per continuation key.
    Record,
}

impl ScopeKind {
    /// The kind's stable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Identity => "identity",
            Self::Mob => "mob",
            Self::Record => "record",
        }
    }
}

impl fmt::Display for ScopeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Refuse text no store keeps under its own name: empty, longer than
/// `max_bytes`, or holding a control character.
fn validate_text(text: &str, max_bytes: usize, what: &str) -> Result<(), HostStoreError> {
    if text.is_empty() {
        return Err(HostStoreError::Invalid(format!("{what} is empty")));
    }
    if text.len() > max_bytes {
        return Err(HostStoreError::Invalid(format!(
            "{what} is longer than {max_bytes} bytes"
        )));
    }
    if text.chars().any(char::is_control) {
        return Err(HostStoreError::Invalid(format!(
            "{what} holds a control character"
        )));
    }
    Ok(())
}

/// A scope's name: unique among the scopes of its kind in one realm. Non
/// empty, at most [`MAX_SCOPE_NAME_BYTES`], with no control characters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ScopeName(String);

impl ScopeName {
    /// Validate `name` as a scope name.
    pub fn try_new(name: impl Into<String>) -> Result<Self, HostStoreError> {
        let name = name.into();
        validate_text(&name, MAX_SCOPE_NAME_BYTES, "a scope name")?;
        Ok(Self(name))
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ScopeName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::try_new(name).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for ScopeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The address of one scope: its kind and its name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScopeRef {
    pub kind: ScopeKind,
    pub name: ScopeName,
}

impl ScopeRef {
    #[must_use]
    pub fn new(kind: ScopeKind, name: ScopeName) -> Self {
        Self { kind, name }
    }
}

impl fmt::Display for ScopeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} scope {:?}", self.kind, self.name.as_str())
    }
}

/// A scope's version. A scope's first version is [`ScopeVersion::FIRST`];
/// each batch that touches the scope moves it to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScopeVersion(u64);

impl ScopeVersion {
    /// The version a scope's creating batch gives it.
    pub const FIRST: Self = Self(1);

    #[must_use]
    pub const fn new(version: u64) -> Self {
        Self(version)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The version after this one, if it does not overflow.
    #[must_use]
    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

impl fmt::Display for ScopeVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A realm's fence token. Each acquire answers a token higher than any
/// acquired before it in the realm; a commit carries the token its writer
/// holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FenceToken(u64);

impl FenceToken {
    #[must_use]
    pub const fn new(token: u64) -> Self {
        Self(token)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for FenceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The index of a row in its scope. Indices are stable: removing rows at
/// either end never renumbers the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RowIndex(u64);

impl RowIndex {
    #[must_use]
    pub const fn new(index: u64) -> Self {
        Self(index)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RowIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A batch's commit id: assigned by Core, opaque to the store, unique per
/// batch in the realm. Non empty, at most [`MAX_COMMIT_ID_BYTES`], with no
/// control characters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CommitId(String);

impl CommitId {
    /// Validate `id` as a commit id.
    pub fn try_new(id: impl Into<String>) -> Result<Self, HostStoreError> {
        let id = id.into();
        validate_text(&id, MAX_COMMIT_ID_BYTES, "a commit id")?;
        Ok(Self(id))
    }

    /// The id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CommitId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = String::deserialize(deserializer)?;
        Self::try_new(id).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One row: opaque bytes, at most [`MAX_ROW_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row(Vec<u8>);

impl Row {
    /// Validate `bytes` as a row.
    pub fn try_new(bytes: Vec<u8>) -> Result<Self, HostStoreError> {
        if bytes.len() > MAX_ROW_BYTES {
            return Err(HostStoreError::Invalid(format!(
                "a row is longer than {MAX_ROW_BYTES} bytes"
            )));
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A scope's visible head: its version, the commit id of the batch that
/// wrote that version, and the head bytes as it left them. After an
/// indeterminate commit, comparing `commit_id` with the batch's tells Core
/// whether it landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedHead {
    pub version: ScopeVersion,
    pub commit_id: CommitId,
    pub head: Vec<u8>,
}

/// One scope's head in a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeHead {
    pub name: ScopeName,
    pub head: VersionedHead,
}

/// One page of [`HostStore::list_heads`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeadPage {
    /// Visible scopes of the listed kind, in name order (by the bytes of
    /// their names), each with its visible head.
    pub heads: Vec<ScopeHead>,
    /// The name to read on after, or `None` when the listing has ended.
    pub next: Option<ScopeName>,
}

/// What one batch does to one scope. The parts apply in this order:
/// `truncate_after`, `drop_before`, `append`, `head`. `delete` stands
/// alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeUpdate {
    pub scope: ScopeRef,
    /// The version this update applies to; `None` creates the scope.
    pub previous: Option<ScopeVersion>,
    /// The scope's version after the batch: the one after `previous`, or
    /// [`ScopeVersion::FIRST`] for a scope this batch creates.
    pub next: ScopeVersion,
    /// The new head; `None` keeps the current one. A creating update must
    /// set it.
    pub head: Option<Vec<u8>>,
    /// Rows appended at the scope's end, in order: the first takes the
    /// scope's end index after any truncation.
    pub append: Vec<Row>,
    /// Remove every row whose index is at least this one, so that the next
    /// appended row takes this index. At or past the end, nothing is
    /// removed.
    pub truncate_after: Option<RowIndex>,
    /// Remove every row whose index is below this one, without renumbering
    /// the rest. Past the end, every row is removed and the end index stays
    /// where it was.
    pub drop_before: Option<RowIndex>,
    /// Delete the scope with its head and rows. Nothing else may be set.
    /// Core never creates a deleted scope's name again: names are unique per
    /// incarnation. A store may refuse such a create as
    /// [`HostStoreError::Invalid`].
    pub delete: bool,
}

impl ScopeUpdate {
    /// The bytes this update counts against [`MAX_BATCH_BYTES`].
    #[must_use]
    pub fn payload_bytes(&self) -> usize {
        let head = self.head.as_ref().map_or(0, Vec::len);
        self.append
            .iter()
            .map(Row::len)
            .fold(head, usize::saturating_add)
    }

    fn validate(&self) -> Result<(), HostStoreError> {
        let expected = match self.previous {
            None => Some(ScopeVersion::FIRST),
            Some(previous) => previous.next(),
        };
        if expected != Some(self.next) {
            return Err(HostStoreError::Invalid(format!(
                "{}: version {} does not follow {}",
                self.scope,
                self.next,
                self.previous
                    .map_or_else(|| "creation".to_string(), |previous| previous.to_string())
            )));
        }
        if self.delete {
            if self.previous.is_none() {
                return Err(HostStoreError::Invalid(format!(
                    "{}: a batch cannot create and delete a scope",
                    self.scope
                )));
            }
            if self.head.is_some()
                || !self.append.is_empty()
                || self.truncate_after.is_some()
                || self.drop_before.is_some()
            {
                return Err(HostStoreError::Invalid(format!(
                    "{}: a delete sets nothing else",
                    self.scope
                )));
            }
        }
        if self.previous.is_none() {
            if self.head.is_none() {
                return Err(HostStoreError::Invalid(format!(
                    "{}: a creating update sets the head",
                    self.scope
                )));
            }
            if self.truncate_after.is_some() || self.drop_before.is_some() {
                return Err(HostStoreError::Invalid(format!(
                    "{}: a creating update has no rows to remove",
                    self.scope
                )));
            }
        }
        Ok(())
    }
}

/// One atomic write: a commit id, the fence token of the writer, and the
/// updates of one or more scopes. Built only through
/// [`CommitBatch::try_new`], so every batch a store receives is well formed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitBatch {
    commit_id: CommitId,
    fence: FenceToken,
    updates: Vec<ScopeUpdate>,
}

impl CommitBatch {
    /// Validate a batch: at least one update, each scope at most once, each
    /// update well formed, and at most [`MAX_BATCH_BYTES`] of heads and rows
    /// in all.
    pub fn try_new(
        commit_id: CommitId,
        fence: FenceToken,
        updates: Vec<ScopeUpdate>,
    ) -> Result<Self, HostStoreError> {
        if updates.is_empty() {
            return Err(HostStoreError::Invalid(
                "a batch updates at least one scope".to_string(),
            ));
        }
        let mut scopes = BTreeSet::new();
        let mut bytes = 0_usize;
        for update in &updates {
            if !scopes.insert(&update.scope) {
                return Err(HostStoreError::Invalid(format!(
                    "{} appears twice in one batch",
                    update.scope
                )));
            }
            update.validate()?;
            bytes = bytes.saturating_add(update.payload_bytes());
        }
        if bytes > MAX_BATCH_BYTES {
            return Err(HostStoreError::Invalid(format!(
                "a batch carries more than {MAX_BATCH_BYTES} bytes"
            )));
        }
        Ok(Self {
            commit_id,
            fence,
            updates,
        })
    }

    #[must_use]
    pub fn commit_id(&self) -> &CommitId {
        &self.commit_id
    }

    #[must_use]
    pub fn fence(&self) -> FenceToken {
        self.fence
    }

    #[must_use]
    pub fn updates(&self) -> &[ScopeUpdate] {
        &self.updates
    }

    /// The bytes this batch counts against [`MAX_BATCH_BYTES`].
    #[must_use]
    pub fn payload_bytes(&self) -> usize {
        self.updates
            .iter()
            .map(ScopeUpdate::payload_bytes)
            .fold(0, usize::saturating_add)
    }
}

/// Why a host store operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HostStoreError {
    /// The store refused the batch at commit: `scope` is at `current`
    /// (`None`: it does not exist), not at the update's previous version.
    /// Only a store that meets obligation 4 at commit answers it; nothing
    /// of the batch is applied.
    #[error("{scope} is at version {current:?}, not the batch's previous version")]
    Conflict {
        scope: ScopeRef,
        current: Option<ScopeVersion>,
    },
    /// A higher fence token than the caller's exists: the caller is fenced
    /// out and stops writing. Answered by `renew_fence`, and by `commit` in
    /// a store that meets obligation 5 at commit; nothing of the batch is
    /// applied.
    #[error("fenced out: token {current} is current")]
    FencedOut { current: FenceToken },
    /// The outcome is unknown: the batch may or may not have landed. Core
    /// retries it with the same commit id, or loads the head to tell.
    #[error("outcome unknown: {0}")]
    Indeterminate(String),
    /// The request breaks a limit or is malformed; nothing was applied.
    #[error("invalid: {0}")]
    Invalid(String),
    /// The store cannot be reached or cannot serve now; nothing was
    /// applied.
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// An exclusive acquire found a live holder: `holder`, whose recorded
    /// lease lasts `remaining` longer. Nothing was acquired.
    #[error("the fence is held by token {holder} for {remaining:?} more")]
    Held {
        holder: FenceToken,
        remaining: Duration,
    },
}

/// The storage interface a host implements for one realm. Core is the only
/// writer; see the module documentation for the obligations.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait HostStore: Send + Sync {
    /// Acquire the realm's fence: answer a token higher than any acquired
    /// before, durably, so that batches carrying lower tokens whose commit
    /// point comes later never become visible, and loads from now on show
    /// every batch committed before (obligation 5). The store records the
    /// new holder's lease ([`HostStore::fence_timing`]). With
    /// [`AcquireMode::Exclusive`], a live holder makes it answer
    /// [`HostStoreError::Held`]; with [`AcquireMode::Takeover`], the grant
    /// carries what remained of that holder's recorded lease.
    async fn acquire_fence(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError>;

    /// Confirm that no token higher than `token` exists, and record a new
    /// lease for its holder from now ([`HostStore::fence_timing`]).
    /// [`HostStoreError::FencedOut`] stops the caller's writes.
    ///
    /// A renew of the current token succeeds even when its holder sat idle
    /// past its recorded lease: the store refuses only a token that was
    /// superseded or that it never issued. Whether a holder may resume after
    /// a failed renew or a lapse is Core's rule (it acquires again; see the
    /// writer rules in the module documentation), not an obligation of the
    /// store.
    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError>;

    /// The renew interval and the lease the host chose for this store. The
    /// store records leases of this length; Core renews at this interval and
    /// fences itself on [`FenceTiming::self_fence_window`]. The default is
    /// [`FenceTiming::default`].
    fn fence_timing(&self) -> FenceTiming {
        FenceTiming::default()
    }

    /// The scope's visible head and version, or `None` when the scope does
    /// not exist (never created, or deleted).
    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError>;

    /// Visible scopes of `kind` whose names sort after `after` (from the
    /// first when `None`), in name order, each with its visible head: at
    /// most `limit`. Serves both listing and the bulk load at boot and
    /// failover (obligation 9).
    async fn list_heads(
        &self,
        kind: ScopeKind,
        after: Option<&ScopeName>,
        limit: usize,
    ) -> Result<HeadPage, HostStoreError>;

    /// The scope's visible rows from index `from`, in index order (which is
    /// commit order, obligation 7): the rows at `from`, `from + 1`, and so on,
    /// at most `limit`, ending early at the scope's end. A `from` below the
    /// scope's first visible row (rows dropped before it) is
    /// [`HostStoreError::Invalid`]. A scope that does not exist has no rows.
    async fn read_rows(
        &self,
        scope: &ScopeRef,
        from: RowIndex,
        limit: usize,
    ) -> Result<Vec<Row>, HostStoreError>;

    /// Commit `batch` atomically (obligation 1). `Ok` means durable
    /// (obligation 2). A commit id that already landed answers `Ok` without
    /// acting again (obligation 3). [`HostStoreError::Indeterminate`] means
    /// the outcome is unknown; Core retries with the same batch.
    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unrun until the candidate's validation.
    #[test]
    fn a_declared_self_fence_margin_sets_the_window() {
        let secs = Duration::from_secs;
        let default = FenceTiming::default();
        assert_eq!(default.self_fence_margin(), default.renew_interval());
        assert_eq!(default.self_fence_window(), secs(20));
        let plain = FenceTiming::try_new(secs(2), secs(6));
        assert_eq!(plain.map(FenceTiming::self_fence_margin), Ok(secs(2)));
        // The host's 10 s margin on a 30 s lease, at 10 s and at 5 s renews.
        let host = FenceTiming::try_with_margin(secs(10), secs(30), secs(10));
        assert_eq!(host.map(FenceTiming::self_fence_window), Ok(secs(20)));
        let faster = FenceTiming::try_with_margin(secs(5), secs(30), secs(10));
        assert_eq!(faster.map(FenceTiming::self_fence_window), Ok(secs(20)));
        // Below one renew interval.
        assert!(FenceTiming::try_with_margin(secs(10), secs(30), secs(4)).is_err());
        assert!(FenceTiming::try_with_margin(secs(10), secs(30), Duration::ZERO).is_err());
        // A window under two renew intervals.
        assert!(FenceTiming::try_with_margin(secs(10), secs(30), secs(12)).is_err());
        assert!(FenceTiming::try_with_margin(secs(10), secs(30), secs(30)).is_err());
    }

    // Unrun until the candidate's validation.
    #[test]
    fn a_declared_longest_commit_is_kept_within_the_lease() {
        let secs = Duration::from_secs;
        assert_eq!(FenceTiming::default().max_commit_latency(), secs(2));
        let slow = FenceTiming::default().try_with_max_commit_latency(secs(10));
        assert_eq!(
            slow.clone().map(FenceTiming::max_commit_latency),
            Ok(secs(10))
        );
        // The rest of the timing is unchanged.
        assert_eq!(
            slow.map(FenceTiming::self_fence_window),
            Ok(FenceTiming::default().self_fence_window())
        );
        assert!(
            FenceTiming::default()
                .try_with_max_commit_latency(Duration::ZERO)
                .is_err()
        );
        assert!(
            FenceTiming::default()
                .try_with_max_commit_latency(secs(31))
                .is_err()
        );
    }
}
