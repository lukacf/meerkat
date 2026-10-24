//! Role-targeted flow runs. One mob holds the reviewers of two issues in one
//! role: `rev-a*` review issue A and `rev-b*` review issue B. A run admitted
//! with role targets dispatches each step only to the bound members of its
//! role; every refusal is typed and comes from the actor's admission, and a
//! bound member lost after admission fails the run with the
//! `InsufficientTargets` class before the step dispatches anything. A keyed
//! admission replays to the run its key names and refuses the key, typed,
//! for other activation params or another binding.

use super::*;
use crate::error::FlowRoleTargetRefusal;
use crate::event::{FlowCancelClass, FlowFailureClass};
use crate::run::FlowRoleTargets;

const REVIEWER: &str = "reviewer";

/// The sample definition plus a `reviewer` profile shaped like `worker`.
fn with_reviewer_role(mut definition: MobDefinition) -> MobDefinition {
    let worker = definition
        .profiles
        .get(&ProfileName::from("worker"))
        .expect("sample worker profile")
        .clone();
    definition
        .profiles
        .insert(ProfileName::from(REVIEWER), worker);
    definition
}

/// One step: the reviewer role reviews.
fn review_definition(policy: CollectionPolicy) -> MobDefinition {
    let mut definition = with_reviewer_role(sample_definition());
    let mut review = flow_step(REVIEWER, "Review the change");
    review.collection_policy = policy;
    review.timeout_ms = Some(10_000);
    let mut steps = IndexMap::new();
    steps.insert(step_id("review"), review);
    definition.flows = BTreeMap::from([(
        flow_id("review"),
        FlowSpec::new(Some("review one change".to_string()), steps, None),
    )]);
    definition
}

/// Two steps: a worker prepares the change, then the reviewer role reviews.
fn prepare_then_review_definition(review_policy: CollectionPolicy) -> MobDefinition {
    let mut definition = with_reviewer_role(sample_definition());
    let mut prepare = flow_step("worker", "Prepare the change");
    prepare.timeout_ms = Some(30_000);
    let mut review = flow_step(REVIEWER, "Review the change");
    review.depends_on = vec![step_id("prepare")];
    review.collection_policy = review_policy;
    review.timeout_ms = Some(10_000);
    let mut steps = IndexMap::new();
    steps.insert(step_id("prepare"), prepare);
    steps.insert(step_id("review"), review);
    definition.flows = BTreeMap::from([(
        flow_id("prepare_review"),
        FlowSpec::new(Some("prepare, then review".to_string()), steps, None),
    )]);
    definition
}

fn bind(roles: &[(&str, &[&str])]) -> FlowRoleTargets {
    FlowRoleTargets::new(roles.iter().map(|(role, members)| {
        (
            *role,
            members
                .iter()
                .map(|member| AgentIdentity::from(*member))
                .collect::<Vec<_>>(),
        )
    }))
    .expect("a well-formed binding")
}

fn names(members: &[&str]) -> BTreeSet<String> {
    members.iter().map(|member| String::from(*member)).collect()
}

async fn spawn_members(handle: &MobHandle, role: &str, members: &[&str]) {
    for member in members {
        handle
            .spawn(ProfileName::from(role), AgentIdentity::from(*member), None)
            .await
            .expect("spawn member");
    }
}

/// The identities one step of one run dispatched a turn to, from the events.
fn dispatched_in(events: &[MobEvent], run_id: &RunId, step: &str) -> BTreeSet<String> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            MobEventKind::StepDispatched {
                run_id: id,
                step_id,
                target,
            } if id == run_id && step_id.as_str() == step => {
                Some(target.identity.as_str().to_string())
            }
            _ => None,
        })
        .collect()
}

async fn dispatched_to(handle: &MobHandle, run_id: &RunId, step: &str) -> BTreeSet<String> {
    let events = handle.events().replay_all().await.expect("replay events");
    dispatched_in(&events, run_id, step)
}

async fn step_completed(handle: &MobHandle, run_id: &RunId, step: &str) -> bool {
    let events = handle.events().replay_all().await.expect("replay events");
    events.iter().any(|event| {
        matches!(
            &event.kind,
            MobEventKind::StepCompleted { run_id: id, step_id }
                if id == run_id && step_id.as_str() == step
        )
    })
}

