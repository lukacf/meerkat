//! Restart re-linking of fork_off children.
//!
//! A detached child's outcome is delivered by a process-local custodian. These
//! tests drop that custodian (the "restart"), rebuild the mob state over the
//! same durable stores, and check that the re-link pass delivers the outcome
//! exactly once, and re-arms the opt-in max_run limit from the original start.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::{
    AgentIdentity, ForkChildRunOutcome, ForkJobBinding, ProfileName, SpawnMemberSpec,
};
use meerkat_mob_mcp::fork_relink::{ChildRetirement, ForkRelinkAction, InjectedChildRetirement};
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
    fixture
        .state
        .session_service()
        .acquire_runtime_adapter(None)
        .expect("acquire runtime authority")
}

/// The re-link delivery for the fixture's runtime: no owner hook, and only
/// the child's own mob to find the owner in.
fn relink_delivery(fixture: &CouncilFixture) -> meerkat_mob_mcp::fork_relink::RelinkDelivery {
    meerkat_mob_mcp::fork_relink::RelinkDelivery::from_state(&fixture.state).without_managed_mobs()
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
                retained_work: None,
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
    // the surviving mob the way Meta restores it, by inserting its handle.
    // That triggers the re-link automatically.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let restarted = Arc::new(
        meerkat_mob_mcp::MobMcpState::new(
            fixture.service.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("construct runtime authority"),
    );
    fixture.bind_continuations(&restarted);
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

/// The compactor's summary of a compacted child.
const COMPACTION_SUMMARY: &str = "COMPACTED-HANDOFF-SUMMARY-7Q";
/// Prompts of the forker's own turns before the fork.
const WARMUP_PROMPT: &str = "forker warm-up turn";

/// The kinds of a persisted transcript's rows in order, naming the typed
/// compaction summary and the row that holds the child's reply.
fn transcript_row_kinds(session: &meerkat_core::Session) -> Vec<&'static str> {
    session
        .messages()
        .iter()
        .map(|message| match message {
            meerkat_core::Message::System(_) => "System",
            meerkat_core::Message::SystemNotice(_) => "SystemNotice",
            meerkat_core::Message::User(user) if user.transcript_role.is_compaction_summary() => {
                "CompactionSummary"
            }
            meerkat_core::Message::User(_) => "User",
            meerkat_core::Message::BlockAssistant(_)
                if format!("{message:?}").contains(CHILD_REPLY) =>
            {
                "BlockAssistant(reply)"
            }
            meerkat_core::Message::BlockAssistant(_) => "BlockAssistant",
            meerkat_core::Message::ToolResults { .. } => "ToolResults",
        })
        .collect()
}

