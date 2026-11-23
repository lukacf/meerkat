//! A Console send to a member of a mob the host serves beside its default
//! mob, at the aggregator: it is reserved, recorded, deduplicated and
//! recovered under that mob, and the default mob's member of the same name
//! has no part in it.
//!
//! The reservation rows run on an aggregator with no runtime registered for
//! the default mob (its identity-first reservations record under the
//! `identity-first` runtime key) that was told its host's default mob and
//! holds the registration of a second mob's console events, as a host
//! makes it. The member lane's row registers a default mob's runtime with a
//! member, since that lane resolves its member there.
//!
//! The rows of a send after it is recorded (its marks, its states as a
//! governed send, its hold, its repeat, its recovery by key and the start's
//! sweep) run over both stock stores, on a console made as a gateway makes
//! it: the store is told the host's default mob as well.

#![allow(clippy::expect_used, clippy::panic)]

use super::*;
use crate::member_key::DefaultMob;

const SESSION: &str = "0193f5d1-0b5a-7c3e-9a1f-1c2d3e4f5a6b";
const LAUNCH: &str = "send-rows-launch";

fn request(identity: &str, key: &str) -> ConsoleSendRequest {
    ConsoleSendRequest {
        identity: identity.to_string(),
        content: json!("hello"),
        origin: "console".to_string(),
        idempotency_key: key.to_string(),
        handling_mode: Some("queue".to_string()),
        origin_kind: None,
        skill_refs: Vec::new(),
    }
}

fn human(subject: &str) -> ConsoleSendScope {
    ConsoleSendScope::Human {
        issuer: "https://issuer.example".to_string(),
        subject: subject.to_string(),
    }
}

/// The acceptance of a reservation that made a new send.
fn fresh(reservation: ScopedReservation) -> ConsoleInteractionAccepted {
    match reservation {
        ScopedReservation::Fresh(accepted, _) => accepted,
        other => panic!("expected a new send: {other:?}"),
    }
}

/// The acceptance of a reservation that found its send.
fn found(reservation: ScopedReservation) -> ConsoleInteractionAccepted {
    match reservation {
        ScopedReservation::Replay(accepted) | ScopedReservation::Unsettled(accepted, ..) => {
            accepted
        }
        other @ ScopedReservation::Fresh(..) => {
            panic!("expected the send it made before: {other:?}")
        }
    }
}

/// An aggregator that was told its host's default mob ([`LAUNCH`]) and
/// holds the registration of a second mob's console events; with it that
/// mob's runtime, which the row shuts down, and its id.
async fn console_of_two_mobs() -> (MetaConsoleAggregator, UnifiedRuntime, meerkat_mob::MobId) {
    let second = meerkat_mob::MobId::from(super::tests::unique_console_mob_id("send-rows-second"));
    let runtime = super::tests::build_empty_runtime(second.as_str()).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.tell_default_mob(DefaultMob::new(meerkat_mob::MobId::from(LAUNCH)));
    aggregator
        .register_mob_console_events(
            &second,
            runtime.mob_runtime().clone(),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        )
        .expect("the second mob's console events register");
    (aggregator, runtime, second)
}

/// The stored frame with the id `frame_id` among the frames of the mob
/// `mob` (none: the default mob), by the store's read of one mob's frames.
async fn stored(
    aggregator: &MetaConsoleAggregator,
    mob: Option<&meerkat_mob::MobId>,
    frame_id: &str,
) -> ConsoleFrame {
    aggregator
        .inner
        .store
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: mob.cloned(),
            limit: 500,
            ..Default::default()
        })
        .await
        .expect("the mob's frames")
        .frames
        .into_iter()
        .find(|frame| frame.id == frame_id)
        .expect("the frame is stored under the mob")
}

/// The ids of the send frames of `identity` in the mob `mob` (none: the
/// default mob), by the store's read of one mob's member.
async fn sends_of(
    aggregator: &MetaConsoleAggregator,
    mob: Option<&meerkat_mob::MobId>,
    identity: &str,
) -> Vec<String> {
    aggregator
        .inner
        .store
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: mob.cloned(),
            identity: Some(identity.to_string()),
            ..Default::default()
        })
        .await
        .expect("the member's frames")
        .frames
        .into_iter()
        .filter(|frame| frame.kind == "user_input")
        .map(|frame| frame.id)
        .collect()
}

/// A send to a second mob's member is recorded under that mob: its frame
/// names the mob and the runtime key of the mob's registration, the read
/// of that mob's member holds it, and the read of the default mob's member
/// of the same name does not.
#[tokio::test]
async fn a_send_to_a_second_mobs_member_is_recorded_under_that_mob() {
    let (aggregator, runtime, second) = console_of_two_mobs().await;
    let accepted = fresh(
        aggregator
            .reserve_identity_first_interaction_for_mob(
                request("alice", "k1"),
                Some(&second),
                Some(SESSION),
                &human("u1"),
                None,
            )
            .await
            .expect("the send to the second mob's alice is reserved"),
    );
    let frame = stored(&aggregator, Some(&second), &accepted.input_frame_id).await;
    assert_eq!(
        frame.mob_id.as_ref(),
        Some(&second),
        "the send's frame names its mob"
    );
    assert_eq!(
        frame.runtime_key,
        format!("mob/{}", second.as_str()),
        "the send's frame is under the runtime key of the mob's registration"
    );
    assert_eq!(
        sends_of(&aggregator, Some(&second), "alice").await,
        std::slice::from_ref(&accepted.input_frame_id),
        "the second mob's alice holds the send"
    );
    assert_eq!(
        sends_of(&aggregator, None, "alice").await,
        Vec::<String>::new(),
        "the default mob's alice holds no send"
    );
    runtime.shutdown().await;
}

/// A governed send to a second mob's member is recorded under that mob, as
/// one that is not governed is.
#[tokio::test]
async fn a_governed_send_to_a_second_mobs_member_is_recorded_under_that_mob() {
    let (aggregator, runtime, second) = console_of_two_mobs().await;
    let accepted = fresh(
        aggregator
            .reserve_governed_interaction_for_mob(
                request("alice", "k1"),
                Some(&second),
                Some(SESSION),
                &human("u1"),
                None,
            )
            .await
            .expect("the governed send to the second mob's alice is reserved"),
    );
    let frame = stored(&aggregator, Some(&second), &accepted.input_frame_id).await;
    assert_eq!(
        frame.mob_id.as_ref(),
        Some(&second),
        "the governed send's frame names its mob"
    );
    assert_eq!(
        sends_of(&aggregator, Some(&second), "alice").await,
        std::slice::from_ref(&accepted.input_frame_id),
        "the second mob's alice holds the send"
    );
    assert_eq!(
        sends_of(&aggregator, None, "alice").await,
        Vec::<String>::new(),
        "the default mob's alice holds no send"
    );
    runtime.shutdown().await;
}

/// One idempotency key for the members of one name in two mobs is two
/// sends: each is new, each has a frame and an interaction of its own, a
/// repeat finds its own, and each is recovered under its own mob.
#[tokio::test]
async fn one_idempotency_key_for_two_mobs_members_of_one_name_is_two_sends() {
    let (aggregator, runtime, second) = console_of_two_mobs().await;
    let scope = human("u1");
    let reserve = |mob: Option<&meerkat_mob::MobId>| {
        let (aggregator, scope) = (aggregator.clone(), scope.clone());
        let mob = mob.cloned();
        async move {
            aggregator
                .reserve_identity_first_interaction_for_mob(
                    request("alice", "k1"),
                    mob.as_ref(),
                    Some(SESSION),
                    &scope,
                    None,
                )
                .await
                .expect("reservation")
        }
    };
    let of_the_default = fresh(reserve(None).await);
    let of_the_second = fresh(reserve(Some(&second)).await);
    assert_ne!(of_the_default.input_frame_id, of_the_second.input_frame_id);
    assert_ne!(of_the_default.interaction_id, of_the_second.interaction_id);
    assert_eq!(
        found(reserve(Some(&second)).await),
        of_the_second,
        "a repeat for the second mob finds that mob's send"
    );
    assert_eq!(
        found(reserve(None).await),
        of_the_default,
        "a repeat for the default mob finds the default mob's send"
    );

    let identities = ["alice".to_string()];
    for (mob, expected) in [(None, &of_the_default), (Some(&second), &of_the_second)] {
        let recovered = aggregator
            .recover_send_for_mob(&identities, mob, "console", "k1", &scope)
            .await
            .expect("recover")
            .map(|recovery| recovery.accepted);
        assert_eq!(
            recovered.as_ref(),
            Some(expected),
            "the send is recovered under its own mob ({mob:?})"
        );
    }
    runtime.shutdown().await;
}

/// A send that names the default mob by its id is the default mob's send:
/// its frame names no mob, and the same send made with no mob finds it.
#[tokio::test]
async fn a_send_that_names_the_default_mob_by_its_id_is_the_default_mobs_send() {
    let (aggregator, runtime, _second) = console_of_two_mobs().await;
    let default_id = meerkat_mob::MobId::from(LAUNCH);
    let scope = human("u1");
    let by_id = fresh(
        aggregator
            .reserve_identity_first_interaction_for_mob(
                request("alice", "k1"),
                Some(&default_id),
                Some(SESSION),
                &scope,
                None,
            )
            .await
            .expect("the send that names the default mob by its id"),
    );
    let frame = stored(&aggregator, None, &by_id.input_frame_id).await;
    assert_eq!(frame.mob_id, None, "the default mob's frame names no mob");
    assert_eq!(frame.runtime_key, "identity-first");
    assert_eq!(
        found(
            aggregator
                .reserve_identity_first_interaction_scoped(
                    request("alice", "k1"),
                    Some(SESSION),
                    &scope,
                    None,
                )
                .await
                .expect("the same send with no mob")
        ),
        by_id,
        "the send with no mob is the same send"
    );
    runtime.shutdown().await;
}

/// A send that names an empty mob names none: it is the default mob's send,
/// as the dedupe key of a send reads an empty mob. A mob of white space
/// alone is not empty: a send for it is refused.
#[tokio::test]
async fn a_send_that_names_an_empty_mob_is_the_default_mobs_send() {
    let (aggregator, runtime, _second) = console_of_two_mobs().await;
    let empty = meerkat_mob::MobId::from("");
    let scope = human("u1");
    let with_no_mob = fresh(
        aggregator
            .reserve_identity_first_interaction_scoped(
                request("alice", "k1"),
                Some(SESSION),
                &scope,
                None,
            )
            .await
            .expect("the send with no mob"),
    );
    assert_eq!(
        found(
            aggregator
                .reserve_identity_first_interaction_for_mob(
                    request("alice", "k1"),
                    Some(&empty),
                    Some(SESSION),
                    &scope,
                    None,
                )
                .await
                .expect("the same send with an empty mob")
        ),
        with_no_mob,
        "an empty mob is no mob"
    );
    assert_eq!(
        stored(&aggregator, None, &with_no_mob.input_frame_id)
            .await
            .mob_id,
        None
    );
    // A mob of white space alone is not empty: it names a mob, one no
    // console events are registered for, so the send finds no agent and is
    // not the default mob's send.
    let of_white_space = meerkat_mob::MobId::from("  ");
    let refused = aggregator
        .reserve_identity_first_interaction_for_mob(
            request("alice", "k1"),
            Some(&of_white_space),
            Some(SESSION),
            &scope,
            None,
        )
        .await;
    assert!(
        refused.is_err(),
        "a send for a mob of white space alone is refused: {:?}",
        refused.as_ref().ok()
    );
    assert_eq!(
        sends_of(&aggregator, None, "alice").await,
        std::slice::from_ref(&with_no_mob.input_frame_id),
        "the default mob's alice holds its one send and no other"
    );
    runtime.shutdown().await;
}

/// A send for a mob whose console events are not registered is refused and
/// stores no frame: it is never recorded under the default mob's member of
/// that name, and its key is never looked for among the default mob's.
#[tokio::test]
async fn a_send_for_a_mob_that_is_not_registered_is_refused_and_stores_no_frame() {
    let (aggregator, runtime, _second) = console_of_two_mobs().await;
    let third = meerkat_mob::MobId::from("send-rows-third");
    let refused = aggregator
        .reserve_identity_first_interaction_for_mob(
            request("alice", "k1"),
            Some(&third),
            Some(SESSION),
            &human("u1"),
            None,
        )
        .await;
    assert!(
        refused.is_err(),
        "a send for a mob with no registration is refused: {:?}",
        refused.as_ref().ok()
    );
    assert_eq!(
        sends_of(&aggregator, None, "alice").await,
        Vec::<String>::new(),
        "the default mob's alice holds no send"
    );
    assert_eq!(
        sends_of(&aggregator, Some(&third), "alice").await,
        Vec::<String>::new(),
        "and no frame is stored under the mob"
    );
    runtime.shutdown().await;
}

