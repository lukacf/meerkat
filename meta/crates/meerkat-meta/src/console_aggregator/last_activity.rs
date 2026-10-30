//! Each identity's newest activity time, kept in memory for
//! `/console/experience`.
//!
//! The console log answers an identity's newest acting frame by walking the
//! identity's frames newest first ([`ConsoleLogStore::latest_activity_times`]).
//! The walk is short when the identity acted recently and long when many of
//! its newest frames record something other than the identity acting, as
//! after a history backfill. The experience asks for every listed identity on
//! every read, so the aggregator walks each identity once, on its first read
//! (its seed), and from then on keeps its time from the frames the aggregator
//! appends itself ([`LastActivityCache::append`]). Which frames record an
//! identity acting is the store's one rule, [`is_activity_frame`].
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
//! A clear holds `log` alone, while every append and every seed hold it
//! shared from their store call to their merge. The clear waits for those in
//! flight and empties the map in the same hold, so no time merged after a
//! clear belongs to a frame the clear removed.
//!
//! # Memory
//!
//! One entry per identity this process has read or seen act: the identity
//! and an optional time. Only a clear drops entries. Retiring an identity
//! keeps its frames in the log and a respawned identity keeps its activity,
//! so an entry lives as long as the frames it summarizes.

use std::collections::HashMap;
use std::sync::{MutexGuard, PoisonError};

use super::store::{ConsoleLogResult, ConsoleLogStore, is_activity_frame};
use super::{AppendDisposition, AppendOutcome, NewConsoleFrame};

/// The aggregator's in-memory last activity, shared by every policy view of
/// one canonical projector (see the module documentation).
#[derive(Default)]
pub(super) struct LastActivityCache {
    /// Each identity's newest activity time, or `None` when its walk found
    /// no acting frame. An identity without an entry has neither been read
    /// nor acted in this process.
    times: std::sync::Mutex<HashMap<String, Option<u64>>>,
    /// Held by a seed for its walk and merge, so concurrent first reads walk
    /// each identity once.
    seeding: tokio::sync::Mutex<()>,
    /// Shared by appends and seeds from their store call to their merge;
    /// held alone by a clear.
    log: tokio::sync::RwLock<()>,
}

impl LastActivityCache {
    /// Each named identity's newest activity time, in milliseconds since the
    /// epoch; an identity without one is absent. Identities without an entry
    /// are walked first, in one store read; the others answer from memory
    /// and touch no store.
    pub(super) async fn read(
        &self,
        store: &dyn ConsoleLogStore,
        identities: &[String],
    ) -> ConsoleLogResult<HashMap<String, u64>> {
        if !self.unseeded(identities).is_empty() {
            let _seeding = self.seeding.lock().await;
            // A seed that held the lock first may have walked them already.
            let unseeded = self.unseeded(identities);
            if !unseeded.is_empty() {
                let _log = self.log.read().await;
                let found = store.latest_activity_times(&unseeded).await?;
                self.merge_seed(unseeded, &found);
            }
        }
        Ok(self.known(identities))
    }

    /// Append a frame to the log, then merge its time into its identity's
    /// entry when the store inserted it and it records the identity acting.
    /// A frame the store already held (a replayed dedupe key) is not merged:
    /// it may be older than the identity's newest acting frame, so it cannot
    /// stand in for a walk.
    pub(super) async fn append(
        &self,
        store: &dyn ConsoleLogStore,
        frame: NewConsoleFrame,
    ) -> ConsoleLogResult<AppendOutcome> {
        let _log = self.log.read().await;
        let outcome = store.append_if_absent(frame).await?;
        if outcome.disposition == AppendDisposition::Inserted && is_activity_frame(&outcome.frame) {
            self.record(&outcome.frame.identity, outcome.frame.timestamp_ms);
        }
        Ok(outcome)
    }

    /// Clear the log's frames and every entry, alone: the appends and seeds
    /// in flight finish first, and none starts before both are cleared.
    pub(super) async fn clear(&self, store: &dyn ConsoleLogStore) -> ConsoleLogResult<()> {
        let _log = self.log.write().await;
        let cleared = store.clear_frames().await;
        // Emptied whatever the store answered: an entry is a summary of the
        // log, and the next read walks the log again.
        self.times().clear();
        cleared
    }

    fn times(&self) -> MutexGuard<'_, HashMap<String, Option<u64>>> {
        self.times.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The named identities without an entry, sorted and without repeats.
    fn unseeded(&self, identities: &[String]) -> Vec<String> {
        let times = self.times();
        let mut unseeded = identities
            .iter()
            .filter(|identity| !times.contains_key(identity.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        drop(times);
        unseeded.sort_unstable();
        unseeded.dedup();
        unseeded
    }

    /// Merge a seed: every walked identity gets an entry, `None` when its
    /// walk found no acting frame.
    fn merge_seed(&self, walked: Vec<String>, found: &HashMap<String, u64>) {
        let mut times = self.times();
        for identity in walked {
            let time = found.get(&identity).copied();
            let entry = times.entry(identity).or_insert(None);
            *entry = (*entry).max(time);
        }
    }

    /// Merge an acting append's time.
    fn record(&self, identity: &str, time: u64) {
        let mut times = self.times();
        if let Some(entry) = times.get_mut(identity) {
            *entry = (*entry).max(Some(time));
        } else {
            times.insert(identity.to_string(), Some(time));
        }
    }

    fn known(&self, identities: &[String]) -> HashMap<String, u64> {
        let times = self.times();
        identities
            .iter()
            .filter_map(|identity| Some((identity.clone(), (*times.get(identity)?)?)))
            .collect()
    }
}
