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

/// The bodies among `bodies` that the operator instruction entry rows of
/// `request` carry, in the order the rows list them, repeats included.
fn entry_bodies(
    request: &meerkat_client::LlmRequest,
    bodies: &[&'static str],
) -> Vec<&'static str> {
    let mut listed = Vec::new();
    for message in &request.messages {
        let Message::System(system) = message else {
            continue;
        };
        if !system.content.starts_with(ENTRY_HEADER) {
            continue;
        }
        let mut hits: Vec<(usize, &'static str)> = bodies
            .iter()
            .flat_map(|body| {
                system
                    .content
                    .match_indices(*body)
                    .map(move |(at, _)| (at, *body))
            })
            .collect();
        hits.sort_unstable();
        listed.extend(hits.into_iter().map(|(_, body)| body));
    }
    listed
}

/// Every message of `earlier` is still in `later`, unchanged and at the same
/// position: whatever `later` holds beyond them was appended.
fn assert_appended(
    earlier: &meerkat_client::LlmRequest,
    later: &meerkat_client::LlmRequest,
    what: &str,
) {
    assert!(
        later.messages.len() > earlier.messages.len(),
        "{what}: the later request holds {} rows, the earlier one {}",
        later.messages.len(),
        earlier.messages.len()
    );
    for (index, message) in earlier.messages.iter().enumerate() {
        assert_eq!(
            later.messages.get(index),
            Some(message),
            "{what}: row {index} of the earlier request was rewritten or moved"
        );
    }
}

impl Fixture {
    /// Fork the source as a host does (`fork_member`): the child is seated
    /// and runs nothing until the host starts a turn on it.
    async fn host_driven_fork(&self, child: &str) -> AgentIdentity {
        let child = AgentIdentity::from(child);
        tokio::time::timeout(
            WAIT,
            self.handle.fork_member(
                &self.identity,
                SpawnMemberSpec::new(ProfileName::from("lead"), child.clone()),
                None,
            ),
        )
        .await
        .expect("the fork answers")
        .expect("the fork commits and seats the child");
        child
    }
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
        self.self_starting_fork_of(&self.identity, child).await
    }

    /// [`Self::self_starting_fork`] of the member `source`.
    async fn self_starting_fork_of(
        &self,
        source: &AgentIdentity,
        child: &str,
    ) -> meerkat_client::LlmRequest {
        let before = self.client.requests().len();
        let outcome = tokio::time::timeout(
            WAIT,
            self.handle.fork_member_then_run_bounded(
                source,
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
    assert_appended(
        &first,
        &second,
        "the revision reached the child as an appended entry",
    );
    assert_appended(
        &second,
        &third,
        "the revoke reached the child as an appended entry",
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

/// A member with two records for it and its forks, in two slots: its fork's
/// first request has both bodies in effect, each once, in the order the
/// source's own entry lists them (slot order).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_of_a_member_with_two_records_has_both_once_in_slot_order() {
    let fixture = Fixture::restored_member().await;
    fixture
        .set(write(
            "house-style",
            1,
            HOUSE_STYLE,
            member_and_forks(&fixture.identity),
        ))
        .await;
    fixture
        .set(write(
            "call-sign",
            1,
            CALL_SIGN,
            member_and_forks(&fixture.identity),
        ))
        .await;
    fixture.instructions_current(&fixture.identity).await;
    let source = fixture
        .turn_request(&fixture.identity, "source turn under both records")
        .await;
    let listed = entry_bodies(&source, &[CALL_SIGN, HOUSE_STYLE]);
    assert_eq!(
        listed,
        vec![CALL_SIGN, HOUSE_STYLE],
        "the source's own entry lists both bodies once, in slot order"
    );
    let request = fixture.self_starting_fork("fork-child").await;
    assert_eq!(
        entry_bodies(&request, &[CALL_SIGN, HOUSE_STYLE]),
        listed,
        "the child's first request has both bodies in effect, each once, in the source's order"
    );
    fixture.finish().await;
}

/// Guard: a mob with no record anywhere. The fork's first request holds no
/// operator instruction entry row and no inherited header.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_of_a_member_with_no_record_has_none() {
    let fixture = Fixture::restored_member().await;
    fixture.run_turn("source turn under no record").await;
    let request = fixture.self_starting_fork("fork-child").await;
    let instruction_rows: Vec<&str> = request
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::System(system)
                if system.content.starts_with(ENTRY_HEADER)
                    || system.content.starts_with(INHERITED_ENTRY_HEADER) =>
            {
                Some(system.content.as_str())
            }
            _ => None,
        })
        .collect();
    assert!(
        instruction_rows.is_empty(),
        "the child's first request holds no operator instruction row: {instruction_rows:?}"
    );
    fixture.finish().await;
}

/// Guard: a record written for the member alone after its fork is seated
/// never reaches the fork. The fork's next turns carry the body nowhere,
/// and nothing in its conversation is rewritten.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_written_after_the_fork_for_the_member_alone_never_reaches_the_fork() {
    let fixture = Fixture::restored_member().await;
    fixture.run_turn("source turn before the fork").await;
    let child = fixture.host_driven_fork("fork-child").await;
    let first = fixture.turn_request(&child, "the child's first task").await;

    fixture
        .set(write(
            "call-sign",
            1,
            CALL_SIGN,
            member_only(&fixture.identity),
        ))
        .await;
    fixture.instructions_current(&fixture.identity).await;
    let source = fixture
        .turn_request(&fixture.identity, "source turn under its own record")
        .await;
    assert_eq!(
        entries_with(&source, CALL_SIGN),
        1,
        "the record is in effect in the member it was written for"
    );

    let second = fixture
        .turn_request(&child, "the child's second task")
        .await;
    let third = fixture.turn_request(&child, "the child's third task").await;
    for (request, turn) in [(&second, "second"), (&third, "third")] {
        assert!(
            !carries(request, CALL_SIGN),
            "the fork's {turn} turn carries the body nowhere"
        );
    }
    assert_appended(&first, &second, "the fork's second turn");
    assert_appended(&second, &third, "the fork's third turn");
    fixture.finish().await;
}

