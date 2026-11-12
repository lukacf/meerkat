//! Operator instructions and forks.
//!
//! A member-targeted operator instruction record written for a member and
//! its forks (`include_forks`) follows every fork of that member: the child
//! is born with it, so its first run already has the body, and it follows
//! later revisions and the revoke like any member the record selects. A
//! record without the scope stays its member's alone. Every fork child is
//! also born with the records the mob targets at every member and at its
//! profile, so its first run has them too.
//!
//! These rows drive each fork route of the mob over a real factory and a
//! real persistent session service, and read what the child's model is
//! sent.

use super::operator_instruction_reseat::{entries_with, write};
use super::*;
use crate::operator_instruction::{
    MemberInstructionDeliveryState, OperatorInstructionStatusQuery, OperatorInstructionTarget,
    OperatorInstructionWrite,
};
use meerkat_core::{InstructionRevision, InstructionSlot};

const ENTRY_HEADER: &str = "[meerkat-operator-instructions-v1]";
const INHERITED_ENTRY_HEADER: &str = "[meerkat-inherited-operator-instructions-v1]";
const CALL_SIGN: &str = "Sign every answer with the call sign KESTREL.";
const CALL_SIGN_REVISED: &str = "Sign every answer with the call sign HERON.";
const HOUSE_STYLE: &str = "Answer in the house style: short sentences.";

/// A member target whose record also selects the member's forks, from its
/// wire form.
fn member_and_forks(identity: &AgentIdentity) -> OperatorInstructionTarget {
    serde_json::from_value(serde_json::json!({
        "kind": "member",
        "agent_identity": identity.as_str(),
        "include_forks": true,
    }))
    .expect("a member target with the fork scope")
}

/// A member target without the fork scope.
fn member_only(identity: &AgentIdentity) -> OperatorInstructionTarget {
    serde_json::from_value(serde_json::json!({
        "kind": "member",
        "agent_identity": identity.as_str(),
    }))
    .expect("a member target")
}

/// Whether any row of `request` carries `body`, in any form.
fn carries(request: &meerkat_client::LlmRequest, body: &str) -> bool {
    format!("{:?}", request.messages).contains(body)
}

/// The positions of the inherited entry headers and of the entry rows that
/// carry `body` among `request`'s messages.
fn header_and_entry_positions(
    request: &meerkat_client::LlmRequest,
    body: &str,
) -> (Vec<usize>, Vec<usize>) {
    let mut headers = Vec::new();
    let mut entries = Vec::new();
    for (index, message) in request.messages.iter().enumerate() {
        let Message::System(system) = message else {
            continue;
        };
        if system.content.starts_with(INHERITED_ENTRY_HEADER) {
            headers.push(index);
        } else if system.content.starts_with(ENTRY_HEADER) && system.content.contains(body) {
            entries.push(index);
        }
    }
    (headers, entries)
}

impl Fixture {
    /// Wait until `identity`'s session recorded the mob's current snapshot.
    async fn instructions_current(&self, identity: &AgentIdentity) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let page = self
                .handle
                .operator_instruction_status(OperatorInstructionStatusQuery::default())
                .await
                .expect("the status read answers");
            let current = page.members.iter().any(|member| {
                &member.agent_identity == identity
                    && member.delivery.state == MemberInstructionDeliveryState::Current
                    && member.delivery.recorded_generation.is_some()
            });
            if current {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{identity} never recorded the mob's snapshot"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn set(&self, write: OperatorInstructionWrite) {
        self.handle
            .set_operator_instruction(write)
            .await
            .expect("the record commits");
    }

    /// The source member with one record of its own under `target`, applied
    /// by a completed turn: its transcript holds its own entry row with the
    /// body, which a durable fork copies.
    async fn source_with_record(target: OperatorInstructionTarget) -> Self {
        let fixture = Self::restored_member().await;
        fixture.set(write("call-sign", 1, CALL_SIGN, target)).await;
        fixture.instructions_current(&fixture.identity).await;
        let before = fixture.client.requests().len();
        fixture.run_turn("source turn under the record").await;
        let request = fixture
            .client
            .requests()
            .get(before)
            .cloned()
            .expect("the source's turn reached the model");
        assert_eq!(
            entries_with(&request, CALL_SIGN),
            1,
            "the source's own run has its record in effect"
        );
        fixture
    }

