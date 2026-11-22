//! The writers under the ids of the mobs a host serves beside its default
//! mob, held to the registration they write for.
//!
//! A stored frame names its mob's id and no incarnation. A mob created
//! under a destroyed mob's id is kept from the earlier mob's frames by one
//! thing: the frames under the id are removed before it is built. That
//! holds only if no write of the earlier mob is in flight when they are
//! removed, and none can start afterwards. This module is that guard.
//!
//! Each registration of a mob's console events
//! ([`super::MetaConsoleAggregator::register_mob_console_events`]) opens a
//! GATE under the mob's runtime key. Whatever writes a frame under a mob's
//! id takes a PERMIT from the gate of the registration it was STARTED
//! under, never from one it finds under the key when it writes: a
//! registration that ended gives none, and another registration's permit
//! is not asked for. A permit is held across the write and counted, so the
//! host can wait until a mob's writers are at rest before it removes the
//! frames under the id ([`MobWriters::rest`]).
//!
//! The gates belong to the HOST, not to one console projection: the runtime
//! hands the same registry to every projection it builds, so a write of a
//! projection that was replaced is still counted, and a policy view writes
//! through its owner's.
//!
//! A gate also says where the sweep of its mob's unsettled sends stands for
//! its registration: the registration claims the sweep once
//! ([`MobWriters::claim_sweep`]), and a sweep that did not run to its end
//! is owed again ([`MobWriters::sweep_over`]). The fact is the gate's: it
//! ends with the registration, and it goes with the gates of the key when
//! the frames under the id are removed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use meerkat_core::service::MobIncarnationId;
use meerkat_mob::MobId;

/// The runtime key the console events of the mob `mob` are registered
/// under, which is the key of every frame of that mob.
pub(crate) fn runtime_key_of(mob: &MobId) -> String {
    format!("mob/{}", mob.as_str())
}

/// One registration of a mob's console events, as its writers see it.
struct Gate {
    registration: uuid::Uuid,
    incarnation: MobIncarnationId,
    /// The registration ended: it gives no permit from here on.
    ended: bool,
    /// The writes of this registration that hold a permit now.
    in_flight: tokio::sync::watch::Sender<usize>,
    /// The cursor of the last console event this registration wrote: its
    /// own, kept here and in no store (a store's watermark under the key
    /// would be read by whatever mob holds the id next).
    cursor: Option<String>,
    /// Where the sweep of the mob's sends a previous process left unsettled
    /// stands for this registration.
    sweep: Sweep,
}

/// The sweep of a registration's mob's unsettled sends
/// ([`super::MetaConsoleAggregator::register_mob_console_events`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sweep {
    /// The registration's sweep is owed: it has not run, or a run did not
    /// complete (a list that failed, the deadline, the limit, a send whose
    /// dispatch another owner held, evidence that was not read, a send that
    /// could not be settled, the registration's end).
    Due,
    /// The registration's sweep runs.
    Running,
    /// A sweep of the mob ran to its end for this registration, or for the
    /// standing registration this one took the place of.
    Completed,
}

impl Gate {
    fn at_rest(&self) -> bool {
        self.ended && *self.in_flight.borrow() == 0
    }
}

/// The gates of a host's mobs, by runtime key. A key holds more than one
/// while an ended registration still has a write in flight beside the one
/// that stands.
#[derive(Clone, Default)]
pub(crate) struct MobWriters(Arc<Mutex<BTreeMap<String, Vec<Gate>>>>);

/// A registration was asked under a key that still holds the gate of
/// ANOTHER incarnation: the frames under the id were not removed since
/// that mob wrote them, so nothing may be registered over them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnotherIncarnationStands {
    pub(crate) runtime_key: String,
}

impl std::fmt::Display for AnotherIncarnationStands {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the console events of another incarnation of the mob were registered under `{}` and \
             its frames were not removed: no mob is registered over them",
            self.runtime_key
        )
    }
}

impl std::error::Error for AnotherIncarnationStands {}

/// One write under a mob's id, in flight for its registration. Dropped when
/// the write is over.
pub(crate) struct WritePermit(tokio::sync::watch::Sender<usize>);

impl Drop for WritePermit {
    fn drop(&mut self) {
        self.0
            .send_modify(|in_flight| *in_flight = in_flight.saturating_sub(1));
    }
}

