//! `ConsoleLogStore` profile of a host that serves a mob beside its default
//! mob: the removal of one mob's frames, the read of one mob's frames, the
//! window of the default mob's unsettled sends, and the operations on one
//! mob's sends that name the mob.
//!
//! A host that creates a mob beside its default mob first removes what its
//! console log keeps under that mob's id, and refuses the create when the
//! store refuses the removal. A store that keeps the trait's own
//! `remove_frames_of_mob`, which answers an error, fails this chapter, as
//! such a host refuses every such create over it.

use meerkat_meta::console_aggregator::{
    ConsoleFrame, ConsoleFrameSourceKind, ConsoleFrameStatus, ConsoleLogStore,
    ConsoleTimelineWindowQuery, NewConsoleFrame, SendTransition,
};
use meerkat_meta::member_key::DefaultMob;
use meerkat_store_conformance::ConformanceFailure;

use crate::factory::ConsoleLogStoreFactory;
use crate::fixtures;
use crate::steps::Steps;

const CHAPTER: &str = "console_log_mobs";

/// The default mob the store is told. It is the one the `console_log`
/// chapter tells, so both chapters run over one handle.
const DEFAULT_MOB: &str = "conformance-default-mob";

pub async fn console_log_mobs(
    factory: &dyn ConsoleLogStoreFactory,
) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let store = factory.open().await?;

    removal_of_one_mobs_frames(&steps, store.as_ref()).await?;
    read_of_one_mob(&steps, store.as_ref()).await?;
    window_of_the_default_mobs_unsettled_sends(&steps, store.as_ref()).await?;
    sends_of_one_mob(&steps, store.as_ref()).await?;
    Ok(())
}