/// The child's job turn compacted its transcript (a tool round trip, then
/// compaction before the next provider call), so the transcript no longer
/// holds the fork prefix the job record indexes from. The re-link still
/// delivers the child's real reply, read from the runtime's durable
/// completion receipt for the job turn, with the turn's usage and counts.
#[tokio::test]
async fn relink_delivers_the_reply_of_a_child_whose_turn_compacted() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let compactions = Arc::new(AtomicUsize::new(0));
    let tool_called = Arc::new(AtomicBool::new(false));
    let (seen_compactions, seen_tool_call) = (Arc::clone(&compactions), Arc::clone(&tool_called));
    // The forker runs more warm-up turns than compaction retains verbatim,
    // each replying with 8 KiB: far below the forker's own compaction
    // threshold, but any retained tail holding one exceeds the compacting
    // child's verbatim retention budget, so the child's rewrite has to drop
    // its fork prefix. The child's first provider call requests a tool and
    // its next one replies; the compactor's summary request (whose prompt is
    // the request's last user text) is answered with a summary.
    let fixture = CouncilFixture::new(move |request| {
        let last_user = support::last_user_text(request);
        if last_user.contains("CONTEXT COMPACTION") {
            seen_compactions.fetch_add(1, Ordering::SeqCst);
            ScriptedTurn::Text(COMPACTION_SUMMARY.to_string())
        } else if last_user.contains(WARMUP_PROMPT) {
            ScriptedTurn::Text("w".repeat(8 * 1024))
        } else if !seen_tool_call.swap(true, Ordering::SeqCst) {
            ScriptedTurn::ToolCall {
                id: "toolu_relink_compaction".to_string(),
                name: "peers".to_string(),
                args: serde_json::json!({}),
            }
        } else {
            ScriptedTurn::Text(CHILD_REPLY.to_string())
        }
    });
    let mob_id = fixture.source_mob_id();
    let mut definition = support::council_definition(mob_id.as_str());
    let mut compacting = support::participant_profile("compacting fork child");
    compacting.auto_compact_threshold = std::num::NonZeroU64::new(1);
    definition.profiles.insert(
        ProfileName::from("compacting"),
        meerkat_mob::ProfileBinding::Inline(Box::new(compacting)),
    );
    fixture.seed_source_mob_from(definition, &["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture.state.handle_for(&mob_id).await.unwrap();
    // The forker's own history is the child's fork prefix: more complete
    // turns than compaction retains, so the rewrite has turns to summarize
    // away.
    let warm_up_turns =
        meerkat_core::config::CompactionRuntimeConfig::default().recent_turn_budget + 2;
    for turn in 0..warm_up_turns {
        drive_turn(&handle, "forker", &format!("{WARMUP_PROMPT} {turn}")).await;
    }
    let job_id = "job-compacted-before-restart".to_string();
    let mut spec = child_spec("compacted-child");
    spec.role_name = ProfileName::from("compacting");

    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            spec,
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: job_id.clone(),
                owner_session_id: owner.clone(),
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    // No custodian: the process died right after the child finished.
    let Some(ForkChildRunOutcome::Completed(turn)) = run.outcome().await else {
        panic!("the child's job turn completed");
    };
    assert_eq!(turn.result().result().text(), CHILD_REPLY);
    assert_eq!(turn.result().tool_calls(), 1, "one tool round trip");
    assert!(tool_called.load(Ordering::SeqCst));
    assert!(
        compactions.load(Ordering::SeqCst) >= 1,
        "the child's job turn compacted its transcript"
    );

    // The scenario the fix is for: the compacted transcript no longer holds
    // the reply where the record's fork prefix says the job's exchange
    // starts.
    let child = AgentIdentity::from("compacted-child");
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    assert!(
        job.turn_delivery.is_some(),
        "a runtime-backed host records the turn's delivery identity"
    );
    let child_session = handle
        .resolve_bridge_session_id(&child)
        .await
        .expect("child session");
    let transcript = <meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder> as meerkat_mob::MobSessionService>::load_persisted_session(
        fixture.service.as_ref(),
        &child_session,
    )
    .await
    .unwrap()
    .expect("child session exists");
    // A summary request alone does not prove the rewrite committed: the
    // persisted transcript must carry the compactor's typed summary.
    assert!(
        transcript.messages().iter().any(|message| matches!(
            message,
            meerkat_core::Message::User(user)
                if user.transcript_role
                    == meerkat_core::types::TranscriptUserRole::CompactionSummary
                    && user.text_content().contains(COMPACTION_SUMMARY)
        )),
        "the child's persisted transcript holds no typed compaction summary; rows: {:?}",
        transcript_row_kinds(&transcript)
    );
    let from_prefix = job
        .durable_terminal_result(&transcript)
        .expect("bounded")
        .map(|result| result.text().to_string());
    assert_ne!(
        from_prefix.as_deref(),
        Some(CHILD_REPLY),
        "compaction left the fork prefix intact (prefix {} of {} messages), so this \
         scenario does not exercise the receipt read",
        job.prefix_message_count,
        transcript.messages().len()
    );

    let restarted = Arc::new(
        meerkat_mob_mcp::MobMcpState::new(
            fixture.service.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("construct runtime authority"),
    );
    fixture.bind_continuations(&restarted);
    restarted
        .mob_insert_handle(mob_id.clone(), handle.clone())
        .await;
    await_completion_record(&fixture, &owner, &job_id).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    let outcome = completion_record_outcome(&fixture, &owner, &job_id).await;
    assert_eq!(outcome["status"], "completed", "{outcome}");
    assert_eq!(outcome["bounded_result"]["text"], CHILD_REPLY, "{outcome}");
    // The receipt carries the same turn result the live custodian reports.
    assert_eq!(outcome["turns"], turn.result().turns(), "{outcome}");
    assert_eq!(
        outcome["tool_calls"],
        turn.result().tool_calls(),
        "{outcome}"
    );
    assert_eq!(
        outcome["usage"],
        serde_json::to_value(turn.result().usage()).unwrap(),
        "{outcome}"
    );
    fixture.teardown().await;
}

/// The outcome JSON job `job_id`'s completion record in `session` carries.
async fn completion_record_outcome(
    fixture: &CouncilFixture,
    session: &meerkat_core::SessionId,
    job_id: &str,
) -> serde_json::Value {
    let persisted = <meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder> as meerkat_mob::MobSessionService>::load_persisted_session(
        fixture.service.as_ref(),
        session,
    )
    .await
    .expect("load forker session")
    .expect("forker session exists");
    let detail = persisted
        .messages()
        .iter()
        .filter(|message| is_completion_record(message, job_id))
        .find_map(|message| match message {
            meerkat_core::Message::SystemNotice(notice) => {
                notice.blocks.iter().find_map(|block| match block {
                    meerkat_core::types::SystemNoticeBlock::BackgroundJob { detail, .. } => {
                        detail.clone()
                    }
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("the completion record carries its outcome");
    serde_json::from_str(&detail).expect("the outcome is JSON")
}

/// A record written before the job turn's delivery identity was recorded
/// (`turn_delivery` absent) still has its finished child's reply delivered,
/// read from the child's transcript after the fork prefix.
#[tokio::test]
async fn relink_of_a_record_without_a_turn_delivery_reads_the_transcript() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-recorded-before-turn-delivery".to_string();
    let child = AgentIdentity::from("legacy-record-child");
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
                retained_work: None,
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
    assert!(job.turn_delivery.is_some());
    // The record as an older host wrote it.
    job.turn_delivery = None;

    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await
    .action;
    assert_eq!(action, ForkRelinkAction::Delivered);
    await_completion_record(&fixture, &owner, &job_id).await;
    let outcome = completion_record_outcome(&fixture, &owner, &job_id).await;
    assert_eq!(outcome["status"], "completed", "{outcome}");
    assert_eq!(outcome["bounded_result"]["text"], CHILD_REPLY, "{outcome}");
    fixture.teardown().await;
}

/// Run one exact turn of `member` with `prompt`.
/// The custodian dies with the "old process": the run's supervisor ends
/// where it waits, as a process exit ends it, before the gated turn reaches
/// an outcome. Nothing but the re-link then acts on the turn's end: a live
/// supervisor would settle or retire the child itself (for a failed turn,
/// `retire_failed_fork_child`), racing what the re-link decides. The child,
/// its session and the runtime stay, as a restart finds them. Every caller
/// holds the turn in its `TurnGate`, so the supervisor cannot have settled.
async fn end_old_process_supervisor(run: meerkat_mob::ForkChildRun) {
    assert_eq!(
        run.end_supervisor_as_process_exit_for_test().await,
        meerkat_mob::ForkSupervisorExitForTest::EndedBeforeOutcome,
        "the gated turn has no outcome yet"
    );
}

async fn drive_turn(handle: &meerkat_mob::MobHandle, member: &str, prompt: &str) {
    let spec = meerkat_mob::BoundedResultSpec::new("warm-up", 4096).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from(member),
            meerkat_mob::WorkSpec::new(
                meerkat_core::types::ContentInput::Text(prompt.to_string()),
                meerkat_mob::WorkOrigin::Internal,
            ),
            meerkat_core::types::HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .unwrap_or_else(|error| panic!("start a turn for {member}: {error}"));
    tokio::time::timeout(Duration::from_secs(60), work.wait_bounded(spec))
        .await
        .expect("the turn finished in time")
        .unwrap_or_else(|error| panic!("turn for {member} failed: {error:?}"));
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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    end_old_process_supervisor(run).await;

    // The durable record as a restarted host reads it. Its original start is
    // five minutes back, and its limit ends 800 ms from now when measured from
    // that start; measured from the re-link instead, the same limit would end
    // five minutes later.
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
    job.started_at_ms = job.started_at_ms.saturating_sub(300_000);
    job.max_run_ms = Some(now_ms().saturating_sub(job.started_at_ms) + 800);
    let deadline_ms = job.started_at_ms + job.max_run_ms.unwrap();

    // The limit's decision is observed as the job's completion record landing
    // in the forker's transcript, concurrently with the re-link: delivery
    // follows the decision, while the re-link goes on to retire the child,
    // which waits out the member retire grace. That retirement is cleanup,
    // bounded only by its own hang guard. The record must land within
    // `await_completion_record`'s 30 s, far inside the five minutes a limit
    // reset at the re-link would add.
    let relink = async {
        tokio::time::timeout(
            Duration::from_secs(120),
            meerkat_mob_mcp::fork_relink::relink_child(
                fixture.state.session_service(),
                &relink_delivery(&fixture),
                &fixture.source_mob_id(),
                &handle,
                &child,
                &job,
            ),
        )
        .await
        .expect("the re-link's retirement settles; the bound is a hang guard only")
    };
    let recorded = async {
        await_completion_record(&fixture, &owner, &job_id).await;
        now_ms()
    };
    let (outcome, recorded_ms) = tokio::join!(relink, recorded);
    assert_eq!(outcome.action, ForkRelinkAction::Delivered);
    // The record cannot land before the limit decides, and the limit decides
    // only once its deadline has passed: a record seen earlier means the
    // limit decided early.
    assert!(
        recorded_ms >= deadline_ms,
        "the limit's record landed {} ms before its deadline",
        deadline_ms - recorded_ms
    );
    let outcome = completion_record_outcome(&fixture, &owner, &job_id).await;
    assert_eq!(outcome["status"], "max_run_elapsed", "{outcome}");
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
                retained_work: None,
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

    let outcome = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(outcome.action, ForkRelinkAction::Delivered);
    assert_eq!(
        outcome.child_retirement,
        ChildRetirement::NotRequired,
        "a completed job's outcome retires nothing"
    );
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
    .await
    .action;
    assert_eq!(action, ForkRelinkAction::AlreadyDelivered);
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    assert!(handle.get_member(&child).await.unwrap().is_some());
    fixture.teardown().await;
}

/// A child whose job is past its limit at the restart, and whose durable
/// reply cannot be read there, is not settled on an absence nobody observed:
/// the re-link delivers `restart_interrupted` and keeps the child seated,
/// instead of reporting `max_run_elapsed` and retiring a child that may have
/// replied. The record carries no turn delivery identity (an older host, or
/// one without a runtime, wrote it), so the reply is read from the child's
/// transcript, the read that fails here. RED before: the failed read was
/// taken for "no reply", so the limit settled the job and retired the child.
#[tokio::test]
async fn relink_past_max_run_with_an_unreadable_reply_keeps_the_child_seated() {
    let store = Arc::new(support::commit_gate::CommitGateRuntimeStore::new());
    let fixture = CouncilFixture::new_with_runtime_store(
        |_| ScriptedTurn::Text(CHILD_REPLY.to_string()),
        store.clone(),
    );
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-unreadable-reply".to_string();
    let child = AgentIdentity::from("relink-unreadable-child");

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
                retained_work: None,
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
    assert!(job.turn_delivery.is_some());
    // The record as an older host wrote it: settled from the transcript.
    job.turn_delivery = None;
    job.max_run_ms = Some(now_ms().saturating_sub(job.started_at_ms).max(1));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(now_ms() > job.started_at_ms + job.max_run_ms.unwrap());

    // The child's session cannot be read at the restart.
    let child_session = handle
        .resolve_bridge_session_id(&child)
        .await
        .expect("child session");
    store.fail_session_reads(meerkat_runtime::LogicalRuntimeId::for_session(
        &child_session,
    ));
    let outcome = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    store.restore_session_reads();
    assert_eq!(outcome.action, ForkRelinkAction::Delivered);
    assert_eq!(
        outcome.child_retirement,
        ChildRetirement::NotRequired,
        "an unreadable reply retires no child"
    );
    await_completion_record(&fixture, &owner, &job_id).await;
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(
        record.contains("restart_interrupted") && !record.contains("max_run_elapsed"),
        "an unreadable reply is inconclusive, never the limit's verdict: {record}"
    );
    assert!(
        handle.get_member(&child).await.unwrap().is_some(),
        "the child stays seated for its forker"
    );
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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    end_old_process_supervisor(run).await;

    let mut job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    // The limit is already past on the record itself: a start a minute back
    // against a 1 ms limit, so no clock has to advance before the re-link.
    job.started_at_ms = job.started_at_ms.saturating_sub(60_000);
    job.max_run_ms = Some(1);

    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await
    .action;
    // The child's turn is still held by its gate, which opens only below:
    // the re-link returning at all proves the elapsed limit applied without
    // waiting for the run. No wall-clock margin is involved.
    assert_eq!(action, ForkRelinkAction::Delivered);
    assert!(handle.get_member(&child).await.unwrap().is_none());
    await_completion_record(&fixture, &owner, &job_id).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(record.contains("max_run_elapsed"), "{record}");
    assert!(!record.contains(CHILD_REPLY), "{record}");
    gate.open();
    fixture.teardown().await;
}

/// A required child retirement that fails once is retried by the re-link
/// (#1391): the child past its max_run is gone when the re-link returns.
#[tokio::test]
async fn relink_retries_a_failed_retirement_of_a_child_past_max_run() {
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
    let job_id = "job-past-limit-retire-retry".to_string();
    let child = AgentIdentity::from("relink-overdue-child-retire-retry");

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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    end_old_process_supervisor(run).await;

    let mut job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    job.started_at_ms = job.started_at_ms.saturating_sub(60_000);
    job.max_run_ms = Some(1);

    // The first retirement the re-link makes fails before it reaches the mob.
    let _ = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture).with_child_retirement_faults(1),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await
    .action;
    assert!(
        handle.get_member(&child).await.unwrap().is_none(),
        "the re-link retries a failed required retirement before it returns"
    );
    await_completion_record(&fixture, &owner, &job_id).await;
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(record.contains("max_run_elapsed"), "{record}");
    gate.open();
    fixture.teardown().await;
}

/// A required child retirement that keeps failing is reported typed after
/// the re-link's bounded retries, the child stays seated with its job
/// record, and a later pass retires it (#1391).
#[tokio::test]
async fn relink_reports_a_child_retirement_that_keeps_failing() {
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
    let job_id = "job-past-limit-retire-fails".to_string();
    let child = AgentIdentity::from("relink-overdue-child-retire-fails");

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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    end_old_process_supervisor(run).await;

    let mut job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    job.started_at_ms = job.started_at_ms.saturating_sub(60_000);
    job.max_run_ms = Some(1);

    // Every attempt the re-link makes (the retirement and its two retries)
    // fails before it reaches the mob.
    let outcome = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture).with_child_retirement_faults(3),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(outcome.action, ForkRelinkAction::Delivered);
    assert!(
        matches!(
            &outcome.child_retirement,
            ChildRetirement::Failed { error }
                if error.contains("injected re-link child retirement failure")
        ),
        "a retirement that keeps failing is reported: {:?}",
        outcome.child_retirement
    );
    assert!(
        handle.get_member(&child).await.unwrap().is_some(),
        "the child stays seated with its job record for the next pass"
    );

    // The next pass finds the delivered outcome and retires the child.
    let outcome = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(outcome.action, ForkRelinkAction::AlreadyDelivered);
    assert_eq!(outcome.child_retirement, ChildRetirement::Retired);
    assert!(handle.get_member(&child).await.unwrap().is_none());
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    gate.open();
    fixture.teardown().await;
}

/// A retirement still in progress (a saga whose runtime teardown did not
/// converge keeps its teardown authority for a retry) is retried within the
/// budget like a failure: retrying resumes the saga, and the child is retired
/// when the re-link returns (#1391).
#[tokio::test]
async fn relink_retries_a_child_retirement_still_in_progress() {
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
    let job_id = "job-past-limit-retire-in-progress".to_string();
    let child = AgentIdentity::from("relink-overdue-child-retire-in-progress");

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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    end_old_process_supervisor(run).await;

    let mut job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    job.started_at_ms = job.started_at_ms.saturating_sub(60_000);
    job.max_run_ms = Some(1);

    // The first retirement answers that it is still in progress; the retry
    // reaches the mob.
    let outcome = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture)
            .with_child_retirement_script([InjectedChildRetirement::InProgress]),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(outcome.action, ForkRelinkAction::Delivered);
    assert_eq!(
        outcome.child_retirement,
        ChildRetirement::Retired,
        "a retirement still in progress is retried, and the retry finishes it"
    );
    assert!(handle.get_member(&child).await.unwrap().is_none());
    gate.open();
    fixture.teardown().await;
}

/// A retry that hangs is dropped when the re-link's retry budget runs out and
/// reported failed; the child stays seated, and the next pass retires it
/// without a second completion record (#1391).
#[tokio::test]
async fn relink_drops_a_hanging_retirement_retry_at_its_budget() {
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
    let job_id = "job-past-limit-retire-hangs".to_string();
    let child = AgentIdentity::from("relink-overdue-child-retire-hangs");

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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    end_old_process_supervisor(run).await;

    let mut job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    job.started_at_ms = job.started_at_ms.saturating_sub(60_000);
    job.max_run_ms = Some(1);

    // The retirement fails, and its retry never answers.
    let outcome = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture).with_child_retirement_script([
            InjectedChildRetirement::Fail,
            InjectedChildRetirement::Hang,
        ]),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(outcome.action, ForkRelinkAction::Delivered);
    assert!(
        matches!(
            &outcome.child_retirement,
            ChildRetirement::Failed { error } if error.contains("retry budget")
        ),
        "a hanging retry is dropped at the budget and reported: {:?}",
        outcome.child_retirement
    );
    assert!(handle.get_member(&child).await.unwrap().is_some());

    // The next pass retires the child, delivering nothing twice.
    let outcome = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await;
    assert_eq!(outcome.action, ForkRelinkAction::AlreadyDelivered);
    assert_eq!(outcome.child_retirement, ChildRetirement::Retired);
    assert!(handle.get_member(&child).await.unwrap().is_none());
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
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
                retained_work: None,
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
    .await
    .action;
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
                retained_work: None,
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
        .await
        .action,
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
    .await
    .action;
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
                retained_work: None,
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
/// them. Each settles on its own task, and their status reads run
/// concurrently against the mob-wide observation capacity while each child's
/// session is busy with its turn. A read that does not observe a child (it
/// is refused after the bounded capacity wait, or neither the status nor the
/// runtime says whether the child's run is open) is read again, never taken
/// for an idle child
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
                    retained_work: None,
                }),
            )
            .await
            .expect("fork");
        end_old_process_supervisor(run).await;
        jobs.push(job_id);
    }
    gate.wait_entered(3).await;
    tokio::time::sleep(Duration::from_millis(5)).await;

    let restarted = Arc::new(
        meerkat_mob_mcp::MobMcpState::new(
            fixture.service.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("construct runtime authority"),
    );
    fixture.bind_continuations(&restarted);
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

/// Member status reads a child's live agent, which is terminal before the
/// service commits the turn, while the re-link reads the outcome from the
/// durable transcript. Holding the child's boundary commit opens that window
/// deterministically: the child reads idle while its reply exists only in the
/// live transcript. The re-link waits for the commit and delivers the real
/// reply; reading the store in the window delivered `restart_interrupted` for
/// a child that had answered (nightly 36275543570, ~10% on a 4-vCPU runner).
#[tokio::test(flavor = "multi_thread")]
async fn relink_waits_for_a_finished_child_turn_to_commit() {
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    let store = Arc::new(support::commit_gate::CommitGateRuntimeStore::new());
    let fixture = CouncilFixture::new_with_runtime_store(
        move |request| {
            if support::last_user_text(request).contains(CHILD_TASK) {
                ScriptedTurn::Gated(Arc::clone(&turn_gate), CHILD_REPLY.to_string())
            } else {
                ScriptedTurn::Text("noted".to_string())
            }
        },
        store.clone(),
    );
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let child = "commit-window-child";
    let job_id = "job-commit-window".to_string();
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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    end_old_process_supervisor(run).await;
    gate.wait_entered(1).await;
    let child_session = handle
        .resolve_bridge_session_id(&AgentIdentity::from(child))
        .await
        .expect("child session");
    // The fact the re-link settles on, exactly as MeerkatMachine documents
    // it: true for the whole active turn.
    let runtime =
        meerkat_mob::MobSessionService::acquire_runtime_adapter(fixture.service.as_ref(), None)
            .expect("acquire runtime authority")
            .expect("the service derives its runtime");
    assert!(
        runtime
            .session_has_uncommitted_run_input(&child_session)
            .await
            .unwrap(),
        "an active turn's input awaits its boundary"
    );
    store.arm(meerkat_runtime::LogicalRuntimeId::for_session(
        &child_session,
    ));

    let restarted = Arc::new(
        meerkat_mob_mcp::MobMcpState::new(
            fixture.service.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("construct runtime authority"),
    );
    fixture.bind_continuations(&restarted);
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await;
    // The turn ends; its commit is held at the store.
    gate.open();
    store.entered().await;
    assert!(
        tokio::time::timeout(
            Duration::from_secs(1),
            runtime.session_has_uncommitted_run_input(&child_session),
        )
        .await
        .is_err(),
        "the read queues behind the boundary commit in progress"
    );
    // Terminal in its live agent, reply not durable: many re-link
    // observations of exactly the state that used to be read as idle. The
    // window outlasts the re-link's bounded run-input read, so a read that
    // timed out behind the held commit is inconclusive and must never be
    // taken as settled.
    tokio::time::sleep(Duration::from_secs(8)).await;
    let premature = if completion_records(&fixture, &owner, &job_id).await > 0 {
        Some(completion_record_text(&fixture, &owner, &job_id).await)
    } else {
        None
    };
    store.release();
    assert_eq!(
        premature, None,
        "the re-link reported the child before its finished turn was committed"
    );
    await_completion_record(&fixture, &owner, &job_id).await;
    assert!(
        !runtime
            .session_has_uncommitted_run_input(&child_session)
            .await
            .unwrap(),
        "no input awaits a boundary once the commit landed"
    );
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(
        record.contains(CHILD_REPLY) && !record.contains("restart_interrupted"),
        "the child is delivered its real reply once its turn is committed: {record}"
    );
    fixture.teardown().await;
}

/// A child whose turn fails leaves no reply. Its failed turn must not leave
/// the re-link waiting for a commit that never comes: the child settles, and
/// its outcome is delivered as failed.
#[tokio::test(flavor = "multi_thread")]
async fn relink_settles_a_running_child_whose_turn_fails() {
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    let fixture = CouncilFixture::new(move |request| {
        if support::last_user_text(request).contains(CHILD_TASK) {
            ScriptedTurn::GatedFail(Arc::clone(&turn_gate), "provider rejected the turn".into())
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
    let job_id = "job-failing-turn".to_string();
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec("failing-child"),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: job_id.clone(),
                owner_session_id: owner.clone(),
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    end_old_process_supervisor(run).await;
    gate.wait_entered(1).await;

    let restarted = Arc::new(
        meerkat_mob_mcp::MobMcpState::new(
            fixture.service.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("construct runtime authority"),
    );
    fixture.bind_continuations(&restarted);
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await;
    // Observed running, then the turn fails with no reply.
    gate.open();
    await_completion_record(&fixture, &owner, &job_id).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(
        record.contains("finished (failed)") && !record.contains(CHILD_REPLY),
        "a child whose turn failed settles as failed: {record}"
    );
    // Read from the job turn's receipt: the turn's own failure is `failed`
    // with its typed error, not `restart_interrupted`, and the child is
    // retired as the live custodian retires it.
    let outcome = completion_record_outcome(&fixture, &owner, &job_id).await;
    assert_eq!(outcome["status"], "failed", "{outcome}");
    assert!(
        outcome["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "{outcome}"
    );
    // The re-link retires the child after it delivers the failed outcome, so
    // the retirement can trail the completion record. Settle on the mob
    // actor's machine-state publications (a roster change publishes). The
    // automatic pass only logs the retire's outcome (`relink_child` returns
    // it typed, #1391), so the deadline is a pure hang guard, and it must
    // exceed the production member-retire deadline (30 s) plus the re-link's
    // retries: equal deadlines made a retire that ran out its own budget
    // indistinguishable from a hang.
    let child = AgentIdentity::from("failing-child");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let mut changes = handle.machine_state_changes();
    while handle.get_member(&child).await.unwrap().is_some() {
        tokio::select! {
            changed = changes.changed() => changed.expect("the mob actor outlives the retirement"),
            () = tokio::time::sleep_until(deadline) => panic!("the failed child is retired"),
        }
    }
    fixture.teardown().await;
}

/// A required child retirement that fails once is retried by the re-link
/// (#1391): the child whose own turn failed is gone when the re-link returns.
#[tokio::test(flavor = "multi_thread")]
async fn relink_retries_a_failed_retirement_of_a_child_whose_turn_failed() {
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    let fixture = CouncilFixture::new(move |request| {
        if support::last_user_text(request).contains(CHILD_TASK) {
            ScriptedTurn::GatedFail(Arc::clone(&turn_gate), "provider rejected the turn".into())
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
    let job_id = "job-failing-turn-retire-retry".to_string();
    let child = AgentIdentity::from("failing-child-retire-retry");
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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    end_old_process_supervisor(run).await;
    gate.wait_entered(1).await;
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");

    // The first retirement the re-link makes fails before it reaches the
    // mob; the turn fails once the re-link watches it.
    let delivery = relink_delivery(&fixture).with_child_retirement_faults(1);
    let mob_id = fixture.source_mob_id();
    let relink = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &delivery,
        &mob_id,
        &handle,
        &child,
        &job,
    );
    let (_relinked, ()) = tokio::join!(relink, async { gate.open() });
    assert!(
        handle.get_member(&child).await.unwrap().is_none(),
        "the re-link retries a failed required retirement before it returns"
    );
    await_completion_record(&fixture, &owner, &job_id).await;
    let outcome = completion_record_outcome(&fixture, &owner, &job_id).await;
    assert_eq!(outcome["status"], "failed", "{outcome}");
    fixture.teardown().await;
}

/// A job turn ended from outside (here cancelled while it ran, as stopping
/// or destroying the runtime in a restart ends it) is not the turn's own
/// failure: its receipt settles as `restart_interrupted`, not `failed`.
#[tokio::test(flavor = "multi_thread")]
async fn relink_delivers_a_cancelled_job_turn_as_restart_interrupted() {
    let gate = TurnGate::new();
    let fixture = held_child_fixture(&gate);
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-cancelled-turn";
    let (child, _child_session) =
        fork_held_child(&fixture, &handle, &gate, "cancelled-child", job_id, &owner).await;
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    assert!(job.turn_delivery.is_some());
    let relink = tokio::spawn({
        let service = fixture.state.session_service();
        let delivery = relink_delivery(&fixture);
        let mob_id = fixture.source_mob_id();
        let handle = handle.clone();
        let child = child.clone();
        async move {
            meerkat_mob_mcp::fork_relink::relink_child(
                service, &delivery, &mob_id, &handle, &child, &job,
            )
            .await
            .action
        }
    });
    handle
        .force_cancel_member(child.clone())
        .await
        .expect("cancel the child's running turn");
    gate.open();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), relink)
            .await
            .expect("the cancelled turn's receipt settles the job")
            .unwrap(),
        ForkRelinkAction::Delivered
    );
    await_completion_record(&fixture, &owner, job_id).await;
    let outcome = completion_record_outcome(&fixture, &owner, job_id).await;
    assert_eq!(outcome["status"], "restart_interrupted", "{outcome}");
    fixture.teardown().await;
}

/// A job whose turn is terminal in its live agent but not yet committed has
/// no receipt: the child reads idle while its input is still owed a
/// terminal. The receipt watch keeps watching (it never settles an input
/// still in flight, which is also the state a requeued input is in after a
/// restart) and delivers the receipt's result once it exists.
#[tokio::test(flavor = "multi_thread")]
async fn relink_keeps_watching_a_job_input_still_owed_its_receipt() {
    let gate = TurnGate::new();
    let fixture = held_child_fixture(&gate);
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-receipt-in-flight";
    let (child, child_session) = fork_held_child(
        &fixture,
        &handle,
        &gate,
        "receipt-in-flight-child",
        job_id,
        &owner,
    )
    .await;
    let runtime =
        meerkat_mob::MobSessionService::acquire_runtime_adapter(fixture.service.as_ref(), None)
            .expect("acquire runtime authority")
            .expect("the service derives its runtime");
    let (entered, release) =
        runtime.arm_runtime_loop_before_terminal_commit_test_hook(child_session.clone());
    gate.open();
    entered.await.expect("the finished turn reaches its commit");
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    assert!(job.turn_delivery.is_some());
    let relink = tokio::spawn({
        let service = fixture.state.session_service();
        let delivery = relink_delivery(&fixture);
        let mob_id = fixture.source_mob_id();
        let handle = handle.clone();
        let child = child.clone();
        async move {
            meerkat_mob_mcp::fork_relink::relink_child(
                service, &delivery, &mob_id, &handle, &child, &job,
            )
            .await
            .action
        }
    });
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        completion_records(&fixture, &owner, job_id).await,
        0,
        "the re-link settled a job whose input was still owed its receipt"
    );
    assert!(!relink.is_finished());
    let _ = release.send(());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), relink)
            .await
            .expect("the receipt settles the job")
            .unwrap(),
        ForkRelinkAction::Delivered
    );
    await_completion_record(&fixture, &owner, job_id).await;
    let outcome = completion_record_outcome(&fixture, &owner, job_id).await;
    assert_eq!(outcome["status"], "completed", "{outcome}");
    assert_eq!(outcome["bounded_result"]["text"], CHILD_REPLY, "{outcome}");
    fixture.teardown().await;
}

/// The runtime-backed composition (RPC, REST, keep-alive CLI) with the same
/// held boundary commit. It races the same way: without the run-input check
/// this test delivers `restart_interrupted` inside the window, because the
/// child's member status reads idle before its service-turn commit lands
/// here too. The check closes it.
#[tokio::test(flavor = "multi_thread")]
async fn runtime_backed_relink_waits_for_a_finished_child_turn_to_commit() {
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    let store = Arc::new(support::commit_gate::CommitGateRuntimeStore::new());
    let fixture = CouncilFixture::new_runtime_backed_with_runtime_store(
        move |request| {
            if support::last_user_text(request).contains(CHILD_TASK) {
                ScriptedTurn::Gated(Arc::clone(&turn_gate), CHILD_REPLY.to_string())
            } else {
                ScriptedTurn::Text("noted".to_string())
            }
        },
        store.clone(),
    );
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let child = "runtime-commit-window-child";
    let job_id = "job-runtime-commit-window".to_string();
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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    end_old_process_supervisor(run).await;
    gate.wait_entered(1).await;
    let child_session = handle
        .resolve_bridge_session_id(&AgentIdentity::from(child))
        .await
        .expect("child session");
    store.arm(meerkat_runtime::LogicalRuntimeId::for_session(
        &child_session,
    ));

    let restarted = Arc::new(
        meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            fixture.service.clone(),
            fixture.runtime_adapter.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("construct runtime authority"),
    );
    fixture.bind_continuations(&restarted);
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await;
    gate.open();
    tokio::time::timeout(Duration::from_secs(30), store.entered())
        .await
        .expect("the runtime-backed turn's boundary commit reaches the held store method");
    // Outlasts the re-link's bounded run-input read (see the test above).
    tokio::time::sleep(Duration::from_secs(8)).await;
    let premature = if completion_records(&fixture, &owner, &job_id).await > 0 {
        Some(completion_record_text(&fixture, &owner, &job_id).await)
    } else {
        None
    };
    store.release();
    assert_eq!(
        premature, None,
        "the re-link reported the child before its finished turn was committed"
    );
    await_completion_record(&fixture, &owner, &job_id).await;
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(
        record.contains(CHILD_REPLY) && !record.contains("restart_interrupted"),
        "the child is delivered its real reply once its turn is committed: {record}"
    );
    fixture.teardown().await;
}

/// A finished turn whose boundary commit is held for good keeps the child's
/// session driver busy, so every read of its run inputs times out. A timeout
/// is inconclusive: the re-link keeps watching, but only until the ceiling,
/// and then delivers `restart_interrupted` without naming a cause, since no
/// reading was machine evidence of an unlanded commit.
#[tokio::test(flavor = "multi_thread")]
async fn relink_stops_waiting_on_unanswered_commit_reads_without_naming_a_cause() {
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    let store = Arc::new(support::commit_gate::CommitGateRuntimeStore::new());
    let fixture = CouncilFixture::new_with_runtime_store(
        move |request| {
            if support::last_user_text(request).contains(CHILD_TASK) {
                ScriptedTurn::Gated(Arc::clone(&turn_gate), CHILD_REPLY.to_string())
            } else {
                ScriptedTurn::Text("noted".to_string())
            }
        },
        store.clone(),
    );
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let child = AgentIdentity::from("stuck-commit-child");
    let job_id = "job-stuck-commit".to_string();
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec("stuck-commit-child"),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: job_id.clone(),
                owner_session_id: owner.clone(),
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    end_old_process_supervisor(run).await;
    gate.wait_entered(1).await;
    let child_session = handle
        .resolve_bridge_session_id(&child)
        .await
        .expect("child session");
    store.arm(meerkat_runtime::LogicalRuntimeId::for_session(
        &child_session,
    ));
    gate.open();
    store.entered().await;

    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    let ceiling = Duration::from_secs(2);
    let started = tokio::time::Instant::now();
    let action = meerkat_mob_mcp::fork_relink::relink_child_within(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
        ceiling,
    )
    .await
    .action;
    assert_eq!(action, ForkRelinkAction::Delivered);
    assert!(
        started.elapsed() >= ceiling,
        "the re-link waited out the ceiling first"
    );
    await_completion_record(&fixture, &owner, &job_id).await;
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(
        record.contains("restart_interrupted") && !record.contains("commit_never_landed"),
        "timed-out reads settle as restart_interrupted, with no typed cause: {record}"
    );
    store.release();
    fixture.teardown().await;
}

/// A boundary commit that fails after the run consumed its inputs in memory
/// leaves the child's runtime with degraded durability: its live state shows
/// nothing awaiting a boundary while its durable history lacks the turn. The
/// runtime's read refuses to answer from that state (an error, never "no
/// input pending"). A session the runtime does not hold is an error too.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_boundary_commit_makes_the_run_input_read_an_error() {
    let store = Arc::new(support::commit_gate::CommitGateRuntimeStore::new());
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    let fixture = CouncilFixture::new_with_runtime_store(
        move |request| {
            if support::last_user_text(request).contains(CHILD_TASK) {
                ScriptedTurn::Gated(Arc::clone(&turn_gate), CHILD_REPLY.to_string())
            } else {
                ScriptedTurn::Text("noted".to_string())
            }
        },
        store.clone(),
    );
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let child = AgentIdentity::from("failed-commit-child");
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec("failed-commit-child"),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: "job-failed-commit".to_string(),
                owner_session_id: owner.clone(),
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    let child_session = handle
        .resolve_bridge_session_id(&child)
        .await
        .expect("child session");
    let runtime =
        meerkat_mob::MobSessionService::acquire_runtime_adapter(fixture.service.as_ref(), None)
            .expect("acquire runtime authority")
            .expect("the service derives its runtime");
    assert!(
        runtime
            .session_has_uncommitted_run_input(&child_session)
            .await
            .unwrap(),
        "the running turn's input awaits its boundary"
    );
    store.arm_failure(meerkat_runtime::LogicalRuntimeId::for_session(
        &child_session,
    ));
    gate.open();
    store.failed().await;

    // The failure is marked under the driver lock the commit holds, so the
    // very first read after it already refuses to answer: never a transient
    // Ok(false) from the consumed-in-memory state.
    let degraded = runtime
        .session_has_uncommitted_run_input(&child_session)
        .await
        .expect_err("the runtime refuses to answer from degraded durability");
    assert!(
        matches!(
            degraded,
            meerkat_runtime::RuntimeDriverError::RecoveryRepairBlocked { .. }
        ),
        "degraded durability is an error, never 'no input pending': {degraded:?}"
    );
    let unheld = runtime
        .session_has_uncommitted_run_input(&meerkat_core::SessionId::new())
        .await
        .expect_err("a session the runtime does not hold is not 'nothing pending'");
    assert!(
        matches!(unheld, meerkat_runtime::RuntimeDriverError::NotReady { .. }),
        "{unheld:?}"
    );
    drop(run);
    // The destroy reloads the degraded member from its durable store and
    // retires it.
    tokio::time::timeout(DEGRADED_DESTROY_BOUND, fixture.teardown())
        .await
        .expect("teardown destroys a mob whose member lost a boundary commit");
}

/// How long a destroy may take when a member lost a boundary commit: twice
/// the per-member retire budget, so a destroy that waits on a reload nobody
/// performs fails the test instead of hanging it.
const DEGRADED_DESTROY_BOUND: Duration = Duration::from_mins(1);

/// A forked child whose boundary commit failed after its run consumed its
/// input in memory (the setup of
/// `a_failed_boundary_commit_makes_the_run_input_read_an_error`). The
/// child's runtime is reload-required, and its exited runtime loop keeps
/// the session's mutation gate until a reload releases it.
struct DegradedChild {
    fixture: CouncilFixture,
    store: Arc<support::commit_gate::CommitGateRuntimeStore>,
    child: AgentIdentity,
    child_session: meerkat_core::SessionId,
}

/// A forked child whose run waits at the scripted turn gate, with its
/// supervisor ended as a process exit ends it.
struct GatedChild {
    fixture: CouncilFixture,
    store: Arc<support::commit_gate::CommitGateRuntimeStore>,
    gate: Arc<TurnGate>,
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    child: AgentIdentity,
    child_session: meerkat_core::SessionId,
}

async fn fork_child_whose_boundary_commit_failed(child: &str) -> DegradedChild {
    let GatedChild {
        fixture,
        store,
        gate,
        runtime,
        child,
        child_session,
    } = fork_child_at_its_gated_turn(child).await;
    store.arm_failure(meerkat_runtime::LogicalRuntimeId::for_session(
        &child_session,
    ));
    gate.open();
    store.failed().await;
    let degraded = runtime
        .session_has_uncommitted_run_input(&child_session)
        .await
        .expect_err("the child's durability is degraded");
    assert!(
        matches!(
            degraded,
            meerkat_runtime::RuntimeDriverError::RecoveryRepairBlocked { .. }
        ),
        "{degraded:?}"
    );
    DegradedChild {
        fixture,
        store,
        child,
        child_session,
    }
}

/// A forked child whose terminal boundary commit waits at the store, armed to
/// fail once released. The child's runtime loop holds the session's mutation
/// gate while the commit waits.
async fn fork_child_whose_boundary_commit_is_held_to_fail(child: &str) -> GatedChild {
    let gated = fork_child_at_its_gated_turn(child).await;
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&gated.child_session);
    gated.store.arm(runtime_id.clone());
    gated.store.arm_failure(runtime_id);
    gated.gate.open();
    gated.store.entered().await;
    gated
}