/// The member lane serves the default mob alone: a send for another mob's
/// member knows no such identity, also where the default mob has a member
/// of that name, and that member gets nothing.
#[tokio::test]
async fn the_member_lane_refuses_another_mob() {
    let launch = super::tests::build_empty_runtime(&super::tests::unique_console_mob_id(
        "send-rows-member-lane",
    ))
    .await;
    let seated = Box::pin(launch.spawn(meerkat_mob::SpawnMemberSpec::new("worker", "alice"))).await;
    assert!(
        seated.is_ok(),
        "the default mob's alice: {:?}",
        seated.as_ref().err()
    );
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.tell_default_mob(DefaultMob::new(launch.mob_handle().mob_id().clone()));
    aggregator.register_runtime_handles_with_policy(
        "runtime-a",
        "",
        launch.mob_runtime().clone(),
        None,
        launch.console_events(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let second = meerkat_mob::MobId::from("send-rows-another-mob");

    let sent = Box::pin(aggregator.send_for_mob(
        request("alice", "k1"),
        Some(&second),
        &ConsoleSendScope::Unscoped,
        None,
    ))
    .await;
    assert!(
        matches!(sent, Err(ConsoleSendError::UnknownIdentity(_))),
        "the member lane knows no member of another mob: {:?}",
        sent.as_ref()
            .map(|_| "accepted")
            .map_err(ToString::to_string)
    );
    assert_eq!(
        sends_of(&aggregator, None, "alice").await,
        Vec::<String>::new(),
        "the default mob's alice got nothing"
    );
    launch.shutdown().await;
}

// What follows is a second mob's send AFTER it is recorded: its marks, its
// states as a governed send, its hold, its repeat and its recovery by key,
// and the start's sweep. Each holds over both stock stores, a row for each.

/// The log a row's console writes to.
#[derive(Clone, Copy, Debug)]
enum Log {
    Memory,
    Sqlite,
}

/// [`console_of_two_mobs`] over the log `log`, made as a gateway makes its
/// console: the store is told the host's default mob too.
struct TwoMobs {
    aggregator: MetaConsoleAggregator,
    /// The second mob's runtime, which the row shuts down.
    runtime: UnifiedRuntime,
    second: meerkat_mob::MobId,
    /// The directory that holds a SQLite log's file.
    _dir: Option<tempfile::TempDir>,
}

/// The log `log` of a host, told the host's default mob as a gateway tells
/// its store, and the directory that holds a SQLite log's file.
fn log_of_a_host(log: Log) -> (Arc<dyn ConsoleLogStore>, Option<tempfile::TempDir>) {
    let dir = match log {
        Log::Memory => None,
        Log::Sqlite => Some(tempfile::tempdir().expect("a directory for the log")),
    };
    let store: Arc<dyn ConsoleLogStore> = match &dir {
        None => Arc::new(InMemoryConsoleLogStore::new()),
        Some(dir) => Arc::new(
            SqliteConsoleLogStore::open(dir.path().join("console.sqlite3")).expect("the log opens"),
        ),
    };
    store
        .bind_default_mob(&DefaultMob::new(meerkat_mob::MobId::from(LAUNCH)))
        .expect("the store is told the default mob");
    (store, dir)
}

/// A console over `store` that was told its host's default mob and holds
/// no mob's console events yet.
fn console_over(store: Arc<dyn ConsoleLogStore>) -> MetaConsoleAggregator {
    let aggregator = MetaConsoleAggregator::new(store);
    aggregator.tell_default_mob(DefaultMob::new(meerkat_mob::MobId::from(LAUNCH)));
    aggregator
}

/// The console events of the mob `second`, whose runtime is `runtime`,
/// register with `aggregator`, as a host registers them when it makes the
/// mob's entry. The registration's token, which ends it.
fn register_the_second_mob(
    aggregator: &MetaConsoleAggregator,
    runtime: &UnifiedRuntime,
    second: &meerkat_mob::MobId,
) -> uuid::Uuid {
    aggregator
        .register_mob_console_events(
            second,
            runtime.mob_runtime().clone(),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        )
        .expect("the second mob's console events register")
}

async fn console_of_two_mobs_over(log: Log) -> TwoMobs {
    let second = meerkat_mob::MobId::from(super::tests::unique_console_mob_id("send-rows-second"));
    let runtime = super::tests::build_empty_runtime(second.as_str()).await;
    let (store, dir) = log_of_a_host(log);
    let aggregator = console_over(store);
    register_the_second_mob(&aggregator, &runtime, &second);
    TwoMobs {
        aggregator,
        runtime,
        second,
        _dir: dir,
    }
}

/// A send's frame as a process that stopped left it in the log: a send to
/// `identity` of the mob `mob` (none: the default mob) under `key`,
/// accepted, recorded under the runtime key a send of that mob is recorded
/// under with no runtime registered for the default mob; with a delivery
/// hold on it when `held`. Its frame id is `frame-<key>` and its
/// interaction `interaction-<key>`.
fn left_send_frame(
    mob: Option<&meerkat_mob::MobId>,
    identity: &str,
    key: &str,
    held: bool,
) -> NewConsoleFrame {
    let mut payload = json!({ "content": "hello", "origin": "console" });
    if held && let Some(fields) = payload.as_object_mut() {
        fields.insert(
            "delivery_hold".to_string(),
            json!({ "condition": "source_held", "retryable": true }),
        );
    }
    NewConsoleFrame {
        mob_id: mob.cloned(),
        id: Some(format!("frame-{key}")),
        dedupe_key: key.to_string(),
        timestamp_ms: 1,
        runtime_key: mob.map_or_else(
            || "identity-first".to_string(),
            |mob| format!("mob/{}", mob.as_str()),
        ),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: Some(SESSION.to_string()),
        kind: "user_input".to_string(),
        status: ConsoleFrameStatus::Accepted,
        payload,
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::Send,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: Some(format!("interaction-{key}")),
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

/// [`left_send_frame`] in the state `status`, stored at the store's own
/// door. Its frame id.
async fn left_send(
    store: &dyn ConsoleLogStore,
    mob: Option<&meerkat_mob::MobId>,
    identity: &str,
    key: &str,
    status: ConsoleFrameStatus,
    held: bool,
) -> String {
    let mut frame = left_send_frame(mob, identity, key, held);
    frame.status = status;
    store
        .append_if_absent(frame)
        .await
        .expect("the send's frame is stored")
        .frame
        .id
}

/// The frame `frame_id` among the frames of the mob `mob` (none: the
/// default mob), by the store's read of one mob's frames.
async fn frame_in(
    store: &dyn ConsoleLogStore,
    mob: Option<&meerkat_mob::MobId>,
    frame_id: &str,
) -> ConsoleFrame {
    store
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: mob.cloned(),
            limit: 500,
            ..Default::default()
        })
        .await
        .expect("the mob's frames")
        .frames
        .into_iter()
        .find(|frame| frame.id == frame_id)
        .expect("the frame is stored under the mob")
}

/// A send to `identity` of the mob `mob` (none: the default mob) under
/// `key`, reserved as a new send; its dispatch claim is given back, as a
/// dispatch that ended gives it.
async fn sent(
    host: &TwoMobs,
    mob: Option<&meerkat_mob::MobId>,
    identity: &str,
    key: &str,
) -> ConsoleInteractionAccepted {
    fresh(
        host.aggregator
            .reserve_identity_first_interaction_for_mob(
                request(identity, key),
                mob,
                Some(SESSION),
                &human("u1"),
                None,
            )
            .await
            .expect("the send is reserved"),
    )
}

/// A governed send to the second mob's `alice` under `key`, reserved as a
/// new send.
async fn sent_governed(host: &TwoMobs, key: &str) -> ConsoleInteractionAccepted {
    fresh(
        host.aggregator
            .reserve_governed_interaction_for_mob(
                request("alice", key),
                Some(&host.second),
                Some(SESSION),
                &human("u1"),
                None,
            )
            .await
            .expect("the governed send is reserved"),
    )
}

/// The status of the second mob's frame `frame_id`, as the log holds it.
async fn status_in_the_second_mob(host: &TwoMobs, frame_id: &str) -> &'static str {
    stored(&host.aggregator, Some(&host.second), frame_id)
        .await
        .status
        .as_str()
}

/// How many changes of the frame `frame_id` the second mob's frames record
/// (its `frame_updated` frames).
async fn recorded_changes_of(host: &TwoMobs, frame_id: &str) -> usize {
    host.aggregator
        .inner
        .store
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: Some(host.second.clone()),
            limit: 500,
            ..Default::default()
        })
        .await
        .expect("the second mob's frames")
        .frames
        .iter()
        .filter(|frame| {
            frame.kind == "frame_updated" && frame.parent_frame_id.as_deref() == Some(frame_id)
        })
        .count()
}

/// What an answer was, in words a row can compare.
fn answer<T>(result: &Result<T, ConsoleSendError>) -> String {
    match result {
        Ok(_) => "ok".to_string(),
        Err(error) => format!("refused: {error}"),
    }
}

/// The runtime's evidence of admitted inputs, as a row gives it: an input
/// id for each admitted send's key. Every session answers the same.
struct Evidence(Vec<(String, String)>);

#[async_trait::async_trait]
impl super::send_recovery::SendAdmissionReader for Evidence {
    async fn admitted_input(
        &self,
        _session_id: &meerkat_core::types::SessionId,
        idempotency_key: &str,
    ) -> Result<Option<super::send_recovery::AdmittedInput>, String> {
        Ok(self
            .0
            .iter()
            .find(|(key, _)| key == idempotency_key)
            .map(|(_, input_id)| super::send_recovery::AdmittedInput {
                input_id: input_id.clone(),
                phase: Some("queued".to_string()),
            }))
    }

