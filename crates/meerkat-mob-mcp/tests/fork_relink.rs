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
use meerkat_mob_mcp::detached_delivery::OwnerRevivalDeferral;
use meerkat_mob_mcp::fork_relink::ForkRelinkAction;
use support::{CouncilFixture, MobBackedOwnerHost, ScriptedTurn, SeenRequests, TurnGate};

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

/// The re-link delivery for the fixture's runtime: no owner hook, and only
/// the child's own mob to find the owner in.
fn relink_delivery(fixture: &CouncilFixture) -> meerkat_mob_mcp::fork_relink::RelinkDelivery {
    meerkat_mob_mcp::fork_relink::RelinkDelivery {
        runtime: relink_runtime(fixture),
        ..Default::default()
    }
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
        &relink_delivery(&fixture),
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
        &relink_delivery(&fixture),
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
        &relink_delivery(&fixture),
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
        &relink_delivery(&fixture),
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

/// Regression for the lifecycle review's probe: a child forked with a LIVE
/// max_run that completes inside it, with its outcome never delivered, is
/// re-linked after the limit has passed. It delivers the real reply and
/// stays seated, instead of being retired as max_run_elapsed.
#[tokio::test]
async fn relink_after_a_live_limit_passed_delivers_the_completed_childs_reply() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-live-limit-completed".to_string();
    let child = AgentIdentity::from("relink-live-limit-child");

    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec(child.as_str()),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            Some(Duration::from_millis(1500)),
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
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(
        handle.get_member(&child).await.unwrap().is_some(),
        "the live limit does not touch a child that completed within it"
    );

    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(action, ForkRelinkAction::Delivered);
    assert!(handle.get_member(&child).await.unwrap().is_some());
    await_completion_record(&fixture, &owner, &job_id).await;
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(
        record.contains(CHILD_REPLY) && !record.contains("max_run_elapsed"),
        "{record}"
    );
    fixture.teardown().await;
}

/// A job whose completion was already delivered is over. A later restore past
/// its limit, while the child (kept seated for further work) is busy with a
/// later task and its transcript no longer shows the job's reply where the
/// job left it (compaction or later work rewrote it), leaves the child alone:
/// no cancel, no retirement, no second record.
#[tokio::test]
async fn relink_leaves_a_child_whose_job_already_ended_alone() {
    const LATER_TASK: &str = "a later task, unrelated to the job";
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    let fixture = CouncilFixture::new(move |request| {
        if support::last_user_text(request).contains(LATER_TASK) {
            ScriptedTurn::Gated(Arc::clone(&turn_gate), "later work done".to_string())
        } else {
            ScriptedTurn::Text(CHILD_REPLY.to_string())
        }
    });
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-ended-earlier".to_string();
    let child = AgentIdentity::from("relink-ended-child");

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
    let mut job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    // The job's outcome reaches the forker.
    assert_eq!(
        meerkat_mob_mcp::fork_relink::relink_child(
            fixture.state.session_service(),
            &relink_delivery(&fixture),
            &fixture.source_mob_id(),
            &handle,
            &child,
            &job,
        )
        .await,
        ForkRelinkAction::Delivered
    );
    await_completion_record(&fixture, &owner, &job_id).await;

    // The child takes later work, and a restore lands past the job's limit.
    let member = handle.member(&child).await.expect("child handle");
    let later_turn = tokio::spawn(async move {
        member
            .internal_turn(meerkat_core::types::ContentInput::Text(
                LATER_TASK.to_string(),
            ))
            .await
    });
    gate.wait_entered(1).await;
    job.max_run_ms = Some(1);
    job.prefix_message_count = usize::MAX / 2;

    let started = tokio::time::Instant::now();
    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(action, ForkRelinkAction::AlreadyDelivered);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        handle.get_member(&child).await.unwrap().is_some(),
        "the child is not retired for a job that ended"
    );
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    // The later turn was not cancelled: released, it completes.
    gate.open();
    later_turn
        .await
        .expect("later turn task")
        .expect("the child's later turn completes");
    fixture.teardown().await;
}

/// A fork job belongs to the incarnation whose turn it admitted: a successor
/// after a respawn carries none, so no later re-link applies the old job's
/// limit to the fresh member.
#[tokio::test]
async fn a_respawned_fork_child_carries_no_fork_job() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let child = AgentIdentity::from("relink-respawned-child");
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec(child.as_str()),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            Some(Duration::from_secs(600)),
            Some(ForkJobBinding {
                job_id: "job-before-respawn".to_string(),
                owner_session_id: owner.clone(),
            }),
        )
        .await
        .expect("fork");
    let _ = run.outcome().await;
    let predecessor = handle
        .roster()
        .await
        .get_by_identity(&child)
        .cloned()
        .expect("child seated");
    assert!(predecessor.fork_job.is_some());

    handle.respawn(child.clone(), None).await.expect("respawn");
    let entry = handle
        .roster()
        .await
        .get_by_identity(&child)
        .cloned()
        .expect("successor seated");
    assert!(
        entry.fork_job.is_none(),
        "the successor carries no fork job"
    );
    assert_eq!(
        entry.spawned_by, predecessor.spawned_by,
        "ownership still belongs to the identity"
    );
    fixture.teardown().await;
}