async fn fork_child_at_its_gated_turn(child: &str) -> GatedChild {
    fork_child_at_its_gated_turn_measured_in(child, None).await
}

/// As [`fork_child_at_its_gated_turn`], with the fixture's runtime measuring
/// its store's commit outcome deadline in busy windows of `busy_window`.
async fn fork_child_at_its_gated_turn_measured_in(
    child: &str,
    busy_window: Option<Duration>,
) -> GatedChild {
    let store = Arc::new(support::commit_gate::CommitGateRuntimeStore::new());
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    let fixture = CouncilFixture::new_with_runtime_store(
        move |request| {
            if support::last_user_text(request).contains(CHILD_TASK) {
                ScriptedTurn::Gated(Arc::clone(&turn_gate), CHILD_REPLY.to_string())
            } else {
                ScriptedTurn::Text("noted".to_string())
            }
        },
        store.clone(),
    );
    if let Some(busy_window) = busy_window {
        meerkat_mob::MobSessionService::acquire_runtime_adapter(fixture.service.as_ref(), None)
            .expect("acquire runtime authority")
            .expect("the service derives its runtime")
            .set_runtime_commit_busy_window_for_test(busy_window);
    }
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let child = AgentIdentity::from(child);
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
                job_id: format!("job-{}", child.as_str()),
                owner_session_id: owner,
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    // End the run's supervisor while it still waits on the turn, as a
    // process exit does: once the turn fails it would retire the child
    // itself, racing the destroy under test.
    assert_eq!(
        run.end_supervisor_as_process_exit_for_test().await,
        meerkat_mob::ForkSupervisorExitForTest::EndedBeforeOutcome,
    );
    let child_session = handle
        .resolve_bridge_session_id(&child)
        .await
        .expect("child session");
    let runtime =
        meerkat_mob::MobSessionService::acquire_runtime_adapter(fixture.service.as_ref(), None)
            .expect("acquire runtime authority")
            .expect("the service derives its runtime");
    GatedChild {
        fixture,
        store,
        gate,
        runtime,
        child,
        child_session,
    }
}

/// Destroying a mob whose member lost a boundary commit finishes. The
/// destroy reloads the member's runtime from its durable store, where the
/// run input whose consumption never committed is still pending, and then
/// retires it: the retirement records that input as abandoned, so its fate
/// is durable instead of lost with the in-memory run.
#[tokio::test(flavor = "multi_thread")]
async fn destroying_a_mob_reloads_and_retires_a_member_whose_boundary_commit_failed() {
    let degraded = fork_child_whose_boundary_commit_failed("reloaded-child").await;
    let mob_id = degraded.fixture.source_mob_id();
    let report = tokio::time::timeout(
        DEGRADED_DESTROY_BOUND,
        degraded.fixture.state.mob_destroy(&mob_id),
    )
    .await
    .expect("the destroy finishes instead of waiting on a reload nobody performs")
    .expect("the destroy reloads the degraded member and retires it");
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(
        report.reloaded_members,
        vec![degraded.child.clone()],
        "the report names the member the destroy reloaded"
    );
    assert!(
        degraded.fixture.state.handle_for(&mob_id).await.is_err(),
        "the destroyed mob is gone"
    );
    let inputs = meerkat_runtime::RuntimeStore::load_input_states_strict(
        degraded.store.as_ref(),
        &meerkat_runtime::LogicalRuntimeId::for_session(&degraded.child_session),
    )
    .await
    .expect("the child's durable inputs");
    assert!(
        inputs.iter().any(|stored| stored.seed.terminal_outcome
            == Some(meerkat_runtime::InputTerminalOutcome::Abandoned {
                reason: meerkat_runtime::InputAbandonReason::Retired,
            })),
        "the retirement records the input the failed commit never consumed durably: {:?}",
        inputs
            .iter()
            .map(|stored| (
                stored.state.input_id.clone(),
                stored.seed.phase,
                stored.seed.terminal_outcome.clone()
            ))
            .collect::<Vec<_>>()
    );
}

/// A degraded member whose runtime cannot reload stops the destroy before it
/// starts: the destroy answers a typed incomplete report naming the member
/// and the reload, and the mob stays for a retry. Once the store reads
/// again, the same destroy completes.
#[tokio::test(flavor = "multi_thread")]
async fn a_destroy_that_cannot_reload_a_degraded_member_is_incomplete_and_retryable() {
    let degraded = fork_child_whose_boundary_commit_failed("unreloadable-child").await;
    let mob_id = degraded.fixture.source_mob_id();
    degraded
        .store
        .fail_input_recovery_reads(meerkat_runtime::LogicalRuntimeId::for_session(
            &degraded.child_session,
        ));
    let refused = tokio::time::timeout(
        DEGRADED_DESTROY_BOUND,
        degraded.fixture.state.mob_destroy(&mob_id),
    )
    .await
    .expect("the destroy answers instead of waiting on a reload nobody performs");
    let report = match refused {
        Err(meerkat_mob_mcp::MobMcpDestroyError::Incomplete { report }) => report,
        other => panic!("a reload that fails makes the destroy incomplete: {other:?}"),
    };
    assert!(
        report.errors.iter().any(|error| {
            error.contains(degraded.child.as_str()) && error.contains("durability reload")
        }),
        "the report names the member and its reload: {report:?}"
    );
    assert!(
        report.reloaded_members.is_empty(),
        "no member was reloaded: {report:?}"
    );
    assert!(
        degraded.fixture.state.handle_for(&mob_id).await.is_ok(),
        "the mob stays for a retry"
    );
    degraded.store.restore_input_recovery_reads();
    let report = tokio::time::timeout(
        DEGRADED_DESTROY_BOUND,
        degraded.fixture.state.mob_destroy(&mob_id),
    )
    .await
    .expect("the retried destroy finishes")
    .expect("the retried destroy reloads the member and completes");
    assert_eq!(report.reloaded_members, vec![degraded.child.clone()]);
    assert!(
        degraded.fixture.state.handle_for(&mob_id).await.is_err(),
        "the destroyed mob is gone"
    );
}

