//! Operator instructions at a seat over a conversation the member already
//! has.
//!
//! A record written while an agent has a conversation but no seated member
//! is owed to the member at its seat. The member is seated dormant, on its
//! conversation, and its first run is the one that wakes it: the mob's
//! record is in the conversation before that run's first model request.

use super::*;
use crate::operator_instruction::{
    MemberInstructionBlockReason, MemberInstructionDeliveryState, OperatorInstructionStatusQuery,
    OperatorInstructionTarget, OperatorInstructionWrite,
};
use crate::runtime::DormantSeatOutcome;
use meerkat_core::{InstructionRevision, InstructionSlot};

const ENTRY_HEADER: &str = "[meerkat-operator-instructions-v1]";
const STANDING: &str = "Open every answer with the word STANDING.";

/// One write of slot `key` in the rows' namespace.
pub(super) fn write(
    key: &str,
    revision: u64,
    body: &str,
    target: OperatorInstructionTarget,
) -> OperatorInstructionWrite {
    OperatorInstructionWrite {
        slot: InstructionSlot {
            namespace: InstructionNamespace::new("app.example").expect("namespace"),
            key: InstructionKey::new(key).expect("key"),
        },
        revision: InstructionRevision::new(revision).expect("revision"),
        body: body.to_string(),
        content_sha256: None,
        target,
    }
}

/// The operator instruction entry rows of `request` that carry `body`.
pub(super) fn entries_with(request: &meerkat_client::LlmRequest, body: &str) -> usize {
    request
        .messages
        .iter()
        .filter(|message| {
            matches!(
                message,
                Message::System(system)
                    if system.content.starts_with(ENTRY_HEADER) && system.content.contains(body)
            )
        })
        .count()
}

impl Fixture {
    /// The member's delivery as the mob's status read shows it.
    async fn delivery_of(
        &self,
        identity: &AgentIdentity,
    ) -> Option<MemberInstructionDeliveryState> {
        self.handle
            .operator_instruction_status(OperatorInstructionStatusQuery::default())
            .await
            .expect("the status read answers")
            .members
            .into_iter()
            .find(|member| &member.agent_identity == identity)
            .map(|member| member.delivery.state)
    }

    /// Write the standing record for the member, not seated yet, and seat
    /// the member dormant on the conversation it already has.
    async fn seat_dormant_under_the_record(&self, session_id: &SessionId) {
        self.seat_dormant_under_the_record_of(session_id, "standing")
            .await;
    }

    /// [`Self::seat_dormant_under_the_record`] with the record in slot `key`.
    async fn seat_dormant_under_the_record_of(&self, session_id: &SessionId, key: &str) {
        self.handle
            .set_operator_instruction(write(
                key,
                1,
                STANDING,
                OperatorInstructionTarget::Member {
                    agent_identity: self.identity.clone(),
                },
            ))
            .await
            .expect("the record commits while the agent is not seated");
        let outcomes = self
            .handle
            .seat_dormant_members(vec![
                SpawnMemberSpec::new(ProfileName::from("lead"), self.identity.clone())
                    .with_launch_mode(crate::launch::MemberLaunchMode::Resume {
                        bridge_session_id: session_id.clone(),
                        resume_from_role: None,
                    }),
            ])
            .await
            .expect("the dormant seat answers");
        assert!(
            matches!(outcomes.first(), Some(DormantSeatOutcome::Seated { .. })),
            "the member is seated dormant on its conversation: {outcomes:?}"
        );
    }

