//! Who manages a mob a member created with its own `mob_create`, after that
//! member's next incarnations.
//!
//! The member's session keeps its management of the mobs it created while
//! the member that serves the session is a same-session successor of the
//! member that created them: the same identity at a later generation, on the
//! same session, with every step between the two a same-session successor
//! step. Every other later generation manages nothing: a fresh session, a
//! retirement followed by a seat on the kept session, a chain of steps with
//! any other step in it, and every generation after a mob reset of the
//! member's mob. Core seats no other identity on the member's session.
//!
//! Every row asks through the agent mob tools of the member's session, built
//! anew for each question with the authority a rebuilt session holds (it may
//! create mobs and manages none), so each answer is the one the session's
//! next build gets.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_core::types::ToolCallView;
use meerkat_core::{AgentToolDispatcher, SessionId, ToolError};
use meerkat_mob::{
    AgentIdentity, Generation, MemberLaunchMode, MemberRespawn, MobDefinition, MobId,
    MobProvenance, MobRuntimeMode, ProfileBinding, ProfileName, SpawnMemberSpec,
};
use meerkat_mob_mcp::{AgentMobToolSurface, MobMcpState};
use serde_json::json;
use support::{CouncilFixture, ScriptedTurn};

/// The member that creates mobs.
const CREATOR: &str = "creator";
/// The role the creator is seated under.
const ROLE: &str = "participant";
/// The role a migration moves the creator to.
const OTHER_ROLE: &str = "reviewer";
/// The member each created mob holds.
const WORKER: &str = "worker-1";

fn creator() -> AgentIdentity {
    AgentIdentity::from(CREATOR)
}

/// A host whose mob holds the creator, seated turn-driven under [`ROLE`];
/// the mob's definition also holds [`OTHER_ROLE`].
struct Host {
    fixture: CouncilFixture,
    parent: MobId,
}

impl Host {
    async fn new() -> Self {
        let fixture = CouncilFixture::new(|_| ScriptedTurn::Text("ok".to_string()));
        let parent = fixture.source_mob_id();
        let mut profiles = BTreeMap::new();
        for role in [ROLE, OTHER_ROLE] {
            profiles.insert(
                ProfileName::from(role),
                ProfileBinding::Inline(Box::new(support::participant_profile(role))),
            );
        }
        let mut definition = MobDefinition::explicit(parent.clone());
        definition.profiles = profiles;
        fixture.seed_source_mob_from(definition, &[CREATOR]).await;
        Self { fixture, parent }
    }

    fn state(&self) -> &Arc<MobMcpState> {
        &self.fixture.state
    }

    async fn parent(&self) -> meerkat_mob::MobHandle {
        self.state()
            .handle_for(&self.parent)
            .await
            .expect("the creator's mob is hosted")
    }

    /// The session `member` is bound to.
    async fn session_of(&self, member: &AgentIdentity) -> SessionId {
        self.state()
            .mob_resolve_bridge_session_id(&self.parent, member)
            .await
            .expect("resolve the member's session")
            .expect("the member is bound to a session")
    }

    /// The member that serves `session` now, with its generation.
    async fn seat_of(&self, session: &SessionId) -> Option<(AgentIdentity, Generation)> {
        self.parent().await.member_of_session(session).await
    }

    /// A mob id scoped to this host.
    fn mob(&self, label: &str) -> String {
        format!("{label}-{}", self.fixture.scope)
    }

    /// The agent mob tools of `session` as its next build holds them: it may
    /// create mobs and manages none.
    fn tools_of(&self, session: &SessionId) -> AgentMobToolSurface {
        AgentMobToolSurface::new(
            Arc::clone(self.state()),
            None,
            meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
                .expect("generated create-only authority"),
            "claude-sonnet-4-5".to_string(),
            session.clone(),
            None,
            None,
            None,
        )
    }

    /// `session` creates the mob `mob_id` with its own `mob_create` and
    /// seats [`WORKER`] in it.
    async fn create(&self, session: &SessionId, mob_id: &str) {
        let tools = self.tools_of(session);
        let created = call(
            &tools,
            "mob_create",
            json!({ "definition": {
                "id": mob_id,
                "profiles": { "worker": {
                    "model": "claude-sonnet-4-5",
                    "tools": { "comms": true },
                    "peer_description": "worker",
                    "runtime_mode": "turn_driven"
                } }
            } }),
        )
        .await
        .unwrap_or_else(|error| panic!("mob_create {mob_id}: {error:?}"));
        assert_eq!(created["mob_id"], mob_id, "{created}");
        // The create's answer granted the session manage over the mob; the
        // spawn uses that grant as the agent loop commits it.
        let granted = self.tools_of(session);
        call(&granted, "mob_list", json!({}))
            .await
            .expect("mob_list");
        self.state()
            .mob_spawn(
                &MobId::from(mob_id),
                ProfileName::from("worker"),
                AgentIdentity::from(WORKER),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("seat the worker of {mob_id}: {error}"));
    }

