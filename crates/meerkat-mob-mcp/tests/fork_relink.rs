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
        "reply with the token".to_string(),
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
        .filter(|message| match message {
            meerkat_core::Message::System(system) => {
                system
                    .identity
                    .as_ref()
                    .and_then(|identity| identity.idempotency_key.as_deref())
                    == Some(format!("fork_off:{job_id}").as_str())
            }
            _ => false,
        })
        .count()
}

/// The custodian died with the old process after the child finished: the
/// re-link pass records the child's result in the forker's transcript once.
#[tokio::test]
#[ignore = "needs a runtime-backed fixture; superseded by impl2's runtime-backed re-link tests"]
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
#[ignore = "needs a runtime-backed fixture; superseded by impl2's runtime-backed re-link tests"]
async fn relink_rearms_max_run_from_the_original_start() {
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    let fixture = CouncilFixture::new(move |_| {
        ScriptedTurn::Gated(Arc::clone(&turn_gate), CHILD_REPLY.to_string())
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
        None,
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