    /// Wait until the member's delivery, as the status read shows it, is one
    /// `reached` accepts, at most `within`.
    async fn wait_for_delivery(
        &self,
        what: &str,
        within: Duration,
        reached: impl Fn(&MemberInstructionDeliveryState) -> bool,
    ) -> MemberInstructionDeliveryState {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let delivery = self.delivery_of(&self.identity).await;
            if let Some(state) = delivery.as_ref().filter(|state| reached(state)) {
                return state.clone();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{what}: the member's delivery is {delivery:?} after {within:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Run one turn on the member and answer the first request it sent the
    /// model.
    async fn first_request_of_a_turn(&self, text: &str) -> meerkat_client::LlmRequest {
        let before = self.client.requests().len();
        let turn = self.start_turn_for(&self.identity, text).await;
        tokio::time::timeout(WAIT, turn.wait())
            .await
            .expect("the turn finishes")
            .expect("the turn succeeds");
        self.client
            .requests()
            .get(before)
            .cloned()
            .expect("the turn reached the model")
    }

    /// Start one turn on the member from a task of its own: the row goes on
    /// while the turn's wake is under way.
    fn spawn_turn(&self, text: &'static str) -> tokio::task::JoinHandle<WorkTurnHandle> {
        let handle = self.handle.clone();
        let identity = self.identity.clone();
        tokio::spawn(async move {
            let entry = handle
                .get_member(&identity)
                .await
                .expect("roster read")
                .expect("member binding");
            handle
                .start_work_with_mode(
                    entry.agent_runtime_id,
                    entry.fence_token,
                    WorkRef::new(),
                    WorkSpec::new(text, WorkOrigin::Internal),
                    HandlingMode::Queue,
                )
                .await
                .expect("turn admitted")
        })
    }

    /// Another command of the mob, answered while a member's wake is under
    /// way: a record for a name the mob does not seat.
    async fn the_mob_answers_another_command(&self, while_what: &str) {
        let answered = tokio::time::timeout(
            Duration::from_secs(3),
            self.handle.set_operator_instruction(write(
                "other",
                1,
                "Another record.",
                OperatorInstructionTarget::Member {
                    agent_identity: AgentIdentity::from("nobody"),
                },
            )),
        )
        .await;
        assert!(
            answered.is_ok(),
            "the mob answers another command {while_what}; it did not within 3 s"
        );
        answered
            .expect("answered")
            .expect("the other record commits");
    }
}

/// The record is written while the agent is not seated; the member is then
/// seated dormant on the conversation it already has. The mob's store-only
/// record cannot reach that conversation until the member's wake has loaded
/// it (`session not found`, as a service whose host keeps the conversation
/// archived until then answers), so the member is still owed when its first
/// run wakes it, and the mob's next attempt is seconds away. That run's
/// first request carries the record's entry all the same: the wake records
/// what the member is owed before the member is Live.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_seated_on_its_conversation_runs_its_first_turn_under_the_record() {
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::RecordsNeedALiveActor, None).await;
    fixture.seat_dormant_under_the_record(&session_id).await;

    // The mob's delivery owner works on the owed member from the seat on.
    // Its attempts fail and back off (200 ms, doubling: they start about
    // 0, 0.2, 0.6, 1.4, 3.0 and 6.2 s after the seat), so after this time
    // the next one is about 2.2 s away: the wake and the run below are over
    // before it.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let before_wake = fixture.delivery_of(&fixture.identity).await;
    assert!(
        matches!(
            &before_wake,
            Some(MemberInstructionDeliveryState::Owed {
                last_failure: Some(failure),
                ..
            }) if failure.contains("session not found")
        ),
        "before its wake the member is owed and the store-only record cannot reach its \
         conversation: {before_wake:?}"
    );

    let before = fixture.client.requests().len();
    let turn = fixture
        .start_turn_for(&fixture.identity, "the first task after the seat")
        .await;
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the first turn finishes")
        .expect("the first turn succeeds");
    let request = fixture
        .client
        .requests()
        .get(before)
        .cloned()
        .expect("the first turn reached the model");
    assert_eq!(
        entries_with(&request, STANDING),
        1,
        "the first run after the seat has the record in effect; the member's delivery before \
         its wake was {before_wake:?}"
    );
    fixture.finish().await;
}

/// The mob's own delivery to the seated member is in the member's session,
/// which does not answer yet, when a turn wakes the member. The wake waits
/// for that delivery off the mob's actor: the mob answers another command
/// meanwhile. Once the session answers, the member's first run carries the
/// record's entry once, whichever of the two wrote it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_behind_a_delivery_in_flight_leaves_the_mob_answering() {
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::RecordsNeedALiveActor, None).await;
    fixture.records.hold_all();
    fixture.seat_dormant_under_the_record(&session_id).await;
    // The delivery owner's record for the seated member is in the session
    // now, under the member's delivery lock.
    fixture.records.wait_held(1).await;

    let before = fixture.client.requests().len();
    let turn = fixture.spawn_turn("the first task after the seat");
    // The wake has loaded the conversation and reached its record by now.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    fixture
        .the_mob_answers_another_command(
            "while a member's wake waits behind a delivery in flight for the member",
        )
        .await;

    fixture.records.open();
    let turn = tokio::time::timeout(WAIT, turn)
        .await
        .expect("the turn is admitted")
        .expect("the turn's task");
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the first turn finishes")
        .expect("the first turn succeeds");
    let request = fixture
        .client
        .requests()
        .get(before)
        .cloned()
        .expect("the first turn reached the model");
    assert_eq!(
        entries_with(&request, STANDING),
        1,
        "the first run carries the record's entry once"
    );
    fixture.finish().await;
}

