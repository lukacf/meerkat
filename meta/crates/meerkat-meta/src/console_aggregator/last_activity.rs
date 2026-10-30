//! Each identity's newest activity time, kept in memory for
//! `/console/experience`.
//!
//! The console log answers an identity's newest acting frame by walking the
//! identity's frames newest first ([`ConsoleLogStore::latest_activity_times`]).
//! The walk is short when the identity acted recently and long when many of
//! its newest frames record something other than the identity acting, as
//! after a history backfill. The experience asks for every listed identity on
//! every read, so the aggregator walks an identity on its first read (its
//! seed), and from then on keeps its time from the frames the aggregator
//! appends itself ([`LastActivityCache::append`]). It walks an identity again
//! only after its entry is dropped: by an operator's retire or delete, by the
//! capacity's eviction, or by a clear (below). Which frames record an identity
//! acting is the store's one rule, [`is_activity_frame`]; which record it
//! leaving is the rule beside it, [`is_departure_frame`].
//!
//! An entry keeps the newest time it has seen, while a walk answers the time
//! of the newest acting frame by its position in the log. The two differ only
//! when a later frame carries an earlier time, as when an older event is
//! replayed late; the entry then keeps the later time.
//!
//! # Why an append that races a seed is never lost
//!
//! An identity's entry changes in one way only: a merge, which keeps the
//! newer of the entry's time and the merged one (any time is newer than
//! none). A seed merges the time its walk found. An acting append merges its
//! frame's time once the store has written the frame, into the identity's
//! entry, which it creates when no seed has run yet. Take the moment a seed's
//! walk reads the identity's frames: an acting frame stored before it is in
//! what the walk reads, and one stored after it is merged by its own append.
//! Merges commute, so whichever lands first, the entry ends as the newer of
//! the seed's time and every appended time; a seed that lands last never
//! replaces a newer appended time with its own older one.
//!
//! An acting append completes an entry by itself, too: the frame it stores is
//! the identity's newest acting frame when it is stored, and every later one
//! is merged by its own append. An identity that acted in this process before
//! its first read is never walked.
//!
//! # Holds
//!
//! A seed walks in chunks of [`SEED_CHUNK`] identities. Each chunk's walk and
//! merge share one hold of `log`, so for every identity the argument above
//! holds unchanged, and the seed yields between chunks. `seeding` stays held
//! across the chunks, so concurrent first reads still walk an identity once.
//! Every append also holds `log` shared, from its store call to its merge.
//!
//! A clear, a departure's drop and an eviction hold `log` alone. Each waits
//! for at most one chunk of a seed in flight and for the appends in flight;
//! the appends that arrive meanwhile wait for it in turn.
//!
//! - A clear empties the map in the same hold as the log, so no time merged
//!   after a clear belongs to a frame the clear removed.
//! - A drop or an eviction lands where no seed is between its walk and its
//!   merge: one that walked before has merged, and one after it walks the log
//!   again. Without that, a seed that walked before and merged after could
//!   leave an older time where an append had merged a newer one. The
//!   identity's frames stay in the log, so its next read finds them.
//!
//! # Cost of a first read
//!
//! At 10,000 identities the first read after a start makes 40 store calls.
//! When each identity's newest frames record it acting, each walk is one short
//! index seek: about 56 ms in total, what the experience used to pay on every
//! read. When many newest frames do not (a backfilled history), a walk passes
//! each of them at about 0.4 to 0.9 microseconds a frame: 10,000 identities
//! with 2,000 such frames each take about 8 to 18 seconds in all, spread over
//! the 40 chunks. An identity with no acting frame is walked again after each
//! start, since its `None` lives only in memory.
//!
//! # Memory
//!
//! - An operator's retire or delete (the `identity_retired` and
//!   `identity_deleted` records the console's and the RPC's lifecycle routes
//!   write, a reset of every member included) drops the identity's entry. A
//!   retirement made through the mob itself (an agent's retire tool, a
//!   member's retire request, the automatic retirement of a failed or timed
//!   out child) and an unregistered runtime write no such record, so their
//!   entries stay until the capacity evicts them.
//! - The map holds at most [`CAPACITY`] entries, or twice the most identities
//!   one read has asked for when that is more. Past it, the least recently
//!   used entries are evicted down to three quarters of it. An entry is used
//!   when a read returns it and when a seed or an acting append merges into
//!   it, so the identities a console lists stay. An evicted identity's next
//!   read walks the log again.
//! - A clear drops every entry. At about 150 bytes an entry, the default
//!   capacity holds about 5 MB.

