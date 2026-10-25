//! Flow run observers. A keyed explicit-target run reports to the member its
//! admission (or a later bind of the same key) names: that member reads the
//! run's status and terminal outcome through `mob_flow_status` without
//! manage scope, and nothing else. The binding lives beside the run, never in
//! its params or role targets, so a keyed replay compares exactly what the
//! caller admitted. MobMachine decides each read; the tool surface mirrors it.

use super::*;
use crate::control_policy::{CommandAuthority, MobControlPrincipal};
use crate::error::FlowRunObserverRefusal;
use crate::run::{
    FlowRoleTargets, FlowRunObserver, FlowRunWake, FlowRunWakeStatus, MobRunObserverBinding,
};
use crate::runtime::FlowRunObserverWake;
use crate::store::MobExternalDeliveryIdentity;
use meerkat_core::event::BackgroundJobTerminalStatus;

const REVIEWER: &str = "reviewer";
const COORDINATOR: &str = "coordinator";
const BYSTANDER: &str = "bystander";

/// Mob tools on the worker role, a reviewer role shaped like it, and one
/// review flow over the reviewer role.
fn observer_definition() -> MobDefinition {
    let mut definition = sample_definition_with_mob_tools();
    let worker = definition
        .profiles
        .get(&ProfileName::from("worker"))
        .expect("sample worker profile")
        .clone();
    definition
        .profiles
        .insert(ProfileName::from(REVIEWER), worker);
    let mut review = flow_step(REVIEWER, "Review the change");
    review.timeout_ms = Some(10_000);
    let mut steps = IndexMap::new();
    steps.insert(step_id("review"), review);
    definition.flows = BTreeMap::from([(
        flow_id("review"),
        FlowSpec::new(Some("review one change".to_string()), steps, None),
    )]);
    definition
}

/// A mob with a coordinator and a bystander (workers without manage scope)
/// and two reviewers.
async fn observer_mob() -> (MobHandle, Arc<MockSessionService>) {
    let (handle, service) = create_test_mob(observer_definition()).await;
    spawn_observer_mob_members(&handle).await;
    (handle, service)
}

async fn spawn_observer_mob_members(handle: &MobHandle) {
    for (role, member) in [
        ("worker", COORDINATOR),
        ("worker", BYSTANDER),
        (REVIEWER, "reviewer-1"),
        (REVIEWER, "reviewer-2"),
    ] {
        handle
            .spawn(ProfileName::from(role), AgentIdentity::from(member), None)
            .await
            .expect("spawn member");
    }
}

fn reviewers() -> FlowRoleTargets {
    FlowRoleTargets::new([(
        REVIEWER,
        vec![
            AgentIdentity::from("reviewer-1"),
            AgentIdentity::from("reviewer-2"),
        ],
    )])
    .expect("a well-formed binding")
}

/// A caller delivery identity; the correlation id is fresh on every call.
fn key(value: &str) -> MobExternalDeliveryIdentity {
    MobExternalDeliveryIdentity::new(value, Uuid::new_v4().to_string())
        .expect("a canonical delivery identity")
}

fn observer(member: &str) -> FlowRunObserver {
    FlowRunObserver::new(AgentIdentity::from(member))
}

/// The logical runtime id of `member`'s current session: the runtime that
/// stages the member's work.
async fn current_runtime(handle: &MobHandle, member: &str) -> String {
    let session = handle
        .resolve_bridge_session_id(&AgentIdentity::from(member))
        .await
        .expect("member session");
    meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string()
}

/// One turn's retained work as `runtime` stages it; every call names a new
/// run, so two calls are two different snapshots of the same runtime.
fn retained(runtime: &str) -> meerkat_core::retained_work::RetainedWorkIdentity {
    meerkat_core::retained_work::RetainedWorkIdentity::new(
        runtime,
        meerkat_core::lifecycle::RunId::new(),
        Vec::new(),
        BTreeMap::new(),
        None,
    )
}

fn params(change: &str) -> serde_json::Value {
    serde_json::json!({ "change": change, "intent": { "reviewers": 2 } })
}

/// `member`'s operator tools, dispatched from its own session: with manage
/// scope over the mob when `manage`, else with spawn scope only.
async fn member_tools(
    handle: &MobHandle,
    member: &str,
    manage: bool,
) -> Arc<dyn AgentToolDispatcher> {
    let session = handle
        .resolve_bridge_session_id(&AgentIdentity::from(member))
        .await
        .expect("member session");
    dispatched_from_session(operator_tools(handle, manage), session)
}

/// The operator tools with no dispatching session (a context-free call),
/// composed from the worker profile with mob tools on.
fn operator_tools(handle: &MobHandle, manage: bool) -> Arc<dyn AgentToolDispatcher> {
    let mob_id = handle.definition().id.to_string();
    let authority = if manage {
        generated_mob_operator_authority_with_scope(&mob_id, handle.live_incarnation())
    } else {
        generated_mob_operator_authority_with_spawn_profile(
            &mob_id,
            handle.live_incarnation(),
            "worker",
        )
    };
    let mut profile = handle
        .definition()
        .profiles
        .get(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline()
        .expect("inline worker profile")
        .clone();
    profile.tools.mob = true;
    crate::runtime::tools::compose_external_tools_for_profile(
        &profile,
        &BTreeMap::new(),
        handle.clone(),
        None,
        None,
        Some(authority),
    )
    .expect("compose the operator dispatcher")
    .expect("the operator dispatcher is visible")
}

async fn call(
    tools: &Arc<dyn AgentToolDispatcher>,
    name: &'static str,
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let raw = RawValue::from_string(args.to_string()).expect("raw args");
    let outcome = tools
        .dispatch(ToolCallView {
            id: "flow-run-observer-call",
            name,
            args: &raw,
        })
        .await?;
    Ok(serde_json::from_str(&outcome.result.text_content()).expect("a JSON tool result"))
}

async fn status(
    tools: &Arc<dyn AgentToolDispatcher>,
    run_id: &RunId,
) -> Result<serde_json::Value, ToolError> {
    call(
        tools,
        "mob_flow_status",
        serde_json::json!({ "run_id": run_id.to_string() }),
    )
    .await
}

fn denied(result: &Result<serde_json::Value, ToolError>) -> bool {
    matches!(result, Err(ToolError::AccessDenied { .. }))
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

/// A fresh observed admission: the member a keyed explicit-target admission names
/// reads that run's status while it runs and its terminal outcome after it
/// completes, without manage scope, in its own view of the run.
#[tokio::test]
async fn a_freshly_observed_run_is_read_through_to_its_completion() {
    let (handle, service) = observer_mob().await;
    service.set_flow_turn_delay_ms(300);
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-1"),
            reviewers(),
            &key("review-c-1"),
            observer(COORDINATOR),
        )
        .await
        .expect("admit the observed run");
    let coordinator = member_tools(&handle, COORDINATOR, false).await;

    let live = status(&coordinator, &run_id)
        .await
        .expect("the observer reads its run while it runs");
    assert_eq!(live["run"]["run_id"], serde_json::json!(run_id.to_string()));

    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(10)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);
    let read = status(&coordinator, &run_id)
        .await
        .expect("the observer reads the terminal outcome");
    assert_eq!(
        read["run"],
        terminal.observer_view_value().expect("the observer's view"),
        "the observer reads its own view of the terminal run"
    );
    assert_eq!(read["run"]["status"], serde_json::json!("completed"));
    assert_eq!(read["run"]["result_class"], serde_json::json!("success"));
}

