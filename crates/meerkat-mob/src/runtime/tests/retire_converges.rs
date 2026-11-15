//! A retire names an identity. The handle reads the identity's roster entry
//! and sends that exact incarnation, and the actor decides in the command's
//! own step what the incarnation has become since the read:
//!
//! - its retirement completed (MobMachine maps the identity to no runtime,
//!   holds no respawn of it, and the roster lists none): the retire answers
//!   what the retire of an absent identity answers, `Ok(())`;
//! - its retirement is durably started (MobMachine marks its runtime
//!   Retiring): the retire joins that retirement and answers its outcome;
//! - MobMachine holds another incarnation's material, or a member that is
//!   not retiring changed its roster transport: the request is stale and is
//!   refused with `StaleMemberOperatorAuthority`, as before.
//!
//! Every test runs a real turn-driven member over the runtime-backed session
//! service. Holds and fault hooks pin each window deterministically.

use super::*;
use crate::machines::mob_machine as mob_dsl;
use crate::roster::RosterEntry;
use crate::runtime::actor::retirement_stage_hold;
use crate::runtime::state::{
    MobCommand, RetireMemberIncarnation, RetireTarget, RetirementRedrive, ScopedRetireStep,
    ScopedRetireStepKind, ScopedRetireVerdict,
};
use crate::runtime::{MemberDeliveryScope, RetirementSettlement};

/// Bound for test steps that must happen promptly; it only turns a hang into
/// a failure.
const STEP: Duration = Duration::from_secs(10);

async fn turn_driven_mob(label: &str) -> (MobHandle, Arc<RuntimeBackedRealCommsSessionService>) {
    let mut definition = with_unique_mob_id(sample_definition(), label);
    definition
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .expect("lead profile")
        .as_inline_mut()
        .expect("inline lead profile")
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    create_test_mob_with_runtime_backed_real_comms(definition).await
}

/// Spawn a member and return the roster entry a retire's handle would read.
async fn spawn_member(handle: &MobHandle, name: &str) -> RosterEntry {
    let identity = AgentIdentity::from(name);
    handle
        .spawn(ProfileName::from("lead"), identity.clone(), None)
        .await
        .expect("spawn member");
    roster_entry(handle, &identity)
        .await
        .expect("the spawned member is listed")
}

async fn roster_entry(handle: &MobHandle, identity: &AgentIdentity) -> Option<RosterEntry> {
    handle.roster.read().await.get(identity).cloned()
}

fn maps(handle: &MobHandle, identity: &AgentIdentity) -> bool {
    handle
        .machine_state_watch_rx
        .borrow()
        .identity_to_runtime
        .contains_key(&mob_dsl::AgentIdentity::from_domain(identity))
}

fn marked_retiring(handle: &MobHandle, entry: &RosterEntry) -> bool {
    handle
        .machine_state_watch_rx
        .borrow()
        .member_state_markers
        .get(&mob_dsl::AgentRuntimeId::from_domain(
            &entry.agent_runtime_id,
        ))
        == Some(&mob_dsl::MobMemberState::Retiring)
}

/// The exact incarnation a handle that read `entry` sends.
fn incarnation_of(entry: &RosterEntry) -> RetireMemberIncarnation {
    RetireMemberIncarnation {
        agent_identity: entry.agent_identity.clone(),
        agent_runtime_id: entry.agent_runtime_id.clone(),
        generation: entry.generation,
        fence_token: entry.fence_token,
        member_ref: entry.member_ref.clone(),
    }
}

/// `entry`'s incarnation as a handle read it before the member's roster
/// transport changed: the same MobMachine material, another member ref.
fn incarnation_before_a_transport_change(entry: &RosterEntry) -> RetireMemberIncarnation {
    let read = RetireMemberIncarnation {
        member_ref: MemberRef::from_bridge_session_id(SessionId::new()),
        ..incarnation_of(entry)
    };
    assert_ne!(read.member_ref, entry.member_ref);
    read
}