use std::collections::HashMap;
use std::sync::{MutexGuard, PoisonError};

use super::store::{ConsoleLogResult, ConsoleLogStore, is_activity_frame, is_departure_frame};
use super::{AppendDisposition, AppendOutcome, NewConsoleFrame};

/// Identities a first read walks per store call, under one shared hold.
pub(super) const SEED_CHUNK: usize = 256;

/// The most entries the map keeps (see the module documentation): room for a
/// console of 10,000 agents with churn three times over.
pub(super) const CAPACITY: usize = 32_768;

/// The aggregator's in-memory last activity, shared by every policy view of
/// one canonical projector (see the module documentation).
pub(super) struct LastActivityCache {
    entries: std::sync::Mutex<Entries>,
    /// Held by a seed across its chunks, so concurrent first reads walk each
    /// identity once.
    seeding: tokio::sync::Mutex<()>,
    /// Shared by appends and by each seed chunk from its store call to its
    /// merge; held alone by a clear, a departure's drop and an eviction.
    log: tokio::sync::RwLock<()>,
    seed_chunk: usize,
    capacity: usize,
}

struct Entries {
    /// Each identity's entry. An identity without one has not been read or
    /// acted in this process since its entry was last dropped, if it was.
    map: HashMap<String, Entry>,
    /// The use clock: every use stamps an entry with the next tick.
    tick: u64,
    /// The most identities one read has asked for.
    largest_read: usize,
}

struct Entry {
    /// The newest activity time, or `None` when the identity's walk found no
    /// acting frame.
    time: Option<u64>,
    /// The tick of the entry's last use.
    used: u64,
}

impl Entries {
    /// Merge `time` into `identity`'s entry, creating it, and stamp it used.
    /// A merge keeps the newer time; any time is newer than none.
    fn merge(&mut self, identity: &str, time: Option<u64>) {
        self.tick += 1;
        let used = self.tick;
        if let Some(entry) = self.map.get_mut(identity) {
            entry.time = entry.time.max(time);
            entry.used = used;
        } else {
            self.map.insert(identity.to_string(), Entry { time, used });
        }
    }

    /// `identity`'s time, stamping its entry used: `None` without an entry,
    /// or when its walk found no acting frame.
    fn read(&mut self, identity: &str) -> Option<u64> {
        let entry = self.map.get_mut(identity)?;
        self.tick += 1;
        entry.used = self.tick;
        entry.time
    }

    /// The capacity in force: the configured one, or twice the most
    /// identities one read has asked for when that is more, so a console's
    /// own listing never evicts itself.
    fn capacity(&self, configured: usize) -> usize {
        configured.max(self.largest_read.saturating_mul(2))
    }
}

impl Default for LastActivityCache {
    fn default() -> Self {
        Self::with_limits(SEED_CHUNK, CAPACITY)
    }
}

impl LastActivityCache {
    /// A cache that seeds `seed_chunk` identities per store call and keeps at
    /// most `capacity` entries (both at least one).
    pub(super) fn with_limits(seed_chunk: usize, capacity: usize) -> Self {
        Self {
            entries: std::sync::Mutex::new(Entries {
                map: HashMap::new(),
                tick: 0,
                largest_read: 0,
            }),
            seeding: tokio::sync::Mutex::new(()),
            log: tokio::sync::RwLock::new(()),
            seed_chunk: seed_chunk.max(1),
            capacity: capacity.max(1),
        }
    }

    /// Each named identity's newest activity time, in milliseconds since the
    /// epoch; an identity without one is absent. Identities without an entry
    /// are walked first, a chunk per store call; the others answer from
    /// memory and touch no store. A failed chunk keeps the entries of the
    /// chunks before it.
    pub(super) async fn read(
        &self,
        store: &dyn ConsoleLogStore,
        identities: &[String],
    ) -> ConsoleLogResult<HashMap<String, u64>> {
        if !self.unseeded(identities).is_empty() {
            let _seeding = self.seeding.lock().await;
            // A seed that held the lock first may have walked some already.
            let unseeded = self.unseeded(identities);
            for (index, chunk) in unseeded.chunks(self.seed_chunk).enumerate() {
                if index > 0 {
                    // The log is free between chunks: a clear, a drop or an
                    // eviction waiting for it goes first.
                    tokio::task::yield_now().await;
                }
                let _log = self.log.read().await;
                let found = store.latest_activity_times(chunk).await?;
                self.merge_seed(chunk, &found);
            }
        }
        let known = self.known(identities);
        self.evict_over_capacity(identities.len()).await;
        Ok(known)
    }