/// A record written for the member and its forks after the fork is seated
/// reaches the seated fork: its next run has the entry once, appended after
/// everything its earlier request held, and the run after that does not
/// repeat it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_written_after_the_fork_for_the_member_and_its_forks_reaches_the_seated_fork_once()
{
    let fixture = Fixture::restored_member().await;
    fixture.run_turn("source turn before the fork").await;
    let child = fixture.host_driven_fork("fork-child").await;
    let first = fixture.turn_request(&child, "the child's first task").await;
    assert!(
        !carries(&first, CALL_SIGN),
        "no record exists yet: the child's first turn carries the body nowhere"
    );

    fixture
        .set(write(
            "call-sign",
            1,
            CALL_SIGN,
            member_and_forks(&fixture.identity),
        ))
        .await;
    fixture.instructions_current(&child).await;
    let second = fixture
        .turn_request(&child, "the child's second task")
        .await;
    assert_eq!(
        entries_with(&second, CALL_SIGN),
        1,
        "the fork's next run has the record's body in effect, once"
    );
    assert_appended(
        &first,
        &second,
        "the record reached the fork as an appended entry",
    );
    let (_, entries) = header_and_entry_positions(&second, CALL_SIGN);
    assert!(
        entries[0] >= first.messages.len(),
        "the entry follows every row of the fork's earlier request: entry at {}, earlier rows {}",
        entries[0],
        first.messages.len()
    );

    let third = fixture.turn_request(&child, "the child's third task").await;
    assert_eq!(
        entries_with(&third, CALL_SIGN),
        1,
        "the run after that does not repeat the entry"
    );
    assert_appended(&second, &third, "the fork's third turn");
    fixture.finish().await;
}

