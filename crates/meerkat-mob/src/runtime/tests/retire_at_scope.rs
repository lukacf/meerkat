//! `MobHandle::retire_at_scope`: a scoped retire retires the member
//! incarnation its delivery scope names, admitted by generated MobMachine
//! authority in every step's own actor command, or it retires nothing.
//!
//! Every test runs a real turn-driven member over the runtime-backed session
//! service. Holds and fault hooks pin each window deterministically.

use super::*;
use crate::machines::mob_machine as mob_dsl;
use crate::runtime::MemberDeliveryScope;

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

async fn spawn_member(handle: &MobHandle, name: &str) -> (AgentIdentity, SessionId) {
    let identity = AgentIdentity::from(name);
    let session_id = handle
        .spawn(ProfileName::from("lead"), identity.clone(), None)
        .await
        .expect("spawn member")
        .bridge_session_id()
        .expect("session-backed member")
        .clone();
    (identity, session_id)
}

async fn spawn_child(handle: &MobHandle, name: &str, parent: &AgentIdentity) -> AgentIdentity {
    let identity = AgentIdentity::from(name);
    let mut spec = SpawnMemberSpec::new("lead", identity.clone());
    spec.spawned_by = Some(parent.clone());
    handle
        .spawn_spec(spec)
        .await
        .expect("spawn a member owned by the parent identity");
    identity
}

async fn roster_has(handle: &MobHandle, identity: &AgentIdentity) -> bool {
    handle.roster.read().await.get(identity).is_some()
}

async fn scope_of(handle: &MobHandle, identity: &AgentIdentity) -> MemberDeliveryScope {
    handle
        .capture_member_delivery_scope(identity)
        .await
        .expect("capture the member's delivery scope")
}

async fn retire_at_scope(handle: &MobHandle, scope: &MemberDeliveryScope) -> Result<(), MobError> {
    tokio::time::timeout(STEP, handle.retire_at_scope(scope))
        .await
        .expect("retire_at_scope answers")
}

fn spawn_retire_at_scope(
    handle: &MobHandle,
    scope: &MemberDeliveryScope,
) -> tokio::task::JoinHandle<Result<(), MobError>> {
    let handle = handle.clone();
    let scope = scope.clone();
    tokio::spawn(async move { handle.retire_at_scope(&scope).await })
}

fn machine_state(handle: &MobHandle) -> mob_dsl::MobMachineState {
    handle.machine_state_watch_rx.borrow().clone()
}

fn assert_stale_fence(
    error: &MobError,
    scope: &MemberDeliveryScope,
    current: &MemberDeliveryScope,
) {
    assert!(
        matches!(
            error,
            MobError::StaleFenceToken {
                runtime_id,
                expected,
                actual,
            } if runtime_id == scope.runtime_id()
                && *expected == current.fence_token()
                && *actual == scope.fence_token()
        ),
        "an older scope is refused against the current incarnation: {error:?}"
    );
}

/// A scope from another incarnation of this mob (one destroyed and created
/// again under its id) never reaches a step: the scoped retire refuses it
/// with `IncarnationMismatch` and retires nothing. Control: the scope
/// captured at this incarnation still retires its member.
#[tokio::test]
async fn a_scope_from_another_mob_incarnation_retires_nothing() {
    let (handle, _service) = turn_driven_mob("k3-incarnation").await;
    let (identity, _) = spawn_member(&handle, "k3-incarnation-member").await;
    let scope = scope_of(&handle, &identity).await;
    let mut foreign = serde_json::to_value(&scope).expect("the scope serializes");
    foreign["mob_incarnation"] =
        serde_json::Value::String(meerkat_core::service::MobIncarnationId::mint().to_string());
    let foreign = MemberDeliveryScope::from_json_value(foreign).expect("the edited scope decodes");

    let refused = retire_at_scope(&handle, &foreign)
        .await
        .expect_err("a scope from another mob incarnation is refused");
    assert!(
        matches!(refused, MobError::IncarnationMismatch { .. }),
        "{refused:?}"
    );
    assert!(roster_has(&handle, &identity).await, "nothing was retired");

    retire_at_scope(&handle, &scope)
        .await
        .expect("a scope from this incarnation retires its member");
    assert!(!roster_has(&handle, &identity).await);
}