impl MobWriters {
    fn gates(&self) -> MutexGuard<'_, BTreeMap<String, Vec<Gate>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Open the gate of the registration `registration` of the mob at
    /// `incarnation` under `runtime_key`. Refused while the key holds a gate
    /// of another incarnation ([`AnotherIncarnationStands`]). An earlier
    /// registration of the same incarnation that has not ended ends here:
    /// the same mob registered again takes its place.
    pub(crate) fn open(
        &self,
        runtime_key: &str,
        registration: uuid::Uuid,
        incarnation: MobIncarnationId,
    ) -> Result<(), AnotherIncarnationStands> {
        let mut gates = self.gates();
        let of_the_key = gates.entry(runtime_key.to_string()).or_default();
        if of_the_key
            .iter()
            .any(|gate| gate.incarnation != incarnation)
        {
            return Err(AnotherIncarnationStands {
                runtime_key: runtime_key.to_string(),
            });
        }
        // A registration that takes the place of a STANDING one whose sweep
        // completed owes none; after a registration that ENDED (the mob was
        // closed and is loaded again) the sweep is owed again.
        let sweep = if of_the_key
            .iter()
            .any(|gate| !gate.ended && gate.sweep == Sweep::Completed)
        {
            Sweep::Completed
        } else {
            Sweep::Due
        };
        of_the_key.retain(|gate| !gate.at_rest());
        for gate in of_the_key.iter_mut() {
            gate.ended = true;
        }
        of_the_key.push(Gate {
            registration,
            incarnation,
            ended: false,
            in_flight: tokio::sync::watch::channel(0).0,
            cursor: None,
            sweep,
        });
        Ok(())
    }

    /// Whether the sweep of the mob's sends is the registration
    /// `registration`'s to run now: `true` once, while the registration
    /// stands and owes it ([`Sweep::Due`]); the sweep is then running.
    pub(crate) fn claim_sweep(&self, runtime_key: &str, registration: uuid::Uuid) -> bool {
        let mut gates = self.gates();
        let Some(gate) = gates.get_mut(runtime_key).and_then(|gates| {
            gates
                .iter_mut()
                .find(|gate| gate.registration == registration)
        }) else {
            return false;
        };
        if gate.ended || gate.sweep != Sweep::Due {
            return false;
        }
        gate.sweep = Sweep::Running;
        true
    }

    /// The registration `registration`'s sweep is over: `completed` when it
    /// ran to its end. Otherwise the gate is due again, and the sweep is run
    /// by the registration that takes this one's place: a registration
    /// claims its sweep once, where it is made, and nothing claims again
    /// for a registration that stands.
    pub(crate) fn sweep_over(&self, runtime_key: &str, registration: uuid::Uuid, completed: bool) {
        if let Some(gate) = self.gates().get_mut(runtime_key).and_then(|gates| {
            gates
                .iter_mut()
                .find(|gate| gate.registration == registration)
        }) {
            gate.sweep = if completed {
                Sweep::Completed
            } else {
                Sweep::Due
            };
        }
    }

    /// Test seam: where the sweep of the registration `registration` under
    /// `runtime_key` stands; none for a registration the key does not hold.
    #[cfg(test)]
    pub(crate) fn sweep(&self, runtime_key: &str, registration: uuid::Uuid) -> Option<Sweep> {
        self.gates()
            .get(runtime_key)?
            .iter()
            .find(|gate| gate.registration == registration)
            .map(|gate| gate.sweep)
    }

    /// A permit for one write of the registration `registration` under
    /// `runtime_key`; none once that registration ended, and none for a
    /// registration the key does not hold.
    pub(crate) fn permit(
        &self,
        runtime_key: &str,
        registration: uuid::Uuid,
    ) -> Option<WritePermit> {
        let gates = self.gates();
        let gate = gates
            .get(runtime_key)?
            .iter()
            .find(|gate| gate.registration == registration)?;
        if gate.ended {
            return None;
        }
        gate.in_flight.send_modify(|in_flight| *in_flight += 1);
        Some(WritePermit(gate.in_flight.clone()))
    }