/// Across a restart: a bound run in flight when the runtime dies converges
/// on resume (Canceled, custody lost) and its observer still reads it from
/// the resumed mob; the binding is durable beside the run.
#[tokio::test]
async fn a_bound_observer_reads_its_run_canceled_by_a_restart() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let incarnation = CrashableMobIncarnation::new();
    let setup_events = events.clone();
    // No mob tools in the definition: no member session holds a mob handle
    // across the crash. The coordinator's tools are composed after resume.
    let definition = sample_definition_with_single_step_flow(60_000, 8);
    let (handle, service, runs, specs, runtime_metadata, identity, identity_status, run_id) =
        incarnation
            .install(async move {
                let (handle, service, runs, specs, runtime_metadata, identity, identity_status) =
                    create_test_mob_with_recoverable_fault_events(definition, setup_events.clone())
                        .await;
                for member in ["w-coord", "w-a1"] {
                    handle
                        .spawn(
                            ProfileName::from("worker"),
                            AgentIdentity::from(member),
                            None,
                        )
                        .await
                        .expect("spawn member");
                }
                service.set_flow_turn_never_terminal(true);
                let run_id = handle
                    .run_flow_with_role_targets_and_external_identity_observed(
                        flow_id("demo"),
                        serde_json::json!({}),
                        FlowRoleTargets::new([("worker", vec![AgentIdentity::from("w-a1")])])
                            .expect("binding"),
                        &key("restart-run"),
                        observer("w-coord"),
                    )
                    .await
                    .expect("admit the observed run");
                let deadline = Instant::now() + Duration::from_secs(8);
                while !runs
                    .get_run(&run_id)
                    .await
                    .expect("read run row")
                    .is_some_and(|run| run.status == MobRunStatus::Running)
                {
                    assert!(Instant::now() < deadline, "the run never started");
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
    incarnation.crash().await;
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
    let coordinator = member_tools(&resumed, "w-coord", false).await;
    let read = status(&coordinator, &run_id)
        .await
        .expect("the observer reads its run after the restart");
    assert_eq!(
        read["run"],
        recovered
            .observer_view_value()
            .expect("the observer's view")
    );
    assert_eq!(read["run"]["result_class"], serde_json::json!("error"));

    // Canceled by recovery: the host's re-link wakes the observer of
    // the run the previous process never finished, once.
    let wakes = Arc::new(LedgerWakes::default());
    resumed.install_flow_run_observer_wake(wakes.clone());
    assert_eq!(
        resumed
            .resubmit_flow_run_observer_wakes()
            .await
            .expect("re-link"),
        1
    );
    assert_eq!(
        wakes.keys(),
        vec![format!("mob_flow_run:{run_id}")],
        "one wake for the recovered run"
    );
    assert_eq!(
        wakes
            .entry(&format!("mob_flow_run:{run_id}"))
            .map(|(member, status, _)| (member, status)),
        Some((
            "w-coord".to_string(),
            BackgroundJobTerminalStatus::Cancelled
        ))
    );
}

/// A member the run does not report to stays denied, for the run while
/// it runs, after it ends, and for a run id that names no run; so does a
/// context-free call without manage scope.
#[tokio::test]
async fn a_member_the_run_does_not_report_to_stays_denied() {
    let (handle, service) = observer_mob().await;
    service.set_flow_turn_delay_ms(300);
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-2"),
            reviewers(),
            &key("review-c-2"),
            observer(COORDINATOR),
        )
        .await
        .expect("admit the observed run");
    let bystander = member_tools(&handle, BYSTANDER, false).await;
    let reviewer = member_tools(&handle, "reviewer-1", false).await;
    let context_free = operator_tools(&handle, false);

    assert!(denied(&status(&bystander, &run_id).await));
    assert!(
        denied(&status(&reviewer, &run_id).await),
        "a role target is not the observer"
    );
    assert!(denied(&status(&context_free, &run_id).await));
    wait_for_run_terminal(&handle, &run_id, Duration::from_secs(10)).await;
    assert!(denied(&status(&bystander, &run_id).await));
    assert!(denied(&status(&context_free, &run_id).await));
    let absent = RunId::new();
    assert!(
        denied(&status(&bystander, &absent).await),
        "no run reads the same as a run the caller does not observe"
    );
    let coordinator = member_tools(&handle, COORDINATOR, false).await;
    assert!(
        denied(&status(&coordinator, &absent).await),
        "an observer of one run reads no other"
    );
}

/// A manager is admitted on manage scope exactly as before, whether or not
/// the run has an observer and whether or not it exists.
#[tokio::test]
async fn a_manager_reads_every_run_as_before() {
    let (handle, _service) = observer_mob().await;
    let observed = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-3"),
            reviewers(),
            &key("review-c-3"),
            observer(COORDINATOR),
        )
        .await
        .expect("admit the observed run");
    let unobserved = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-4"),
            reviewers(),
            &key("review-c-4"),
        )
        .await
        .expect("admit an unobserved run");
    let observed_run = wait_for_run_terminal(&handle, &observed, Duration::from_secs(10)).await;
    let unobserved_run = wait_for_run_terminal(&handle, &unobserved, Duration::from_secs(10)).await;

    for manager in [
        operator_tools(&handle, true),
        member_tools(&handle, BYSTANDER, true).await,
    ] {
        assert_eq!(
            status(&manager, &observed).await.expect("observed run")["run"],
            serde_json::to_value(&observed_run).expect("encode")
        );
        assert_eq!(
            status(&manager, &unobserved).await.expect("unobserved run")["run"],
            serde_json::to_value(&unobserved_run).expect("encode")
        );
        assert_eq!(
            status(&manager, &RunId::new()).await.expect("absent run"),
            serde_json::json!({ "run": null }),
            "a manager still reads an absent run as null"
        );
    }
}

/// Observing a run grants nothing else: the observer can neither cancel
/// it, nor start a run, nor list the mob's flows.
#[tokio::test]
async fn the_observer_can_neither_cancel_nor_start_runs() {
    let (handle, service) = observer_mob().await;
    service.set_flow_turn_delay_ms(300);
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-5"),
            reviewers(),
            &key("review-c-5"),
            observer(COORDINATOR),
        )
        .await
        .expect("admit the observed run");
    let coordinator = member_tools(&handle, COORDINATOR, false).await;
    assert!(denied(
        &call(
            &coordinator,
            "mob_cancel_flow",
            serde_json::json!({ "run_id": run_id.to_string() })
        )
        .await
    ));
    assert!(denied(
        &call(
            &coordinator,
            "mob_run_flow",
            serde_json::json!({ "flow_id": "review", "params": {} })
        )
        .await
    ));
    assert!(denied(
        &call(&coordinator, "mob_list_flows", serde_json::json!({})).await
    ));
    assert_eq!(
        wait_for_run_terminal(&handle, &run_id, Duration::from_secs(10))
            .await
            .status,
        MobRunStatus::Completed,
        "the denied cancel left the run running to its end"
    );
    assert_eq!(handle.list_runs(None).await.expect("list runs").len(), 1);
}