/// Several children of one mob are still running when the re-link reaches
/// them. Each settles on its own task and the mob has one member status
/// observation lane, so their status reads collide. A read that loses the
/// lane observes nothing and is read again, never taken for an idle child
/// (lifecycle review: all but one were reported `restart_interrupted` while
/// still running, and their real replies were lost).
#[tokio::test(flavor = "multi_thread")]
async fn relink_of_several_running_children_delivers_each_real_reply() {
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
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
    let mut jobs = Vec::new();
    for (index, child) in ["running-a", "running-b", "running-c"].iter().enumerate() {
        let job_id = format!("job-running-{index}");
        let (_fork, run) = handle
            .fork_member_then_run_detached(
                &AgentIdentity::from("forker"),
                child_spec(child),
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
        // The custodian dies with the "old process".
        drop(run);
        jobs.push(job_id);
    }
    gate.wait_entered(3).await;
    tokio::time::sleep(Duration::from_millis(5)).await;

    let restarted = Arc::new(meerkat_mob_mcp::MobMcpState::new(
        fixture.service.clone(),
        meerkat_mob::MobControlPrincipal::Owner,
    ));
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await;
    // Every child is still mid-turn: nothing may be reported yet.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mut premature = Vec::new();
    for job_id in &jobs {
        if completion_records(&fixture, &owner, job_id).await > 0 {
            premature.push(completion_record_text(&fixture, &owner, job_id).await);
        }
    }
    gate.open();
    assert!(
        premature.is_empty(),
        "running children were reported before they finished: {premature:?}"
    );
    for job_id in &jobs {
        await_completion_record(&fixture, &owner, job_id).await;
        assert_eq!(completion_records(&fixture, &owner, job_id).await, 1);
        let record = completion_record_text(&fixture, &owner, job_id).await;
        assert!(
            record.contains(CHILD_REPLY) && !record.contains("restart_interrupted"),
            "job {job_id} is delivered its real reply: {record}"
        );
    }
    fixture.teardown().await;
}

/// A host that restores a stopped mob inserts its handle before activating
/// it (MobKit's identity-first gateway after a clean shutdown). The forker is
/// not live and cannot be revived while its mob is stopped: the re-link
/// reports that typed, then delivers the outcome once the mob runs, exactly
/// once (lifecycle review: the single attempt failed and was never retried).
#[tokio::test(flavor = "multi_thread")]
async fn relink_on_a_stopped_mob_delivers_once_the_mob_runs() {
    let fixture =
        CouncilFixture::new_runtime_backed(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-stopped-mob".to_string();
    // A caller-turn fork, as fork_off makes it: the forker owns the child,
    // so the re-link revives the forker through its mob.
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec("stopped-mob-child"),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::CallerTurn,
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
    // The "restart": the mob comes back stopped and the forker is not live.
    handle.stop().await.expect("stop the mob");
    let runtime = fixture.runtime_adapter.clone().expect("runtime-backed");
    runtime
        .unregister_session(&owner)
        .await
        .expect("the forker is not live after the restart");
    tokio::time::sleep(Duration::from_millis(5)).await;
    let restarted = Arc::new(meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
        fixture.service.clone(),
        Some(Arc::clone(&runtime)),
        meerkat_mob::MobControlPrincipal::Owner,
    ));
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await;

    let reports = restarted.relink_restored_fork_children().await;
    let report = reports
        .iter()
        .find(|report| report.job_id == job_id)
        .expect("the child is visited");
    assert_eq!(
        report.action,
        ForkRelinkAction::AwaitingOwner {
            mob_id: fixture.source_mob_id(),
            reason: OwnerRevivalDeferral::MobNotRunning {
                phase: meerkat_mob::MobState::Stopped
            },
        }
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 0);

    // The host activates the mob: the automatic pass delivers now.
    handle.resume().await.expect("activate the mob");
    await_completion_record(&fixture, &owner, &job_id).await;
    assert!(
        completion_record_text(&fixture, &owner, &job_id)
            .await
            .contains(CHILD_REPLY)
    );
    let reports = restarted.relink_restored_fork_children().await;
    assert_eq!(
        reports
            .iter()
            .find(|report| report.job_id == job_id)
            .map(|report| report.action.clone()),
        Some(ForkRelinkAction::AlreadyDelivered)
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        completion_records(&fixture, &owner, &job_id).await,
        1,
        "delivered exactly once"
    );
    fixture.teardown().await;
}

/// Seat `owner` in a second mob of the fixture, which the restarted state
/// will not manage: to the re-link its session is a plain session (the
/// test's owner hook revives it through this mob). Returns the mob's handle
/// and the owner's session.
async fn seat_unmanaged_owner(
    fixture: &CouncilFixture,
) -> (meerkat_mob::MobHandle, meerkat_core::SessionId) {
    seat_owner_in(fixture, "owners").await
}

/// [`seat_unmanaged_owner`] in the mob `<source>-<suffix>`.
async fn seat_owner_in(
    fixture: &CouncilFixture,
    suffix: &str,
) -> (meerkat_mob::MobHandle, meerkat_core::SessionId) {
    let owner_mob = meerkat_mob::MobId::from(format!("{}-{suffix}", fixture.source_mob_id()));
    fixture
        .state
        .mob_create_definition(support::council_definition(owner_mob.as_str()))
        .await
        .expect("create the owner mob");
    fixture
        .state
        .mob_spawn(
            &owner_mob,
            ProfileName::from("participant"),
            AgentIdentity::from("owner"),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            Some(meerkat_mob::MobBackendKind::Session),
            None,
        )
        .await
        .expect("seat the owner");
    let owner_handle = fixture.state.handle_for(&owner_mob).await.unwrap();
    let owner = owner_handle
        .resolve_bridge_session_id(&AgentIdentity::from("owner"))
        .await
        .expect("owner session");
    (owner_handle, owner)
}

/// A fork job bound to a plain session: the child's forker does not own it
/// (spawned_by unset), the job names a session outside every mob the
/// restarted host manages, as a library host binding a job to its own
/// session does. The session is not live at restart.
struct PlainOwnerJob {
    fixture: CouncilFixture,
    seen: SeenRequests,
    handle: meerkat_mob::MobHandle,
    owner_handle: meerkat_mob::MobHandle,
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    owner: meerkat_core::SessionId,
    job_id: String,
}

impl PlainOwnerJob {
    async fn after_restart(tag: &str) -> Self {
        let seen = SeenRequests::default();
        let fixture = CouncilFixture::new_runtime_backed({
            let seen = seen.clone();
            move |request| {
                seen.record(request);
                ScriptedTurn::Text(CHILD_REPLY.to_string())
            }
        });
        fixture.seed_source_mob(&["forker"]).await;
        let handle = fixture
            .state
            .handle_for(&fixture.source_mob_id())
            .await
            .unwrap();
        let (owner_handle, owner) = seat_unmanaged_owner(&fixture).await;
        let job_id = format!("job-plain-owner-{tag}");
        let (_fork, run) = handle
            .fork_member_then_run_detached(
                &AgentIdentity::from("forker"),
                child_spec(&format!("plain-owner-child-{tag}")),
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
        let runtime = fixture.runtime_adapter.clone().expect("runtime-backed");
        runtime
            .unregister_session(&owner)
            .await
            .expect("the owner session is not live after the restart");
        tokio::time::sleep(Duration::from_millis(5)).await;
        Self {
            fixture,
            seen,
            handle,
            owner_handle,
            runtime,
            owner,
            job_id,
        }
    }

    fn restarted_state(&self) -> Arc<meerkat_mob_mcp::MobMcpState> {
        Arc::new(meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            self.fixture.service.clone(),
            Some(Arc::clone(&self.runtime)),
            meerkat_mob::MobControlPrincipal::Owner,
        ))
    }

    fn marker(&self) -> String {
        format!("Background fork_off job {} finished (", self.job_id)
    }

    async fn records(&self) -> usize {
        completion_records(&self.fixture, &self.owner, &self.job_id).await
    }

    /// Wait until the owner's woken turn has run, then give a duplicate
    /// time to show.
    async fn await_one_wake(&self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while self.seen.turns_that_saw(&self.marker()) == 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the owner was never woken"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            self.seen.turns_that_saw(&self.marker()),
            1,
            "the owner is woken for one turn"
        );
    }
}