async fn wait_for_dispatch(handle: &MobHandle, run_id: &RunId, step: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while dispatched_to(handle, run_id, step).await.is_empty() {
        assert!(
            Instant::now() < deadline,
            "step '{step}' of run {run_id} was never dispatched"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn refusal_of(result: Result<RunId, MobError>) -> FlowRoleTargetRefusal {
    match result {
        Err(MobError::FlowRoleTargetsRefused { refusal, .. }) => refusal,
        other => panic!("expected a typed role-targets refusal, got {other:?}"),
    }
}

async fn has_flow_member(handle: &MobHandle) -> bool {
    handle
        .list_all_members()
        .await
        .iter()
        .any(|entry| entry.agent_identity.as_str().starts_with("__flow_"))
}

/// A refused binding leaves nothing behind: no run row, no started flow, no
/// dispatched turn and no provisioned flow member.
async fn assert_refusal_left_nothing(handle: &MobHandle) {
    let runs = handle.list_runs(None).await.expect("list runs");
    assert!(
        runs.is_empty(),
        "a refused binding must record no run, found {}",
        runs.len()
    );
    let events = handle.events().replay_all().await.expect("replay events");
    assert!(
        !events.iter().any(|event| matches!(
            event.kind,
            MobEventKind::FlowStarted { .. } | MobEventKind::StepDispatched { .. }
        )),
        "a refused binding must start no flow and dispatch no turn"
    );
    assert!(
        !has_flow_member(handle).await,
        "a refused binding must provision no flow member"
    );
}

/// Control for the rows below: a role-wide run dispatches the review step to
/// every Active member of the role, the other issue's reviewer included.
/// This is the behaviour a binding narrows.
#[tokio::test]
async fn role_wide_review_step_reaches_every_reviewer_of_the_role() {
    let (handle, _service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, REVIEWER, &["rev-a1", "rev-b1"]).await;

    let run_id = handle
        .run_flow(flow_id("review"), serde_json::json!({}))
        .await
        .expect("run the role-wide flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(10)).await;

    assert_eq!(terminal.status, MobRunStatus::Completed);
    assert!(
        terminal.role_targets.is_none(),
        "a role-wide run records no binding"
    );
    assert_eq!(
        dispatched_to(&handle, &run_id, "review").await,
        names(&["rev-a1", "rev-b1"])
    );
}

/// RED row (revert the bound branch of the flow engine's target selection):
/// a run bound to issue A's reviewers dispatches the review step to exactly
/// them and never to issue B's reviewer of the same role. Per-member
/// attribution is the dispatch and completion target and the aggregate's
/// keys; the step ledger stays system provenance. The binding is recorded on
/// the run that `flow_status` and `list_runs` return.
#[tokio::test]
async fn bound_review_step_dispatches_only_to_the_bound_reviewers() {
    let (handle, _service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, REVIEWER, &["rev-a1", "rev-a2", "rev-b1"]).await;
    let binding = bind(&[(REVIEWER, &["rev-a1", "rev-a2"])]);

    let run_id = handle
        .run_flow_with_role_targets(
            flow_id("review"),
            serde_json::json!({}),
            binding.clone(),
            None,
        )
        .await
        .expect("admit the bound run");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(10)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let issue_a = names(&["rev-a1", "rev-a2"]);
    let events = handle.events().replay_all().await.expect("replay events");
    assert_eq!(
        dispatched_in(&events, &run_id, "review"),
        issue_a,
        "the review step must dispatch to exactly the bound reviewers"
    );
    let completed_by = events
        .iter()
        .filter_map(|event| match &event.kind {
            MobEventKind::StepTargetCompleted {
                run_id: id,
                step_id,
                target,
                ..
            } if id == &run_id && step_id.as_str() == "review" => {
                Some(target.identity.as_str().to_string())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(completed_by, issue_a);

    let aggregate = terminal
        .step_ledger
        .iter()
        .find(|entry| {
            entry.step_id.as_str() == "review" && entry.status == StepRunStatus::Completed
        })
        .and_then(|entry| entry.output.clone())
        .expect("the review aggregate is persisted");
    let producers = aggregate
        .as_object()
        .expect("an all-policy fan-out aggregate is keyed by producer")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(producers, issue_a);
    assert!(
        terminal
            .step_ledger
            .iter()
            .all(|entry| entry.agent_identity == crate::runtime::flow_system_member_id()),
        "the step ledger records system provenance only"
    );

    assert_eq!(terminal.role_targets.as_ref(), Some(&binding));
    let listed = handle
        .list_runs(Some(&flow_id("review")))
        .await
        .expect("list runs");
    assert!(
        listed
            .iter()
            .any(|run| run.run_id == run_id && run.role_targets.as_ref() == Some(&binding)),
        "list_runs returns the recorded binding"
    );
}

/// Two issues' bound runs of one flow, in flight together, each dispatch to
/// their own reviewers only.
#[tokio::test]
async fn concurrent_bound_runs_for_two_issues_never_cross_dispatch() {
    let (handle, service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, REVIEWER, &["rev-a1", "rev-a2", "rev-b1", "rev-b2"]).await;
    service.set_flow_turn_delay_ms(300);

    let issue_a = handle
        .run_flow_with_role_targets(
            flow_id("review"),
            serde_json::json!({"issue": "a"}),
            bind(&[(REVIEWER, &["rev-a1", "rev-a2"])]),
            None,
        )
        .await
        .expect("admit issue A's run");
    let issue_b = handle
        .run_flow_with_role_targets(
            flow_id("review"),
            serde_json::json!({"issue": "b"}),
            bind(&[(REVIEWER, &["rev-b1", "rev-b2"])]),
            None,
        )
        .await
        .expect("admit issue B's run");

    let terminal_a = wait_for_run_terminal(&handle, &issue_a, Duration::from_secs(10)).await;
    let terminal_b = wait_for_run_terminal(&handle, &issue_b, Duration::from_secs(10)).await;
    assert_eq!(terminal_a.status, MobRunStatus::Completed);
    assert_eq!(terminal_b.status, MobRunStatus::Completed);
    assert_eq!(
        dispatched_to(&handle, &issue_a, "review").await,
        names(&["rev-a1", "rev-a2"])
    );
    assert_eq!(
        dispatched_to(&handle, &issue_b, "review").await,
        names(&["rev-b1", "rev-b2"])
    );
}

#[tokio::test]
async fn role_targets_refuse_a_non_member() {
    let (handle, _service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, REVIEWER, &["rev-a1"]).await;

    let refusal = refusal_of(
        handle
            .run_flow_with_role_targets(
                flow_id("review"),
                serde_json::json!({}),
                bind(&[(REVIEWER, &["rev-a1", "rev-ghost"])]),
                None,
            )
            .await,
    );

    assert_eq!(
        refusal,
        FlowRoleTargetRefusal::NotAMember {
            role: ProfileName::from(REVIEWER),
            member: AgentIdentity::from("rev-ghost"),
        }
    );
    assert_refusal_left_nothing(&handle).await;
}

#[tokio::test]
async fn role_targets_refuse_a_member_holding_another_role() {
    let (handle, _service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, "worker", &["coder-a"]).await;
    spawn_members(&handle, REVIEWER, &["rev-a1"]).await;

    let refusal = refusal_of(
        handle
            .run_flow_with_role_targets(
                flow_id("review"),
                serde_json::json!({}),
                bind(&[(REVIEWER, &["rev-a1", "coder-a"])]),
                None,
            )
            .await,
    );

    assert_eq!(
        refusal,
        FlowRoleTargetRefusal::RoleMismatch {
            role: ProfileName::from(REVIEWER),
            member: AgentIdentity::from("coder-a"),
            member_role: ProfileName::from("worker"),
        }
    );
    assert_refusal_left_nothing(&handle).await;
}

#[tokio::test]
async fn role_targets_refuse_a_retired_member() {
    let (handle, _service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, REVIEWER, &["rev-a1", "rev-a2"]).await;
    handle
        .retire(AgentIdentity::from("rev-a2"))
        .await
        .expect("retire rev-a2");

    let refusal = refusal_of(
        handle
            .run_flow_with_role_targets(
                flow_id("review"),
                serde_json::json!({}),
                bind(&[(REVIEWER, &["rev-a1", "rev-a2"])]),
                None,
            )
            .await,
    );

    // A retired member is either gone from the machine's member facts or
    // still recorded with a non-Active lifecycle; both refuse it by name.
    assert!(
        matches!(
            &refusal,
            FlowRoleTargetRefusal::NotAMember { member, .. }
                | FlowRoleTargetRefusal::NotActive { member, .. }
                if member.as_str() == "rev-a2"
        ),
        "a retired member must be refused by name: {refusal:?}"
    );
    assert_refusal_left_nothing(&handle).await;
}

#[tokio::test]
async fn role_targets_refuse_a_role_the_flow_does_not_use() {
    let (handle, _service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, "worker", &["coder-a"]).await;
    spawn_members(&handle, REVIEWER, &["rev-a1"]).await;

    let refusal = refusal_of(
        handle
            .run_flow_with_role_targets(
                flow_id("review"),
                serde_json::json!({}),
                bind(&[(REVIEWER, &["rev-a1"]), ("worker", &["coder-a"])]),
                None,
            )
            .await,
    );

    assert_eq!(
        refusal,
        FlowRoleTargetRefusal::RoleNotInFlow {
            role: ProfileName::from("worker"),
        }
    );
    assert_refusal_left_nothing(&handle).await;
}

#[tokio::test]
async fn role_targets_refuse_an_unbound_flow_role() {
    let (handle, _service) =
        create_test_mob(prepare_then_review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, "worker", &["coder-a"]).await;
    spawn_members(&handle, REVIEWER, &["rev-a1"]).await;

    let refusal = refusal_of(
        handle
            .run_flow_with_role_targets(
                flow_id("prepare_review"),
                serde_json::json!({}),
                bind(&[(REVIEWER, &["rev-a1"])]),
                None,
            )
            .await,
    );

    assert_eq!(
        refusal,
        FlowRoleTargetRefusal::RoleUnbound {
            role: ProfileName::from("worker"),
        }
    );
    assert_refusal_left_nothing(&handle).await;
}

#[tokio::test]
async fn role_targets_refuse_a_quorum_above_the_bound_set() {
    let (handle, _service) =
        create_test_mob(review_definition(CollectionPolicy::Quorum { n: 2 })).await;
    spawn_members(&handle, REVIEWER, &["rev-a1", "rev-a2"]).await;

    let refusal = refusal_of(
        handle
            .run_flow_with_role_targets(
                flow_id("review"),
                serde_json::json!({}),
                bind(&[(REVIEWER, &["rev-a1"])]),
                None,
            )
            .await,
    );

    assert_eq!(
        refusal,
        FlowRoleTargetRefusal::QuorumExceedsBoundSet {
            step_id: step_id("review"),
            required: 2,
            bound: 1,
        }
    );
    assert_refusal_left_nothing(&handle).await;
}

/// RED row (drop the bound-run guard around flow-member provisioning): a
/// role with no runnable member gets a `__flow_<role>_<flow>` member before a
/// role-wide run is admitted, never for a bound run, refused or admitted.
#[tokio::test]
async fn bound_run_provisions_no_flow_member() {
    let (handle, _service) =
        create_test_mob(prepare_then_review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, REVIEWER, &["rev-a1"]).await;

    // No worker member exists yet: a role-wide run would provision one.
    let refusal = refusal_of(
        handle
            .run_flow_with_role_targets(
                flow_id("prepare_review"),
                serde_json::json!({}),
                bind(&[("worker", &["coder-ghost"]), (REVIEWER, &["rev-a1"])]),
                None,
            )
            .await,
    );
    assert_eq!(
        refusal,
        FlowRoleTargetRefusal::NotAMember {
            role: ProfileName::from("worker"),
            member: AgentIdentity::from("coder-ghost"),
        }
    );
    assert_refusal_left_nothing(&handle).await;

    spawn_members(&handle, "worker", &["coder-a"]).await;
    let run_id = handle
        .run_flow_with_role_targets(
            flow_id("prepare_review"),
            serde_json::json!({}),
            bind(&[("worker", &["coder-a"]), (REVIEWER, &["rev-a1"])]),
            None,
        )
        .await
        .expect("admit the bound run");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(10)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);
    assert!(
        !has_flow_member(&handle).await,
        "an admitted bound run must provision no flow member"
    );
}

/// Admit a prepare-then-review run binding three issue-A reviewers with every
/// bound member Active, then retire `rev-a2` while the prepare step's turn is
/// still in flight, so the review step selects its targets after the retire
/// with two bound reviewers left (`rev-a1`, `rev-a3`).
async fn run_with_reviewer_retired_after_admission(
    review_policy: CollectionPolicy,
) -> (MobHandle, RunId, crate::run::MobRun) {
    let (handle, service) = create_test_mob(prepare_then_review_definition(review_policy)).await;
    spawn_members(&handle, "worker", &["coder-a"]).await;
    spawn_members(&handle, REVIEWER, &["rev-a1", "rev-a2", "rev-a3", "rev-b1"]).await;
    service.set_flow_turn_delay_ms(3_000);

    let run_id = handle
        .run_flow_with_role_targets(
            flow_id("prepare_review"),
            serde_json::json!({}),
            bind(&[
                ("worker", &["coder-a"]),
                (REVIEWER, &["rev-a1", "rev-a2", "rev-a3"]),
            ]),
            None,
        )
        .await
        .expect("admit the bound run while every bound member is Active");
    wait_for_dispatch(&handle, &run_id, "prepare").await;
    handle
        .retire(AgentIdentity::from("rev-a2"))
        .await
        .expect("retire a bound reviewer after admission");
    assert!(
        !step_completed(&handle, &run_id, "prepare").await,
        "precondition: the retire must land while the prepare turn is in flight"
    );
    service.set_flow_turn_delay_ms(0);

    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(20)).await;
    (handle, run_id, terminal)
}

/// RED row (revert the bound branch of the flow engine's target selection):
/// a bound member lost after admission leaves an all-policy review step one
/// producer short (three required, two available). The run fails with the
/// `InsufficientTargets` class, naming the lost member, before the review
/// step dispatches any turn; the other issue's reviewer is never substituted
/// in.
#[tokio::test]
async fn bound_member_retired_after_admission_fails_the_run_typed_before_dispatch() {
    let (handle, run_id, terminal) =
        run_with_reviewer_retired_after_admission(CollectionPolicy::All).await;

    assert_eq!(terminal.status, MobRunStatus::Failed);
    let events = handle.events().replay_all().await.expect("replay events");
    let failures = events
        .iter()
        .filter_map(|event| match &event.kind {
            MobEventKind::FlowFailed {
                run_id: id,
                cause,
                reason,
                ..
            } if id == &run_id => Some((*cause, reason.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        matches!(
            failures.as_slice(),
            [(FlowFailureClass::InsufficientTargets, reason)]
                if reason.contains("required 3") && reason.contains("unavailable [rev-a2]")
        ),
        "the shortfall must fail the run once, typed, naming the lost member: {failures:?}"
    );
    assert!(
        dispatched_in(&events, &run_id, "review").is_empty(),
        "the shortfall must fail the run before the review step dispatches any turn"
    );
}

/// A bound quorum step runs on every bound member still Active when it
/// dispatches, not on the first one: after `rev-a2` was retired, the quorum
/// of one fans out to both `rev-a1` and `rev-a3`, and issue B's reviewer is
/// never substituted in.
#[tokio::test]
async fn bound_quorum_step_runs_on_the_remaining_bound_members_without_substitution() {
    let (handle, run_id, terminal) =
        run_with_reviewer_retired_after_admission(CollectionPolicy::Quorum { n: 1 }).await;

    assert_eq!(terminal.status, MobRunStatus::Completed);
    assert_eq!(
        dispatched_to(&handle, &run_id, "review").await,
        names(&["rev-a1", "rev-a3"])
    );
}

/// A bound run in flight when its runtime dies converges on resume like any
/// other in-flight run: Canceled with the custody-lost cause, never
/// re-executed. The recorded binding stays on the run for readers.
#[tokio::test]
async fn bound_run_interrupted_by_restart_cancels_custody_lost_and_keeps_binding() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let incarnation = CrashableMobIncarnation::new();
    let setup_events = events.clone();
    let binding = bind(&[("worker", &["w-a1"])]);
    let admitted = binding.clone();
    let (handle, service, runs, specs, runtime_metadata, identity, identity_status, run_id) =
        incarnation
            .install(async move {
                let (handle, service, runs, specs, runtime_metadata, identity, identity_status) =
                    create_test_mob_with_recoverable_fault_events(
                        sample_definition_with_single_step_flow(60_000, 8),
                        setup_events.clone(),
                    )
                    .await;
                spawn_members(&handle, "worker", &["w-a1", "w-b1"]).await;
                service.set_flow_turn_never_terminal(true);

                let run_id = handle
                    .run_flow_with_role_targets(
                        flow_id("demo"),
                        serde_json::json!({}),
                        admitted,
                        None,
                    )
                    .await
                    .expect("admit the bound run");
                // Crash at one durable point: Running, with the only step
                // dispatched into a turn that never returns.
                let deadline = Instant::now() + Duration::from_secs(8);
                loop {
                    let running = runs
                        .get_run(&run_id)
                        .await
                        .expect("read run row")
                        .is_some_and(|run| run.status == MobRunStatus::Running);
                    let replayed = setup_events
                        .replay_all()
                        .await
                        .expect("replay pre-crash events");
                    if running && !dispatched_in(&replayed, &run_id, "start").is_empty() {
                        break;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "the bound run never reached a dispatched Running step before the crash"
                    );
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                (
                    handle,
                    service,
                    runs,
                    specs,
                    runtime_metadata,
                    identity,
                    identity_status,
                    run_id,
                )
            })
            .await;

    let mob_id = handle.mob_id().clone();
    assert!(
        !handle.command_tx.is_closed(),
        "the executing incarnation must be live before the simulated process death"
    );
    incarnation.crash().await;
    assert!(
        handle.command_tx.is_closed(),
        "the simulated process death must leave no live actor behind"
    );
    drop(handle);
    await_supervisor_route_release(&mob_id).await;

    let resumed = cold_resume_after_terminal_append_fail_stop(
        events.clone(),
        runs,
        specs,
        runtime_metadata,
        identity,
        identity_status,
        service,
    )
    .await;
    let recovered = resumed
        .flow_status(run_id.clone())
        .await
        .expect("query the converged run")
        .expect("the converged run stays queryable");
    assert_eq!(recovered.status, MobRunStatus::Canceled);
    assert_eq!(
        recovered.role_targets.as_ref(),
        Some(&binding),
        "the recorded binding survives the custody-lost convergence"
    );

    let replayed = events.replay_all().await.expect("replay events");
    let cancel_cause = replayed
        .iter()
        .find_map(|event| match &event.kind {
            MobEventKind::FlowCanceled {
                run_id: id, cause, ..
            } if id == &run_id => Some(*cause),
            _ => None,
        })
        .expect("the converged run has a FlowCanceled carrier");
    assert_eq!(cancel_cause, Some(FlowCancelClass::ExecutionCustodyLost));
    let dispatches = replayed
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepDispatched { run_id: id, .. } if id == &run_id
            )
        })
        .count();
    assert_eq!(
        dispatches, 1,
        "the resume must not execute the bound run again"
    );
    assert_eq!(dispatched_in(&replayed, &run_id, "start"), names(&["w-a1"]));
}

/// A caller delivery identity for a keyed admission. The correlation id is
/// fresh on every call: a keyed flow run neither stores nor compares it.
fn delivery_key(key: &str) -> crate::store::MobExternalDeliveryIdentity {
    crate::store::MobExternalDeliveryIdentity::new(key, Uuid::new_v4().to_string())
        .expect("a canonical delivery identity")
}

fn flow_started_count(events: &[MobEvent], run_id: &RunId) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::FlowStarted { run_id: id, .. } if id == run_id
            )
        })
        .count()
}

fn step_dispatch_count(events: &[MobEvent], run_id: &RunId) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepDispatched { run_id: id, .. } if id == run_id
            )
        })
        .count()
}