/// The observer is not part of the params or targets a keyed replay
/// compares. The run's params are byte-identical to an unobserved run's; a
/// replay with the same observer, or with none, returns the same run without
/// running it again.
#[tokio::test]
async fn an_observed_admission_keeps_the_params_and_replays_without_conflict() {
    let (handle, _service) = observer_mob().await;
    let observed = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-6"),
            reviewers(),
            &key("review-c-6"),
            observer(COORDINATOR),
        )
        .await
        .expect("admit the observed run");
    let unobserved = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-6"),
            reviewers(),
            &key("review-c-6-unobserved"),
        )
        .await
        .expect("admit the same params unobserved");
    let observed_run = wait_for_run_terminal(&handle, &observed, Duration::from_secs(10)).await;
    let unobserved_run = wait_for_run_terminal(&handle, &unobserved, Duration::from_secs(10)).await;
    assert_eq!(
        serde_json::to_vec(&observed_run.activation_params).expect("encode"),
        serde_json::to_vec(&params("c-6")).expect("encode"),
        "the admitted params are stored exactly as given"
    );
    assert_eq!(
        serde_json::to_vec(&observed_run.activation_params).expect("encode"),
        serde_json::to_vec(&unobserved_run.activation_params).expect("encode")
    );
    assert_eq!(observed_run.role_targets, unobserved_run.role_targets);

    let before = handle.events().replay_all().await.expect("replay events");
    let replayed = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-6"),
            reviewers(),
            &key("review-c-6"),
            observer(COORDINATOR),
        )
        .await
        .expect("replay with the same observer");
    assert_eq!(replayed, observed);
    let without_observer = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-6"),
            reviewers(),
            &key("review-c-6"),
        )
        .await
        .expect("replay without an observer");
    assert_eq!(without_observer, observed);
    let after = handle.events().replay_all().await.expect("replay events");
    assert_eq!(flow_started_count(&after, &observed), 1);
    assert_eq!(
        step_dispatch_count(&after, &observed),
        step_dispatch_count(&before, &observed),
        "a replay runs nothing again"
    );
}

/// A replay with an observer binds it to a run first admitted without
/// one: no conflict, no second run.
#[tokio::test]
async fn a_replay_binds_an_observer_to_a_run_admitted_without_one() {
    let (handle, _service) = observer_mob().await;
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-7"),
            reviewers(),
            &key("review-c-7"),
        )
        .await
        .expect("admit without an observer");
    let coordinator = member_tools(&handle, COORDINATOR, false).await;
    assert!(denied(&status(&coordinator, &run_id).await));

    let replayed = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-7"),
            reviewers(),
            &key("review-c-7"),
            observer(COORDINATOR),
        )
        .await
        .expect("the replay binds the observer");
    assert_eq!(replayed, run_id);
    status(&coordinator, &run_id)
        .await
        .expect("the bound observer reads the run");
    let events = handle.events().replay_all().await.expect("replay events");
    assert_eq!(flow_started_count(&events, &run_id), 1);
    assert_eq!(handle.list_runs(None).await.expect("list runs").len(), 1);
}

/// Another observer under the same key is refused, typed, and the
/// first binding stands.
#[tokio::test]
async fn another_observer_under_the_same_key_is_refused_typed() {
    let (handle, _service) = observer_mob().await;
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-8"),
            reviewers(),
            &key("review-c-8"),
            observer(COORDINATOR),
        )
        .await
        .expect("admit the observed run");
    let refused = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-8"),
            reviewers(),
            &key("review-c-8"),
            observer(BYSTANDER),
        )
        .await;
    assert!(
        matches!(
            &refused,
            Err(MobError::FlowRunObserverConflict { run_id: id, bound })
                if id == &run_id && bound.as_str() == COORDINATOR
        ),
        "another observer is the typed conflict: {refused:?}"
    );
    status(&member_tools(&handle, COORDINATOR, false).await, &run_id)
        .await
        .expect("the first observer still reads");
    assert!(denied(
        &status(&member_tools(&handle, BYSTANDER, false).await, &run_id).await
    ));
}

/// A non-member observer is refused before anything is written: no run,
/// no started flow, no binding.
#[tokio::test]
async fn a_non_member_observer_is_refused_before_anything_is_written() {
    let (handle, _service) = observer_mob().await;
    let refused = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-9"),
            reviewers(),
            &key("review-c-9"),
            observer("not-a-member"),
        )
        .await;
    assert!(
        matches!(&refused, Err(MobError::MemberNotFound(member)) if member.as_str() == "not-a-member"),
        "{refused:?}"
    );
    assert!(handle.list_runs(None).await.expect("list runs").is_empty());
    let events = handle.events().replay_all().await.expect("replay events");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::FlowStarted { .. }))
    );
    let run_id = RunId::for_work_execution(handle.mob_id(), &flow_id("review"), "review-c-9");
    assert_eq!(
        handle
            .run_store
            .get_run_observer(&run_id)
            .await
            .expect("read the binding"),
        None
    );
}

/// Late bind: runs admitted through the API without an observer
/// (one already completed, one still running) are bound later by their keys.
/// The coordinator then reads both, and each run's id, params and role
/// targets are byte-identical before and after the bind; the completed run's
/// whole row is unchanged, and nothing starts again.
#[tokio::test]
async fn a_run_admitted_without_an_observer_is_bound_late_and_stays_byte_identical() {
    let (handle, service) = observer_mob().await;
    let ended = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-10"),
            reviewers(),
            &key("review-c-10"),
        )
        .await
        .expect("admit a run that ends first");
    let ended_before = wait_for_run_terminal(&handle, &ended, Duration::from_secs(10)).await;
    service.set_flow_turn_delay_ms(300);
    let live = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-11"),
            reviewers(),
            &key("review-c-11"),
        )
        .await
        .expect("admit a run that is still running");
    let live_before = handle
        .flow_status(live.clone())
        .await
        .expect("read the live run")
        .expect("the live run");
    let coordinator = member_tools(&handle, COORDINATOR, false).await;
    assert!(denied(&status(&coordinator, &live).await));
    assert!(denied(&status(&coordinator, &ended).await));

    let bound_live = handle
        .bind_flow_run_observer(
            flow_id("review"),
            &key("review-c-11"),
            observer(COORDINATOR),
        )
        .await
        .expect("bind the live run");
    assert_eq!(bound_live.run_id, live);
    assert_eq!(bound_live.observer, observer(COORDINATOR));
    status(&coordinator, &live)
        .await
        .expect("the observer reads the live run");
    let bound_ended = handle
        .bind_flow_run_observer(
            flow_id("review"),
            &key("review-c-10"),
            observer(COORDINATOR),
        )
        .await
        .expect("bind the ended run");
    assert_eq!(bound_ended.run_id, ended);
    let read = status(&coordinator, &ended)
        .await
        .expect("the observer reads the ended run");
    assert_eq!(
        read["run"],
        ended_before
            .observer_view_value()
            .expect("the observer's view"),
        "the observer reads the run as it ended"
    );
    let ended_after = handle
        .flow_status(ended.clone())
        .await
        .expect("read the ended run")
        .expect("the ended run");
    assert_eq!(
        serde_json::to_vec(&ended_after).expect("encode"),
        serde_json::to_vec(&ended_before).expect("encode"),
        "the bind left the completed run's row byte-identical"
    );

    let live_after = wait_for_run_terminal(&handle, &live, Duration::from_secs(10)).await;
    assert_eq!(live_after.run_id, live_before.run_id);
    assert_eq!(
        serde_json::to_vec(&live_after.activation_params).expect("encode"),
        serde_json::to_vec(&live_before.activation_params).expect("encode")
    );
    assert_eq!(
        serde_json::to_vec(&live_after.role_targets).expect("encode"),
        serde_json::to_vec(&live_before.role_targets).expect("encode")
    );
    assert_eq!(handle.list_runs(None).await.expect("list runs").len(), 2);
    let events = handle.events().replay_all().await.expect("replay events");
    assert_eq!(flow_started_count(&events, &live), 1);
    assert_eq!(flow_started_count(&events, &ended), 1);
}

/// Binding the same observer again is a no-op; another observer
/// is the typed conflict.
#[tokio::test]
async fn the_bind_route_is_idempotent_and_refuses_another_observer() {
    let (handle, _service) = observer_mob().await;
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-12"),
            reviewers(),
            &key("review-c-12"),
        )
        .await
        .expect("admit the run");
    for _ in 0..2 {
        assert_eq!(
            handle
                .bind_flow_run_observer(
                    flow_id("review"),
                    &key("review-c-12"),
                    observer(COORDINATOR)
                )
                .await
                .expect("bind the observer")
                .run_id,
            run_id
        );
    }
    let refused = handle
        .bind_flow_run_observer(flow_id("review"), &key("review-c-12"), observer(BYSTANDER))
        .await;
    assert!(
        matches!(
            &refused,
            Err(MobError::FlowRunObserverConflict { run_id: id, bound })
                if id == &run_id && bound.as_str() == COORDINATOR
        ),
        "{refused:?}"
    );
}