/// `remove_frames_of_mob` removes the frames of one mob, whoever their
/// member, and answers how many went. Every other frame stays as it is and
/// keeps its cursor, a cursor that was given out is not given to a later
/// frame, a mob with no frame has none to remove, and the default mob's
/// frames are never removed: a blank id and the id of the default mob the
/// store was told are refused.
async fn removal_of_one_mobs_frames(
    steps: &Steps,
    store: &dyn ConsoleLogStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "removal_of_one_mobs_frames";
    const IDENTITY: &str = "console:removal-probe";
    const REMOVED_MOB: &str = "conformance-removed-mob";
    const KEPT_MOB: &str = "conformance-kept-mob";

    let mut given = Vec::new();
    let mut kept = Vec::new();
    for (key, mob, timestamp_ms) in [
        ("conformance-removal-default", None, 2_000),
        ("conformance-removal-first", Some(REMOVED_MOB), 2_001),
        ("conformance-removal-kept", Some(KEPT_MOB), 2_002),
        ("conformance-removal-second", Some(REMOVED_MOB), 2_003),
    ] {
        let mut frame = fixtures::console_frame(key, IDENTITY, timestamp_ms);
        frame.mob_id = mob.map(Into::into);
        let stored = steps.wrap(STEP, store.append_if_absent(frame).await)?;
        given.push(stored.frame.cursor.clone());
        if mob != Some(REMOVED_MOB) {
            kept.push((key, stored.frame.cursor, stored.frame.mob_id));
        }
    }

    let removed = steps.wrap(STEP, store.remove_frames_of_mob(&REMOVED_MOB.into()).await)?;
    steps.ensure(
        STEP,
        removed == 2,
        format!("the removal must answer how many frames of the mob went: got {removed}"),
    )?;
    for key in ["conformance-removal-first", "conformance-removal-second"] {
        let found = steps.wrap(STEP, store.frame_by_dedupe_key(key).await)?;
        steps.ensure(
            STEP,
            found.is_none(),
            format!("a removed mob's frame must not be found by its key ({key})"),
        )?;
    }
    let of_the_member = steps.wrap(
        STEP,
        store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some(IDENTITY.to_string()),
                mob_id: Some(REMOVED_MOB.into()),
                ..ConsoleTimelineWindowQuery::default()
            })
            .await,
    )?;
    steps.ensure(
        STEP,
        of_the_member.frames.is_empty(),
        "a read of a removed mob's member must answer no frame",
    )?;
    // Each frame that stays is read by the read that names its mob.
    // `frame_by_dedupe_key` names no mob: above it is asked only where no
    // store may answer a frame (a removed mob's keys), and it is not asked
    // to answer another mob's frame, since the two stores of `meerkat-meta`
    // differ there (the SQLite store answers the default mob's frame alone,
    // the store in memory a frame of any mob) and no door of a host serves
    // the difference.
    for (key, cursor, mob_id) in &kept {
        let of_the_member = steps.wrap(
            STEP,
            store
                .query_windowed_frames(ConsoleTimelineWindowQuery {
                    identity: Some(IDENTITY.to_string()),
                    mob_id: mob_id.clone(),
                    ..ConsoleTimelineWindowQuery::default()
                })
                .await,
        )?;
        let found = of_the_member
            .frames
            .iter()
            .find(|frame| frame.dedupe_key == *key)
            .ok_or_else(|| {
                steps.fail(
                    STEP,
                    format!("the removal of one mob's frames must leave every other frame ({key})"),
                )
            })?;
        steps.ensure(
            STEP,
            found.cursor == *cursor && found.mob_id == *mob_id,
            format!("a frame that stays must keep its cursor and its mob ({key})"),
        )?;
    }

    let again = steps.wrap(STEP, store.remove_frames_of_mob(&REMOVED_MOB.into()).await)?;
    steps.ensure(
        STEP,
        again == 0,
        format!("a mob with no frame has none to remove: got {again}"),
    )?;

    let mut later = fixtures::console_frame("conformance-removal-later", IDENTITY, 2_004);
    later.mob_id = Some(REMOVED_MOB.into());
    let later = steps.wrap(STEP, store.append_if_absent(later).await)?;
    steps.ensure(
        STEP,
        !given.contains(&later.frame.cursor),
        "a cursor that was given out must not be given to a later frame, also after a removal",
    )?;

    steps.ensure(
        STEP,
        store.remove_frames_of_mob(&"".into()).await.is_err(),
        "a blank id names no mob: the removal must refuse it",
    )?;
    steps.wrap(
        STEP,
        store.bind_default_mob(&DefaultMob::new(DEFAULT_MOB.into())),
    )?;
    steps.ensure(
        STEP,
        store
            .remove_frames_of_mob(&DEFAULT_MOB.into())
            .await
            .is_err(),
        "the default mob's frames are never removed: the removal must refuse the id of the \
         default mob the store was told",
    )?;
    let of_the_default = steps.wrap(
        STEP,
        store
            .frame_by_dedupe_key("conformance-removal-default")
            .await,
    )?;
    steps.ensure(
        STEP,
        of_the_default.is_some(),
        "a refused removal must remove nothing",
    )?;
    Ok(())
}