    /// The creation record of `mob_id` names the creator at `generation` on
    /// `session`, as the creator of its own create.
    async fn assert_recorded_creator(&self, mob_id: &str, generation: u64, session: &SessionId) {
        let provenance = self
            .state()
            .handle_for(&MobId::from(mob_id))
            .await
            .expect("the created mob is hosted")
            .provenance()
            .await
            .expect("the creation record reads");
        assert_eq!(
            provenance,
            MobProvenance::OperatorMember {
                parent_mob_id: self.parent.clone(),
                parent_incarnation: self.parent().await.incarnation(),
                creator_identity: creator(),
                creator_generation: Generation::new(generation),
                owner_session: session.clone(),
            },
            "the record names the creating member and its session"
        );
    }

    /// `session` lists `mob_id`, reads its members and checks its member.
    async fn assert_manages(&self, session: &SessionId, mob_id: &str, when: &str) {
        let tools = self.tools_of(session);
        assert!(
            listed(&tools).await.iter().any(|listed| listed == mob_id),
            "{when}: the session lists the mob it created"
        );
        let members = call(&tools, "mob_list_members", json!({ "mob_id": mob_id }))
            .await
            .unwrap_or_else(|error| panic!("{when}: mob_list_members: {error:?}"));
        assert!(
            members.to_string().contains(WORKER),
            "{when}: the member list names the worker: {members}"
        );
        call(
            &tools,
            "mob_check_member",
            json!({ "mob_id": mob_id, "member_id": WORKER }),
        )
        .await
        .unwrap_or_else(|error| panic!("{when}: mob_check_member: {error:?}"));
    }

    /// `session` destroys `mob_id` with its own tool, and the mob is gone.
    async fn assert_destroys(&self, session: &SessionId, mob_id: &str, when: &str) {
        call(
            &self.tools_of(session),
            "mob_destroy",
            json!({ "mob_id": mob_id }),
        )
        .await
        .unwrap_or_else(|error| panic!("{when}: mob_destroy: {error:?}"));
        assert!(
            self.state().handle_for(&MobId::from(mob_id)).await.is_err(),
            "{when}: the destroyed mob is no longer hosted"
        );
    }

    /// `session` does not list `mob_id`, each of its calls on the mob is the
    /// ordinary access refusal, and the mob stays hosted.
    async fn assert_manages_nothing(&self, session: &SessionId, mob_id: &str, when: &str) {
        let tools = self.tools_of(session);
        assert!(
            !listed(&tools).await.iter().any(|listed| listed == mob_id),
            "{when}: the session does not list the mob"
        );
        for (tool, args) in [
            ("mob_list_members", json!({ "mob_id": mob_id })),
            (
                "mob_check_member",
                json!({ "mob_id": mob_id, "member_id": WORKER }),
            ),
            (
                "mob_retire_member",
                json!({ "mob_id": mob_id, "member_id": WORKER }),
            ),
            ("mob_destroy", json!({ "mob_id": mob_id })),
        ] {
            let refused = call(&tools, tool, args).await;
            assert!(
                matches!(refused, Err(ToolError::AccessDenied { .. })),
                "{when}: {tool} is refused: {refused:?}"
            );
        }
        assert!(
            self.state().handle_for(&MobId::from(mob_id)).await.is_ok(),
            "{when}: the refused calls left the mob in place"
        );
    }

    /// Seat the creator's same-session successor: its next generation on its
    /// own session.
    async fn successor(&self, request: MemberRespawn) {
        self.parent()
            .await
            .respawn_member(creator(), request)
            .await
            .expect("the same-session successor is seated");
    }

    /// Retire `member`, whose session is archived.
    async fn retire(&self, member: &AgentIdentity) {
        self.state()
            .mob_retire(&self.parent, member.clone())
            .await
            .unwrap_or_else(|error| panic!("retire {member}: {error}"));
    }