/// A member's boundary commit fails while the destroy's retire of that member
/// waits for the member session's mutation gate, which the runtime loop holds
/// across the commit. The parked detach takes the gate when the loop releases
/// it, and the retire's admission refuses typed for the member's degraded
/// durability, so the destroy reloads the member and retries the retirement
/// once instead of answering incomplete.
#[tokio::test(flavor = "multi_thread")]
async fn a_boundary_commit_that_fails_during_the_destroy_detach_is_reloaded_and_retired() {
    let held = fork_child_whose_boundary_commit_is_held_to_fail("detach-reloaded-child").await;
    let mob_id = held.fixture.source_mob_id();
    let parked = held
        .runtime
        .arm_mutation_gate_wait_observer_for_test(held.child_session.clone());
    let destroy = held.fixture.state.mob_destroy(&mob_id);
    let fail_the_commit_during_the_wait = async {
        parked
            .await
            .expect("the wait observer stays armed until a wait parks");
        held.store.release();
        held.store.failed().await;
    };
    let (report, ()) = tokio::time::timeout(DEGRADED_DESTROY_BOUND, async {
        tokio::join!(destroy, fail_the_commit_during_the_wait)
    })
    .await
    .expect("the destroy finishes within the bound");
    let report = report.expect("the destroy reloads the degraded member and retires it");
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(
        report.reloaded_members,
        vec![held.child.clone()],
        "the report names the member the destroy reloaded"
    );
    assert!(
        held.fixture.state.handle_for(&mob_id).await.is_err(),
        "the destroyed mob is gone"
    );
    let inputs = meerkat_runtime::RuntimeStore::load_input_states_strict(
        held.store.as_ref(),
        &meerkat_runtime::LogicalRuntimeId::for_session(&held.child_session),
    )
    .await
    .expect("the child's durable inputs");
    assert!(
        inputs.iter().any(|stored| stored.seed.terminal_outcome
            == Some(meerkat_runtime::InputTerminalOutcome::Abandoned {
                reason: meerkat_runtime::InputAbandonReason::Retired,
            })),
        "the retirement records the input the failed commit never consumed durably: {:?}",
        inputs
            .iter()
            .map(|stored| (
                stored.state.input_id.clone(),
                stored.seed.phase,
                stored.seed.terminal_outcome.clone()
            ))
            .collect::<Vec<_>>()
    );
}

/// The busy window the hung-commit destroy test measures its store's
/// declaration in: the deadline is two of them.
const HUNG_COMMIT_BUSY_WINDOW: Duration = Duration::from_secs(1);

/// A forked child whose terminal boundary commit waits at the store, which
/// never answers until the test releases it.
async fn fork_child_whose_boundary_commit_hangs(child: &str) -> GatedChild {
    fork_child_whose_boundary_commit_hangs_measured_in(child, HUNG_COMMIT_BUSY_WINDOW).await
}

/// [`fork_child_whose_boundary_commit_hangs`] with the store's declaration
/// measured in `busy_window`: the member's session degrades two of them after
/// the commit entered the store.
async fn fork_child_whose_boundary_commit_hangs_measured_in(
    child: &str,
    busy_window: Duration,
) -> GatedChild {
    let gated = fork_child_at_its_gated_turn_measured_in(child, Some(busy_window)).await;
    gated
        .store
        .arm(meerkat_runtime::LogicalRuntimeId::for_session(
            &gated.child_session,
        ));
    gated.gate.open();
    gated.store.entered().await;
    gated
}

/// Destroy `held`'s mob once the member's released boundary commit has
/// reported its outcome to the member's runtime.
///
/// Releasing the store lets the commit answer, but the commit's own task
/// reports that answer to the runtime only when it runs next. A reload
/// before then refuses, typed and retryable, so the destroy answers
/// incomplete with exactly that refusal for the member and reloads nothing.
/// Only that answer is retried, within the caller's bound; any other is
/// returned as it is.
async fn destroy_once_the_released_commit_reports(
    held: &GatedChild,
) -> Result<meerkat_mob::MobDestroyReport, meerkat_mob_mcp::MobMcpDestroyError> {
    let run_id = held
        .store
        .held_run()
        .expect("the held boundary commit carries its run's receipt");
    let outcome_pending = meerkat_mob::MemberReloadRefusal::RepairBlocked {
        reason: format!(
            "runtime commit of run {run_id} has not reported its durable outcome; retry the reload after it does"
        ),
        evidence_digest: None,
    };
    let mob_id = held.fixture.source_mob_id();
    loop {
        match held.fixture.state.mob_destroy(&mob_id).await {
            Err(meerkat_mob_mcp::MobMcpDestroyError::Incomplete { report })
                if report.reloaded_members.is_empty()
                    && report.errors.len() == 1
                    && matches!(
                        report.member_reload_refusals.as_slice(),
                        [refused] if refused.member == held.child
                            && refused.refusal == outcome_pending
                    ) =>
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            other => return other,
        }
    }
}

/// A member whose boundary commit the store never answers no longer keeps a
/// destroy waiting. At the store's declared deadline the member's session
/// degrades, so the destroy's retirement of it ends typed, and the destroy
/// answers incomplete, naming the member, while the store still has not
/// answered. Once the store answers and the commit reports that to the
/// member's runtime, the same destroy reloads the member and completes.
#[tokio::test(flavor = "multi_thread")]
async fn a_destroy_behind_a_commit_the_store_never_answers_is_incomplete_until_it_answers() {
    let held = fork_child_whose_boundary_commit_hangs("hung-commit-child").await;
    let mob_id = held.fixture.source_mob_id();
    let refused = tokio::time::timeout(
        DEGRADED_DESTROY_BOUND,
        held.fixture.state.mob_destroy(&mob_id),
    )
    .await
    .expect("the destroy answers while the store has not");
    let report = match refused {
        Err(meerkat_mob_mcp::MobMcpDestroyError::Incomplete { report }) => report,
        other => panic!("a destroy behind the hung commit is incomplete: {other:?}"),
    };
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.contains(held.child.as_str())),
        "the report names the member: {report:?}"
    );
    assert!(
        held.fixture.state.handle_for(&mob_id).await.is_ok(),
        "the mob stays for a retry"
    );

    held.store.release();
    let report = tokio::time::timeout(
        DEGRADED_DESTROY_BOUND,
        destroy_once_the_released_commit_reports(&held),
    )
    .await
    .expect("the retried destroy finishes")
    .expect("the retried destroy reloads the member and completes");
    assert_eq!(report.reloaded_members, vec![held.child.clone()]);
    assert!(
        held.fixture.state.handle_for(&mob_id).await.is_err(),
        "the destroyed mob is gone"
    );
}

/// A destroy retried while the store still has not answered a member's
/// boundary commit past its deadline cannot reload the degraded member: the
/// runtime refuses the reload, repair-blocked, until the commit reports its
/// durable outcome. The incomplete report keeps that refusal typed beside its
/// text, in the incomplete data every surface returns, so a caller can tell a
/// destroy to retry later from one that failed. Once the commit reports, the
/// same destroy completes and reports no refusal.
#[tokio::test(flavor = "multi_thread")]
async fn a_destroy_whose_member_reload_the_runtime_refuses_reports_the_refusal_typed() {
    let held = fork_child_whose_boundary_commit_hangs("refused-reload-child").await;
    let mob_id = held.fixture.source_mob_id();
    let first = tokio::time::timeout(
        DEGRADED_DESTROY_BOUND,
        held.fixture.state.mob_destroy(&mob_id),
    )
    .await
    .expect("the destroy answers while the store has not");
    assert!(
        matches!(
            first,
            Err(meerkat_mob_mcp::MobMcpDestroyError::Incomplete { .. })
        ),
        "a destroy behind the hung commit is incomplete: {first:?}"
    );

    let run_id = held
        .store
        .held_run()
        .expect("the held boundary commit carries its run's receipt");
    let refused = tokio::time::timeout(
        DEGRADED_DESTROY_BOUND,
        held.fixture.state.mob_destroy(&mob_id),
    )
    .await
    .expect("the retried destroy answers while the store has not");
    let report = match refused {
        Err(meerkat_mob_mcp::MobMcpDestroyError::Incomplete { report }) => report,
        other => panic!("a reload the runtime refuses makes the destroy incomplete: {other:?}"),
    };
    let member = format!("{}: ", held.child.as_str());
    assert!(
        matches!(report.errors.as_slice(), [error] if error.starts_with(&member)),
        "the refused reload keeps its text entry: {report:?}"
    );
    assert!(
        report.reloaded_members.is_empty(),
        "no member was reloaded: {report:?}"
    );
    let data = meerkat_mob_mcp::MobMcpDestroyError::incomplete_error_data(&report);
    assert_eq!(data["retryable"], serde_json::json!(true), "{data}");
    assert_eq!(
        data["destroy_report"]["member_reload_refusals"],
        serde_json::json!([{
            "member": held.child.as_str(),
            "refusal": {
                "kind": "repair_blocked",
                "reason": format!(
                    "runtime commit of run {run_id} has not reported its durable outcome; retry the reload after it does"
                ),
            },
        }]),
        "the incomplete data keeps the runtime's refusal typed: {data}"
    );
    assert!(
        held.fixture.state.handle_for(&mob_id).await.is_ok(),
        "the mob stays for a retry"
    );

    held.store.release();
    let report = tokio::time::timeout(
        DEGRADED_DESTROY_BOUND,
        destroy_once_the_released_commit_reports(&held),
    )
    .await
    .expect("the retried destroy finishes")
    .expect("the retried destroy reloads the member and completes");
    assert_eq!(report.reloaded_members, vec![held.child.clone()]);
    assert!(
        serde_json::to_value(&report)
            .expect("the report serializes")
            .get("member_reload_refusals")
            .is_none(),
        "a completed destroy reports no refusal: {report:?}"
    );
}

/// A destroy report without a refused reload serializes exactly as it did
/// before refusals were typed: no `member_reload_refusals` key.
#[test]
fn a_destroy_report_without_a_refused_reload_serializes_unchanged() {
    let mut report = meerkat_mob::MobDestroyReport::default();
    report.errors.push("worker-1: archive failed".to_string());
    report
        .reloaded_members
        .push(AgentIdentity::from("worker-2"));
    assert_eq!(
        serde_json::to_string(&report).expect("the report serializes"),
        r#"{"remote_cleanup_deadline_exceeded":false,"metadata_scrubbed":false,"events_cleared":false,"namespace_cleaned":false,"errors":["worker-1: archive failed"],"reloaded_members":["worker-2"]}"#
    );
    let data = meerkat_mob_mcp::MobMcpDestroyError::incomplete_error_data(&report);
    assert!(
        data["destroy_report"]
            .get("member_reload_refusals")
            .is_none(),
        "{data}"
    );
}

/// Closing a mob's admission if it is idle asks each member whether it has
/// active inputs. Behind a member's boundary commit that the store never
/// answers, that read answers busy once the member's session degrades at the
/// store's deadline, so the close answers within the bound and leaves the
/// busy mob open. Before, the read waited for the member's driver, which the
/// commit holds, and so for the store.
#[tokio::test(flavor = "multi_thread")]
async fn closing_a_mob_if_idle_behind_a_commit_the_store_never_answers_answers_busy() {
    let held = fork_child_whose_boundary_commit_hangs("hung-close-child").await;
    let handle = held
        .fixture
        .state
        .handle_for(&held.fixture.source_mob_id())
        .await
        .unwrap();
    let outcome = tokio::time::timeout(DEGRADED_DESTROY_BOUND, handle.close_if_idle())
        .await
        .expect("closing if idle answers while the store has not")
        .expect("the mob answers the close");
    assert!(
        handle.admission_closed().borrow().is_none(),
        "the member answers busy, so the mob stays open: {outcome:?}"
    );
    held.store.release();
}

/// Redrive `held`'s stuck retirement of its member once the member's released
/// boundary commit has reported its outcome to the member's runtime.
///
/// Until the commit's own task reports the store's answer, the redriven
/// retirement's reload refuses, typed and retryable, and the retirement stays
/// stuck on exactly that cause. Only that answer is retried, within the
/// caller's bound; any other is returned as it is.
async fn redrive_once_the_released_commit_reports(
    held: &GatedChild,
    handle: &meerkat_mob::MobHandle,
) -> Result<(), meerkat_mob::MobError> {
    loop {
        match handle.redrive_retirement(held.child.clone()).await {
            Err(meerkat_mob::MobError::MemberRetirementStuck { cause, .. })
                if cause
                    .structured_data()
                    .and_then(|data| data.get("kind").cloned())
                    == Some(serde_json::json!("mob_member_reload_repair_blocked")) =>
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            other => return other,
        }
    }
}

/// A retire of a member whose session degraded behind a boundary commit the
/// store has not answered cannot reload the member: the runtime refuses the
/// reload, repair-blocked, until the commit reports its durable outcome. The
/// retirement sticks with that refusal as its typed cause, retryable and
/// carrying the runtime's reason, instead of an internal error that only says
/// it in text. Once the commit reports, redriving the retirement completes it.
#[tokio::test(flavor = "multi_thread")]
async fn a_retire_whose_member_reload_the_runtime_refuses_sticks_with_the_refusal_typed() {
    let held = fork_child_whose_boundary_commit_hangs("retire-refused-child").await;
    let mob_id = held.fixture.source_mob_id();
    let handle = held.fixture.state.handle_for(&mob_id).await.unwrap();
    tokio::time::timeout(DEGRADED_DESTROY_BOUND, handle.close_if_idle())
        .await
        .expect("closing if idle answers once the member's session degrades")
        .expect("the mob answers the close");
    let run_id = held
        .store
        .held_run()
        .expect("the held boundary commit carries its run's receipt");

    let refused = tokio::time::timeout(DEGRADED_DESTROY_BOUND, handle.retire(held.child.clone()))
        .await
        .expect("the retire answers while the store has not");
    let (stage, cause) = match refused {
        Err(meerkat_mob::MobError::MemberRetirementStuck { stage, cause, .. }) => (stage, cause),
        other => panic!("a retire whose reload the runtime refuses sticks: {other:?}"),
    };
    assert_eq!(stage, "retire-runtime-quiesce");
    assert_eq!(
        cause.structured_data(),
        Some(serde_json::json!({
            "kind": "mob_member_reload_repair_blocked",
            "session_id": held.child_session.to_string(),
            "reason": format!(
                "runtime commit of run {run_id} has not reported its durable outcome; retry the reload after it does"
            ),
            "retryable": true,
            "authority_retained": true,
            "live_shell_discarded": false,
        })),
        "the stuck retirement keeps the runtime's refusal typed: {cause:?}"
    );
    assert_eq!(
        cause.wire_error_code(),
        Some(meerkat_contracts::ErrorCode::SessionBusy),
        "{cause:?}"
    );

    held.store.release();
    tokio::time::timeout(
        DEGRADED_DESTROY_BOUND,
        redrive_once_the_released_commit_reports(&held, &handle),
    )
    .await
    .expect("the redriven retirement finishes")
    .expect("the redriven retirement reloads the member and retires it");
    assert!(
        handle
            .get_member(&held.child)
            .await
            .expect("the roster answers")
            .is_none(),
        "the retired member left the roster"
    );
}

/// The busy window of the rows whose retire must start before the member's
/// session degrades: its deadline, two of them, leaves the retire's admission
/// and first stages ample time to reach the quiesce first.
const DEGRADING_RETIRE_BUSY_WINDOW: Duration = Duration::from_secs(3);

/// Start a retire of `held`'s member before its session degrades: the
/// retirement waits at its quiesce stage for the hung commit, and the
/// session degrades at the store's deadline meanwhile.
async fn retire_before_the_member_degrades(held: &GatedChild, handle: &meerkat_mob::MobHandle) {
    let started = tokio::time::timeout(DEGRADED_DESTROY_BOUND, handle.retire(held.child.clone()))
        .await
        .expect("the retire answers while the store has not");
    assert!(
        matches!(
            &started,
            Err(meerkat_mob::MobError::MemberRetirementInProgress { stage, .. })
                if stage == "retire-runtime-quiesce"
        ),
        "a retire started before the commit's deadline waits at its quiesce: {started:?}"
    );
    assert!(
        held.runtime
            .durability_reload_required(&held.child_session)
            .await
            .is_some(),
        "the member's session degraded while the retire waited"
    );
}

/// Retire `held`'s member until its retirement answers other than in
/// progress, within the caller's bound.
async fn retire_until_settled(
    held: &GatedChild,
    handle: &meerkat_mob::MobHandle,
) -> Result<(), meerkat_mob::MobError> {
    tokio::time::timeout(DEGRADED_DESTROY_BOUND, async {
        loop {
            match handle.retire(held.child.clone()).await {
                Err(meerkat_mob::MobError::MemberRetirementInProgress { .. }) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                other => return other,
            }
        }
    })
    .await
    .expect("the retirement settles within the bound")
}