/// A conflict is decided on the member alone.
/// While the stored retained work still names the member's current runtime,
/// the same member binding again with other retained work (a later turn's),
/// through the bind route or an observed replay, is a no-op: the first
/// binding stands whole, retained work included, and the bind returns it.
#[tokio::test]
async fn the_same_member_with_other_retained_work_keeps_the_first_binding() {
    let (handle, _service) = observer_mob().await;
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-16"),
            reviewers(),
            &key("review-c-16"),
        )
        .await
        .expect("admit the run");
    let runtime = current_runtime(&handle, COORDINATOR).await;
    let first = observer(COORDINATOR).with_retained_work(retained(&runtime));
    let bound = handle
        .bind_flow_run_observer(flow_id("review"), &key("review-c-16"), first.clone())
        .await
        .expect("bind with the first turn's retained work");
    assert_eq!(bound.observer, first);

    let again = handle
        .bind_flow_run_observer(
            flow_id("review"),
            &key("review-c-16"),
            observer(COORDINATOR).with_retained_work(retained(&runtime)),
        )
        .await
        .expect("the same member again is a no-op");
    assert_eq!(again, bound, "the bind returns the first binding, whole");
    let replayed = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-16"),
            reviewers(),
            &key("review-c-16"),
            observer(COORDINATOR).with_retained_work(retained(&runtime)),
        )
        .await
        .expect("an observed replay by the same member is a no-op");
    assert_eq!(replayed, run_id);
    assert_eq!(
        handle
            .run_store
            .get_run_observer(&run_id)
            .await
            .expect("read the binding"),
        Some(bound),
        "nothing rewrote the first binding"
    );
}

/// Retained work must have been staged by the member's current
/// runtime. A snapshot from another runtime (another member's, or one the
/// member no longer runs on) is refused at bind, typed, through the bind
/// route and the observed admission alike, and never stored: no binding is
/// written and the refused admission writes no run. Work from the member's
/// current runtime then binds.
#[tokio::test]
async fn a_retained_work_snapshot_from_another_runtime_is_refused_at_bind() {
    let (handle, _service) = observer_mob().await;
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-17"),
            reviewers(),
            &key("review-c-17"),
        )
        .await
        .expect("admit the run");
    let another_members = current_runtime(&handle, BYSTANDER).await;
    let vanished = meerkat_runtime::LogicalRuntimeId::for_session(&SessionId::new()).to_string();
    for runtime in [another_members.as_str(), vanished.as_str()] {
        let bound = handle
            .bind_flow_run_observer(
                flow_id("review"),
                &key("review-c-17"),
                observer(COORDINATOR).with_retained_work(retained(runtime)),
            )
            .await;
        assert!(
            matches!(
                &bound,
                Err(MobError::FlowRunObserverRefused {
                    run_id: id,
                    refusal: FlowRunObserverRefusal::RetainedWorkElsewhere,
                }) if id == &run_id
            ),
            "{bound:?}"
        );
        let admitted = handle
            .run_flow_with_role_targets_and_external_identity_observed(
                flow_id("review"),
                params("c-18"),
                reviewers(),
                &key("review-c-18"),
                observer(COORDINATOR).with_retained_work(retained(runtime)),
            )
            .await;
        assert!(
            matches!(
                &admitted,
                Err(MobError::FlowRunObserverRefused {
                    refusal: FlowRunObserverRefusal::RetainedWorkElsewhere,
                    ..
                })
            ),
            "{admitted:?}"
        );
    }
    assert_eq!(
        handle
            .run_store
            .get_run_observer(&run_id)
            .await
            .expect("read the binding"),
        None,
        "a refused snapshot is never stored"
    );
    let unadmitted = RunId::for_work_execution(handle.mob_id(), &flow_id("review"), "review-c-18");
    assert_eq!(
        handle
            .run_store
            .get_run_observer(&unadmitted)
            .await
            .expect("read the binding"),
        None
    );
    let runs = handle.list_runs(None).await.expect("list runs");
    assert_eq!(
        runs.iter()
            .map(|run| run.run_id.clone())
            .collect::<Vec<_>>(),
        vec![run_id.clone()],
        "the refused admission wrote no run"
    );
    assert!(denied(
        &status(&member_tools(&handle, COORDINATOR, false).await, &run_id).await
    ));

    let current = observer(COORDINATOR)
        .with_retained_work(retained(&current_runtime(&handle, COORDINATOR).await));
    let bound = handle
        .bind_flow_run_observer(flow_id("review"), &key("review-c-17"), current.clone())
        .await
        .expect("work from the member's current runtime binds");
    assert_eq!(bound.observer, current);
}

/// A respawn or repoint cannot silently lose the binding's
/// retained work. Once the member runs on a new runtime, the stored work is
/// stale: a rebind with the old snapshot is refused (another runtime's work) and changes
/// nothing, a rebind without retained work is a no-op, and a rebind with
/// work from the new runtime replaces only the retained work. The refreshed
/// binding is current again, so a later turn's work is a no-op, another
/// member still conflicts, and the observer reads its run from its new
/// session. After the next respawn the observed replay refreshes the same
/// way. The run itself never changes.
#[tokio::test]
async fn a_respawned_member_rebinds_with_fresh_work_and_the_binding_carries_it() {
    let (handle, _service) = observer_mob().await;
    let member = AgentIdentity::from(COORDINATOR);
    let first_runtime = current_runtime(&handle, COORDINATOR).await;
    let first = observer(COORDINATOR).with_retained_work(retained(&first_runtime));
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-19"),
            reviewers(),
            &key("review-c-19"),
            first.clone(),
        )
        .await
        .expect("admit the observed run");
    let ended = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(10)).await;
    let admitted = handle
        .run_store
        .get_run_observer(&run_id)
        .await
        .expect("read the binding")
        .expect("the binding");
    assert_eq!(admitted.observer, first);

    handle
        .respawn(member.clone(), None)
        .await
        .expect("respawn the coordinator");
    let second_runtime = current_runtime(&handle, COORDINATOR).await;
    assert_ne!(
        second_runtime, first_runtime,
        "the respawn moved the member"
    );

    let stale = handle
        .bind_flow_run_observer(flow_id("review"), &key("review-c-19"), first.clone())
        .await;
    assert!(
        matches!(
            &stale,
            Err(MobError::FlowRunObserverRefused {
                refusal: FlowRunObserverRefusal::RetainedWorkElsewhere,
                ..
            })
        ),
        "{stale:?}"
    );
    let bare = handle
        .bind_flow_run_observer(
            flow_id("review"),
            &key("review-c-19"),
            observer(COORDINATOR),
        )
        .await
        .expect("a rebind without retained work is a no-op");
    assert_eq!(bare, admitted, "nothing replaced the stale work");

    let fresh = observer(COORDINATOR).with_retained_work(retained(&second_runtime));
    let refreshed = handle
        .bind_flow_run_observer(flow_id("review"), &key("review-c-19"), fresh.clone())
        .await
        .expect("a rebind with work from the new runtime");
    assert_eq!(
        refreshed,
        MobRunObserverBinding {
            observer: fresh.clone(),
            ..admitted.clone()
        },
        "only the retained work changed"
    );
    assert_eq!(
        handle
            .run_store
            .get_run_observer(&run_id)
            .await
            .expect("read the binding"),
        Some(refreshed.clone())
    );
    let later = handle
        .bind_flow_run_observer(
            flow_id("review"),
            &key("review-c-19"),
            observer(COORDINATOR).with_retained_work(retained(&second_runtime)),
        )
        .await
        .expect("a later turn's work on the same runtime is a no-op");
    assert_eq!(later, refreshed);
    let other = handle
        .bind_flow_run_observer(
            flow_id("review"),
            &key("review-c-19"),
            observer(BYSTANDER)
                .with_retained_work(retained(&current_runtime(&handle, BYSTANDER).await)),
        )
        .await;
    assert!(
        matches!(
            &other,
            Err(MobError::FlowRunObserverConflict { bound, .. }) if bound.as_str() == COORDINATOR
        ),
        "{other:?}"
    );
    status(&member_tools(&handle, COORDINATOR, false).await, &run_id)
        .await
        .expect("the observer reads its run from its new session");

    handle
        .respawn(member.clone(), None)
        .await
        .expect("respawn the coordinator again");
    let third_runtime = current_runtime(&handle, COORDINATOR).await;
    assert_ne!(
        third_runtime, second_runtime,
        "the respawn moved the member"
    );
    let stale_replay = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-19"),
            reviewers(),
            &key("review-c-19"),
            fresh,
        )
        .await;
    assert!(
        matches!(
            &stale_replay,
            Err(MobError::FlowRunObserverRefused {
                refusal: FlowRunObserverRefusal::RetainedWorkElsewhere,
                ..
            })
        ),
        "{stale_replay:?}"
    );
    let third = observer(COORDINATOR).with_retained_work(retained(&third_runtime));
    let replayed = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-19"),
            reviewers(),
            &key("review-c-19"),
            third.clone(),
        )
        .await
        .expect("an observed replay with work from the new runtime");
    assert_eq!(replayed, run_id);
    assert_eq!(
        handle
            .run_store
            .get_run_observer(&run_id)
            .await
            .expect("read the binding"),
        Some(MobRunObserverBinding {
            observer: third,
            ..admitted
        })
    );

    let after = handle
        .flow_status(run_id.clone())
        .await
        .expect("read the run")
        .expect("the run");
    assert_eq!(
        serde_json::to_vec(&after).expect("encode"),
        serde_json::to_vec(&ended).expect("encode"),
        "the rebinds left the run's row byte-identical"
    );
    assert_eq!(handle.list_runs(None).await.expect("list runs").len(), 1);
    let events = handle.events().replay_all().await.expect("replay events");
    assert_eq!(flow_started_count(&events, &run_id), 1);
}