/// The mob's own delivery to the member is in the member's session and is
/// never answered, so it holds the member's delivery lock for good. The
/// wake waits for it at most its own short bound: the member goes Live and
/// runs its turn, without the entry, and its status says why it is still
/// owed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivery_in_flight_past_the_bound_does_not_hold_the_wake() {
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::RecordsNeedALiveActor, None).await;
    fixture.records.hold_all();
    fixture.seat_dormant_under_the_record(&session_id).await;
    fixture.records.wait_held(1).await;

    let before = fixture.client.requests().len();
    let turn = fixture.spawn_turn("the first task after the seat");
    let turn = tokio::time::timeout(Duration::from_secs(25), turn)
        .await
        .expect("a delivery in flight does not hold the member's wake past its bound")
        .expect("the turn's task");
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the first turn finishes")
        .expect("the first turn succeeds");
    let request = fixture
        .client
        .requests()
        .get(before)
        .cloned()
        .expect("the first turn reached the model");
    assert_eq!(entries_with(&request, STANDING), 0);
    let after_wake = fixture.delivery_of(&fixture.identity).await;
    assert!(
        matches!(
            &after_wake,
            Some(MemberInstructionDeliveryState::Owed {
                last_failure: Some(failure),
                ..
            }) if failure.contains("still in flight")
        ),
        "the member is Live and owed, and its status names the delivery in flight: \
         {after_wake:?}"
    );
    fixture.records.open();
    fixture.finish().await;
}

/// The member's session takes the record at the wake and does not answer
/// (it waits for its turn boundary or its store). The wake does not wait
/// for it past its own short bound: the member goes Live and runs its turn,
/// without the entry, and stays owed. When the session answers, the mob's
/// own retry records the snapshot and the next run carries the entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_the_session_holds_past_the_bound_leaves_the_member_live_and_owed() {
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::RecordsNeedALiveActor, None).await;
    fixture.seat_dormant_under_the_record(&session_id).await;
    fixture.records.set_live(LiveRecord::Hold);

    let before = fixture.client.requests().len();
    let turn = fixture
        .start_turn_for(&fixture.identity, "the first task after the seat")
        .await;
    tokio::time::timeout(Duration::from_secs(25), turn.wait())
        .await
        .expect("a record the session does not answer does not hold the member's wake")
        .expect("the first turn succeeds");
    let request = fixture
        .client
        .requests()
        .get(before)
        .cloned()
        .expect("the first turn reached the model");
    assert_eq!(
        entries_with(&request, STANDING),
        0,
        "the record was not written before the run: the run has no entry"
    );
    let after_wake = fixture.delivery_of(&fixture.identity).await;
    assert!(
        matches!(
            &after_wake,
            Some(MemberInstructionDeliveryState::Owed { .. })
        ),
        "the member is Live and still owed: {after_wake:?}"
    );

    fixture.records.open();
    fixture
        .wait_for_delivery(
            "the mob's own retry records the snapshot once the session answers",
            Duration::from_secs(40),
            |state| matches!(state, MemberInstructionDeliveryState::Current),
        )
        .await;
    let request = fixture.first_request_of_a_turn("the next task").await;
    assert_eq!(
        entries_with(&request, STANDING),
        1,
        "the first run after the record landed carries the entry"
    );
    fixture.finish().await;
}

