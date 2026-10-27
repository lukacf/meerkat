//! A Meta-hosted member's committed reads while its run holds a round
//! commit's provisional tail, on the persistent composition (HeadCanonical:
//! Core's session and runtime stores share `runtime.sqlite`).
//!
//! The member's turn answers one tool call, whose round commits and stands as
//! the session's provisional tail, then holds its second model call. While it
//! holds, the strict committed read refuses the boundary, and:
//! - the history reads that Meta's operator transcript facts and memory
//!   hygienist make (`read_history`, `list_transcript_revisions`) serve the
//!   committed snapshot from before the turn;
//! - the tolerant committed read through Meta's production session-service
//!   wrapper serves the same snapshot.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::service::{SessionHistoryQuery, SessionTranscriptRevisionListQuery};
use meerkat_core::types::{ContentInput, HandlingMode, SessionId, StopReason};
use meerkat_meta::{MemberTurnOptions, UnifiedRuntime};
use meerkat_mob::{MobDefinition, MobSessionService, SpawnMemberSpec};
use tokio::sync::Semaphore;

/// The one definition of the normalized-provider-accounting contract every
/// Meta LLM double must satisfy. See the module docs.
#[path = "support/llm_usage.rs"]
mod llm_usage;

const MODEL: &str = "gpt-5.5";
const ALIAS: &str = "tail-worker";
const PROMPT: &str = "HELD-TURN-QUEBEC-6";
const ROUND_CALL_ID: &str = "held-round-call";
const DEADLINE: Duration = Duration::from_secs(30);

/// The member's model: its first call that offers tools answers one call of
/// the first tool, and every later call holds until released.
struct HeldRoundClient {
    calls: AtomicUsize,
    entered: Semaphore,
    release: Semaphore,
}

impl HeldRoundClient {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }
}

impl meerkat_client::LlmClient for HeldRoundClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0
            && let Some(tool) = request.tools.first()
        {
            let [usage, done] =
                llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::ToolUse);
            let tool_call = LlmEvent::ToolCallComplete {
                id: ROUND_CALL_ID.to_string(),
                name: tool.name.to_string(),
                args: serde_json::json!({}),
                meta: None,
            };
            return Box::pin(futures::stream::iter([Ok(tool_call), Ok(usage), Ok(done)]));
        }
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            self.entered.add_permits(1);
            if let Ok(permit) = self.release.acquire().await {
                permit.forget();
            }
            yield Ok(LlmEvent::TextDelta { delta: "done".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// The fixture mob `mob_id`: each row runs its own mob, since the rows share
/// one process and a mob's supervisor name is process-wide.
fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "{MODEL}"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
    ))
    .expect("the fixture definition parses")
}

/// A member whose turn holds at its second model call, after its first
/// round committed.
struct HeldTurn {
    runtime: UnifiedRuntime,
    client: Arc<HeldRoundClient>,
    session_id: SessionId,
    turn: meerkat_meta::MemberTurnHandle,
    _state: tempfile::TempDir,
}

impl HeldTurn {
    async fn start(mob_id: &str) -> Self {
        let state = tempfile::tempdir().expect("state directory");
        let client = Arc::new(HeldRoundClient::new());
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .definition(definition(mob_id))
                .default_llm_client(client.clone())
                .persistent_state(state.path())
                .max_sessions(4)
                .build(),
        )
        .await
        .unwrap_or_else(|error| panic!("the runtime builds: {error}"));
        runtime
            .spawn(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                ALIAS.to_string(),
                None,
                None,
                None,
            ))
            .await
            .unwrap_or_else(|error| panic!("the member spawns: {error}"));
        let admission = runtime
            .start_member_turn(
                ALIAS,
                ContentInput::Text(PROMPT.to_string()),
                HandlingMode::Queue,
                MemberTurnOptions::new(),
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("the turn is admitted: {error}"));
        let session_id = SessionId::parse(&admission.session_id).expect("a session id");
        tokio::time::timeout(DEADLINE, client.entered.acquire())
            .await
            .expect("the turn reaches its second model call")
            .expect("the model gate")
            .forget();
        Self {
            runtime,
            client,
            session_id,
            turn: admission.turn,
            _state: state,
        }
    }

    /// The mob's session service: Meta's production wrapper.
    fn service(&self) -> Arc<dyn MobSessionService> {
        self.runtime
            .mob_runtime()
            .session_service()
            .cloned()
            .expect("the mob's session service")
    }

    /// Release the held model call, let the turn complete and shut down.
    async fn finish(self) {
        self.client.release.add_permits(1);
        let completed = tokio::time::timeout(DEADLINE, self.turn.wait()).await;
        self.runtime.shutdown().await;
        assert!(
            matches!(completed, Ok(Ok(_))),
            "the held turn completes once released: {completed:?}"
        );
    }
}

