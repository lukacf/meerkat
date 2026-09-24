//! Outcome contract for `session/export_atif`.
//!
//! The export distinguishes three states that are easy to collapse into one
//! another: a host with no durable replay projection, an existing session whose
//! durable log is empty, and a session that does not exist. Each has its own
//! honest answer and this battery pins all three.
//!
//! Deterministic (mock LLM, no API keys, no `#[ignore]`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat::{AgentBuildConfig, AgentFactory};
use meerkat_client::{LlmClient, LlmError};
use meerkat_core::{AgentEvent, BlobStore, Config, SessionId, StopReason};
use meerkat_rpc::handlers::session::{
    AtifExportBounds, handle_export_atif, handle_export_atif_bounded,
};
use meerkat_rpc::protocol::RpcId;
use meerkat_rpc::session_runtime::SessionRuntime;
use meerkat_session::event_store::EventStore;
use meerkat_store::MemoryBlobStore;
use serde_json::value::RawValue;

struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>>
    {
        Box::pin(futures::stream::iter(vec![
            Ok(meerkat_client::LlmEvent::TextDelta {
                delta: "Hello from mock".to_string(),
                meta: None,
            }),
            Ok(meerkat_client::LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::Anthropic,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            }),
            Ok(meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                },
            }),
        ]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn mock_build_config() -> AgentBuildConfig {
    AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    }
}

/// A runtime whose realm bundle installs the durable event projection, plus a
/// handle on the same durable event store so a committed log can be seeded.
async fn projection_backed_runtime_with_event_store(
    temp: &tempfile::TempDir,
) -> (Arc<SessionRuntime>, Arc<dyn EventStore>) {
    let (_manifest, persistence) =
        meerkat::open_realm_persistence_in(&temp.path().join("realms"), "atif-export", None, None)
            .await
            .expect("open realm persistence");
    let (event_store, _projector) = persistence
        .event_projection()
        .expect("realm persistence wires the durable event projection");
    let runtime = SessionRuntime::new(
        AgentFactory::new(temp.path().join("sessions")),
        Config::default(),
        4,
        persistence,
        meerkat_rpc::router::NotificationSink::noop(),
    );
    runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
    (Arc::new(runtime), event_store)
}

async fn projection_backed_runtime(temp: &tempfile::TempDir) -> Arc<SessionRuntime> {
    projection_backed_runtime_with_event_store(temp).await.0
}

/// One committed turn: a user step and an agent step, four events.
async fn seed_committed_turn(event_store: &Arc<dyn EventStore>, session_id: &SessionId) {
    event_store
        .append(
            session_id,
            &[
                AgentEvent::RunStarted {
                    session_id: session_id.clone(),
                    input: meerkat_core::RunInput::Content {
                        content: "hello".into(),
                    },
                },
                AgentEvent::TurnStarted { turn_number: 0 },
                AgentEvent::TextComplete {
                    content: "answered".to_string(),
                },
                AgentEvent::TurnCompleted {
                    stop_reason: StopReason::EndTurn,
                    usage: Some(meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::Anthropic,
                        "claude-sonnet-4-5",
                        meerkat_core::Usage::default(),
                    )),
                },
            ],
        )
        .await
        .expect("seed a committed event log");
}

/// A runtime composed without an event projection, as in-memory hosts are.
fn projectionless_runtime(temp: &tempfile::TempDir) -> Arc<SessionRuntime> {
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let blob_store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let runtime = SessionRuntime::new(
        AgentFactory::new(temp.path().join("sessions")),
        Config::default(),
        4,
        meerkat::PersistenceBundle::new(
            session_store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            blob_store,
        ),
        meerkat_rpc::router::NotificationSink::noop(),
    );
    runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
    Arc::new(runtime)
}

fn export_params(session_id: &str) -> Box<RawValue> {
    serde_json::value::to_raw_value(&serde_json::json!({ "session_id": session_id }))
        .expect("serialize export params")
}

#[tokio::test]
async fn eventless_session_exports_an_empty_trajectory_naming_the_session() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = projection_backed_runtime(&temp).await;
    let session_id = runtime
        .create_session(
            mock_build_config(),
            None,
            Some("deferred prompt".into()),
            Vec::new(),
        )
        .await
        .expect("stage a deferred session");

    let params = export_params(&session_id.to_string());
    let response = handle_export_atif(Some(RpcId::Num(1)), Some(&params), &runtime).await;

    assert!(
        response.error.is_none(),
        "an existing session with no durable events must not be an error: {:?}",
        response.error
    );
    let result: serde_json::Value =
        serde_json::from_str(response.result.expect("export result").get()).unwrap();
    assert_eq!(
        result["session_id"].as_str(),
        Some(session_id.to_string().as_str()),
        "the empty trajectory must name the session it came from: {result}"
    );
    assert_eq!(
        result["steps"].as_array().map(Vec::len),
        Some(0),
        "an eventless log has no steps: {result}"
    );
    assert_eq!(
        result["final_metrics"]["total_steps"].as_u64(),
        Some(0),
        "{result}"
    );
}