/// Least privilege: the observer reads its own narrower view of its run
/// (ids, status, timestamps, the result class, the result and the root step
/// outputs) and never the activation params, ledgers, frames, loops, role
/// targets or authority inputs. A manager's view of the same run is the
/// whole record, unchanged.
#[tokio::test]
async fn an_observer_reads_a_narrower_view_than_a_manager() {
    let (handle, _service) = observer_mob().await;
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-20"),
            reviewers(),
            &key("review-c-20"),
            observer(COORDINATOR),
        )
        .await
        .expect("admit the observed run");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(10)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let read = status(&member_tools(&handle, COORDINATOR, false).await, &run_id)
        .await
        .expect("the observer reads its run");
    let view = read["run"].as_object().expect("the observer's view");
    for withheld in [
        "activation_params",
        "step_ledger",
        "failure_ledger",
        "frames",
        "loops",
        "loop_iteration_ledger",
        "loop_iteration_outputs",
        "flow_authority_inputs",
        "role_targets",
        "flow_state",
        "flow_definition_digest",
        "schema_version",
    ] {
        assert!(
            !view.contains_key(withheld),
            "the observer's view withholds {withheld}: {view:?}"
        );
    }
    for field in [
        "run_id",
        "mob_id",
        "flow_id",
        "status",
        "created_at",
        "completed_at",
        "result_class",
    ] {
        assert!(
            view.contains_key(field),
            "the observer's view has {field}: {view:?}"
        );
    }
    assert_eq!(view["run_id"], serde_json::json!(run_id.to_string()));
    assert_eq!(view["result_class"], serde_json::json!("success"));
    assert_eq!(
        view.get("outputs"),
        serde_json::to_value(terminal.public_result_value().expect("envelope"))
            .expect("encode")
            .get("outputs"),
        "the observer reads the root step outputs"
    );

    let manager = status(&operator_tools(&handle, true), &run_id)
        .await
        .expect("the manager reads the run");
    assert_eq!(
        manager["run"],
        serde_json::to_value(&terminal).expect("encode"),
        "a manager reads the whole record, as before"
    );
    assert!(manager["run"].get("flow_authority_inputs").is_some());
    assert!(manager["run"].get("activation_params").is_some());
}

/// An observed admission refused after its observer was admitted (here
/// the run's own row cannot be written) leaves no binding behind, and a
/// later admission under the same key is not pre-bound: it binds the member
/// it names.
#[tokio::test]
async fn a_refused_observed_admission_leaves_no_binding() {
    let run_store = Arc::new(RecordingRunStore::new());
    let (handle, _service) =
        create_test_mob_with_run_store(observer_definition(), run_store.clone()).await;
    spawn_observer_mob_members(&handle).await;
    run_store.fail_next_create_run();
    let refused = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-21"),
            reviewers(),
            &key("review-c-21"),
            observer(COORDINATOR),
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
    let run_id = RunId::for_work_execution(handle.mob_id(), &flow_id("review"), "review-c-21");
    assert!(
        run_store
            .get_run(&run_id)
            .await
            .expect("read the run")
            .is_none(),
        "the refused admission wrote no run"
    );
    assert_eq!(
        run_store
            .get_run_observer(&run_id)
            .await
            .expect("read the binding"),
        None,
        "the refused admission left no binding"
    );

    let admitted = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-21"),
            reviewers(),
            &key("review-c-21"),
            observer(BYSTANDER),
        )
        .await
        .expect("a later admission under the key is not pre-bound");
    assert_eq!(admitted, run_id);
    assert_eq!(
        run_store
            .get_run_observer(&run_id)
            .await
            .expect("read the binding")
            .map(|binding| binding.observer),
        Some(observer(BYSTANDER))
    );
}

/// A binding made without retained work takes the member's current
/// retained work when the same member binds again with it; a later turn's
/// work on that runtime then changes nothing.
#[tokio::test]
async fn a_binding_without_retained_work_takes_the_members_current_work() {
    let (handle, _service) = observer_mob().await;
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-22"),
            reviewers(),
            &key("review-c-22"),
            observer(COORDINATOR),
        )
        .await
        .expect("admit the run with a bare observer");
    let runtime = current_runtime(&handle, COORDINATOR).await;
    let with_work = observer(COORDINATOR).with_retained_work(retained(&runtime));
    let refreshed = handle
        .bind_flow_run_observer(flow_id("review"), &key("review-c-22"), with_work.clone())
        .await
        .expect("rebind with the member's current work");
    assert_eq!(refreshed.run_id, run_id);
    assert_eq!(refreshed.observer, with_work);
    let later = handle
        .bind_flow_run_observer(
            flow_id("review"),
            &key("review-c-22"),
            observer(COORDINATOR).with_retained_work(retained(&runtime)),
        )
        .await
        .expect("a later turn's work on the same runtime is a no-op");
    assert_eq!(later, refreshed);
    assert_eq!(
        handle
            .run_store
            .get_run_observer(&run_id)
            .await
            .expect("read the binding"),
        Some(refreshed)
    );
}