/// A retire that started before its member's session degraded behind a
/// boundary commit the store has not answered waits at its quiesce stage for
/// the commit. Once the commit reports, the retirement reaches its ingress
/// detach with the session still degraded. The stage reloads the member's
/// registration first, as the quiesce does, and detaches the cold successor,
/// so a plain retry completes the retirement with no manual reload. Before,
/// the runtime refused the detach and the retirement stuck there.
#[tokio::test(flavor = "multi_thread")]
async fn a_retire_whose_member_degraded_while_it_waited_completes_once_the_commit_reports() {
    let held = fork_child_whose_boundary_commit_hangs_measured_in(
        "retire-detach-child",
        DEGRADING_RETIRE_BUSY_WINDOW,
    )
    .await;
    let handle = held
        .fixture
        .state
        .handle_for(&held.fixture.source_mob_id())
        .await
        .unwrap();
    retire_before_the_member_degrades(&held, &handle).await;
    held.store.release();

    retire_until_settled(&held, &handle)
        .await
        .expect("the retirement reloads the degraded member at its ingress detach and completes");
    assert!(
        handle
            .get_member(&held.child)
            .await
            .expect("the roster answers")
            .is_none(),
        "the retired member left the roster"
    );
    assert!(
        held.runtime
            .current_session_registration_witness(&held.child_session)
            .await
            .is_none(),
        "no registration of the member's session remains"
    );
    assert!(
        matches!(
            held.runtime.peer_ingress_owner(&held.child_session).await,
            meerkat_runtime::PeerIngressOwner::Unattached
        ),
        "the member's session has no peer ingress owner"
    );
}

/// A retire whose reload at its ingress detach has not settled when the
/// caller's budget ends answers that the retirement is in progress at its
/// ingress detach, retryable by a plain retry; it does not stick. Once the
/// reload settles, a plain retry completes the retirement.
#[tokio::test(flavor = "multi_thread")]
async fn a_retire_whose_ingress_reload_is_held_answers_in_progress_until_it_settles() {
    let held = fork_child_whose_boundary_commit_hangs_measured_in(
        "retire-held-reload-child",
        DEGRADING_RETIRE_BUSY_WINDOW,
    )
    .await;
    let handle = held
        .fixture
        .state
        .handle_for(&held.fixture.source_mob_id())
        .await
        .unwrap();
    retire_before_the_member_degrades(&held, &handle).await;
    let (reload_entered, release_reload) = held
        .runtime
        .arm_reload_required_discard_before_mutation_gate_test_hook(held.child_session.clone());
    held.store.release();

    tokio::time::timeout(DEGRADED_DESTROY_BOUND, async {
        loop {
            match handle.retire(held.child.clone()).await {
                Err(meerkat_mob::MobError::MemberRetirementInProgress { stage, .. })
                    if stage == "retire-ingress-detach" =>
                {
                    return;
                }
                Err(meerkat_mob::MobError::MemberRetirementInProgress { .. }) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                other => panic!("the retire waits on its held ingress reload: {other:?}"),
            }
        }
    })
    .await
    .expect("the retirement reaches its ingress detach");
    tokio::time::timeout(DEGRADED_DESTROY_BOUND, reload_entered)
        .await
        .expect("the ingress detach reloads the degraded member")
        .expect("the held reload's hook stays armed until it is entered");
    let waiting = handle.retire(held.child.clone()).await;
    assert!(
        matches!(
            &waiting,
            Err(meerkat_mob::MobError::MemberRetirementInProgress { stage, .. })
                if stage == "retire-ingress-detach"
        ),
        "a retire while the reload is held is in progress, not stuck: {waiting:?}"
    );
    assert!(
        waiting
            .as_ref()
            .is_err_and(meerkat_mob::MobError::is_retirement_in_progress),
        "{waiting:?}"
    );

    release_reload
        .send(())
        .expect("the held reload waits for its release");
    retire_until_settled(&held, &handle)
        .await
        .expect("a plain retry completes the retirement once the reload settles");
    assert!(
        handle
            .get_member(&held.child)
            .await
            .expect("the roster answers")
            .is_none(),
        "the retired member left the roster"
    );
}

/// Force-cancelling a member whose runtime needs a durability reload answers
/// the typed `MemberReloadRequired`, naming the member, at once: it neither
/// waits on the member session's gates nor fails untyped behind them. A
/// cancel of all of its work answers the same. The repair it names,
/// `reload_member_registration`, then lets the cancel run, and the member's
/// durable input ends terminal.
#[tokio::test(flavor = "multi_thread")]
async fn force_cancelling_a_member_that_needs_a_reload_answers_typed_until_it_is_reloaded() {
    let degraded = fork_child_whose_boundary_commit_failed("cancelled-child").await;
    let handle = degraded
        .fixture
        .state
        .handle_for(&degraded.fixture.source_mob_id())
        .await
        .unwrap();
    let refused = tokio::time::timeout(
        Duration::from_secs(10),
        handle.force_cancel_member(degraded.child.clone()),
    )
    .await
    .expect("the cancel answers at once");
    match refused {
        Err(meerkat_mob::MobError::MemberReloadRequired { member_id, .. }) => {
            assert_eq!(member_id, degraded.child, "the refusal names the member");
        }
        other => panic!("a degraded member's cancel answers MemberReloadRequired: {other:?}"),
    }
    let entry = handle
        .get_member(&degraded.child)
        .await
        .expect("the roster answers")
        .expect("the degraded member stays in the roster");
    let refused = tokio::time::timeout(
        Duration::from_secs(10),
        handle.cancel_all_work(entry.agent_runtime_id.clone(), entry.fence_token),
    )
    .await
    .expect("the cancel of all its work answers at once");
    match refused {
        Err(meerkat_mob::MobError::MemberReloadRequired { member_id, .. }) => {
            assert_eq!(member_id, degraded.child, "the refusal names the member");
        }
        other => panic!(
            "a degraded member's cancel of all its work answers MemberReloadRequired: {other:?}"
        ),
    }

    handle
        .reload_member_registration(&degraded.child)
        .await
        .expect("the named repair reloads the member");
    tokio::time::timeout(
        Duration::from_secs(10),
        handle.force_cancel_member(degraded.child.clone()),
    )
    .await
    .expect("the cancel answers")
    .expect("the reloaded member's cancel runs");
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&degraded.child_session);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let inputs = meerkat_runtime::RuntimeStore::load_input_states_strict(
                degraded.store.as_ref(),
                &runtime_id,
            )
            .await
            .expect("the child's durable inputs");
            if !inputs.is_empty()
                && inputs
                    .iter()
                    .all(|stored| stored.seed.terminal_outcome.is_some())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the member's durable input ends terminal");
}

/// A receipt that lands while the ceiling-deciding status read is held is
/// the job's outcome: before delivering `restart_interrupted` at the ceiling
/// the re-link reads the exact receipt once more. (Review P1: without that
/// read, the stale phase decided, and the completion dedupe then suppressed
/// the real reply for good.)
#[tokio::test(flavor = "multi_thread")]
async fn a_receipt_that_lands_during_the_ceiling_status_read_is_delivered() {
    let gate = TurnGate::new();
    let fixture = held_child_fixture(&gate);
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-receipt-at-ceiling";
    let (child, child_session) = fork_held_child(
        &fixture,
        &handle,
        &gate,
        "receipt-at-ceiling-child",
        job_id,
        &owner,
    )
    .await;
    let runtime =
        meerkat_mob::MobSessionService::acquire_runtime_adapter(fixture.service.as_ref(), None)
            .expect("acquire runtime authority")
            .expect("the service derives its runtime");
    let (commit_entered, release_commit) =
        runtime.arm_runtime_loop_before_terminal_commit_test_hook(child_session.clone());
    gate.open();
    commit_entered
        .await
        .expect("the finished turn reaches its commit");
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    let turn_delivery = job.turn_delivery.clone().expect("receipt-anchored job");

    // A zero ceiling: the first idle reading decides.
    let (status_entered, release_status) =
        meerkat_mob::MobHandle::arm_member_status_read_test_gate(child.clone());
    let relink = tokio::spawn({
        let service = fixture.state.session_service();
        let delivery = relink_delivery(&fixture);
        let mob_id = fixture.source_mob_id();
        let handle = handle.clone();
        let child = child.clone();
        async move {
            meerkat_mob_mcp::fork_relink::relink_child_within(
                service,
                &delivery,
                &mob_id,
                &handle,
                &child,
                &job,
                Duration::ZERO,
            )
            .await
            .action
        }
    });
    // The receipt wait found the input still owed; the status read that
    // decides the ceiling is held here while the commit lands.
    status_entered
        .await
        .expect("the re-link reads the child's status");
    let _ = release_commit.send(());
    let spec = meerkat_mob::BoundedResultSpec::new("fork_off_result", 16 * 1024).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let report = handle
            .wait_bounded_work_for_identity_with_delivery_identity(
                &child,
                &turn_delivery,
                &spec,
                meerkat_core::time_compat::Instant::now() + Duration::from_secs(1),
            )
            .await
            .expect("receipt read");
        if matches!(
            report.work(),
            meerkat_mob::DeliveryTerminalWait::Terminal(_)
        ) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the released commit publishes the receipt"
        );
    }
    release_status
        .send(())
        .expect("the ceiling status read was still held when the receipt landed");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), relink)
            .await
            .expect("the re-link decides at the ceiling")
            .unwrap(),
        ForkRelinkAction::Delivered
    );
    await_completion_record(&fixture, &owner, job_id).await;
    let outcome = completion_record_outcome(&fixture, &owner, job_id).await;
    assert_eq!(outcome["status"], "completed", "{outcome}");
    assert_eq!(outcome["bounded_result"]["text"], CHILD_REPLY, "{outcome}");
    fixture.teardown().await;
}

/// The last receipt read before a ceiling delivery is a real wait on the
/// runtime, not only the waiter's 100 ms evidence floor: a job input still
/// owed its terminal when that read starts, whose receipt lands while the
/// read waits, is delivered its real reply. (Review MAJOR: the floor-only
/// read found the input pending and delivered `restart_interrupted`, and the
/// completion dedupe then suppressed the real reply for good.)
#[tokio::test(flavor = "multi_thread")]
async fn a_receipt_that_lands_during_the_last_ceiling_read_is_delivered() {
    let gate = TurnGate::new();
    let fixture = held_child_fixture(&gate);
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-receipt-in-last-read";
    let (child, child_session) = fork_held_child(
        &fixture,
        &handle,
        &gate,
        "receipt-in-last-read-child",
        job_id,
        &owner,
    )
    .await;
    let runtime =
        meerkat_mob::MobSessionService::acquire_runtime_adapter(fixture.service.as_ref(), None)
            .expect("acquire runtime authority")
            .expect("the service derives its runtime");
    let (commit_entered, release_commit) =
        runtime.arm_runtime_loop_before_terminal_commit_test_hook(child_session.clone());
    gate.open();
    commit_entered
        .await
        .expect("the finished turn reaches its commit");
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    assert!(job.turn_delivery.is_some());

    // A zero ceiling: the first idle reading decides, after the last read.
    let (status_entered, release_status) =
        meerkat_mob::MobHandle::arm_member_status_read_test_gate(child.clone());
    let mut relink = tokio::spawn({
        let service = fixture.state.session_service();
        let delivery = relink_delivery(&fixture);
        let mob_id = fixture.source_mob_id();
        let handle = handle.clone();
        let child = child.clone();
        async move {
            meerkat_mob_mcp::fork_relink::relink_child_within(
                service,
                &delivery,
                &mob_id,
                &handle,
                &child,
                &job,
                Duration::ZERO,
            )
            .await
            .action
        }
    });
    status_entered
        .await
        .expect("the re-link reads the child's status");
    release_status
        .send(())
        .expect("the ceiling status read was held");
    // The last read starts once the status read returns. The commit lands
    // well after the waiter's evidence floor and well inside the last read's
    // own bound.
    let settled_early = tokio::time::timeout(Duration::from_secs(1), &mut relink).await;
    release_commit
        .send(())
        .expect("the finished turn's commit was still held");
    assert!(
        settled_early.is_err(),
        "the re-link settled while the job turn's receipt was still owed: {settled_early:?}"
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), relink)
            .await
            .expect("the receipt settles the job")
            .unwrap(),
        ForkRelinkAction::Delivered
    );
    await_completion_record(&fixture, &owner, job_id).await;
    assert_eq!(completion_records(&fixture, &owner, job_id).await, 1);
    let outcome = completion_record_outcome(&fixture, &owner, job_id).await;
    assert_eq!(outcome["status"], "completed", "{outcome}");
    assert_eq!(outcome["bounded_result"]["text"], CHILD_REPLY, "{outcome}");
    fixture.teardown().await;
}

/// The last receipt read before a ceiling delivery can wait on the child's
/// session driver while the job turn's own boundary commit holds it, across
/// the durable write. Such a read says nothing: the re-link delivers
/// `restart_interrupted` neither at the waiter's evidence floor nor at the
/// read's own bound, keeps watching, and delivers the real reply once the
/// commit lands. (Review MAJOR: any read short of a terminal at the ceiling
/// counted as proof that no receipt existed.)
#[tokio::test(flavor = "multi_thread")]
async fn a_last_ceiling_read_held_by_the_turns_own_commit_is_not_proof_of_absence() {
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    let store = Arc::new(support::commit_gate::CommitGateRuntimeStore::new());
    let fixture = CouncilFixture::new_with_runtime_store(
        move |request| {
            if support::last_user_text(request).contains(CHILD_TASK) {
                ScriptedTurn::Gated(Arc::clone(&turn_gate), CHILD_REPLY.to_string())
            } else {
                ScriptedTurn::Text("noted".to_string())
            }
        },
        store.clone(),
    );
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-last-read-held-by-commit";
    let (child, child_session) = fork_held_child(
        &fixture,
        &handle,
        &gate,
        "last-read-held-by-commit-child",
        job_id,
        &owner,
    )
    .await;
    // The finished turn's durable boundary write is held in the store while
    // its commit holds the child's session driver: every receipt read waits.
    store.arm(meerkat_runtime::LogicalRuntimeId::for_session(
        &child_session,
    ));
    gate.open();
    tokio::time::timeout(Duration::from_secs(30), store.entered())
        .await
        .expect("the finished turn's boundary commit reaches the held store write");
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    assert!(job.turn_delivery.is_some());

    // A zero ceiling: the first idle reading decides, after the last read.
    let (status_entered, release_status) =
        meerkat_mob::MobHandle::arm_member_status_read_test_gate(child.clone());
    let mut relink = tokio::spawn({
        let service = fixture.state.session_service();
        let delivery = relink_delivery(&fixture);
        let mob_id = fixture.source_mob_id();
        let handle = handle.clone();
        let child = child.clone();
        async move {
            meerkat_mob_mcp::fork_relink::relink_child_within(
                service,
                &delivery,
                &mob_id,
                &handle,
                &child,
                &job,
                Duration::ZERO,
            )
            .await
            .action
        }
    });
    status_entered
        .await
        .expect("the re-link reads the child's status");
    release_status
        .send(())
        .expect("the ceiling status read was held");
    // Past the waiter's evidence floor and past the last read's own bound:
    // neither ending of the held read settles the job.
    let settled_early = tokio::time::timeout(Duration::from_secs(4), &mut relink).await;
    store.release();
    assert!(
        settled_early.is_err(),
        "the re-link settled on a receipt read held by the turn's own commit: {settled_early:?}"
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), relink)
            .await
            .expect("the landed receipt settles the job")
            .unwrap(),
        ForkRelinkAction::Delivered
    );
    await_completion_record(&fixture, &owner, job_id).await;
    assert_eq!(completion_records(&fixture, &owner, job_id).await, 1);
    let outcome = completion_record_outcome(&fixture, &owner, job_id).await;
    assert_eq!(outcome["status"], "completed", "{outcome}");
    assert_eq!(outcome["bounded_result"]["text"], CHILD_REPLY, "{outcome}");
    fixture.teardown().await;
}