/// The over-ceiling refusal goes through the handler that serves the wire, is
/// typed as a param rejection, and names the bound it enforced.
#[tokio::test]
async fn over_ceiling_export_is_a_typed_param_refusal_naming_the_bound() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, event_store) = projection_backed_runtime_with_event_store(&temp).await;
    let session_id = SessionId::new();
    seed_committed_turn(&event_store, &session_id).await;

    let params = export_params(&session_id.to_string());
    let response = handle_export_atif_bounded(
        Some(RpcId::Num(4)),
        Some(&params),
        &runtime,
        AtifExportBounds {
            max_events: 1,
            ..AtifExportBounds::host_default()
        },
    )
    .await;

    let error = response
        .error
        .expect("a log past the replay bound must be refused");
    assert_eq!(
        error.code,
        meerkat_rpc::error::INVALID_PARAMS,
        "the refusal is a param-level rejection, not an internal error: {error:?}"
    );
    assert!(
        error.message.contains('1') && error.message.contains("-event bound"),
        "the refusal must name the bound it enforced: {error:?}"
    );
    assert!(
        error.message.contains("events/list_since"),
        "the refusal must point at pagination: {error:?}"
    );

    // The same log under the host bound exports in full, so the refusal is the
    // bound speaking and not a broken replay.
    let allowed = handle_export_atif(Some(RpcId::Num(5)), Some(&params), &runtime).await;
    assert!(allowed.error.is_none(), "{:?}", allowed.error);
    let result: serde_json::Value =
        serde_json::from_str(allowed.result.expect("export result").get()).unwrap();
    assert_eq!(
        result["steps"].as_array().map(Vec::len),
        Some(2),
        "one seeded turn is a user step and an agent step: {result}"
    );
}

/// A log whose event count is nowhere near the replay bound can still fold into
/// a document the response cannot carry. That refusal arrives mid-fold, and it
/// is the answer outbound admission would have given for the finished document.
#[tokio::test]
async fn undeliverable_document_is_refused_while_folding() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, event_store) = projection_backed_runtime_with_event_store(&temp).await;
    let session_id = SessionId::new();
    seed_committed_turn(&event_store, &session_id).await;

    let params = export_params(&session_id.to_string());
    let response = handle_export_atif_bounded(
        Some(RpcId::Num(6)),
        Some(&params),
        &runtime,
        AtifExportBounds {
            max_retained_bytes: 4,
            ..AtifExportBounds::host_default()
        },
    )
    .await;

    let error = response
        .error
        .expect("a document past the response limit must be refused");
    assert_eq!(
        error.code,
        meerkat_rpc::error::BUDGET_EXHAUSTED,
        "an undeliverable result answers the way outbound admission answers: {error:?}"
    );
    assert!(
        error.message.contains("4-byte outbound response limit"),
        "the refusal must name the limit it enforced: {error:?}"
    );
    assert!(
        error.message.contains("rkat session export-atif"),
        "the refusal must point at the unbounded file export: {error:?}"
    );

    // The same log under the host bounds exports in full, so the refusal is the
    // byte bound speaking and not a broken replay.
    let allowed = handle_export_atif(Some(RpcId::Num(7)), Some(&params), &runtime).await;
    assert!(allowed.error.is_none(), "{:?}", allowed.error);
}

#[tokio::test]
async fn missing_session_is_still_reported_as_not_found() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = projection_backed_runtime(&temp).await;

    let params = export_params(&SessionId::new().to_string());
    let response = handle_export_atif(Some(RpcId::Num(2)), Some(&params), &runtime).await;

    let error = response
        .error
        .expect("a session that does not exist errors");
    assert_eq!(
        error.code,
        meerkat_rpc::error::SESSION_NOT_FOUND,
        "unexpected error for a missing session: {error:?}"
    );
}

#[tokio::test]
async fn host_without_event_replay_says_so_instead_of_not_found() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = projectionless_runtime(&temp);
    let session_id = runtime
        .create_session(
            mock_build_config(),
            None,
            Some("deferred prompt".into()),
            Vec::new(),
        )
        .await
        .expect("stage a deferred session");

    let params = export_params(&session_id.to_string());
    let response = handle_export_atif(Some(RpcId::Num(3)), Some(&params), &runtime).await;

    let error = response
        .error
        .expect("a host without durable replay cannot export");
    assert_ne!(
        error.code,
        meerkat_rpc::error::SESSION_NOT_FOUND,
        "the session exists; refusing it as missing is a lie: {error:?}"
    );
    assert_eq!(error.code, meerkat_rpc::error::INVALID_REQUEST);
    assert!(
        error.message.contains("event replay is not enabled"),
        "unexpected message: {error:?}"
    );
}