/// A run store that holds no observer bindings (a host
/// store over the storage SPI, for now) refuses every observer typed, before
/// anything is written: the observed admission writes no run, and the bind
/// route leaves the run it names unbound.
#[tokio::test]
async fn a_store_without_observer_bindings_refuses_typed() {
    let run_store = Arc::new(RecordingRunStore::new());
    let (handle, _service) =
        create_test_mob_with_run_store(observer_definition(), run_store.clone()).await;
    spawn_observer_mob_members(&handle).await;
    let unbound = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-23"),
            reviewers(),
            &key("review-c-23"),
        )
        .await
        .expect("admit a run without an observer");
    run_store.hold_no_run_observers();
    let admission = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-24"),
            reviewers(),
            &key("review-c-24"),
            observer(COORDINATOR),
        )
        .await;
    assert!(
        matches!(
            &admission,
            Err(MobError::FlowRunObserverRefused {
                refusal: FlowRunObserverRefusal::PersistenceUnavailable,
                ..
            })
        ),
        "{admission:?}"
    );
    let unadmitted = RunId::for_work_execution(handle.mob_id(), &flow_id("review"), "review-c-24");
    assert!(
        run_store
            .get_run(&unadmitted)
            .await
            .expect("read")
            .is_none()
    );
    let bind = handle
        .bind_flow_run_observer(
            flow_id("review"),
            &key("review-c-23"),
            observer(COORDINATOR),
        )
        .await;
    assert!(
        matches!(
            &bind,
            Err(MobError::FlowRunObserverRefused {
                run_id,
                refusal: FlowRunObserverRefusal::PersistenceUnavailable,
            }) if run_id == &unbound
        ),
        "{bind:?}"
    );
    assert_eq!(
        run_store.get_run_observer(&unbound).await.expect("read"),
        None
    );
}

/// The bind route names only a run admitted under the key, and
/// only a keyed explicit-target run.
#[tokio::test]
async fn the_bind_route_names_only_keyed_explicit_target_runs() {
    let (handle, _service) = observer_mob().await;
    let unknown = handle
        .bind_flow_run_observer(
            flow_id("review"),
            &key("never-admitted"),
            observer(COORDINATOR),
        )
        .await;
    assert!(
        matches!(&unknown, Err(MobError::RunNotFound(_))),
        "{unknown:?}"
    );
    let role_wide = handle
        .run_flow_with_external_identity(flow_id("review"), params("c-13"), &key("role-wide"))
        .await
        .expect("admit a keyed run without role targets");
    let refused = handle
        .bind_flow_run_observer(flow_id("review"), &key("role-wide"), observer(COORDINATOR))
        .await;
    assert!(
        matches!(
            &refused,
            Err(MobError::FlowRunObserverRefused {
                run_id,
                refusal: FlowRunObserverRefusal::NotExplicitTarget,
            }) if run_id == &role_wide
        ),
        "{refused:?}"
    );
    assert!(denied(
        &status(&member_tools(&handle, COORDINATOR, false).await, &role_wide).await
    ));
}

/// The bind route needs the `SendCommand` scope on the principal
/// lane; a principal without it binds nothing.
#[tokio::test]
async fn the_bind_route_needs_the_send_command_scope() {
    let (handle, _service) = observer_mob().await;
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-14"),
            reviewers(),
            &key("review-c-14"),
        )
        .await
        .expect("admit the run");
    let unresolved = handle
        .clone()
        .with_command_authority(CommandAuthority::principal(MobControlPrincipal::Unresolved));
    let refused = unresolved
        .bind_flow_run_observer(
            flow_id("review"),
            &key("review-c-14"),
            observer(COORDINATOR),
        )
        .await;
    assert!(
        matches!(&refused, Err(MobError::ScopeDenied(_))),
        "{refused:?}"
    );
    assert_eq!(
        handle
            .run_store
            .get_run_observer(&run_id)
            .await
            .expect("read the binding"),
        None
    );
    assert!(denied(
        &status(&member_tools(&handle, COORDINATOR, false).await, &run_id).await
    ));
}

/// No member tool binds an observer: the operator surface lists none
/// and never issues the bind.
#[tokio::test]
async fn no_member_tool_binds_an_observer() {
    let (handle, _service) = observer_mob().await;
    let names: Vec<String> = operator_tools(&handle, true)
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert!(
        names.iter().all(|name| !name.contains("observer")),
        "{names:?}"
    );
    let surface = include_str!("../tools.rs");
    assert!(!surface.contains("BindFlowRunObserver"));
    assert!(!surface.contains("bind_flow_run_observer"));
}

/// The runtime's governed work host: it admits no governed input, which
/// these rows never submit.
struct AdmitsNothing;

impl meerkat_runtime::input_authority::NativeWorkAuthorizationHost for AdmitsNothing {
    fn authenticate_association(
        &self,
        _runtime_id: &meerkat_runtime::LogicalRuntimeId,
        _input: &meerkat_runtime::input::Input,
        _ingress: &meerkat_runtime::input_authority::NativeIngressContext,
        _association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    ) -> Result<(), meerkat_runtime::input_authority::NativeAdmissionError> {
        Err(meerkat_core::OperationRefused::new(meerkat_core::OperationRefusalKind::Denied).into())
    }

    fn work_context(
        &self,
        _batch: &meerkat_runtime::input_authority::NativeWorkBatch,
    ) -> Result<
        meerkat_core::authorization::WorkAuthorizationContext,
        meerkat_runtime::input_authority::NativeWorkContextError,
    > {
        Err(meerkat_runtime::input_authority::NativeWorkContextError::MalformedAcceptedWork)
    }
}

/// On a runtime with a native work authorization host an observer
/// without the member's retained work is refused, typed, before anything is
/// written; with it the same admission binds.
#[tokio::test]
async fn a_governed_runtime_refuses_an_observer_without_retained_work() {
    let service = Arc::new(MockSessionService::new());
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::ephemeral()
            .with_native_work_authorization_host(Arc::new(AdmitsNothing))
            .expect("install the governed work host"),
    );
    service.set_runtime_adapter(machine);
    let handle = MobBuilder::new(observer_definition(), MobStorage::in_memory())
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    // Turn-driven: an autonomous member's start prompt is governed input,
    // which this host refuses.
    for (role, member) in [
        ("worker", COORDINATOR),
        (REVIEWER, "reviewer-1"),
        (REVIEWER, "reviewer-2"),
    ] {
        let mut spec = SpawnMemberSpec::new(role, member);
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        handle.spawn_spec(spec).await.expect("spawn member");
    }

    let refused = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-15"),
            reviewers(),
            &key("review-c-15"),
            observer(COORDINATOR),
        )
        .await;
    assert!(
        matches!(
            &refused,
            Err(MobError::FlowRunObserverRefused {
                refusal: FlowRunObserverRefusal::RetainedWorkRequired,
                ..
            })
        ),
        "{refused:?}"
    );
    assert!(handle.list_runs(None).await.expect("list runs").is_empty());

    let work = retained(&current_runtime(&handle, COORDINATOR).await);
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-15"),
            reviewers(),
            &key("review-c-15"),
            observer(COORDINATOR).with_retained_work(work.clone()),
        )
        .await
        .expect("an observer with retained work binds");
    let bound = handle
        .run_store
        .get_run_observer(&run_id)
        .await
        .expect("read the binding")
        .expect("the binding");
    assert_eq!(bound.observer.retained_work(), Some(&work));
}

