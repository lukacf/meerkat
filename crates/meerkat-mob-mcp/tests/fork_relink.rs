//! Restart re-linking of fork_off children.
//!
//! A detached child's outcome is delivered by a process-local custodian. These
//! tests drop that custodian (the "restart"), rebuild the mob state over the
//! same durable stores, and check that the re-link pass delivers the outcome
//! exactly once, and re-arms the opt-in max_run limit from the original start.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::{
    AgentIdentity, ForkChildRunOutcome, ForkJobBinding, ProfileName, SpawnMemberSpec,
};
use meerkat_mob_mcp::fork_relink::ForkRelinkAction;
use support::{CouncilFixture, ScriptedTurn, TurnGate};

const CHILD_REPLY: &str = "RELINKED-REPLY-4K";
const CHILD_TASK: &str = "reply with the token";

async fn forker_session(fixture: &CouncilFixture) -> meerkat_core::SessionId {
    fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("handle")
        .resolve_bridge_session_id(&AgentIdentity::from("forker"))
        .await
        .expect("forker session")
}

fn child_spec(child: &str) -> SpawnMemberSpec {
    let mut spec =
        SpawnMemberSpec::new(ProfileName::from("participant"), AgentIdentity::from(child));
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    spec.initial_message = Some(meerkat_core::types::ContentInput::Text(
        CHILD_TASK.to_string(),
    ));
    spec
}

async fn completion_records(
    fixture: &CouncilFixture,
    session: &meerkat_core::SessionId,
    job_id: &str,
) -> usize {
    let persisted = <meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder> as meerkat_mob::MobSessionService>::load_persisted_session(
        fixture.service.as_ref(),
        session,
    )
    .await
    .expect("load forker session")
    .expect("forker session exists");
    persisted
        .messages()
        .iter()
        .filter(|message| is_completion_record(message, job_id))
        .count()
}

/// Whether `message` is the durable completion record of fork_off job
/// `job_id`.
fn is_completion_record(message: &meerkat_core::Message, job_id: &str) -> bool {
    match message {
        meerkat_core::Message::SystemNotice(notice) => notice.blocks.iter().any(|block| {
            matches!(
                block,
                meerkat_core::types::SystemNoticeBlock::BackgroundJob {
                    job_id: recorded,
                    display_name: Some(tool),
                    persisted: true,
                    ..
                } if recorded == job_id && tool == "fork_off"
            )
        }),
        _ => false,
    }
}

/// The runtime that admits detached completions for the fixture's sessions.
fn relink_runtime(fixture: &CouncilFixture) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
    fixture.state.session_service().runtime_adapter()
}

/// Wait until job `job_id`'s completion record is in `session`: delivery
/// admits it as the owner's next turn input, which commits with that turn.
async fn await_completion_record(
    fixture: &CouncilFixture,
    session: &meerkat_core::SessionId,
    job_id: &str,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while completion_records(fixture, session, job_id).await == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the completion record never landed"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The text of job `job_id`'s completion record(s) in `session`.
async fn completion_record_text(
    fixture: &CouncilFixture,
    session: &meerkat_core::SessionId,
    job_id: &str,
) -> String {
    let persisted = <meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder> as meerkat_mob::MobSessionService>::load_persisted_session(
        fixture.service.as_ref(),
        session,
    )
    .await
    .expect("load forker session")
    .expect("forker session exists");
    persisted
        .messages()
        .iter()
        .filter(|message| is_completion_record(message, job_id))
        .map(|message| format!("{message:?}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The custodian died with the old process after the child finished: the
/// re-link pass records the child's result in the forker's transcript once.
#[tokio::test]
async fn relink_delivers_a_finished_childs_result_exactly_once() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-finished-before-restart".to_string();

    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec("relink-child"),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: job_id.clone(),
                owner_session_id: owner.clone(),
            }),
        )
        .await
        .expect("fork");
    // No custodian: the outcome is observed here and never delivered, as if
    // the process had died right after the child finished.
    assert!(matches!(
        run.outcome().await,
        Some(ForkChildRunOutcome::Completed(_))
    ));
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 0);

    // The restarted host: a fresh state built after the job started receives
    // the surviving mob the way MobKit restores it, by inserting its handle.
    // That triggers the re-link automatically.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let restarted = Arc::new(meerkat_mob_mcp::MobMcpState::new(
        fixture.service.clone(),
        meerkat_mob::MobControlPrincipal::Owner,
    ));
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while completion_records(&fixture, &owner, &job_id).await == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the re-link never delivered"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);

    // Running it again (a later restart) records nothing more.
    let reports = restarted.relink_restored_fork_children().await;
    let report = reports
        .iter()
        .find(|report| report.job_id == job_id)
        .expect("the child is visited");
    assert_eq!(report.action, ForkRelinkAction::AlreadyDelivered);
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);

    let persisted = <meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder> as meerkat_mob::MobSessionService>::load_persisted_session(
        fixture.service.as_ref(),
        &owner,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(format!("{:?}", persisted.messages()).contains(CHILD_REPLY));
    fixture.teardown().await;
}