    async fn admitted_input_by_id(
        &self,
        session_id: &meerkat_core::types::SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<Option<super::send_recovery::AdmittedInput>, String> {
        self.admitted_input(session_id, &input_id.to_string()).await
    }
}

/// The console reads `admitted` (a send's frame id, and the input admitted
/// as that send's) as the runtime's evidence from here on.
fn admit(host: &TwoMobs, admitted: &[(&str, &str)]) {
    host.aggregator.read_send_admission_from(Arc::new(Evidence(
        admitted
            .iter()
            .map(|(key, input)| ((*key).to_string(), (*input).to_string()))
            .collect(),
    )));
}

/// The marks of a second mob's send, each naming that mob as the door that
/// made the send names it, are recorded on its frame: delivered for one
/// send, failed for another, and each change is one recorded change of that
/// frame among the mob's frames.
async fn the_marks_of_a_second_mobs_send_are_recorded(log: Log) {
    let host = console_of_two_mobs_over(log).await;
    let delivered = sent(&host, Some(&host.second), "alice", "delivered").await;
    let failed = sent(&host, Some(&host.second), "alice", "failed").await;
    let marked_delivered = answer(
        &host
            .aggregator
            .mark_interaction_delivered_of_mob(Some(&host.second), &delivered.input_frame_id)
            .await,
    );
    let marked_failed = answer(
        &host
            .aggregator
            .mark_interaction_delivery_failed_of_mob(Some(&host.second), &failed.input_frame_id)
            .await,
    );
    let statuses = [
        status_in_the_second_mob(&host, &delivered.input_frame_id).await,
        status_in_the_second_mob(&host, &failed.input_frame_id).await,
    ];
    let changes = [
        recorded_changes_of(&host, &delivered.input_frame_id).await,
        recorded_changes_of(&host, &failed.input_frame_id).await,
    ];
    host.runtime.shutdown().await;

    assert_eq!(
        [marked_delivered.as_str(), marked_failed.as_str()],
        ["ok", "ok"],
        "{log:?}: the two marks are answered"
    );
    assert_eq!(
        statuses,
        ["delivered", "delivery_failed"],
        "{log:?}: each send's frame holds its mark"
    );
    assert_eq!(
        changes,
        [1, 1],
        "{log:?}: each mark is one recorded change of its frame, among the mob's frames"
    );
}

#[tokio::test]
async fn the_marks_of_a_second_mobs_send_are_recorded_on_sqlite() {
    the_marks_of_a_second_mobs_send_are_recorded(Log::Sqlite).await;
}

#[tokio::test]
async fn the_marks_of_a_second_mobs_send_are_recorded_in_memory() {
    the_marks_of_a_second_mobs_send_are_recorded(Log::Memory).await;
}

/// A governed send to a second mob's member moves through the send's own
/// states, each move naming that mob: its dispatch starts (dispatching),
/// its bind binds a session (submitted), and a submission whose answer was
/// lost keeps it submitted.
async fn a_second_mobs_governed_send_moves_through_its_states(log: Log) {
    let host = console_of_two_mobs_over(log).await;
    let send = sent_governed(&host, "governed").await;
    let id = send.input_frame_id.as_str();
    let mob = Some(&host.second);
    let mut seen = Vec::new();
    let started = answer(
        &host
            .aggregator
            .mark_interaction_dispatching_of_mob(mob, id)
            .await,
    );
    seen.push((started, status_in_the_second_mob(&host, id).await));
    let bound = meerkat_core::types::SessionId::new();
    let submitted = answer(
        &host
            .aggregator
            .mark_interaction_submitted_of_mob(mob, id, &bound)
            .await,
    );
    seen.push((submitted, status_in_the_second_mob(&host, id).await));
    let unresolved = answer(
        &host
            .aggregator
            .mark_interaction_unresolved_of_mob(mob, id)
            .await,
    );
    seen.push((unresolved, status_in_the_second_mob(&host, id).await));
    host.runtime.shutdown().await;

    assert_eq!(
        seen,
        [
            ("ok".to_string(), "dispatching"),
            ("ok".to_string(), "submitted"),
            ("ok".to_string(), "submitted"),
        ],
        "{log:?}: the start of the dispatch, the bound submission and the lost answer, each \
         with the state the frame then holds"
    );
}

#[tokio::test]
async fn a_second_mobs_governed_send_moves_through_its_states_on_sqlite() {
    a_second_mobs_governed_send_moves_through_its_states(Log::Sqlite).await;
}

#[tokio::test]
async fn a_second_mobs_governed_send_moves_through_its_states_in_memory() {
    a_second_mobs_governed_send_moves_through_its_states(Log::Memory).await;
}

/// A governed send to a second mob's member that was held records its hold
/// (it is accepted again, and its frame says why it waits), and the hold is
/// given to one taker: the first take gets it, the next gets none. Each
/// mark and take names that mob.
async fn a_second_mobs_held_send_records_its_hold_and_gives_it_once(log: Log) {
    let host = console_of_two_mobs_over(log).await;
    let send = sent_governed(&host, "held").await;
    let id = send.input_frame_id.as_str();
    let mob = Some(&host.second);
    let started = answer(
        &host
            .aggregator
            .mark_interaction_dispatching_of_mob(mob, id)
            .await,
    );
    let hold = crate::identity_first::GovernedHold {
        condition: meerkat_core::authorization::ControllerReadinessFailure::SourceHeld,
        waited: std::time::Duration::from_millis(300),
        binds: 1,
        input_id: meerkat_core::lifecycle::InputId::new(),
        idempotency_key: None,
    };
    let held = answer(
        &host
            .aggregator
            .mark_interaction_delivery_held_of_mob(mob, id, &hold)
            .await,
    );
    let frame = stored(&host.aggregator, Some(&host.second), id).await;
    let recorded = (
        frame.status.as_str(),
        frame.payload.get("delivery_hold").is_some(),
    );
    let takes = [
        host.aggregator
            .take_interaction_delivery_hold_of_mob(mob, id)
            .await
            .map_err(|error| error.to_string()),
        host.aggregator
            .take_interaction_delivery_hold_of_mob(mob, id)
            .await
            .map_err(|error| error.to_string()),
    ];
    let after = stored(&host.aggregator, Some(&host.second), id)
        .await
        .payload
        .get("delivery_hold")
        .is_some();
    host.runtime.shutdown().await;

    assert_eq!(
        [started.as_str(), held.as_str()],
        ["ok", "ok"],
        "{log:?}: the dispatch starts and the hold is recorded"
    );
    assert_eq!(
        recorded,
        ("accepted", true),
        "{log:?}: the held send is accepted again and its frame records the hold"
    );
    assert_eq!(
        takes,
        [Ok(true), Ok(false)],
        "{log:?}: the hold is given to the first taker and to no other"
    );
    assert!(!after, "{log:?}: the taken hold is off the frame");
}

#[tokio::test]
async fn a_second_mobs_held_send_records_its_hold_and_gives_it_once_on_sqlite() {
    a_second_mobs_held_send_records_its_hold_and_gives_it_once(Log::Sqlite).await;
}

#[tokio::test]
async fn a_second_mobs_held_send_records_its_hold_and_gives_it_once_in_memory() {
    a_second_mobs_held_send_records_its_hold_and_gives_it_once(Log::Memory).await;
}

/// A second mob's send made again under its key is the same send: the
/// repeat answers the frame made before and stores no second one, and a
/// changed request under the key is refused.
async fn a_second_mobs_send_again_under_its_key_is_the_same_send(log: Log) {
    let host = console_of_two_mobs_over(log).await;
    let first = sent(&host, Some(&host.second), "alice", "k1").await;
    let again = host
        .aggregator
        .reserve_identity_first_interaction_for_mob(
            request("alice", "k1"),
            Some(&host.second),
            Some(SESSION),
            &human("u1"),
            None,
        )
        .await
        .map(found)
        .map(|accepted| accepted.input_frame_id)
        .map_err(|error| error.to_string());
    let mut changed = request("alice", "k1");
    changed.content = json!("another text under the same key");
    let conflict = host
        .aggregator
        .reserve_identity_first_interaction_for_mob(
            changed,
            Some(&host.second),
            Some(SESSION),
            &human("u1"),
            None,
        )
        .await;
    let held = sends_of(&host.aggregator, Some(&host.second), "alice").await;
    host.runtime.shutdown().await;

    assert_eq!(
        again.as_ref(),
        Ok(&first.input_frame_id),
        "{log:?}: the repeat answers the send made before"
    );
    assert!(
        matches!(conflict, Err(ConsoleSendError::IdempotencyConflict(_))),
        "{log:?}: a changed request under the key is refused: {}",
        answer(&conflict)
    );
    assert_eq!(
        held,
        std::slice::from_ref(&first.input_frame_id),
        "{log:?}: the mob's alice holds the one send"
    );
}

#[tokio::test]
async fn a_second_mobs_send_again_under_its_key_is_the_same_send_on_sqlite() {
    a_second_mobs_send_again_under_its_key_is_the_same_send(Log::Sqlite).await;
}

#[tokio::test]
async fn a_second_mobs_send_again_under_its_key_is_the_same_send_in_memory() {
    a_second_mobs_send_again_under_its_key_is_the_same_send(Log::Memory).await;
}

/// What the recovery of the send under the key `k1` to `alice` of the mob
/// `mob` (none: the default mob) finds: the send's frame id, the status the
/// recovery answers, and what the read of the runtime's evidence answered
/// (`admitted`, `absent`, or `unknown` with its cause).
async fn recovered(
    host: &TwoMobs,
    mob: Option<&meerkat_mob::MobId>,
) -> Result<Option<(String, &'static str, String)>, String> {
    host.aggregator
        .recover_send_for_mob(&["alice".to_string()], mob, "console", "k1", &human("u1"))
        .await
        .map(|found| {
            found.map(|found| {
                let evidence = match found.native {
                    NativeSendObservation::Admitted { .. } => "admitted".to_string(),
                    NativeSendObservation::Absent => "absent".to_string(),
                    NativeSendObservation::Unknown { cause } => format!("unknown: {cause}"),
                };
                (
                    found.accepted.input_frame_id,
                    found.accepted.status.as_str(),
                    evidence,
                )
            })
        })
        .map_err(|error| error.to_string())
}

/// A second mob's send is recovered by its key, under that mob alone, and
/// the recovery settles it from the evidence of the runtime it was made
/// to. First with nothing in the way: the recovery reads the runtime of the
/// send's own mob, which holds no input of the send, so the send is
/// reported as it is, accepted. Then with evidence that its input was
/// admitted: it is settled delivered. The same key asked for the default
/// mob's member of the name finds no send.
async fn a_second_mobs_send_is_recovered_by_its_key(log: Log) {
    let host = console_of_two_mobs_over(log).await;
    let send = sent(&host, Some(&host.second), "alice", "k1").await;
    let absent = recovered(&host, Some(&host.second)).await;
    let of_the_default_mob = recovered(&host, None).await;
    admit(&host, &[(send.input_frame_id.as_str(), "input-1")]);
    let admitted = recovered(&host, Some(&host.second)).await;
    let stored_status = status_in_the_second_mob(&host, &send.input_frame_id).await;
    host.runtime.shutdown().await;

    assert_eq!(
        absent,
        Ok(Some((
            send.input_frame_id.clone(),
            "accepted",
            "absent".to_string()
        ))),
        "{log:?}: the send is found by its key, and its own mob's runtime holds no input of it"
    );
    assert_eq!(
        of_the_default_mob,
        Ok(None),
        "{log:?}: the key asked for the default mob's member of the name finds no send"
    );
    assert_eq!(
        admitted,
        Ok(Some((
            send.input_frame_id.clone(),
            "delivered",
            "admitted".to_string()
        ))),
        "{log:?}: the recovery settles the send whose input was admitted"
    );
    assert_eq!(
        stored_status, "delivered",
        "{log:?}: and the frame holds it"
    );
}

#[tokio::test]
async fn a_second_mobs_send_is_recovered_by_its_key_on_sqlite() {
    a_second_mobs_send_is_recovered_by_its_key(Log::Sqlite).await;
}

#[tokio::test]
async fn a_second_mobs_send_is_recovered_by_its_key_in_memory() {
    a_second_mobs_send_is_recovered_by_its_key(Log::Memory).await;
}

/// A start's sweep is the default mob's: it neither counts nor examines a
/// send of a mob whose console events are not registered when it runs, and
/// leaves that send as it is. The log holds, as a process that stopped left
/// them, an accepted send to the default mob's `bob` and a submitted send
/// to a second mob's `alice`; no mob's console events are registered. The
/// census counts one send, the sweep examines one, and the second mob's
/// send stays submitted.
async fn a_start_leaves_the_sends_of_a_mob_whose_events_are_not_registered(log: Log) {
    let (store, _dir) = log_of_a_host(log);
    let second = meerkat_mob::MobId::from("send-rows-not-registered");
    left_send(
        store.as_ref(),
        None,
        "bob",
        "of-the-default-mob",
        ConsoleFrameStatus::Accepted,
        false,
    )
    .await;
    let of_the_second = left_send(
        store.as_ref(),
        Some(&second),
        "alice",
        "of-the-second-mob",
        ConsoleFrameStatus::Submitted,
        false,
    )
    .await;
    let aggregator = console_over(Arc::clone(&store));
    let counted = aggregator.unsettled_send_count().await;
    let outcome = aggregator.reconcile_interrupted_sends().await;
    let left = frame_in(store.as_ref(), Some(&second), &of_the_second)
        .await
        .status
        .as_str();

    assert_eq!(
        counted, 1,
        "{log:?}: the start's census counts the default mob's send and no send of the mob \
         whose events are not registered"
    );
    assert_eq!(
        (outcome.examined, outcome.delivered, outcome.lost),
        (1, 0, 0),
        "{log:?}: the start's sweep examines the default mob's send alone: {outcome:?}"
    );
    assert_eq!(
        left, "submitted",
        "{log:?}: the second mob's send is left as it is"
    );
}

#[tokio::test]
async fn a_start_leaves_the_sends_of_a_mob_whose_events_are_not_registered_on_sqlite() {
    a_start_leaves_the_sends_of_a_mob_whose_events_are_not_registered(Log::Sqlite).await;
}

#[tokio::test]
async fn a_start_leaves_the_sends_of_a_mob_whose_events_are_not_registered_in_memory() {
    a_start_leaves_the_sends_of_a_mob_whose_events_are_not_registered(Log::Memory).await;
}

/// A mob's own sweep runs when its console events register, and reads that
/// mob's own runtime. The log holds, as a process that stopped left them:
/// an accepted send to the second mob's `bob`; a submitted send to the
/// second mob's `alice`; and a submitted send to the default mob's `carol`,
/// recorded under a runtime that is registered for the default mob. No
/// runtime holds an input of any of them.
///
/// When the second mob's console events register, the submitted send of
/// that mob fails, its outcome unknown: its own runtime holds no input of
/// it. The mob's accepted send, which the sweep examines before it (it is
/// the older frame), stays accepted: an absent input fails no send that
/// was never submitted. The default mob's submitted send stays submitted:
/// it is no send of this mob, though its own runtime would answer the same.
async fn a_mobs_own_sweep_at_its_registration_fails_its_lost_send(log: Log) {
    let host = swept_at_its_registration(log).await;
    let (failed, _) = host.the_lost_send_once_failed().await;
    let others = [
        frame_in(host.store.as_ref(), Some(&host.second), &host.waiting)
            .await
            .status
            .as_str(),
        frame_in(host.store.as_ref(), None, &host.of_the_default)
            .await
            .status
            .as_str(),
    ];
    host.shut_down().await;

    assert_eq!(
        (
            failed.status.as_str(),
            failed
                .payload
                .pointer("/delivery_failure/reason")
                .and_then(Value::as_str),
        ),
        ("delivery_failed", Some("outcome_unknown")),
        "{log:?}: the mob's sweep at its registration fails the submitted send its own runtime \
         holds no input of"
    );
    assert_eq!(
        others,
        ["accepted", "submitted"],
        "{log:?}: the mob's accepted send stays accepted, and the default mob's submitted send \
         is not this mob's to sweep"
    );
}

/// The host of the rows of a mob's own sweep: its log, the runtime
/// registered for the default mob, the second mob with its runtime, the
/// console, and the three sends the log held before the second mob's
/// console events registered.
struct SweptAtItsRegistration {
    store: Arc<dyn ConsoleLogStore>,
    aggregator: MetaConsoleAggregator,
    launch: UnifiedRuntime,
    runtime: UnifiedRuntime,
    second: meerkat_mob::MobId,
    /// The default mob's submitted send, the oldest of the three.
    of_the_default: String,
    /// The second mob's accepted send, older than its submitted one.
    waiting: String,
    /// The second mob's submitted send, the newest of the three.
    lost: String,
    /// When the second mob's console events registered.
    registered: tokio::time::Instant,
    /// The token of that registration, which ends it.
    registration: uuid::Uuid,
    _dir: Option<tempfile::TempDir>,
}

impl SweptAtItsRegistration {
    /// The second mob's submitted send, read once the mob's sweep failed it
    /// (or as it is when nothing failed it in time), and how long after
    /// the mob's registration that was. The sweep is the registration's own
    /// work: a row waits for its outcome and asks for nothing.
    async fn the_lost_send_once_failed(&self) -> (ConsoleFrame, std::time::Duration) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let frame = frame_in(self.store.as_ref(), Some(&self.second), &self.lost).await;
            if frame.status == ConsoleFrameStatus::DeliveryFailed
                || tokio::time::Instant::now() >= deadline
            {
                return (frame, self.registered.elapsed());
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    async fn shut_down(self) {
        self.runtime.shutdown().await;
        self.launch.shutdown().await;
    }
}

async fn swept_at_its_registration(log: Log) -> SweptAtItsRegistration {
    let (store, dir) = log_of_a_host(log);
    let launch = super::tests::build_empty_runtime(&super::tests::unique_console_mob_id(
        "send-rows-launch-runtime",
    ))
    .await;
    let second = meerkat_mob::MobId::from(super::tests::unique_console_mob_id(
        "send-rows-swept-at-registration",
    ));
    let runtime = super::tests::build_empty_runtime(second.as_str()).await;
    // The default mob's send is the oldest frame: a sweep that took every
    // mob's sends would reach it before this mob's. It is recorded under
    // the runtime key its runtime is registered under, so that runtime's
    // evidence of it can be read.
    let mut of_the_default = left_send_frame(None, "carol", "of-the-default-mob", false);
    of_the_default.status = ConsoleFrameStatus::Submitted;
    of_the_default.runtime_key = "runtime-a".to_string();
    let of_the_default = store
        .append_if_absent(of_the_default)
        .await
        .expect("the default mob's send's frame is stored")
        .frame
        .id;
    let waiting = left_send(
        store.as_ref(),
        Some(&second),
        "bob",
        "waiting",
        ConsoleFrameStatus::Accepted,
        false,
    )
    .await;
    let lost = left_send(
        store.as_ref(),
        Some(&second),
        "alice",
        "lost",
        ConsoleFrameStatus::Submitted,
        false,
    )
    .await;
    let aggregator = console_over(Arc::clone(&store));
    aggregator.register_runtime_handles_with_policy(
        "runtime-a",
        "",
        launch.mob_runtime().clone(),
        None,
        launch.console_events(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let registered = tokio::time::Instant::now();
    let registration = register_the_second_mob(&aggregator, &runtime, &second);
    SweptAtItsRegistration {
        store,
        aggregator,
        launch,
        runtime,
        second,
        of_the_default,
        waiting,
        lost,
        registered,
        registration,
        _dir: dir,
    }
}

#[tokio::test]
async fn a_mobs_own_sweep_at_its_registration_fails_its_lost_send_on_sqlite() {
    a_mobs_own_sweep_at_its_registration_fails_its_lost_send(Log::Sqlite).await;
}

#[tokio::test]
async fn a_mobs_own_sweep_at_its_registration_fails_its_lost_send_in_memory() {
    a_mobs_own_sweep_at_its_registration_fails_its_lost_send(Log::Memory).await;
}

/// A reader of the runtime's evidence that records each send it is asked
/// for (the key or the input id of the read) and answers that the runtime
/// holds no input of it.
#[derive(Default)]
struct RecordedReads(std::sync::Mutex<Vec<String>>);

impl RecordedReads {
    fn record(&self, read: String) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(read);
    }

    fn all(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl super::send_recovery::SendAdmissionReader for RecordedReads {
    async fn admitted_input(
        &self,
        _session_id: &meerkat_core::types::SessionId,
        idempotency_key: &str,
    ) -> Result<Option<super::send_recovery::AdmittedInput>, String> {
        self.record(idempotency_key.to_string());
        Ok(None)
    }

    async fn admitted_input_by_id(
        &self,
        _session_id: &meerkat_core::types::SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<Option<super::send_recovery::AdmittedInput>, String> {
        self.record(input_id.to_string());
        Ok(None)
    }
}

/// A registration that takes the place of a STANDING registration of the
/// mob, whose sweep completed, sweeps nothing again. After the mob's first
/// registration failed its lost send (the sweep ran to its end), the log
/// gets another submitted send of that mob (as a row can leave one: no
/// runtime holds an input of it), and the mob's console events register
/// again while the first registration stands. That send stays submitted,
/// left to its sender's recovery, and no send's evidence is read: a sweep
/// that examined the send would have read it.
///
/// "Nothing is swept" is waited for: eight times as long as the first
/// registration's sweep took to fail its send, and two seconds at least.
async fn a_registration_in_a_standing_ones_place_sweeps_nothing_after_a_completed_sweep(log: Log) {
    let host = swept_at_its_registration(log).await;
    let (failed, took) = host.the_lost_send_once_failed().await;
    let later = left_send(
        host.store.as_ref(),
        Some(&host.second),
        "alice",
        "later",
        ConsoleFrameStatus::Submitted,
        false,
    )
    .await;
    let reads = Arc::new(RecordedReads::default());
    host.aggregator.read_send_admission_from(reads.clone());
    register_the_second_mob(&host.aggregator, &host.runtime, &host.second);
    // No first sweep, nothing to compare a second with: the row is red at
    // its first assertion and waits for nothing.
    if failed.status == ConsoleFrameStatus::DeliveryFailed {
        tokio::time::sleep((took * 8).max(std::time::Duration::from_secs(2))).await;
    }
    let left = frame_in(host.store.as_ref(), Some(&host.second), &later)
        .await
        .status
        .as_str();
    let read = reads.all();
    host.shut_down().await;

    assert_eq!(
        failed.status.as_str(),
        "delivery_failed",
        "{log:?}: the mob's first registration swept its lost send"
    );
    assert_eq!(
        left, "submitted",
        "{log:?}: the mob's second registration sweeps nothing"
    );
    assert_eq!(
        read,
        Vec::<String>::new(),
        "{log:?}: the mob's second registration reads no send's evidence"
    );
}

#[tokio::test]
async fn a_registration_in_a_standing_ones_place_sweeps_nothing_after_a_completed_sweep_on_sqlite()
{
    a_registration_in_a_standing_ones_place_sweeps_nothing_after_a_completed_sweep(Log::Sqlite)
        .await;
}

#[tokio::test]
async fn a_registration_in_a_standing_ones_place_sweeps_nothing_after_a_completed_sweep_in_memory()
{
    a_registration_in_a_standing_ones_place_sweeps_nothing_after_a_completed_sweep(Log::Memory)
        .await;
}

/// A mob whose registration ENDED and whose console events register again
/// (it was closed and is loaded again in the process) is swept again: that
/// is a new registration. After the first registration failed the mob's
/// lost send, it ends; the log gets another submitted send of the mob that
/// no runtime holds an input of; the mob registers again: that send fails,
/// its outcome unknown.
async fn a_mob_loaded_again_after_a_close_is_swept_again(log: Log) {
    let host = swept_at_its_registration(log).await;
    let (failed, _) = host.the_lost_send_once_failed().await;
    host.aggregator
        .end_mob_console_events(&host.second, host.registration);
    let later = left_send(
        host.store.as_ref(),
        Some(&host.second),
        "alice",
        "after-the-load",
        ConsoleFrameStatus::Submitted,
        false,
    )
    .await;
    register_the_second_mob(&host.aggregator, &host.runtime, &host.second);
    let swept = once_failed(host.store.as_ref(), &host.second, &later).await;
    host.shut_down().await;

    assert_eq!(
        failed.status.as_str(),
        "delivery_failed",
        "{log:?}: the mob's first registration swept its lost send"
    );
    assert_eq!(
        swept.status.as_str(),
        "delivery_failed",
        "{log:?}: the mob loaded again is swept at its new registration"
    );
}

#[tokio::test]
async fn a_mob_loaded_again_after_a_close_is_swept_again_on_sqlite() {
    a_mob_loaded_again_after_a_close_is_swept_again(Log::Sqlite).await;
}

#[tokio::test]
async fn a_mob_loaded_again_after_a_close_is_swept_again_in_memory() {
    a_mob_loaded_again_after_a_close_is_swept_again(Log::Memory).await;
}

/// The frame `frame_id` of the mob `mob`, read once a sweep failed it, or as
/// it is when nothing failed it within ten seconds.
async fn once_failed(
    store: &dyn ConsoleLogStore,
    mob: &meerkat_mob::MobId,
    frame_id: &str,
) -> ConsoleFrame {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let frame = frame_in(store, Some(mob), frame_id).await;
        if frame.status == ConsoleFrameStatus::DeliveryFailed
            || tokio::time::Instant::now() >= deadline
        {
            return frame;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// A mob created again under an id is another mob: its sweep runs at its
/// first registration, although the earlier mob of that id was swept in
/// this process. The earlier mob's registration sweeps its lost send, the
/// registration ends and its runtime stops, and its frames are released as
/// a host releases them before it creates a mob under the id (removed, and
/// the writers' gates of the key let go once they rest); a mob is made
/// again under the id (another incarnation), the log gets a submitted send
/// of it that no runtime holds an input of, and its console events
/// register: that send fails, its outcome unknown.
async fn a_mob_created_again_under_its_id_is_swept_at_its_first_registration(log: Log) {
    let host = swept_at_its_registration(log).await;
    let first = once_failed(host.store.as_ref(), &host.second, &host.lost).await;
    let earlier = host.runtime.mob_runtime().handle().incarnation();
    host.aggregator
        .end_mob_console_events(&host.second, host.registration);
    host.runtime.shutdown().await;
    let key = super::mob_runtime_key(&host.second);
    let writers = super::mob_writers_of(&host.aggregator.inner);
    let rested = writers.rest(&key, std::time::Duration::from_secs(5)).await;
    let removed = host
        .store
        .remove_frames_of_mob(&host.second)
        .await
        .map_err(|error| error.to_string());
    writers.released(&key);
    let again = super::tests::build_empty_runtime(host.second.as_str()).await;
    let later = again.mob_runtime().handle().incarnation();
    let of_the_new_mob = left_send(
        host.store.as_ref(),
        Some(&host.second),
        "alice",
        "of-the-mob-made-again",
        ConsoleFrameStatus::Submitted,
        false,
    )
    .await;
    register_the_second_mob(&host.aggregator, &again, &host.second);
    let swept = once_failed(host.store.as_ref(), &host.second, &of_the_new_mob).await;
    again.shutdown().await;
    host.launch.shutdown().await;

    assert_eq!(
        first.status.as_str(),
        "delivery_failed",
        "{log:?}: the earlier mob's registration swept its lost send"
    );
    assert!(
        rested && removed.is_ok(),
        "{log:?}: the earlier mob's writers rest and its frames are removed: {removed:?}"
    );
    assert_ne!(
        earlier, later,
        "{log:?}: the mob made again is another incarnation"
    );
    assert_eq!(
        (
            swept.status.as_str(),
            swept
                .payload
                .pointer("/delivery_failure/reason")
                .and_then(Value::as_str),
        ),
        ("delivery_failed", Some("outcome_unknown")),
        "{log:?}: the mob made again under the id is swept at its first registration"
    );
}

#[tokio::test]
async fn a_mob_created_again_under_its_id_is_swept_at_its_first_registration_on_sqlite() {
    a_mob_created_again_under_its_id_is_swept_at_its_first_registration(Log::Sqlite).await;
}

#[tokio::test]
async fn a_mob_created_again_under_its_id_is_swept_at_its_first_registration_in_memory() {
    a_mob_created_again_under_its_id_is_swept_at_its_first_registration(Log::Memory).await;
}

/// A runtime over the mob `mob` whose launch is a candidate: it does not
/// speak for its composition.
async fn candidate_runtime(
    mob: &meerkat_mob::MobId,
    scratch: &tempfile::TempDir,
) -> UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        mob.as_str()
    ))
    .expect("mob definition");
    let spec = crate::MobBootstrapSpec::ephemeral(
        definition,
        meerkat_mob::MobStorage::in_memory(),
        scratch.path().to_path_buf(),
        4,
        None,
    )
    .expect("ephemeral spec")
    .with_options(crate::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
    })
    .with_composition_authority(
        crate::mob_composition_manifest::CompositionAuthority::NonAuthoritative,
    );
    UnifiedRuntime::bootstrap_with_options(
        spec,
        crate::MetaConfig {
            modules: Vec::new(),
            discovery: crate::DiscoverySpec {
                namespace: format!("{}-candidate", mob.as_str()),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Vec::new(),
        std::time::Duration::from_secs(2),
        crate::runtime::RuntimeOptions::default(),
        Arc::new(crate::InMemoryMetadataStore::new()),
    )
    .await
    .expect("the candidate runtime builds")
}

/// A launch that does not speak for its composition settles none of a
/// mob's sends when the mob's console events register: it counts them and
/// leaves them to the authoritative launch. The log holds a submitted send
/// of the mob that no runtime holds an input of; the mob's runtime is a
/// candidate's. Two seconds after the registration the send is still
/// submitted, and no send's evidence was read.
async fn a_candidate_launchs_registration_settles_none_of_the_mobs_sends(log: Log) {
    let scratch = tempfile::tempdir().expect("a directory for the runtime");
    let (store, _dir) = log_of_a_host(log);
    let mob = meerkat_mob::MobId::from(super::tests::unique_console_mob_id("send-rows-candidate"));
    let runtime = candidate_runtime(&mob, &scratch).await;
    let left = left_send(
        store.as_ref(),
        Some(&mob),
        "alice",
        "left-to-the-authoritative-launch",
        ConsoleFrameStatus::Submitted,
        false,
    )
    .await;
    let aggregator = console_over(Arc::clone(&store));
    let reads = Arc::new(RecordedReads::default());
    aggregator.read_send_admission_from(reads.clone());
    register_the_second_mob(&aggregator, &runtime, &mob);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let status = frame_in(store.as_ref(), Some(&mob), &left)
        .await
        .status
        .as_str();
    let read = reads.all();
    let speaks = runtime.mob_runtime().speaks_for_composition();
    runtime.shutdown().await;

    assert!(!speaks, "{log:?}: the runtime is a candidate's");
    assert_eq!(
        status, "submitted",
        "{log:?}: a candidate launch settles none of the mob's sends"
    );
    assert_eq!(
        read,
        Vec::<String>::new(),
        "{log:?}: a candidate launch reads no send's evidence"
    );
}

#[tokio::test]
async fn a_candidate_launchs_registration_settles_none_of_the_mobs_sends_on_sqlite() {
    a_candidate_launchs_registration_settles_none_of_the_mobs_sends(Log::Sqlite).await;
}

#[tokio::test]
async fn a_candidate_launchs_registration_settles_none_of_the_mobs_sends_in_memory() {
    a_candidate_launchs_registration_settles_none_of_the_mobs_sends(Log::Memory).await;
}

/// An operation of the store on a send by its frame id, its key or its
/// interaction, as the published store offers it: it names no mob.
#[derive(Clone, Copy, Debug)]
enum NamesNoMob {
    /// A status mark by frame id.
    Mark,
    /// A move of the send's own state by frame id.
    Move,
    /// A take of the send's delivery hold by frame id.
    Take,
    /// The read of a frame by its key.
    ByKey,
    /// The read of a send's frame by its interaction.
    ByInteraction,
}

/// Whether `operation` answered a frame for the send [`left_send`] stored
/// under `key`.
async fn answered_a_frame(
    store: &dyn ConsoleLogStore,
    operation: NamesNoMob,
    key: &str,
) -> Result<bool, String> {
    let frame_id = format!("frame-{key}");
    match operation {
        NamesNoMob::Mark => {
            store
                .update_frame_status(&frame_id, ConsoleFrameStatus::Delivered)
                .await
        }
        NamesNoMob::Move => {
            store
                .transition_send_frame(&frame_id, SendTransition::StartDispatch, None)
                .await
        }
        NamesNoMob::Take => store.take_send_frame_hold(&frame_id).await,
        NamesNoMob::ByKey => store.frame_by_dedupe_key(key).await,
        NamesNoMob::ByInteraction => {
            store
                .send_frame_by_interaction(&format!("interaction-{key}"))
                .await
        }
    }
    .map(|frame| frame.is_some())
    .map_err(|error| error.to_string())
}

/// An operation by frame id, by key or by interaction that names no mob
/// takes the default mob's frames alone, in both stores. Each mob holds an
/// accepted send with a delivery hold. The operation made for the second
/// mob's send answers no frame and leaves that send as it is: its status,
/// its version and its hold. The same operation made for the default mob's
/// send answers its frame.
async fn an_operation_that_names_no_mob_leaves_a_second_mobs_send(log: Log, operation: NamesNoMob) {
    const OF_THE_DEFAULT: &str = "of-the-default-mob";
    const OF_THE_SECOND: &str = "of-the-second-mob";
    let (store, _dir) = log_of_a_host(log);
    let second = meerkat_mob::MobId::from("send-rows-another-mob");
    left_send(
        store.as_ref(),
        None,
        "alice",
        OF_THE_DEFAULT,
        ConsoleFrameStatus::Accepted,
        true,
    )
    .await;
    let of_the_second = left_send(
        store.as_ref(),
        Some(&second),
        "alice",
        OF_THE_SECOND,
        ConsoleFrameStatus::Accepted,
        true,
    )
    .await;
    let before = frame_in(store.as_ref(), Some(&second), &of_the_second).await;
    let answered = [
        answered_a_frame(store.as_ref(), operation, OF_THE_SECOND).await,
        answered_a_frame(store.as_ref(), operation, OF_THE_DEFAULT).await,
    ];
    let after = frame_in(store.as_ref(), Some(&second), &of_the_second).await;

    assert_eq!(
        answered,
        [Ok(false), Ok(true)],
        "{log:?} {operation:?}: no frame for the second mob's send, the default mob's send's \
         own frame"
    );
    assert_eq!(
        (after.status.as_str(), after.frame_version, &after.payload),
        (
            before.status.as_str(),
            before.frame_version,
            &before.payload
        ),
        "{log:?} {operation:?}: the second mob's send is as it was"
    );
}

#[tokio::test]
async fn a_mark_that_names_no_mob_leaves_a_second_mobs_send_as_it_is_on_sqlite() {
    an_operation_that_names_no_mob_leaves_a_second_mobs_send(Log::Sqlite, NamesNoMob::Mark).await;
}

#[tokio::test]
async fn a_mark_that_names_no_mob_leaves_a_second_mobs_send_as_it_is_in_memory() {
    an_operation_that_names_no_mob_leaves_a_second_mobs_send(Log::Memory, NamesNoMob::Mark).await;
}

#[tokio::test]
async fn a_state_move_that_names_no_mob_leaves_a_second_mobs_send_as_it_is_on_sqlite() {
    an_operation_that_names_no_mob_leaves_a_second_mobs_send(Log::Sqlite, NamesNoMob::Move).await;
}

#[tokio::test]
async fn a_state_move_that_names_no_mob_leaves_a_second_mobs_send_as_it_is_in_memory() {
    an_operation_that_names_no_mob_leaves_a_second_mobs_send(Log::Memory, NamesNoMob::Move).await;
}

#[tokio::test]
async fn a_take_of_the_hold_that_names_no_mob_leaves_a_second_mobs_send_as_it_is_on_sqlite() {
    an_operation_that_names_no_mob_leaves_a_second_mobs_send(Log::Sqlite, NamesNoMob::Take).await;
}

#[tokio::test]
async fn a_take_of_the_hold_that_names_no_mob_leaves_a_second_mobs_send_as_it_is_in_memory() {
    an_operation_that_names_no_mob_leaves_a_second_mobs_send(Log::Memory, NamesNoMob::Take).await;
}

#[tokio::test]
async fn a_read_by_key_that_names_no_mob_answers_no_send_of_a_second_mob_on_sqlite() {
    an_operation_that_names_no_mob_leaves_a_second_mobs_send(Log::Sqlite, NamesNoMob::ByKey).await;
}

#[tokio::test]
async fn a_read_by_key_that_names_no_mob_answers_no_send_of_a_second_mob_in_memory() {
    an_operation_that_names_no_mob_leaves_a_second_mobs_send(Log::Memory, NamesNoMob::ByKey).await;
}

#[tokio::test]
async fn a_read_by_interaction_that_names_no_mob_answers_no_send_of_a_second_mob_on_sqlite() {
    an_operation_that_names_no_mob_leaves_a_second_mobs_send(
        Log::Sqlite,
        NamesNoMob::ByInteraction,
    )
    .await;
}

#[tokio::test]
async fn a_read_by_interaction_that_names_no_mob_answers_no_send_of_a_second_mob_in_memory() {
    an_operation_that_names_no_mob_leaves_a_second_mobs_send(
        Log::Memory,
        NamesNoMob::ByInteraction,
    )
    .await;
}

/// Whether `operation`, named for the mob `mob` (none: the default mob), answered
/// a frame for the send [`left_send`] stored under `key`.
async fn answered_a_frame_of(
    store: &dyn ConsoleLogStore,
    operation: NamesNoMob,
    mob: Option<&meerkat_mob::MobId>,
    key: &str,
) -> Result<bool, String> {
    answer_of(store, operation, mob, key)
        .await
        .map(|frame| frame.is_some())
        .map_err(|error| error.to_string())
}

/// What `operation`, named for the mob `mob`, answered for the send
/// [`left_send`] stored under `key`, as the store answered it.
async fn answer_of(
    store: &dyn ConsoleLogStore,
    operation: NamesNoMob,
    mob: Option<&meerkat_mob::MobId>,
    key: &str,
) -> ConsoleLogResult<Option<ConsoleFrame>> {
    let frame_id = format!("frame-{key}");
    match operation {
        NamesNoMob::Mark => {
            store
                .update_frame_status_of_mob(mob, &frame_id, ConsoleFrameStatus::Delivered)
                .await
        }
        NamesNoMob::Move => {
            store
                .transition_send_frame_of_mob(mob, &frame_id, SendTransition::StartDispatch, None)
                .await
        }
        NamesNoMob::Take => store.take_send_frame_hold_of_mob(mob, &frame_id).await,
        NamesNoMob::ByKey => store.frame_by_dedupe_key_of_mob(mob, key).await,
        NamesNoMob::ByInteraction => {
            store
                .send_frame_by_interaction_of_mob(mob, &format!("interaction-{key}"))
                .await
        }
    }
}

/// The refusal a store's provided form of an operation answered for a mob
/// beside the default mob, when that is what it answered.
fn refusal_in<T>(answer: &ConsoleLogResult<T>) -> Option<&OperationServesTheDefaultMobAlone> {
    answer.as_ref().err()?.downcast_ref()
}

/// An operation by frame id, by key or by interaction that names a mob takes
/// that mob's frames alone, in both stores. Each of the default mob and a
/// second mob holds an accepted send with a delivery hold. Named for a
/// third mob, the operation made for the second mob's send answers no
/// frame; named for the second mob, the one made for the default mob's send
/// answers none; and both sends are as they were (status, version, hold).
/// Named for the second mob, the operation made for that mob's send answers
/// its frame. Each operation runs on a log of its own.
async fn an_operation_that_names_another_mob_leaves_its_send_as_it_is(log: Log) {
    const OF_THE_DEFAULT: &str = "of-the-default-mob";
    const OF_THE_SECOND: &str = "of-the-second-mob";
    let second = meerkat_mob::MobId::from("send-rows-another-mob");
    let third = meerkat_mob::MobId::from("send-rows-a-third-mob");
    for operation in [
        NamesNoMob::Mark,
        NamesNoMob::Move,
        NamesNoMob::Take,
        NamesNoMob::ByKey,
        NamesNoMob::ByInteraction,
    ] {
        let (store, _dir) = log_of_a_host(log);
        let of_the_default = left_send(
            store.as_ref(),
            None,
            "alice",
            OF_THE_DEFAULT,
            ConsoleFrameStatus::Accepted,
            true,
        )
        .await;
        let of_the_second = left_send(
            store.as_ref(),
            Some(&second),
            "alice",
            OF_THE_SECOND,
            ConsoleFrameStatus::Accepted,
            true,
        )
        .await;
        let as_it_is = |frame: ConsoleFrame| (frame.status, frame.frame_version, frame.payload);
        let before = (
            as_it_is(frame_in(store.as_ref(), None, &of_the_default).await),
            as_it_is(frame_in(store.as_ref(), Some(&second), &of_the_second).await),
        );
        let named_for_another = [
            answered_a_frame_of(store.as_ref(), operation, Some(&third), OF_THE_SECOND).await,
            answered_a_frame_of(store.as_ref(), operation, Some(&second), OF_THE_DEFAULT).await,
        ];
        let after = (
            as_it_is(frame_in(store.as_ref(), None, &of_the_default).await),
            as_it_is(frame_in(store.as_ref(), Some(&second), &of_the_second).await),
        );
        let named_for_its_own =
            answered_a_frame_of(store.as_ref(), operation, Some(&second), OF_THE_SECOND).await;

        assert_eq!(
            named_for_another,
            [Ok(false), Ok(false)],
            "{log:?} {operation:?}: no frame for a send of a mob the operation does not name"
        );
        assert_eq!(
            after, before,
            "{log:?} {operation:?}: both sends are as they were"
        );
        assert_eq!(
            named_for_its_own,
            Ok(true),
            "{log:?} {operation:?}: the second mob's send's own frame, named for its mob"
        );
    }
}

#[tokio::test]
async fn an_operation_that_names_another_mob_leaves_its_send_as_it_is_on_sqlite() {
    an_operation_that_names_another_mob_leaves_its_send_as_it_is(Log::Sqlite).await;
}

#[tokio::test]
async fn an_operation_that_names_another_mob_leaves_its_send_as_it_is_in_memory() {
    an_operation_that_names_another_mob_leaves_its_send_as_it_is(Log::Memory).await;
}

/// A store that implements the operations that name no mob and the read of
/// one mob's frames, and keeps the provided forms of the six that name a
/// mob, as a store written before them does: it forwards each of those to
/// an in-memory log.
struct KeepsTheProvidedForms(InMemoryConsoleLogStore);

#[async_trait::async_trait]
impl ConsoleLogStore for KeepsTheProvidedForms {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        self.0.append_if_absent(frame).await
    }
    async fn update_frame_status(
        &self,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.0.update_frame_status(frame_id, status).await
    }
    async fn transition_send_frame(
        &self,
        frame_id: &str,
        transition: SendTransition,
        note: Option<(&'static str, Value)>,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.0
            .transition_send_frame(frame_id, transition, note)
            .await
    }
    fn supports_send_transitions(&self) -> bool {
        self.0.supports_send_transitions()
    }
    async fn take_send_frame_hold(&self, frame_id: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.0.take_send_frame_hold(frame_id).await
    }
    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage> {
        self.0.query_frames(query).await
    }
    async fn query_windowed_frames(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        self.0.query_windowed_frames(query).await
    }
    async fn query_windowed_frames_of_mob(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        self.0.query_windowed_frames_of_mob(query).await
    }
    fn reads_one_mob(&self) -> bool {
        self.0.reads_one_mob()
    }
    async fn frame_by_dedupe_key(&self, key: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.0.frame_by_dedupe_key(key).await
    }
    async fn send_frame_by_interaction(
        &self,
        interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.0.send_frame_by_interaction(interaction_id).await
    }
    async fn unsettled_send_frames(
        &self,
        window: usize,
        limit: usize,
    ) -> ConsoleLogResult<Vec<ConsoleFrame>> {
        self.0.unsettled_send_frames(window, limit).await
    }
    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        self.0.latest_cursor().await
    }
    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        self.0.clear_frames().await
    }
    async fn record_source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
        source_cursor: &str,
    ) -> ConsoleLogResult<()> {
        self.0
            .record_source_watermark(runtime_key, source_kind, source_cursor)
            .await
    }
    async fn source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        self.0.source_watermark(runtime_key, source_kind).await
    }
}

/// The provided forms of the six operations that name a mob, on a store
/// that keeps them: named for the default mob (no mob, or an empty one)
/// each answers what the operation that names no mob answers, the default
/// mob's send; named for another mob each refuses, and the default mob's
/// send is as it was. Each operation runs on a log of its own.
#[tokio::test]
async fn a_store_that_keeps_the_provided_forms_answers_the_default_mob_and_refuses_another() {
    const KEY: &str = "of-the-default-mob";
    let another = meerkat_mob::MobId::from("send-rows-another-mob");
    let empty = meerkat_mob::MobId::from("");
    for operation in [
        NamesNoMob::Mark,
        NamesNoMob::Move,
        NamesNoMob::Take,
        NamesNoMob::ByKey,
        NamesNoMob::ByInteraction,
    ] {
        let store = KeepsTheProvidedForms(InMemoryConsoleLogStore::new());
        let frame_id = left_send(
            &store,
            None,
            "alice",
            KEY,
            ConsoleFrameStatus::Accepted,
            true,
        )
        .await;
        let as_it_is = |frame: ConsoleFrame| (frame.status, frame.frame_version, frame.payload);
        let before = as_it_is(frame_in(&store, None, &frame_id).await);
        let for_another = answer_of(&store, operation, Some(&another), KEY).await;
        let after = as_it_is(frame_in(&store, None, &frame_id).await);
        let for_the_default = answered_a_frame_of(&store, operation, Some(&empty), KEY).await;

        assert_eq!(
            refusal_in(&for_another).map(|refusal| &refusal.mob),
            Some(&another),
            "{operation:?}: another mob is refused, by the refusal's type: {:?}",
            for_another
                .as_ref()
                .map(|frame| frame.as_ref().map(|frame| &frame.id))
        );
        assert_eq!(
            after, before,
            "{operation:?}: the default mob's send is as it was"
        );
        assert_eq!(
            for_the_default,
            Ok(true),
            "{operation:?}: an empty mob is the default mob, answered by the operation that names none"
        );
    }
    let store = KeepsTheProvidedForms(InMemoryConsoleLogStore::new());
    left_send(
        &store,
        None,
        "alice",
        KEY,
        ConsoleFrameStatus::Accepted,
        false,
    )
    .await;
    let listed = |frames: ConsoleLogResult<Vec<ConsoleFrame>>| {
        frames
            .map(|frames| {
                frames
                    .into_iter()
                    .map(|frame| frame.dedupe_key)
                    .collect::<Vec<_>>()
            })
            .map_err(|error| error.to_string())
    };
    assert_eq!(
        listed(store.unsettled_send_frames_of_mob(None, 16, 16).await),
        Ok(vec![KEY.to_string()]),
        "the default mob's unsettled sends, as the read that names no mob lists them"
    );
    let of_another = store
        .unsettled_send_frames_of_mob(Some(&another), 16, 16)
        .await;
    assert_eq!(
        refusal_in(&of_another).map(|refusal| &refusal.mob),
        Some(&another),
        "another mob's unsettled sends are refused, by the refusal's type"
    );
}

/// A console log that forwards every operation to `inner` and refuses the
/// first `fail_lists` lists of a mob's unsettled sends, the first
/// `fail_moves` moves of a mob's send through its states and the first
/// `fail_send_reads` reads of a send by its interaction and the first
/// `fail_gap_writes` writes of a gap marker, as a store with a passing
/// fault answers them.
struct FailsTheFirstLists {
    inner: Arc<dyn ConsoleLogStore>,
    fail_lists: std::sync::atomic::AtomicUsize,
    fail_moves: std::sync::atomic::AtomicUsize,
    fail_send_reads: std::sync::atomic::AtomicUsize,
    fail_gap_writes: std::sync::atomic::AtomicUsize,
}

/// Whether this call is one of the first calls `left` still counts: it is
/// then taken off the count.
fn one_of_the_first(left: &std::sync::atomic::AtomicUsize) -> bool {
    left.fetch_update(
        std::sync::atomic::Ordering::SeqCst,
        std::sync::atomic::Ordering::SeqCst,
        |left| left.checked_sub(1),
    )
    .is_ok()
}

#[async_trait::async_trait]
impl ConsoleLogStore for FailsTheFirstLists {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        if frame.identity == crate::console_contracts::CONSOLE_GAP_IDENTITY
            && one_of_the_first(&self.fail_gap_writes)
        {
            return Err(std::io::Error::other("the write of a gap marker failed").into());
        }
        self.inner.append_if_absent(frame).await
    }
    async fn update_frame_status(
        &self,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.update_frame_status(frame_id, status).await
    }
    async fn update_frame_status_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner
            .update_frame_status_of_mob(mob_id, frame_id, status)
            .await
    }
    async fn transition_send_frame(
        &self,
        frame_id: &str,
        transition: SendTransition,
        note: Option<(&'static str, Value)>,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner
            .transition_send_frame(frame_id, transition, note)
            .await
    }
    async fn transition_send_frame_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        frame_id: &str,
        transition: SendTransition,
        note: Option<(&'static str, Value)>,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        if one_of_the_first(&self.fail_moves) {
            return Err(std::io::Error::other("the move of a mob's send failed").into());
        }
        self.inner
            .transition_send_frame_of_mob(mob_id, frame_id, transition, note)
            .await
    }
    fn supports_send_transitions(&self) -> bool {
        self.inner.supports_send_transitions()
    }
    async fn take_send_frame_hold(&self, frame_id: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.take_send_frame_hold(frame_id).await
    }
    async fn take_send_frame_hold_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        frame_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner
            .take_send_frame_hold_of_mob(mob_id, frame_id)
            .await
    }
    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage> {
        self.inner.query_frames(query).await
    }
    async fn query_windowed_frames(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        self.inner.query_windowed_frames(query).await
    }
    async fn query_windowed_frames_of_mob(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        self.inner.query_windowed_frames_of_mob(query).await
    }
    fn reads_one_mob(&self) -> bool {
        self.inner.reads_one_mob()
    }
    async fn frame_by_dedupe_key(&self, key: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.frame_by_dedupe_key(key).await
    }
    async fn frame_by_dedupe_key_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        key: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.frame_by_dedupe_key_of_mob(mob_id, key).await
    }
    async fn send_frame_by_interaction(
        &self,
        interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.send_frame_by_interaction(interaction_id).await
    }
    async fn send_frame_by_interaction_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        if one_of_the_first(&self.fail_send_reads) {
            return Err(
                std::io::Error::other("the read of a send by its interaction failed").into(),
            );
        }
        self.inner
            .send_frame_by_interaction_of_mob(mob_id, interaction_id)
            .await
    }
    async fn unsettled_send_frames(
        &self,
        window: usize,
        limit: usize,
    ) -> ConsoleLogResult<Vec<ConsoleFrame>> {
        self.inner.unsettled_send_frames(window, limit).await
    }
    async fn unsettled_send_frames_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        window: usize,
        limit: usize,
    ) -> ConsoleLogResult<Vec<ConsoleFrame>> {
        if one_of_the_first(&self.fail_lists) {
            return Err(std::io::Error::other("the list of a mob's unsettled sends failed").into());
        }
        self.inner
            .unsettled_send_frames_of_mob(mob_id, window, limit)
            .await
    }
    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        self.inner.latest_cursor().await
    }
    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        self.inner.clear_frames().await
    }
    async fn record_source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
        source_cursor: &str,
    ) -> ConsoleLogResult<()> {
        self.inner
            .record_source_watermark(runtime_key, source_kind, source_cursor)
            .await
    }
    async fn source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        self.inner.source_watermark(runtime_key, source_kind).await
    }
}