/// The scope captured before a respawn never retires the successor; the
/// successor's own scope retires it; once no incarnation is current, both
/// scopes answer `Ok(())` again (retry-safe after a lost reply).
#[tokio::test]
async fn a_scope_retires_its_own_incarnation_and_never_a_successor() {
    let (handle, _service) = turn_driven_mob("k3-successor").await;
    let (identity, _) = spawn_member(&handle, "k3-successor-member").await;
    let first = scope_of(&handle, &identity).await;
    handle
        .respawn(identity.clone(), None)
        .await
        .expect("respawn the member");
    let successor = scope_of(&handle, &identity).await;
    assert_ne!(first.runtime_id(), successor.runtime_id());

    let error = retire_at_scope(&handle, &first)
        .await
        .expect_err("an older scope must not retire the successor");
    assert_stale_fence(&error, &first, &successor);
    assert!(roster_has(&handle, &identity).await);
    assert_eq!(scope_of(&handle, &identity).await, successor);

    retire_at_scope(&handle, &successor)
        .await
        .expect("the current scope retires its incarnation");
    assert!(!roster_has(&handle, &identity).await);

    retire_at_scope(&handle, &successor)
        .await
        .expect("a retry after the retirement settled answers Ok");
    retire_at_scope(&handle, &first)
        .await
        .expect("an older scope answers Ok once no incarnation is current");
}

/// A scope whose session no longer holds the member's binding is refused,
/// and the member stays.
#[tokio::test]
async fn a_moved_session_binding_refuses_the_saved_scope() {
    let (handle, _service) = turn_driven_mob("k3-moved-session").await;
    let (identity, _) = spawn_member(&handle, "k3-moved-session-member").await;
    let scope = scope_of(&handle, &identity).await;
    let moved = SessionId::new();
    handle
        .rebind_member_session_for_test(&identity, moved.clone())
        .await
        .expect("move the member's session binding");

    let error = retire_at_scope(&handle, &scope)
        .await
        .expect_err("a scope for a moved session binding must not retire");
    assert!(
        matches!(
            &error,
            MobError::StaleDeliveryScope {
                agent_identity,
                expected_session,
                actual_session,
            } if agent_identity == &identity
                && expected_session == scope.session_id()
                && actual_session.as_ref() == Some(&moved)
        ),
        "the refusal names both sessions: {error:?}"
    );
    assert!(roster_has(&handle, &identity).await);
}

#[tokio::test]
async fn a_stopped_mob_retires_the_scoped_incarnation() {
    let (handle, _service) = turn_driven_mob("k3-stopped").await;
    let (identity, _) = spawn_member(&handle, "k3-stopped-member").await;
    let scope = scope_of(&handle, &identity).await;
    handle.stop().await.expect("stop the mob");

    retire_at_scope(&handle, &scope)
        .await
        .expect("a stopped mob retires its current incarnation");
    assert!(!roster_has(&handle, &identity).await);
}

#[tokio::test]
async fn a_completed_mob_refuses_a_scoped_retire() {
    let (handle, _service) = turn_driven_mob("k3-completed").await;
    let (identity, _) = spawn_member(&handle, "k3-completed-member").await;
    let scope = scope_of(&handle, &identity).await;
    handle.complete().await.expect("complete the mob");

    let error = retire_at_scope(&handle, &scope)
        .await
        .expect_err("a completed mob retires nothing");
    assert!(
        matches!(
            error,
            MobError::InvalidTransition {
                from: MobState::Completed,
                to: MobState::Running,
            }
        ),
        "a completed mob answers as a scoped submit does: {error:?}"
    );
}