    /// Seat `member` on the existing session `session`.
    async fn resume_on(&self, member: &AgentIdentity, session: &SessionId) {
        let mut spec =
            SpawnMemberSpec::new(ROLE, member.clone()).with_launch_mode(MemberLaunchMode::Resume {
                bridge_session_id: session.clone(),
                resume_from_role: None,
            });
        spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
        self.state()
            .mob_spawn_spec(&self.parent, spec)
            .await
            .unwrap_or_else(|error| panic!("seat {member} on the kept session: {error}"));
    }
}

async fn call(
    tools: &AgentMobToolSurface,
    tool: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
    let outcome = tools
        .dispatch(ToolCallView {
            id: "creator-call",
            name: tool,
            args: &raw,
        })
        .await?;
    Ok(serde_json::from_str(&outcome.result.text_content())
        .unwrap_or_else(|_| json!(outcome.result.text_content())))
}

/// The ids of the mobs `tools` lists.
async fn listed(tools: &AgentMobToolSurface) -> Vec<String> {
    let listed = call(tools, "mob_list", json!({})).await.expect("mob_list");
    listed["mobs"]
        .as_array()
        .expect("mobs")
        .iter()
        .map(|mob| mob["mob_id"].as_str().expect("mob_id").to_string())
        .collect()
}

fn generation(value: u64) -> Generation {
    Generation::new(value)
}

/// A member that created mobs with its own tool keeps managing them as its
/// same-session successor: it lists them, reads and checks their members and
/// destroys them, in the same process and after a restart of the host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_same_session_successor_keeps_its_management_of_the_mobs_it_created() {
    let mut host = Host::new().await;
    let session = host.session_of(&creator()).await;
    let (first, second) = (host.mob("created-first"), host.mob("created-second"));
    host.create(&session, &first).await;
    host.create(&session, &second).await;
    host.assert_recorded_creator(&first, 0, &session).await;
    for mob in [&first, &second] {
        host.assert_manages(&session, mob, "before the successor")
            .await;
    }

    host.successor(MemberRespawn::new()).await;
    assert_eq!(
        host.seat_of(&session).await,
        Some((creator(), generation(1))),
        "the successor serves the same session one generation up"
    );
    host.assert_recorded_creator(&first, 0, &session).await;
    for mob in [&first, &second] {
        host.assert_manages(&session, mob, "after the successor, in the same process")
            .await;
    }
    host.assert_destroys(&session, &first, "after the successor, in the same process")
        .await;

    host.fixture
        .restart_cold_with(|_| ScriptedTurn::Text("ok".to_string()), |state, _| state)
        .await;
    assert_eq!(
        host.seat_of(&session).await,
        Some((creator(), generation(1))),
        "the restart keeps the successor's seat"
    );
    host.assert_manages(&session, &second, "after a restart")
        .await;
    host.assert_destroys(&session, &second, "after a restart")
        .await;
    host.fixture.teardown().await;
}

/// Two same-session successors in a row keep the creator's management: the
/// member two generations after the one that created the mob still manages
/// it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_same_session_successors_in_a_row_keep_the_creators_management() {
    let host = Host::new().await;
    let session = host.session_of(&creator()).await;
    let mob = host.mob("created-two-back");
    host.create(&session, &mob).await;

    host.successor(MemberRespawn::new()).await;
    host.successor(MemberRespawn::new()).await;
    assert_eq!(
        host.seat_of(&session).await,
        Some((creator(), generation(2))),
        "two successors, the same session"
    );
    host.assert_recorded_creator(&mob, 0, &session).await;
    host.assert_manages(&session, &mob, "two successors later")
        .await;
    host.assert_destroys(&session, &mob, "two successors later")
        .await;
    host.fixture.teardown().await;
}

/// A same-session successor seated under another role keeps the creator's
/// management of the mobs it created: the session's management follows its
/// successors, and the role's own tools decide which calls it can make.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_role_migration_successor_keeps_its_management_of_the_mob_it_created() {
    let host = Host::new().await;
    let session = host.session_of(&creator()).await;
    let mob = host.mob("created-before-migration");
    host.create(&session, &mob).await;

    host.successor(MemberRespawn::new().with_profile(OTHER_ROLE))
        .await;
    assert_eq!(
        host.seat_of(&session).await,
        Some((creator(), generation(1))),
        "the migration keeps the session"
    );
    host.assert_manages(&session, &mob, "after a role migration")
        .await;
    host.assert_destroys(&session, &mob, "after a role migration")
        .await;
    host.fixture.teardown().await;
}