/// Send the exact retire command a handle sends after its roster read, and
/// return the actor's answer to it.
async fn exact_retire(
    handle: &MobHandle,
    expected: RetireMemberIncarnation,
    redrive: RetirementRedrive,
) -> Result<(), MobError> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let (admission_tx, _admission_rx) = tokio::sync::watch::channel(false);
    handle
        .command_tx
        .send(crate::runtime::scope_gate::RoutedMobCommand {
            authority: handle.command_authority.clone(),
            cmd: MobCommand::Retire {
                agent_identity: expected.agent_identity.clone(),
                redrive,
                target: RetireTarget::Exact(expected),
                deadline: Instant::now() + Duration::from_secs(15),
                admission_tx,
                reply_tx,
            },
        })
        .await
        .expect("send the exact retire");
    tokio::time::timeout(STEP, reply_rx)
        .await
        .expect("the actor answers the exact retire")
        .expect("the actor answers before it drops the reply")
}

/// Send the descendant step a scoped retire (`retire_at_scope`) of `root`
/// sends for a descendant it read from the roster as `expected`, while the
/// root's incarnation is current. Returns the actor's answer and the root
/// scope's verdict the step took.
async fn scoped_descendant_retire(
    handle: &MobHandle,
    root: &MemberDeliveryScope,
    expected: RetireMemberIncarnation,
) -> (Result<(), MobError>, ScopedRetireVerdict) {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let (admission_tx, _admission_rx) = tokio::sync::watch::channel(false);
    let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel();
    handle
        .command_tx
        .send(crate::runtime::scope_gate::RoutedMobCommand {
            authority: handle.command_authority.clone(),
            cmd: MobCommand::Retire {
                agent_identity: expected.agent_identity.clone(),
                redrive: RetirementRedrive::Plain,
                target: RetireTarget::Scoped(ScopedRetireStep {
                    root: root.clone(),
                    kind: ScopedRetireStepKind::DescendantBeforeRoot(expected),
                    verdict_tx,
                }),
                deadline: Instant::now() + Duration::from_secs(15),
                admission_tx,
                reply_tx,
            },
        })
        .await
        .expect("send the scoped descendant step");
    let answer = tokio::time::timeout(STEP, reply_rx)
        .await
        .expect("the actor answers the scoped step")
        .expect("the actor answers before it drops the reply");
    let verdict = verdict_rx
        .await
        .expect("the step classified the root scope");
    (answer, verdict)
}

fn settlement(handle: &MobHandle, identity: &AgentIdentity) -> RetirementSettlement {
    handle
        .retirement_settlement(identity)
        .expect("the retirement published a settlement")
        .current()
}

/// Make `identity`'s retirement stuck after its durable start: the session
/// archive stage fails. The member stays listed, MobMachine marks its
/// runtime Retiring, and the roster entry a later retire's handle reads is
/// returned.
async fn stuck_retirement(
    handle: &MobHandle,
    service: &RuntimeBackedRealCommsSessionService,
    identity: &AgentIdentity,
) -> RosterEntry {
    service.set_fail_archive(true);
    let error = tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("retire answers")
        .expect_err("a failing archive stage does not retire the member");
    service.set_fail_archive(false);
    assert!(
        matches!(error, MobError::MemberRetirementStuck { .. }),
        "a stage failure after the durable start is stuck: {error:?}"
    );
    let anchor = roster_entry(handle, identity)
        .await
        .expect("a stuck member stays listed");
    assert!(
        marked_retiring(handle, &anchor),
        "MobMachine marks the stuck member's runtime Retiring"
    );
    anchor
}

fn assert_stale(answer: &Result<(), MobError>, identity: &AgentIdentity, what: &str) {
    assert!(
        matches!(
            answer,
            Err(MobError::StaleMemberOperatorAuthority { member_id, .. }) if member_id == identity
        ),
        "{what}: {answer:?}"
    );
}