    /// Append a frame to the log, then merge its time into its identity's
    /// entry when the store inserted it and it records the identity acting,
    /// or drop the entry when it records the identity leaving. A frame the
    /// store already held (a replayed dedupe key) changes nothing: it may be
    /// older than the identity's newest acting frame, so it cannot stand in
    /// for a walk, and a replayed departure was dropped when it was first
    /// stored.
    pub(super) async fn append(
        &self,
        store: &dyn ConsoleLogStore,
        frame: NewConsoleFrame,
    ) -> ConsoleLogResult<AppendOutcome> {
        let (outcome, acted) = {
            let _log = self.log.read().await;
            let outcome = store.append_if_absent(frame).await?;
            let acted = outcome.disposition == AppendDisposition::Inserted
                && is_activity_frame(&outcome.frame);
            if acted {
                self.entries()
                    .merge(&outcome.frame.identity, Some(outcome.frame.timestamp_ms));
            }
            (outcome, acted)
        };
        if acted {
            // The merge may have taken the map past its capacity.
            self.evict_over_capacity(0).await;
        } else if outcome.disposition == AppendDisposition::Inserted
            && is_departure_frame(&outcome.frame)
        {
            self.drop_entry(&outcome.frame.identity).await;
        }
        Ok(outcome)
    }

    /// Clear the log's frames and every entry, alone: the appends and the
    /// seed chunk in flight finish first, and none starts before both are
    /// cleared.
    pub(super) async fn clear(&self, store: &dyn ConsoleLogStore) -> ConsoleLogResult<()> {
        let _log = self.log.write().await;
        let cleared = store.clear_frames().await;
        // Emptied whatever the store answered: an entry is a summary of the
        // log, and the next read walks the log again.
        self.entries().map.clear();
        cleared
    }

    /// Drop a departed identity's entry, holding `log` alone (see the module
    /// documentation). Its next read walks the log again.
    async fn drop_entry(&self, identity: &str) {
        let _log = self.log.write().await;
        self.entries().map.remove(identity);
    }

    /// Past the capacity in force, evict the least recently used entries down
    /// to three quarters of it, holding `log` alone as a drop does. A read
    /// first counts its own identities into the capacity.
    async fn evict_over_capacity(&self, read_size: usize) {
        {
            let mut entries = self.entries();
            entries.largest_read = entries.largest_read.max(read_size);
            if entries.map.len() <= entries.capacity(self.capacity) {
                return;
            }
        }
        let _log = self.log.write().await;
        let mut entries = self.entries();
        let capacity = entries.capacity(self.capacity);
        if entries.map.len() <= capacity {
            // Another eviction went first.
            return;
        }
        let keep = capacity - capacity / 4;
        let excess = entries.map.len() - keep;
        // Every use takes a fresh tick, so exactly `excess` entries were used
        // at or before the `excess`-th oldest use.
        let mut used = entries
            .map
            .values()
            .map(|entry| entry.used)
            .collect::<Vec<_>>();
        let oldest_kept_after = *used.select_nth_unstable(excess - 1).1;
        entries
            .map
            .retain(|_, entry| entry.used > oldest_kept_after);
    }

    fn entries(&self) -> MutexGuard<'_, Entries> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The named identities without an entry, sorted and without repeats.
    fn unseeded(&self, identities: &[String]) -> Vec<String> {
        let entries = self.entries();
        let mut unseeded = identities
            .iter()
            .filter(|identity| !entries.map.contains_key(identity.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        drop(entries);
        unseeded.sort_unstable();
        unseeded.dedup();
        unseeded
    }

    /// Merge a seed chunk: every walked identity gets an entry, `None` when
    /// its walk found no acting frame.
    fn merge_seed(&self, walked: &[String], found: &HashMap<String, u64>) {
        let mut entries = self.entries();
        for identity in walked {
            entries.merge(identity, found.get(identity).copied());
        }
    }

    /// The named identities' times, each entry found stamped used.
    fn known(&self, identities: &[String]) -> HashMap<String, u64> {
        let mut entries = self.entries();
        identities
            .iter()
            .filter_map(|identity| Some((identity.clone(), entries.read(identity)?)))
            .collect()
    }
}
