//! The journal's latest placed-completion quiesce marker, folded by cursor.

use crate::event::{MobEvent, MobEventKind};
use crate::ids::MobId;
use crate::machines::mob_machine as mob_dsl;
use crate::runtime::builder::LaterLogRead;
use crate::store::{MobEventStore, MobStoreError};

/// The latest quiesce marker a mob's journal holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime) enum LatestQuiesceMarker {
    /// The journal holds no quiesce marker and no reset.
    Absent,
    /// A `PlacedCompletionLifecycleQuiesceStarted` of this intent, with
    /// nothing after it that closes it.
    Started(mob_dsl::PlacedCompletionLifecycleIntentKind),
    /// A `PlacedCompletionLifecycleQuiesceEnded` or a `MobReset`: whatever
    /// was open is closed.
    Closed,
}

/// A fold of the mob's journal for the quiesce marker's idempotency check
/// (`MobActor::ensure_placed_completion_quiesce_marker`): the latest
/// quiesce marker the journal holds for the mob, which the check read from
/// a whole replay before. It is rebuilt from the log at every start and
/// caught up from the journal before every check, so it is a cache of the
/// journal's answer and never a lifecycle fact: MobMachine holds the
/// lifecycle intent.
pub(in crate::runtime) struct PlacedCompletionQuiesceMarkerFold {
    /// The last journal event folded, of any mob (`None`: none yet).
    last: Option<MobEvent>,
    latest: LatestQuiesceMarker,
}

impl PlacedCompletionQuiesceMarkerFold {
    /// The fold of `replayed`, the whole journal in cursor order as the
    /// actor's start read it.
    pub(in crate::runtime) fn seeded(replayed: &[MobEvent], mob_id: &MobId) -> Self {
        let mut fold = Self {
            last: None,
            latest: LatestQuiesceMarker::Absent,
        };
        fold.fold_events(replayed, mob_id);
        fold
    }

    /// Catch the fold up to the journal and answer its latest marker: the
    /// events appended since the last one folded, or the whole journal when
    /// the store no longer holds that event where it was read.
    pub(in crate::runtime) async fn refreshed(
        &mut self,
        events: &dyn MobEventStore,
        mob_id: &MobId,
    ) -> Result<LatestQuiesceMarker, MobStoreError> {
        match LaterLogRead::after(events, self.last.as_ref()).await? {
            LaterLogRead::Appended(appended) => self.fold_events(&appended, mob_id),
            LaterLogRead::Whole(whole) => *self = Self::seeded(&whole, mob_id),
        }
        Ok(self.latest)
    }