/// The defect's interleaving, pinned: the mob's own re-drive of a member's
/// retirement is in flight when a caller retires the member. The handle
/// reads the member's roster entry and sends that exact incarnation; the
/// actor holds the command until its own retirement of the member ended, and
/// then the roster no longer lists the incarnation. The retire answers
/// `Ok(())`: the member is retired. It used to answer
/// `SharedRetirementFailure(StaleMemberOperatorAuthority)` although the
/// member was retired and a second retire answered `Ok(())`.
#[tokio::test]
async fn a_retire_that_meets_the_mobs_own_redrive_answers_ok_once_the_member_is_retired() {
    let (handle, service) = turn_driven_mob("retire-converges-owned").await;
    let identity = spawn_member(&handle, "retire-converges-owned-member")
        .await
        .agent_identity;
    let entry = stuck_retirement(&handle, &service, &identity).await;

    // A resume hands the stuck retirement back as the mob's own retirement.
    // It is held at its last stage, whose commit removes the roster anchor.
    let hold = retirement_stage_hold::install(&identity, "retire-projection-cleanup");
    handle.stop().await.expect("stop");
    handle.resume().await.expect("resume");
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the mob's own re-drive reaches projection cleanup");
    assert!(
        roster_entry(&handle, &identity)
            .await
            .is_some_and(|listed| incarnation_of(&listed) == incarnation_of(&entry)),
        "the roster anchor goes last: the retire's handle still reads the incarnation"
    );

    let mut parked = handle
        .lifecycle_observations
        .parked_member_controls
        .subscribe();
    let parked_before = parked.borrow().get(&identity).copied().unwrap_or(0);
    let retire = {
        let handle = handle.clone();
        let identity = identity.clone();
        tokio::spawn(async move { handle.retire(identity).await })
    };
    tokio::time::timeout(
        STEP,
        parked.wait_for(|parked| parked.get(&identity).copied().unwrap_or(0) > parked_before),
    )
    .await
    .expect("the actor holds the retire behind its own retirement of the member")
    .expect("the probe outlives the test");
    assert!(
        !retire.is_finished(),
        "the held retire answers only once the mob's retirement ended"
    );

    hold.release();
    let answer = tokio::time::timeout(STEP, retire)
        .await
        .expect("the retire answers once the mob's retirement ended")
        .expect("join the retire");
    assert!(
        matches!(answer, Ok(())),
        "a retire that met the mob's own retirement of the member answers Ok once the \
         member is retired: {answer:?}"
    );
    assert!(roster_entry(&handle, &identity).await.is_none());
    assert!(!maps(&handle, &identity));
    assert!(matches!(
        settlement(&handle, &identity),
        RetirementSettlement::Retired
    ));
}

/// A retire whose budget ends while the mob's own retirement of the member
/// still runs gets the documented in-progress answer naming the stage, as a
/// retire whose own retirement outlives its budget does, and no refusal. The
/// mob's retirement completes without it. The retirement is held past its
/// archive observation, where MobMachine no longer marks the runtime
/// Retiring: the answer there used to be `MemberRetirementAdmissionPending`.
#[tokio::test]
async fn a_retire_whose_budget_ends_during_the_mobs_own_redrive_answers_in_progress() {
    let (handle, service) = turn_driven_mob("retire-converges-budget").await;
    let identity = spawn_member(&handle, "retire-converges-budget-member")
        .await
        .agent_identity;
    stuck_retirement(&handle, &service, &identity).await;
    let hold = retirement_stage_hold::install(&identity, "retire-projection-cleanup");
    handle.stop().await.expect("stop");
    handle.resume().await.expect("resume");
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the mob's own re-drive reaches projection cleanup");
    assert!(
        !maps(&handle, &identity),
        "the held retirement is past its archive observation"
    );

    let error = tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("the caller's wait ends at its budget")
        .expect_err("the mob's retirement is held past the caller's budget");
    assert!(
        matches!(
            &error,
            MobError::MemberRetirementInProgress { member_id, stage }
                if member_id == &identity && stage == "retire-projection-cleanup"
        ),
        "the budget's end answers the retirement in progress, naming its stage: {error:?}"
    );

    let mut watch = handle
        .retirement_settlement(&identity)
        .expect("the mob's retirement published its progress");
    hold.release();
    let settled = tokio::time::timeout(STEP, watch.settled())
        .await
        .expect("the mob's retirement settles on its own")
        .expect("the actor settles the retirement before exiting");
    assert!(
        matches!(settled, RetirementSettlement::Retired),
        "{settled:?}"
    );
    tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("retire answers")
        .expect("a retire of the retired member answers Ok");
    assert!(roster_entry(&handle, &identity).await.is_none());
}

