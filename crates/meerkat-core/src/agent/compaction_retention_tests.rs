//! Transcript-history retention inside one long run, through the real agent
//! loop.
//!
//! A session that a durable HeadCanonical head describes keeps every rewrite
//! occurrence that head does not cover yet: the next checkpoint proves the
//! pending rewrite suffix from that head, which needs their bodies. A run
//! that compacts many times before its run-end checkpoint must still keep its
//! in-memory history bounded by the retention, not by how often it compacted:
//! its round commits move the durable head over its rewrites.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use crate as meerkat_core;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_core::compact::{
    COMPACTION_SUMMARY_PREFIX, CompactionContext, CompactionDiscard, CompactionResult,
    CompactionRetained, CompactionSummary, Compactor, SummarizationSource,
    SummarizationSourceBudget,
};
use meerkat_core::types::UserMessage;
use meerkat_core::{
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AssistantBlock, LlmStreamResult, Message, RunCheckpointReceipt, RunId, Session,
    SessionCheckpointer, StopReason, ToolCallView, ToolDef, ToolResult, TranscriptHistoryRetention,
    TurnUsage, Usage,
};
use serde_json::value::RawValue;

const RETAINED_REWRITES: usize = 2;
const TOOL_TURNS: usize = 7;
const COMPACTION_PROMPT: &str = "COMPACT NOW";

fn mock_usage() -> Usage {
    TurnUsage::host_declared(
        meerkat_core::Provider::Other,
        "mock-model",
        Usage::default(),
    )
    .into_inner()
}

/// Calls one tool on each of its first `TOOL_TURNS` ordinary requests, then
/// answers. A compaction summary request gets a short summary.
struct ToolLoopClient {
    ordinary_calls: AtomicUsize,
}

#[async_trait]
impl AgentLlmClient for ToolLoopClient {
    async fn stream_response(
        &self,
        messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let summary_request = messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .is_some_and(|text| text == COMPACTION_PROMPT);
        if summary_request {
            return Ok(LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "summary".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                mock_usage(),
            ));
        }
        let call = self.ordinary_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call <= TOOL_TURNS {
            return Ok(LlmStreamResult::new(
                vec![AssistantBlock::ToolUse {
                    id: format!("call-{call}"),
                    name: "probe".into(),
                    args: RawValue::from_string("{}".to_string()).unwrap(),
                    meta: None,
                }],
                StopReason::ToolUse,
                mock_usage(),
            ));
        }
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: "done".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            mock_usage(),
        ))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    fn model(&self) -> &'static str {
        "mock-model"
    }
}

struct ProbeTool {
    tools: Arc<[Arc<ToolDef>]>,
}

#[async_trait]
impl AgentToolDispatcher for ProbeTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(ToolResult::new(call.id.to_string(), "probe ok".to_string(), false).into())
    }
}

/// Compacts at every model boundary after the first: the System rows stay,
/// everything else becomes one summary row.
struct EveryBoundaryCompactor;

impl Compactor for EveryBoundaryCompactor {
    fn should_compact(&self, ctx: &CompactionContext) -> bool {
        ctx.session_boundary_index >= 1
            && ctx.last_compaction_boundary_index != Some(ctx.session_boundary_index)
    }

    fn transcript_history_retention(&self) -> TranscriptHistoryRetention {
        TranscriptHistoryRetention::from_count(RETAINED_REWRITES).expect("non-zero retention")
    }

    fn compaction_prompt(&self) -> &str {
        COMPACTION_PROMPT
    }

    fn max_summary_tokens(&self) -> u32 {
        32
    }

    fn prepare_for_summarization(
        &self,
        messages: &[Message],
        _budget: Option<SummarizationSourceBudget>,
    ) -> SummarizationSource {
        SummarizationSource::whole(
            meerkat_core::types::materialize_latest_system_prompt_versions(messages),
        )
    }