/// A store that says it reads one mob's frames itself (`reads_one_mob`)
/// answers `query_windowed_frames_of_mob` with the frames of the mob the
/// query names (none: the default mob), whoever their member, and with no
/// frame of another mob; with an identity, that mob's member's frames. A
/// store that says it does not serves a host's default mob alone (the
/// console events of another mob are not registered over it, and a read of
/// another mob's timeline is refused): its plain pages are read for the
/// default mob's frames, so each frame of a page answers its mob.
async fn read_of_one_mob(
    steps: &Steps,
    store: &dyn ConsoleLogStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "read_of_one_mob";
    const MARK: &str = "conformance-one-mob-";
    const ALICE: &str = "console:one-mob-alice";
    const BOB: &str = "console:one-mob-bob";
    const OTHER_MOB: &str = "conformance-read-mob";

    for (key, identity, mob, timestamp_ms) in [
        ("conformance-one-mob-default-alice", ALICE, None, 2_100),
        (
            "conformance-one-mob-other-alice",
            ALICE,
            Some(OTHER_MOB),
            2_101,
        ),
        ("conformance-one-mob-default-bob", BOB, None, 2_102),
        ("conformance-one-mob-other-bob", BOB, Some(OTHER_MOB), 2_103),
    ] {
        let mut frame = fixtures::console_frame(key, identity, timestamp_ms);
        frame.mob_id = mob.map(Into::into);
        steps.wrap(STEP, store.append_if_absent(frame).await)?;
    }

    if !store.reads_one_mob() {
        let page = steps.wrap(
            STEP,
            store
                .query_windowed_frames(ConsoleTimelineWindowQuery::default())
                .await,
        )?;
        for (key, mob) in [
            ("conformance-one-mob-default-alice", None),
            ("conformance-one-mob-other-alice", Some(OTHER_MOB)),
            ("conformance-one-mob-default-bob", None),
            ("conformance-one-mob-other-bob", Some(OTHER_MOB)),
        ] {
            let frame = page
                .frames
                .iter()
                .find(|frame| frame.dedupe_key == key)
                .ok_or_else(|| {
                    steps.fail(
                        STEP,
                        format!("a plain page must answer the frames of every mob ({key})"),
                    )
                })?;
            steps.ensure(
                STEP,
                frame.mob_id == mob.map(Into::into),
                format!("a frame of a plain page must answer its mob ({key})"),
            )?;
        }
        return Ok(());
    }

    for (mob, of_the_mob, of_alice) in [
        (
            None,
            [
                "conformance-one-mob-default-alice",
                "conformance-one-mob-default-bob",
            ],
            "conformance-one-mob-default-alice",
        ),
        (
            Some(OTHER_MOB),
            [
                "conformance-one-mob-other-alice",
                "conformance-one-mob-other-bob",
            ],
            "conformance-one-mob-other-alice",
        ),
    ] {
        let page = steps.wrap(
            STEP,
            store
                .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
                    mob_id: mob.map(Into::into),
                    ..ConsoleTimelineWindowQuery::default()
                })
                .await,
        )?;
        steps.ensure(
            STEP,
            page.frames
                .iter()
                .all(|frame| frame.mob_id == mob.map(Into::into)),
            format!("the read of one mob must answer no frame of another mob (mob_id {mob:?})"),
        )?;
        let keys = page
            .frames
            .iter()
            .map(|frame| frame.dedupe_key.as_str())
            .filter(|key| key.starts_with(MARK))
            .collect::<Vec<_>>();
        steps.ensure(
            STEP,
            keys == of_the_mob,
            format!(
                "the read of one mob must answer that mob's frames, whoever their member \
                 (mob_id {mob:?}): got {keys:?}"
            ),
        )?;

        let page = steps.wrap(
            STEP,
            store
                .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
                    identity: Some(ALICE.to_string()),
                    mob_id: mob.map(Into::into),
                    ..ConsoleTimelineWindowQuery::default()
                })
                .await,
        )?;
        let keys = page
            .frames
            .iter()
            .map(|frame| frame.dedupe_key.as_str())
            .collect::<Vec<_>>();
        steps.ensure(
            STEP,
            keys == [of_alice],
            format!(
                "the read of one mob with an identity must answer that mob's member's frames \
                 (mob_id {mob:?}): got {keys:?}"
            ),
        )?;
    }
    Ok(())
}

