//! Change capture, export and import of the in-memory runtime store for the
//! host store journal ([`crate::store::spi`]).
//!
//! Every map of [`Inner`] (and of the ops lifecycle state) is a
//! [`Tracked`] map: it has no `DerefMut`, so every mutation goes through one
//! of its four mutators, and each mutator reports the journal entry it
//! touches to the change log of the task it runs on ([`capture`]). Outside a
//! capture nothing is recorded, so a plain in-memory store pays one failed
//! task-local lookup per mutation.
//!
//! A touch names a scope ([`TouchScope`]: a runtime, or a realm record) and
//! either one entry of a group, or the whole group of that scope. A nested
//! map (one runtime's input states, its idempotency index, its delivery
//! records) is touched whole by its outer mutators; the helpers that mutate
//! it entry by entry narrow it ([`Narrow`]) and report each entry instead,
//! so a round re-exports the inputs it changed, not every input.
//!
//! The export and import of what was recorded live in
//! [`super::journal_image`] (native targets only, like the host store
//! adapters that use them).

// A browser build records nothing (no task-local change log): the journal's
// readers of the recorded touches are native only.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::borrow::Borrow;
#[cfg(not(target_arch = "wasm32"))]
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::Hash;

use super::ReceiptKey;
use meerkat_core::lifecycle::RunId;

/// The journal group a tracked map holds. Each group has fixed entry key
/// names ([`Group::names`]) inside its scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Group {
    InputStates,
    PendingOwners,
    Nonterminal,
    InputSetRevision,
    IdempotencyIndex,
    Receipts,
    RecoveryBoundaries,
    Sessions,
    SessionAuthorities,
    Catalog,
    ProvisionalTails,
    Quarantine,
    Lifecycle,
    MemberHighWaters,
    CompactionOutbox,
    DeliveryAuthority,
    DeliveryRecords,
    KeyBindings,
    Admissions,
    OpsSnapshots,
    OpsRetiredEpochs,
}

impl Group {
    /// The entry key groups (the text before `/`) this group writes.
    pub(crate) const fn names(self) -> &'static [&'static str] {
        match self {
            Self::InputStates => &["is"],
            Self::PendingOwners => &["pto"],
            Self::Nonterminal => &["rni"],
            Self::InputSetRevision => &["isr"],
            Self::IdempotencyIndex => &["iik"],
            Self::Receipts => &["rc"],
            Self::RecoveryBoundaries => &["rb"],
            Self::Sessions => &["ss"],
            Self::SessionAuthorities => &["sa"],
            Self::Catalog => &["cat"],
            Self::ProvisionalTails => &["wpt", "wptb"],
            Self::Quarantine => &["pq"],
            Self::Lifecycle => &["lc"],
            Self::CompactionOutbox => &["co"],
            Self::DeliveryAuthority => &["rda"],
            Self::DeliveryRecords => &["rdr"],
            Self::Admissions => &["ca"],
            Self::OpsSnapshots => &["ops"],
            Self::OpsRetiredEpochs => &["ore"],
            Self::MemberHighWaters | Self::KeyBindings => &["v"],
        }
    }

    /// Every group of a runtime scope.
    pub(crate) const RUNTIME: [Self; 19] = [
        Self::InputStates,
        Self::PendingOwners,
        Self::Nonterminal,
        Self::InputSetRevision,
        Self::IdempotencyIndex,
        Self::Receipts,
        Self::RecoveryBoundaries,
        Self::Sessions,
        Self::SessionAuthorities,
        Self::Catalog,
        Self::ProvisionalTails,
        Self::Quarantine,
        Self::Lifecycle,
        Self::CompactionOutbox,
        Self::DeliveryAuthority,
        Self::DeliveryRecords,
        Self::Admissions,
        Self::OpsSnapshots,
        Self::OpsRetiredEpochs,
    ];
}

/// The journal scope a touched entry belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum TouchScope {
    /// One runtime (session scope).
    Runtime(String),
    /// One continuation key binding (record scope).
    KeyBinding { owner: String, key: String },
    /// One direct member's incarnation high water (record scope).
    MemberHighWater(String),
}