    fn rebuild_history(&self, messages: &[Message], summary: &str) -> CompactionResult {
        let mut rebuilt = Vec::new();
        let mut retained = Vec::new();
        let mut discarded = Vec::new();
        for (offset, message) in messages.iter().enumerate() {
            let source_offset = u64::try_from(offset).unwrap_or(u64::MAX);
            if matches!(message, Message::System(_)) {
                retained.push(CompactionRetained::new(
                    source_offset,
                    u64::try_from(rebuilt.len()).unwrap_or(u64::MAX),
                    message.clone(),
                ));
                rebuilt.push(message.clone());
            } else {
                discarded.push(CompactionDiscard::new(source_offset, message.clone()));
            }
        }
        let summary_offset = u64::try_from(rebuilt.len()).unwrap_or(u64::MAX);
        let summary_message = Message::User(UserMessage::compaction_summary(format!(
            "{COMPACTION_SUMMARY_PREFIX}{summary}"
        )));
        rebuilt.push(summary_message.clone());
        CompactionResult {
            messages: rebuilt,
            summary: CompactionSummary::new(summary_offset, summary_message),
            retained,
            discarded,
        }
    }
}

/// What a HeadCanonical store checkpointer does with the live session, without
/// the store: it proves the pending rewrite suffix from the durable head (so
/// it refuses when the live graph retired an occurrence that head does not
/// cover), then acknowledges the successor, whose head covers every rewrite,
/// and issues the next provisional receipt of the run.
#[derive(Default)]
struct HeadCanonicalCheckpointer {
    /// `(commit_count, retired_count)` of the live graph at each checkpoint
    /// whose session carries a graph.
    checkpoints: Mutex<Vec<(usize, usize)>>,
    /// The checkpointed session's tail at each checkpoint.
    tails: Mutex<Vec<CheckpointTail>>,
    /// The run cursor each checkpoint carried, by round and resume point.
    cursors: Mutex<Vec<Option<(u64, CursorPoint)>>>,
    /// Stand in for a WholeBlob store: no durable head is acknowledged.
    whole_blob: bool,
    /// Re-base a run after a rewrite (only with `whole_blob`).
    rebases: bool,
    /// Which checkpoints were re-bases, in order.
    rebased: Mutex<Vec<bool>>,
    /// The re-based flag of the run cursor each checkpoint carried.
    cursor_rebased: Mutex<Vec<Option<bool>>>,
}

impl HeadCanonicalCheckpointer {
    /// A WholeBlob stand-in that re-bases runs, or does not.
    fn whole_blob(rebases: bool) -> Self {
        Self {
            whole_blob: true,
            rebases,
            ..Self::default()
        }
    }
}

/// Where a checkpointed run cursor resumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorPoint {
    ToolsPending,
    AwaitingModel,
}

fn cursor_rebased(session: &Session) -> Option<bool> {
    session
        .try_run_cursor()
        .expect("the cursor decodes")
        .map(|cursor| cursor.rebased)
}

fn cursor_point(session: &Session) -> Option<(u64, CursorPoint)> {
    let cursor = session.try_run_cursor().expect("the cursor decodes")?;
    let point = match cursor.resume_point {
        meerkat_core::RunResumePoint::ToolsPending { .. } => CursorPoint::ToolsPending,
        meerkat_core::RunResumePoint::AwaitingModel => CursorPoint::AwaitingModel,
    };
    Some((cursor.round, point))
}

/// The next provisional receipt of the run: contiguous after `previous`, as
/// the agent requires of every checkpointer.
fn next_receipt(
    session: &Session,
    run_id: &RunId,
    previous: Option<&RunCheckpointReceipt>,
) -> RunCheckpointReceipt {
    let sequence = previous.map_or(1, |receipt| receipt.candidate_sequence() + 1);
    let authority = meerkat_core::HeadCanonicalProvisionalTailAuthority::issued(
        session.id().clone(),
        1,
        "committed-head".to_string(),
        1 + sequence,
        format!("provisional-head-{sequence}"),
        run_id.clone(),
        sequence,
    )
    .expect("a contiguous provisional authority");
    RunCheckpointReceipt::issued(
        meerkat_core::RunCheckpointAuthority::HeadCanonical(authority),
        format!("digest-{sequence}"),
        session.messages().len() as u64,
    )
    .expect("a valid receipt")
}