/// In every store a refresh replaces a binding only while it is
/// still exactly the one the caller read, and may change only its retained
/// work (in SQLite, across a reopen).
#[tokio::test]
async fn run_observer_refresh_is_a_compare_and_swap_in_every_store() {
    async fn assert_refresh(store: &dyn MobRunStore, run_id: &RunId) {
        let first = MobRunObserverBinding {
            run_id: run_id.clone(),
            mob_id: MobId::from("observer-store-mob"),
            flow_id: flow_id("review"),
            observer: observer(COORDINATOR).with_retained_work(retained("rt:session:first")),
        };
        let next = MobRunObserverBinding {
            observer: observer(COORDINATOR).with_retained_work(retained("rt:session:next")),
            ..first.clone()
        };
        assert!(
            !store
                .refresh_run_observer(&first, &next)
                .await
                .expect("refresh an unbound run"),
            "nothing is bound to refresh"
        );
        assert_eq!(store.get_run_observer(run_id).await.expect("read"), None);
        assert!(store.put_run_observer(&first).await.expect("bind"));
        let another_member = MobRunObserverBinding {
            observer: observer(BYSTANDER).with_retained_work(retained("rt:session:next")),
            ..first.clone()
        };
        assert!(
            matches!(
                store.refresh_run_observer(&first, &another_member).await,
                Err(MobStoreError::Internal(_))
            ),
            "a refresh may not change the member"
        );
        assert!(
            store
                .refresh_run_observer(&first, &next)
                .await
                .expect("refresh")
        );
        assert_eq!(
            store.get_run_observer(run_id).await.expect("read"),
            Some(next.clone())
        );
        assert!(
            !store
                .refresh_run_observer(&first, &next)
                .await
                .expect("refresh from a stale read"),
            "the binding changed since it was read"
        );
        assert_eq!(
            store.get_run_observer(run_id).await.expect("read"),
            Some(next)
        );
    }

    let run_id = RunId::new();
    assert_refresh(&InMemoryMobRunStore::new(), &run_id).await;
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("observer-refresh.db");
    let stores = SqliteMobStores::open(&path).expect("open sqlite stores");
    assert_refresh(&stores.run_store(), &run_id).await;
    drop(stores);
    let reopened = SqliteMobStores::open(&path).expect("reopen sqlite stores");
    assert_eq!(
        reopened
            .run_store()
            .get_run_observer(&run_id)
            .await
            .expect("read after reopen")
            .and_then(|binding| {
                binding
                    .observer
                    .retained_work()
                    .map(|work| work.runtime_id().to_string())
            }),
        Some("rt:session:next".to_string())
    );
}

/// Every store keeps one observer binding per run: an identical put is a
/// no-op, another binding fails closed, and the first stands (in SQLite,
/// across a reopen).
#[tokio::test]
async fn run_observer_bindings_are_write_once_in_every_store() {
    async fn assert_write_once(store: &dyn MobRunStore, run_id: &RunId) {
        let binding = MobRunObserverBinding {
            run_id: run_id.clone(),
            mob_id: MobId::from("observer-store-mob"),
            flow_id: flow_id("review"),
            observer: observer(COORDINATOR),
        };
        assert_eq!(store.get_run_observer(run_id).await.expect("read"), None);
        assert!(store.put_run_observer(&binding).await.expect("first put"));
        assert!(
            !store
                .put_run_observer(&binding)
                .await
                .expect("identical put"),
            "an identical binding again is a no-op"
        );
        let other = MobRunObserverBinding {
            observer: observer(BYSTANDER),
            ..binding.clone()
        };
        assert!(matches!(
            store.put_run_observer(&other).await,
            Err(MobStoreError::CasConflict(_))
        ));
        assert_eq!(
            store.get_run_observer(run_id).await.expect("read"),
            Some(binding)
        );
    }

    let run_id = RunId::new();
    assert_write_once(&InMemoryMobRunStore::new(), &run_id).await;
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("observers.db");
    let stores = SqliteMobStores::open(&path).expect("open sqlite stores");
    assert_write_once(&stores.run_store(), &run_id).await;
    drop(stores);
    let reopened = SqliteMobStores::open(&path).expect("reopen sqlite stores");
    assert_eq!(
        reopened
            .run_store()
            .get_run_observer(&run_id)
            .await
            .expect("read after reopen")
            .map(|binding| binding.observer),
        Some(observer(COORDINATOR))
    );
}

/// A wake host that keeps one entry per wake key, as the continuation
/// ledger does, counts every submission, and answers the statuses a row
/// sets (pending for a submitted key otherwise).
#[derive(Default)]
struct LedgerWakes {
    ledger: std::sync::Mutex<BTreeMap<String, (String, BackgroundJobTerminalStatus, String)>>,
    statuses: std::sync::Mutex<BTreeMap<String, FlowRunWakeStatus>>,
    submissions: std::sync::atomic::AtomicUsize,
}

impl LedgerWakes {
    fn keys(&self) -> Vec<String> {
        self.ledger
            .lock()
            .expect("ledger")
            .keys()
            .cloned()
            .collect()
    }

    fn entry(&self, key: &str) -> Option<(String, BackgroundJobTerminalStatus, String)> {
        self.ledger.lock().expect("ledger").get(key).cloned()
    }

    fn set_status(&self, key: &str, status: FlowRunWakeStatus) {
        self.statuses
            .lock()
            .expect("statuses")
            .insert(key.to_string(), status);
    }
}

#[async_trait::async_trait]
impl FlowRunObserverWake for LedgerWakes {
    async fn submit(
        &self,
        _handle: &MobHandle,
        binding: &MobRunObserverBinding,
        wake: &FlowRunWake,
    ) -> Result<(), String> {
        self.submissions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.ledger
            .lock()
            .expect("ledger")
            .entry(wake.key().to_string())
            .or_insert_with(|| {
                (
                    binding.observer.member().as_str().to_string(),
                    wake.status(),
                    wake.result_digest().to_string(),
                )
            });
        Ok(())
    }

    async fn status(
        &self,
        _handle: &MobHandle,
        _binding: &MobRunObserverBinding,
        key: &str,
    ) -> Result<FlowRunWakeStatus, String> {
        if let Some(status) = self.statuses.lock().expect("statuses").get(key) {
            return Ok(status.clone());
        }
        Ok(if self.ledger.lock().expect("ledger").contains_key(key) {
            FlowRunWakeStatus::Pending
        } else {
            FlowRunWakeStatus::NotSubmitted
        })
    }
}

/// The wake keys the host holds once it holds `count`, within a bound.
async fn wait_for_wakes(wakes: &LedgerWakes, count: usize) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let keys = wakes.keys();
        if keys.len() >= count {
            return keys;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the host never held {count} wake(s): {keys:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The wake `run_id`'s binding is owed, read back from the stores.
async fn expected_wake(handle: &MobHandle, run_id: &RunId) -> FlowRunWake {
    let (_, wake) = handle
        .flow_run_observer_wake_facts(run_id)
        .await
        .expect("read the wake facts")
        .expect("a bound terminal run");
    wake
}

/// A fresh observed admission and its completion: the run wakes
/// its observer exactly once, under `mob_flow_run:{run_id}`, with the
/// completed status and the outcome digest the run's record confirms. The
/// host's re-link adds nothing for a key it already holds, and
/// `mob_flow_status` shows where the wake stands, to the observer too.
#[tokio::test]
async fn a_completed_run_wakes_its_observer_once() {
    let (handle, _service) = observer_mob().await;
    let wakes = Arc::new(LedgerWakes::default());
    handle.install_flow_run_observer_wake(wakes.clone());
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-30"),
            reviewers(),
            &key("review-c-30"),
            observer(COORDINATOR),
        )
        .await
        .expect("admit the observed run");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(10)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);
    let keys = wait_for_wakes(&wakes, 1).await;
    assert_eq!(keys, vec![format!("mob_flow_run:{run_id}")]);
    let wake = expected_wake(&handle, &run_id).await;
    assert_eq!(wake.key(), keys[0]);
    assert_eq!(
        wakes.entry(&keys[0]),
        Some((
            COORDINATOR.to_string(),
            BackgroundJobTerminalStatus::Completed,
            wake.result_digest().to_string()
        ))
    );
    assert_eq!(
        wake.outcome(),
        &serde_json::json!({
            "mob_id": handle.mob_id().to_string(),
            "flow_id": "review",
            "run_id": run_id.to_string(),
            "status": "completed",
        })
    );

    assert_eq!(
        handle
            .resubmit_flow_run_observer_wakes()
            .await
            .expect("re-link"),
        1
    );
    assert_eq!(wakes.keys(), keys, "the re-link adds no wake");

    let read = status(&member_tools(&handle, COORDINATOR, false).await, &run_id)
        .await
        .expect("the observer reads its run");
    assert_eq!(
        read["observer_wake"],
        serde_json::json!({ "status": "pending", "member": COORDINATOR, "key": keys[0] })
    );
}

