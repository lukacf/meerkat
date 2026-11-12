//! Operator instructions at a seat over a conversation the member already
//! has.
//!
//! A record written while an agent has a conversation but no seated member
//! is owed to the member at its seat. The member is seated dormant, on its
//! conversation, and its first run is the one that wakes it: the mob's
//! record is in the conversation before that run's first model request.

use super::*;
use crate::operator_instruction::{
    MemberInstructionDeliveryState, OperatorInstructionStatusQuery, OperatorInstructionTarget,
    OperatorInstructionWrite,
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

    let outcomes = fixture
        .handle
        .seat_dormant_members(vec![
            SpawnMemberSpec::new(ProfileName::from("lead"), fixture.identity.clone())
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

    // The mob's delivery owner works on the owed member from the seat on.
    // Its attempts fail and back off (200 ms, doubling), so after this time
    // the next one is more than three seconds away: the wake and the run
    // below are long over before it.
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
