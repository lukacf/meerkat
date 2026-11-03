//! Exact integration evidence for the real Meta durable member-fork seam.
//!
//! Completion authority in this suite comes only from the admitted turn's
//! `MemberTurnHandle::wait`. The running-source case uses an explicit `Notify`
//! handshake to establish that the source executor is inside its stream. No
//! sleep, poll loop, event count, output preview, or session-wide event is used
//! to infer either terminality or liveness.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::{Stream, StreamExt as _, stream};
use meerkat::{
    AgentFactory, AgentToolDispatcher, Config, FactoryAgentBuilder, PersistentSessionService,
    Provider, StopReason, ToolDef, ToolResult, Usage,
};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::error::ToolError;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::service::{SessionHistoryQuery, SessionServiceHistoryExt};
use meerkat_core::types::{
    ContentBlock, ContentInput, HandlingMode, ImageData, Message, ToolCallView,
};
use meerkat_core::{BlobStore, ForkCacheInheritance, ForkCacheInheritanceUnavailableReason};
use meerkat_meta::fork::ForkMemberError;
use meerkat_meta::{
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, UnifiedRuntime,
};
use meerkat_mob::error::ForkSourceUnavailableCause;
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::{MemberLaunchMode, MobDefinition, MobError, MobStorage, SpawnMemberSpec};
use serde_json::json;
use tokio::sync::Notify;

const IMAGE_BASE64: &str = "iVBORw0KGgo=";
const TOOL_CALL_ID: &str = "fork-proof-call";
const TOOL_RESULT_TEXT: &str = "durable tool result";

struct ForkProbeTool;

#[async_trait::async_trait]
impl AgentToolDispatcher for ForkProbeTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            "fork_probe",
            "Return deterministic durable fork evidence",
            json!({"type": "object", "properties": {}}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        assert_eq!(call.id, TOOL_CALL_ID);
        assert_eq!(call.name, "fork_probe");
        Ok(ToolResult::new(call.id.to_string(), TOOL_RESULT_TEXT.to_string(), false).into())
    }
}

struct ForkScriptClient {
    calls: AtomicUsize,
    running_entered: Arc<Notify>,
    release_running: Arc<Notify>,
}

impl ForkScriptClient {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            running_entered: Arc::new(Notify::new()),
            release_running: Arc::new(Notify::new()),
        }
    }

    fn usage_then_done(
        request: &LlmRequest,
        stop_reason: StopReason,
    ) -> [Result<LlmEvent, LlmError>; 2] {
        [
            Ok(LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::OpenAI,
                    &request.model,
                    Usage::default(),
                ),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(stop_reason),
            }),
        ]
    }
}

impl LlmClient for ForkScriptClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                let [usage, done] = Self::usage_then_done(request, StopReason::ToolUse);
                Box::pin(stream::iter([
                    Ok(LlmEvent::ToolCallComplete {
                        id: TOOL_CALL_ID.to_string(),
                        name: "fork_probe".to_string(),
                        args: json!({}),
                        meta: None,
                    }),
                    usage,
                    done,
                ]))
            }
            1 => {
                let [usage, done] = Self::usage_then_done(request, StopReason::EndTurn);
                Box::pin(stream::iter([
                    Ok(LlmEvent::TextDelta {
                        delta: "fork source complete".to_string(),
                        meta: None,
                    }),
                    usage,
                    done,
                ]))
            }
            2 => {
                let entered = Arc::clone(&self.running_entered);
                let release = Arc::clone(&self.release_running);
                let [usage, done] = Self::usage_then_done(request, StopReason::EndTurn);
                let blocked = stream::once(async move {
                    entered.notify_one();
                    release.notified().await;
                    Ok(LlmEvent::TextDelta {
                        delta: "running source released".to_string(),
                        meta: None,
                    })
                });
                Box::pin(blocked.chain(stream::iter([usage, done])))
            }
            _ => {
                let [usage, done] = Self::usage_then_done(request, StopReason::EndTurn);
                Box::pin(stream::iter([
                    Ok(LlmEvent::TextDelta {
                        delta: "deterministic follow-up".to_string(),
                        meta: None,
                    }),
                    usage,
                    done,
                ]))
            }
        }
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