/// A mob, its runtime and a console over `store` that holds no registration
/// of the mob yet; the mob's lost send (submitted, no runtime holds an input
/// of it) is in the log.
struct OneMobLeftASend {
    aggregator: MetaConsoleAggregator,
    store: Arc<dyn ConsoleLogStore>,
    runtime: UnifiedRuntime,
    mob: meerkat_mob::MobId,
    lost: String,
}

async fn one_mob_left_a_send(store: Arc<dyn ConsoleLogStore>, tag: &str) -> OneMobLeftASend {
    let mob = meerkat_mob::MobId::from(super::tests::unique_console_mob_id(tag));
    let runtime = super::tests::build_empty_runtime(mob.as_str()).await;
    let lost = left_send(
        store.as_ref(),
        Some(&mob),
        "alice",
        "lost",
        ConsoleFrameStatus::Submitted,
        false,
    )
    .await;
    let aggregator = console_over(Arc::clone(&store));
    OneMobLeftASend {
        aggregator,
        store,
        runtime,
        mob,
        lost,
    }
}

/// A sweep that could not list the mob's sends did not run: the mob stays
/// to be swept at its next registration. The log fails the first list; the
/// mob's first registration sweeps nothing (its lost send stays submitted
/// for two seconds); a registration in its place, while the first stands,
/// sweeps the mob and fails the lost send, its outcome unknown.
async fn a_sweep_that_could_not_list_leaves_the_mob_to_its_next_registration(log: Log) {
    let (inner, _dir) = log_of_a_host(log);
    let store: Arc<dyn ConsoleLogStore> = Arc::new(FailsTheFirstLists {
        inner,
        fail_lists: std::sync::atomic::AtomicUsize::new(1),
        fail_moves: std::sync::atomic::AtomicUsize::new(0),
        fail_send_reads: std::sync::atomic::AtomicUsize::new(0),
        fail_gap_writes: std::sync::atomic::AtomicUsize::new(0),
    });
    let host = one_mob_left_a_send(store, "send-rows-list-fails").await;
    register_the_second_mob(&host.aggregator, &host.runtime, &host.mob);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let after_the_failed_list = frame_in(host.store.as_ref(), Some(&host.mob), &host.lost)
        .await
        .status
        .as_str();
    register_the_second_mob(&host.aggregator, &host.runtime, &host.mob);
    let swept = once_failed(host.store.as_ref(), &host.mob, &host.lost).await;
    host.runtime.shutdown().await;

    assert_eq!(
        after_the_failed_list, "submitted",
        "{log:?}: a sweep that could not list the sends settled none"
    );
    assert_eq!(
        swept.status.as_str(),
        "delivery_failed",
        "{log:?}: the mob's next registration sweeps it"
    );
}