/// A host-driven fork is seated and the mob's host restarts before the
/// child's first run. The resumed mob still knows what the child was forked
/// from: the child's first turn has its source's record once, and a higher
/// revision written after the restart reaches the child at its next run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forks_inheritance_survives_a_restart_before_its_first_run() {
    let fixture =
        Fixture::source_with_record(member_and_forks(&AgentIdentity::from("restored-member")))
            .await;
    let child = fixture.host_driven_fork("fork-child").await;

    let fixture = fixture.crash_and_resume().await;
    let first = fixture.turn_request(&child, "the child's first task").await;
    assert_eq!(
        entries_with(&first, CALL_SIGN),
        1,
        "after the restart the child's first request has the record's body in effect, once"
    );

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
        "a revision written after the restart reaches the child: its lineage was restored"
    );
    assert_appended(
        &first,
        &second,
        "the revision reached the child as an appended entry",
    );
    fixture.finish().await;
}

impl Fixture {
    /// The slot keys the mob's status read lists in `identity`'s row.
    async fn listed_slots(&self, identity: &AgentIdentity) -> Vec<String> {
        self.handle
            .operator_instruction_status(OperatorInstructionStatusQuery::default())
            .await
            .expect("the status read answers")
            .members
            .into_iter()
            .find(|member| &member.agent_identity == identity)
            .unwrap_or_else(|| panic!("{identity} has a status row"))
            .slots
            .into_iter()
            .map(|slot| slot.slot.key.to_string())
            .collect()
    }
}

/// A fork's seat records what the fork descends from in the step that seats
/// it, before the mob can prepare a delivery for the seat. So the first
/// snapshot the child's session records, the one it holds when the mob
/// reads the child as current, is at the mob's generation and names its
/// source's record with the body, before the child has run anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forks_first_recorded_snapshot_holds_its_sources_record() {
    let fixture =
        Fixture::source_with_record(member_and_forks(&AgentIdentity::from("restored-member")))
            .await;
    let child = fixture.host_driven_fork("fork-child").await;
    fixture.instructions_current(&child).await;

    let page = fixture
        .handle
        .operator_instruction_status(OperatorInstructionStatusQuery::default())
        .await
        .expect("the status read answers");
    let row = page
        .members
        .iter()
        .find(|member| member.agent_identity == child)
        .expect("the child has a status row");
    assert_eq!(
        row.delivery.recorded_generation,
        Some(page.generation),
        "the mob reads the child as current at its generation"
    );

    let session = fixture
        .member_session(&child)
        .await
        .expect("the child has a session");
    let durable = fixture
        .durable(&session)
        .await
        .expect("the child's durable session");
    let target = durable
        .instruction_target_state()
        .expect("the child's recorded target reads back typed")
        .expect("the child's session recorded a snapshot");
    assert_eq!(
        target.target_generation(),
        page.generation,
        "the snapshot the child's session holds is the one the mob reads as current"
    );
    let slot = target
        .slot(&InstructionSlot {
            namespace: InstructionNamespace::new("app.example").expect("namespace"),
            key: InstructionKey::new("call-sign").expect("key"),
        })
        .expect("the child's first recorded snapshot names its source's record");
    assert_eq!(slot.effect, meerkat_core::InstructionEffect::Active);
    assert_eq!(
        slot.pending_body.as_deref(),
        Some(CALL_SIGN),
        "with its body, pending until the child's first run applies it"
    );
    assert_eq!(
        fixture.listed_slots(&child).await,
        vec!["call-sign".to_string()],
        "the child's status row lists its source's record"
    );
    fixture.finish().await;
}