/// Where a tracked key lives: its scope, and its entry within the group
/// (`None`: the whole group of that scope).
pub(super) struct Touch {
    scope: TouchScope,
    sub: Option<String>,
}

#[allow(
    clippy::ptr_arg,
    reason = "a locator is a `fn(&K)` for maps keyed by `String`"
)]
pub(super) fn whole_runtime(runtime_id: &String) -> Touch {
    Touch {
        scope: TouchScope::Runtime(runtime_id.clone()),
        sub: None,
    }
}

pub(super) fn receipt_entry(key: &ReceiptKey) -> Touch {
    Touch {
        scope: TouchScope::Runtime(key.runtime_id.clone()),
        sub: Some(receipt_sub(&key.run_id, key.sequence)),
    }
}

pub(super) fn runtime_pair_entry(key: &(String, String)) -> Touch {
    Touch {
        scope: TouchScope::Runtime(key.0.clone()),
        sub: Some(key.1.clone()),
    }
}

pub(super) fn key_binding(key: &(String, String)) -> Touch {
    Touch {
        scope: TouchScope::KeyBinding {
            owner: key.0.clone(),
            key: key.1.clone(),
        },
        sub: None,
    }
}

#[allow(
    clippy::ptr_arg,
    reason = "a locator is a `fn(&K)` for maps keyed by `String`"
)]
pub(super) fn member_high_water(member: &String) -> Touch {
    Touch {
        scope: TouchScope::MemberHighWater(member.clone()),
        sub: None,
    }
}

pub(super) fn retired_epoch(key: &(String, meerkat_core::RuntimeEpochId)) -> Touch {
    Touch {
        scope: TouchScope::Runtime(key.0.clone()),
        sub: Some(key.1.0.to_string()),
    }
}

pub(super) fn receipt_sub(run_id: &RunId, sequence: u64) -> String {
    format!("{}/{sequence}", run_id.0)
}

/// What one captured call touched.
#[derive(Debug, Default)]
pub(crate) struct Touches {
    /// Groups touched whole, per scope.
    pub(crate) whole: BTreeSet<(TouchScope, Group)>,
    /// Single entries touched, per scope and group.
    pub(crate) entries: BTreeSet<(TouchScope, Group, String)>,
    narrowing: Vec<Group>,
}

impl Touches {
    /// Every scope the call touched.
    pub(crate) fn scopes(&self) -> BTreeSet<TouchScope> {
        self.whole
            .iter()
            .map(|(scope, _)| scope.clone())
            .chain(self.entries.iter().map(|(scope, _, _)| scope.clone()))
            .collect()
    }
}

#[cfg(not(target_arch = "wasm32"))]
tokio::task_local! {
    static TOUCHES: RefCell<Touches>;
}

/// Run `future` and answer what it touched in tracked maps.
pub(crate) async fn capture<F: std::future::Future>(future: F) -> (F::Output, Touches) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        TOUCHES
            .scope(RefCell::new(Touches::default()), async move {
                let output = future.await;
                let touches = TOUCHES
                    .try_with(|cell| std::mem::take(&mut *cell.borrow_mut()))
                    .unwrap_or_default();
                (output, touches)
            })
            .await
    }
    #[cfg(target_arch = "wasm32")]
    {
        (future.await, Touches::default())
    }
}

fn record(group: Group, locate: impl FnOnce() -> Touch) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = TOUCHES.try_with(|cell| {
            let mut touches = cell.borrow_mut();
            let Touch { scope, sub } = locate();
            match sub {
                Some(sub) => {
                    touches.entries.insert((scope, group, sub));
                }
                None => {
                    if !touches.narrowing.contains(&group) {
                        touches.whole.insert((scope, group));
                    }
                }
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (group, locate);
    }
}

/// Report one entry of a narrowed group: `sub` of `group` in runtime
/// `runtime_id`.
pub(super) fn touch_runtime_entry(runtime_id: &str, group: Group, sub: impl FnOnce() -> String) {
    record(group, || Touch {
        scope: TouchScope::Runtime(runtime_id.to_string()),
        sub: Some(sub()),
    });
}

/// While alive, whole-group touches of its groups are not recorded: the
/// helper that holds it reports every entry it changes.
pub(super) struct Narrow {
    count: usize,
}

impl Narrow {
    pub(super) fn begin(groups: &[Group]) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let pushed = TOUCHES
                .try_with(|cell| {
                    cell.borrow_mut().narrowing.extend_from_slice(groups);
                    groups.len()
                })
                .unwrap_or(0);
            Self { count: pushed }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = groups;
            Self { count: 0 }
        }
    }
}