/// The same decision without a race: an exact retire naming the incarnation
/// whose retirement already completed answers `Ok(())`, MobMachine's answer
/// to the retire of an absent identity, and changes nothing.
#[tokio::test]
async fn an_exact_retire_naming_the_incarnation_whose_retirement_completed_answers_ok() {
    let (handle, _service) = turn_driven_mob("retire-converges-completed").await;
    let entry = spawn_member(&handle, "retire-converges-completed-member").await;
    let identity = entry.agent_identity.clone();
    tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("retire answers")
        .expect("the member retires");
    assert!(roster_entry(&handle, &identity).await.is_none());
    assert!(!maps(&handle, &identity));

    let before = handle.machine_state_watch_rx.borrow().clone();
    let answer = exact_retire(&handle, incarnation_of(&entry), RetirementRedrive::Plain).await;
    assert!(
        matches!(answer, Ok(())),
        "the named incarnation's retirement completed: {answer:?}"
    );
    assert!(
        *handle.machine_state_watch_rx.borrow() == before,
        "the answer changes no machine state"
    );
    assert!(roster_entry(&handle, &identity).await.is_none());
}

/// The same decision in a step of a scoped retire (`retire_at_scope`): the
/// step for a descendant the handle read from the roster, sent once that
/// descendant's retirement completed, is admitted by the root scope and
/// answers `Ok(())` without retiring anything. A descendant step naming
/// material MobMachine does not hold is still refused as stale.
#[tokio::test]
async fn a_scoped_retire_step_naming_a_descendant_whose_retirement_completed_answers_ok() {
    let (handle, _service) = turn_driven_mob("retire-converges-scoped").await;
    let root = spawn_member(&handle, "retire-converges-scoped-root").await;
    let child_identity = AgentIdentity::from("retire-converges-scoped-child");
    let mut spec = SpawnMemberSpec::new("lead", child_identity.clone());
    spec.spawned_by = Some(root.agent_identity.clone());
    handle
        .spawn_spec(spec)
        .await
        .expect("spawn a member owned by the root");
    let child = roster_entry(&handle, &child_identity)
        .await
        .expect("the child is listed");
    let scope = handle
        .capture_member_delivery_scope(&root.agent_identity)
        .await
        .expect("capture the root's delivery scope");

    // The child retires after the scoped retire's handle read it.
    tokio::time::timeout(STEP, handle.retire(child_identity.clone()))
        .await
        .expect("retire answers")
        .expect("the child retires");
    assert!(roster_entry(&handle, &child_identity).await.is_none());
    assert!(!maps(&handle, &child_identity));

    let before = handle.machine_state_watch_rx.borrow().clone();
    let (answer, verdict) = scoped_descendant_retire(&handle, &scope, incarnation_of(&child)).await;
    assert!(
        verdict.admitted && verdict.verdict == mob_dsl::RetireAtScopeVerdictKind::Current,
        "the root scope is current and admits its descendant's step: {verdict:?}"
    );
    assert!(
        matches!(answer, Ok(())),
        "the named descendant's retirement completed: {answer:?}"
    );
    assert!(
        *handle.machine_state_watch_rx.borrow() == before,
        "the answer changes no machine state"
    );
    assert!(
        roster_entry(&handle, &root.agent_identity)
            .await
            .is_some_and(|listed| incarnation_of(&listed) == incarnation_of(&root)),
        "the root stays seated"
    );

    let stale = RetireMemberIncarnation {
        fence_token: FenceToken::new(child.fence_token.get() + 1),
        ..incarnation_of(&child)
    };
    let (answer, verdict) = scoped_descendant_retire(&handle, &scope, stale).await;
    assert!(verdict.admitted, "{verdict:?}");
    assert_stale(
        &answer,
        &child_identity,
        "a descendant step with other MobMachine material is refused",
    );
}