/// While the mob still owns the identity's retirement, a scoped retire of
/// the same incarnation waits for it, past the archive observation that
/// unmaps the identity, and answers `Ok(())` only at the barrier. It never
/// answers `LifecycleOperationPending`.
#[tokio::test]
async fn a_scoped_retire_waits_out_an_owned_retirement_until_its_barrier() {
    let (handle, _service) = turn_driven_mob("k3-owned").await;
    let (identity, _) = spawn_member(&handle, "k3-owned-member").await;
    let scope = scope_of(&handle, &identity).await;
    let hold = super::actor::retirement_stage_hold::install(&identity, "retire-projection-cleanup");
    let unscoped = {
        let handle = handle.clone();
        let identity = identity.clone();
        tokio::spawn(async move { handle.retire(identity).await })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the owned retirement reaches projection cleanup");
    assert!(
        !machine_state(&handle)
            .identity_to_runtime
            .contains_key(&mob_dsl::AgentIdentity::from_domain(&identity)),
        "the archive observation already unmapped the identity"
    );
    assert!(
        roster_has(&handle, &identity).await,
        "the roster anchor goes last"
    );

    let mut scoped = spawn_retire_at_scope(&handle, &scope);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut scoped)
            .await
            .is_err(),
        "the scoped retire must not answer before the owned retirement's barrier"
    );
    hold.release();
    unscoped
        .await
        .expect("join the unscoped retire")
        .expect("the owned retirement settles");
    tokio::time::timeout(STEP, scoped)
        .await
        .expect("the scoped retire answers once the retirement settled")
        .expect("join the scoped retire")
        .expect("the scoped retire answers Ok at the barrier");
    assert!(!roster_has(&handle, &identity).await);
}

/// A retirement stuck after its archive observation leaves no incarnation
/// current but keeps the roster anchor: the scoped retire answers the stuck
/// retirement, never `Ok(())`.
#[tokio::test]
async fn a_retirement_stuck_after_its_archive_observation_answers_stuck() {
    let (handle, _service) = turn_driven_mob("k3-stuck-archived").await;
    let (identity, _) = spawn_member(&handle, "k3-stuck-archived-member").await;
    let scope = scope_of(&handle, &identity).await;
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
        !machine_state(&handle)
            .identity_to_runtime
            .contains_key(&mob_dsl::AgentIdentity::from_domain(&identity)),
        "the retirement got stuck after its archive observation"
    );

    let error = retire_at_scope(&handle, &scope)
        .await
        .expect_err("a stuck retirement is not the barrier");
    assert!(
        matches!(error, MobError::MemberRetirementStuck { .. }),
        "the scoped retire answers the stuck retirement: {error:?}"
    );
    assert!(roster_has(&handle, &identity).await);
}

/// A retirement stuck after its Retire input moved the session binding into
/// the retirement: the incarnation is still current for the session that
/// retirement is releasing.
#[tokio::test]
async fn a_retirement_stuck_after_its_retire_input_stays_current_for_its_session() {
    let (handle, _service) = turn_driven_mob("k3-stuck-releasing").await;
    let (identity, session_id) = spawn_member(&handle, "k3-stuck-releasing-member").await;
    let scope = scope_of(&handle, &identity).await;
    super::actor::fail_session_ingress_detach_for_test(&session_id);
    let error = tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("retire answers")
        .expect_err("a failing ingress detach leaves the retirement stuck");
    assert!(
        matches!(error, MobError::MemberRetirementStuck { .. }),
        "{error:?}"
    );
    let state = machine_state(&handle);
    let dsl_identity = mob_dsl::AgentIdentity::from_domain(&identity);
    let dsl_runtime = mob_dsl::AgentRuntimeId::from_domain(scope.runtime_id());
    assert!(
        !state.member_session_bindings.contains_key(&dsl_identity),
        "the Retire input moved the binding into the retirement"
    );
    assert_eq!(
        state.runtime_retire_pending_sessions.get(&dsl_runtime),
        Some(&mob_dsl::SessionId::from_domain(&session_id))
    );

    let error = retire_at_scope(&handle, &scope)
        .await
        .expect_err("a stuck retirement is not the barrier");
    assert!(
        matches!(error, MobError::MemberRetirementStuck { .. }),
        "the releasing incarnation is current for its own session: {error:?}"
    );
}