/// `unsettled_send_frames` names no mob: it answers the unsettled sends of
/// the host's default mob, among the newest `window` of THAT mob's own
/// frames. Another mob's frames are not counted in that window: however
/// many of them lie between the default mob's send and the default mob's
/// newest frame, or after it, they never move the send out. Another mob's
/// unsettled send is not answered.
///
/// A store of a host of several mobs answers its default mob's unsettled
/// sends: one that lists none fails the step. It is not asked of a store
/// that says it reads no one mob's frames: a host registers no other mob's
/// console events over it.
async fn window_of_the_default_mobs_unsettled_sends(
    steps: &Steps,
    store: &dyn ConsoleLogStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "window_of_the_default_mobs_unsettled_sends";
    const IDENTITY: &str = "console:send-window-probe";
    const OTHER_MOB: &str = "conformance-send-window-mob";
    const OF_THE_DEFAULT: &str = "conformance-send-window-default";
    const OF_THE_OTHER: &str = "conformance-send-window-other";
    /// The window the step asks with, and how many frames of the other mob
    /// lie between the default mob's send and its next frame.
    const WINDOW: usize = 128;

    if !store.reads_one_mob() {
        return Ok(());
    }
    let send = |key: &str, mob: Option<&str>, timestamp_ms: u64| {
        let mut frame = fixtures::console_frame(key, IDENTITY, timestamp_ms);
        frame.mob_id = mob.map(Into::into);
        frame.kind = "user_input".to_string();
        frame.status = ConsoleFrameStatus::Accepted;
        frame.source.kind = ConsoleFrameSourceKind::Send;
        frame.interaction_id = Some(format!("{key}-interaction"));
        frame
    };
    let listed =
        |frames: &[ConsoleFrame], key: &str| frames.iter().any(|frame| frame.dedupe_key == key);

    steps.wrap(
        STEP,
        store
            .append_if_absent(send(OF_THE_DEFAULT, None, 2_200))
            .await,
    )?;
    let at_once = steps.wrap(STEP, store.unsettled_send_frames(WINDOW, WINDOW).await)?;
    steps.ensure(
        STEP,
        listed(&at_once, OF_THE_DEFAULT),
        "the default mob's unsettled send, its newest frame, must be answered",
    )?;

    // The other mob's frames lie between the default mob's send and the
    // default mob's next frame, and one more of them, an unsettled send of
    // its own, after that frame.
    for between in 0..WINDOW {
        let mut frame = fixtures::console_frame(
            &format!("conformance-send-window-between-{between}"),
            IDENTITY,
            2_201,
        );
        frame.mob_id = Some(OTHER_MOB.into());
        steps.wrap(STEP, store.append_if_absent(frame).await)?;
    }
    steps.wrap(
        STEP,
        store
            .append_if_absent(fixtures::console_frame(
                "conformance-send-window-default-later",
                IDENTITY,
                2_202,
            ))
            .await,
    )?;
    steps.wrap(
        STEP,
        store
            .append_if_absent(send(OF_THE_OTHER, Some(OTHER_MOB), 2_203))
            .await,
    )?;

    let unsettled = steps.wrap(STEP, store.unsettled_send_frames(WINDOW, WINDOW).await)?;
    steps.ensure(
        STEP,
        listed(&unsettled, OF_THE_DEFAULT),
        format!(
            "the default mob's unsettled send must stay in its window with {WINDOW} frames of \
             another mob between it and the default mob's newest frame: the window is \
             {WINDOW} of the default mob's own frames, and another mob's frames are never \
             counted in it"
        ),
    )?;
    steps.ensure(
        STEP,
        !listed(&unsettled, OF_THE_OTHER),
        "another mob's unsettled send must not be answered by the read that names no mob",
    )?;
    Ok(())
}