/// The owner of a fork job is a plain session that is not live at restart.
/// The restored mob's re-link asks the host's owner hook to make it live:
/// the outcome is recorded once and wakes the owner once.
#[tokio::test(flavor = "multi_thread")]
async fn relink_revives_a_plain_session_owner_through_the_host_hook() {
    let job = PlainOwnerJob::after_restart("hooked").await;
    let host = MobBackedOwnerHost::new(job.owner_handle.clone(), "owner");
    let restarted = job.restarted_state();
    restarted.set_detached_owner_host(Some(host.clone()));
    restarted
        .mob_insert_handle(job.fixture.source_mob_id(), job.handle.clone())
        .await;

    await_completion_record(&job.fixture, &job.owner, &job.job_id).await;
    job.await_one_wake().await;
    assert_eq!(job.records().await, 1, "recorded exactly once");
    assert!(
        completion_record_text(&job.fixture, &job.owner, &job.job_id)
            .await
            .contains(CHILD_REPLY)
    );
    assert_eq!(host.calls(), 1, "the hook made the owner live once");
    job.fixture.teardown().await;
}

/// The same owner on a host without an owner hook: the re-link cannot make
/// the session live, so nothing is recorded and the owner is not woken. The
/// job stays owed: a re-link on a host that can make the owner live
/// delivers it, once.
#[tokio::test(flavor = "multi_thread")]
async fn without_a_host_hook_a_plain_session_owner_stays_owed() {
    let job = PlainOwnerJob::after_restart("unhooked").await;
    let restarted = job.restarted_state();
    restarted
        .mob_insert_handle(job.fixture.source_mob_id(), job.handle.clone())
        .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let reports = restarted.relink_restored_fork_children().await;
    let action = reports
        .iter()
        .find(|report| report.job_id == job.job_id)
        .map(|report| report.action.clone())
        .expect("the child is visited");
    assert!(
        matches!(action, ForkRelinkAction::Failed(_)),
        "the runtime's refusal is reported: {action:?}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(job.records().await, 0, "nothing is recorded");
    assert_eq!(job.seen.turns_that_saw(&job.marker()), 0, "no wake");

    // Still owed: a host that can make the owner live delivers it.
    let host = MobBackedOwnerHost::new(job.owner_handle.clone(), "owner");
    restarted.set_detached_owner_host(Some(host.clone()));
    let reports = restarted.relink_restored_fork_children().await;
    assert_eq!(
        reports
            .iter()
            .find(|report| report.job_id == job.job_id)
            .map(|report| report.action.clone()),
        Some(ForkRelinkAction::Delivered)
    );
    await_completion_record(&job.fixture, &job.owner, &job.job_id).await;
    job.await_one_wake().await;
    assert_eq!(job.records().await, 1, "recorded exactly once");
    assert_eq!(host.calls(), 1);
    job.fixture.teardown().await;
}

/// A fork_off child (forked in its forker's turn, so the forker is its
/// spawner and a member of the mob) whose opt-in max_run elapsed while the
/// host was down is still running at restart. The forker is not live and
/// the host has no owner hook. The re-link cancels the run, delivers
/// max_run_elapsed through the forker's mob (member revival: one record, one
/// wake), and only then retires the child (lifecycle review: the owner was
/// read from the retired child's roster entry, taken for a plain session,
/// and the outcome was lost for good).
#[tokio::test(flavor = "multi_thread")]
async fn relink_past_max_run_revives_a_member_forker_that_is_not_live() {
    let seen = SeenRequests::default();
    let gate = TurnGate::new();
    let fixture = CouncilFixture::new_runtime_backed({
        let (seen, gate) = (seen.clone(), Arc::clone(&gate));
        move |request| {
            seen.record(request);
            if support::last_user_text(request).contains(CHILD_TASK) {
                ScriptedTurn::Gated(Arc::clone(&gate), CHILD_REPLY.to_string())
            } else {
                ScriptedTurn::Text("noted".to_string())
            }
        }
    });
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-overdue-member-forker".to_string();
    let child = AgentIdentity::from("overdue-member-child");
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec(child.as_str()),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::CallerTurn,
            None,
            Some(ForkJobBinding {
                job_id: job_id.clone(),
                owner_session_id: owner.clone(),
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    // The custodian dies with the "old process".
    drop(run);
    let entry = handle
        .roster()
        .await
        .get_by_identity(&child)
        .cloned()
        .expect("child seated");
    assert_eq!(
        entry.spawned_by,
        Some(AgentIdentity::from("forker")),
        "fork_off's shape: the forker spawned the child"
    );
    let mut job = entry.fork_job.expect("durable fork job record");
    // The limit elapsed while the host was down.
    job.max_run_ms = Some(now_ms().saturating_sub(job.started_at_ms).max(1));
    let runtime = fixture.runtime_adapter.clone().expect("runtime-backed");
    runtime
        .unregister_session(&owner)
        .await
        .expect("the forker is not live after the restart");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &meerkat_mob_mcp::fork_relink::RelinkDelivery {
            runtime: Some(Arc::clone(&runtime)),
            ..Default::default()
        },
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    gate.open();
    assert_eq!(action, ForkRelinkAction::Delivered);
    await_completion_record(&fixture, &owner, &job_id).await;
    let marker = format!("Background fork_off job {job_id} finished (");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while seen.turns_that_saw(&marker) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the forker was never woken"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(seen.turns_that_saw(&marker), 1, "the forker is woken once");
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(record.contains("max_run_elapsed"), "{record}");
    assert!(
        handle.get_member(&child).await.unwrap().is_none(),
        "the child is retired once its outcome is delivered"
    );
    fixture.teardown().await;
}

/// When the limit won but its outcome cannot be delivered yet (here a plain
/// session owner on a host without an owner hook), the child is cancelled
/// but not retired: it keeps its job record, so a later pass that can reach
/// the owner delivers max_run_elapsed once and retires it then. A retired
/// child with an undelivered outcome would be lost.
#[tokio::test(flavor = "multi_thread")]
async fn relink_past_max_run_keeps_the_job_while_its_outcome_cannot_be_delivered() {
    let seen = SeenRequests::default();
    let gate = TurnGate::new();
    let fixture = CouncilFixture::new_runtime_backed({
        let (seen, gate) = (seen.clone(), Arc::clone(&gate));
        move |request| {
            seen.record(request);
            if support::last_user_text(request).contains(CHILD_TASK) {
                ScriptedTurn::Gated(Arc::clone(&gate), CHILD_REPLY.to_string())
            } else {
                ScriptedTurn::Text("noted".to_string())
            }
        }
    });
    fixture.seed_source_mob(&["forker"]).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let (owner_handle, owner) = seat_unmanaged_owner(&fixture).await;
    let job_id = "job-overdue-plain-owner".to_string();
    let child = AgentIdentity::from("overdue-plain-child");
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
    let runtime = fixture.runtime_adapter.clone().expect("runtime-backed");
    runtime
        .unregister_session(&owner)
        .await
        .expect("the owner session is not live after the restart");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let hookless = meerkat_mob_mcp::fork_relink::RelinkDelivery {
        runtime: Some(Arc::clone(&runtime)),
        ..Default::default()
    };
    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &hookless,
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert!(
        matches!(action, ForkRelinkAction::Failed(_)),
        "the outcome cannot reach the owner: {action:?}"
    );
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 0);
    assert!(
        handle
            .get_member(&child)
            .await
            .unwrap()
            .and_then(|entry| entry.fork_job)
            .is_some_and(|recorded| recorded.job_id == job_id),
        "the child keeps its job record while the outcome is owed"
    );

    // A pass that can reach the owner delivers it once and retires the
    // child.
    let host = MobBackedOwnerHost::new(owner_handle, "owner");
    let hooked = meerkat_mob_mcp::fork_relink::RelinkDelivery {
        owner_host: Some(host.clone()),
        ..hookless
    };
    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &hooked,
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    gate.open();
    assert_eq!(action, ForkRelinkAction::Delivered);
    await_completion_record(&fixture, &owner, &job_id).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(record.contains("max_run_elapsed"), "{record}");
    assert!(
        handle.get_member(&child).await.unwrap().is_none(),
        "the child is retired once its outcome is delivered"
    );
    assert_eq!(host.calls(), 1);
    fixture.teardown().await;
}

/// A fixture whose children's task turns wait on `gate` and whose other
/// turns answer at once, recording every request in `seen`.
fn gated_child_fixture(seen: &SeenRequests, gate: &Arc<TurnGate>) -> CouncilFixture {
    let (seen, gate) = (seen.clone(), Arc::clone(gate));
    CouncilFixture::new_runtime_backed(move |request| {
        seen.record(request);
        if support::last_user_text(request).contains(CHILD_TASK) {
            ScriptedTurn::Gated(Arc::clone(&gate), CHILD_REPLY.to_string())
        } else {
            ScriptedTurn::Text("noted".to_string())
        }
    })
}

/// Fork `child` from the forker with a job bound to `owner`, and return the
/// job record with its limit already elapsed (it elapsed while the host was
/// down).
async fn overdue_job(
    handle: &meerkat_mob::MobHandle,
    child: &AgentIdentity,
    admission: meerkat_core::DurableForkSourceAdmission,
    job_id: &str,
    owner: &meerkat_core::SessionId,
) -> meerkat_mob::ForkJobRecord {
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec(child.as_str()),
            None,
            "fork_off_result",
            16 * 1024,
            admission,
            None,
            Some(ForkJobBinding {
                job_id: job_id.to_string(),
                owner_session_id: owner.clone(),
            }),
        )
        .await
        .expect("fork");
    // The custodian dies with the "old process".
    drop(run);
    let mut job = handle
        .roster()
        .await
        .get_by_identity(child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    job.max_run_ms = Some(now_ms().saturating_sub(job.started_at_ms).max(1));
    job
}

/// A child kept (cancelled, its job owed) by a pass whose delivery could not
/// reach the owner gets max_run_elapsed on the next pass, every time: its
/// limit has passed, so the pass does not race a status read of the now idle
/// child against a zero-length timer (lifecycle review: the idle child
/// usually answered first and was delivered restart_interrupted).
#[tokio::test(flavor = "multi_thread")]
async fn a_kept_overdue_child_gets_max_run_elapsed_on_every_later_pass() {
    const CHILDREN: usize = 6;
    let seen = SeenRequests::default();
    let gate = TurnGate::new();
    let fixture = gated_child_fixture(&seen, &gate);
    fixture.seed_source_mob(&["forker"]).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let (owner_handle, owner) = seat_unmanaged_owner(&fixture).await;
    let mut jobs = Vec::new();
    for index in 0..CHILDREN {
        let child = AgentIdentity::from(format!("kept-overdue-{index}"));
        let job = overdue_job(
            &handle,
            &child,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            &format!("job-kept-overdue-{index}"),
            &owner,
        )
        .await;
        jobs.push((child, job));
    }
    gate.wait_entered(CHILDREN).await;
    let runtime = fixture.runtime_adapter.clone().expect("runtime-backed");
    runtime
        .unregister_session(&owner)
        .await
        .expect("the owner session is not live after the restart");
    let hookless = meerkat_mob_mcp::fork_relink::RelinkDelivery {
        runtime: Some(Arc::clone(&runtime)),
        ..Default::default()
    };
    for (child, job) in &jobs {
        let action = meerkat_mob_mcp::fork_relink::relink_child(
            fixture.state.session_service(),
            &hookless,
            &fixture.source_mob_id(),
            &handle,
            child,
            job,
        )
        .await;
        assert!(matches!(action, ForkRelinkAction::Failed(_)), "{action:?}");
    }
    // The kept children are idle now; a status read answers at once.
    gate.open();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let hooked = meerkat_mob_mcp::fork_relink::RelinkDelivery {
        owner_host: Some(MobBackedOwnerHost::new(owner_handle, "owner")),
        ..hookless
    };
    for (child, job) in &jobs {
        let action = meerkat_mob_mcp::fork_relink::relink_child(
            fixture.state.session_service(),
            &hooked,
            &fixture.source_mob_id(),
            &handle,
            child,
            job,
        )
        .await;
        assert_eq!(action, ForkRelinkAction::Delivered, "{child}");
        await_completion_record(&fixture, &owner, &job.job_id).await;
        let record = completion_record_text(&fixture, &owner, &job.job_id).await;
        assert!(
            record.contains("max_run_elapsed"),
            "{child} is delivered its limit, not a settled outcome: {record}"
        );
        assert!(handle.get_member(child).await.unwrap().is_none(), "{child}");
    }
    fixture.teardown().await;
}

/// A job that ended by its limit is over, but its child must be retired. If
/// the process died (or the retire failed) after max_run_elapsed was
/// delivered, a later pass finds the job admitted, reads the committed
/// record's typed status and retires the child, with no second record.
#[tokio::test(flavor = "multi_thread")]
async fn relink_retires_a_child_left_seated_after_its_limit_was_delivered() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-limit-delivered-not-retired".to_string();
    let child = AgentIdentity::from("limit-delivered-child");
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
    let _ = run.outcome().await;
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    // max_run_elapsed was delivered; the retire after it never happened.
    let runtime = relink_runtime(&fixture).expect("runtime");
    let delivered = meerkat_mob_mcp::deliver_detached_completion(
        &runtime,
        &owner,
        "fork_off",
        &job_id,
        meerkat_core::event::BackgroundJobTerminalStatus::Terminated,
        serde_json::json!({"agent_identity": child.as_str(), "status": "max_run_elapsed"}),
    )
    .await
    .expect("deliver");
    assert_eq!(
        delivered,
        meerkat_mob_mcp::DetachedCompletionDelivered::Delivered
    );
    await_completion_record(&fixture, &owner, &job_id).await;
    assert!(handle.get_member(&child).await.unwrap().is_some());

    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(action, ForkRelinkAction::AlreadyDelivered);
    assert!(
        handle.get_member(&child).await.unwrap().is_none(),
        "the child of a job that ended by its limit is retired"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    fixture.teardown().await;
}

/// A fork_off child is owned by its forker, a member of the child's mob.
/// When the forker was respawned, the session the job was bound to is not
/// seated there any more: the owner is gone. Past the limit, the child and
/// its descendants are retired and no fork job is left (lifecycle review:
/// it was taken for a plain session, failed on a hookless host and was kept
/// forever).
#[tokio::test(flavor = "multi_thread")]
async fn relink_past_max_run_retires_a_child_whose_forker_was_respawned() {
    let seen = SeenRequests::default();
    let gate = TurnGate::new();
    let _open_on_exit = scopeguard_open(&gate);
    let fixture = gated_child_fixture(&seen, &gate);
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let child = AgentIdentity::from("respawned-forker-child");
    let job = overdue_job(
        &handle,
        &child,
        meerkat_core::DurableForkSourceAdmission::CallerTurn,
        "job-respawned-forker",
        &owner,
    )
    .await;
    gate.wait_entered(1).await;
    // A descendant of the child, forked in the child's turn.
    let grandchild = AgentIdentity::from("respawned-forker-grandchild");
    let (_fork, grandchild_run) = handle
        .fork_member_then_run_detached(
            &child,
            child_spec(grandchild.as_str()),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::CallerTurn,
            None,
            None,
        )
        .await
        .expect("fork a grandchild");
    drop(grandchild_run);
    gate.wait_entered(2).await;
    handle
        .respawn(AgentIdentity::from("forker"), None)
        .await
        .expect("respawn the forker");
    assert!(
        handle.get_member(&child).await.unwrap().is_some(),
        "the forker's respawn leaves its child seated"
    );
    assert_ne!(
        forker_session(&fixture).await,
        owner,
        "the respawned forker has a new session"
    );

    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(action, ForkRelinkAction::OwnerGone);
    assert!(handle.get_member(&child).await.unwrap().is_none());
    assert!(handle.get_member(&grandchild).await.unwrap().is_none());
    assert!(
        handle.roster().await.list().all(|entry| entry
            .fork_job
            .as_ref()
            .is_none_or(|j| j.job_id != job.job_id)),
        "no fork job is left for the gone owner"
    );
    fixture.teardown().await;
}

/// Open `gate` when the returned guard drops, so a failing test does not
/// leave gated turns hanging.
fn scopeguard_open(gate: &Arc<TurnGate>) -> impl Drop {
    struct OpenOnDrop(Arc<TurnGate>);
    impl Drop for OpenOnDrop {
        fn drop(&mut self) {
            self.0.open();
        }
    }
    OpenOnDrop(Arc::clone(gate))
}

/// A job a library host bound in mob A to a member of mob B: a host that
/// restores its mobs one by one inserts A, then B. The owner is looked up
/// among the managed mobs when the outcome is delivered, so B's member is
/// found and revived through B, once (lifecycle review: the owner mobs were
/// fixed when A was inserted, and the owner was taken for a plain session).
#[tokio::test(flavor = "multi_thread")]
async fn a_job_owner_in_a_mob_inserted_later_is_revived_through_it() {
    let seen = SeenRequests::default();
    let gate = TurnGate::new();
    let _open_on_exit = scopeguard_open(&gate);
    let fixture = gated_child_fixture(&seen, &gate);
    fixture.seed_source_mob(&["forker"]).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let (owner_handle, owner) = seat_unmanaged_owner(&fixture).await;
    let job_id = "job-owner-in-a-later-mob".to_string();
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec("later-mob-child"),
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
    drop(run);
    gate.wait_entered(1).await;
    let runtime = fixture.runtime_adapter.clone().expect("runtime-backed");
    runtime
        .unregister_session(&owner)
        .await
        .expect("the owner is not live after the restart");
    tokio::time::sleep(Duration::from_millis(5)).await;

    let restarted = Arc::new(meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
        fixture.service.clone(),
        Some(Arc::clone(&runtime)),
        meerkat_mob::MobControlPrincipal::Owner,
    ));
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await;
    restarted
        .mob_insert_handle(owner_handle.mob_id().clone(), owner_handle.clone())
        .await;
    gate.open();

    await_completion_record(&fixture, &owner, &job_id).await;
    let marker = format!("Background fork_off job {job_id} finished (");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while seen.turns_that_saw(&marker) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the owner was never woken"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(seen.turns_that_saw(&marker), 1, "woken once");
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    assert!(
        completion_record_text(&fixture, &owner, &job_id)
            .await
            .contains(CHILD_REPLY)
    );
    assert!(
        runtime.contains_session(&owner).await,
        "the owner was revived through its mob"
    );
    fixture.teardown().await;
}

/// A job in mob A (running) whose owner is a member of mob B, stopped with
/// the owner not live. Delivery defers on B, and the re-link waits on B, not
/// on the child's mob: while A runs, no attempts are spent, and when only B
/// resumes the outcome is delivered, once (lifecycle review: the wait was on
/// the child's running mob, so all attempts were spent at once and nothing
/// was left waiting when B resumed).
#[tokio::test(flavor = "multi_thread")]
async fn a_deferred_owner_in_another_mob_is_waited_on_in_its_own_mob() {
    let seen = SeenRequests::default();
    let fixture = CouncilFixture::new_runtime_backed({
        let seen = seen.clone();
        move |request| {
            seen.record(request);
            ScriptedTurn::Text(CHILD_REPLY.to_string())
        }
    });
    fixture.seed_source_mob(&["forker"]).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let (owner_handle, owner) = seat_unmanaged_owner(&fixture).await;
    let job_id = "job-owner-in-a-stopped-mob".to_string();
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec("stopped-owner-mob-child"),
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
    // The "restart": the owner's mob B comes back stopped, the owner not
    // live; the child's mob A runs.
    owner_handle.stop().await.expect("stop the owner's mob");
    let runtime = fixture.runtime_adapter.clone().expect("runtime-backed");
    runtime
        .unregister_session(&owner)
        .await
        .expect("the owner is not live after the restart");
    tokio::time::sleep(Duration::from_millis(5)).await;
    let restarted = Arc::new(meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
        fixture.service.clone(),
        Some(Arc::clone(&runtime)),
        meerkat_mob::MobControlPrincipal::Owner,
    ));
    restarted
        .mob_insert_handle(owner_handle.mob_id().clone(), owner_handle.clone())
        .await;
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await;

    // Long enough for a wait on the running mob A to spend every attempt.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 0);
    // The deferral is observed: it waits on B, the owner's mob.
    let reports = restarted.relink_restored_fork_children().await;
    assert_eq!(
        reports
            .iter()
            .find(|report| report.job_id == job_id)
            .map(|report| report.action.clone()),
        Some(ForkRelinkAction::AwaitingOwner {
            mob_id: owner_handle.mob_id().clone(),
            reason: OwnerRevivalDeferral::MobNotRunning {
                phase: meerkat_mob::MobState::Stopped,
            },
        })
    );

    // The automatic re-link's waiter is armed, on B.
    assert_eq!(
        restarted.fork_relink_waiting_owners(),
        1,
        "one deferred outcome is waiting on its owner's mob"
    );

    // Only the owner's mob resumes.
    owner_handle.resume().await.expect("resume the owner's mob");
    await_completion_record(&fixture, &owner, &job_id).await;
    let marker = format!("Background fork_off job {job_id} finished (");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while seen.turns_that_saw(&marker) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the owner was never woken"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        completion_records(&fixture, &owner, &job_id).await,
        1,
        "delivered exactly once"
    );
    assert_eq!(seen.turns_that_saw(&marker), 1, "the owner is woken once");
    assert_eq!(restarted.fork_relink_waiting_owners(), 0);
    assert!(
        completion_record_text(&fixture, &owner, &job_id)
            .await
            .contains(CHILD_REPLY)
    );
    fixture.teardown().await;
}