/// A keyed bound admission replayed with the same key, activation params and
/// binding (members in another order, a fresh correlation id) returns the same
/// run, in flight and after it ended, without admitting it again. The run id is
/// the one the key derives.
#[tokio::test]
async fn keyed_bound_replay_with_the_same_key_returns_the_same_run() {
    let (handle, service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, REVIEWER, &["rev-a1", "rev-a2", "rev-b1"]).await;
    service.set_flow_turn_delay_ms(300);
    let params = serde_json::json!({ "candidate": "abc123" });

    let first = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params.clone(),
            bind(&[(REVIEWER, &["rev-a1", "rev-a2"])]),
            &delivery_key("issue-a-review-1"),
        )
        .await
        .expect("admit the keyed bound run");
    assert_eq!(
        first,
        RunId::for_work_execution(handle.mob_id(), &flow_id("review"), "issue-a-review-1"),
        "a keyed run's id is the one its key derives"
    );
    let in_flight = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params.clone(),
            bind(&[(REVIEWER, &["rev-a2", "rev-a1"])]),
            &delivery_key("issue-a-review-1"),
        )
        .await
        .expect("replay while the run is in flight");
    assert_eq!(in_flight, first);

    let terminal = wait_for_run_terminal(&handle, &first, Duration::from_secs(10)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);
    let after_end = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params,
            bind(&[(REVIEWER, &["rev-a1", "rev-a2"])]),
            &delivery_key("issue-a-review-1"),
        )
        .await
        .expect("replay after the run ended");
    assert_eq!(after_end, first);

    assert_eq!(handle.list_runs(None).await.expect("list runs").len(), 1);
    let events = handle.events().replay_all().await.expect("replay events");
    assert_eq!(flow_started_count(&events, &first), 1);
    assert_eq!(
        step_dispatch_count(&events, &first),
        2,
        "each bound reviewer is dispatched once"
    );
    assert_eq!(
        dispatched_in(&events, &first, "review"),
        names(&["rev-a1", "rev-a2"])
    );
}