/// A child still running when the re-link reaches it keeps its opt-in
/// max_run, measured from the ORIGINAL start: the re-link waits for the run,
/// and the limit winning cancels and retires the child and delivers
/// max_run_elapsed.
#[tokio::test]
async fn relink_rearms_max_run_from_the_original_start() {
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    // Only the child's task is held; the forker's wake turn (delivery) runs.
    let fixture = CouncilFixture::new(move |request| {
        if support::last_user_text(request).contains(CHILD_TASK) {
            ScriptedTurn::Gated(Arc::clone(&turn_gate), CHILD_REPLY.to_string())
        } else {
            ScriptedTurn::Text("noted".to_string())
        }
    });
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-running-across-restart".to_string();
    let child = AgentIdentity::from("relink-running-child");

    // The old process's supervisor has no limit of its own here, so only the
    // re-link can end the run.
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec(child.as_str()),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: job_id.clone(),
                owner_session_id: owner.clone(),
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    drop(run);

    // The durable record as a restarted host reads it, with a limit that
    // ends 800 ms from now when measured from the original start.
    let mut job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    assert!(
        job.prefix_message_count > 0,
        "the fork prefix length is recorded"
    );
    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    job.max_run_ms = Some(now_ms.saturating_sub(job.started_at_ms) + 800);

    let rearmed_at = tokio::time::Instant::now();
    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        relink_runtime(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(action, ForkRelinkAction::Delivered);
    let fired_after = rearmed_at.elapsed();
    assert!(
        fired_after >= Duration::from_millis(500) && fired_after < Duration::from_secs(5),
        "the limit fired {fired_after:?} after the re-link; it counts from the original start"
    );
    await_completion_record(&fixture, &owner, &job_id).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    let persisted = <meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder> as meerkat_mob::MobSessionService>::load_persisted_session(
        fixture.service.as_ref(),
        &owner,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(format!("{:?}", persisted.messages()).contains("max_run_elapsed"));
    assert!(handle.get_member(&child).await.unwrap().is_none());
    gate.open();
    fixture.teardown().await;
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// A child that finished within its max_run, but whose outcome the old
/// process never recorded, delivers its REAL reply when the restart lands
/// after the limit, and stays seated. A second pass (the outcome is now
/// delivered) does not retire it either.
#[tokio::test]
async fn relink_past_max_run_delivers_a_child_that_finished_within_its_limit() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-finished-in-time".to_string();
    let child = AgentIdentity::from("relink-in-time-child");

    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec(child.as_str()),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: job_id.clone(),
                owner_session_id: owner.clone(),
            }),
        )
        .await
        .expect("fork");
    assert!(matches!(
        run.outcome().await,
        Some(ForkChildRunOutcome::Completed(_))
    ));
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 0);

    // The limit ended just after the child finished, and the restart lands
    // after it: measured from the original start, no time remains.
    let mut job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    job.max_run_ms = Some(now_ms().saturating_sub(job.started_at_ms).max(1));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(now_ms() > job.started_at_ms + job.max_run_ms.unwrap());

    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        relink_runtime(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(action, ForkRelinkAction::Delivered);
    await_completion_record(&fixture, &owner, &job_id).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(
        record.contains(CHILD_REPLY) && record.contains("completed"),
        "the child's real reply is delivered: {record}"
    );
    assert!(
        !record.contains("max_run_elapsed"),
        "a child that finished in time is not reported as timed out: {record}"
    );
    assert!(
        handle.get_member(&child).await.unwrap().is_some(),
        "a completed child stays seated for its forker"
    );

    // Already delivered: a later restart past the limit still leaves the
    // completed child seated and records nothing more.
    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        relink_runtime(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(action, ForkRelinkAction::AlreadyDelivered);
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    assert!(handle.get_member(&child).await.unwrap().is_some());
    fixture.teardown().await;
}

/// A child genuinely still running when the restart lands past its max_run
/// has no reply to deliver: the limit applies at once, the child (and its
/// descendants) is retired and max_run_elapsed is delivered.
#[tokio::test]
async fn relink_past_max_run_retires_a_child_still_running() {
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    // Only the child's task is held; the forker's wake turn (delivery) runs.
    let fixture = CouncilFixture::new(move |request| {
        if support::last_user_text(request).contains(CHILD_TASK) {
            ScriptedTurn::Gated(Arc::clone(&turn_gate), CHILD_REPLY.to_string())
        } else {
            ScriptedTurn::Text("noted".to_string())
        }
    });
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-running-past-limit".to_string();
    let child = AgentIdentity::from("relink-overdue-child");

    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec(child.as_str()),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: job_id.clone(),
                owner_session_id: owner.clone(),
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    drop(run);

    let mut job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    job.max_run_ms = Some(now_ms().saturating_sub(job.started_at_ms).max(1));
    tokio::time::sleep(Duration::from_millis(100)).await;

    let started = tokio::time::Instant::now();
    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        relink_runtime(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(action, ForkRelinkAction::Delivered);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "an elapsed limit applies at once, without waiting for the run"
    );
    assert!(handle.get_member(&child).await.unwrap().is_none());
    await_completion_record(&fixture, &owner, &job_id).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(record.contains("max_run_elapsed"), "{record}");
    assert!(!record.contains(CHILD_REPLY), "{record}");
    gate.open();
    fixture.teardown().await;
}
