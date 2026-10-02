//! A spawn completes while another member's turn runs, on the real stack.
//!
//! Regression for the OB3 spawn stall: a coordinator spawns workers from a
//! tool call (its turn stays in flight), while a host-side session-task
//! command is parked on the coordinator's busy session task. Every stage of the worker spawn,
//! including `finalize_spawn_admit`'s supervisor private-trust install for a
//! comms-capable member, must complete before the coordinator's turn ends.
//!
//! Before the fix, the parked command held the session service's map for the
//! whole coordinator turn and the worker's session create queued behind it.
//! With the map released, nothing in the spawn path may wait on the
//! coordinator's turn; a lock the coordinator's turn holds anywhere in the
//! spawn (supervisor trust included) fails this test.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt as _;
use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::HandlingMode;
use meerkat_mob::{
    AgentIdentity, MemberTurnOptions, MobBuilder, MobDefinition, MobId, MobRuntimeMode, MobStorage,
    Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};

const COORDINATOR: &str = "ob3-coordinator";
const WORKER: &str = "ob3-review-worker";
const FINISHED: &str = "ob3-finished-worker";
const HELD_PROMPT: &str = "spawn the review workers";
/// Bounds a failure only; the passing path never waits for it.
const FAILURE_BOUND: Duration = Duration::from_secs(60);

/// The coordinator's first request waits for the test (its turn stays in
/// flight); every other request answers at once.
struct GatedClient {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

fn done(request: &LlmRequest) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta {
            delta: "done".to_string(),
            meta: None,
        },
        LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::OpenAI,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        },
        LlmEvent::Done {
            outcome: LlmDoneOutcome::Success {
                stop_reason: meerkat_core::StopReason::EndTurn,
            },
        },
    ]
}

fn mentions(messages: &[Message], needle: &str) -> bool {
    messages
        .iter()
        .filter(|message| !matches!(message, Message::System(_)))
        .any(|message| {
            serde_json::to_string(message)
                .expect("message renders")
                .contains(needle)
        })
}

#[async_trait::async_trait]
impl LlmClient for GatedClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let events = done(request);
        if mentions(&request.messages, HELD_PROMPT) {
            return Box::pin(
                futures::stream::once(async move {
                    self.entered.notify_one();
                    self.release.notified().await;
                })
                .flat_map(move |()| futures::stream::iter(events.clone().into_iter().map(Ok))),
            );
        }
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

async fn build_service(
    root: &Path,
    client: Arc<GatedClient>,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<meerkat_runtime::MeerkatMachine>,
) {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "ob3-spawn-realm",
        Some(meerkat_store::RealmBackend::Sqlite),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let factory = AgentFactory::new(root.join("sessions"))
        .runtime_root(root.join("realm"))
        .project_root(&project)
        .builtins(true)
        .comms(true);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(client);
    meerkat::surface::build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        8,
        persistence,
        root.join("config-state.json"),
    )
}

fn mob_definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(Profile {
            model_fallback: None,
            model: "gpt-5.5".to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..Default::default()
            },
            peer_description: "OB3 member".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        })),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "ob3-spawn-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles = profiles;
    definition
}

/// Captured log lines, used to prove the supervisor-trust stage ran.
#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogCapture;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn comms_member_spawn_completes_while_the_coordinator_turn_runs() {
    let logs = LogCapture::default();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "meerkat_mob::runtime::actor=debug",
        ))
        .with_writer(logs.clone())
        .with_ansi(false)
        .init();

    let temp = tempfile::tempdir().expect("temp dir");
    let client = Arc::new(GatedClient {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let (service, adapter) = build_service(temp.path(), Arc::clone(&client)).await;
    let storage = MobStorage::persistent(temp.path().join("mob.db")).expect("mob storage");
    let handle = MobBuilder::new(mob_definition(), storage)
        .with_session_service(service.clone())
        .with_runtime_adapter(adapter.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create persistent mob");

    let coordinator = AgentIdentity::from(COORDINATOR);
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", coordinator.clone()))
        .await
        .expect("spawn the coordinator");
    let coordinator_session = handle
        .resolve_bridge_session_id(&coordinator)
        .await
        .expect("coordinator session");
    let finished = AgentIdentity::from(FINISHED);
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", finished.clone()))
        .await
        .expect("spawn a member that finishes during the coordinator turn");

    // The coordinator's turn stays in flight, as a spawn tool call holds it.
    let member = handle
        .member(&coordinator)
        .await
        .expect("coordinator handle");
    let turn = member
        .start_turn(
            HELD_PROMPT,
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("start the coordinator turn");
    tokio::time::timeout(FAILURE_BOUND, client.entered.notified())
        .await
        .expect("the coordinator's provider request is in flight");

    // A host-side report parks on the coordinator's busy session task: a
    // session-task command whose reply arrives only after the turn ends. Any
    // such command used to hold the session service's map while it waited.
    let parked = tokio::spawn({
        let service = Arc::clone(&service);
        let coordinator_session = coordinator_session.clone();
        async move {
            meerkat_core::service::SessionService::record_live_terminal_error(
                service.as_ref(),
                &coordinator_session,
                meerkat_core::live_adapter::LiveAdapterErrorCode::ConnectionLost,
            )
            .await
        }
    });
    // Let the parked command reach the session task's queue (it cannot be
    // observed directly); before the fix it then held the session map.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !parked.is_finished(),
        "the coordinator's task is busy with its turn"
    );

    // Another member retires meanwhile: archiving its session writes the
    // session service's map. Before the fix that writer queued behind the
    // parked command, and every later reader of the map queued behind it.
    let retiring = tokio::spawn({
        let handle = handle.clone();
        let finished = finished.clone();
        async move { handle.retire(finished).await }
    });

    let worker = AgentIdentity::from(WORKER);
    tokio::time::timeout(
        FAILURE_BOUND,
        handle.spawn_spec(SpawnMemberSpec::new("worker", worker.clone())),
    )
    .await
    .expect("the worker spawn must not wait for the coordinator's turn to end")
    .expect("spawn the worker");
    assert!(
        handle.get_member(&worker).await.unwrap().is_some(),
        "the worker is seated"
    );
    assert!(
        !parked.is_finished(),
        "the coordinator's turn is still running, so the parked command is still pending"
    );
    {
        let captured = String::from_utf8_lossy(&logs.0.lock().unwrap()).into_owned();
        assert!(
            captured.lines().any(|line| {
                line.contains("finalize_spawn_admit installed supervisor private trust")
                    && line.contains(WORKER)
            }),
            "the worker spawn ran the supervisor private-trust stage"
        );
    }

    tokio::time::timeout(FAILURE_BOUND, retiring)
        .await
        .expect("the retire must not wait for the coordinator's turn to end")
        .expect("retire task")
        .expect("retire the finished member");

    client.release.notify_one();
    tokio::time::timeout(FAILURE_BOUND, turn.wait())
        .await
        .expect("the coordinator turn completes once released")
        .expect("the coordinator turn");
    let _ = parked.await;
    let _ = std::io::stdout().flush();
}