    /// Run one turn on `identity` and return the first request it sent.
    async fn turn_request(
        &self,
        identity: &AgentIdentity,
        text: &str,
    ) -> meerkat_client::LlmRequest {
        let before = self.client.requests().len();
        let turn = self.start_turn_for(identity, text).await;
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

    /// Fork the source as `fork_off` does (the child's first turn is started
    /// by the fork itself) and return the child's first request.
    async fn self_starting_fork(&self, child: &str) -> meerkat_client::LlmRequest {
        let before = self.client.requests().len();
        let outcome = tokio::time::timeout(
            WAIT,
            self.handle.fork_member_then_run_bounded(
                &self.identity,
                SpawnMemberSpec::new(ProfileName::from("lead"), AgentIdentity::from(child))
                    .with_initial_message("the forked task"),
                None,
                "result",
                4_096,
                meerkat_core::DurableForkSourceAdmission::Quiescent,
            ),
        )
        .await
        .expect("the fork answers");
        if let Err(error) = outcome {
            panic!("the fork child runs its first turn: {error:?}");
        }
        self.client
            .requests()
            .get(before)
            .cloned()
            .expect("the child's first turn reached the model")
    }
}

/// `fork_off`'s composition starts the child's only turn itself. The child
/// of a member with a record for it and its forks sends its first request
/// with that body in effect, as its own entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_self_starting_fork_runs_its_first_turn_under_its_sources_record() {
    let fixture =
        Fixture::source_with_record(member_and_forks(&AgentIdentity::from("restored-member")))
            .await;
    let request = fixture.self_starting_fork("fork-child").await;
    assert_eq!(
        entries_with(&request, CALL_SIGN),
        1,
        "the child's first request has the record's body in effect, once"
    );
    fixture.finish().await;
}

/// The same child's first request holds the entry row it copied from its
/// source as a header without the body, then its own entry with the body,
/// in that order: the copied row is history, and the child's own later
/// entry for the slot is what applies.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_childs_first_request_holds_the_inherited_header_then_its_own_entry() {
    let fixture =
        Fixture::source_with_record(member_and_forks(&AgentIdentity::from("restored-member")))
            .await;
    let request = fixture.self_starting_fork("fork-child").await;
    let (headers, entries) = header_and_entry_positions(&request, CALL_SIGN);
    assert_eq!(
        headers.len(),
        1,
        "the copied entry row reads as one inherited header"
    );
    let header = match &request.messages[headers[0]] {
        Message::System(system) => system.content.clone(),
        other => panic!("the header is a System row: {other:?}"),
    };
    assert!(
        !header.contains(CALL_SIGN) && header.ends_with("status=not_active_in_child"),
        "the inherited header carries no body: {header}"
    );
    assert_eq!(entries.len(), 1, "the child's own entry carries the body");
    assert!(
        headers[0] < entries[0],
        "the header precedes the child's own entry: header at {}, entry at {}",
        headers[0],
        entries[0]
    );
    fixture.finish().await;
}

/// A host-driven fork (`fork_member`, the route a side conversation takes)
/// seats a child the host starts later. Its first turn has the record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_driven_fork_runs_its_first_turn_under_its_sources_record() {
    let fixture =
        Fixture::source_with_record(member_and_forks(&AgentIdentity::from("restored-member")))
            .await;
    let child = AgentIdentity::from("fork-child");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), child.clone()),
            None,
        ),
    )
    .await
    .expect("the fork answers")
    .expect("the fork commits and seats the child");
    let request = fixture.turn_request(&child, "the child's task").await;
    assert_eq!(
        entries_with(&request, CALL_SIGN),
        1,
        "the child's first request has the record's body in effect, once"
    );
    fixture.finish().await;
}