impl Drop for Narrow {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.count > 0 {
            let count = self.count;
            let _ = TOUCHES.try_with(|cell| {
                let mut touches = cell.borrow_mut();
                let keep = touches.narrowing.len().saturating_sub(count);
                touches.narrowing.truncate(keep);
            });
        }
    }
}

/// A map whose mutations report the journal entries they touch. It derefs
/// to the map for every read; it has no `DerefMut`.
pub(super) struct Tracked<K, V> {
    map: HashMap<K, V>,
    group: Group,
    locate: fn(&K) -> Touch,
}

impl<K, V> Tracked<K, V> {
    pub(super) fn new(group: Group, locate: fn(&K) -> Touch) -> Self {
        Self {
            map: HashMap::new(),
            group,
            locate,
        }
    }

    /// The map without tracking: for the journal's own import and eviction.
    pub(super) fn untracked(&mut self) -> &mut HashMap<K, V> {
        &mut self.map
    }
}

impl<K: Eq + Hash, V> Tracked<K, V> {
    pub(super) fn insert(&mut self, key: K, value: V) -> Option<V> {
        let locate = self.locate;
        record(self.group, || locate(&key));
        self.map.insert(key, value)
    }

    pub(super) fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let (key, value) = self.map.remove_entry(key)?;
        let locate = self.locate;
        record(self.group, || locate(&key));
        Some(value)
    }

    pub(super) fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        if let Some((stored, _)) = self.map.get_key_value(key) {
            let locate = self.locate;
            record(self.group, || locate(stored));
        }
        self.map.get_mut(key)
    }

    pub(super) fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        let locate = self.locate;
        record(self.group, || locate(&key));
        self.map.entry(key)
    }
}

impl<K, V> std::ops::Deref for Tracked<K, V> {
    type Target = HashMap<K, V>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl<'a, K, V> IntoIterator for &'a Tracked<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = std::collections::hash_map::Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}

impl<K: std::fmt::Debug, V: std::fmt::Debug> std::fmt::Debug for Tracked<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.map.fmt(f)
    }
}

/// A set whose mutations report the journal entries they touch.
pub(super) struct TrackedSet<K> {
    set: HashSet<K>,
    group: Group,
    locate: fn(&K) -> Touch,
}

impl<K> TrackedSet<K> {
    pub(super) fn new(group: Group, locate: fn(&K) -> Touch) -> Self {
        Self {
            set: HashSet::new(),
            group,
            locate,
        }
    }

    pub(super) fn untracked(&mut self) -> &mut HashSet<K> {
        &mut self.set
    }
}

impl<K: Eq + Hash> TrackedSet<K> {
    pub(super) fn insert(&mut self, key: K) -> bool {
        let locate = self.locate;
        record(self.group, || locate(&key));
        self.set.insert(key)
    }

    pub(super) fn remove<Q>(&mut self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        match self.set.take(key) {
            Some(key) => {
                let locate = self.locate;
                record(self.group, || locate(&key));
                true
            }
            None => false,
        }
    }
}

impl<K> std::ops::Deref for TrackedSet<K> {
    type Target = HashSet<K>;

    fn deref(&self) -> &Self::Target {
        &self.set
    }
}

impl<'a, K> IntoIterator for &'a TrackedSet<K> {
    type Item = &'a K;
    type IntoIter = std::collections::hash_set::Iter<'a, K>;

    fn into_iter(self) -> Self::IntoIter {
        self.set.iter()
    }
}

impl<K: std::fmt::Debug> std::fmt::Debug for TrackedSet<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.set.fmt(f)
    }
}