/// A record for a member and its forks follows the member's fork family
/// from its root. Three members: the first, a fork of it, and a fork of
/// that fork. A record written for the fork between and its forks selects
/// that fork only: the fork below it never carries the body, and its status
/// row does not list the record. A record written for the first member and
/// its forks reaches all three. The write's receipt counts exactly the
/// seated members the record selects, and each member's status row lists
/// the records of its snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_for_a_fork_and_its_forks_selects_that_fork_only() {
    let fixture = Fixture::restored_member().await;
    fixture.run_turn("the first member's turn").await;
    let first = fixture.identity.clone();
    let between = fixture.host_driven_fork("fork-between").await;
    fixture
        .turn_request(&between, "the fork between runs a turn")
        .await;
    let below = AgentIdentity::from("fork-below");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &between,
            SpawnMemberSpec::new(ProfileName::from("lead"), below.clone()),
            None,
        ),
    )
    .await
    .expect("the second fork answers")
    .expect("the second fork commits and seats the fork below");

    // For the fork between and its forks: that fork only.
    let receipt = fixture
        .handle
        .set_operator_instruction(write(
            "house-style",
            1,
            HOUSE_STYLE,
            member_and_forks(&between),
        ))
        .await
        .expect("the record for the fork between commits");
    assert_eq!(
        receipt.matched_members, 1,
        "a record for a fork and its forks selects that fork only"
    );
    fixture.instructions_current(&between).await;
    let request = fixture
        .turn_request(&between, "the fork between under its record")
        .await;
    assert_eq!(
        entries_with(&request, HOUSE_STYLE),
        1,
        "the fork the record names has it in effect"
    );
    let request = fixture
        .turn_request(&below, "the fork below runs a turn")
        .await;
    assert!(
        !carries(&request, HOUSE_STYLE),
        "the fork below the named fork carries the body nowhere"
    );
    assert_eq!(
        fixture.listed_slots(&between).await,
        vec!["house-style".to_string()]
    );
    for other in [&first, &below] {
        assert!(
            fixture.listed_slots(other).await.is_empty(),
            "{other}'s status row does not list the record for the fork between"
        );
    }

    // For the first member and its forks: all three.
    let receipt = fixture
        .handle
        .set_operator_instruction(write("call-sign", 1, CALL_SIGN, member_and_forks(&first)))
        .await
        .expect("the record for the first member commits");
    assert_eq!(
        receipt.matched_members, 3,
        "a record for the first member and its forks selects it and every fork below it"
    );
    for member in [&first, &between, &below] {
        fixture.instructions_current(member).await;
        let request = fixture
            .turn_request(member, "a turn under the family's record")
            .await;
        assert_eq!(
            entries_with(&request, CALL_SIGN),
            1,
            "{member} has the first member's record in effect, once"
        );
    }
    assert_eq!(
        fixture.listed_slots(&first).await,
        vec!["call-sign".to_string()]
    );
    assert_eq!(
        fixture.listed_slots(&between).await,
        vec!["call-sign".to_string(), "house-style".to_string()]
    );
    assert_eq!(
        fixture.listed_slots(&below).await,
        vec!["call-sign".to_string()],
        "the fork below lists its root's record, not the record of the fork between"
    );
    let request = fixture
        .turn_request(&below, "the fork below runs another turn")
        .await;
    assert!(
        !carries(&request, HOUSE_STYLE),
        "the fork below still carries the other record's body nowhere"
    );
    fixture.finish().await;
}

/// A fork of a fork is recorded with the first member of its family by
/// every fork route, never with the fork it was taken from: a fork that
/// starts its own turn and a helper fork, each taken from a fork of the
/// first member, run their first turn under the first member's record and
/// not under the record written for the fork between and its forks, and
/// the mob's status read lists the first member's record in their rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_of_a_fork_is_recorded_with_the_first_member_by_every_route() {
    let fixture =
        Fixture::source_with_record(member_and_forks(&AgentIdentity::from("restored-member")))
            .await;
    let between = fixture.host_driven_fork("fork-between").await;
    fixture
        .turn_request(&between, "the fork between runs a turn")
        .await;
    let receipt = fixture
        .handle
        .set_operator_instruction(write(
            "house-style",
            1,
            HOUSE_STYLE,
            member_and_forks(&between),
        ))
        .await
        .expect("the record for the fork between commits");
    assert_eq!(receipt.matched_members, 1);
    fixture.instructions_current(&between).await;
    fixture
        .turn_request(&between, "the fork between under its own record")
        .await;

    // A fork that starts its own turn, taken from the fork between.
    let request = fixture
        .self_starting_fork_of(&between, "self-starting-below")
        .await;
    assert_eq!(
        entries_with(&request, CALL_SIGN),
        1,
        "the self-starting fork of a fork has the first member's record in effect, once"
    );
    assert!(
        !carries(&request, HOUSE_STYLE),
        "and carries the record of the fork between nowhere"
    );

    // A helper fork, taken from the fork between.
    let before = fixture.client.requests().len();
    fixture
        .handle
        .spawn_spec(
            context_fork_spec(
                "helper-below",
                &between,
                crate::launch::ForkContext::FullHistory,
            )
            .with_initial_message("the forked task")
            .with_runtime_mode(crate::MobRuntimeMode::TurnDriven),
        )
        .await
        .expect("the context fork of the fork between spawns");
    let request = fixture.first_request_after(before).await;
    assert_eq!(
        entries_with(&request, CALL_SIGN),
        1,
        "the helper fork of a fork has the first member's record in effect, once"
    );
    assert!(
        !carries(&request, HOUSE_STYLE),
        "and carries the record of the fork between nowhere"
    );

    for below in ["self-starting-below", "helper-below"] {
        assert_eq!(
            fixture.listed_slots(&AgentIdentity::from(below)).await,
            vec!["call-sign".to_string()],
            "{below}'s status row lists the first member's record and no other"
        );
    }
    fixture.finish().await;
}