struct ForkHarness<B> {
    runtime: UnifiedRuntime,
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    blob_store: Arc<B>,
    client: Arc<ForkScriptClient>,
    _state: tempfile::TempDir,
}

/// Each test boots its own mob, named `mob_id`: tests in one binary share a
/// process, and a mob id names its supervisor's process-wide participant
/// (`<mob>/__mob_supervisor__`), so a shared id would collide.
async fn harness(mob_id: &str) -> ForkHarness<meerkat_store::MemoryBlobStore> {
    harness_with_blob_store(mob_id, Arc::new(meerkat_store::MemoryBlobStore::new())).await
}

async fn harness_with_blob_store<B: BlobStore + 'static>(
    mob_id: &str,
    blob_store: Arc<B>,
) -> ForkHarness<B> {
    let state = tempfile::tempdir().expect("state tempdir");
    let root = state.path().join("state");
    std::fs::create_dir_all(&root).expect("state directory");

    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
        meerkat_store::SqliteSessionStore::open(root.join("sessions.sqlite"))
            .expect("session store"),
    );
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(root.join("runtime.sqlite"))
            .expect("runtime store"),
    );
    let blob_store_trait: Arc<dyn BlobStore> = blob_store.clone();
    let client = Arc::new(ForkScriptClient::new());
    let factory = AgentFactory::new(&root).comms(true).builtins(false);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store_trait.clone());
    builder.default_llm_client = Some(client.clone());
    let service = Arc::new(PersistentSessionService::new(
        builder,
        8,
        session_store,
        runtime_store,
        blob_store_trait,
    ));

    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
    ))
    .expect("fork integration definition");
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), service.clone())
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(client.clone()),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: mob_id.to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("persistent UnifiedRuntime bootstrap");

    ForkHarness {
        runtime,
        service,
        blob_store,
        client,
        _state: state,
    }
}

fn worker_spec(alias: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::new("worker", AgentIdentity::from(alias))
}

fn assert_complete_tool_group(messages: &[Message]) -> (usize, usize) {
    let tool_use_index = messages
        .iter()
        .position(|message| {
            matches!(message, Message::BlockAssistant(assistant)
                if assistant.get_tool_use(TOOL_CALL_ID).is_some())
        })
        .expect("forked transcript carries tool use");
    let result_index = messages
        .iter()
        .position(|message| {
            matches!(message, Message::ToolResults { results, .. }
                if results.len() == 1
                    && results[0].tool_use_id == TOOL_CALL_ID
                    && results[0].text_content() == TOOL_RESULT_TEXT)
        })
        .expect("forked transcript carries matching tool result");
    assert_eq!(
        result_index,
        tool_use_index + 1,
        "tool-use/result must remain one adjacent complete transcript group"
    );
    (tool_use_index, result_index)
}

fn image_blob_id(messages: &[Message]) -> meerkat_core::BlobId {
    messages
        .iter()
        .find_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Image {
                    media_type,
                    data: ImageData::Blob { blob_id },
                } if media_type == "image/png" => Some(blob_id.clone()),
                _ => None,
            }),
            _ => None,
        })
        .expect("persisted transcript carries an externalized PNG blob")
}