/// What a checkpoint saw at the end of the session it persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckpointTail {
    /// The run's own tool call, then its results: a classifiable live tail.
    RunToolUseThenResults,
    /// The run's own final answer.
    RunAnswer,
    /// A compaction summary: nothing the run wrote after its last rewrite.
    CompactionSummary,
    /// Any other shape.
    Other,
}

fn checkpoint_tail(session: &Session, run_id: &RunId) -> CheckpointTail {
    let stamped = |message: &Message| match message {
        Message::BlockAssistant(assistant) => assistant.identity.run_id.as_ref() == Some(run_id),
        _ => false,
    };
    match session.messages() {
        [.., Message::User(user)] if user.transcript_role.is_compaction_summary() => {
            CheckpointTail::CompactionSummary
        }
        [.., call, Message::ToolResults { .. }] if stamped(call) => {
            CheckpointTail::RunToolUseThenResults
        }
        [.., answer] if stamped(answer) => CheckpointTail::RunAnswer,
        _ => CheckpointTail::Other,
    }
}

#[async_trait]
impl SessionCheckpointer for HeadCanonicalCheckpointer {
    async fn checkpoint_run(
        &self,
        session: &mut Session,
        run_id: &RunId,
        previous: Option<&RunCheckpointReceipt>,
    ) -> Result<Option<RunCheckpointReceipt>, AgentError> {
        self.tails
            .lock()
            .unwrap()
            .push(checkpoint_tail(session, run_id));
        self.cursors.lock().unwrap().push(cursor_point(session));
        self.cursor_rebased
            .lock()
            .unwrap()
            .push(cursor_rebased(session));
        self.rebased.lock().unwrap().push(false);
        if self.whole_blob {
            return Ok(Some(next_receipt(session, run_id, previous)));
        }
        let Some(graph) = session
            .transcript_history_state()
            .map_err(|error| AgentError::InternalError(error.to_string()))?
        else {
            return Ok(Some(next_receipt(session, run_id, previous)));
        };
        if let Some(durable) = session.durable_transcript_rewrite_count_for_test() {
            graph.refuse_if_retired_count(durable).map_err(|error| {
                AgentError::InternalError(format!(
                    "failed to prove pending rewrite suffix: {error}"
                ))
            })?;
        }
        self.checkpoints
            .lock()
            .unwrap()
            .push((graph.commit_count(), graph.retired_count()));
        session.note_durable_transcript_rewrites(graph.commit_count() as u64);
        Ok(Some(next_receipt(session, run_id, previous)))
    }

    fn rebases_runs(&self) -> bool {
        self.rebases
    }

    /// The re-based document becomes the committed base; the tail restarts
    /// on it at candidate sequence 1.
    async fn rebase_run(
        &self,
        session: &mut Session,
        run_id: &RunId,
        _previous: Option<&RunCheckpointReceipt>,
    ) -> Result<RunCheckpointReceipt, AgentError> {
        self.tails
            .lock()
            .unwrap()
            .push(checkpoint_tail(session, run_id));
        self.cursors.lock().unwrap().push(cursor_point(session));
        self.cursor_rebased
            .lock()
            .unwrap()
            .push(cursor_rebased(session));
        self.rebased.lock().unwrap().push(true);
        Ok(next_receipt(session, run_id, None))
    }
}

struct NoopStore;

#[async_trait]
impl AgentSessionStore for NoopStore {
    async fn save(&self, _session: &Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
        Ok(None)
    }
}