/// The six operations on a send that name its mob
/// (`update_frame_status_of_mob`, `transition_send_frame_of_mob`,
/// `take_send_frame_hold_of_mob`, `frame_by_dedupe_key_of_mob`,
/// `send_frame_by_interaction_of_mob`, `unsettled_send_frames_of_mob`)
/// reach the send of the mob they name and no other. The default mob and
/// another mob each hold an accepted send with a delivery hold. Named for
/// the other mob, each operation finds that mob's send; named for a third
/// mob, or for the default mob (by no mob and by its id), it finds no send
/// of the other mob and changes nothing of it.
///
/// It is not asked of a store that says it reads no one mob's frames: a host
/// registers no other mob's console events over it, so its sends are the
/// default mob's alone. The state move and the take of the hold are asked
/// of a store that moves a send through its states
/// (`supports_send_transitions`).
async fn sends_of_one_mob(
    steps: &Steps,
    store: &dyn ConsoleLogStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "sends_of_one_mob";
    const IDENTITY: &str = "console:mob-sends-probe";
    const OTHER_MOB: &str = "conformance-mob-sends-other";
    const THIRD_MOB: &str = "conformance-mob-sends-third";
    const OF_THE_DEFAULT: &str = "conformance-mob-sends-default";
    const OF_THE_OTHER: &str = "conformance-mob-sends-other-send";
    const WINDOW: usize = 128;

    if !store.reads_one_mob() {
        return Ok(());
    }
    let held_send = |key: &str, mob: Option<&str>, timestamp_ms: u64| -> NewConsoleFrame {
        let mut frame = fixtures::console_frame(key, IDENTITY, timestamp_ms);
        frame.mob_id = mob.map(Into::into);
        frame.id = Some(format!("{key}-id"));
        frame.kind = "user_input".to_string();
        frame.status = ConsoleFrameStatus::Accepted;
        frame.source.kind = ConsoleFrameSourceKind::Send;
        frame.interaction_id = Some(format!("{key}-interaction"));
        frame.payload = serde_json::json!({
            "marker": key,
            "delivery_hold": { "condition": "source_held", "retryable": true },
        });
        frame
    };
    steps.wrap(
        STEP,
        store
            .append_if_absent(held_send(OF_THE_DEFAULT, None, 2_300))
            .await,
    )?;
    steps.wrap(
        STEP,
        store
            .append_if_absent(held_send(OF_THE_OTHER, Some(OTHER_MOB), 2_301))
            .await,
    )?;
    let other = Some(OTHER_MOB.into());
    let third = Some(THIRD_MOB.into());
    let default_by_id = Some(DEFAULT_MOB.into());
    let other_id = format!("{OF_THE_OTHER}-id");
    let other_interaction = format!("{OF_THE_OTHER}-interaction");
    let key_of = |frame: Option<ConsoleFrame>| frame.map(|frame| frame.dedupe_key);

    // The reads.
    for (mob, expected, says) in [
        (
            other.as_ref(),
            Some(OF_THE_OTHER),
            "the mob named finds its own send by its key",
        ),
        (
            third.as_ref(),
            None,
            "a third mob finds no send of another mob by its key",
        ),
        (
            None,
            None,
            "the default mob finds no send of another mob by its key",
        ),
        (
            default_by_id.as_ref(),
            None,
            "the default mob by its id finds no send of another mob by its key",
        ),
    ] {
        let found = steps.wrap(
            STEP,
            store.frame_by_dedupe_key_of_mob(mob, OF_THE_OTHER).await,
        )?;
        steps.ensure(STEP, key_of(found).as_deref() == expected, says)?;
    }
    let found = steps.wrap(
        STEP,
        store
            .frame_by_dedupe_key_of_mob(default_by_id.as_ref(), OF_THE_DEFAULT)
            .await,
    )?;
    steps.ensure(
        STEP,
        key_of(found).as_deref() == Some(OF_THE_DEFAULT),
        "the default mob named by its id finds its own send by its key",
    )?;
    for (mob, expected, says) in [
        (
            other.as_ref(),
            Some(OF_THE_OTHER),
            "the mob named finds its own send by its interaction",
        ),
        (
            third.as_ref(),
            None,
            "a third mob finds no send of another mob by its interaction",
        ),
        (
            None,
            None,
            "the default mob finds no send of another mob by its interaction",
        ),
    ] {
        let found = steps.wrap(
            STEP,
            store
                .send_frame_by_interaction_of_mob(mob, &other_interaction)
                .await,
        )?;
        steps.ensure(STEP, key_of(found).as_deref() == expected, says)?;
    }
    let listed = steps.wrap(
        STEP,
        store
            .unsettled_send_frames_of_mob(other.as_ref(), WINDOW, WINDOW)
            .await,
    )?;
    let keys: Vec<&str> = listed
        .iter()
        .map(|frame| frame.dedupe_key.as_str())
        .collect();
    steps.ensure(
        STEP,
        keys.contains(&OF_THE_OTHER) && !keys.contains(&OF_THE_DEFAULT),
        format!("the mob named lists its own unsettled send and no other mob's: {keys:?}"),
    )?;

    // The changes named for a mob that does not hold the send change nothing.
    let before = steps.wrap(
        STEP,
        store
            .frame_by_dedupe_key_of_mob(other.as_ref(), OF_THE_OTHER)
            .await,
    )?;
    for mob in [third.as_ref(), None] {
        let changed = steps.wrap(
            STEP,
            store
                .update_frame_status_of_mob(mob, &other_id, ConsoleFrameStatus::Delivered)
                .await,
        )?;
        steps.ensure(
            STEP,
            changed.is_none(),
            "a status change named for a mob that does not hold the send finds no frame",
        )?;
        if store.supports_send_transitions() {
            let moved = steps.wrap(
                STEP,
                store
                    .transition_send_frame_of_mob(
                        mob,
                        &other_id,
                        SendTransition::StartDispatch,
                        None,
                    )
                    .await,
            )?;
            steps.ensure(
                STEP,
                moved.is_none(),
                "a state move named for a mob that does not hold the send finds no frame",
            )?;
            let taken = steps.wrap(
                STEP,
                store.take_send_frame_hold_of_mob(mob, &other_id).await,
            )?;
            steps.ensure(
                STEP,
                taken.is_none(),
                "a take of the hold named for a mob that does not hold the send finds no frame",
            )?;
        }
    }
    let after = steps.wrap(
        STEP,
        store
            .frame_by_dedupe_key_of_mob(other.as_ref(), OF_THE_OTHER)
            .await,
    )?;
    let state = |frame: &Option<ConsoleFrame>| {
        frame
            .as_ref()
            .map(|frame| (frame.status, frame.frame_version, frame.payload.clone()))
    };
    steps.ensure(
        STEP,
        state(&before) == state(&after) && before.is_some(),
        "the send is as it was after the operations named for mobs that do not hold it",
    )?;

    // Named for the mob that holds it, each change reaches it.
    if store.supports_send_transitions() {
        let taken = steps.wrap(
            STEP,
            store
                .take_send_frame_hold_of_mob(other.as_ref(), &other_id)
                .await,
        )?;
        steps.ensure(
            STEP,
            taken.is_some_and(|frame| frame.payload.get("delivery_hold").is_none()),
            "the take of the hold named for the mob that holds the send takes it",
        )?;
        let again = steps.wrap(
            STEP,
            store
                .take_send_frame_hold_of_mob(other.as_ref(), &other_id)
                .await,
        )?;
        steps.ensure(STEP, again.is_none(), "the hold is given once")?;
        let moved = steps.wrap(
            STEP,
            store
                .transition_send_frame_of_mob(
                    other.as_ref(),
                    &other_id,
                    SendTransition::StartDispatch,
                    None,
                )
                .await,
        )?;
        steps.ensure(
            STEP,
            moved.is_some_and(|frame| frame.status == ConsoleFrameStatus::Dispatching),
            "the state move named for the mob that holds the send moves it",
        )?;
    }
    let marked = steps.wrap(
        STEP,
        store
            .update_frame_status_of_mob(other.as_ref(), &other_id, ConsoleFrameStatus::Delivered)
            .await,
    )?;
    steps.ensure(
        STEP,
        marked.is_some_and(|frame| frame.status == ConsoleFrameStatus::Delivered),
        "the status change named for the mob that holds the send changes it",
    )?;
    let still = steps.wrap(
        STEP,
        store
            .unsettled_send_frames_of_mob(other.as_ref(), WINDOW, WINDOW)
            .await,
    )?;
    steps.ensure(
        STEP,
        !still.iter().any(|frame| frame.dedupe_key == OF_THE_OTHER),
        "a settled send of the mob is no longer listed among its unsettled sends",
    )?;
    Ok(())
}