/// Two children of one mob whose jobs share an id (bindings are the host's;
/// nothing makes a job id unique across children), bound to owners in two
/// different mobs, both stopped with their owners not live. Each deferred
/// outcome is retried as exactly its child's job: when only C resumes, Y is
/// delivered once and X keeps waiting on B; when B resumes, X is delivered
/// once (lifecycle review: a retry selected every child with the job id and
/// adopted the first report, so a child could take the other's report).
#[tokio::test(flavor = "multi_thread")]
async fn children_sharing_a_job_id_are_each_retried_as_their_own_job() {
    let fixture =
        CouncilFixture::new_runtime_backed(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let (b_handle, b_owner) = seat_owner_in(&fixture, "owners-b").await;
    let (c_handle, c_owner) = seat_owner_in(&fixture, "owners-c").await;
    let job_id = "job-shared-id".to_string();
    // X is forked first and sorts first: a retry that took the first report
    // for the id would hand Y the report of X.
    for (child, owner) in [("aaa-shared-x", &b_owner), ("zzz-shared-y", &c_owner)] {
        let (_fork, run) = handle
            .fork_member_then_run_detached(
                &AgentIdentity::from("forker"),
                child_spec(child),
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
    }
    // The "restart": both owners' mobs come back stopped, owners not live.
    let runtime = fixture.runtime_adapter.clone().expect("runtime-backed");
    for (owner_mob, owner) in [(&b_handle, &b_owner), (&c_handle, &c_owner)] {
        owner_mob.stop().await.expect("stop the owner's mob");
        runtime
            .unregister_session(owner)
            .await
            .expect("the owner is not live after the restart");
    }
    tokio::time::sleep(Duration::from_millis(5)).await;
    let restarted = Arc::new(meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
        fixture.service.clone(),
        Some(Arc::clone(&runtime)),
        meerkat_mob::MobControlPrincipal::Owner,
    ));
    for owner_mob in [&b_handle, &c_handle] {
        restarted
            .mob_insert_handle(owner_mob.mob_id().clone(), owner_mob.clone())
            .await;
    }
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while restarted.fork_relink_waiting_owners() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "both outcomes should wait on their owners' mobs"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // Only C resumes: Y is delivered, X keeps waiting on B.
    c_handle.resume().await.expect("resume C");
    await_completion_record(&fixture, &c_owner, &job_id).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(completion_records(&fixture, &c_owner, &job_id).await, 1);
    assert!(
        completion_record_text(&fixture, &c_owner, &job_id)
            .await
            .contains("zzz-shared-y"),
        "C's owner is delivered Y's outcome"
    );
    assert_eq!(completion_records(&fixture, &b_owner, &job_id).await, 0);
    assert_eq!(
        restarted.fork_relink_waiting_owners(),
        1,
        "only X still waits, on B"
    );

    // B resumes: X is delivered, once.
    b_handle.resume().await.expect("resume B");
    await_completion_record(&fixture, &b_owner, &job_id).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(completion_records(&fixture, &b_owner, &job_id).await, 1);
    assert!(
        completion_record_text(&fixture, &b_owner, &job_id)
            .await
            .contains("aaa-shared-x"),
        "B's owner is delivered X's outcome"
    );
    assert_eq!(completion_records(&fixture, &c_owner, &job_id).await, 1);
    assert_eq!(restarted.fork_relink_waiting_owners(), 0);
    fixture.teardown().await;
}