fn mentions_the_turn(message: &meerkat_core::Message) -> bool {
    serde_json::to_string(message)
        .map(|text| text.contains(PROMPT) || text.contains(ROUND_CALL_ID))
        .unwrap_or(false)
}

/// The history reads Meta's operator transcript facts
/// (`rpc/operator_methods.rs` `read_transcript_facts`,
/// `handle_bound_member_transcript`) and memory hygienist
/// (`SessionServiceRevisionSeam::read_messages`) make serve the committed
/// snapshot while the member's run holds a round commit's tail, instead of
/// refusing with the revision conflict.
#[tokio::test(flavor = "multi_thread")]
async fn history_reads_mid_run_serve_the_committed_snapshot() {
    let held = HeldTurn::start("committed-reads-history").await;
    let service = held.service();
    let strict = service.load_persisted_session(&held.session_id).await;
    // Bounded: a read that waits for the held turn would never answer.
    let history = tokio::time::timeout(
        DEADLINE,
        service.read_history(
            &held.session_id,
            SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        ),
    )
    .await;
    let revisions = tokio::time::timeout(
        DEADLINE,
        service.list_transcript_revisions(
            &held.session_id,
            SessionTranscriptRevisionListQuery {
                limit: Some(0),
                offset: None,
            },
        ),
    )
    .await;
    held.finish().await;

    assert!(
        strict.is_err(),
        "the strict read refuses the boundary while the round's tail stands: {strict:?}"
    );
    let history = history
        .expect("the history read answers while the turn holds")
        .unwrap_or_else(|error| panic!("the history is served mid-run: {error}"));
    assert!(
        !history.messages.iter().any(mentions_the_turn),
        "the history is the committed snapshot, with nothing of the running turn: {:?}",
        history.messages
    );
    revisions
        .expect("the revision read answers while the turn holds")
        .unwrap_or_else(|error| panic!("the head revision is served mid-run: {error}"));
}

/// The tolerant committed read through Meta's production session-service
/// wrapper serves the committed snapshot while the member's run holds a round
/// commit's tail; the strict read still refuses.
#[tokio::test(flavor = "multi_thread")]
async fn the_tolerant_read_mid_run_serves_the_committed_snapshot_through_the_wrapper() {
    let held = HeldTurn::start("committed-reads-tolerant").await;
    let service = held.service();
    let strict = service.load_persisted_session(&held.session_id).await;
    let tolerant = service
        .load_persisted_session_tolerating_tail(&held.session_id)
        .await;
    held.finish().await;

    assert!(
        strict.is_err(),
        "the strict read refuses the boundary while the round's tail stands: {strict:?}"
    );
    let snapshot = tolerant
        .unwrap_or_else(|error| panic!("the tolerant read serves mid-run: {error}"))
        .expect("the member's session is committed");
    assert!(
        !snapshot.messages().iter().any(mentions_the_turn),
        "the tolerant read serves the committed snapshot, with nothing of the running \
         turn: {:?}",
        snapshot.messages()
    );
}