/// A failed run and a run its caller canceled each wake their observer
/// once, with the failed and cancelled statuses.
#[tokio::test]
async fn failed_and_canceled_runs_wake_their_observer_once() {
    let (handle, service) = observer_mob().await;
    let wakes = Arc::new(LedgerWakes::default());
    handle.install_flow_run_observer_wake(wakes.clone());

    service.set_flow_turn_fail(true);
    let failed = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-31"),
            reviewers(),
            &key("review-c-31"),
            observer(COORDINATOR),
        )
        .await
        .expect("admit the run that fails");
    let terminal = wait_for_run_terminal(&handle, &failed, Duration::from_secs(10)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);
    wait_for_wakes(&wakes, 1).await;
    service.set_flow_turn_fail(false);

    service.set_flow_turn_never_terminal(true);
    let canceled = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-32"),
            reviewers(),
            &key("review-c-32"),
            observer(COORDINATOR),
        )
        .await
        .expect("admit the run to cancel");
    handle
        .cancel_flow(canceled.clone())
        .await
        .expect("cancel the run");
    let terminal = wait_for_run_terminal(&handle, &canceled, Duration::from_secs(10)).await;
    assert_eq!(terminal.status, MobRunStatus::Canceled);
    let keys = wait_for_wakes(&wakes, 2).await;
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert_eq!(
        wakes
            .entry(&format!("mob_flow_run:{failed}"))
            .map(|(_, status, _)| status),
        Some(BackgroundJobTerminalStatus::Failed)
    );
    assert_eq!(
        wakes
            .entry(&format!("mob_flow_run:{canceled}"))
            .map(|(_, status, _)| status),
        Some(BackgroundJobTerminalStatus::Cancelled)
    );
}

/// The late bind: a run admitted through the old API that is
/// already terminal wakes its observer at once when it is bound, exactly
/// once however often it is bound again; a run nobody observes wakes
/// nobody.
#[tokio::test]
async fn a_terminal_run_bound_late_wakes_its_observer_once() {
    let (handle, _service) = observer_mob().await;
    let wakes = Arc::new(LedgerWakes::default());
    handle.install_flow_run_observer_wake(wakes.clone());
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-33"),
            reviewers(),
            &key("review-c-33"),
        )
        .await
        .expect("admit a run without an observer");
    let unobserved = handle
        .run_flow_with_role_targets_and_external_identity(
            flow_id("review"),
            params("c-34"),
            reviewers(),
            &key("review-c-34"),
        )
        .await
        .expect("admit a run nobody observes");
    wait_for_run_terminal(&handle, &run_id, Duration::from_secs(10)).await;
    wait_for_run_terminal(&handle, &unobserved, Duration::from_secs(10)).await;
    assert!(wakes.keys().is_empty(), "an unbound run wakes nobody");

    for _ in 0..2 {
        handle
            .bind_flow_run_observer(
                flow_id("review"),
                &key("review-c-33"),
                observer(COORDINATOR),
            )
            .await
            .expect("bind the terminal run");
    }
    let keys = wait_for_wakes(&wakes, 1).await;
    assert_eq!(keys, vec![format!("mob_flow_run:{run_id}")]);
    assert_eq!(
        handle
            .resubmit_flow_run_observer_wakes()
            .await
            .expect("re-link"),
        1,
        "only the bound run owes a wake"
    );
    assert_eq!(wakes.keys(), keys);
}

/// A wake refused at delivery is visible to the observer through
/// `mob_flow_status`; after a respawn, a rebind that refreshes the binding's
/// retained work wakes the observer again at once, under the refreshed
/// binding's own key.
#[tokio::test]
async fn a_refused_wake_shows_and_a_refreshing_rebind_wakes_again() {
    let (handle, _service) = observer_mob().await;
    let wakes = Arc::new(LedgerWakes::default());
    handle.install_flow_run_observer_wake(wakes.clone());
    let first = retained(&current_runtime(&handle, COORDINATOR).await);
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-35"),
            reviewers(),
            &key("review-c-35"),
            observer(COORDINATOR).with_retained_work(first.clone()),
        )
        .await
        .expect("admit the observed run");
    wait_for_run_terminal(&handle, &run_id, Duration::from_secs(10)).await;
    let keys = wait_for_wakes(&wakes, 1).await;
    let first_key = format!("mob_flow_run:{run_id}:{}", first.run_id());
    assert_eq!(keys, vec![first_key.clone()]);
    wakes.set_status(
        &first_key,
        FlowRunWakeStatus::Refused {
            reason: meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding,
        },
    );
    let read = status(&member_tools(&handle, COORDINATOR, false).await, &run_id)
        .await
        .expect("the observer reads its run");
    assert_eq!(
        read["observer_wake"],
        serde_json::json!({
            "status": "refused",
            "reason": "no_admissible_work_binding",
            "member": COORDINATOR,
            "key": first_key,
        })
    );

    handle
        .respawn(AgentIdentity::from(COORDINATOR), None)
        .await
        .expect("respawn the coordinator");
    let fresh = retained(&current_runtime(&handle, COORDINATOR).await);
    handle
        .bind_flow_run_observer(
            flow_id("review"),
            &key("review-c-35"),
            observer(COORDINATOR).with_retained_work(fresh.clone()),
        )
        .await
        .expect("rebind with the new runtime's work");
    let keys = wait_for_wakes(&wakes, 2).await;
    let fresh_key = format!("mob_flow_run:{run_id}:{}", fresh.run_id());
    assert!(keys.contains(&fresh_key), "{keys:?}");
    let read = status(&member_tools(&handle, COORDINATOR, false).await, &run_id)
        .await
        .expect("the observer reads its run from its new session");
    assert_eq!(
        read["observer_wake"],
        serde_json::json!({ "status": "pending", "member": COORDINATOR, "key": fresh_key })
    );
}

/// A mob whose host installed no wake host still lets the observer read
/// its run; `observer_wake` says the host delivers none.
#[tokio::test]
async fn without_a_wake_host_the_observer_reads_an_unavailable_wake() {
    let (handle, _service) = observer_mob().await;
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            flow_id("review"),
            params("c-36"),
            reviewers(),
            &key("review-c-36"),
            observer(COORDINATOR),
        )
        .await
        .expect("admit the observed run");
    wait_for_run_terminal(&handle, &run_id, Duration::from_secs(10)).await;
    assert_eq!(
        handle
            .resubmit_flow_run_observer_wakes()
            .await
            .expect("re-link"),
        0
    );
    let read = status(&member_tools(&handle, COORDINATOR, false).await, &run_id)
        .await
        .expect("the observer reads its run");
    assert_eq!(
        read["observer_wake"],
        serde_json::json!({
            "status": "unavailable",
            "member": COORDINATOR,
            "key": format!("mob_flow_run:{run_id}"),
        })
    );
}