/// The member's store cannot take the record at the wake: the session
/// answers a store error, as a durable service does during an outage of its
/// store. The member goes Live and stays owed, with the failure in its
/// status: it is not blocked. The store comes back while the member is
/// dormant again, and its next wake writes the record: that run carries the
/// entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_the_store_cannot_take_stays_owed_and_the_next_wake_writes_it() {
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::RecordsNeedALiveActor, None).await;
    fixture.seat_dormant_under_the_record(&session_id).await;
    fixture.records.set_live(LiveRecord::StoreUnavailable);

    let request = fixture
        .first_request_of_a_turn("the first task after the seat")
        .await;
    assert_eq!(entries_with(&request, STANDING), 0);
    let after_wake = fixture.delivery_of(&fixture.identity).await;
    assert!(
        matches!(
            &after_wake,
            Some(MemberInstructionDeliveryState::Owed {
                last_failure: Some(failure),
                ..
            }) if failure.contains("the session store is unavailable")
        ),
        "a store error leaves the member owed with the failure, not blocked: {after_wake:?}"
    );

    // Dormant again: the mob's own retry cannot reach the conversation, so
    // the member is still owed when its next turn wakes it.
    tokio::time::timeout(WAIT, async {
        while fixture
            .handle
            .release_member(&fixture.identity)
            .await
            .is_err()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the idle member is released");
    fixture.records.set_live(LiveRecord::Forward);
    let request = fixture.first_request_of_a_turn("the next task").await;
    assert_eq!(
        entries_with(&request, STANDING),
        1,
        "the member's next wake wrote the record it was still owed"
    );
    fixture.finish().await;
}

/// The mob runs over a session service that keeps no durable session
/// document: every record is answered `durability_unavailable`, for good.
/// The member is blocked with that reason from its seat on, and the mob
/// does not try the record again: not on its own retry, and not at the
/// member's wake.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_service_that_keeps_no_durable_sessions_blocks_the_member_for_good() {
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::NeverDurable, None).await;
    fixture.seat_dormant_under_the_record(&session_id).await;
    fixture
        .wait_for_delivery(
            "a service that keeps no durable member sessions blocks the member",
            Duration::from_secs(10),
            |state| {
                matches!(
                    state,
                    MemberInstructionDeliveryState::Blocked {
                        reason: MemberInstructionBlockReason::DurabilityUnavailable
                    }
                )
            },
        )
        .await;
    let recorded = fixture.records.calls();

    let request = fixture
        .first_request_of_a_turn("the first task after the seat")
        .await;
    assert_eq!(entries_with(&request, STANDING), 0);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        fixture.records.calls(),
        recorded,
        "the blocked member's record is not tried again, at its wake or after"
    );
    let after_wake = fixture.delivery_of(&fixture.identity).await;
    assert_eq!(
        after_wake,
        Some(MemberInstructionDeliveryState::Blocked {
            reason: MemberInstructionBlockReason::DurabilityUnavailable
        }),
        "the member is still blocked after its wake"
    );
    fixture.finish().await;
}

/// Another mob owns the member's session target (`owned_by_other_mob`).
/// The member goes Live, its delivery is blocked with that reason, and the
/// mob does not try the record again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_another_owner_refuses_at_the_wake_blocks_the_member() {
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::RecordsNeedALiveActor, None).await;
    fixture.seat_dormant_under_the_record(&session_id).await;
    fixture.records.set_live(LiveRecord::OwnedByOtherMob);

    let request = fixture
        .first_request_of_a_turn("the first task after the seat")
        .await;
    assert_eq!(entries_with(&request, STANDING), 0);
    fixture
        .wait_for_delivery(
            "a refusal by the target's owner blocks the member",
            Duration::from_secs(10),
            |state| {
                matches!(
                    state,
                    MemberInstructionDeliveryState::Blocked {
                        reason: MemberInstructionBlockReason::OwnedByOtherMob
                    }
                )
            },
        )
        .await;
    let recorded = fixture.records.calls();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        fixture.records.calls(),
        recorded,
        "a blocked member's record is not tried again"
    );
    fixture.finish().await;
}

/// A member that is owed nothing wakes without a journal read: its own
/// record was delivered while it was dormant, so its wake has no body to
/// read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_of_a_member_that_is_owed_nothing_reads_no_journal_row() {
    use crate::runtime::operator_instruction_delivery::journaled_body_row_reads;
    // A slot key only this row writes: the probe counts per slot key, so
    // another row's journal read in the same process is not counted here.
    const KEY: &str = "standing-unread-at-wake";
    let slot_key = write(KEY, 1, STANDING, OperatorInstructionTarget::Mob)
        .slot
        .machine_key();
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::Real, None).await;
    fixture
        .seat_dormant_under_the_record_of(&session_id, KEY)
        .await;
    fixture
        .wait_for_delivery(
            "the dormant member's store-only record lands",
            WAIT,
            |state| matches!(state, MemberInstructionDeliveryState::Current),
        )
        .await;

    let reads = journaled_body_row_reads(&slot_key);
    let request = fixture
        .first_request_of_a_turn("the first task after the seat")
        .await;
    assert_eq!(entries_with(&request, STANDING), 1);
    assert_eq!(
        journaled_body_row_reads(&slot_key),
        reads,
        "the wake of a member that is owed nothing reads no row of the mob's journal"
    );
    fixture.finish().await;
}

