//! Restart re-linking of detached councils.
//!
//! A detached council's outcome is delivered to its convener by a
//! process-local task. These tests drop that task (the "restart"), rebuild the
//! state over the same durable stores, and check that the convener still gets
//! the council's outcome exactly once: the real sealed result when the council
//! finished before the restart, or a typed `coordinator_interrupted` outcome
//! once the dead coordinator's claim lease has expired.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::time::Duration;

use meerkat_mob::AgentIdentity;
use meerkat_mob::ProfileName;
use meerkat_mob::temporary_council::{TemporaryCouncilDurability, TemporaryCouncilJobBinding};
use meerkat_mob_mcp::council_relink::CouncilRelinkAction;
use meerkat_mob_mcp::temporary_council::{
    MergeBackPolicy, TemporaryCouncilBounds, TemporaryCouncilParticipantSpec,
    TemporaryCouncilRequest,
};
use support::{CouncilFixture, ScriptedTurn, council_definition, identity};

async fn member_session(fixture: &CouncilFixture, member: &str) -> meerkat_core::SessionId {
    fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("handle")
        .resolve_bridge_session_id(&AgentIdentity::from(member))
        .await
        .expect("member session")
}

/// Whether `message` is the durable completion record of council job
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
                } if recorded == job_id && tool == "council"
            )
        }),
        _ => false,
    }
}

/// Wait until job `job_id`'s completion record is in `session`: delivery
/// admits it as the convener's next turn input, which commits with that
/// turn.
async fn await_completion_records(
    fixture: &CouncilFixture,
    session: &meerkat_core::SessionId,
    job_id: &str,
) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let records = completion_records(fixture, session, job_id).await;
        if !records.is_empty() {
            return records;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the completion record never landed"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The completion records of job `job_id` in `session`, as text.
async fn completion_records(
    fixture: &CouncilFixture,
    session: &meerkat_core::SessionId,
    job_id: &str,
) -> Vec<String> {
    let persisted = <meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder> as meerkat_mob::MobSessionService>::load_persisted_session(
        fixture.service.as_ref(),
        session,
    )
    .await
    .expect("load convener session")
    .expect("convener session exists");
    persisted
        .messages()
        .iter()
        .filter(|message| is_completion_record(message, job_id))
        .map(|message| format!("{message:?}"))
        .collect()
}

fn one_participant_request(fixture: &CouncilFixture, id: &str) -> TemporaryCouncilRequest {
    TemporaryCouncilRequest::new(
        fixture.council_id(id),
        council_definition("template-is-replaced"),
        vec![TemporaryCouncilParticipantSpec::new(
            0,
            "analyst",
            fixture.source_mob_id(),
            identity("researcher"),
            identity("analyst"),
            ProfileName::from("participant"),
        )],
        "Should we ship the migration this week?",
        TemporaryCouncilBounds::relative(Duration::from_secs(240), 1, 4096),
        MergeBackPolicy::NoMerge,
    )
}

/// The council finished before the restart, but the process died before its
/// outcome reached the convener. The re-link delivers the council's REAL
/// result once, and a later re-link records nothing more.
#[tokio::test(flavor = "multi_thread")]
async fn relink_delivers_a_council_sealed_before_the_restart_exactly_once() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text("RELINKED-POSITION".to_string()));
    fixture.seed_source_mob(&["convener", "researcher"]).await;
    let owner = member_session(&fixture, "convener").await;
    let job_id = "council-job-sealed";

    // No delivery task: the council runs to its sealed result and nobody
    // records it, as if the process had died right after.
    let outcome = fixture
        .state
        .temporary_council()
        .run_detached(
            one_participant_request(&fixture, "relink-sealed"),
            TemporaryCouncilJobBinding::new(job_id, owner.clone()),
        )
        .await
        .expect("the council runs");
    assert_eq!(
        outcome.result.exit_reason,
        meerkat_mob::temporary_council::TemporaryCouncilExitReason::Completed
    );
    assert!(
        completion_records(&fixture, &owner, job_id)
            .await
            .is_empty()
    );
    let store = fixture.state.temporary_council_store_for_tests();
    let record = store
        .load(&fixture.council_id("relink-sealed"))
        .await
        .unwrap()
        .unwrap();
    let binding = record.detached_job.expect("the job is bound durably");
    assert_eq!(binding.job_id, job_id);
    assert_eq!(binding.owner_session_id, owner);
    assert!(binding.settled_at.is_none());

    tokio::time::sleep(Duration::from_millis(5)).await;
    let restarted = fixture.restart_state();
    let reports = restarted.relink_detached_councils().await;
    let report = reports
        .iter()
        .find(|report| report.council_id == fixture.council_id("relink-sealed"))
        .expect("the council is visited");
    assert_eq!(report.job_id, job_id);
    assert_eq!(report.action, CouncilRelinkAction::Delivered);

    let delivered = await_completion_records(&fixture, &owner, job_id).await;
    assert_eq!(delivered.len(), 1, "the re-link delivers exactly once");
    assert!(
        delivered[0].contains("RELINKED-POSITION") && delivered[0].contains("completed"),
        "the council's real result is delivered: {}",
        delivered[0]
    );

    // The binding is marked delivered, so a later re-link skips it.
    let record = store
        .load(&fixture.council_id("relink-sealed"))
        .await
        .unwrap()
        .unwrap();
    assert!(
        record
            .detached_job
            .as_ref()
            .is_some_and(|job| job.settled_at.is_some()),
        "a delivered job is marked durably"
    );
    assert!(restarted.relink_detached_councils().await.is_empty());
    assert_eq!(completion_records(&fixture, &owner, job_id).await.len(), 1);
    fixture.teardown().await;
}