impl Fixture {
    /// [`Self::restored_member`] in a mob that keeps its journal in a SQLite
    /// database: its rows are stored encoded, and a resume reads them back.
    async fn restored_member_on_a_sqlite_journal() -> Self {
        let (fixture, session_id) =
            Self::unseated_on_journal(ServiceDouble::Real, None, None, true).await;
        fixture
            .handle
            .spawn_spec(
                SpawnMemberSpec::new(ProfileName::from("lead"), fixture.identity.clone())
                    .with_launch_mode(crate::launch::MemberLaunchMode::Resume {
                        bridge_session_id: session_id,
                        resume_from_role: None,
                    }),
            )
            .await
            .expect("restore the member onto its durable session");
        fixture
    }

    /// The targets of the mob's records, as its status read lists them.
    async fn desired_targets(&self) -> Vec<OperatorInstructionTarget> {
        self.handle
            .operator_instruction_status(OperatorInstructionStatusQuery::default())
            .await
            .expect("the status read answers")
            .desired
            .into_iter()
            .map(|record| record.target)
            .collect()
    }
}

/// A fork's family and a record written for a member and its forks are
/// stored facts: a mob that keeps its journal in SQLite is stopped and
/// resumed from the stored rows, and the record still carries its scope,
/// the fork's status row still lists it, the fork's first run has it once,
/// and a revision written after the resume selects and reaches the fork.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forks_family_and_a_scoped_record_survive_a_resume_from_a_sqlite_journal() {
    let fixture = Fixture::restored_member_on_a_sqlite_journal().await;
    let family = member_and_forks(&fixture.identity);
    fixture
        .set(write("call-sign", 1, CALL_SIGN, family.clone()))
        .await;
    fixture.instructions_current(&fixture.identity).await;
    fixture.run_turn("source turn under the record").await;
    let child = fixture.host_driven_fork("fork-child").await;

    let fixture = fixture.crash_and_resume().await;
    assert_eq!(
        fixture.desired_targets().await,
        vec![family.clone()],
        "the stored record reads back as a record for the member and its forks"
    );
    assert_eq!(
        fixture.listed_slots(&child).await,
        vec!["call-sign".to_string()],
        "the resumed mob still counts the child in its source's fork family"
    );
    let first = fixture.turn_request(&child, "the child's first task").await;
    assert_eq!(
        entries_with(&first, CALL_SIGN),
        1,
        "after the resume the child's first request has the record's body in effect, once"
    );

    let receipt = fixture
        .handle
        .set_operator_instruction(write("call-sign", 2, CALL_SIGN_REVISED, family))
        .await
        .expect("the revision commits");
    assert_eq!(
        receipt.matched_members, 2,
        "the revision selects the member and its fork"
    );
    fixture.instructions_current(&child).await;
    let second = fixture
        .turn_request(&child, "the child's second task")
        .await;
    assert_eq!(
        entries_with(&second, CALL_SIGN_REVISED),
        1,
        "a revision written after the resume reaches the child"
    );
    assert_appended(
        &first,
        &second,
        "the revision reached the child as an appended entry",
    );
    fixture.finish().await;
}