/// The wake's record is in the member's session, which has not answered,
/// and the member's turn is admitted: no run starts while the record is in
/// flight. Once the session answers, the run starts with the entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_run_starts_while_the_wake_record_is_in_flight() {
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::RecordsNeedALiveActor, None).await;
    fixture.seat_dormant_under_the_record(&session_id).await;
    fixture.records.set_live(LiveRecord::Hold);

    let before = fixture.client.requests().len();
    let turn = fixture.spawn_turn("the first task after the seat");
    fixture.records.wait_held(1).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        fixture.client.requests().len(),
        before,
        "no run of the member starts while its wake's record is in flight"
    );

    fixture.records.open();
    let turn = tokio::time::timeout(WAIT, turn)
        .await
        .expect("the turn is admitted")
        .expect("the turn's task");
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the first turn finishes")
        .expect("the first turn succeeds");
    let request = fixture
        .client
        .requests()
        .get(before)
        .cloned()
        .expect("the first turn reached the model");
    assert_eq!(entries_with(&request, STANDING), 1);
    fixture.finish().await;
}

/// The member is retired while its wake's record is in the session, which
/// has not answered. The mob goes on answering, the retirement lands once
/// the session answers, and the late answer settles nothing: the mob no
/// longer lists the member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_closed_while_its_record_is_in_flight_settles_nothing() {
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::RecordsNeedALiveActor, None).await;
    fixture.seat_dormant_under_the_record(&session_id).await;
    fixture.records.set_live(LiveRecord::Hold);

    let turn = fixture.spawn_turn("the first task after the seat");
    fixture.records.wait_held(1).await;
    let retiring = {
        let handle = fixture.handle.clone();
        let identity = fixture.identity.clone();
        tokio::spawn(async move { handle.retire(identity).await })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    fixture
        .the_mob_answers_another_command("while a retiring member's wake record is in flight")
        .await;

    fixture.records.open();
    let retired = tokio::time::timeout(WAIT, retiring)
        .await
        .expect("the retirement answers")
        .expect("the retirement's task");
    // The retirement lands, or answers that it is still in progress once
    // its caller's budget is spent: the mob finishes it on its own.
    assert!(
        retired
            .as_ref()
            .map_or_else(MobError::is_retirement_in_progress, |()| true),
        "the retirement lands or goes on: {retired:?}"
    );
    let _ = tokio::time::timeout(WAIT, turn).await;
    tokio::time::timeout(WAIT, async {
        while fixture.delivery_of(&fixture.identity).await.is_some() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the retired member has no delivery left");
    fixture.finish().await;
}

/// A member whose record failed is owed and dormant again, and the mob's
/// retry has backed off. The mob is stopped and lifted again, which
/// materializes the dormant member itself, outside a delivery's wake: the
/// mob tries the record at once when the member is Live again, not when its
/// back-off is due.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lift_of_the_mob_tries_an_owed_members_record_at_once() {
    let (fixture, session_id) = Fixture::unseated_with(
        ServiceDouble::RecordsNeedALiveActor,
        None,
        Some(crate::runtime::MemberRestorePolicy::Eager),
    )
    .await;
    let seated = tokio::time::Instant::now();
    fixture.seat_dormant_under_the_record(&session_id).await;
    fixture.records.set_live(LiveRecord::SessionGone);
    let request = fixture
        .first_request_of_a_turn("the first task after the seat")
        .await;
    assert_eq!(entries_with(&request, STANDING), 0);
    tokio::time::timeout(WAIT, async {
        while fixture
            .handle
            .release_member(&fixture.identity)
            .await
            .is_err()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the idle member is released");

    // The mob's retry fails and backs off (200 ms, doubling): its attempts
    // start about 0, 0.2, 0.6, 1.4, 3.0, 6.2 and 12.6 s after it was armed
    // (at the seat, or about half a second later), so 14 s after the seat
    // the next one is more than 11 s away.
    tokio::time::sleep_until(seated + Duration::from_secs(14)).await;
    let backed_off = fixture.delivery_of(&fixture.identity).await;
    assert!(
        matches!(
            &backed_off,
            Some(MemberInstructionDeliveryState::Owed { .. })
        ),
        "the member is owed and its retry has backed off: {backed_off:?}"
    );

    fixture.handle.stop().await.expect("the mob stops");
    fixture.records.set_live(LiveRecord::Forward);
    fixture.handle.resume().await.expect("the mob is lifted");
    fixture
        .wait_for_delivery(
            "the lift materialized the member and the mob tried its record at once",
            Duration::from_secs(6),
            |state| matches!(state, MemberInstructionDeliveryState::Current),
        )
        .await;
    fixture.finish().await;
}

/// A member seated eagerly on a conversation it already has (a spawn with
/// a Resume launch) is recorded by the mob's own delivery once its spawn
/// has activated, not inside the spawn: the record is not promised to its
/// first run. A run that starts after the delivery landed carries the
/// entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_eager_seat_on_a_conversation_is_recorded_by_the_mobs_delivery() {
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::Real, None).await;
    fixture
        .handle
        .set_operator_instruction(write(
            "standing",
            1,
            STANDING,
            OperatorInstructionTarget::Member {
                agent_identity: fixture.identity.clone(),
            },
        ))
        .await
        .expect("the record commits while the agent is not seated");
    fixture
        .handle
        .spawn_spec(
            SpawnMemberSpec::new(ProfileName::from("lead"), fixture.identity.clone())
                .with_launch_mode(crate::launch::MemberLaunchMode::Resume {
                    bridge_session_id: session_id.clone(),
                    resume_from_role: None,
                }),
        )
        .await
        .expect("the member is seated on its conversation");
    fixture
        .wait_for_delivery(
            "the mob's delivery records the eagerly seated member",
            WAIT,
            |state| matches!(state, MemberInstructionDeliveryState::Current),
        )
        .await;
    let request = fixture.first_request_of_a_turn("a task").await;
    assert_eq!(
        entries_with(&request, STANDING),
        1,
        "a run that starts after the delivery landed carries the entry"
    );
    fixture.finish().await;
}

/// A release of the member that lands between the preparation of its wake's
/// record and the write moves the member's release epoch on: the write does
/// not reach the session, and nothing is fed back. With the epoch that
/// stands, the write reaches the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_between_the_preparation_and_the_write_writes_nothing() {
    use crate::operator_instruction::MemberInstructionDelivery;
    use crate::runtime::operator_instruction_delivery::{
        OperatorInstructionDeliveryShared, WAKE_INSTRUCTION_RECORD_BOUND, WakeInstructionRecord,
        member_release_epoch, record_owed_snapshot_at_wake,
    };
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::RecordsNeedALiveActor, None).await;
    let shared = OperatorInstructionDeliveryShared::new(MobId::from("a-mob"), 1);
    let prepared_at =
        member_release_epoch(&shared, &fixture.identity, WAKE_INSTRUCTION_RECORD_BOUND)
            .await
            .expect("no delivery holds the member's lock");
    // The release: under the member's lock, its epoch moves on.
    {
        let lock = shared.member_lock(&fixture.identity);
        let mut epoch = lock.lock().await;
        *epoch = epoch.wrapping_add(1);
    }
    let released_at =
        member_release_epoch(&shared, &fixture.identity, WAKE_INSTRUCTION_RECORD_BOUND)
            .await
            .expect("no delivery holds the member's lock");
    let delivery = || {
        MemberInstructionDelivery::authorized(
            session_id.clone(),
            meerkat_core::InstructionTargetRecord {
                mob_id: "a-mob".to_string(),
                agent_identity: fixture.identity.to_string(),
                generation: 1,
                slots: Vec::new(),
            },
        )
    };

    let record = record_owed_snapshot_at_wake(
        Arc::clone(&shared),
        Arc::clone(&fixture.mob_service),
        fixture.identity.clone(),
        delivery(),
        prepared_at,
        WAKE_INSTRUCTION_RECORD_BOUND,
    )
    .await;
    assert!(
        matches!(record, WakeInstructionRecord::NotWritten),
        "a record prepared before the member's release writes nothing"
    );
    assert_eq!(
        fixture.records.calls(),
        0,
        "the session saw no record after the release"
    );

    let record = record_owed_snapshot_at_wake(
        shared,
        Arc::clone(&fixture.mob_service),
        fixture.identity.clone(),
        delivery(),
        released_at,
        WAKE_INSTRUCTION_RECORD_BOUND,
    )
    .await;
    assert_eq!(
        fixture.records.calls(),
        1,
        "with the epoch that stands the record reaches the session"
    );
    assert!(
        matches!(record, WakeInstructionRecord::Failed),
        "the session has no live actor: the record fails and the member stays owed"
    );
    fixture.finish().await;
}
