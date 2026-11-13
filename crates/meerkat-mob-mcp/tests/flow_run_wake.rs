//! A keyed flow run's observer is woken through the host's continuation
//! owner once the run is terminal, over the production persistent session
//! service and an armed delivery owner: one durable `mob_flow_run`
//! completion record in the observer's transcript, and `observer_wake`
//! reports it applied. Only the LLM is scripted.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use meerkat_core::event::BackgroundJobTerminalStatus;
use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind};
use meerkat_core::{Message, SessionId};
use meerkat_mob::store::MobExternalDeliveryIdentity;
use meerkat_mob::{
    AgentIdentity, FlowId, FlowRoleTargets, FlowRunObserver, FlowRunWakeStatus, MobHandle,
    MobRunStatus, RunId,
};
use support::{CouncilFixture, ScriptedTurn, council_definition};

const COORDINATOR: &str = "coordinator";
const REVIEWER: &str = "reviewer";

/// The source mob's definition with one review flow over its participants.
fn observed_definition(mob_id: &str) -> meerkat_mob::MobDefinition {
    let mut definition = council_definition(mob_id);
    let review: meerkat_mob::definition::FlowSpec = serde_json::from_value(serde_json::json!({
        "steps": {
            "review": { "role": "participant", "message": "REVIEW-TASK-3W review the change" }
        }
    }))
    .expect("a review flow");
    definition.flows.insert(FlowId::from("review"), review);
    definition
}

async fn observed_fixture() -> (CouncilFixture, MobHandle) {
    let fixture =
        CouncilFixture::new_runtime_backed(|_request| ScriptedTurn::Text("ACK-7P".to_string()));
    fixture
        .seed_source_mob_from(
            observed_definition(fixture.source_mob_id().as_str()),
            &[COORDINATOR, REVIEWER],
        )
        .await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("source mob handle");
    (fixture, handle)
}

fn reviewers() -> FlowRoleTargets {
    FlowRoleTargets::new([("participant", vec![AgentIdentity::from(REVIEWER)])])
        .expect("a well-formed binding")
}

/// A caller delivery identity; the correlation id is fresh on every call.
fn key(value: &str) -> MobExternalDeliveryIdentity {
    MobExternalDeliveryIdentity::new(value, uuid::Uuid::new_v4().to_string())
        .expect("a canonical delivery identity")
}