/// The same key with another binding is the typed conflict: the first run
/// stands, and no second run is admitted.
#[tokio::test]
async fn keyed_bound_run_refuses_a_changed_binding_under_the_same_key() {
    let (handle, _service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, REVIEWER, &["rev-a1", "rev-a2", "rev-b1"]).await;
    let params = serde_json::json!({ "candidate": "abc123" });
    let first = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params.clone(),
            bind(&[(REVIEWER, &["rev-a1", "rev-a2"])]),
            &delivery_key("issue-a-review-1"),
        )
        .await
        .expect("admit the keyed bound run");

    let refused = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params,
            bind(&[(REVIEWER, &["rev-a1", "rev-b1"])]),
            &delivery_key("issue-a-review-1"),
        )
        .await;
    assert!(
        matches!(
            &refused,
            Err(MobError::FlowRunIdempotencyConflict { run_id, flow_id })
                if run_id == &first && flow_id.as_str() == "review"
        ),
        "a changed binding under the same key is the typed conflict: {refused:?}"
    );
    assert_eq!(handle.list_runs(None).await.expect("list runs").len(), 1);
    let events = handle.events().replay_all().await.expect("replay events");
    assert_eq!(flow_started_count(&events, &first), 1);
    let terminal = wait_for_run_terminal(&handle, &first, Duration::from_secs(10)).await;
    assert_eq!(
        terminal.role_targets.as_ref(),
        Some(&bind(&[(REVIEWER, &["rev-a1", "rev-a2"])])),
        "the first binding stands"
    );
}

