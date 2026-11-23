//! A peer input whose payload is JSON null (a peer response sent without a
//! result) is consumed by a run, its completion is finalized, and the input
//! queued behind it runs.
//!
//! On the SQLite store such a row is written with `"payload":null`, which the
//! typed decoder reads back as no payload. The store's exact compare used to
//! judge the row by that reading, so the finalizing write was refused for
//! good: the run had committed, the runtime loop left, and everything queued
//! behind the input was accepted and never ran.
#![cfg(feature = "sqlite-store")]
#![allow(
    clippy::expect_used,
    clippy::large_futures,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use meerkat_core::BlobStore;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceiptDraft, RunId};
use meerkat_core::types::SessionId;
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, LogicalRuntimeId,
    MeerkatMachine, PeerConvention, PeerInput, PromptInput, ResponseProgressPhase, RuntimeStore,
};
use meerkat_store::MemoryBlobStore;

const ANSWER_BOUND: Duration = Duration::from_secs(20);

fn header(source: InputOrigin, durability: InputDurability) -> InputHeader {
    InputHeader {
        ingress_context: None,
        retained_resume: None,
        authority_association: None,
        id: InputId::new(),
        timestamp: Utc::now(),
        source,
        durability,
        visibility: InputVisibility::default(),
        idempotency_key: None,
        supersession_key: None,
        correlation_id: None,
        requester: None,
    }
}

fn prompt(text: &str) -> Input {
    Input::Prompt(PromptInput {
        injected_context: Vec::new(),
        header: header(InputOrigin::Operator, InputDurability::Durable),
        content: text.into(),
        typed_turn_appends: Vec::new(),
        turn_metadata: None,
    })
}

fn peer(
    convention: PeerConvention,
    durability: InputDurability,
    payload: Option<serde_json::Value>,
) -> Input {
    Input::Peer(PeerInput {
        directed_interaction_id: None,
        objective_id: None,
        system_prompts: Vec::new(),
        injected_context: Vec::new(),
        sender_taint: None,
        header: header(
            InputOrigin::Peer {
                peer_id: "responder".into(),
                display_identity: None,
                runtime_id: None,
            },
            durability,
        ),
        convention: Some(convention),
        content: "from a peer".into(),
        payload,
        handling_mode: None,
        in_reply_to: None,
    })
}

/// A peer's progress notice ("accepted") to a request this session sent.
fn progress_notice(payload: Option<serde_json::Value>) -> Input {
    peer(
        PeerConvention::ResponseProgress {
            request_id: "request-1".to_string(),
            phase: ResponseProgressPhase::Accepted,
        },
        InputDurability::Ephemeral,
        payload,
    )
}

/// Commits its session at every boundary and returns a plain result.
struct CommittingExecutor {
    session: meerkat_core::Session,
    calls: usize,
}

#[async_trait::async_trait]
impl CoreExecutor for CommittingExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        self.calls += 1;
        let call = self.calls;
        self.session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text(format!("run {call}")),
        ));
        let digest = self
            .session
            .transcript_content_digest()
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))?;
        CoreApplyOutput::with_run_result(
            RunBoundaryReceiptDraft {
                run_id,
                boundary: primitive.apply_boundary(),
                contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                conversation_digest: Some(digest),
                message_count: self.session.messages().len(),
            },
            None,
            meerkat_core::RunResult {
                reasoning_effort: None,
                text: format!("result of run {call}"),
                session_id: self.session.id().clone(),
                usage: Default::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
                run_usage: None,
                request_usage: Vec::new(),
            },
        )
        .with_session(Arc::new(self.session.clone()))
        .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn checkpoint_committed_session_snapshot(
        &mut self,
        _session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn acknowledge_committed_session_boundary(
        &mut self,
        _authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

enum Store {
    Sqlite,
    Memory,
}

/// Put `first` on an idle session, then a prompt, and require that both
/// completions answer with a result and that the store names no pending
/// terminal owner afterwards.
async fn the_input_is_finalized_and_the_prompt_behind_it_runs(store: Store, first: Input) {
    let dir = tempfile::tempdir().expect("temp dir");
    let store: Arc<dyn RuntimeStore> = match store {
        Store::Sqlite => Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(dir.path().join("runtime.sqlite"))
                .expect("sqlite runtime store"),
        ),
        Store::Memory => Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new()),
    };
    let blobs: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let machine = Arc::new(
        MeerkatMachine::persistent(Arc::clone(&store), blobs).expect("persistent machine"),
    );
    let session_id = SessionId::new();
    machine
        .register_session_with_executor(
            session_id.clone(),
            Box::new(CommittingExecutor {
                session: meerkat_core::Session::with_id(session_id.clone()),
                calls: 0,
            }),
        )
        .await
        .expect("register the session");

    let mut waiters = Vec::new();
    for (name, input) in [
        ("the peer input", first),
        ("the prompt behind it", prompt("next")),
    ] {
        let (_outcome, handle) = machine
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("admission answers");
        let handle = handle.expect("the input has a completion");
        waiters.push((name, tokio::spawn(async move { handle.wait().await })));
        // The peer input is taken by a run of its own before the prompt
        // arrives.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let deadline = Instant::now() + ANSWER_BOUND;
    for (name, waiter) in waiters {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let outcome = tokio::time::timeout(remaining, waiter)
            .await
            .unwrap_or_else(|_| panic!("{name}: its completion never answered"))
            .expect("the waiter task ends");
        assert!(
            matches!(outcome, Ok(CompletionOutcome::Completed(_))),
            "{name}: unexpected completion {outcome:?}"
        );
    }
    let owners = store
        .load_pending_terminal_owner_ids_page(&LogicalRuntimeId::for_session(&session_id), None, 16)
        .await
        .expect("load pending terminal owners");
    assert!(
        owners.is_empty(),
        "a completion is still pending: {owners:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_progress_notice_without_a_result_is_finalized_on_the_sqlite_store() {
    the_input_is_finalized_and_the_prompt_behind_it_runs(
        Store::Sqlite,
        progress_notice(Some(serde_json::Value::Null)),
    )
    .await;
}

/// Pins: the same input with a payload that is not null, and the null
/// payload on the in-memory store, were always finalized.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_progress_notice_with_a_result_is_finalized_on_the_sqlite_store() {
    the_input_is_finalized_and_the_prompt_behind_it_runs(
        Store::Sqlite,
        progress_notice(Some(serde_json::json!({ "token": "ok" }))),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_progress_notice_without_a_result_is_finalized_on_the_memory_store() {
    the_input_is_finalized_and_the_prompt_behind_it_runs(
        Store::Memory,
        progress_notice(Some(serde_json::Value::Null)),
    )
    .await;
}