    /// The registration `registration` under `runtime_key` ended: it gives
    /// no permit from here on. Its writes in flight go on to their end.
    /// Another registration under the key is left as it is.
    pub(crate) fn end(&self, runtime_key: &str, registration: uuid::Uuid) {
        if let Some(gate) = self.gates().get_mut(runtime_key).and_then(|gates| {
            gates
                .iter_mut()
                .find(|gate| gate.registration == registration)
        }) {
            gate.ended = true;
        }
    }

    /// Whether the writers under `runtime_key` are at rest: every
    /// registration under it ended and none of their writes is in flight.
    /// Waits up to `bound` for the writes in flight. A registration that
    /// has not ended is not at rest, and no wait makes it so.
    pub(crate) async fn rest(&self, runtime_key: &str, bound: Duration) -> bool {
        let in_flight: Vec<tokio::sync::watch::Receiver<usize>> = {
            let gates = self.gates();
            let Some(of_the_key) = gates.get(runtime_key) else {
                return true;
            };
            if of_the_key.iter().any(|gate| !gate.ended) {
                return false;
            }
            of_the_key
                .iter()
                .map(|gate| gate.in_flight.subscribe())
                .collect()
        };
        let drained = async {
            for mut writes in in_flight {
                // A sender that is gone has no write left to wait for.
                let _ = writes.wait_for(|in_flight| *in_flight == 0).await;
            }
        };
        tokio::time::timeout(bound, drained).await.is_ok()
    }

    /// The frames under `runtime_key` were removed: the gates of the
    /// registrations that wrote them go. Only gates at rest go; one that is
    /// not stays and keeps refusing another incarnation.
    pub(crate) fn released(&self, runtime_key: &str) {
        let mut gates = self.gates();
        if let Some(of_the_key) = gates.get_mut(runtime_key) {
            of_the_key.retain(|gate| !gate.at_rest());
            if of_the_key.is_empty() {
                gates.remove(runtime_key);
            }
        }
    }

    /// The cursor of the last console event the registration wrote.
    pub(crate) fn cursor(&self, runtime_key: &str, registration: uuid::Uuid) -> Option<String> {
        self.gates()
            .get(runtime_key)?
            .iter()
            .find(|gate| gate.registration == registration)?
            .cursor
            .clone()
    }