    /// Fold `events`, a journal slice in cursor order, as the check read a
    /// whole replay: the last marker of the mob's own events.
    fn fold_events(&mut self, events: &[MobEvent], mob_id: &MobId) {
        for event in events.iter().filter(|event| &event.mob_id == mob_id) {
            match &event.kind {
                MobEventKind::PlacedCompletionLifecycleQuiesceStarted { intent } => {
                    self.latest = LatestQuiesceMarker::Started(*intent);
                }
                MobEventKind::PlacedCompletionLifecycleQuiesceEnded { .. }
                | MobEventKind::MobReset => self.latest = LatestQuiesceMarker::Closed,
                _ => {}
            }
        }
        if let Some(last) = events.last() {
            self.last = Some(last.clone());
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::event::NewMobEvent;
    use crate::store::InMemoryMobEventStore;
    use mob_dsl::PlacedCompletionLifecycleIntentKind as Intent;
    use std::sync::Arc;

    fn started(intent: Intent) -> MobEventKind {
        MobEventKind::PlacedCompletionLifecycleQuiesceStarted { intent }
    }

    fn ended(intent: Intent) -> MobEventKind {
        MobEventKind::PlacedCompletionLifecycleQuiesceEnded { intent }
    }

    fn quiet() -> MobEventKind {
        MobEventKind::TopologyViolation {
            from_role: crate::ids::ProfileName::from("worker"),
            to_role: crate::ids::ProfileName::from("lead"),
        }
    }

    /// The latest quiesce marker as the check read it from a whole replay
    /// before the fold existed, written out again here so the fold is held
    /// against that reading and not against itself.
    fn whole_replay_reading(events: &[MobEvent], mob_id: &MobId) -> LatestQuiesceMarker {
        let latest = events
            .iter()
            .filter(|event| event.mob_id == *mob_id)
            .filter_map(|event| match event.kind {
                MobEventKind::PlacedCompletionLifecycleQuiesceStarted { intent: recorded } => {
                    Some(Some(recorded))
                }
                MobEventKind::PlacedCompletionLifecycleQuiesceEnded { .. }
                | MobEventKind::MobReset => Some(None),
                _ => None,
            })
            .next_back();
        match latest {
            None => LatestQuiesceMarker::Absent,
            Some(None) => LatestQuiesceMarker::Closed,
            Some(Some(intent)) => LatestQuiesceMarker::Started(intent),
        }
    }

    /// Over a journal with several quiesce markers, of the mob and of
    /// another mob, and events appended after the seed, the fold caught up
    /// from the journal answers at every step what a whole replay answers.
    #[tokio::test]
    async fn the_quiesce_marker_fold_answers_what_a_whole_replay_answers_at_every_step() {
        let mob_id = MobId::from("quiesce-fold");
        let neighbour = MobId::from("quiesce-fold-neighbour");
        let store: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
        let before_seed = vec![
            (&mob_id, quiet()),
            (&mob_id, started(Intent::Stop)),
            (&neighbour, started(Intent::Complete)),
            (&mob_id, ended(Intent::Stop)),
            (&mob_id, quiet()),
        ];
        for (mob, kind) in before_seed {
            store
                .append(NewMobEvent::for_test(mob.clone(), kind))
                .await
                .expect("append before the seed");
        }
        let replayed = store.replay_all().await.expect("replay the journal");
        let mut fold = PlacedCompletionQuiesceMarkerFold::seeded(&replayed, &mob_id);
        assert_eq!(
            fold.latest,
            whole_replay_reading(&replayed, &mob_id),
            "the seed holds what the whole replay reads"
        );
        assert_eq!(
            fold.latest,
            LatestQuiesceMarker::Closed,
            "the seed ends at the mob's End"
        );

        let after_seed = vec![
            (&mob_id, started(Intent::Stop)),
            (&neighbour, ended(Intent::Complete)),
            (&mob_id, quiet()),
            (&mob_id, ended(Intent::Stop)),
            (&mob_id, started(Intent::Destroy)),
            (&neighbour, MobEventKind::MobReset),
            (&mob_id, MobEventKind::MobReset),
            (&mob_id, started(Intent::Complete)),
        ];
        let mut answers = Vec::new();
        for (mob, kind) in after_seed {
            store
                .append(NewMobEvent::for_test(mob.clone(), kind))
                .await
                .expect("append after the seed");
            let answer = fold
                .refreshed(store.as_ref(), &mob_id)
                .await
                .expect("catch the fold up");
            let journal = store.replay_all().await.expect("replay the journal");
            assert_eq!(
                answer,
                whole_replay_reading(&journal, &mob_id),
                "after {} journal events",
                journal.len()
            );
            answers.push(answer);
        }
        assert_eq!(
            answers,
            vec![
                LatestQuiesceMarker::Started(Intent::Stop),
                LatestQuiesceMarker::Started(Intent::Stop),
                LatestQuiesceMarker::Started(Intent::Stop),
                LatestQuiesceMarker::Closed,
                LatestQuiesceMarker::Started(Intent::Destroy),
                LatestQuiesceMarker::Started(Intent::Destroy),
                LatestQuiesceMarker::Closed,
                LatestQuiesceMarker::Started(Intent::Complete),
            ],
            "the other mob's markers and reset never move this mob's answer"
        );
    }

    /// A journal cleared and written again answers its own latest marker:
    /// the fold finds the event it ended at gone and folds the journal
    /// whole.
    #[tokio::test]
    async fn the_quiesce_marker_fold_reads_a_rewritten_journal_whole() {
        let mob_id = MobId::from("quiesce-fold-cleared");
        let store: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
        for kind in [quiet(), started(Intent::Stop), quiet()] {
            store
                .append(NewMobEvent::for_test(mob_id.clone(), kind))
                .await
                .expect("append");
        }
        let replayed = store.replay_all().await.expect("replay the journal");
        let mut fold = PlacedCompletionQuiesceMarkerFold::seeded(&replayed, &mob_id);
        assert_eq!(fold.latest, LatestQuiesceMarker::Started(Intent::Stop));

        store.clear().await.expect("clear the journal");
        for kind in [
            quiet(),
            ended(Intent::Stop),
            started(Intent::Complete),
            quiet(),
        ] {
            store
                .append(NewMobEvent::for_test(mob_id.clone(), kind))
                .await
                .expect("append to the new journal");
        }
        let answer = fold
            .refreshed(store.as_ref(), &mob_id)
            .await
            .expect("catch the fold up");
        let journal = store.replay_all().await.expect("replay the journal");
        assert_eq!(answer, whole_replay_reading(&journal, &mob_id));
        assert_eq!(
            answer,
            LatestQuiesceMarker::Started(Intent::Complete),
            "the new journal's own latest marker"
        );
    }
}