/// The same key with other activation params is the typed conflict too.
#[tokio::test]
async fn keyed_bound_run_refuses_changed_activation_params_under_the_same_key() {
    let (handle, _service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, REVIEWER, &["rev-a1", "rev-a2"]).await;
    let binding = bind(&[(REVIEWER, &["rev-a1", "rev-a2"])]);
    let first = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            serde_json::json!({ "candidate": "abc123" }),
            binding.clone(),
            &delivery_key("issue-a-review-1"),
        )
        .await
        .expect("admit the keyed bound run");

    let refused = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            serde_json::json!({ "candidate": "def456" }),
            binding,
            &delivery_key("issue-a-review-1"),
        )
        .await;
    assert!(
        matches!(
            &refused,
            Err(MobError::FlowRunIdempotencyConflict { run_id, .. }) if run_id == &first
        ),
        "changed activation params under the same key are the typed conflict: {refused:?}"
    );
    assert_eq!(handle.list_runs(None).await.expect("list runs").len(), 1);
    let events = handle.events().replay_all().await.expect("replay events");
    assert_eq!(flow_started_count(&events, &first), 1);
}

/// A runtime that dies after admitting a keyed bound run, before the caller
/// read the answer: on resume the run is Canceled with the custody-lost cause
/// and keeps its binding, and the caller's replay with the same key returns
/// that run. Nothing runs again and no second run is admitted.
#[tokio::test]
async fn keyed_bound_run_crash_between_admission_and_receipt_returns_the_same_bound_run() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let incarnation = CrashableMobIncarnation::new();
    let setup_events = events.clone();
    let binding = bind(&[("worker", &["w-a1"])]);
    let admitted = binding.clone();
    let params = serde_json::json!({ "candidate": "abc123" });
    let admitted_params = params.clone();
    let (handle, service, runs, specs, runtime_metadata, identity, identity_status) = incarnation
        .install(async move {
            let (handle, service, runs, specs, runtime_metadata, identity, identity_status) =
                create_test_mob_with_recoverable_fault_events(
                    sample_definition_with_single_step_flow(60_000, 8),
                    setup_events.clone(),
                )
                .await;
            spawn_members(&handle, "worker", &["w-a1", "w-b1"]).await;
            service.set_flow_turn_never_terminal(true);
            // The answer is dropped: the caller never reads the run id.
            let _ = handle
                .run_flow_with_role_targets_and_external_identity(
                    flow_id("demo"),
                    admitted_params,
                    admitted,
                    &delivery_key("issue-a-review-1"),
                )
                .await
                .expect("admit the keyed bound run");
            let run_id =
                RunId::for_work_execution(handle.mob_id(), &flow_id("demo"), "issue-a-review-1");
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                let running = runs
                    .get_run(&run_id)
                    .await
                    .expect("read run row")
                    .is_some_and(|run| run.status == MobRunStatus::Running);
                let replayed = setup_events
                    .replay_all()
                    .await
                    .expect("replay pre-crash events");
                if running && !dispatched_in(&replayed, &run_id, "start").is_empty() {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "the keyed bound run never reached a dispatched Running step before the crash"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            (
                handle,
                service,
                runs,
                specs,
                runtime_metadata,
                identity,
                identity_status,
            )
        })
        .await;

    let mob_id = handle.mob_id().clone();
    assert!(
        !handle.command_tx.is_closed(),
        "the executing incarnation must be live before the simulated process death"
    );
    incarnation.crash().await;
    assert!(
        handle.command_tx.is_closed(),
        "the simulated process death must leave no live actor behind"
    );
    drop(handle);
    await_supervisor_route_release(&mob_id).await;

    let resumed = cold_resume_after_terminal_append_fail_stop(
        events.clone(),
        runs,
        specs,
        runtime_metadata,
        identity,
        identity_status,
        service,
    )
    .await;
    let expected = RunId::for_work_execution(&mob_id, &flow_id("demo"), "issue-a-review-1");
    let replayed = resumed
        .run_flow_with_role_targets_and_external_identity(
            flow_id("demo"),
            params,
            binding.clone(),
            &delivery_key("issue-a-review-1"),
        )
        .await
        .expect("the replay after the restart answers with the admitted run");
    assert_eq!(replayed, expected);

    let recovered = resumed
        .flow_status(expected.clone())
        .await
        .expect("query the converged run")
        .expect("the converged run stays queryable");
    assert_eq!(recovered.status, MobRunStatus::Canceled);
    assert_eq!(recovered.role_targets.as_ref(), Some(&binding));
    assert_eq!(resumed.list_runs(None).await.expect("list runs").len(), 1);
    let replayed_events = events.replay_all().await.expect("replay events");
    let cancel_cause = replayed_events
        .iter()
        .find_map(|event| match &event.kind {
            MobEventKind::FlowCanceled {
                run_id: id, cause, ..
            } if id == &expected => Some(*cause),
            _ => None,
        })
        .expect("the converged run has a FlowCanceled carrier");
    assert_eq!(cancel_cause, Some(FlowCancelClass::ExecutionCustodyLost));
    assert_eq!(flow_started_count(&replayed_events, &expected), 1);
    assert_eq!(
        step_dispatch_count(&replayed_events, &expected),
        1,
        "the replay must not run the bound run again"
    );
}

