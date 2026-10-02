//! OB3 on the real runtime-backed stack: a graceful mob Shutdown with a
//! member turn held in its provider call across the Shutdown, plus an owned
//! retirement settled before it, completes and accounts for every member.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::HandlingMode;
use meerkat_mob::{
    AgentIdentity, MemberShutdownOutcome, MemberTurnOptions, MobBuilder, MobDefinition, MobId,
    MobRuntimeMode, MobStorage, Profile, ProfileBinding, ProfileName, RetirementSettlement,
    SpawnMemberSpec, ToolConfig,
};

/// Bound for test steps that must happen promptly; it only turns a hang into
/// a failure.
const STEP: Duration = Duration::from_secs(60);
const BUSY_PROMPT: &str = "OB3-BUSY";

/// The busy prompt's provider call never answers (its turn is held across
/// the Shutdown); every other request answers at once.
struct Client {
    busy_entered: tokio::sync::Notify,
}

fn done(request: &LlmRequest) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta {
            delta: "ok".to_string(),
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

#[async_trait::async_trait]
impl LlmClient for Client {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let busy = request.messages.iter().any(|message| {
            !matches!(message, Message::System(_))
                && serde_json::to_string(message)
                    .unwrap_or_default()
                    .contains(BUSY_PROMPT)
        });
        if busy {
            return Box::pin(
                futures::stream::once(async move {
                    self.busy_entered.notify_one();
                    std::future::pending::<()>().await;
                })
                .map(|()| unreachable!("the held provider call never answers")),
            );
        }
        Box::pin(futures::stream::iter(done(request).into_iter().map(Ok)))
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
    client: Arc<Client>,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<meerkat_runtime::MeerkatMachine>,
) {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "ob3-shutdown-realm",
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
        64,
        persistence,
        root.join("config-state.json"),
    )
}

fn definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("sweeper"),
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
            peer_description: "sweeper".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        })),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "ob3-shutdown-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles = profiles;
    definition
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_with_a_turn_held_across_it_completes_and_reports_each_member() {
    let temp = tempfile::tempdir().expect("temp dir");
    let client = Arc::new(Client {
        busy_entered: tokio::sync::Notify::new(),
    });
    let (service, adapter) = build_service(temp.path(), Arc::clone(&client)).await;
    let storage = MobStorage::persistent(temp.path().join("mob.db")).expect("mob storage");
    let handle = MobBuilder::new(definition(), storage)
        .with_session_service(service.clone())
        .with_runtime_adapter(adapter.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create mob");

    let busy = AgentIdentity::from("busy-sweeper");
    let idle = AgentIdentity::from("idle-sweeper");
    let retiring = AgentIdentity::from("retired-sweeper");
    for identity in [&busy, &idle, &retiring] {
        handle
            .spawn_spec(SpawnMemberSpec::new("sweeper", identity.clone()))
            .await
            .expect("spawn");
    }

    // An idle retire is owned and settles Retired.
    tokio::time::timeout(STEP, handle.retire(retiring.clone()))
        .await
        .expect("retire answers")
        .expect("idle retire completes");
    let settled = handle
        .retirement_settlement(&retiring)
        .expect("settlement published")
        .current();
    assert!(
        matches!(settled, RetirementSettlement::Retired),
        "{settled:?}"
    );

    // A turn held in its provider call across the Shutdown.
    let _turn = handle
        .member(&busy)
        .await
        .expect("member")
        .start_turn(
            BUSY_PROMPT,
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("busy turn admitted");
    tokio::time::timeout(STEP, client.busy_entered.notified())
        .await
        .expect("the busy turn reaches its provider call");

    let report = tokio::time::timeout(
        STEP,
        handle.shutdown_with_report(meerkat_mob::ShutdownOptions::default()),
    )
    .await
    .expect("shutdown completes with a turn held across it")
    .expect("shutdown succeeds");
    assert!(
        matches!(
            report.members.get(&idle),
            Some(MemberShutdownOutcome::Unregistered)
        ),
        "the idle member is unregistered: {report:?}"
    );
    // The held turn's member is accounted for either way: unregistered, or
    // left to its runtime coordinator and reported, never waited on.
    assert!(
        matches!(
            report.members.get(&busy),
            Some(
                MemberShutdownOutcome::Unregistered
                    | MemberShutdownOutcome::UnregisterPending { .. }
            )
        ),
        "the busy member is reported: {report:?}"
    );
    assert!(
        !report.members.contains_key(&retiring),
        "a retired member is not part of the Shutdown: {report:?}"
    );
}