/// A retire held behind a respawn of the member names the incarnation the
/// respawn superseded. The respawn's retirement of that incarnation ends
/// with MobMachine holding the respawn for its successor, so the retire is
/// not answered `Ok(())` (the member is not retired: a successor is owed):
/// it is refused as stale (a shared retirement failure for the caller),
/// whether it is answered before or after the successor is seated, and the
/// successor is not retired.
#[tokio::test]
async fn a_retire_held_behind_a_respawn_names_a_superseded_incarnation_and_is_refused_stale() {
    let (handle, _service) = turn_driven_mob("retire-converges-respawn").await;
    let first = spawn_member(&handle, "retire-converges-respawn-member").await;
    let identity = first.agent_identity.clone();
    let hold = retirement_stage_hold::install(&identity, "retire-projection-cleanup");
    let respawn = {
        let handle = handle.clone();
        let identity = identity.clone();
        tokio::spawn(async move { handle.respawn(identity, None).await })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the respawn's retirement of the member reaches projection cleanup");

    let mut parked = handle
        .lifecycle_observations
        .parked_member_controls
        .subscribe();
    let parked_before = parked.borrow().get(&identity).copied().unwrap_or(0);
    let retire = {
        let handle = handle.clone();
        let identity = identity.clone();
        tokio::spawn(async move { handle.retire(identity).await })
    };
    tokio::time::timeout(
        STEP,
        parked.wait_for(|parked| parked.get(&identity).copied().unwrap_or(0) > parked_before),
    )
    .await
    .expect("the actor holds the retire behind the respawn's retirement")
    .expect("the probe outlives the test");

    hold.release();
    tokio::time::timeout(STEP, respawn)
        .await
        .expect("the respawn answers")
        .expect("join the respawn")
        .expect("the respawn seats a successor");
    let answer = tokio::time::timeout(STEP, retire)
        .await
        .expect("the retire answers")
        .expect("join the retire");
    assert!(
        matches!(
            &answer,
            Err(MobError::SharedRetirementFailure(cause))
                if matches!(
                    cause.as_ref(),
                    MobError::StaleMemberOperatorAuthority { member_id, .. } if member_id == &identity
                )
        ),
        "a retire held behind a respawn names the superseded incarnation: {answer:?}"
    );
    let successor = roster_entry(&handle, &identity)
        .await
        .expect("the successor is seated and was not retired");
    assert_ne!(first.agent_runtime_id, successor.agent_runtime_id);
    assert!(maps(&handle, &identity));
    assert!(!marked_retiring(&handle, &successor));
}

/// Negative: a request naming an incarnation MobMachine no longer holds is
/// still refused as stale, whatever became of the identity since. While a
/// successor is seated the stale request does not retire it, and once the
/// successor itself retired the stale request still does not get the
/// completed incarnation's `Ok(())`.
#[tokio::test]
async fn an_exact_retire_naming_a_superseded_incarnation_is_refused_stale() {
    let (handle, _service) = turn_driven_mob("retire-converges-superseded").await;
    let first = spawn_member(&handle, "retire-converges-superseded-member").await;
    let identity = first.agent_identity.clone();
    handle
        .respawn(identity.clone(), None)
        .await
        .expect("respawn the member");
    let successor = roster_entry(&handle, &identity)
        .await
        .expect("the successor is listed");
    assert_ne!(first.agent_runtime_id, successor.agent_runtime_id);

    let answer = exact_retire(&handle, incarnation_of(&first), RetirementRedrive::Plain).await;
    assert_stale(
        &answer,
        &identity,
        "a request naming the superseded incarnation must not retire the successor",
    );
    assert!(
        roster_entry(&handle, &identity)
            .await
            .is_some_and(|listed| incarnation_of(&listed) == incarnation_of(&successor)),
        "the successor stays seated"
    );
    assert!(handle.retirement_settlement(&identity).is_none());

    tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("retire answers")
        .expect("the successor retires");
    assert!(roster_entry(&handle, &identity).await.is_none());
    assert!(!maps(&handle, &identity));

    let answer = exact_retire(&handle, incarnation_of(&first), RetirementRedrive::Plain).await;
    assert_stale(
        &answer,
        &identity,
        "the superseded incarnation is not the one whose retirement completed",
    );
}

/// An exact retire of a member whose retirement is durably started joins
/// that retirement even when the member's roster transport changed since the
/// handle read it: it answers the retirement's own outcome (stuck, for a
/// plain retire of a stuck retirement) and its typed re-drive completes it.
/// A request whose MobMachine material differs is refused as stale and
/// re-drives nothing.
#[tokio::test]
async fn an_exact_retire_joins_a_durably_started_retirement_whose_roster_transport_changed() {
    let (handle, service) = turn_driven_mob("retire-converges-join").await;
    let identity = spawn_member(&handle, "retire-converges-join-member")
        .await
        .agent_identity;
    let entry = stuck_retirement(&handle, &service, &identity).await;

    let answer = exact_retire(
        &handle,
        incarnation_before_a_transport_change(&entry),
        RetirementRedrive::Plain,
    )
    .await;
    assert!(
        matches!(
            &answer,
            Err(MobError::MemberRetirementStuck { member_id, .. }) if member_id == &identity
        ),
        "a plain retire joins the stuck retirement and answers its outcome: {answer:?}"
    );
    assert!(roster_entry(&handle, &identity).await.is_some());

    // Negative: another fence token is another incarnation's material. Its
    // re-drive is refused and the retirement stays stuck.
    let stale = RetireMemberIncarnation {
        fence_token: FenceToken::new(entry.fence_token.get() + 1),
        ..incarnation_of(&entry)
    };
    let answer = exact_retire(&handle, stale, RetirementRedrive::Redrive).await;
    assert_stale(
        &answer,
        &identity,
        "a request with other MobMachine material re-drives nothing",
    );
    assert!(matches!(
        settlement(&handle, &identity),
        RetirementSettlement::Stuck { .. }
    ));
    assert!(roster_entry(&handle, &identity).await.is_some());

    let answer = exact_retire(
        &handle,
        incarnation_before_a_transport_change(&entry),
        RetirementRedrive::Redrive,
    )
    .await;
    assert!(
        matches!(answer, Ok(())),
        "the typed re-drive joins the durably started retirement and completes it: {answer:?}"
    );
    assert!(roster_entry(&handle, &identity).await.is_none());
    assert!(!maps(&handle, &identity));
}

/// Negative: a member that is not retiring and whose roster transport
/// differs from the one the request read is still refused as stale. Nothing
/// is retired and no retirement starts.
#[tokio::test]
async fn an_exact_retire_of_a_live_member_whose_roster_transport_changed_is_refused_stale() {
    let (handle, _service) = turn_driven_mob("retire-converges-live").await;
    let entry = spawn_member(&handle, "retire-converges-live-member").await;
    let identity = entry.agent_identity.clone();

    let answer = exact_retire(
        &handle,
        incarnation_before_a_transport_change(&entry),
        RetirementRedrive::Plain,
    )
    .await;
    assert_stale(
        &answer,
        &identity,
        "a live member's changed roster transport makes the request stale",
    );
    assert!(
        roster_entry(&handle, &identity)
            .await
            .is_some_and(|listed| incarnation_of(&listed) == incarnation_of(&entry)),
        "the member stays seated"
    );
    assert!(maps(&handle, &identity));
    assert!(!marked_retiring(&handle, &entry));
    assert!(handle.retirement_settlement(&identity).is_none());
}

/// `Ok(())` is the retire's terminal barrier, so it is never answered while
/// the roster anchor remains. A retirement stuck after its archive
/// observation leaves the identity mapped to no runtime and keeps the
/// anchor: the exact retire of that incarnation answers the stuck
/// retirement, and a request that read another roster transport is refused
/// as stale, never answered `Ok(())`.
#[tokio::test]
async fn an_exact_retire_never_answers_ok_while_the_roster_anchor_remains() {
    let (handle, _service) = turn_driven_mob("retire-converges-anchor").await;
    let entry = spawn_member(&handle, "retire-converges-anchor-member").await;
    let identity = entry.agent_identity.clone();
    super::actor::fail_retirement_projection_cleanup_for_test(&identity);
    let error = tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("retire answers")
        .expect_err("a failing projection cleanup leaves the retirement stuck");
    assert!(
        matches!(error, MobError::MemberRetirementStuck { .. }),
        "{error:?}"
    );
    assert!(
        !maps(&handle, &identity),
        "the retirement got stuck after its archive observation"
    );
    let anchor = roster_entry(&handle, &identity)
        .await
        .expect("the roster anchor remains");

    let answer = exact_retire(
        &handle,
        incarnation_before_a_transport_change(&anchor),
        RetirementRedrive::Plain,
    )
    .await;
    assert_stale(&answer, &identity, "no Ok while the roster anchor remains");
    let answer = exact_retire(&handle, incarnation_of(&anchor), RetirementRedrive::Plain).await;
    assert!(
        matches!(
            &answer,
            Err(MobError::MemberRetirementStuck { member_id, .. }) if member_id == &identity
        ),
        "the exact retire answers the stuck retirement: {answer:?}"
    );
    assert!(roster_entry(&handle, &identity).await.is_some());
}