/// Without a key nothing changes: two identical role-targeted admissions are
/// two runs with fresh ids.
#[tokio::test]
async fn unkeyed_bound_runs_keep_fresh_run_ids() {
    let (handle, _service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, REVIEWER, &["rev-a1", "rev-a2"]).await;
    let mut run_ids = BTreeSet::new();
    for _ in 0..2 {
        let run_id = handle
            .run_flow_with_role_targets(
                flow_id("review"),
                serde_json::json!({ "candidate": "abc123" }),
                bind(&[(REVIEWER, &["rev-a1", "rev-a2"])]),
                None,
            )
            .await
            .expect("admit an unkeyed bound run");
        run_ids.insert(run_id.to_string());
    }
    assert_eq!(run_ids.len(), 2, "each unkeyed admission is its own run");
    assert_eq!(handle.list_runs(None).await.expect("list runs").len(), 2);
}

/// A replay is not admitted again: a bound member retired after the first
/// admission does not refuse a replay, which returns the existing run.
#[tokio::test]
async fn keyed_bound_replay_does_not_readmit_after_a_membership_change() {
    let (handle, _service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, REVIEWER, &["rev-a1", "rev-a2"]).await;
    let params = serde_json::json!({ "candidate": "abc123" });
    let binding = bind(&[(REVIEWER, &["rev-a1", "rev-a2"])]);
    let first = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params.clone(),
            binding.clone(),
            &delivery_key("issue-a-review-1"),
        )
        .await
        .expect("admit the keyed bound run");
    let terminal = wait_for_run_terminal(&handle, &first, Duration::from_secs(10)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);
    handle
        .retire(AgentIdentity::from("rev-a2"))
        .await
        .expect("retire a bound reviewer after the run");

    let replay = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params,
            binding,
            &delivery_key("issue-a-review-1"),
        )
        .await
        .expect("a replay returns the existing run instead of admitting again");
    assert_eq!(replay, first);
    assert_eq!(handle.list_runs(None).await.expect("list runs").len(), 1);
}

/// The existing keyed role-wide admission gets the same typed conflict: the
/// same key with other activation params is no longer an internal error.
#[tokio::test]
async fn external_identity_conflict_is_typed() {
    let (handle, _service) = create_test_mob(review_definition(CollectionPolicy::All)).await;
    spawn_members(&handle, REVIEWER, &["rev-a1"]).await;
    let first = handle
        .run_flow_with_external_identity(
            flow_id("review"),
            serde_json::json!({ "candidate": "abc123" }),
            &delivery_key("issue-a-review-1"),
        )
        .await
        .expect("admit the keyed role-wide run");

    let refused = handle
        .run_flow_with_external_identity(
            flow_id("review"),
            serde_json::json!({ "candidate": "def456" }),
            &delivery_key("issue-a-review-1"),
        )
        .await;
    assert!(
        matches!(
            &refused,
            Err(MobError::FlowRunIdempotencyConflict { run_id, .. }) if run_id == &first
        ),
        "the keyed role-wide conflict is typed: {refused:?}"
    );
    assert_eq!(handle.list_runs(None).await.expect("list runs").len(), 1);
}