/// One long run of `TOOL_TURNS` tool turns that compacts at every model
/// boundary after the first. `durable_floor` is the rewrite count of the
/// durable HeadCanonical head the session was materialized from (`None` for a
/// session no such head describes, as with WholeBlob); `checkpointer` is the
/// run's checkpointer, if any.
async fn run_long_compacting_run(
    durable_floor: Option<u64>,
    checkpointer: Option<Arc<HeadCanonicalCheckpointer>>,
) -> Session {
    let mut session = Session::new();
    session
        .set_build_state(meerkat_core::SessionBuildState::default())
        .expect("test session build state should serialize");
    if let Some(floor) = durable_floor {
        session.note_durable_transcript_rewrites(floor);
    }
    let mut builder = AgentBuilder::new()
        .resume_session(session)
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_runtime_execution_kind_for_test(
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
        )
        .compactor(Arc::new(EveryBoundaryCompactor));
    if let Some(checkpointer) = checkpointer {
        builder = builder.with_checkpointer(checkpointer);
    }
    let mut agent = builder
        .build_standalone(
            Arc::new(ToolLoopClient {
                ordinary_calls: AtomicUsize::new(0),
            }),
            Arc::new(ProbeTool {
                tools: Arc::from([Arc::new(ToolDef::new(
                    "probe",
                    "returns",
                    serde_json::json!({ "type": "object" }),
                ))]),
            }),
            Arc::new(NoopStore),
        )
        .await;

    agent.run("start".into()).await.expect("the run completes");
    agent.session().clone()
}

/// One run compacts at each of its `TOOL_TURNS` tool turns. Its session was
/// materialized from a durable HeadCanonical head that covers no rewrite, and
/// the run checkpoints only at its end: before the fix, retirement waited for
/// that checkpoint, so the run held every compaction's body until it ended.
#[tokio::test]
async fn a_long_run_keeps_its_history_within_the_retention_while_it_compacts() {
    let checkpointer = Arc::new(HeadCanonicalCheckpointer::default());
    let session = run_long_compacting_run(Some(0), Some(checkpointer.clone())).await;

    let graph = session
        .transcript_history_state()
        .expect("graph decodes")
        .expect("graph present");
    assert_eq!(
        graph.commit_count(),
        TOOL_TURNS,
        "one compaction rewrite per tool turn"
    );
    let retained = graph.commit_count() - graph.retired_count();
    assert!(
        retained <= RETAINED_REWRITES,
        "after {} compactions in one run the live graph keeps {retained} rewrite bodies; \
         the retention is {RETAINED_REWRITES} (checkpoints: {:?})",
        graph.commit_count(),
        checkpointer.checkpoints.lock().unwrap()
    );
    let checkpoints = checkpointer.checkpoints.lock().unwrap().clone();
    assert!(
        checkpoints
            .iter()
            .all(|(commits, retired)| commits - retired <= RETAINED_REWRITES + 1),
        "no checkpoint saw more than one rewrite body beyond the retention: {checkpoints:?}"
    );
}

/// A long compacting run commits each of its tool rounds twice. The first
/// commit, after the model call, stages the round's assistant message in the
/// cursor and writes no row, so its tail is the run's input in the first
/// round and the compaction summary after that: the cursor names the run.
/// The second commit follows the run's own tool call and results. The run's
/// end saves its answer without a cursor.
#[tokio::test]
async fn a_long_run_commits_each_tool_round_twice() {
    let checkpointer = Arc::new(HeadCanonicalCheckpointer::default());
    run_long_compacting_run(Some(0), Some(checkpointer.clone())).await;

    let tails = checkpointer.tails.lock().unwrap().clone();
    let cursors = checkpointer.cursors.lock().unwrap().clone();
    let mut expected_tails = Vec::new();
    let mut expected_cursors = Vec::new();
    for round in 0..TOOL_TURNS as u64 {
        expected_tails.push(if round == 0 {
            CheckpointTail::Other
        } else {
            CheckpointTail::CompactionSummary
        });
        expected_cursors.push(Some((round, CursorPoint::ToolsPending)));
        expected_tails.push(CheckpointTail::RunToolUseThenResults);
        expected_cursors.push(Some((round, CursorPoint::AwaitingModel)));
    }
    expected_tails.push(CheckpointTail::RunAnswer);
    expected_cursors.push(None);
    assert_eq!(tails, expected_tails);
    assert_eq!(cursors, expected_cursors);
}

