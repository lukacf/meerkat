//! The lineage of a mob and of a member across the hosted set
//! (`MobMcpState::lineage_of`, `MobMcpState::mob_lineage`), and the creator's
//! proof that calls through the mob's own seat proof.

use super::*;
use crate::lineage::{Lineage, LineageEnd, LineageStep};
use meerkat_mob::{LineageClosed, MobLineageCause, SeatNow};

/// A host-seated worker of `mob_id`, and its bound session.
async fn host_worker(state: &MobMcpState, mob_id: &MobId, name: &str) -> SessionId {
    state
        .mob_spawn(
            mob_id,
            ProfileName::from("worker"),
            AgentIdentity::from(name),
            Some(MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .expect("the host seats a worker");
    state
        .mob_resolve_bridge_session_id(mob_id, &AgentIdentity::from(name))
        .await
        .expect("resolve bridge session")
        .expect("worker bridge session")
}

/// A worker of `mob_id` whose seat nobody attested.
async fn unsaid_worker(state: &MobMcpState, mob_id: &MobId, name: &str) {
    let mut spec = SpawnMemberSpec::new("worker", AgentIdentity::from(name));
    spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    state
        .mob_spawn_spec(mob_id, spec)
        .await
        .expect("a worker is seated");
}

/// The mob `id` created by `session` with its own create.
async fn created_by(state: &MobMcpState, id: &str, session: &SessionId) -> MobId {
    state
        .create_mob_for_owner_bridge_session(
            explicit_definition(id),
            session.clone(),
            false,
            false,
            SessionCreateCaller::Session,
        )
        .await
        .expect("the session creates a mob")
        .mob_id
}

async fn lineage(state: &MobMcpState, mob_id: &MobId, member: &str) -> Lineage {
    state.lineage_of(mob_id, &AgentIdentity::from(member)).await
}

fn worker(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

/// A member the host seated in a mob the host created ends at the host's
/// declaration; a member whose seat nobody attested closes; a name no
/// member holds closes; a mob that is not hosted closes.
#[tokio::test]
async fn a_host_seat_in_a_hosts_mob_ends_host_declared() {
    let svc = Arc::new(MockSessionSvc::new());
    let root = tempfile::tempdir().expect("tempdir");
    let state = restored_state(&svc, root.path());
    let mob = state
        .mob_create_definition(explicit_definition("lineage-host"))
        .await
        .expect("a host create");
    host_worker(&state, &mob, "seated").await;
    unsaid_worker(&state, &mob, "unsaid").await;

    assert_eq!(
        lineage(&state, &mob, "seated").await,
        Lineage {
            steps: vec![LineageStep::MobsOwn {
                mob_id: mob.clone(),
                identity: worker("seated"),
                cause: MobLineageCause::HostSeat,
            }],
            end: LineageEnd::HostDeclared,
        }
    );
    assert!(lineage(&state, &mob, "seated").await.is_host_declared());
    assert_eq!(
        state.mob_lineage(&mob).await,
        Lineage {
            steps: Vec::new(),
            end: LineageEnd::HostDeclared,
        }
    );
    assert_eq!(
        lineage(&state, &mob, "unsaid").await.end,
        LineageEnd::Closed(LineageClosed::Unproven)
    );
    assert_eq!(
        lineage(&state, &mob, "nobody").await.end,
        LineageEnd::Closed(LineageClosed::NoMember)
    );
    assert_eq!(
        lineage(&state, &MobId::from("no-such-mob"), "seated")
            .await
            .end,
        LineageEnd::Closed(LineageClosed::NotHosted)
    );
}

/// A member of a mob that a member's session created continues at that
/// member: here to the host's declaration. Once the creating member is
/// gone the walk closes there.
#[tokio::test]
async fn a_mob_a_members_session_created_continues_at_that_member() {
    let svc = Arc::new(MockSessionSvc::new());
    let root = tempfile::tempdir().expect("tempdir");
    let state = restored_state(&svc, root.path());
    let parent = state
        .mob_create_definition(explicit_definition("lineage-parent"))
        .await
        .expect("a host create");
    let creator_session = host_worker(&state, &parent, "creator").await;
    let child = created_by(&state, "lineage-child", &creator_session).await;
    host_worker(&state, &child, "helper").await;

    assert_eq!(
        lineage(&state, &child, "helper").await,
        Lineage {
            steps: vec![
                LineageStep::MobsOwn {
                    mob_id: child.clone(),
                    identity: worker("helper"),
                    cause: MobLineageCause::HostSeat,
                },
                LineageStep::MobCreatedBy {
                    mob_id: child.clone(),
                    parent_mob_id: parent.clone(),
                    creator: worker("creator"),
                    now: SeatNow::Current,
                },
                LineageStep::MobsOwn {
                    mob_id: parent.clone(),
                    identity: worker("creator"),
                    cause: MobLineageCause::HostSeat,
                },
            ],
            end: LineageEnd::HostDeclared,
        }
    );

    state
        .mob_retire(&parent, worker("creator"))
        .await
        .expect("retire the creating member");
    let gone = lineage(&state, &child, "helper").await;
    assert_eq!(gone.end, LineageEnd::Closed(LineageClosed::CreatorGone));
    assert_eq!(
        gone.steps.last(),
        Some(&LineageStep::MobCreatedBy {
            mob_id: child.clone(),
            parent_mob_id: parent.clone(),
            creator: worker("creator"),
            now: SeatNow::Gone,
        })
    );
}

/// A mob created by a creating member whose own seat nobody attested
/// closes at that member; a mob created by a session that no roster names
/// closes.
#[tokio::test]
async fn a_mob_whose_creator_is_not_host_declared_closes() {
    let svc = Arc::new(MockSessionSvc::new());
    let root = tempfile::tempdir().expect("tempdir");
    let state = restored_state(&svc, root.path());
    let parent = state
        .mob_create_definition(explicit_definition("lineage-unsaid-parent"))
        .await
        .expect("a host create");
    unsaid_worker(&state, &parent, "creator").await;
    let creator_session = state
        .mob_resolve_bridge_session_id(&parent, &worker("creator"))
        .await
        .expect("resolve bridge session")
        .expect("creator bridge session");
    let child = created_by(&state, "lineage-unsaid-child", &creator_session).await;
    host_worker(&state, &child, "helper").await;
    assert_eq!(
        lineage(&state, &child, "helper").await.end,
        LineageEnd::Closed(LineageClosed::Unproven)
    );

    let caller = SessionId::new();
    svc.insert_persisted_session(Session::with_id(caller.clone()))
        .await;
    let sessions = created_by(&state, "lineage-session-child", &caller).await;
    host_worker(&state, &sessions, "helper").await;
    assert_eq!(
        lineage(&state, &sessions, "helper").await.end,
        LineageEnd::Closed(LineageClosed::SessionNoRoster)
    );
    assert_eq!(
        state.mob_lineage(&sessions).await.end,
        LineageEnd::Closed(LineageClosed::SessionNoRoster)
    );
}

/// The walk closes at its bound.
#[tokio::test]
async fn the_walk_closes_at_its_bound() {
    let svc = Arc::new(MockSessionSvc::new());
    let root = tempfile::tempdir().expect("tempdir");
    let state = restored_state(&svc, root.path());
    let mut mob = state
        .mob_create_definition(explicit_definition("lineage-depth-0"))
        .await
        .expect("a host create");
    // Each level adds two steps: the member in its mob, and the mob's
    // creating member.
    let levels = crate::lineage::LINEAGE_STEP_BOUND / 2 + 2;
    for level in 1..=levels {
        let session = host_worker(&state, &mob, "member").await;
        mob = created_by(&state, &format!("lineage-depth-{level}"), &session).await;
    }
    host_worker(&state, &mob, "member").await;
    let deep = lineage(&state, &mob, "member").await;
    assert_eq!(deep.end, LineageEnd::Closed(LineageClosed::Bound));
    assert_eq!(deep.steps.len(), crate::lineage::LINEAGE_STEP_BOUND);
}

/// [`restored_state`] that loads a mob only when a call needs it.
fn on_demand_state(svc: &Arc<MockSessionSvc>, root: &std::path::Path) -> Arc<MobMcpState> {
    Arc::new(
        MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
            .expect("runtime custody")
            .with_persistent_storage_root(Some(root.to_path_buf()))
            .with_mob_restore_policy(MobRestorePolicy::OnDemand),
    )
}

/// A parent mob with a creating member and the child mob that member's
/// session created, of an earlier process; the creating session.
async fn parent_and_child_of_an_earlier_process(
    svc: &Arc<MockSessionSvc>,
    root: &std::path::Path,
    parent_id: &str,
    child_id: &str,
) -> (MobId, MobId, SessionId) {
    let state = restored_state(svc, root);
    let parent = state
        .mob_create_definition(explicit_definition(parent_id))
        .await
        .expect("a host create");
    let creator_session = host_worker(&state, &parent, "creator").await;
    let child = created_by(&state, child_id, &creator_session).await;
    host_worker(&state, &child, "helper").await;
    for mob_id in [&child, &parent] {
        state
            .handle_for(mob_id)
            .await
            .expect("predecessor mob handle")
            .shutdown()
            .await
            .expect("retire the predecessor supervisor route");
    }
    (parent, child, creator_session)
}

async fn loaded(state: &MobMcpState, mob_id: &MobId) -> bool {
    state.hosted.read().await.is_loaded(mob_id)
}

/// The read loads nothing: a mob that is hosted and not loaded closes the
/// walk, as the member's own mob and as a parent.
#[tokio::test]
async fn a_parent_that_is_not_loaded_closes_and_nothing_is_loaded() {
    let svc = Arc::new(MockSessionSvc::new());
    let root = tempfile::tempdir().expect("tempdir");
    let (parent, child, _) = parent_and_child_of_an_earlier_process(
        &svc,
        root.path(),
        "lineage-lazy-parent",
        "lineage-lazy-child",
    )
    .await;
    let state = on_demand_state(&svc, root.path());
    state.ensure_restored().await.expect("restore the set");

    assert_eq!(
        lineage(&state, &child, "helper").await.end,
        LineageEnd::Closed(LineageClosed::NotLoaded),
        "the member's own mob is not loaded"
    );
    assert!(!loaded(&state, &child).await && !loaded(&state, &parent).await);

    state.handle_for(&child).await.expect("load the child");
    let read = lineage(&state, &child, "helper").await;
    assert_eq!(read.end, LineageEnd::Closed(LineageClosed::NotLoaded));
    assert_eq!(
        read.steps,
        vec![LineageStep::MobsOwn {
            mob_id: child.clone(),
            identity: worker("helper"),
            cause: MobLineageCause::HostSeat,
        }],
        "the walk stops before the parent"
    );
    assert!(!loaded(&state, &parent).await, "the read loaded the parent");
}

/// The creator's proof refuses a session that is not the recorded creator
/// before it resolves the creating member's mob: nothing is loaded for it.
/// For the recorded creator's session it resolves that mob, as before.
#[tokio::test]
async fn a_session_that_is_not_the_recorded_creator_is_refused_and_no_parent_is_resolved_or_loaded()
{
    let svc = Arc::new(MockSessionSvc::new());
    let root = tempfile::tempdir().expect("tempdir");
    let (parent, child, creator_session) = parent_and_child_of_an_earlier_process(
        &svc,
        root.path(),
        "lineage-proof-parent",
        "lineage-proof-child",
    )
    .await;
    let state = on_demand_state(&svc, root.path());
    let child_handle = state.handle_for(&child).await.expect("load the child");

    assert!(
        !state
            .session_is_recorded_creator(&child_handle, &SessionId::new())
            .await
    );
    assert!(
        !loaded(&state, &parent).await,
        "another session's ask loaded the creating member's mob"
    );
    let _ = state
        .session_is_recorded_creator(&child_handle, &creator_session)
        .await;
    assert!(
        loaded(&state, &parent).await,
        "the recorded creator's ask resolves the creating member's mob"
    );
}

/// The read spans every hosted mob, so only the owner's console answers it:
/// any other console's read closes.
#[tokio::test]
async fn a_console_that_is_not_the_owners_reads_no_lineage() {
    let svc = Arc::new(MockSessionSvc::new());
    let root = tempfile::tempdir().expect("tempdir");
    let state = restored_state(&svc, root.path());
    let mob = state
        .mob_create_definition(explicit_definition("lineage-scoped"))
        .await
        .expect("a host create");
    host_worker(&state, &mob, "seated").await;
    let principal = meerkat_core::auth::PrincipalId::new("someone").expect("principal id");
    let scoped =
        MobMcpState::new_in_memory_as(meerkat_mob::MobControlPrincipal::External(principal));
    scoped
        .mob_insert_handle(mob.clone(), state.handle_for(&mob).await.expect("handle"))
        .await;
    assert!(lineage(&state, &mob, "seated").await.is_host_declared());
    assert_eq!(
        lineage(&scoped, &mob, "seated").await.end,
        LineageEnd::Closed(LineageClosed::Unreadable)
    );
}