/// A retirement stuck after `ObserveRuntimeRetired` dropped the runtime's
/// fence: the per-identity fence still names the incarnation.
#[tokio::test]
async fn a_retirement_stuck_after_its_runtime_retired_stays_current_for_its_fence() {
    let (handle, service) = turn_driven_mob("k3-stuck-runtime-retired").await;
    let (identity, _) = spawn_member(&handle, "k3-stuck-runtime-retired-member").await;
    let scope = scope_of(&handle, &identity).await;
    service.set_fail_archive(true);
    let error = tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("retire answers")
        .expect_err("a failing archive leaves the retirement stuck");
    assert!(
        matches!(error, MobError::MemberRetirementStuck { .. }),
        "{error:?}"
    );
    let state = machine_state(&handle);
    let dsl_runtime = mob_dsl::AgentRuntimeId::from_domain(scope.runtime_id());
    assert!(
        !state.runtime_fence_tokens.contains_key(&dsl_runtime),
        "ObserveRuntimeRetired dropped the runtime's fence"
    );
    assert_eq!(
        state
            .identity_to_runtime
            .get(&mob_dsl::AgentIdentity::from_domain(&identity)),
        Some(&dsl_runtime)
    );

    let error = retire_at_scope(&handle, &scope)
        .await
        .expect_err("a stuck retirement is not the barrier");
    assert!(
        matches!(error, MobError::MemberRetirementStuck { .. }),
        "the incarnation is current through its identity's fence: {error:?}"
    );
}

/// A successor minted while the scoped retire waits before its first
/// descendant step keeps its own tree: the descendant step re-checks the
/// root scope, the cascade stops, and the call refuses.
#[tokio::test]
async fn a_successor_minted_before_the_cascade_keeps_its_tree() {
    let (handle, _service) = turn_driven_mob("k3-cascade-before").await;
    let (root, _) = spawn_member(&handle, "k3-cascade-before-root").await;
    let scope = scope_of(&handle, &root).await;
    let hold = super::handle::retire_at_scope_hold::install(
        &root,
        super::handle::retire_at_scope_hold::BEFORE_DESCENDANTS,
    );
    let call = spawn_retire_at_scope(&handle, &scope);
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the scoped retire waits before its descendants");
    handle
        .respawn(root.clone(), None)
        .await
        .expect("respawn the root while the scoped retire waits");
    let successor = scope_of(&handle, &root).await;
    let child = spawn_child(&handle, "k3-cascade-before-child", &root).await;
    hold.release();

    let error = tokio::time::timeout(STEP, call)
        .await
        .expect("the scoped retire answers")
        .expect("join the scoped retire")
        .expect_err("the root scope is stale");
    assert_stale_fence(&error, &scope, &successor);
    assert!(roster_has(&handle, &root).await, "the successor stays");
    assert!(
        roster_has(&handle, &child).await,
        "the successor's child stays"
    );
}

/// A fresh incarnation minted after the scoped retire retired its root keeps
/// its own tree: the second descendant pass runs only while no incarnation
/// of the root is current.
#[tokio::test]
async fn a_member_minted_after_the_root_retired_keeps_its_tree() {
    let (handle, _service) = turn_driven_mob("k3-cascade-after").await;
    let (root, _) = spawn_member(&handle, "k3-cascade-after-root").await;
    let scope = scope_of(&handle, &root).await;
    let hold = super::handle::retire_at_scope_hold::install(
        &root,
        super::handle::retire_at_scope_hold::AFTER_ROOT,
    );
    let call = spawn_retire_at_scope(&handle, &scope);
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the scoped retire waits after the root's step");
    assert!(!roster_has(&handle, &root).await, "the root retired");
    spawn_member(&handle, "k3-cascade-after-root").await;
    let child = spawn_child(&handle, "k3-cascade-after-child", &root).await;
    hold.release();

    tokio::time::timeout(STEP, call)
        .await
        .expect("the scoped retire answers")
        .expect("join the scoped retire")
        .expect("the root's own incarnation retired");
    assert!(
        roster_has(&handle, &root).await,
        "the fresh incarnation stays"
    );
    assert!(
        roster_has(&handle, &child).await,
        "the fresh incarnation's child stays"
    );
}