/// A session no durable HeadCanonical head describes (WholeBlob) is
/// recovered only from a strict continuation of its committed transcript, so
/// with a checkpointer that does not re-base, its run takes no round commit
/// after its first rewrite: the first round's two commits, then only the
/// end-of-run save.
#[tokio::test]
async fn a_long_run_without_a_durable_head_stops_round_commits_at_its_first_rewrite() {
    let checkpointer = Arc::new(HeadCanonicalCheckpointer::whole_blob(false));
    let session = run_long_compacting_run(None, Some(checkpointer.clone())).await;

    let tails = checkpointer.tails.lock().unwrap().clone();
    assert_eq!(
        tails,
        vec![
            CheckpointTail::Other,
            CheckpointTail::RunToolUseThenResults,
            CheckpointTail::RunAnswer,
        ]
    );
    let cursors = checkpointer.cursors.lock().unwrap().clone();
    assert_eq!(
        cursors,
        vec![
            Some((0, CursorPoint::ToolsPending)),
            Some((0, CursorPoint::AwaitingModel)),
            None,
        ]
    );
    let graph = session
        .transcript_history_state()
        .expect("graph decodes")
        .expect("graph present");
    assert_eq!(graph.commit_count(), TOOL_TURNS);
    assert!(graph.commit_count() - graph.retired_count() <= RETAINED_REWRITES);
    assert!(
        session
            .try_run_cursor()
            .expect("the cursor decodes")
            .is_none(),
        "the end-of-run save removed the cursor"
    );
}

/// With a checkpointer that re-bases runs, a WholeBlob run keeps committing
/// its rounds after a rewrite: the first round commit after each rewrite is
/// a re-base (here every round's first commit, since the run compacts at
/// every boundary after the first), and the second commit chains on it. The
/// end-of-run save after the final boundary's compaction is a re-base too,
/// without a cursor, so no tail recovery would refuse is ever written. From
/// the run's first re-base on, every cursor says the run re-based, so a
/// restart does not forget that its inputs are in the committed history.
#[tokio::test]
async fn a_long_run_without_a_durable_head_rebases_after_each_rewrite() {
    let checkpointer = Arc::new(HeadCanonicalCheckpointer::whole_blob(true));
    run_long_compacting_run(None, Some(checkpointer.clone())).await;

    let cursors = checkpointer.cursors.lock().unwrap().clone();
    let rebased = checkpointer.rebased.lock().unwrap().clone();
    let cursor_rebased = checkpointer.cursor_rebased.lock().unwrap().clone();
    let mut expected_cursors = Vec::new();
    let mut expected_rebased = Vec::new();
    let mut expected_cursor_rebased = Vec::new();
    for round in 0..TOOL_TURNS as u64 {
        expected_cursors.push(Some((round, CursorPoint::ToolsPending)));
        expected_rebased.push(round > 0);
        expected_cursor_rebased.push(Some(round > 0));
        expected_cursors.push(Some((round, CursorPoint::AwaitingModel)));
        expected_rebased.push(false);
        expected_cursor_rebased.push(Some(round > 0));
    }
    expected_cursors.push(None);
    expected_rebased.push(true);
    expected_cursor_rebased.push(None);
    assert_eq!(cursors, expected_cursors);
    assert_eq!(rebased, expected_rebased);
    assert_eq!(cursor_rebased, expected_cursor_rebased);
}

/// Without a checkpointer a run cannot move its durable head, so it takes no
/// checkpoint and keeps what that head does not cover, as before.
#[tokio::test]
async fn a_long_run_without_a_checkpointer_keeps_what_its_durable_head_lacks() {
    let session = run_long_compacting_run(Some(0), None).await;

    let graph = session
        .transcript_history_state()
        .expect("graph decodes")
        .expect("graph present");
    assert_eq!(graph.commit_count(), TOOL_TURNS);
    assert_eq!(
        graph.retired_count(),
        0,
        "nothing the durable head lacks is retired without a checkpoint"
    );
}