/// A detached fork (`fork_member_then_run_detached`) admits the child's
/// turn before it returns. That turn has the record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_detached_fork_runs_its_first_turn_under_its_sources_record() {
    let fixture =
        Fixture::source_with_record(member_and_forks(&AgentIdentity::from("restored-member")))
            .await;
    let before = fixture.client.requests().len();
    let (_fork, run) = tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member_then_run_detached(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), AgentIdentity::from("fork-child"))
                .with_initial_message("the forked task"),
            None,
            "result",
            4_096,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            None,
        ),
    )
    .await
    .expect("the fork answers")
    .unwrap_or_else(|error| panic!("the detached fork is admitted: {error:?}"));
    let request = fixture.first_request_after(before).await;
    assert_eq!(
        entries_with(&request, CALL_SIGN),
        1,
        "the child's first request has the record's body in effect, once"
    );
    drop(run);
    fixture.finish().await;
}

/// A live delegation's durable fork admits the child's bounded turn itself.
/// That turn has the record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegated_fork_runs_its_first_turn_under_its_sources_record() {
    let fixture =
        Fixture::source_with_record(member_and_forks(&AgentIdentity::from("restored-member")))
            .await;
    let before = fixture.client.requests().len();
    let request = crate::runtime::delegation::DelegationExecutionRequest::new(
        AgentIdentity::from("delegated-child"),
        "the delegated task",
        BoundedResultSpec::new("delegation-result", 4_096).expect("result spec"),
    )
    .with_durable_fork(fixture.identity.clone(), None);
    let service =
        crate::runtime::delegation::DelegationExecutionService::new(fixture.handle.clone());
    let execution = tokio::time::timeout(WAIT, service.start(request))
        .await
        .expect("the delegation answers")
        .expect("the delegation starts");
    let terminalized = tokio::time::timeout(WAIT, execution.await_terminal())
        .await
        .expect("the delegated turn ends");
    match terminalized.terminal() {
        crate::runtime::delegation::DelegationTurnTerminal::Completed(_) => {}
        terminal => panic!("the delegated child completes its bounded turn: {terminal:?}"),
    }
    let request = fixture
        .client
        .requests()
        .get(before)
        .cloned()
        .expect("the delegated child reached the model");
    assert_eq!(
        entries_with(&request, CALL_SIGN),
        1,
        "the child's first request has the record's body in effect, once"
    );
    fixture.finish().await;
}

/// A helper fork (a spawn with a fork launch mode) mints a fresh session
/// and renders its source's conversation into its first prompt. It is born
/// with the record too, as its own entry, and the rendered context carries
/// no instruction body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_helper_fork_runs_its_first_turn_under_its_sources_record() {
    let fixture =
        Fixture::source_with_record(member_and_forks(&AgentIdentity::from("restored-member")))
            .await;
    let before = fixture.client.requests().len();
    fixture
        .handle
        .spawn_spec(
            context_fork_spec(
                "context-child",
                &fixture.identity,
                crate::launch::ForkContext::FullHistory,
            )
            .with_initial_message("the forked task")
            .with_runtime_mode(crate::MobRuntimeMode::TurnDriven),
        )
        .await
        .expect("the context fork spawns");
    let request = fixture.first_request_after(before).await;
    assert_eq!(
        entries_with(&request, CALL_SIGN),
        1,
        "the helper's first request has the record's body in effect, once"
    );
    let block = fork_block(&request).expect("the first request carries the fork context");
    assert!(
        !block.contains(CALL_SIGN),
        "the rendered context carries no instruction body: {block}"
    );
    fixture.finish().await;
}

/// A record without the fork scope is its member's alone: no row of its
/// fork's first request carries the body, by any fork route.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_without_the_fork_scope_is_not_in_effect_in_a_fork() {
    let fixture =
        Fixture::source_with_record(member_only(&AgentIdentity::from("restored-member"))).await;
    let request = fixture.self_starting_fork("fork-child").await;
    assert!(
        !carries(&request, CALL_SIGN),
        "the durable fork's first request carries the body nowhere"
    );
    let before = fixture.client.requests().len();
    fixture
        .handle
        .spawn_spec(
            context_fork_spec(
                "context-child",
                &fixture.identity,
                crate::launch::ForkContext::FullHistory,
            )
            .with_initial_message("the forked task")
            .with_runtime_mode(crate::MobRuntimeMode::TurnDriven),
        )
        .await
        .expect("the context fork spawns");
    let request = fixture.first_request_after(before).await;
    assert!(
        !carries(&request, CALL_SIGN),
        "the helper fork's first request carries the body nowhere"
    );
    fixture.finish().await;
}