/// A creator reset onto a fresh session manages nothing it created: neither
/// its earlier session nor its new one lists or reaches the mob.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_creator_reset_onto_a_fresh_session_manages_nothing_it_created() {
    let host = Host::new().await;
    let session = host.session_of(&creator()).await;
    let mob = host.mob("created-before-reset");
    host.create(&session, &mob).await;

    let mut spec = SpawnMemberSpec::new(ROLE, creator());
    spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    let fresh = host
        .parent()
        .await
        .rotate_member(spec)
        .await
        .expect("the reset seats the creator on a fresh session");
    assert_ne!(fresh, session, "a reset takes a fresh session");
    assert_eq!(
        host.seat_of(&fresh).await,
        Some((creator(), generation(1))),
        "the next generation serves the fresh session"
    );
    host.assert_manages_nothing(&session, &mob, "the earlier session after a reset")
        .await;
    host.assert_manages_nothing(&fresh, &mob, "the fresh session after a reset")
        .await;
    host.fixture.teardown().await;
}

/// A creator that was retired and then seated again on its kept session
/// manages nothing it created: the later generation serves the same session,
/// and no same-session successor step seated it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_creator_retired_and_seated_again_on_its_session_manages_nothing_it_created() {
    let host = Host::new().await;
    let session = host.session_of(&creator()).await;
    let mob = host.mob("created-before-retire");
    host.create(&session, &mob).await;

    host.retire(&creator()).await;
    host.resume_on(&creator(), &session).await;
    assert_eq!(
        host.seat_of(&session).await,
        Some((creator(), generation(1))),
        "the same identity serves the same session one generation up"
    );
    host.assert_manages_nothing(&session, &mob, "after a retire and a seat on the session")
        .await;
    host.fixture.teardown().await;
}

/// No other identity is ever seated on the creator's session: Core refuses
/// to seat another identity on a session a member of the mob was bound to,
/// so the session's management can only ever be asked for by the creating
/// identity's own incarnations. The creator's mob stays in place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_identity_is_never_seated_on_the_creators_session() {
    let host = Host::new().await;
    let session = host.session_of(&creator()).await;
    let mob = host.mob("created-by-the-first-identity");
    host.create(&session, &mob).await;

    let other = AgentIdentity::from("other");
    host.retire(&creator()).await;
    let mut spec =
        SpawnMemberSpec::new(ROLE, other.clone()).with_launch_mode(MemberLaunchMode::Resume {
            bridge_session_id: session.clone(),
            resume_from_role: None,
        });
    spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    let refused = host.state().mob_spawn_spec(&host.parent, spec).await;
    assert!(
        refused.is_err(),
        "another identity is refused the creator's session: {refused:?}"
    );
    assert_eq!(
        host.seat_of(&session).await,
        None,
        "no member serves the session after the refusal"
    );
    host.assert_manages_nothing(&session, &mob, "no member on the session")
        .await;
    host.fixture.teardown().await;
}

/// A chain of generations with a step in it that is not a same-session
/// successor step is broken at that step: the member after it manages
/// nothing created before it, and still manages what it created after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_successor_chain_broken_by_another_step_manages_nothing_created_before_it() {
    let host = Host::new().await;
    let session = host.session_of(&creator()).await;
    let before = host.mob("created-before-the-break");
    host.create(&session, &before).await;

    host.successor(MemberRespawn::new()).await;
    host.retire(&creator()).await;
    host.resume_on(&creator(), &session).await;
    assert_eq!(
        host.seat_of(&session).await,
        Some((creator(), generation(2))),
        "a successor, then a retire and a seat on the session"
    );
    let after = host.mob("created-after-the-break");
    host.create(&session, &after).await;
    host.assert_recorded_creator(&after, 2, &session).await;

    host.successor(MemberRespawn::new()).await;
    assert_eq!(
        host.seat_of(&session).await,
        Some((creator(), generation(3))),
        "one more successor"
    );
    host.assert_manages_nothing(&session, &before, "across the break")
        .await;
    host.assert_manages(&session, &after, "after the break")
        .await;
    host.assert_destroys(&session, &after, "after the break")
        .await;
    host.fixture.teardown().await;
}