/// Write the record of a council whose coordinator died before sealing:
/// created by the previous process, bound to the convener's detached job,
/// with the dead coordinator's claim lease running for another ten minutes.
async fn write_interrupted_council(
    store: &std::sync::Arc<dyn meerkat_mob::store::TemporaryCouncilStore>,
    council_id: &meerkat_mob::temporary_council::TemporaryCouncilId,
    job_id: &str,
    owner: &meerkat_core::SessionId,
) -> chrono::DateTime<chrono::Utc> {
    use meerkat_mob::machines::temporary_council_lifecycle::{
        TemporaryCouncilLifecycleInput, TemporaryCouncilLifecycleMachineAuthority,
        TemporaryCouncilLifecycleMachineMutator,
    };
    use meerkat_mob::store::TemporaryCouncilRecord;

    let fingerprint = format!("tcf1:sha256:{council_id}");
    let mut authority = TemporaryCouncilLifecycleMachineAuthority::new();
    TemporaryCouncilLifecycleMachineMutator::apply(
        &mut authority,
        TemporaryCouncilLifecycleInput::Open {
            request_fingerprint: fingerprint.clone(),
        },
    )
    .expect("open the council record");
    TemporaryCouncilLifecycleMachineMutator::apply(
        &mut authority,
        TemporaryCouncilLifecycleInput::Claim {
            claim_id: "coordinator-of-the-dead-process".to_string(),
            lease_expired: false,
        },
    )
    .expect("the previous process's coordinator claims it");
    let created = chrono::Utc::now() - chrono::Duration::seconds(5);
    let lease = chrono::Utc::now() + chrono::Duration::seconds(600);
    store
        .insert_new(&TemporaryCouncilRecord {
            council_id: council_id.clone(),
            request_fingerprint: fingerprint,
            temporary_mob_id: council_id.temporary_mob_id(),
            deadline: created + chrono::Duration::seconds(600),
            machine_state: authority.state().clone(),
            durability: TemporaryCouncilDurability::Durable,
            claim_lease_expires_at: lease,
            participants: Vec::new(),
            exchanges: Vec::new(),
            result: None,
            cleanup: None,
            detached_job: Some(TemporaryCouncilJobBinding::new(job_id, owner.clone())),
            revision: 0,
            created_at: created,
            updated_at: created,
        })
        .await
        .expect("write the crashed council record");
    lease
}