    /// The registration wrote the console event at `cursor`.
    pub(crate) fn wrote(&self, runtime_key: &str, registration: uuid::Uuid, cursor: &str) {
        if let Some(gate) = self.gates().get_mut(runtime_key).and_then(|gates| {
            gates
                .iter_mut()
                .find(|gate| gate.registration == registration)
        }) {
            gate.cursor = Some(cursor.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "mob/second";
    /// Long enough for a test's own steps, never reached by a passing row.
    const A_MOMENT: Duration = Duration::from_millis(50);

    fn registration() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    /// A permit is given to the registration that asks with ITS id, and to
    /// no other: a registration that ended gets none while another
    /// registration stands under its key and gets them. A writer started
    /// under the earlier registration therefore writes nothing under the
    /// later one, though it asks under the same key.
    #[test]
    fn an_ended_registrations_permit_is_refused_while_another_stands_under_its_key() {
        let writers = MobWriters::default();
        let incarnation = MobIncarnationId::mint();
        let (earlier, later) = (registration(), registration());
        assert_eq!(writers.open(KEY, earlier, incarnation), Ok(()));
        writers.end(KEY, earlier);
        assert_eq!(writers.open(KEY, later, incarnation), Ok(()));

        assert!(
            writers.permit(KEY, earlier).is_none(),
            "the ended registration's writer is given no permit, though a registration stands \
             under its key"
        );
        assert!(
            writers.permit(KEY, later).is_some(),
            "the registration that stands is given its permit"
        );
        assert!(
            writers.permit(KEY, registration()).is_none(),
            "a registration the key never held is given none"
        );
        assert!(
            writers.permit("mob/third", later).is_none(),
            "a registration is given none under another key"
        );
    }

    /// The same mob registered again takes the earlier registration's
    /// place: the earlier one ends there, with no end of its own.
    #[test]
    fn a_second_registration_of_one_incarnation_ends_the_first() {
        let writers = MobWriters::default();
        let incarnation = MobIncarnationId::mint();
        let (earlier, later) = (registration(), registration());
        assert_eq!(writers.open(KEY, earlier, incarnation), Ok(()));
        assert_eq!(writers.open(KEY, later, incarnation), Ok(()));
        assert!(writers.permit(KEY, earlier).is_none());
        assert!(writers.permit(KEY, later).is_some());
    }

    /// The end of one registration leaves another under its key as it is:
    /// an end that comes late, after the same mob registered again, ends
    /// the registration it names and not the one that stands.
    #[test]
    fn the_end_of_a_registration_leaves_another_under_its_key_as_it_is() {
        let writers = MobWriters::default();
        let incarnation = MobIncarnationId::mint();
        let (earlier, later) = (registration(), registration());
        assert_eq!(writers.open(KEY, earlier, incarnation), Ok(()));
        assert_eq!(writers.open(KEY, later, incarnation), Ok(()));
        writers.end(KEY, earlier);
        assert!(
            writers.permit(KEY, later).is_some(),
            "the registration that stands still writes after the earlier one's end"
        );
        writers.end(KEY, later);
        assert!(writers.permit(KEY, later).is_none());
    }

    /// A registration of ANOTHER incarnation is refused while the key holds
    /// a gate of the earlier one, at rest or not: the frames under the id
    /// were not removed. Once the host says they were, it is made.
    #[tokio::test]
    async fn another_incarnation_is_refused_until_the_frames_under_the_id_were_removed() {
        let writers = MobWriters::default();
        let (earlier, later) = (registration(), registration());
        assert_eq!(writers.open(KEY, earlier, MobIncarnationId::mint()), Ok(()));
        let successor = MobIncarnationId::mint();
        let refused = Err(AnotherIncarnationStands {
            runtime_key: KEY.to_string(),
        });
        assert_eq!(
            writers.open(KEY, later, successor),
            refused,
            "refused while the earlier incarnation's registration stands"
        );
        writers.end(KEY, earlier);
        assert!(
            writers.rest(KEY, A_MOMENT).await,
            "ended, nothing in flight"
        );
        assert_eq!(
            writers.open(KEY, later, successor),
            refused,
            "refused at rest too: the earlier incarnation's frames are still under the id"
        );
        assert!(
            writers.permit(KEY, later).is_none(),
            "a refused registration has no gate and is given no permit"
        );
        writers.released(KEY);
        assert_eq!(
            writers.open(KEY, later, successor),
            Ok(()),
            "made once the frames under the id were removed"
        );
        assert!(writers.permit(KEY, later).is_some());
    }

    /// A write in flight keeps its registration's key from rest until it is
    /// over, also after the registration ended; and a registration that has
    /// not ended is not at rest, however long one waits.
    #[tokio::test]
    async fn a_write_in_flight_keeps_its_key_from_rest_until_it_is_over() {
        let writers = MobWriters::default();
        let incarnation = MobIncarnationId::mint();
        let earlier = registration();
        assert!(
            writers.rest(KEY, Duration::ZERO).await,
            "a key no registration was made under is at rest"
        );
        assert_eq!(writers.open(KEY, earlier, incarnation), Ok(()));
        assert!(
            !writers.rest(KEY, A_MOMENT).await,
            "a registration that has not ended is not at rest"
        );
        let in_flight = writers.permit(KEY, earlier);
        assert!(in_flight.is_some());
        writers.end(KEY, earlier);
        assert!(
            !writers.rest(KEY, A_MOMENT).await,
            "the ended registration's write is still in flight"
        );
        // The gate of a write in flight is not forgotten with the frames:
        // it stays and goes on refusing another incarnation.
        writers.released(KEY);
        assert_eq!(
            writers.open(KEY, registration(), MobIncarnationId::mint()),
            Err(AnotherIncarnationStands {
                runtime_key: KEY.to_string()
            }),
        );
        assert!(!writers.rest(KEY, A_MOMENT).await);

        let waiting = {
            let writers = writers.clone();
            tokio::spawn(async move { writers.rest(KEY, Duration::from_secs(30)).await })
        };
        tokio::task::yield_now().await;
        assert!(
            !waiting.is_finished(),
            "the wait goes on while the write is in flight"
        );
        drop(in_flight);
        assert!(
            matches!(waiting.await, Ok(true)),
            "the wait ends with the write"
        );
        assert!(writers.rest(KEY, Duration::ZERO).await);
    }

    /// A registration's cursor is its own: another registration under the
    /// key, of the same mob, reads none of it.
    #[test]
    fn a_registrations_cursor_is_its_own() {
        let writers = MobWriters::default();
        let incarnation = MobIncarnationId::mint();
        let (earlier, later) = (registration(), registration());
        assert_eq!(writers.open(KEY, earlier, incarnation), Ok(()));
        assert_eq!(writers.cursor(KEY, earlier), None);
        writers.wrote(KEY, earlier, "7");
        assert_eq!(writers.cursor(KEY, earlier).as_deref(), Some("7"));
        let held = writers.permit(KEY, earlier);
        assert_eq!(writers.open(KEY, later, incarnation), Ok(()));
        assert_eq!(
            writers.cursor(KEY, later),
            None,
            "the later registration starts with no cursor"
        );
        writers.wrote(KEY, later, "9");
        assert_eq!(writers.cursor(KEY, earlier).as_deref(), Some("7"));
        drop(held);
    }

    /// The sweep of a mob's sends is its standing registration's to run,
    /// once: a claim while the sweep runs or after it completed is refused;
    /// a registration that ended claims none, and neither does one the key
    /// does not hold.
    #[test]
    fn a_registrations_sweep_is_claimed_once_while_it_stands() {
        let writers = MobWriters::default();
        let incarnation = MobIncarnationId::mint();
        let (ended, standing) = (registration(), registration());
        assert_eq!(writers.open(KEY, ended, incarnation), Ok(()));
        writers.end(KEY, ended);
        assert!(
            !writers.claim_sweep(KEY, ended),
            "a registration that ended claims no sweep"
        );
        assert_eq!(writers.open(KEY, standing, incarnation), Ok(()));
        assert!(
            writers.claim_sweep(KEY, standing),
            "the standing registration's sweep is its to run"
        );
        assert!(
            !writers.claim_sweep(KEY, standing),
            "not again while it runs"
        );
        writers.sweep_over(KEY, standing, true);
        assert!(
            !writers.claim_sweep(KEY, standing),
            "not again after it completed"
        );
        assert!(
            !writers.claim_sweep(KEY, registration()),
            "a registration the key does not hold claims none"
        );
        assert!(
            !writers.claim_sweep("mob/third", standing),
            "a registration claims none under another key"
        );
    }

    /// A sweep that did not run to its end is owed again: by its own
    /// registration, and by the registration that takes its place.
    #[test]
    fn a_sweep_that_did_not_complete_is_owed_again() {
        let writers = MobWriters::default();
        let incarnation = MobIncarnationId::mint();
        let (earlier, later) = (registration(), registration());
        assert_eq!(writers.open(KEY, earlier, incarnation), Ok(()));
        assert!(writers.claim_sweep(KEY, earlier));
        writers.sweep_over(KEY, earlier, false);
        assert!(
            writers.claim_sweep(KEY, earlier),
            "owed again by the registration whose sweep did not complete"
        );
        writers.sweep_over(KEY, earlier, false);
        assert_eq!(writers.open(KEY, later, incarnation), Ok(()));
        assert!(
            writers.claim_sweep(KEY, later),
            "owed by the registration that takes its place"
        );
    }

    /// A completed sweep is kept by a registration that takes the place of
    /// the STANDING registration it completed for, and by no other: a
    /// registration made after one that ended owes the sweep again, though
    /// the ended registration's gate is still under the key.
    #[test]
    fn a_completed_sweep_is_kept_by_a_registration_in_a_standing_ones_place_alone() {
        let writers = MobWriters::default();
        let incarnation = MobIncarnationId::mint();
        let (first, second, third) = (registration(), registration(), registration());
        assert_eq!(writers.open(KEY, first, incarnation), Ok(()));
        assert!(writers.claim_sweep(KEY, first));
        writers.sweep_over(KEY, first, true);
        assert_eq!(writers.open(KEY, second, incarnation), Ok(()));
        assert!(
            !writers.claim_sweep(KEY, second),
            "in a standing registration's place after its completed sweep: none owed"
        );
        writers.end(KEY, second);
        assert_eq!(writers.open(KEY, third, incarnation), Ok(()));
        assert!(
            writers.claim_sweep(KEY, third),
            "after a registration that ended (the mob was closed and is loaded again): owed"
        );
    }
}