/// The job turn's commit can take the child's session driver partway
/// through the last receipt read at the ceiling: the read's first reading
/// finds the input pending, then its final evidence read waits on the driver
/// until its bound runs out. The waiter then reports that earlier pending
/// reading as `EvidenceReadTimedOut`, which says nothing about the input now,
/// so the re-link keeps watching and delivers the real reply once the commit
/// lands. (Verification of the review MAJOR fix: that stale reading counted as
/// proof of absence, with `commit_never_landed` from its stale phase.)
#[tokio::test(flavor = "multi_thread")]
async fn a_commit_taking_the_driver_mid_way_through_the_last_ceiling_read_is_not_proof_of_absence()
{
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    let store = Arc::new(support::commit_gate::CommitGateRuntimeStore::new());
    let fixture = CouncilFixture::new_with_runtime_store(
        move |request| {
            if support::last_user_text(request).contains(CHILD_TASK) {
                ScriptedTurn::Gated(Arc::clone(&turn_gate), CHILD_REPLY.to_string())
            } else {
                ScriptedTurn::Text("noted".to_string())
            }
        },
        store.clone(),
    );
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-commit-mid-last-read";
    let (child, child_session) = fork_held_child(
        &fixture,
        &handle,
        &gate,
        "commit-mid-last-read-child",
        job_id,
        &owner,
    )
    .await;
    // The finished turn stops before its commit takes the driver (the child
    // reads idle, the input pending); once released, its durable boundary
    // write is held in the store with the driver taken.
    let runtime =
        meerkat_mob::MobSessionService::acquire_runtime_adapter(fixture.service.as_ref(), None)
            .expect("acquire runtime authority")
            .expect("the service derives its runtime");
    let (commit_entered, release_commit) =
        runtime.arm_runtime_loop_before_terminal_commit_test_hook(child_session.clone());
    store.arm(meerkat_runtime::LogicalRuntimeId::for_session(
        &child_session,
    ));
    gate.open();
    commit_entered
        .await
        .expect("the finished turn reaches its commit");
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    assert!(job.turn_delivery.is_some());

    // A zero ceiling: the first idle reading decides, then the last read.
    let (status_entered, release_status) =
        meerkat_mob::MobHandle::arm_member_status_read_test_gate(child.clone());
    let mut relink = tokio::spawn({
        let service = fixture.state.session_service();
        let delivery = relink_delivery(&fixture);
        let mob_id = fixture.source_mob_id();
        let handle = handle.clone();
        let child = child.clone();
        async move {
            meerkat_mob_mcp::fork_relink::relink_child_within(
                service,
                &delivery,
                &mob_id,
                &handle,
                &child,
                &job,
                Duration::ZERO,
            )
            .await
            .action
        }
    });
    status_entered
        .await
        .expect("the re-link reads the child's status");
    release_status
        .send(())
        .expect("the ceiling status read was held");
    // The last read begins as the status read returns; its first reading
    // finds the input pending. A few hundred milliseconds in (past the
    // waiter's 100 ms evidence floor, well inside the read's 2 s bound) the
    // commit takes the driver and holds it across the store write.
    tokio::time::sleep(Duration::from_millis(400)).await;
    release_commit
        .send(())
        .expect("the finished turn's commit was still held before the driver");
    tokio::time::timeout(Duration::from_secs(30), store.entered())
        .await
        .expect("the commit reaches the held store write with the driver taken");
    let settled_early = tokio::time::timeout(Duration::from_secs(4), &mut relink).await;
    store.release();
    assert!(
        settled_early.is_err(),
        "the re-link settled on a pending reading from before a final read that ran out: \
         {settled_early:?}"
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), relink)
            .await
            .expect("the landed receipt settles the job")
            .unwrap(),
        ForkRelinkAction::Delivered
    );
    await_completion_record(&fixture, &owner, job_id).await;
    assert_eq!(completion_records(&fixture, &owner, job_id).await, 1);
    let outcome = completion_record_outcome(&fixture, &owner, job_id).await;
    assert_eq!(outcome["status"], "completed", "{outcome}");
    assert_eq!(outcome["bounded_result"]["text"], CHILD_REPLY, "{outcome}");
    fixture.teardown().await;
}

/// A status read held past an active `max_run` does not hold the limit
/// back: the read is bounded by the deadline, and the re-link cancels,
/// delivers `max_run_elapsed` and retires the child at the deadline while the
/// read is still held. (Review P2b.)
#[tokio::test(flavor = "multi_thread")]
async fn a_status_read_held_past_max_run_still_retires_the_child_at_the_deadline() {
    let gate = TurnGate::new();
    let fixture = held_child_fixture(&gate);
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-status-held-past-limit";
    // The turn stays running: its gate is never opened.
    let (child, _child_session) = fork_held_child(
        &fixture,
        &handle,
        &gate,
        "status-held-past-limit-child",
        job_id,
        &owner,
    )
    .await;
    let mut job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    assert!(job.turn_delivery.is_some());
    job.max_run_ms = Some(now_ms().saturating_sub(job.started_at_ms) + 2_500);
    let deadline_ms = job.started_at_ms + job.max_run_ms.unwrap();

    let (status_entered, mut release_status) =
        meerkat_mob::MobHandle::arm_member_status_read_test_gate(child.clone());
    // The held read owns the gate's release receiver, so the release sender
    // closes exactly when the re-link abandons that read: the moment the
    // limit decides. The sender is handed back so the gate stays unreleased.
    let read_abandoned = tokio::spawn(async move {
        release_status.closed().await;
        (now_ms(), release_status)
    });
    let action = tokio::time::timeout(
        Duration::from_secs(20),
        meerkat_mob_mcp::fork_relink::relink_child(
            fixture.state.session_service(),
            &relink_delivery(&fixture),
            &fixture.source_mob_id(),
            &handle,
            &child,
            &job,
        ),
    )
    .await
    .expect("the limit decides although the status read is held")
    .action;
    assert_eq!(action, ForkRelinkAction::Delivered);
    status_entered
        .await
        .expect("the re-link's status read was held");
    // The re-link decided by abandoning the read it was still holding: the
    // gate was never released (the release sender is returned unsent). That
    // the read's bound is cut to the deadline, not its own 5 s, is the pure
    // `within_limit` rule, unit-tested in `fork_relink`; this test proves the
    // held read does not hold the limit back, which needs no wall-clock
    // margin. Timers never fire early, so the lower bound is load-proof.
    let (abandoned_ms, release_status) = read_abandoned
        .await
        .expect("the held read was abandoned when the limit decided");
    assert!(
        abandoned_ms >= deadline_ms,
        "decided at the deadline, not before"
    );
    await_completion_record(&fixture, &owner, job_id).await;
    let outcome = completion_record_outcome(&fixture, &owner, job_id).await;
    assert_eq!(outcome["status"], "max_run_elapsed", "{outcome}");
    assert!(
        handle.get_member(&child).await.unwrap().is_none(),
        "retired"
    );
    drop(release_status);
    gate.open();
    fixture.teardown().await;
}

/// A job whose delivered outcome retires its child (its own turn failed) but
/// whose child was left seated (a crash between delivery and retirement) has
/// the child retired by the next pass. A delivered `restart_interrupted`
/// shares the record's `failed` notice status, and its child stays seated.
/// (Review P2a.)
#[tokio::test(flavor = "multi_thread")]
async fn a_delivered_failure_retires_its_seated_child_but_restart_interrupted_stays_seated() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    for (name, delivered_status, retired) in [
        ("failed-kept-child", "failed", true),
        ("interrupted-kept-child", "restart_interrupted", false),
    ] {
        let detail = serde_json::json!({
            "agent_identity": name,
            "status": delivered_status,
            "error": "the job's turn failed",
        });
        assert_eq!(
            relink_after_a_delivered_record(
                &fixture,
                &handle,
                &owner,
                name,
                meerkat_core::event::BackgroundJobTerminalStatus::Failed,
                detail,
            )
            .await,
            retired,
            "{name}: retired only when its delivered outcome retires it"
        );
    }
    fixture.teardown().await;
}

/// A delivered record whose detail carries no typed outcome (it does not
/// decode) falls back to its notice status: `terminated` (a limit autokill)
/// retires the child left seated, `failed` (which a `restart_interrupted`
/// outcome shares) keeps it seated.
#[tokio::test(flavor = "multi_thread")]
async fn a_delivered_record_without_a_typed_outcome_falls_back_to_its_notice_status() {
    use meerkat_core::event::BackgroundJobTerminalStatus;
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    for (name, notice, retired) in [
        (
            "opaque-terminated-child",
            BackgroundJobTerminalStatus::Terminated,
            true,
        ),
        (
            "opaque-failed-child",
            BackgroundJobTerminalStatus::Failed,
            false,
        ),
    ] {
        assert_eq!(
            relink_after_a_delivered_record(
                &fixture,
                &handle,
                &owner,
                name,
                notice,
                serde_json::json!("an outcome this build cannot decode"),
            )
            .await,
            retired,
            "{name}: an untyped record retires its child only on a terminated notice"
        );
    }
    fixture.teardown().await;
}

/// A job id is not unique across children (a host binds it), and records
/// are keyed by owner session and job id. A delivered record whose detail
/// names another child, by identity or by member ref, is not this child's:
/// its retiring outcome does not retire this child. (Review: the committed
/// record was matched by job id alone.)
#[tokio::test(flavor = "multi_thread")]
async fn a_delivered_record_of_another_child_does_not_retire_this_one() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let other_mob_member_ref =
        meerkat_contracts::WireMemberRef::encode("another-mob", "same-name-other-mob-child");
    for (name, detail) in [
        (
            "other-identity-child",
            serde_json::json!({
                "agent_identity": "another-child",
                "status": "failed",
            }),
        ),
        (
            "same-name-other-mob-child",
            serde_json::json!({
                "agent_identity": "same-name-other-mob-child",
                "member_ref": other_mob_member_ref,
                "status": "max_run_elapsed",
            }),
        ),
    ] {
        assert!(
            !relink_after_a_delivered_record(
                &fixture,
                &handle,
                &owner,
                name,
                meerkat_core::event::BackgroundJobTerminalStatus::Terminated,
                detail,
            )
            .await,
            "{name}: another child's record does not retire it"
        );
    }
    fixture.teardown().await;
}

/// A job turn that fails on its own retires its child once that outcome is
/// settled, but a delivery deduplicated against a record admitted meanwhile
/// (here another pass delivered `restart_interrupted` for the job while this
/// one watched the turn) delivered nothing: the child is retired only when
/// the committed record's outcome retires it, and this one stays seated.
/// (Review: every deduplicated delivery of a failure retired the child.)
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_turn_deduplicated_against_restart_interrupted_keeps_its_child_seated() {
    let gate = TurnGate::new();
    let turn_gate = Arc::clone(&gate);
    let fixture = CouncilFixture::new(move |request| {
        if support::last_user_text(request).contains(CHILD_TASK) {
            ScriptedTurn::GatedFail(Arc::clone(&turn_gate), "provider rejected the turn".into())
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
    let runtime = relink_runtime(&fixture).expect("runtime-backed fixture");
    let name = "failed-dedup-child";
    let job_id = "job-failed-dedup";
    let (child, _child_session) =
        fork_held_child(&fixture, &handle, &gate, name, job_id, &owner).await;
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    assert!(job.turn_delivery.is_some());

    // The re-link passes its entry check (nothing delivered yet) and is held
    // at its first status read while the turn still runs.
    let (status_entered, release_status) =
        meerkat_mob::MobHandle::arm_member_status_read_test_gate(child.clone());
    let relink = tokio::spawn({
        let service = fixture.state.session_service();
        let delivery = relink_delivery(&fixture);
        let mob_id = fixture.source_mob_id();
        let handle = handle.clone();
        let child = child.clone();
        async move {
            meerkat_mob_mcp::fork_relink::relink_child(
                service, &delivery, &mob_id, &handle, &child, &job,
            )
            .await
            .action
        }
    });
    status_entered
        .await
        .expect("the re-link reads the child's status");
    support::admit_pre_upgrade_completion(
        &runtime,
        &handle,
        &AgentIdentity::from("forker"),
        &owner,
        "fork_off",
        job_id,
        meerkat_core::event::BackgroundJobTerminalStatus::Failed,
        serde_json::json!({
            "agent_identity": name,
            "status": "restart_interrupted",
        }),
    )
    .await
    .expect("another pass delivers restart_interrupted");
    await_completion_record(&fixture, &owner, job_id).await;
    // The turn now fails on its own; its receipt is the job's outcome.
    gate.open();
    release_status
        .send(())
        .expect("the status read was still held");

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), relink)
            .await
            .expect("the failed turn's receipt settles the job")
            .unwrap(),
        ForkRelinkAction::AlreadyDelivered
    );
    assert_eq!(completion_records(&fixture, &owner, job_id).await, 1);
    let outcome = completion_record_outcome(&fixture, &owner, job_id).await;
    assert_eq!(outcome["status"], "restart_interrupted", "{outcome}");
    assert!(
        handle.get_member(&child).await.unwrap().is_some(),
        "the child whose delivered outcome is restart_interrupted stays seated"
    );
    fixture.teardown().await;
}

/// #1227 (a): a crash can land after a retiring outcome's completion is
/// admitted to the forker and before its child is retired. The admitted input
/// is durable, but the forker's run has not committed the record into its
/// transcript yet, so the committed record alone says nothing. The re-link
/// reads the outcome the pending input carries and retires the child. The
/// forker's run is held before its terminal commit by a typed barrier, which
/// is exactly the state that crash leaves on restart.
#[tokio::test(flavor = "multi_thread")]
async fn a_retiring_outcome_admitted_but_not_yet_committed_retires_its_child() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let runtime = relink_runtime(&fixture).expect("runtime-backed fixture");
    let name = "admitted-uncommitted-child";
    let child = AgentIdentity::from(name);
    let job_id = format!("job-{name}");
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec(name),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: job_id.clone(),
                owner_session_id: owner.clone(),
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    assert!(matches!(
        run.outcome().await,
        Some(ForkChildRunOutcome::Completed(_))
    ));
    let (commit_entered, release_commit) =
        runtime.arm_runtime_loop_before_terminal_commit_test_hook(owner.clone());
    support::admit_pre_upgrade_completion(
        &runtime,
        &handle,
        &AgentIdentity::from("forker"),
        &owner,
        "fork_off",
        &job_id,
        meerkat_core::event::BackgroundJobTerminalStatus::Failed,
        serde_json::json!({ "agent_identity": name, "status": "failed" }),
    )
    .await
    .expect("admit the retiring outcome");
    commit_entered
        .await
        .expect("the forker's run takes the completion up and reaches its commit");
    assert_eq!(
        completion_records(&fixture, &owner, &job_id).await,
        0,
        "the record is admitted but not committed"
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
    .await
    .action;
    assert_eq!(action, ForkRelinkAction::AlreadyDelivered);
    assert!(
        handle.get_member(&child).await.unwrap().is_none(),
        "the admitted outcome retires the child before its record is committed"
    );
    release_commit
        .send(())
        .expect("the forker's commit was still held");
    await_completion_record(&fixture, &owner, &job_id).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    fixture.teardown().await;
}

/// Fork child `name` for job `job-{name}` and let its turn complete; admit a
/// completion record for that job to the forker (`notice` status, `detail`
/// its detail), as if delivered before a crash that skipped the retirement
/// after it; then re-link the child. Returns whether the child was retired.
async fn relink_after_a_delivered_record(
    fixture: &CouncilFixture,
    handle: &meerkat_mob::MobHandle,
    owner: &meerkat_core::SessionId,
    name: &str,
    notice: meerkat_core::event::BackgroundJobTerminalStatus,
    detail: serde_json::Value,
) -> bool {
    let runtime = relink_runtime(fixture).expect("runtime-backed fixture");
    let child = AgentIdentity::from(name);
    let job_id = format!("job-{name}");
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec(name),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: job_id.clone(),
                owner_session_id: owner.clone(),
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    assert!(matches!(
        run.outcome().await,
        Some(ForkChildRunOutcome::Completed(_))
    ));
    support::admit_pre_upgrade_completion(
        &runtime,
        handle,
        &AgentIdentity::from("forker"),
        owner,
        "fork_off",
        &job_id,
        notice,
        detail,
    )
    .await
    .expect("pre-admit the completion");
    await_completion_record(fixture, owner, &job_id).await;
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");

    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(fixture),
        &fixture.source_mob_id(),
        handle,
        &child,
        &job,
    )
    .await
    .action;
    assert_eq!(action, ForkRelinkAction::AlreadyDelivered, "{name}");
    assert_eq!(completion_records(fixture, owner, &job_id).await, 1);
    handle.get_member(&child).await.unwrap().is_none()
}

/// Fork a child whose turn is held at its model call, and return the child,
/// its session and the gate that holds the turn.
async fn fork_held_child(
    fixture: &CouncilFixture,
    handle: &meerkat_mob::MobHandle,
    gate: &Arc<TurnGate>,
    child: &str,
    job_id: &str,
    owner: &meerkat_core::SessionId,
) -> (AgentIdentity, meerkat_core::SessionId) {
    let _ = fixture;
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
                job_id: job_id.to_string(),
                owner_session_id: owner.clone(),
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    end_old_process_supervisor(run).await;
    gate.wait_entered(1).await;
    let child = AgentIdentity::from(child);
    let child_session = handle
        .resolve_bridge_session_id(&child)
        .await
        .expect("child session");
    (child, child_session)
}