/// An older scope never joins the successor's unscoped retirement: it waits
/// for that retirement to settle and then answers for its own incarnation.
#[tokio::test]
async fn an_older_scope_never_joins_the_successors_retirement() {
    let (handle, _service) = turn_driven_mob("k3-no-join").await;
    let (identity, _) = spawn_member(&handle, "k3-no-join-member").await;
    let first = scope_of(&handle, &identity).await;
    handle
        .respawn(identity.clone(), None)
        .await
        .expect("respawn the member");
    let successor = scope_of(&handle, &identity).await;
    let hold = super::actor::retirement_stage_hold::install(&identity, "retire-ingress-detach");
    super::actor::fail_session_ingress_detach_for_test(successor.session_id());
    let unscoped = {
        let handle = handle.clone();
        let identity = identity.clone();
        tokio::spawn(async move { handle.retire(identity).await })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the successor's retirement reaches its ingress detach");
    let scoped = spawn_retire_at_scope(&handle, &first);
    hold.release();

    let unscoped = tokio::time::timeout(STEP, unscoped)
        .await
        .expect("the unscoped retire answers")
        .expect("join the unscoped retire")
        .expect_err("the successor's retirement gets stuck");
    assert!(
        matches!(unscoped, MobError::MemberRetirementStuck { .. }),
        "{unscoped:?}"
    );
    let error = tokio::time::timeout(STEP, scoped)
        .await
        .expect("the scoped retire answers")
        .expect("join the scoped retire")
        .expect_err("an older scope names no current incarnation of its own");
    assert_stale_fence(&error, &first, &successor);
}

/// A member's retirement in progress is counted work: while `retire` has not
/// answered, an idle observation reads not idle, with at least one roster
/// transition and admission open, and an idle close answers busy and leaves
/// admission open. Observing does not change it: a second observation reads
/// the same, and the retirement is still in progress afterwards and ends
/// `Ok(())` once it is released.
///
/// The row asserts no count beyond "at least one", does not compare the two
/// observations, and asserts nothing after the release but the retire's own
/// answer.
#[tokio::test]
async fn a_retirement_in_progress_reads_not_idle_and_observing_it_changes_nothing() {
    let (handle, _service) = turn_driven_mob("idle-retiring").await;
    let (identity, _) = spawn_member(&handle, "idle-retiring-member").await;
    let hold = super::actor::retirement_stage_hold::install(&identity, "retire-projection-cleanup");
    let retire = {
        let handle = handle.clone();
        let identity = identity.clone();
        tokio::spawn(async move { handle.retire(identity).await })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the retirement reaches its held stage");
    assert!(
        !retire.is_finished(),
        "the retire has not answered while its stage is held"
    );

    let first = tokio::time::timeout(STEP, handle.observe_idle())
        .await
        .expect("the first observation answers during the retirement")
        .expect("observe during the retirement");
    assert!(!first.is_idle(), "{first:?}");
    assert!(first.busy.roster_transitions >= 1, "{first:?}");
    assert!(first.admission_closed.is_none(), "{first:?}");

    let second = tokio::time::timeout(STEP, handle.observe_idle())
        .await
        .expect("the second observation answers during the retirement")
        .expect("observe the retirement again");
    assert!(!second.is_idle(), "{second:?}");
    assert!(second.busy.roster_transitions >= 1, "{second:?}");
    assert!(second.admission_closed.is_none(), "{second:?}");

    let close = tokio::time::timeout(STEP, handle.close_if_idle())
        .await
        .expect("the idle close answers during the retirement")
        .expect("an idle close during the retirement");
    assert!(
        matches!(close, crate::CloseIfIdleOutcome::Busy(_)),
        "a retirement in progress keeps the mob busy: {close:?}"
    );
    assert!(
        handle.admission_closed().borrow().is_none(),
        "neither an observation nor a refused idle close closes admission"
    );
    assert!(
        !retire.is_finished(),
        "observing did not end the retirement"
    );

    hold.release();
    tokio::time::timeout(STEP, retire)
        .await
        .expect("the retire answers once its stage is released")
        .expect("join the retire")
        .expect("the retirement ends Ok");
}