#[tokio::test]
async fn unified_runtime_fork_is_durable_typed_and_recoverable() {
    let ForkHarness {
        runtime,
        service,
        blob_store,
        client,
        _state,
    } = harness("persistent-fork-integration-durable").await;

    let mut source_spec = worker_spec("source");
    source_spec.external_tools = Some(Arc::new(ForkProbeTool));
    runtime.spawn(source_spec).await.expect("spawn source");

    let admission = runtime
        .start_member_turn(
            "source",
            ContentInput::Blocks(vec![
                ContentBlock::Text {
                    text: "seed durable fork".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Inline {
                        data: IMAGE_BASE64.to_string(),
                    },
                },
            ]),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await
        .expect("admit source turn");
    let source_session_id =
        meerkat_core::SessionId::parse(&admission.session_id).expect("source admission session id");
    admission
        .turn
        .wait()
        .await
        .expect("source turn reaches its exact committed terminal");

    let source_history = service
        .read_history(&source_session_id, SessionHistoryQuery::default())
        .await
        .expect("read committed source history");
    let (source_tool_use_index, _) = assert_complete_tool_group(&source_history.messages);
    let source_blob_id = image_blob_id(&source_history.messages);
    let source_blob = blob_store
        .get(&source_blob_id)
        .await
        .expect("read source image blob");
    assert_eq!(source_blob.media_type, "image/png");
    assert_eq!(source_blob.data, IMAGE_BASE64);

    let split_error = runtime
        .fork_member(
            "source",
            worker_spec("split-child"),
            Some(source_tool_use_index + 1),
        )
        .await
        .expect_err("a prefix ending after tool-use but before tool-result is refused");
    assert!(matches!(
        split_error,
        ForkMemberError::Mob(MobError::SessionError(meerkat_core::SessionError::Agent(_)))
    ));
    assert!(
        runtime
            .mob_handle()
            .get_member(&AgentIdentity::from("split-child"))
            .await
            .expect("observe split child")
            .is_none(),
        "a rejected split never seats a child"
    );

    let forked = runtime
        .fork_member("source", worker_spec("child"), None)
        .await
        .expect("full durable fork succeeds");
    assert_eq!(forked.source_member_alias, "source");
    assert_eq!(forked.member_alias, "child");
    assert_eq!(forked.result.agent_identity, AgentIdentity::from("child"));
    assert!(matches!(
        forked.result.cache_inheritance,
        ForkCacheInheritance::Unavailable {
            message_count,
            reason: ForkCacheInheritanceUnavailableReason::TargetIdentityUnresolved,
        } if message_count == source_history.message_count
    ));

    let child_history = service
        .read_history(&forked.result.session_id, SessionHistoryQuery::default())
        .await
        .expect("read committed child history");
    assert_ne!(forked.result.session_id, source_session_id);
    assert_eq!(child_history.message_count, source_history.message_count);
    assert_eq!(
        child_history.messages, source_history.messages,
        "the durable child inherits every committed typed transcript message exactly"
    );
    assert_eq!(
        meerkat_core::transcript_messages_digest(&child_history.messages)
            .expect("digest child transcript"),
        meerkat_core::transcript_messages_digest(&source_history.messages)
            .expect("digest source transcript"),
        "the durable child inherits the exact committed transcript bytes"
    );
    assert_complete_tool_group(&child_history.messages);
    let child_blob_id = image_blob_id(&child_history.messages);
    assert_eq!(child_blob_id, source_blob_id);
    let child_blob = blob_store
        .get(&child_blob_id)
        .await
        .expect("read child image blob");
    assert_eq!(child_blob.media_type, "image/png");
    assert_eq!(child_blob.data, IMAGE_BASE64);

    let running = runtime
        .start_member_turn(
            "source",
            ContentInput::Text("hold source running".to_string()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await
        .expect("admit blocking source turn");
    client.running_entered.notified().await;
    let running_error = runtime
        .fork_member("source", worker_spec("running-child"), None)
        .await
        .expect_err("running source must be refused");
    assert!(matches!(
        running_error,
        ForkMemberError::Mob(MobError::ForkSourceUnavailable {
            source_member_id,
            cause: ForkSourceUnavailableCause::Running,
        }) if source_member_id == "source"
    ));
    client.release_running.notify_one();
    running
        .turn
        .wait()
        .await
        .expect("released source turn reaches its exact committed terminal");

    runtime
        .spawn(worker_spec("recoverable"))
        .await
        .expect("seat collision member");
    let provision_error = runtime
        .fork_member("source", worker_spec("recoverable"), None)
        .await
        .expect_err("existing child alias fails only after durable fork commit");
    let (committed_child_session_id, structured) = match &provision_error {
        ForkMemberError::Mob(MobError::ForkMemberProvisionFailed {
            member_id,
            fork_session_id,
            ..
        }) => {
            assert_eq!(member_id, &AgentIdentity::from("recoverable"));
            (
                fork_session_id.clone(),
                provision_error
                    .structured_data()
                    .expect("provision failure has recovery data"),
            )
        }
        other => panic!("expected typed committed provision failure, got {other:?}"),
    };
    assert!(provision_error.committed_fork_is_recoverable());
    assert_eq!(
        structured,
        json!({
            "kind": "fork_member_provision_failed",
            "member_id": "recoverable",
            "fork_session_id": committed_child_session_id.to_string(),
            "recovery": "resume_committed_fork_session",
        })
    );
    let committed_unseated_history = service
        .read_history(&committed_child_session_id, SessionHistoryQuery::default())
        .await
        .expect("committed child remains readable after provision failure");
    assert_complete_tool_group(&committed_unseated_history.messages);
    assert_eq!(
        image_blob_id(&committed_unseated_history.messages),
        source_blob_id
    );

    runtime
        .mob_handle()
        .retire(AgentIdentity::from("recoverable"))
        .await
        .expect("retire colliding exact member incarnation");
    let mut recovery_spec = worker_spec("recoverable");
    recovery_spec.launch_mode = MemberLaunchMode::Resume {
        // 0.8.25: no migration authority on this path; a declaration is
        // attached only where resume_session detects a genuine role divergence.
        resume_from_role: None,
        bridge_session_id: committed_child_session_id.clone(),
    };
    runtime
        .spawn(recovery_spec)
        .await
        .expect("resume exact committed fork session");
    assert_eq!(
        runtime
            .mob_handle()
            .resolve_bridge_session_id(&AgentIdentity::from("recoverable"))
            .await,
        Some(committed_child_session_id.clone()),
        "recovery seats the exact committed child session, not a replacement"
    );
    let recovered_history = service
        .read_history(&committed_child_session_id, SessionHistoryQuery::default())
        .await
        .expect("read recovered child history");
    assert_eq!(
        recovered_history.messages, committed_unseated_history.messages,
        "ordinary resume preserves every committed typed transcript message exactly"
    );
    assert_eq!(
        meerkat_core::transcript_messages_digest(&recovered_history.messages)
            .expect("digest recovered transcript"),
        meerkat_core::transcript_messages_digest(&committed_unseated_history.messages)
            .expect("digest committed unseated transcript"),
        "ordinary resume preserves the exact committed fork transcript"
    );
    assert_complete_tool_group(&recovered_history.messages);
    assert_eq!(image_blob_id(&recovered_history.messages), source_blob_id);

    let shutdown = runtime.shutdown().await;
    assert!(
        shutdown.cleanup_completed(),
        "fork harness shutdown must close every authority owner: {shutdown:?}"
    );
}

/// Rewrite every blob-backed image reference `from` to `to`.
fn with_image_blob_id(
    messages: &[Message],
    from: &meerkat_core::BlobId,
    to: &meerkat_core::BlobId,
) -> Vec<Message> {
    let mut messages = messages.to_vec();
    for message in &mut messages {
        if let Message::User(user) = message {
            for block in &mut user.content {
                if let ContentBlock::Image {
                    data: ImageData::Blob { blob_id },
                    ..
                } = block
                    && blob_id == from
                {
                    *blob_id = to.clone();
                }
            }
        }
    }
    messages
}

/// Downstream-app regression: MobKit before 0.8.41 stored meerkat images under its
/// raw-bytes address, `sha256(media_type || 0x00 || decoded_bytes)`, and those
/// references stay in committed transcripts. meerkat's durable-fork preflight
/// recomputes its own content address and refused every fork of such a
/// transcript with "blob identity mismatch". Meta's adapter now attests its
/// own recipe, so the fork commits, the child's reference is re-homed to the
/// meerkat content address, and the source keeps its reference and object.
#[tokio::test]
async fn unified_runtime_forks_a_transcript_holding_a_pre_0841_image_reference() {
    use meerkat_meta::{Base64BlobStoreAdapter, BinaryBlobStore, ObjectStoreBlobStore};

    let binary: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
    // Exactly what the pre-0.8.41 adapter did with meerkat's base64 image.
    let legacy = binary
        .put_bytes("image/png", bytes::Bytes::from_static(b"\x89PNG\r\n\x1a\n"))
        .await
        .expect("store image under the pre-0.8.41 address");
    let legacy_id = legacy.blob_id;
    let content_id = meerkat_core::content_blob_id("image/png", IMAGE_BASE64);
    assert_ne!(
        legacy_id, content_id,
        "the fixture must model a non-content address"
    );

    let ForkHarness {
        runtime,
        service,
        blob_store,
        client,
        _state,
    } = harness_with_blob_store(
        "persistent-fork-integration-pre-0841-image",
        Arc::new(Base64BlobStoreAdapter::new(binary.clone())),
    )
    .await;

    let mut source_spec = worker_spec("source");
    source_spec.external_tools = Some(Arc::new(ForkProbeTool));
    runtime.spawn(source_spec).await.expect("spawn source");
    let admission = runtime
        .start_member_turn(
            "source",
            ContentInput::Blocks(vec![
                ContentBlock::Text {
                    text: "seed a legacy image reference".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Blob {
                        blob_id: legacy_id.clone(),
                    },
                },
            ]),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await
        .expect("admit source turn");
    let source_session_id =
        meerkat_core::SessionId::parse(&admission.session_id).expect("source admission session id");
    admission
        .turn
        .wait()
        .await
        .expect("source turn reaches its exact committed terminal");

    let source_history = service
        .read_history(&source_session_id, SessionHistoryQuery::default())
        .await
        .expect("read committed source history");
    assert_eq!(
        image_blob_id(&source_history.messages),
        legacy_id,
        "the source transcript holds the pre-0.8.41 reference"
    );

    let forked = runtime
        .fork_member("source", worker_spec("child"), None)
        .await
        .expect("a transcript with a pre-0.8.41 image reference forks");
    assert_eq!(forked.member_alias, "child");

    let child_history = service
        .read_history(&forked.result.session_id, SessionHistoryQuery::default())
        .await
        .expect("read committed child history");
    assert_eq!(image_blob_id(&child_history.messages), content_id);
    assert_eq!(
        child_history.messages,
        with_image_blob_id(&source_history.messages, &legacy_id, &content_id),
        "the child inherits the transcript with only the image reference re-homed"
    );
    meerkat_core::verify_stored_image_blob(blob_store.as_ref(), &content_id, "image/png", 1 << 20)
        .await
        .expect("the child's reference verifies under meerkat's strict gate");

    let source_after = service
        .read_history(&source_session_id, SessionHistoryQuery::default())
        .await
        .expect("read source history after the fork");
    assert_eq!(
        source_after.messages, source_history.messages,
        "the fork leaves the source transcript untouched"
    );
    let source_object = binary
        .get_bytes(&legacy_id)
        .await
        .expect("the source's legacy object is kept");
    assert_eq!(source_object.data.as_ref(), b"\x89PNG\r\n\x1a\n");

    // The child runs a turn over its re-homed history. The script's third
    // call parks until `release_running`; a stored permit lets it through.
    client.release_running.notify_one();
    runtime
        .start_member_turn(
            "child",
            ContentInput::Text("continue in the child".to_string()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await
        .expect("admit child turn")
        .turn
        .wait()
        .await
        .expect("child turn over the re-homed image reaches its committed terminal");

    let shutdown = runtime.shutdown().await;
    assert!(
        shutdown.cleanup_completed(),
        "fork harness shutdown must close every authority owner: {shutdown:?}"
    );
}