/// A seated fork follows its source's record live: a later revision is in
/// effect at the child's next run, and so is the revoke.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seated_fork_follows_a_later_revision_and_the_revoke() {
    let fixture =
        Fixture::source_with_record(member_and_forks(&AgentIdentity::from("restored-member")))
            .await;
    let child = AgentIdentity::from("fork-child");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), child.clone()),
            None,
        ),
    )
    .await
    .expect("the fork answers")
    .expect("the fork commits and seats the child");
    let first = fixture.turn_request(&child, "the child's first task").await;
    assert_eq!(entries_with(&first, CALL_SIGN), 1);

    fixture
        .set(write(
            "call-sign",
            2,
            CALL_SIGN_REVISED,
            member_and_forks(&fixture.identity),
        ))
        .await;
    fixture.instructions_current(&child).await;
    let second = fixture
        .turn_request(&child, "the child's second task")
        .await;
    assert_eq!(
        entries_with(&second, CALL_SIGN_REVISED),
        1,
        "the child's next run has the new revision in effect"
    );

    fixture
        .handle
        .revoke_operator_instruction(
            InstructionSlot {
                namespace: InstructionNamespace::new("app.example").expect("namespace"),
                key: InstructionKey::new("call-sign").expect("key"),
            },
            InstructionRevision::new(3).expect("revision"),
        )
        .await
        .expect("the revoke commits");
    fixture.instructions_current(&child).await;
    let third = fixture.turn_request(&child, "the child's third task").await;
    let session = fixture
        .member_session(&child)
        .await
        .expect("the child has a session");
    let durable = fixture
        .durable(&session)
        .await
        .expect("the child's durable session");
    let effective = meerkat_core::effective_operator_instructions(durable.id(), durable.messages())
        .expect("the child's entry rows are well formed");
    assert_eq!(
        effective.active_count(),
        0,
        "the child's own entries revoked the slot: {effective:?}"
    );
    assert!(
        third.messages.len() > second.messages.len(),
        "the revoke appended an entry, nothing was rewritten"
    );
    fixture.finish().await;
}

/// A fork of a fork follows the root: the second child's first request has
/// the root's record in effect.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_of_a_fork_follows_the_root_members_record() {
    let fixture =
        Fixture::source_with_record(member_and_forks(&AgentIdentity::from("restored-member")))
            .await;
    let child = AgentIdentity::from("fork-child");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), child.clone()),
            None,
        ),
    )
    .await
    .expect("the fork answers")
    .expect("the fork commits and seats the child");
    fixture.turn_request(&child, "the child's task").await;
    let grandchild = AgentIdentity::from("fork-grandchild");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &child,
            SpawnMemberSpec::new(ProfileName::from("lead"), grandchild.clone()),
            None,
        ),
    )
    .await
    .expect("the second fork answers")
    .expect("the second fork commits and seats the grandchild");
    let request = fixture
        .turn_request(&grandchild, "the grandchild's task")
        .await;
    assert_eq!(
        entries_with(&request, CALL_SIGN),
        1,
        "the grandchild's first request has the root's record in effect, once"
    );
    fixture.finish().await;
}

/// A durable fork child is seated over a session that already exists, so no
/// build carries the mob's snapshot into it. Its only turn, started by the
/// fork itself, still has the records the mob targets at every member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_self_starting_fork_runs_its_first_turn_under_the_shared_records() {
    let fixture = Fixture::restored_member().await;
    fixture
        .set(write(
            "house-style",
            1,
            HOUSE_STYLE,
            OperatorInstructionTarget::Mob,
        ))
        .await;
    fixture.instructions_current(&fixture.identity).await;
    fixture
        .run_turn("source turn under the shared record")
        .await;
    let request = fixture.self_starting_fork("fork-child").await;
    assert_eq!(
        entries_with(&request, HOUSE_STYLE),
        1,
        "the child's first request has the shared record's body in effect, once"
    );
    fixture.finish().await;
}