fn held_child_fixture(gate: &Arc<TurnGate>) -> CouncilFixture {
    let turn_gate = Arc::clone(gate);
    CouncilFixture::new(move |request| {
        if support::last_user_text(request).contains(CHILD_TASK) {
            ScriptedTurn::Gated(Arc::clone(&turn_gate), CHILD_REPLY.to_string())
        } else {
            ScriptedTurn::Text("noted".to_string())
        }
    })
}

/// The typed arm, without a timeout anywhere: the child's turn is terminal in
/// its live agent while its commit has not started, so its session driver is
/// free and its run input is still applied. The runtime answers `true` at
/// once, and the re-link keeps watching on that machine evidence; once the
/// commit lands it delivers the real reply.
#[tokio::test(flavor = "multi_thread")]
async fn relink_waits_while_a_finished_turn_s_run_input_awaits_its_boundary() {
    let gate = TurnGate::new();
    let fixture = held_child_fixture(&gate);
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-applied-input";
    let (child, child_session) = fork_held_child(
        &fixture,
        &handle,
        &gate,
        "applied-input-child",
        job_id,
        &owner,
    )
    .await;
    let runtime =
        meerkat_mob::MobSessionService::acquire_runtime_adapter(fixture.service.as_ref(), None)
            .expect("acquire runtime authority")
            .expect("the service derives its runtime");
    let (entered, release) =
        runtime.arm_runtime_loop_before_terminal_commit_test_hook(child_session.clone());
    gate.open();
    entered.await.expect("the finished turn reaches its commit");
    assert!(
        tokio::time::timeout(
            Duration::from_secs(1),
            runtime.session_has_uncommitted_run_input(&child_session),
        )
        .await
        .expect("the driver is free: the read answers at once")
        .expect("healthy durability"),
        "the finished turn's run input awaits its boundary"
    );
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    let relink = tokio::spawn({
        let service = fixture.state.session_service();
        let delivery = relink_delivery(&fixture);
        let mob_id = fixture.source_mob_id();
        let handle = handle.clone();
        let child = child.clone();
        async move {
            meerkat_mob_mcp::fork_relink::relink_child(
                service, &delivery, &mob_id, &handle, &child, &job,
            )
            .await
            .action
        }
    });
    // Many observations of a settled member whose commit has not landed.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        completion_records(&fixture, &owner, job_id).await,
        0,
        "the re-link reported the child before its turn committed"
    );
    assert!(!relink.is_finished());
    let _ = release.send(());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), relink)
            .await
            .expect("the re-link finishes once the commit lands")
            .unwrap(),
        ForkRelinkAction::Delivered
    );
    await_completion_record(&fixture, &owner, job_id).await;
    assert_eq!(completion_records(&fixture, &owner, job_id).await, 1);
    let record = completion_record_text(&fixture, &owner, job_id).await;
    assert!(
        record.contains(CHILD_REPLY) && !record.contains("restart_interrupted"),
        "the child is delivered its real reply once its turn is committed: {record}"
    );
    fixture.teardown().await;
}

/// #1227 (b): at the ceiling the job turn's input can read not terminal while
/// it stays admitted in the child's runtime: here its run finished and is
/// held before its terminal commit. A `restart_interrupted` fixed on that
/// reading is contradicted as soon as the run answers the input. The re-link
/// fences the exact input first (it settles that input, then the job's
/// outcome is its terminal), so it fixes nothing before the fence, and the
/// run that answers the input before the fence cancels anything wins. Every
/// step is ordered by typed barriers: the held commit, the fence's gate and
/// the receipt.
#[tokio::test(flavor = "multi_thread")]
async fn a_ceiling_outcome_is_fixed_only_after_the_job_input_is_fenced() {
    let gate = TurnGate::new();
    let fixture = held_child_fixture(&gate);
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-fenced-at-the-ceiling";
    let (child, child_session) = fork_held_child(
        &fixture,
        &handle,
        &gate,
        "fenced-at-the-ceiling-child",
        job_id,
        &owner,
    )
    .await;
    let runtime =
        meerkat_mob::MobSessionService::acquire_runtime_adapter(fixture.service.as_ref(), None)
            .expect("acquire runtime authority")
            .expect("the service derives its runtime");
    let (commit_entered, release_commit) =
        runtime.arm_runtime_loop_before_terminal_commit_test_hook(child_session.clone());
    gate.open();
    commit_entered
        .await
        .expect("the finished turn reaches its commit");
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    let turn_delivery = job.turn_delivery.clone().expect("receipt-anchored job");

    // A zero ceiling: the first idle reading decides.
    let (fence_entered, release_fence) =
        meerkat_mob::MobHandle::arm_delivery_input_settle_test_gate(child.clone());
    let mut relink = tokio::spawn({
        let service = fixture.state.session_service();
        let delivery = relink_delivery(&fixture);
        let mob_id = fixture.source_mob_id();
        let handle = handle.clone();
        let child = child.clone();
        async move {
            meerkat_mob_mcp::fork_relink::relink_child_within(
                service,
                &delivery,
                &mob_id,
                &handle,
                &child,
                &job,
                Duration::ZERO,
            )
            .await
            .action
        }
    });
    tokio::select! {
        action = &mut relink => panic!(
            "the re-link fixed the job's outcome while its input was still admitted: {action:?}"
        ),
        entered = fence_entered => entered.expect("the re-link fences the job input"),
    }
    assert_eq!(
        completion_records(&fixture, &owner, job_id).await,
        0,
        "nothing is delivered before the fence"
    );

    // The held run answers the input before the fence cancels anything.
    release_commit
        .send(())
        .expect("the finished turn's commit was still held");
    let spec = meerkat_mob::BoundedResultSpec::new("fork_off_result".to_string(), 16 * 1024)
        .expect("bounded result spec");
    let answered = handle
        .wait_bounded_work_for_identity_with_delivery_identity(
            &child,
            &turn_delivery,
            &spec,
            meerkat_core::time_compat::Instant::now() + Duration::from_secs(30),
        )
        .await
        .expect("read the job turn's receipt");
    assert!(
        matches!(
            answered.work(),
            meerkat_mob::DeliveryTerminalWait::Terminal(record)
                if matches!(
                    record.resolution(),
                    meerkat_mob::DeliveryTerminalResolution::Receipt { result: Ok(_), .. }
                )
        ),
        "the released run answered the job input: {answered:?}"
    );
    release_fence
        .send(meerkat_mob::DeliveryInputSettleTestRelease::Proceed)
        .expect("the fence was still held");

    assert_eq!(relink.await.unwrap(), ForkRelinkAction::Delivered);
    await_completion_record(&fixture, &owner, job_id).await;
    assert_eq!(completion_records(&fixture, &owner, job_id).await, 1);
    let outcome = completion_record_outcome(&fixture, &owner, job_id).await;
    assert_eq!(
        outcome["status"], "completed",
        "the outcome is the fenced input's terminal: {outcome}"
    );
    assert_eq!(outcome["bounded_result"]["text"], CHILD_REPLY, "{outcome}");
    fixture.teardown().await;
}

/// #1227 (b), the unsettled arm: when the fence cannot settle the job input
/// (injected here as a failed cancellation, as a run that is no longer
/// current leaves it), the re-link retires the child, which fences the input
/// for good, and delivers the input's terminal if one landed first. Here the
/// held run answers the input while the retirement waits for its boundary, so
/// the job is `completed`, never a bare `Failed` left for a restart.
#[tokio::test(flavor = "multi_thread")]
async fn an_unsettled_fence_retires_the_child_and_delivers_a_definitive_outcome() {
    let gate = TurnGate::new();
    let fixture = held_child_fixture(&gate);
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-unsettled-fence";
    let (child, child_session) = fork_held_child(
        &fixture,
        &handle,
        &gate,
        "unsettled-fence-child",
        job_id,
        &owner,
    )
    .await;
    let runtime =
        meerkat_mob::MobSessionService::acquire_runtime_adapter(fixture.service.as_ref(), None)
            .expect("acquire runtime authority")
            .expect("the service derives its runtime");
    let (commit_entered, release_commit) =
        runtime.arm_runtime_loop_before_terminal_commit_test_hook(child_session.clone());
    gate.open();
    commit_entered
        .await
        .expect("the finished turn reaches its commit");
    let job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");

    let (fence_entered, release_fence) =
        meerkat_mob::MobHandle::arm_delivery_input_settle_test_gate(child.clone());
    let mut relink = tokio::spawn({
        let service = fixture.state.session_service();
        let delivery = relink_delivery(&fixture);
        let mob_id = fixture.source_mob_id();
        let handle = handle.clone();
        let child = child.clone();
        async move {
            meerkat_mob_mcp::fork_relink::relink_child_within(
                service,
                &delivery,
                &mob_id,
                &handle,
                &child,
                &job,
                Duration::ZERO,
            )
            .await
            .action
        }
    });
    tokio::select! {
        action = &mut relink => panic!(
            "the re-link fixed the job's outcome before fencing its input: {action:?}"
        ),
        entered = fence_entered => entered.expect("the re-link fences the job input"),
    }
    release_fence
        .send(meerkat_mob::DeliveryInputSettleTestRelease::FailCancellation)
        .expect("the fence was still held");
    release_commit
        .send(())
        .expect("the finished turn's commit was still held");

    assert_eq!(relink.await.unwrap(), ForkRelinkAction::Delivered);
    assert!(
        handle.get_member(&child).await.unwrap().is_none(),
        "the unsettled fence retired the child"
    );
    await_completion_record(&fixture, &owner, job_id).await;
    assert_eq!(completion_records(&fixture, &owner, job_id).await, 1);
    let outcome = completion_record_outcome(&fixture, &owner, job_id).await;
    assert_eq!(
        outcome["status"], "completed",
        "the input's terminal that landed first is the outcome: {outcome}"
    );
    fixture.teardown().await;
}

/// #1227 (b), the fence's own cancellation: a job input still queued behind
/// an idle child (as a restart requeues it before the recovered run opens)
/// reads not terminal at the ceiling. The fence abandons that exact input, so
/// the job settles `restart_interrupted` on the input's own terminal (no run
/// can answer an abandoned input), and the child stays seated. The child's
/// run loop is held before it takes queue authority, a typed barrier.
#[tokio::test(flavor = "multi_thread")]
async fn the_fence_cancels_a_queued_job_input_before_restart_interrupted() {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let runtime = relink_runtime(&fixture).expect("runtime-backed fixture");
    let name = "queued-fence-child";
    let child = AgentIdentity::from(name);
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec(name),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: format!("job-{name}-first"),
                owner_session_id: owner.clone(),
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    assert!(matches!(
        run.outcome().await,
        Some(ForkChildRunOutcome::Completed(_))
    ));
    let child_session = handle
        .resolve_bridge_session_id(&child)
        .await
        .expect("child session");

    // A second job turn, admitted under its own delivery identity while the
    // child's run loop is held before queue authority: it stays queued.
    let (loop_entered, release_loop) =
        runtime.arm_runtime_loop_before_queue_authority_test_hook(child_session.clone());
    let job_id = format!("job-{name}");
    let turn_delivery = meerkat_mob::store::MobDeliveryIdentity::new(
        format!("{job_id}-turn"),
        uuid::Uuid::new_v4().to_string(),
    )
    .expect("delivery identity");
    let mut prompt = meerkat_runtime::PromptInput::new(CHILD_TASK, None);
    prompt.header.idempotency_key = Some(meerkat_runtime::identifiers::IdempotencyKey::new(
        turn_delivery.idempotency_key.clone(),
    ));
    let (accepted, _completion) = runtime
        .accept_input_with_completion(&child_session, meerkat_runtime::Input::Prompt(prompt))
        .await
        .expect("admit the queued job turn");
    assert!(
        matches!(accepted, meerkat_runtime::AcceptOutcome::Accepted { .. }),
        "{accepted:?}"
    );
    loop_entered
        .await
        .expect("the child's run loop is held before queue authority");
    let mut job = handle
        .roster()
        .await
        .get_by_identity(&child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    // The queued turn is the job's own (its job id stays the spawned one, as
    // a committed terminal must name a job the mob spawned).
    let job_id = job.job_id.clone();
    job.turn_delivery = Some(turn_delivery.clone());

    let action = meerkat_mob_mcp::fork_relink::relink_child_within(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
        Duration::ZERO,
    )
    .await
    .action;
    assert_eq!(action, ForkRelinkAction::Delivered);
    await_completion_record(&fixture, &owner, &job_id).await;
    let outcome = completion_record_outcome(&fixture, &owner, &job_id).await;
    assert_eq!(outcome["status"], "restart_interrupted", "{outcome}");
    assert!(
        handle.get_member(&child).await.unwrap().is_some(),
        "a settled restart_interrupted keeps its child seated"
    );

    // The fence abandoned the exact input: releasing the loop runs nothing.
    let spec = meerkat_mob::BoundedResultSpec::new("fork_off_result".to_string(), 16 * 1024)
        .expect("bounded result spec");
    let settled = handle
        .wait_bounded_work_for_identity_with_delivery_identity(
            &child,
            &turn_delivery,
            &spec,
            meerkat_core::time_compat::Instant::now() + Duration::from_secs(5),
        )
        .await
        .expect("read the job turn's receipt");
    assert!(
        matches!(
            settled.work(),
            meerkat_mob::DeliveryTerminalWait::Terminal(record)
                if matches!(
                    record.terminal(),
                    meerkat_runtime::InputTerminalOutcome::Abandoned { .. }
                )
        ),
        "the fence's cancellation is the input's terminal: {settled:?}"
    );
    release_loop.send(()).expect("the run loop was still held");
    fixture.teardown().await;
}

/// A host that restores a stopped mob inserts its handle before activating
/// it (Meta's identity-first gateway after a clean shutdown). The forker is
/// not live and cannot be revived while its mob is stopped: the re-link
/// submits the outcome durably at once, and the host's delivery owner
/// applies it once the mob runs, exactly once, with no re-link pass of its
/// own (P3-B B1: the outcome used to wait on a bounded revival loop).
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
                retained_work: None,
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
    let restarted = Arc::new(
        meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            fixture.service.clone(),
            Some(Arc::clone(&runtime)),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("construct runtime authority"),
    );
    fixture.bind_continuations(&restarted);
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await;

    let reports = restarted.relink_restored_fork_children().await;
    let report = reports
        .iter()
        .find(|report| report.job_id == job_id)
        .expect("the child is visited");
    assert_eq!(report.action, ForkRelinkAction::Delivered);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 0);

    // The host activates the mob: the delivery owner applies the outcome.
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
                    retained_work: None,
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
        let state = Arc::new(
            meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
                self.fixture.service.clone(),
                Some(Arc::clone(&self.runtime)),
                meerkat_mob::MobControlPrincipal::Owner,
            )
            .expect("construct runtime authority"),
        );
        self.fixture.bind_continuations(&state);
        state
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