/// Wait until the council's detached job is settled and return its record.
async fn await_settled(
    store: &std::sync::Arc<dyn meerkat_mob::store::TemporaryCouncilStore>,
    council_id: &meerkat_mob::temporary_council::TemporaryCouncilId,
) -> meerkat_mob::store::TemporaryCouncilRecord {
    for _ in 0..600 {
        let record = store.load(council_id).await.unwrap().unwrap();
        if record
            .detached_job
            .as_ref()
            .is_some_and(|job| job.settled_at.is_some())
        {
            return record;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the council's detached job was never settled");
}

/// The process died mid-council and the host restarted INSIDE the dead
/// coordinator's claim lease (the common case). Restoration alone arms the
/// sweep: its first pass must skip the held record (reported, not silent),
/// and the sweep retries once the lease expires. Councils are never
/// re-executed, so the retry seals a typed `coordinator_interrupted` outcome
/// and the re-link delivers it to the convener once.
#[tokio::test(flavor = "multi_thread")]
async fn restart_inside_the_lease_retries_and_delivers_the_interrupted_council() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text("unused".to_string()));
    fixture.seed_source_mob(&["convener"]).await;
    let owner = member_session(&fixture, "convener").await;
    let job_id = "council-job-interrupted";
    // Quiesce this process's mob actors before another process opens the
    // same durable stores; a real restart would have destroyed them.
    if let Ok(handle) = fixture.state.handle_for(&fixture.source_mob_id()).await {
        handle.shutdown().await.expect("quiesce the source mob");
    }

    let restarted = fixture.restart_state();
    let store = restarted.temporary_council_store_for_tests();
    let council_id = fixture.council_id("relink-interrupted");
    let lease = write_interrupted_council(&store, &council_id, job_id, &owner).await;

    // Any ordinary mob verb restores the state; no re-link verb is called.
    let _ = restarted.mob_handles_snapshot().await;
    // The dead coordinator still holds the lease: nothing is sealed or
    // delivered yet.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        completion_records(&fixture, &owner, job_id)
            .await
            .is_empty()
    );
    assert!(
        store
            .load(&council_id)
            .await
            .unwrap()
            .unwrap()
            .result
            .is_none()
    );
    // A sweep reports the held record with the lease it waits for.
    let sweep = restarted
        .temporary_council()
        .sweep_unfinished()
        .await
        .expect("sweep");
    assert!(sweep.recovered.is_empty());
    let held = sweep
        .held
        .iter()
        .find(|held| held.council_id == council_id)
        .expect("the held record is reported");
    assert_eq!(held.claim_lease_expires_at, lease);

    // The lease expires: the sweep's follow-up pass takes over.
    restarted.set_temporary_council_clock_offset(chrono::Duration::seconds(700));
    let record = await_settled(&store, &council_id).await;
    assert_eq!(
        record.result.expect("sealed").exit_reason,
        meerkat_mob::temporary_council::TemporaryCouncilExitReason::CoordinatorInterrupted
    );
    let delivered = await_completion_records(&fixture, &owner, job_id).await;
    assert_eq!(delivered.len(), 1);
    assert!(
        delivered[0].contains("coordinator_interrupted"),
        "the typed interrupted outcome is delivered: {}",
        delivered[0]
    );

    // Running it again records nothing more.
    assert!(restarted.relink_detached_councils().await.is_empty());
    assert_eq!(completion_records(&fixture, &owner, job_id).await.len(), 1);
    fixture.teardown().await;
}

/// A host built like MobKit: no persistent root, a durable council store it
/// supplies itself, `into_shared`, and mobs restored by inserting their
/// handles. That alone runs the council sweep after restore, including the
/// retry after a held lease and the detached-council re-link.
#[tokio::test(flavor = "multi_thread")]
async fn a_mobkit_style_host_recovers_and_relinks_councils_after_restore() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text("unused".to_string()));
    fixture.seed_source_mob(&["convener"]).await;
    let owner = member_session(&fixture, "convener").await;
    let job_id = "council-job-mobkit";
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("source mob handle");

    let council_store: std::sync::Arc<dyn meerkat_mob::store::TemporaryCouncilStore> =
        std::sync::Arc::new(
            meerkat_mob::store::SqliteTemporaryCouncilStore::open(
                meerkat_mob_mcp::MobMcpState::persistent_forked_participant_store_path(
                    &fixture.root.join("state"),
                ),
            )
            .expect("open the durable council store"),
        );
    let council_id = fixture.council_id("relink-mobkit");
    write_interrupted_council(&council_store, &council_id, job_id, &owner).await;

    // The restarted host: no persistent root, only the durable council store.
    let restarted = meerkat_mob_mcp::MobMcpState::new(
        fixture.service.clone(),
        meerkat_mob::MobControlPrincipal::Owner,
    )
    .with_temporary_council_store(council_store.clone())
    .into_shared();
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle)
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        council_store
            .load(&council_id)
            .await
            .unwrap()
            .unwrap()
            .result
            .is_none(),
        "the dead coordinator's lease still holds the record"
    );

    restarted.set_temporary_council_clock_offset(chrono::Duration::seconds(700));
    let record = await_settled(&council_store, &council_id).await;
    assert_eq!(
        record.result.expect("sealed").exit_reason,
        meerkat_mob::temporary_council::TemporaryCouncilExitReason::CoordinatorInterrupted
    );
    let delivered = await_completion_records(&fixture, &owner, job_id).await;
    assert_eq!(delivered.len(), 1);
    assert!(delivered[0].contains("coordinator_interrupted"));
    fixture.teardown().await;
}