/// A same-session successor that is woken, and woken again after a restart
/// of the host, keeps its management of the mob it created: a wake moves the
/// member to no other session. (On this host a wake of a member whose
/// session exists journals nothing for it; the row a wake can journal for a
/// seated member is covered in each of its shapes by the fold's own row.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_successor_woken_before_and_after_a_restart_keeps_its_management() {
    let mut host = Host::new().await;
    let session = host.session_of(&creator()).await;
    let mob = host.mob("created-before-the-wakes");
    host.create(&session, &mob).await;
    host.successor(MemberRespawn::new()).await;

    host.parent()
        .await
        .ensure_member_live(&creator())
        .await
        .expect("the successor wakes");
    assert_eq!(
        host.seat_of(&session).await,
        Some((creator(), generation(1))),
        "the wake keeps the successor's seat and session"
    );
    host.assert_manages(&session, &mob, "after the successor's wake")
        .await;

    host.fixture
        .restart_cold_with(|_| ScriptedTurn::Text("ok".to_string()), |state, _| state)
        .await;
    host.parent()
        .await
        .ensure_member_live(&creator())
        .await
        .expect("the successor wakes after the restart");
    assert_eq!(
        host.seat_of(&session).await,
        Some((creator(), generation(1))),
        "the restart and the wake keep the successor's seat and session"
    );
    host.assert_manages(&session, &mob, "after a restart and a wake")
        .await;
    host.assert_destroys(&session, &mob, "after a restart and a wake")
        .await;
    host.fixture.teardown().await;
}

/// A same-session successor that its host then adopts as an identity entry
/// keeps its management of the mob it created: the adoption seats nothing
/// and moves the member to no other session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_adopted_successor_keeps_its_management_of_the_mob_it_created() {
    let host = Host::new().await;
    // A member a host labelled with its own id, which the adoption takes.
    let adopted = AgentIdentity::from("adopted");
    let mut spec = SpawnMemberSpec::new(ROLE, adopted.clone());
    spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    spec.labels = Some(BTreeMap::from([(
        "agent_identity".to_string(),
        "adopted".to_string(),
    )]));
    host.state()
        .mob_spawn_spec(&host.parent, spec)
        .await
        .expect("seat the labelled member");
    let session = host.session_of(&adopted).await;
    let mob = host.mob("created-before-adoption");
    host.create(&session, &mob).await;

    host.parent()
        .await
        .respawn_member(adopted.clone(), MemberRespawn::new())
        .await
        .expect("the same-session successor is seated");
    host.assert_manages(&session, &mob, "after the successor")
        .await;

    let adoption = host
        .parent()
        .await
        .adopt_legacy_identity_entry(&adopted)
        .await
        .expect("the adoption answers");
    assert_eq!(
        adoption,
        meerkat_mob::LegacyIdentityEntryAdoption::Adopted,
        "the host adopts the member as an identity entry"
    );
    assert_eq!(
        host.seat_of(&session).await,
        Some((adopted, generation(1))),
        "the adoption keeps the member's seat and session"
    );
    host.assert_manages(&session, &mob, "after the adoption")
        .await;
    host.assert_destroys(&session, &mob, "after the adoption")
        .await;
    host.fixture.teardown().await;
}

/// After a reset of the creator's mob no successor's chain is followed. A
/// mob reset ends every member and restarts the mob's generations, so from
/// then on a recorded generation is taken to name only the member that
/// serves the session at exactly that generation. The creator seated again
/// on its session after the reset manages nothing it created before it; a
/// mob it creates after the reset it manages at the generation that created
/// it, and its same-session successor manages neither.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_a_mob_reset_no_successor_keeps_the_creators_management() {
    let host = Host::new().await;
    let session = host.session_of(&creator()).await;
    let before = host.mob("created-before-the-mob-reset");
    host.create(&session, &before).await;

    host.state()
        .mob_reset(&host.parent)
        .await
        .expect("reset the creator's mob");
    host.resume_on(&creator(), &session).await;
    let (member, reseated) = host
        .seat_of(&session)
        .await
        .expect("the creator serves its session again");
    assert_eq!(member, creator());
    host.assert_manages_nothing(&session, &before, "seated again after a mob reset")
        .await;

    let after = host.mob("created-after-the-mob-reset");
    host.create(&session, &after).await;
    host.assert_recorded_creator(&after, reseated.get(), &session)
        .await;
    host.assert_manages(&session, &after, "at the generation that created it")
        .await;

    host.successor(MemberRespawn::new()).await;
    assert_eq!(
        host.seat_of(&session).await,
        Some((creator(), generation(reseated.get() + 1))),
        "a same-session successor after the reset"
    );
    host.assert_manages_nothing(&session, &after, "a successor after a mob reset")
        .await;
    host.assert_manages_nothing(&session, &before, "a successor after a mob reset")
        .await;
    host.fixture.teardown().await;
}