/// The same owner on a host without an owner hook: the re-link submits the
/// outcome durably, but nothing can make the session live, so nothing is
/// recorded and the owner is not woken. The outcome stays owed and is
/// applied, once, when the session is attached again, with no re-link pass
/// of its own (P3-B B2: a completion its live path could not deliver used to
/// wait for the next restart).
#[tokio::test(flavor = "multi_thread")]
async fn without_a_host_hook_a_plain_session_owner_is_delivered_on_attach() {
    let job = PlainOwnerJob::after_restart("unhooked").await;
    let restarted = job.restarted_state();
    restarted
        .mob_insert_handle(job.fixture.source_mob_id(), job.handle.clone())
        .await;
    let reports = restarted.relink_restored_fork_children().await;
    assert_eq!(
        reports
            .iter()
            .find(|report| report.job_id == job.job_id)
            .map(|report| report.action.clone()),
        Some(ForkRelinkAction::Delivered),
        "the outcome is submitted durably"
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(job.records().await, 0, "nothing is recorded");
    assert_eq!(job.seen.turns_that_saw(&job.marker()), 0, "no wake");

    // The session is attached again: the owed outcome is applied, once.
    job.owner_handle
        .ensure_member_live(&AgentIdentity::from("owner"))
        .await
        .expect("attach the owner's session");
    await_completion_record(&job.fixture, &job.owner, &job.job_id).await;
    job.await_one_wake().await;
    assert_eq!(job.records().await, 1, "recorded exactly once");
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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    end_old_process_supervisor(run).await;
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
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await
    .action;
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

/// When the limit won while the owner cannot be served yet (here a plain
/// session owner on a host without an owner hook), max_run_elapsed is
/// committed and submitted durably, so the child is retired at once: the
/// outcome no longer depends on the child's job record. It is applied, once,
/// when the owner's session is attached again.
#[tokio::test(flavor = "multi_thread")]
async fn relink_past_max_run_retires_the_child_once_its_outcome_is_durable() {
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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    gate.wait_entered(1).await;
    end_old_process_supervisor(run).await;
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

    let action = meerkat_mob_mcp::fork_relink::relink_child(
        fixture.state.session_service(),
        &relink_delivery(&fixture),
        &fixture.source_mob_id(),
        &handle,
        &child,
        &job,
    )
    .await
    .action;
    gate.open();
    assert_eq!(action, ForkRelinkAction::Delivered, "submitted durably");
    assert!(
        handle.get_member(&child).await.unwrap().is_none(),
        "the child is retired once its outcome is durable"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 0);

    // The owner's session is attached again: applied once.
    owner_handle
        .ensure_member_live(&AgentIdentity::from("owner"))
        .await
        .expect("attach the owner's session");
    await_completion_record(&fixture, &owner, &job_id).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);
    let record = completion_record_text(&fixture, &owner, &job_id).await;
    assert!(record.contains("max_run_elapsed"), "{record}");
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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    end_old_process_supervisor(run).await;
    let mut job = handle
        .roster()
        .await
        .get_by_identity(child)
        .and_then(|entry| entry.fork_job.clone())
        .expect("durable fork job record");
    job.max_run_ms = Some(now_ms().saturating_sub(job.started_at_ms).max(1));
    job
}

/// Overdue children whose owner cannot be served yet each get max_run_elapsed,
/// every time: the limit has passed, so the pass does not race a status read
/// against a zero-length timer (lifecycle review: an idle child usually
/// answered first and was delivered restart_interrupted). The outcomes are
/// durable at once and applied when the owner is attached again.
#[tokio::test(flavor = "multi_thread")]
async fn overdue_children_each_get_max_run_elapsed() {
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
    let delivery = relink_delivery(&fixture);
    for (child, job) in &jobs {
        let action = meerkat_mob_mcp::fork_relink::relink_child(
            fixture.state.session_service(),
            &delivery,
            &fixture.source_mob_id(),
            &handle,
            child,
            job,
        )
        .await
        .action;
        assert_eq!(action, ForkRelinkAction::Delivered, "{child}");
        assert!(handle.get_member(child).await.unwrap().is_none(), "{child}");
    }
    gate.open();
    owner_handle
        .ensure_member_live(&AgentIdentity::from("owner"))
        .await
        .expect("attach the owner's session");
    for (child, job) in &jobs {
        await_completion_record(&fixture, &owner, &job.job_id).await;
        let record = completion_record_text(&fixture, &owner, &job.job_id).await;
        assert!(
            record.contains("max_run_elapsed"),
            "{child} is delivered its limit, not a settled outcome: {record}"
        );
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
                retained_work: None,
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
    let delivered = support::admit_pre_upgrade_session_completion(
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
    .await
    .action;
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
    end_old_process_supervisor(grandchild_run).await;
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
    .await
    .action;
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
                retained_work: None,
            }),
        )
        .await
        .expect("fork");
    end_old_process_supervisor(run).await;
    gate.wait_entered(1).await;
    let runtime = fixture.runtime_adapter.clone().expect("runtime-backed");
    runtime
        .unregister_session(&owner)
        .await
        .expect("the owner is not live after the restart");
    tokio::time::sleep(Duration::from_millis(5)).await;

    let restarted = Arc::new(
        meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            fixture.service.clone(),
            Some(Arc::clone(&runtime)),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("construct runtime authority"),
    );
    fixture.bind_continuations(&restarted);
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
/// the owner not live. The outcome is submitted durably to the owner's
/// incarnation, and applied when only B resumes, once, whatever A does
/// (lifecycle review: a wait on the child's running mob spent every attempt
/// at once and left nothing waiting when B resumed).
#[tokio::test(flavor = "multi_thread")]
async fn an_owner_in_another_stopped_mob_receives_its_outcome_when_that_mob_runs() {
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
                retained_work: None,
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
    let restarted = Arc::new(
        meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            fixture.service.clone(),
            Some(Arc::clone(&runtime)),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("construct runtime authority"),
    );
    fixture.bind_continuations(&restarted);
    restarted
        .mob_insert_handle(owner_handle.mob_id().clone(), owner_handle.clone())
        .await;
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await;

    let reports = restarted.relink_restored_fork_children().await;
    assert_eq!(
        reports
            .iter()
            .find(|report| report.job_id == job_id)
            .map(|report| report.action.clone()),
        Some(ForkRelinkAction::Delivered),
        "submitted durably to the owner's incarnation"
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 0);

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
    assert!(
        completion_record_text(&fixture, &owner, &job_id)
            .await
            .contains(CHILD_REPLY)
    );
    fixture.teardown().await;
}

/// Two children of one mob whose jobs share an id (bindings are the host's;
/// nothing makes a job id unique across children), bound to owners in two
/// different mobs, both stopped with their owners not live. Each outcome is
/// committed and submitted as exactly its child's job: when only C resumes,
/// Y is delivered once and X keeps waiting on B; when B resumes, X is
/// delivered once (lifecycle review: a retry selected every child with the
/// job id and adopted the first report, so a child could take the other's).
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
                    retained_work: None,
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
    let restarted = Arc::new(
        meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            fixture.service.clone(),
            Some(Arc::clone(&runtime)),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("construct runtime authority"),
    );
    fixture.bind_continuations(&restarted);
    for owner_mob in [&b_handle, &c_handle] {
        restarted
            .mob_insert_handle(owner_mob.mob_id().clone(), owner_mob.clone())
            .await;
    }
    restarted
        .mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await;
    let reports = restarted.relink_restored_fork_children().await;
    let shared: Vec<_> = reports
        .iter()
        .filter(|report| report.job_id == job_id)
        .map(|report| report.action.clone())
        .collect();
    assert_eq!(
        shared,
        vec![ForkRelinkAction::Delivered; 2],
        "both outcomes are submitted durably"
    );

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
    assert_eq!(
        completion_records(&fixture, &b_owner, &job_id).await,
        0,
        "X still waits, on B"
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
    fixture.teardown().await;
}

/// The worker stack size, in bytes, of the child processes the stack-budget
/// tests below start. Their fixtures do nothing without it; a diagnosis can
/// run a fixture with a smaller size to measure its headroom.
const RELINK_WORKER_STACK_ENV: &str = "MEERKAT_TEST_FORK_RELINK_WORKER_STACK_BYTES";
/// The child-process fixture of [`restart_relink_fits_the_worker_stack_budget`].
const RELINK_STACK_CHILD_TEST: &str = "restart_relink_on_a_sized_worker_stack_child";
/// The child-process fixture of [`session_create_fits_the_worker_stack_budget`].
const SESSION_CREATE_STACK_CHILD_TEST: &str = "session_create_on_a_sized_worker_stack_child";
/// The worker stack, 1.5 MiB, that the stack-budget tests give their child
/// processes. It is below the 2 MiB an embedding host gives its tokio
/// runtime (Tokio's default), so the paths they cover keep headroom under
/// that limit.
///
/// The worst path on these workers is the member session task's turn. A
/// member's turns run on the host's runtime, where they ran on Core's
/// cleanup runtime before, so the fork child's text-only turn is polled on
/// these workers, beneath the session task's own poll frame. In a debug build
/// the restart re-link scenario needs 1,343,488 bytes for it (1,331,200
/// overflows, at the run loop's first model-request boundary).
///
/// Before that move the worst path was the scenario's runtime delivery owner
/// admitting the re-linked child's continuation: 999,424 bytes (983,040
/// overflowed) once the resume path built the resume preparation's hosted
/// body and recovery in their own frames (the resume path then reached
/// 861,512), and 1,228,800 (1,212,416 overflowed) while it built them
/// inline. The budget was 1,179,648 then. Session creation, which runs no
/// turn, needed 901,120.
///
/// This budget leaves about 17% (229,376 bytes) over the 1,343,488 need and
/// stays 25% (524,288 bytes) under a host's 2 MiB, so a regression that eats
/// that headroom fails here before it nears a host's limit.
const WORKER_STACK_BUDGET: usize = 1_572_864;
/// The stack of a child's driver thread, which builds the fixture and waits
/// on the scenario: the debug test-thread budget `.cargo/config.toml` sets.
/// What the scenario measures runs on the workers.
const STACK_CHILD_DRIVER_STACK: usize = 8 * 1024 * 1024;
/// How long the parent waits for a child test process before killing it.
const STACK_CHILD_DEADLINE: Duration = Duration::from_mins(5);
/// How long a child's scenario may run.
const STACK_CHILD_SCENARIO_DEADLINE: Duration = Duration::from_mins(3);

/// A restarted host re-links a fork child on a tokio worker with the
/// [`WORKER_STACK_BUDGET`], in a debug build, and then destroys the restored
/// mob from a task on its workers.
/// Inserting the restored mob's handle spawns the re-link pass as a task. The
/// pass first reads the forker's durable input index for the job's completion
/// through the runtime's machine command dispatch, then settles the child and
/// delivers its outcome. At opt-level 0 every branch of a function reserves
/// its own stack slots, so the dispatch matches and the re-link's per-child
/// futures had poll frames that together needed more than 2 MiB: an
/// embedding host's worker overflowed its guard page on restart. The mob
/// actor's destroy of the restored mob runs on a worker as well. A stack
/// overflow aborts the whole process, so the scenario runs in a child
/// process, and this test asserts that the child exits successfully.
#[test]
fn restart_relink_fits_the_worker_stack_budget() {
    assert_stack_child_passes(RELINK_STACK_CHILD_TEST);
}

/// An embedding host constructs sessions from tasks on tokio workers with the
/// [`WORKER_STACK_BUDGET`], in a debug build: it creates a runtime-backed
/// session (the agent build, the session's runtime registration and its
/// materialization run on the workers), then builds a mob member, whose
/// session the mob actor provisions on a worker as well, and finally
/// destroys the mob from a task on its workers. Run in a child process, like
/// [`restart_relink_fits_the_worker_stack_budget`].
#[test]
fn session_create_fits_the_worker_stack_budget() {
    assert_stack_child_passes(SESSION_CREATE_STACK_CHILD_TEST);
}

/// Run this binary's ignored child-process fixture `fixture_test` on workers
/// with the [`WORKER_STACK_BUDGET`], and assert that the child passes. A
/// stack overflow aborts the whole process, so it is observed from outside.
fn assert_stack_child_passes(fixture_test: &str) {
    let output_dir = tempfile::tempdir().expect("child output dir");
    let output_path = output_dir.path().join("child-output.txt");
    let stdout = std::fs::File::create(&output_path).expect("child output file");
    let stderr = stdout.try_clone().expect("child output file");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            fixture_test,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(RELINK_WORKER_STACK_ENV, WORKER_STACK_BUDGET.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("start the child test process");
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the child test process") {
            break status;
        }
        if started.elapsed() > STACK_CHILD_DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "the child test process did not finish within {STACK_CHILD_DEADLINE:?}\n{}",
                child_output_tail(&output_path)
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let output = std::fs::read_to_string(&output_path).unwrap_or_default();
    assert!(
        status.success() && output.contains("test result: ok. 1 passed"),
        "{fixture_test} must fit a {WORKER_STACK_BUDGET}-byte worker stack; the child test \
         process exited with {status}\n{}",
        child_output_tail(&output_path)
    );
}

/// The last lines of the child test process's output, where a stack overflow
/// names its thread.
fn child_output_tail(path: &std::path::Path) -> String {
    let output = std::fs::read_to_string(path).unwrap_or_default();
    let lines = output.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(200)..].join("\n")
}

/// The child process of [`restart_relink_fits_the_worker_stack_budget`].
#[test]
#[ignore = "child-process fixture of restart_relink_fits_the_worker_stack_budget"]
fn restart_relink_on_a_sized_worker_stack_child() {
    run_on_sized_workers(restart_relink_on_a_worker_scenario);
}

/// The child process of [`session_create_fits_the_worker_stack_budget`].
#[test]
#[ignore = "child-process fixture of session_create_fits_the_worker_stack_budget"]
fn session_create_on_a_sized_worker_stack_child() {
    run_on_sized_workers(session_create_on_a_worker_scenario);
}

/// Run `scenario` on a runtime whose workers have the stack size
/// [`RELINK_WORKER_STACK_ENV`] gives. Does nothing without it, so the
/// fixtures stay inert unless a stack-budget test starts them.
fn run_on_sized_workers<F>(scenario: fn() -> F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    let Some(worker_stack) = std::env::var_os(RELINK_WORKER_STACK_ENV) else {
        return;
    };
    let worker_stack = worker_stack
        .to_str()
        .and_then(|bytes| bytes.parse::<usize>().ok())
        .expect("the worker stack size in bytes");
    let driver = std::thread::Builder::new()
        .name("stack-child-driver".into())
        .stack_size(STACK_CHILD_DRIVER_STACK)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .thread_name("stack-child-worker")
                .thread_stack_size(worker_stack)
                .enable_all()
                .build()
                .expect("runtime with sized workers");
            runtime.block_on(async {
                tokio::time::timeout(STACK_CHILD_SCENARIO_DEADLINE, Box::pin(scenario()))
                    .await
                    .expect("the scenario finishes in time");
            });
            runtime.shutdown_timeout(Duration::from_secs(10));
        })
        .expect("start the driver thread");
    if let Err(panic) = driver.join() {
        std::panic::resume_unwind(panic);
    }
}

/// A child finished its job, but its custodian died with the old process.
/// The restarted host inserts the surviving mob's handle, which spawns the
/// re-link pass on one of the runtime's workers, and the pass delivers the
/// child's outcome once.
async fn restart_relink_on_a_worker_scenario() {
    let fixture =
        CouncilFixture::new_runtime_backed(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker"]).await;
    let owner = forker_session(&fixture).await;
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let job_id = "job-relinked-on-a-2-mib-worker".to_string();
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &AgentIdentity::from("forker"),
            child_spec("relink-stack-child"),
            None,
            "fork_off_result",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: job_id.clone(),
                owner_session_id: owner.clone(),
                retained_work: None,
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

    tokio::time::sleep(Duration::from_millis(5)).await;
    let runtime = fixture.runtime_adapter.clone().expect("runtime-backed");
    let restarted = Arc::new(
        meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            fixture.service.clone(),
            Some(runtime),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("construct runtime authority"),
    );
    fixture.bind_continuations(&restarted);
    restarted
        .try_mob_insert_handle(fixture.source_mob_id(), handle.clone())
        .await
        .expect("insert the restored mob");
    await_completion_record(&fixture, &owner, &job_id).await;
    assert_eq!(completion_records(&fixture, &owner, &job_id).await, 1);

    // The host destroys the restored mob from a task on its workers.
    tokio::spawn({
        let restarted = Arc::clone(&restarted);
        let mob_id = fixture.source_mob_id();
        async move { restarted.mob_destroy(&mob_id).await }
    })
    .await
    .expect("the destroy task")
    .expect("destroy the restored mob");
    fixture.teardown().await;
}

/// An embedding host constructs sessions from tasks on its workers: it
/// creates a runtime-backed session (agent build, runtime registration and
/// materialization; its turns are the runtime's), then builds a mob member,
/// whose session the mob actor provisions on a worker as well, and destroys
/// the mob from a task on its workers.
async fn session_create_on_a_worker_scenario() {
    use meerkat_core::service::SessionService as _;
    let fixture = Arc::new(CouncilFixture::new_runtime_backed(|_| {
        ScriptedTurn::Text(CHILD_REPLY.to_string())
    }));
    let service = Arc::clone(&fixture.service);
    let created = tokio::spawn(async move {
        service
            .create_session(meerkat_core::service::CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-haiku-4-5-20251001".to_string(),
                prompt: meerkat_core::types::ContentInput::Text(CHILD_TASK.to_string()),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(meerkat_core::service::SessionBuildOptions::default()),
                labels: None,
            })
            .await
    })
    .await
    .expect("the create task")
    .expect("create the session");
    assert!(
        fixture
            .service
            .read(&created.session_id)
            .await
            .is_ok_and(|view| view.state.session_id == created.session_id),
        "the created session reads back"
    );

    tokio::spawn({
        let fixture = Arc::clone(&fixture);
        async move { fixture.seed_source_mob(&["member"]).await }
    })
    .await
    .expect("the member build task");
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("the source mob");
    assert!(
        handle
            .get_member(&AgentIdentity::from("member"))
            .await
            .expect("read the member")
            .is_some(),
        "the member is seated"
    );
    // A handle kept across the destroy would still hold the mob's stores.
    drop(handle);

    // The host destroys the mob from a task on its workers.
    tokio::spawn({
        let fixture = Arc::clone(&fixture);
        async move { fixture.state.mob_destroy(&fixture.source_mob_id()).await }
    })
    .await
    .expect("the destroy task")
    .expect("destroy the mob");
    fixture.teardown().await;
}