/// Retirement after a checkpoint works only on the graph this process holds
/// validated, for a session with a durable HeadCanonical head. A session
/// without one (WholeBlob) keeps its graph as it is, so the first turn after a
/// resume neither retires nor validates a graph its run did not change. A
/// graph without the session's validation marker, held or not, is not read
/// there, and the checkpoint still refuses it. A held graph the durable head
/// covers retires to the retention.
#[tokio::test]
async fn retirement_after_a_checkpoint_needs_a_durable_head_and_a_held_graph() {
    let retention =
        TranscriptHistoryRetention::from_count(RETAINED_REWRITES).expect("non-zero retention");

    let mut whole_blob = run_long_compacting_run(None, None).await;
    let before = whole_blob
        .transcript_history_state()
        .expect("graph decodes")
        .expect("graph present");
    assert_eq!(
        whole_blob
            .retire_transcript_history_after_checkpoint(retention)
            .expect("no error"),
        None,
        "a session without a durable head retires after each compaction instead"
    );
    let after = whole_blob
        .transcript_history_state()
        .expect("graph decodes")
        .expect("graph present");
    assert_eq!(
        (after.commit_count(), after.retired_count()),
        (before.commit_count(), before.retired_count())
    );

    let held = run_long_compacting_run(Some(0), None).await;
    let graph = held
        .transcript_history_state()
        .expect("graph decodes")
        .expect("graph present");
    assert_eq!(
        (graph.commit_count(), graph.retired_count()),
        (TOOL_TURNS, 0),
        "the durable head held every body back"
    );

    // The same graph as bytes without verified authority: retirement does not
    // parse it, and every checkpoint still refuses it.
    let mut unverified = held.clone();
    unverified.note_durable_transcript_rewrites(TOOL_TURNS as u64);
    unverified.set_metadata_unchecked_for_test(
        crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
        serde_json::to_value(&graph).expect("graph value"),
    );
    assert_eq!(
        unverified
            .retire_transcript_history_after_checkpoint(retention)
            .expect("no error"),
        None,
        "a graph this process does not hold validated is left alone"
    );
    assert!(
        unverified.validated_transcript_history_state().is_err(),
        "a HeadCanonical checkpoint's graph read still refuses an unverified graph"
    );
    // That read cached the parsed graph without verifying it. Holding a graph
    // is not enough: without the session's validation marker it is still left
    // alone, and it is unchanged.
    assert_eq!(
        unverified
            .retire_transcript_history_after_checkpoint(retention)
            .expect("a cached but unverified graph is not read here"),
        None,
        "a held graph without the session's validation marker is left alone"
    );
    let cached = unverified
        .transcript_history_state()
        .expect("graph decodes")
        .expect("graph present");
    assert_eq!(
        (cached.commit_count(), cached.retired_count()),
        (TOOL_TURNS, 0),
        "the unverified graph is unchanged"
    );
    assert!(
        unverified.to_persisted_bytes().is_err(),
        "a WholeBlob checkpoint still refuses an unverified graph"
    );

    let mut covered = held;
    covered.note_durable_transcript_rewrites(TOOL_TURNS as u64);
    assert_eq!(
        covered
            .retire_transcript_history_after_checkpoint(retention)
            .expect("no error"),
        Some(TOOL_TURNS - RETAINED_REWRITES),
        "a held graph the durable head covers retires to the retention"
    );
    let retired = covered
        .transcript_history_state()
        .expect("graph decodes")
        .expect("graph present");
    assert_eq!(
        (retired.commit_count(), retired.retired_count()),
        (TOOL_TURNS, TOOL_TURNS - RETAINED_REWRITES),
        "retirement keeps every commit and drops only the bodies past the retention"
    );
}