async fn wait_for_terminal(handle: &MobHandle, run_id: &RunId) -> MobRunStatus {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(run) = handle
            .flow_status(run_id.clone())
            .await
            .expect("read the run")
            && matches!(
                run.status,
                MobRunStatus::Completed | MobRunStatus::Failed | MobRunStatus::Canceled
            )
        {
            return run.status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run {run_id} never ended"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

type PersistentService = meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>;

async fn persisted_messages(service: &PersistentService, session: &SessionId) -> Vec<Message> {
    <PersistentService as meerkat_mob::MobSessionService>::load_persisted_session(service, session)
        .await
        .expect("load persisted session")
        .expect("session exists")
        .messages()
        .to_vec()
}

/// The durable `mob_flow_run` completion records of `run_id`: the status and
/// typed detail of each.
fn wake_records(
    messages: &[Message],
    run_id: &RunId,
) -> Vec<(BackgroundJobTerminalStatus, String)> {
    let job_id = run_id.to_string();
    messages
        .iter()
        .filter_map(|message| match message {
            Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::BackgroundJob => {
                notice.blocks.iter().find_map(|block| match block {
                    SystemNoticeBlock::BackgroundJob {
                        job_id: id,
                        display_name,
                        status,
                        detail,
                        persisted: true,
                    } if id == &job_id && display_name.as_deref() == Some("mob_flow_run") => {
                        Some((*status, detail.clone().unwrap_or_default()))
                    }
                    _ => None,
                })
            }
            _ => None,
        })
        .collect()
}

/// The observer's one wake record for `run_id`, once its transcript holds
/// it; more than one fails.
async fn wait_for_wake(
    fixture: &CouncilFixture,
    observer: &SessionId,
    run_id: &RunId,
) -> (BackgroundJobTerminalStatus, String) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let mut records = wake_records(
            &persisted_messages(fixture.service.as_ref(), observer).await,
            run_id,
        );
        assert!(
            records.len() <= 1,
            "run {run_id} woke its observer more than once: {records:?}"
        );
        if let Some(record) = records.pop() {
            return record;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the observer was never woken for run {run_id}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// `observer_wake` of `run_id` once the host reports it applied.
async fn wait_for_applied(handle: &MobHandle, run_id: &RunId) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let (_, status) = handle
            .flow_run_observer_wake_status(run_id)
            .await
            .expect("read the wake status")
            .expect("a bound run");
        if status == FlowRunWakeStatus::Applied {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the wake of run {run_id} was never applied: {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// End to end, a fresh observed admission and its completion: the run's
/// outcome reaches the coordinator's transcript once, as a `mob_flow_run`
/// completion record of the completed run, through the continuation owner
/// its host bound; the wake reads applied, and the host's re-link adds no
/// second record.
#[tokio::test(flavor = "multi_thread")]
async fn a_bound_flow_run_wakes_its_observer_once_through_the_continuation_owner() {
    let (fixture, handle) = observed_fixture().await;
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            FlowId::from("review"),
            serde_json::json!({ "change": "c-40" }),
            reviewers(),
            &key("review-c-40"),
            FlowRunObserver::new(AgentIdentity::from(COORDINATOR)),
        )
        .await
        .expect("admit the observed run");
    assert_eq!(
        wait_for_terminal(&handle, &run_id).await,
        MobRunStatus::Completed
    );
    let coordinator = handle
        .resolve_bridge_session_id(&AgentIdentity::from(COORDINATOR))
        .await
        .expect("the coordinator's session");
    let (status, detail) = wait_for_wake(&fixture, &coordinator, &run_id).await;
    assert_eq!(status, BackgroundJobTerminalStatus::Completed);
    assert!(
        detail.contains(&run_id.to_string()) && detail.contains("completed"),
        "the record names the run and its status: {detail}"
    );
    wait_for_applied(&handle, &run_id).await;

    assert_eq!(
        handle
            .resubmit_flow_run_observer_wakes()
            .await
            .expect("re-link"),
        1
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        wake_records(
            &persisted_messages(fixture.service.as_ref(), &coordinator).await,
            &run_id
        )
        .len(),
        1,
        "the re-link delivers no second wake"
    );
    let reviewer = handle
        .resolve_bridge_session_id(&AgentIdentity::from(REVIEWER))
        .await
        .expect("the reviewer's session");
    assert!(
        wake_records(
            &persisted_messages(fixture.service.as_ref(), &reviewer).await,
            &run_id
        )
        .is_empty(),
        "only the observer is woken"
    );
    fixture.teardown().await;
}

/// End to end, the late bind: a run admitted without an observer
/// and already completed wakes the member bound to it later, once, however
/// often it is bound again.
#[tokio::test(flavor = "multi_thread")]
async fn a_completed_run_bound_late_wakes_its_observer_once() {
    let (fixture, handle) = observed_fixture().await;
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity(
            FlowId::from("review"),
            serde_json::json!({ "change": "c-41" }),
            reviewers(),
            &key("review-c-41"),
        )
        .await
        .expect("admit a run without an observer");
    assert_eq!(
        wait_for_terminal(&handle, &run_id).await,
        MobRunStatus::Completed
    );
    for _ in 0..2 {
        handle
            .bind_flow_run_observer(
                FlowId::from("review"),
                &key("review-c-41"),
                FlowRunObserver::new(AgentIdentity::from(COORDINATOR)),
            )
            .await
            .expect("bind the completed run");
    }
    let coordinator = handle
        .resolve_bridge_session_id(&AgentIdentity::from(COORDINATOR))
        .await
        .expect("the coordinator's session");
    let (status, _) = wait_for_wake(&fixture, &coordinator, &run_id).await;
    assert_eq!(status, BackgroundJobTerminalStatus::Completed);
    wait_for_applied(&handle, &run_id).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        wake_records(
            &persisted_messages(fixture.service.as_ref(), &coordinator).await,
            &run_id
        )
        .len(),
        1
    );
    fixture.teardown().await;
}

/// One turn's retained work as `session`'s runtime stages it; every call
/// names a new run.
fn retained_on(session: &SessionId) -> meerkat_core::retained_work::RetainedWorkIdentity {
    meerkat_core::retained_work::RetainedWorkIdentity::new(
        meerkat_runtime::LogicalRuntimeId::for_session(session).to_string(),
        meerkat_core::lifecycle::RunId::new(),
        Vec::new(),
        BTreeMap::new(),
        None,
    )
}

/// The native job owner answers a flow run wake from the run's observer
/// binding and its committed terminal record, as a governed delivery asks
/// it to: the binding's retained work, owed to the session whose runtime
/// staged it, with the digest of the run's outcome. After the member is
/// respawned and its binding refreshed, the owner answers the new work, so
/// a wake row committed with the old work no longer matches the owner's
/// answer and its delivery is refused. A binding without retained work
/// confirms nothing. A restarted host binds its continuations before it
/// restores its mobs, and until then a run it cannot find waits; once its
/// mobs are restored the run is found again.
#[tokio::test(flavor = "multi_thread")]
async fn the_job_owner_answers_a_flow_run_wake_from_its_binding() {
    let (fixture, handle) = observed_fixture().await;
    let coordinator = AgentIdentity::from(COORDINATOR);
    let first_session = handle
        .resolve_bridge_session_id(&coordinator)
        .await
        .expect("the coordinator's session");
    let first = retained_on(&first_session);
    let run_id = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            FlowId::from("review"),
            serde_json::json!({ "change": "c-42" }),
            reviewers(),
            &key("review-c-42"),
            FlowRunObserver::new(coordinator.clone()).with_retained_work(first.clone()),
        )
        .await
        .expect("admit the observed run");
    assert_eq!(
        wait_for_terminal(&handle, &run_id).await,
        MobRunStatus::Completed
    );
    let bare_run = handle
        .run_flow_with_role_targets_and_external_identity_observed(
            FlowId::from("review"),
            serde_json::json!({ "change": "c-43" }),
            reviewers(),
            &key("review-c-43"),
            FlowRunObserver::new(coordinator.clone()),
        )
        .await
        .expect("admit a run with a bare observer");
    assert_eq!(
        wait_for_terminal(&handle, &bare_run).await,
        MobRunStatus::Completed
    );

    let bindings = Arc::clone(
        &fixture
            .continuations
            .as_ref()
            .expect("continuations")
            .bindings,
    );
    let lookup = |run_id: &RunId| {
        let (bindings, job_id) = (Arc::clone(&bindings), run_id.to_string());
        async move {
            bindings
                .job_source()
                .expect("a bound job owner")
                .retained_job(&meerkat::ContinuationProducer::MobFlowRun, &job_id)
                .await
        }
    };
    let digest = |handle: MobHandle, run_id: RunId| async move {
        handle
            .flow_run_observer_wake_facts(&run_id)
            .await
            .expect("read the wake facts")
            .expect("a bound terminal run")
            .1
            .result_digest()
            .to_string()
    };

    match lookup(&run_id).await {
        meerkat::RetainedJobLookup::Found(facts) => {
            assert_eq!(facts.retained_work.as_ref(), Some(&first));
            assert_eq!(facts.owner_session_id, first_session);
            assert_eq!(
                facts.result_digest,
                digest(handle.clone(), run_id.clone()).await
            );
        }
        other => panic!("the bound run's wake is confirmed: {other:?}"),
    }
    assert_eq!(
        lookup(&bare_run).await,
        meerkat::RetainedJobLookup::Absent,
        "a binding without retained work confirms nothing"
    );

    handle
        .respawn(coordinator.clone(), None)
        .await
        .expect("respawn the coordinator");
    let second_session = handle
        .resolve_bridge_session_id(&coordinator)
        .await
        .expect("the respawned coordinator's session");
    assert_ne!(second_session, first_session);
    let second = retained_on(&second_session);
    handle
        .bind_flow_run_observer(
            FlowId::from("review"),
            &key("review-c-42"),
            FlowRunObserver::new(coordinator.clone()).with_retained_work(second.clone()),
        )
        .await
        .expect("refresh the binding with the new runtime's work");
    match lookup(&run_id).await {
        meerkat::RetainedJobLookup::Found(facts) => {
            assert_eq!(facts.retained_work.as_ref(), Some(&second));
            assert_eq!(facts.owner_session_id, second_session);
            // A governed delivery confirms a row's retained work against
            // this answer: a row committed with the first work mismatches
            // and is refused (`no_admissible_work_binding`).
            assert_ne!(facts.retained_work.as_ref(), Some(&first));
        }
        other => panic!("the refreshed wake is confirmed: {other:?}"),
    }

    // The process dies; the restarted host binds before anything restores.
    fixture.shut_down_predecessor().await;
    let restarted = fixture.restart_state();
    assert!(
        matches!(
            lookup(&run_id).await,
            meerkat::RetainedJobLookup::Unavailable(_)
        ),
        "a run in a mob not restored yet waits"
    );
    restarted
        .mob_handles_snapshot()
        .await
        .expect("restore the mobs");
    match lookup(&run_id).await {
        meerkat::RetainedJobLookup::Found(facts) => {
            assert_eq!(facts.retained_work.as_ref(), Some(&second));
        }
        other => panic!("the restored mob confirms its run's wake: {other:?}"),
    }
    fixture.teardown().await;
}