/// A fork's family follows its identity through a respawn: the respawned
/// fork runs on a new session, its first run there has its source's record
/// once, its status row lists the record, and after a restart of the mob's
/// host a revision still selects and reaches it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_respawned_fork_keeps_its_family() {
    let fixture =
        Fixture::source_with_record(member_and_forks(&AgentIdentity::from("restored-member")))
            .await;
    let child = fixture.host_driven_fork("fork-child").await;
    fixture.turn_request(&child, "the child's first task").await;
    let before = fixture
        .member_session(&child)
        .await
        .expect("the child has a session");

    tokio::time::timeout(WAIT, fixture.handle.respawn(child.clone(), None))
        .await
        .expect("the respawn answers")
        .expect("the fork is respawned");
    let after = fixture
        .member_session(&child)
        .await
        .expect("the respawned child has a session");
    assert_ne!(before, after, "the respawned fork runs on a new session");
    assert_eq!(
        fixture.listed_slots(&child).await,
        vec!["call-sign".to_string()],
        "the respawned fork's status row still lists its source's record"
    );
    let request = fixture
        .turn_request(&child, "the respawned child's first task")
        .await;
    assert_eq!(
        entries_with(&request, CALL_SIGN),
        1,
        "the respawned fork's first run has its source's record in effect, once"
    );

    let fixture = fixture.crash_and_resume().await;
    let receipt = fixture
        .handle
        .set_operator_instruction(write(
            "call-sign",
            2,
            CALL_SIGN_REVISED,
            member_and_forks(&fixture.identity),
        ))
        .await
        .expect("the revision commits");
    assert_eq!(
        receipt.matched_members, 2,
        "after the restart the revision selects the member and its respawned fork"
    );
    fixture.instructions_current(&child).await;
    let request = fixture
        .turn_request(&child, "the respawned child's next task")
        .await;
    assert_eq!(
        entries_with(&request, CALL_SIGN_REVISED),
        1,
        "the revision reaches the respawned fork"
    );
    fixture.finish().await;
}

/// A durable fork's child takes its operator instruction snapshot before it
/// is seated, and a child that cannot take it is not seated. Over a session
/// service that refuses a record for a session with no live actor, a fork
/// taken while the mob holds no record seats its child (nothing is
/// written), and a fork taken after the mob's first record fails with the
/// provision failure that names the snapshot, and seats nobody.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_whose_child_cannot_take_its_snapshot_fails_and_seats_nobody() {
    let (fixture, session_id) = Fixture::unseated(ServiceDouble::RecordsNeedALiveActor, None).await;
    fixture
        .handle
        .spawn_spec(
            SpawnMemberSpec::new(ProfileName::from("lead"), fixture.identity.clone())
                .with_launch_mode(crate::launch::MemberLaunchMode::Resume {
                    bridge_session_id: session_id,
                    resume_from_role: None,
                }),
        )
        .await
        .expect("restore the member onto its durable session");
    fixture.run_turn("source turn before any record").await;

    let fork = |child: &str| {
        let handle = fixture.handle.clone();
        let source = fixture.identity.clone();
        let child = AgentIdentity::from(child);
        async move {
            tokio::time::timeout(
                WAIT,
                handle.fork_member(
                    &source,
                    SpawnMemberSpec::new(ProfileName::from("lead"), child),
                    None,
                ),
            )
            .await
            .expect("the fork answers")
            .map(|_| ())
        }
    };
    fork("fork-before")
        .await
        .expect("a mob with no record writes nothing: the child is seated");

    fixture
        .set(write(
            "call-sign",
            1,
            CALL_SIGN,
            member_and_forks(&fixture.identity),
        ))
        .await;
    let refused = fork("fork-after").await;
    let after = AgentIdentity::from("fork-after");
    match refused {
        Err(MobError::ForkMemberProvisionFailed {
            member_id, reason, ..
        }) => {
            assert_eq!(member_id, after);
            assert!(
                reason.contains("operator instruction snapshot could not be recorded"),
                "the failure names the snapshot: {reason}"
            );
        }
        other => panic!("the fork fails with its provision failure: {other:?}"),
    }
    assert!(
        fixture
            .handle
            .get_member(&after)
            .await
            .expect("roster read")
            .is_none(),
        "the child that could not take its snapshot is not seated"
    );
    fixture.finish().await;
}