#[tokio::test]
async fn a_sweep_that_could_not_list_leaves_the_mob_to_its_next_registration_on_sqlite() {
    a_sweep_that_could_not_list_leaves_the_mob_to_its_next_registration(Log::Sqlite).await;
}

#[tokio::test]
async fn a_sweep_that_could_not_list_leaves_the_mob_to_its_next_registration_in_memory() {
    a_sweep_that_could_not_list_leaves_the_mob_to_its_next_registration(Log::Memory).await;
}

/// Where the sweep of the mob's registration `registration` stands once it
/// no longer runs, read at the registration's own gate: owed again, or
/// completed. As it stands after a minute when it still runs: no passing
/// row waits that long.
async fn sweep_once_over(
    aggregator: &MetaConsoleAggregator,
    mob: &meerkat_mob::MobId,
    registration: uuid::Uuid,
) -> Option<super::mob_writers::Sweep> {
    let writers = super::mob_writers_of(&aggregator.inner);
    let key = super::mob_runtime_key(mob);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_mins(1);
    loop {
        let sweep = writers.sweep(&key, registration);
        if sweep != Some(super::mob_writers::Sweep::Running)
            || tokio::time::Instant::now() >= deadline
        {
            return sweep;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// A sweep cut at its limit leaves the rest of the mob's sends to the
/// mob's next registration. The log holds one more unsettled send of the
/// mob than a sweep examines: a lost send (the oldest), accepted sends
/// that no sweep settles (the runtime holds no input of them and they were
/// not submitted), a second lost send (the last one within the limit) and
/// a third (the newest, past the limit). The first registration's sweep
/// reads the evidence of as many sends as its limit, fails the two lost
/// sends among them and does not read the newest, which stays submitted;
/// its gate says the sweep is owed again. A registration in its place,
/// while the first stands, sweeps again: it reads the newest, fails it,
/// and its sweep completed.
///
/// Each sweep is waited for at its registration's gate (it no longer
/// runs), not for an interval, and no sweep here can meet its deadline:
/// the evidence comes from a reader that answers at once, and the two
/// sweeps settle three sends between them.
async fn a_sweep_cut_at_its_limit_leaves_the_rest_to_the_next_registration(log: Log) {
    use super::mob_writers::Sweep;

    let (store, _dir) = log_of_a_host(log);
    let host = one_mob_left_a_send(store, "send-rows-limit").await;
    let reads = Arc::new(RecordedReads::default());
    host.aggregator.read_send_admission_from(reads.clone());
    for index in 0..send_recovery::SEND_SWEEP_LIMIT - 2 {
        left_send(
            host.store.as_ref(),
            Some(&host.mob),
            "alice",
            &format!("accepted-{index}"),
            ConsoleFrameStatus::Accepted,
            false,
        )
        .await;
    }
    let last_within = left_send(
        host.store.as_ref(),
        Some(&host.mob),
        "alice",
        "lost-within-the-limit",
        ConsoleFrameStatus::Submitted,
        false,
    )
    .await;
    let newest = left_send(
        host.store.as_ref(),
        Some(&host.mob),
        "alice",
        "lost-past-the-limit",
        ConsoleFrameStatus::Submitted,
        false,
    )
    .await;
    let status_of = |frame_id: &str| {
        let (store, mob, frame_id) = (
            Arc::clone(&host.store),
            host.mob.clone(),
            frame_id.to_string(),
        );
        async move {
            frame_in(store.as_ref(), Some(&mob), &frame_id)
                .await
                .status
                .as_str()
        }
    };
    // The sends whose evidence was read, by the frame id each is first read
    // under.
    let sends_read = || {
        reads
            .all()
            .into_iter()
            .filter(|read| read.starts_with("frame-"))
            .collect::<Vec<_>>()
    };

    let first = register_the_second_mob(&host.aggregator, &host.runtime, &host.mob);
    let first_sweep = sweep_once_over(&host.aggregator, &host.mob, first).await;
    let after_the_first = (
        status_of(&host.lost).await,
        status_of(&last_within).await,
        status_of(&newest).await,
    );
    let read_by_the_first = sends_read();
    let second = register_the_second_mob(&host.aggregator, &host.runtime, &host.mob);
    let second_sweep = sweep_once_over(&host.aggregator, &host.mob, second).await;
    let after_the_second = status_of(&newest).await;
    let read_by_both = sends_read();
    host.runtime.shutdown().await;

    assert_eq!(
        (
            first_sweep,
            after_the_first,
            read_by_the_first.len(),
            read_by_the_first.contains(&newest),
        ),
        (
            Some(Sweep::Due),
            ("delivery_failed", "delivery_failed", "submitted"),
            send_recovery::SEND_SWEEP_LIMIT,
            false,
        ),
        "{log:?}: the first sweep reads as many sends as its limit, the oldest first, fails \
         the lost sends among them and leaves the newest unread; the sweep is owed again"
    );
    assert_eq!(
        (
            second_sweep,
            after_the_second,
            read_by_both
                .iter()
                .skip(read_by_the_first.len())
                .any(|read| read == &newest),
        ),
        (Some(Sweep::Completed), "delivery_failed", true),
        "{log:?}: the next registration sweeps the send the limit left, and its sweep \
         completed"
    );
}

#[tokio::test]
async fn a_sweep_cut_at_its_limit_leaves_the_rest_to_the_next_registration_on_sqlite() {
    a_sweep_cut_at_its_limit_leaves_the_rest_to_the_next_registration(Log::Sqlite).await;
}

#[tokio::test]
async fn a_sweep_cut_at_its_limit_leaves_the_rest_to_the_next_registration_in_memory() {
    a_sweep_cut_at_its_limit_leaves_the_rest_to_the_next_registration(Log::Memory).await;
}

/// A reader of the runtime's evidence that holds each read until it is let
/// go, then answers that the runtime holds no input of the send.
struct HeldReads {
    asked: tokio::sync::Notify,
    go: tokio::sync::watch::Sender<bool>,
}

impl HeldReads {
    fn new() -> Self {
        Self {
            asked: tokio::sync::Notify::new(),
            go: tokio::sync::watch::channel(false).0,
        }
    }

    async fn held(&self) -> Result<Option<super::send_recovery::AdmittedInput>, String> {
        self.asked.notify_one();
        let mut go = self.go.subscribe();
        go.wait_for(|go| *go)
            .await
            .map_err(|error| error.to_string())?;
        Ok(None)
    }
}

#[async_trait::async_trait]
impl super::send_recovery::SendAdmissionReader for HeldReads {
    async fn admitted_input(
        &self,
        _session_id: &meerkat_core::types::SessionId,
        _idempotency_key: &str,
    ) -> Result<Option<super::send_recovery::AdmittedInput>, String> {
        self.held().await
    }

    async fn admitted_input_by_id(
        &self,
        _session_id: &meerkat_core::types::SessionId,
        _input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<Option<super::send_recovery::AdmittedInput>, String> {
        self.held().await
    }
}

/// A sweep of a registration that ended settles nothing: it was started for
/// that registration, and writes for it alone. The mob's registration
/// starts its sweep, whose read of the lost send's evidence is held; the
/// registration ends; the read is let go (the runtime holds no input of the
/// send). The lost send stays submitted.
async fn a_sweep_of_a_registration_that_ended_settles_nothing(log: Log) {
    let (store, _dir) = log_of_a_host(log);
    let host = one_mob_left_a_send(store, "send-rows-ended-sweep").await;
    let reads = Arc::new(HeldReads::new());
    host.aggregator.read_send_admission_from(reads.clone());
    let registration = register_the_second_mob(&host.aggregator, &host.runtime, &host.mob);
    let asked = tokio::time::timeout(std::time::Duration::from_secs(10), reads.asked.notified())
        .await
        .is_ok();
    host.aggregator
        .end_mob_console_events(&host.mob, registration);
    reads.go.send_replace(true);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let left = frame_in(host.store.as_ref(), Some(&host.mob), &host.lost)
        .await
        .status
        .as_str();
    host.runtime.shutdown().await;

    assert!(
        asked,
        "{log:?}: the registration's sweep read the lost send's evidence"
    );
    assert_eq!(
        left, "submitted",
        "{log:?}: the sweep of a registration that ended settles nothing"
    );
}

#[tokio::test]
async fn a_sweep_of_a_registration_that_ended_settles_nothing_on_sqlite() {
    a_sweep_of_a_registration_that_ended_settles_nothing(Log::Sqlite).await;
}

#[tokio::test]
async fn a_sweep_of_a_registration_that_ended_settles_nothing_in_memory() {
    a_sweep_of_a_registration_that_ended_settles_nothing(Log::Memory).await;
}

/// The read of the send a frame names, for the frame's witness, is an
/// operation of the store: a store that refuses it refuses the frame's
/// projection. Over a log that reads one mob's frames and keeps the
/// provided operations that name a mob, a hosted mob's event that names an
/// interaction (and no session) is not stored with no witness: its
/// projection answers the refusal, and the log holds no frame of it.
#[tokio::test]
async fn a_refused_read_of_the_send_a_frame_names_is_an_error_of_its_projection() {
    let store: Arc<dyn ConsoleLogStore> =
        Arc::new(KeepsTheProvidedForms(InMemoryConsoleLogStore::new()));
    let host = one_mob_left_a_send(Arc::clone(&store), "send-rows-witness-refused").await;
    let registration = register_the_second_mob(&host.aggregator, &host.runtime, &host.mob);
    let projected = host
        .aggregator
        .project_mob_console_events(
            &host.mob,
            registration,
            vec![crate::console_contracts::ConsoleIdentityEventEnvelope {
                event_id: "witness-refused-event".to_string(),
                interaction_id: Some("interaction-of-a-send".to_string()),
                identity: "alice".to_string(),
                event_type: "identity_respawned".to_string(),
                timestamp_ms: 1,
                data: json!({ "text": "names a send" }),
            }],
        )
        .await
        .map_err(|error| error.to_string());
    let stored: Vec<String> = store
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: Some(host.mob.clone()),
            limit: 500,
            ..Default::default()
        })
        .await
        .expect("the mob's frames")
        .frames
        .into_iter()
        .filter(|frame| frame.source_event_id.as_deref() == Some("witness-refused-event"))
        .map(|frame| frame.id)
        .collect();
    host.runtime.shutdown().await;

    assert!(
        projected.is_err(),
        "the projection answers the store's refusal: {projected:?}"
    );
    assert_eq!(
        stored,
        Vec::<String>::new(),
        "no frame of the event is stored"
    );
}

/// The legacy reasoning key of an event is read among the frames of the
/// event's own mob. A hosted mob's reasoning frame stored under the key an
/// older log gave it is found again for the same event of that mob, and
/// the event keeps that key; the same event named for the default mob does
/// not take another mob's frame's key.
#[tokio::test]
async fn the_legacy_reasoning_key_of_an_event_is_read_among_its_mobs_frames() {
    let (store, _dir) = log_of_a_host(Log::Memory);
    let mob = meerkat_mob::MobId::from("send-rows-legacy-reasoning");
    let runtime_key = format!("mob/{}", mob.as_str());
    let text = "thinking about the send";
    let legacy_key = format!(
        "console-reasoning:{runtime_key}:interaction-1:{}",
        hash_short(&normalize_transcript_fingerprint_text(text))
    );
    let reasoning = |mob_id: Option<&meerkat_mob::MobId>, dedupe_key: &str| NewConsoleFrame {
        mob_id: mob_id.cloned(),
        id: Some("legacy-reasoning-frame".to_string()),
        dedupe_key: dedupe_key.to_string(),
        timestamp_ms: 1,
        runtime_key: runtime_key.clone(),
        identity: "alice".to_string(),
        conversation_id: Some("alice".to_string()),
        session_id: None,
        kind: "reasoning_delta".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({ "delta": text }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::ConsoleEvent,
            source_cursor: None,
        },
        source_event_id: Some("reasoning-event".to_string()),
        interaction_id: Some("interaction-1".to_string()),
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    };
    store
        .append_if_absent(reasoning(Some(&mob), &legacy_key))
        .await
        .expect("the legacy frame is stored");
    let mut of_its_mob = reasoning(Some(&mob), "console-reasoning-new-key");
    retain_legacy_reasoning_replay_key(store.as_ref(), &mut of_its_mob)
        .await
        .expect("the legacy key is read");
    let mut of_the_default_mob = reasoning(None, "console-reasoning-new-key");
    retain_legacy_reasoning_replay_key(store.as_ref(), &mut of_the_default_mob)
        .await
        .expect("the legacy key is read");

    assert_eq!(
        of_its_mob.dedupe_key, legacy_key,
        "the event of the mob keeps its frame's legacy key"
    );
    assert_eq!(
        of_the_default_mob.dedupe_key, "console-reasoning-new-key",
        "the event named for the default mob takes no other mob's frame's key"
    );
}

/// A reader of the runtime's evidence whose first reads fail (as many as it
/// was made with: the runtime's store did not answer them) and that answers
/// every later read that the runtime holds no input of the send.
struct FailsTheFirstReads(std::sync::atomic::AtomicUsize);

impl FailsTheFirstReads {
    fn read(&self) -> Result<Option<super::send_recovery::AdmittedInput>, String> {
        let fails = self
            .0
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok();
        if fails {
            Err("the runtime's store did not answer the read".to_string())
        } else {
            Ok(None)
        }
    }

    fn left(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl super::send_recovery::SendAdmissionReader for FailsTheFirstReads {
    async fn admitted_input(
        &self,
        _session_id: &meerkat_core::types::SessionId,
        _idempotency_key: &str,
    ) -> Result<Option<super::send_recovery::AdmittedInput>, String> {
        self.read()
    }

    async fn admitted_input_by_id(
        &self,
        _session_id: &meerkat_core::types::SessionId,
        _input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<Option<super::send_recovery::AdmittedInput>, String> {
        self.read()
    }
}

/// A sweep that could not read a send's evidence did not run to its end:
/// the mob stays to be swept at its next registration. The first read of
/// the runtime's evidence fails; the mob's first registration leaves its
/// lost send submitted (unknown evidence settles nothing); a registration
/// in its place, while the first stands, sweeps the mob and fails the lost
/// send, its outcome unknown.
async fn a_sweep_that_could_not_read_a_sends_evidence_leaves_the_mob_to_its_next_registration(
    log: Log,
) {
    let (store, _dir) = log_of_a_host(log);
    let host = one_mob_left_a_send(store, "send-rows-evidence-unread").await;
    let reads = Arc::new(FailsTheFirstReads(std::sync::atomic::AtomicUsize::new(1)));
    host.aggregator.read_send_admission_from(reads.clone());
    register_the_second_mob(&host.aggregator, &host.runtime, &host.mob);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while reads.left() > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let read_failed = reads.left() == 0;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let after_the_unread_evidence = frame_in(host.store.as_ref(), Some(&host.mob), &host.lost)
        .await
        .status
        .as_str();
    register_the_second_mob(&host.aggregator, &host.runtime, &host.mob);
    let swept = once_failed(host.store.as_ref(), &host.mob, &host.lost).await;
    host.runtime.shutdown().await;

    assert!(
        read_failed,
        "{log:?}: the first registration's sweep read the lost send's evidence, and the read \
         failed"
    );
    assert_eq!(
        after_the_unread_evidence, "submitted",
        "{log:?}: a send whose evidence was not read is not settled"
    );
    assert_eq!(
        swept.status.as_str(),
        "delivery_failed",
        "{log:?}: the mob's next registration sweeps it"
    );
}

#[tokio::test]
async fn a_sweep_that_could_not_read_a_sends_evidence_leaves_the_mob_to_its_next_registration_on_sqlite()
 {
    a_sweep_that_could_not_read_a_sends_evidence_leaves_the_mob_to_its_next_registration(
        Log::Sqlite,
    )
    .await;
}

#[tokio::test]
async fn a_sweep_that_could_not_read_a_sends_evidence_leaves_the_mob_to_its_next_registration_in_memory()
 {
    a_sweep_that_could_not_read_a_sends_evidence_leaves_the_mob_to_its_next_registration(
        Log::Memory,
    )
    .await;
}

/// A sweep that could not settle a send did not run to its end: the mob
/// stays to be swept at its next registration. The log fails the first
/// move of a send through its states; the mob's first registration's sweep
/// reads that the runtime holds no input of the lost send and cannot fail
/// it (the send stays submitted for two seconds); a registration in its
/// place, while the first stands, sweeps the mob and fails the lost send,
/// its outcome unknown.
async fn a_sweep_that_could_not_settle_a_send_leaves_the_mob_to_its_next_registration(log: Log) {
    let (inner, _dir) = log_of_a_host(log);
    let store: Arc<dyn ConsoleLogStore> = Arc::new(FailsTheFirstLists {
        inner,
        fail_lists: std::sync::atomic::AtomicUsize::new(0),
        fail_moves: std::sync::atomic::AtomicUsize::new(1),
        fail_send_reads: std::sync::atomic::AtomicUsize::new(0),
        fail_gap_writes: std::sync::atomic::AtomicUsize::new(0),
    });
    let host = one_mob_left_a_send(store, "send-rows-settle-fails").await;
    register_the_second_mob(&host.aggregator, &host.runtime, &host.mob);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let after_the_failed_move = frame_in(host.store.as_ref(), Some(&host.mob), &host.lost)
        .await
        .status
        .as_str();
    register_the_second_mob(&host.aggregator, &host.runtime, &host.mob);
    let swept = once_failed(host.store.as_ref(), &host.mob, &host.lost).await;
    host.runtime.shutdown().await;

    assert_eq!(
        after_the_failed_move, "submitted",
        "{log:?}: a sweep whose move of the send failed settled nothing"
    );
    assert_eq!(
        swept.status.as_str(),
        "delivery_failed",
        "{log:?}: the mob's next registration sweeps it"
    );
}

#[tokio::test]
async fn a_sweep_that_could_not_settle_a_send_leaves_the_mob_to_its_next_registration_on_sqlite() {
    a_sweep_that_could_not_settle_a_send_leaves_the_mob_to_its_next_registration(Log::Sqlite).await;
}

#[tokio::test]
async fn a_sweep_that_could_not_settle_a_send_leaves_the_mob_to_its_next_registration_in_memory() {
    a_sweep_that_could_not_settle_a_send_leaves_the_mob_to_its_next_registration(Log::Memory).await;
}

/// The look by key of a registration's member (the session history passes'
/// and the notice pass's) reads the frames of the mob registered under the
/// runtime key, and the default mob's for a key no mob is registered under.
/// The log holds a frame of the second mob and one of the default mob,
/// each under a key of its own.
#[tokio::test]
async fn the_look_by_key_of_a_registrations_member_reads_that_registrations_mob() {
    let host = console_of_two_mobs_over(Log::Memory).await;
    let store = Arc::clone(&host.aggregator.inner.store);
    let of_the_second = left_send(
        store.as_ref(),
        Some(&host.second),
        "alice",
        "of-the-second-mob",
        ConsoleFrameStatus::Accepted,
        false,
    )
    .await;
    let of_the_default = left_send(
        store.as_ref(),
        None,
        "alice",
        "of-the-default-mob",
        ConsoleFrameStatus::Accepted,
        false,
    )
    .await;
    let second_key = format!("mob/{}", host.second.as_str());
    let look = |runtime_key: String, dedupe_key: &'static str| {
        let inner = Arc::clone(&host.aggregator.inner);
        async move {
            frame_of_registration_by_key(&inner, &runtime_key, dedupe_key)
                .await
                .map(|frame| frame.map(|frame| frame.id))
                .map_err(|error| error.to_string())
        }
    };
    let found = [
        look(second_key.clone(), "of-the-second-mob").await,
        look(second_key, "of-the-default-mob").await,
        look("runtime-a".to_string(), "of-the-second-mob").await,
        look("runtime-a".to_string(), "of-the-default-mob").await,
    ];
    host.runtime.shutdown().await;

    assert_eq!(
        found,
        [
            Ok(Some(of_the_second)),
            Ok(None),
            Ok(None),
            Ok(Some(of_the_default)),
        ],
        "the second mob's registration reads its mob's frame alone; a key no mob is \
         registered under reads the default mob's"
    );
}

/// A sweep leaves a send whose dispatch another owner holds (a live send,
/// or the sweep of an earlier registration) to that owner, and such a sweep
/// did not run to its end: the mob stays to be swept at its next
/// registration. The dispatch of the mob's lost send is held when the mob's
/// first registration sweeps: the send stays submitted, and the
/// registration's gate says its sweep is owed again. The holder lets go
/// without settling it; a registration in the first one's place, while the
/// first stands, sweeps the mob and fails the send, its outcome unknown, and
/// that sweep completed. Each sweep is waited for at its registration's
/// gate.
async fn a_send_another_owner_holds_leaves_the_mob_to_its_next_registration(log: Log) {
    use super::mob_writers::Sweep;

    let (store, _dir) = log_of_a_host(log);
    let host = one_mob_left_a_send(store, "send-rows-held-dispatch").await;
    let held = host.aggregator.inner.send_dispatch_claims.claim(&host.lost);
    let was_held = held.is_some();
    let first = register_the_second_mob(&host.aggregator, &host.runtime, &host.mob);
    let first_sweep = sweep_once_over(&host.aggregator, &host.mob, first).await;
    let while_held = frame_in(host.store.as_ref(), Some(&host.mob), &host.lost)
        .await
        .status
        .as_str();
    drop(held);
    let second = register_the_second_mob(&host.aggregator, &host.runtime, &host.mob);
    let second_sweep = sweep_once_over(&host.aggregator, &host.mob, second).await;
    let swept = frame_in(host.store.as_ref(), Some(&host.mob), &host.lost).await;
    host.runtime.shutdown().await;

    assert!(
        was_held,
        "{log:?}: the row holds the dispatch of the lost send"
    );
    assert_eq!(
        (first_sweep, while_held),
        (Some(Sweep::Due), "submitted"),
        "{log:?}: a sweep settles no send whose dispatch another owner holds, and is owed \
         again"
    );
    assert_eq!(
        (second_sweep, swept.status.as_str()),
        (Some(Sweep::Completed), "delivery_failed"),
        "{log:?}: the mob's next registration sweeps the send the first sweep left to its \
         owner, and its sweep completed"
    );
}

#[tokio::test]
async fn a_send_another_owner_holds_leaves_the_mob_to_its_next_registration_on_sqlite() {
    a_send_another_owner_holds_leaves_the_mob_to_its_next_registration(Log::Sqlite).await;
}

#[tokio::test]
async fn a_send_another_owner_holds_leaves_the_mob_to_its_next_registration_in_memory() {
    a_send_another_owner_holds_leaves_the_mob_to_its_next_registration(Log::Memory).await;
}

/// A sweep leaves a send whose dispatch is alive, and the dispatch settles
/// it. A governed send to the mob's member is reserved (its caller holds
/// its dispatch), started and submitted; the runtime holds no input of it,
/// so a sweep that took it would fail it. The sweep of the mob's
/// registration, run while the dispatch is held, examines the send, reads
/// no evidence of it and settles nothing: it stays submitted, and the sweep
/// did not run to its end (the send is left to its dispatch's owner). The
/// dispatch then marks it delivered and lets go; a sweep after that finds
/// nothing to settle, the send stays delivered, and that sweep ran to its
/// end.
async fn a_sweep_leaves_a_send_whose_dispatch_is_alive(log: Log) {
    let (store, _dir) = log_of_a_host(log);
    let mob = meerkat_mob::MobId::from(super::tests::unique_console_mob_id(
        "send-rows-live-dispatch",
    ));
    let runtime = super::tests::build_empty_runtime(mob.as_str()).await;
    let aggregator = console_over(Arc::clone(&store));
    let reads = Arc::new(RecordedReads::default());
    aggregator.read_send_admission_from(reads.clone());
    let registration = register_the_second_mob(&aggregator, &runtime, &mob);
    let reservation = aggregator
        .reserve_governed_interaction_for_mob(
            request("alice", "live"),
            Some(&mob),
            Some(SESSION),
            &human("u1"),
            None,
        )
        .await
        .expect("the governed send is reserved");
    let (send, dispatch) = match reservation {
        ScopedReservation::Fresh(send, Some(dispatch)) => (send, dispatch),
        other => panic!("expected a new send whose caller holds its dispatch: {other:?}"),
    };
    let id = send.input_frame_id.clone();
    let bound = meerkat_core::types::SessionId::new();
    let started = answer(
        &aggregator
            .mark_interaction_dispatching_of_mob(Some(&mob), &id)
            .await,
    );
    let submitted = answer(
        &aggregator
            .mark_interaction_submitted_of_mob(Some(&mob), &id, &bound)
            .await,
    );
    let swept_while_held = aggregator
        .reconcile_interrupted_sends_of_mob(Some(&mob), Some(registration))
        .await;
    let while_held = frame_in(store.as_ref(), Some(&mob), &id)
        .await
        .status
        .as_str();
    let read_while_held = reads.all();
    let delivered = answer(
        &aggregator
            .mark_interaction_delivered_of_mob(Some(&mob), &id)
            .await,
    );
    drop(dispatch);
    let swept_after = aggregator
        .reconcile_interrupted_sends_of_mob(Some(&mob), Some(registration))
        .await;
    let after = frame_in(store.as_ref(), Some(&mob), &id)
        .await
        .status
        .as_str();
    runtime.shutdown().await;

    assert_eq!(
        [started.as_str(), submitted.as_str(), delivered.as_str()],
        ["ok", "ok", "ok"],
        "{log:?}: the send's own dispatch starts it, submits it and marks it delivered"
    );
    assert_eq!(
        (
            swept_while_held.examined,
            swept_while_held.delivered,
            swept_while_held.lost,
            swept_while_held.completed,
            while_held,
        ),
        (1, 0, 0, false, "submitted"),
        "{log:?}: the sweep examines the send whose dispatch is alive, settles nothing and \
         did not run to its end"
    );
    assert_eq!(
        read_while_held,
        Vec::<String>::new(),
        "{log:?}: the sweep reads no evidence of a send whose dispatch is alive"
    );
    assert_eq!(
        (
            swept_after.delivered,
            swept_after.lost,
            swept_after.completed,
            after,
        ),
        (0, 0, true, "delivered"),
        "{log:?}: the dispatch settled its send; a later sweep leaves it as it is and runs \
         to its end"
    );
}

#[tokio::test]
async fn a_sweep_leaves_a_send_whose_dispatch_is_alive_on_sqlite() {
    a_sweep_leaves_a_send_whose_dispatch_is_alive(Log::Sqlite).await;
}

#[tokio::test]
async fn a_sweep_leaves_a_send_whose_dispatch_is_alive_in_memory() {
    a_sweep_leaves_a_send_whose_dispatch_is_alive(Log::Memory).await;
}

/// The log lines a row's own thread writes.
#[derive(Clone, Default)]
struct CapturedLog(Arc<std::sync::Mutex<Vec<u8>>>);

impl CapturedLog {
    fn contents(&self) -> String {
        String::from_utf8_lossy(
            &self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned()
    }
}

impl std::io::Write for CapturedLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLog {
    type Writer = CapturedLog;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Whose console event a row projects.
#[derive(Clone, Copy, Debug)]
enum EventOf {
    /// A runtime registered for the host's default mob.
    TheDefaultMob,
    /// A mob the host serves beside its default mob.
    AHostedMob,
}

/// Which of a registration's two tasks projects the row's event.
#[derive(Clone, Copy, Debug)]
enum ProjectedBy {
    /// The replay of what the event store held when it was registered.
    TheReplay,
    /// The live projection of an event made after the registration.
    TheLiveProjection,
}

/// A console event that a registration's replay or live projection could
/// not write is said in a warning: those tasks have no caller to answer,
/// and a dropped error would leave the missing frame unexplained. The log
/// fails every read of a send by its interaction; an event of the member
/// `alice` that names a send's interaction and no session is made in the
/// registration's event store (before the registration for the replay,
/// after it for the live projection). Its frame's witness is read from the
/// send it names, the read fails, the frame is not written, and a warning
/// names the registration's runtime key and the task that could not write
/// it. No line of the log holds the event's text: the warning carries the
/// key, the task and the store's error.
async fn an_event_that_could_not_be_written_is_said_in_a_warning(of: EventOf, by: ProjectedBy) {
    const WARNING: &str = "a console event was not written to the console log";
    const EVENT_TEXT: &str = "names a send";
    let task = match by {
        ProjectedBy::TheReplay => "task=\"replay\"",
        ProjectedBy::TheLiveProjection => "task=\"live projection\"",
    };
    let (inner, _dir) = log_of_a_host(Log::Memory);
    let store: Arc<dyn ConsoleLogStore> = Arc::new(FailsTheFirstLists {
        inner,
        fail_lists: std::sync::atomic::AtomicUsize::new(0),
        fail_moves: std::sync::atomic::AtomicUsize::new(0),
        fail_send_reads: std::sync::atomic::AtomicUsize::new(usize::MAX),
        fail_gap_writes: std::sync::atomic::AtomicUsize::new(0),
    });
    let mob = meerkat_mob::MobId::from(super::tests::unique_console_mob_id(
        "send-rows-event-not-written",
    ));
    let runtime = super::tests::build_empty_runtime(mob.as_str()).await;
    let aggregator = console_over(Arc::clone(&store));
    let events = runtime.console_events();

    let log = CapturedLog::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .finish();
    // A second registered Dispatch keeps callsite interest from being
    // cached `never` by threads without a subscriber.
    let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
    let logged = tracing::subscriber::set_default(subscriber);

    if matches!(by, ProjectedBy::TheReplay) {
        events
            .append(
                "alice",
                Some("interaction-of-a-send".to_string()),
                "identity_respawned",
                json!({ "text": EVENT_TEXT }),
            )
            .await;
    }
    let runtime_key = match of {
        EventOf::AHostedMob => {
            register_the_second_mob(&aggregator, &runtime, &mob);
            format!("mob/{}", mob.as_str())
        }
        EventOf::TheDefaultMob => {
            aggregator.register_runtime_handles_with_policy(
                "runtime-a",
                "",
                runtime.mob_runtime().clone(),
                None,
                runtime.console_events(),
                Arc::new(AllowAllConsoleVisibilityPolicy),
            );
            "runtime-a".to_string()
        }
    };
    let mut replay_over = true;
    if matches!(by, ProjectedBy::TheLiveProjection) {
        // The registration's replay of its (empty) event store is over
        // before the event is made, so only the live projection sees it:
        // the console says that no pass is owed.
        replay_over = tokio::time::timeout(
            std::time::Duration::from_mins(1),
            aggregator.history_backfill_converged(),
        )
        .await
        .is_ok();
        events
            .append(
                "alice",
                Some("interaction-of-a-send".to_string()),
                "identity_respawned",
                json!({ "text": EVENT_TEXT }),
            )
            .await;
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let warned = loop {
        let said = log.contents().lines().any(|line| {
            line.contains("WARN")
                && line.contains(WARNING)
                && line.contains(&runtime_key)
                && line.contains(task)
        });
        if said || tokio::time::Instant::now() >= deadline {
            break said;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    drop(logged);
    let written = store
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: matches!(of, EventOf::AHostedMob).then(|| mob.clone()),
            limit: 500,
            ..Default::default()
        })
        .await
        .expect("the mob's frames")
        .frames
        .into_iter()
        .filter(|frame| frame.identity == "alice")
        .count();
    runtime.shutdown().await;

    assert!(
        warned,
        "{of:?}, {by:?}: a warning says the event was not written and names `{runtime_key}` \
         and `{task}`; the log: {}",
        log.contents()
    );
    assert_eq!(
        written, 0,
        "{of:?}, {by:?}: no frame of the member's event is written"
    );
    assert!(
        replay_over,
        "{of:?}, {by:?}: the registration's replay was over before the live event was made"
    );
    let captured = log.contents();
    assert!(
        !captured.contains(EVENT_TEXT),
        "{of:?}, {by:?}: no line of the log holds the event's text; the log: {captured}"
    );
}

#[tokio::test]
async fn a_hosted_mobs_replayed_event_that_could_not_be_written_is_said_in_a_warning() {
    an_event_that_could_not_be_written_is_said_in_a_warning(
        EventOf::AHostedMob,
        ProjectedBy::TheReplay,
    )
    .await;
}

#[tokio::test]
async fn a_hosted_mobs_live_event_that_could_not_be_written_is_said_in_a_warning() {
    an_event_that_could_not_be_written_is_said_in_a_warning(
        EventOf::AHostedMob,
        ProjectedBy::TheLiveProjection,
    )
    .await;
}

#[tokio::test]
async fn the_default_mobs_replayed_event_that_could_not_be_written_is_said_in_a_warning() {
    an_event_that_could_not_be_written_is_said_in_a_warning(
        EventOf::TheDefaultMob,
        ProjectedBy::TheReplay,
    )
    .await;
}

#[tokio::test]
async fn the_default_mobs_live_event_that_could_not_be_written_is_said_in_a_warning() {
    an_event_that_could_not_be_written_is_said_in_a_warning(
        EventOf::TheDefaultMob,
        ProjectedBy::TheLiveProjection,
    )
    .await;
}

/// A console event that a registration's recovery after a lag could not
/// write is said in a warning too, and the warning names that task. The
/// registration reads an event store of its own whose live channel holds
/// 4096 events. While its live projection waits for an event, more events
/// than that are made without a yield, so the live projection cannot take
/// one before they are all made: its next read answers that it lagged.
/// Every event names a send's interaction and the log fails that read.
///
/// What the recovery then meets differs by registration. A hosted mob's
/// registration has written no event, so it writes the mob's gap frame,
/// replays what the store holds and ends at its first event, whose send
/// cannot be read. A runtime registered for the default mob wrote the
/// marker its event store was born with, and that marker is its stored
/// position: after a lag it is older than everything the store still holds
/// (the store keeps as many events as the live channel), so the recovery
/// replays nothing and writes a gap marker in the log; the row's log fails
/// that write. The row tells the two apart: it reads whether a gap
/// marker's write failed and how many gap frames the registration's mob
/// holds.
///
/// The row reads that the LAG path was taken: the warning it waits for
/// names the task `lag recovery`, which only the two lag call sites pass.
/// (After its recovery the live projection takes the events its channel
/// still holds and warns of each under its own task's name.) No line of
/// the log, of either task, holds an event's text.
async fn a_lagged_event_that_could_not_be_written_is_said_in_a_warning(of: EventOf) {
    const WARNING: &str = "a console event was not written to the console log";
    const EVENT_TEXT: &str = "names a send";
    const LAG_RECOVERY: &str = "task=\"lag recovery\"";
    // The live channel of an event store made for no member holds 4096.
    const MORE_THAN_THE_CHANNEL_HOLDS: usize = 4096 + 64;
    let (inner, _dir) = log_of_a_host(Log::Memory);
    let gap_writes_to_fail = match of {
        EventOf::AHostedMob => 0,
        EventOf::TheDefaultMob => usize::MAX,
    };
    let failing = Arc::new(FailsTheFirstLists {
        inner,
        fail_lists: std::sync::atomic::AtomicUsize::new(0),
        fail_moves: std::sync::atomic::AtomicUsize::new(0),
        fail_send_reads: std::sync::atomic::AtomicUsize::new(usize::MAX),
        fail_gap_writes: std::sync::atomic::AtomicUsize::new(gap_writes_to_fail),
    });
    let store: Arc<dyn ConsoleLogStore> = failing.clone();
    let mob = meerkat_mob::MobId::from(super::tests::unique_console_mob_id(
        "send-rows-lagged-event",
    ));
    let runtime = super::tests::build_empty_runtime(mob.as_str()).await;
    let aggregator = console_over(Arc::clone(&store));
    let events = ConsoleEventStore::with_member_capacity(0);

    let log = CapturedLog::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .finish();
    // A second registered Dispatch keeps callsite interest from being
    // cached `never` by threads without a subscriber.
    let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
    let logged = tracing::subscriber::set_default(subscriber);

    let runtime_key = match of {
        EventOf::AHostedMob => {
            aggregator
                .register_mob_console_events(
                    &mob,
                    runtime.mob_runtime().clone(),
                    events.clone(),
                    Arc::new(AllowAllConsoleVisibilityPolicy),
                )
                .expect("the mob's console events register");
            format!("mob/{}", mob.as_str())
        }
        EventOf::TheDefaultMob => {
            aggregator.register_runtime_handles_with_policy(
                "runtime-a",
                "",
                runtime.mob_runtime().clone(),
                None,
                events.clone(),
                Arc::new(AllowAllConsoleVisibilityPolicy),
            );
            "runtime-a".to_string()
        }
    };
    // The registration's replay of its event store is over (a runtime
    // registered for the default mob has recorded the store's marker as its
    // position): the console says that no pass is owed.
    let replay_over = tokio::time::timeout(
        std::time::Duration::from_mins(1),
        aggregator.history_backfill_converged(),
    )
    .await
    .is_ok();
    tokio::task::unconstrained(async {
        for _ in 0..MORE_THAN_THE_CHANNEL_HOLDS {
            events
                .append(
                    "alice",
                    Some("interaction-of-a-send".to_string()),
                    "identity_respawned",
                    json!({ "text": EVENT_TEXT }),
                )
                .await;
        }
    })
    .await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let warned = loop {
        let said = log.contents().lines().any(|line| {
            line.contains("WARN")
                && line.contains(WARNING)
                && line.contains(&runtime_key)
                && line.contains(LAG_RECOVERY)
        });
        if said || tokio::time::Instant::now() >= deadline {
            break said;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    drop(logged);
    let watermark = store
        .source_watermark(&runtime_key, ConsoleFrameSourceKind::ConsoleEvent)
        .await
        .map_err(|error| error.to_string());
    let frames: Vec<String> = store
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: matches!(of, EventOf::AHostedMob).then(|| mob.clone()),
            limit: 500,
            ..Default::default()
        })
        .await
        .expect("the mob's frames")
        .frames
        .into_iter()
        .map(|frame| format!("{}:{}", frame.identity, frame.kind))
        .collect();
    let gap_writes_failed = gap_writes_to_fail.saturating_sub(
        failing
            .fail_gap_writes
            .load(std::sync::atomic::Ordering::SeqCst),
    );
    let gap_frames = frames
        .iter()
        .filter(|frame| {
            frame.as_str()
                == format!(
                    "{}:replay_unavailable",
                    crate::console_contracts::CONSOLE_GAP_IDENTITY
                )
        })
        .count();
    runtime.shutdown().await;

    let log = log.contents();
    let not_live: Vec<&str> = log
        .lines()
        .filter(|line| !line.contains("task=\"live projection\""))
        .take(5)
        .collect();
    assert!(
        replay_over,
        "{of:?}: the registration's replay was over before the events were made"
    );
    assert!(
        warned,
        "{of:?}: a warning says the recovery after a lag could not write an event and names \
         `{runtime_key}` and `{LAG_RECOVERY}`; the log has {} line(s); those that are not the \
         live projection's: {not_live:?}; the stored event watermark: {watermark:?}; the \
         frames of the registration's mob: {frames:?}",
        log.lines().count()
    );
    let with_the_text: Vec<&str> = log
        .lines()
        .filter(|line| line.contains(EVENT_TEXT))
        .take(3)
        .collect();
    assert!(
        with_the_text.is_empty(),
        "{of:?}: no line of the log holds an event's text; the first such lines: \
         {with_the_text:?}"
    );
    assert_eq!(
        (gap_writes_failed > 0, gap_frames),
        match of {
            EventOf::AHostedMob => (false, 1),
            EventOf::TheDefaultMob => (true, 0),
        },
        "{of:?}: a hosted mob's registration writes its gap frame and meets the failure at \
         the first event it replays; a runtime registered for the default mob meets it at \
         its gap marker's write, which was attempted and holds no frame; the frames of the \
         registration's mob: {frames:?}"
    );
}

/// A hosted mob's registration that wrote no event tells the mob's reader of
/// the loss when its live projection lagged. The registration reads an
/// event store of its own, whose live channel holds 4096 events and which
/// keeps as many. Its replay at the registration is over and wrote nothing
/// (the store's own marker is no event of the mob). Then 64 more events
/// than the channel holds are made without a yield, so the live projection
/// takes none before they are all made and its next read answers that it
/// lagged: the store no longer holds the first 64. The recovery has no
/// position to replay from: it writes the mob's gap frame first, then
/// replays what the store still holds. The two oldest frames of the mob
/// are that gap frame and the frame of the 65th event.
///
/// Nothing is waited for by an interval: the replay's end is the console's
/// own word that no pass is owed, and the recovery's end is the
/// registration's gate holding the last event's id as the last it wrote.
#[tokio::test]
async fn a_hosted_mobs_registration_that_wrote_no_event_says_the_gap_after_a_lag() {
    // The live channel of an event store made for no member holds 4096
    // events, and the store keeps as many.
    const KEPT: usize = 4096;
    const LOST: usize = 64;
    let (store, _dir) = log_of_a_host(Log::Memory);
    let mob = meerkat_mob::MobId::from(super::tests::unique_console_mob_id(
        "send-rows-gap-after-a-lag",
    ));
    let runtime = super::tests::build_empty_runtime(mob.as_str()).await;
    let aggregator = console_over(Arc::clone(&store));
    let events = ConsoleEventStore::with_member_capacity(0);
    let registration = aggregator
        .register_mob_console_events(
            &mob,
            runtime.mob_runtime().clone(),
            events.clone(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        )
        .expect("the mob's console events register");
    let writers = super::mob_writers_of(&aggregator.inner);
    let key = super::mob_runtime_key(&mob);
    let a_minute = std::time::Duration::from_mins(1);

    let replayed = tokio::time::timeout(a_minute, aggregator.history_backfill_converged())
        .await
        .is_ok();
    let wrote_before = writers.cursor(&key, registration);
    tokio::task::unconstrained(async {
        for index in 0..KEPT + LOST {
            events
                .append(
                    "alice",
                    None,
                    "identity_respawned",
                    json!({ "text": format!("event-{index}") }),
                )
                .await;
        }
    })
    .await;
    let last = events.latest_event_id().await;
    let deadline = tokio::time::Instant::now() + a_minute;
    let wrote_the_last = loop {
        let wrote = writers.cursor(&key, registration) == last;
        if wrote || tokio::time::Instant::now() >= deadline {
            break wrote;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    let oldest: Vec<(String, String, Option<String>)> = store
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: Some(mob.clone()),
            limit: 2,
            ..Default::default()
        })
        .await
        .expect("the mob's frames")
        .frames
        .into_iter()
        .map(|frame| {
            let said = ["text", "source_kind"].into_iter().find_map(|field| {
                frame
                    .payload
                    .get(field)
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            });
            (frame.identity, frame.kind, said)
        })
        .collect();
    runtime.shutdown().await;

    assert_eq!(
        (replayed, wrote_before, wrote_the_last),
        (true, None, true),
        "the registration's replay is over and wrote no event; after the lag the \
         registration wrote the last event the store holds"
    );
    assert_eq!(
        oldest
            .iter()
            .map(|(identity, kind, said)| (identity.as_str(), kind.as_str(), said.as_deref()))
            .collect::<Vec<_>>(),
        [
            (
                crate::console_contracts::CONSOLE_GAP_IDENTITY,
                "replay_unavailable",
                Some("console_event"),
            ),
            ("alice", "identity_respawned", Some("event-64")),
        ],
        "the mob's oldest frames: its gap frame, written before the replay, then the oldest \
         event the store still holds (64 events were lost)"
    );
}

#[tokio::test]
async fn a_hosted_mobs_lagged_event_that_could_not_be_written_is_said_in_a_warning() {
    a_lagged_event_that_could_not_be_written_is_said_in_a_warning(EventOf::AHostedMob).await;
}

#[tokio::test]
async fn the_default_mobs_lagged_event_that_could_not_be_written_is_said_in_a_warning() {
    a_lagged_event_that_could_not_be_written_is_said_in_a_warning(EventOf::TheDefaultMob).await;
}
