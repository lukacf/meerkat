#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use async_trait::async_trait;
use meerkat_core::{
    AgentError, AgentEvent, AgentLlmClient, AssistantBlock, ContentBlock, LlmStreamResult, Message,
    Provider, Session, StopReason, ToolDef, TurnUsage, Usage,
    skills::{SkillFilter, SourceHealthState},
    skills_config::SkillRepoTransport,
};
use std::sync::Mutex;

#[derive(Default)]
struct CaptureClient(Mutex<Vec<Message>>);

#[async_trait]
impl AgentLlmClient for CaptureClient {
    async fn stream_response(
        &self,
        messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        *self.0.lock().unwrap() = messages.to_vec();
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: "synthetic review".into(),
                meta: None,
            }],
            StopReason::EndTurn,
            TurnUsage::host_declared(Provider::Anthropic, self.model(), Usage::default())
                .into_inner(),
        ))
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    fn model(&self) -> &str {
        "claude-sonnet-4-6"
    }
}

#[async_trait]
impl meerkat_client::LlmClient for CaptureClient {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat::LlmRequest,
    ) -> std::pin::Pin<
        Box<
            dyn futures::Stream<Item = Result<meerkat::LlmEvent, meerkat_client::LlmError>>
                + Send
                + 'a,
        >,
    > {
        *self.0.lock().unwrap() = request.messages.clone();
        Box::pin(futures::stream::iter([
            Ok(meerkat::LlmEvent::TextDelta {
                delta: "synthetic review".into(),
                meta: None,
            }),
            Ok(meerkat::LlmEvent::UsageUpdate {
                usage: TurnUsage::host_declared(
                    Provider::Anthropic,
                    &request.model,
                    Usage::default(),
                ),
            }),
            Ok(meerkat::LlmEvent::Done {
                outcome: meerkat::LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                },
            }),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

#[tokio::test]
async fn all_three_canonical_skills_list_and_filesystem_key_loads() {
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let engine = skill_engine(root.path()).unwrap();
    let listed = engine.list_skills(&SkillFilter::default()).await.unwrap();
    let mut keys: Vec<_> = listed.iter().map(|d| d.key.clone()).collect();
    let fs_key = SkillKey::new(
        SourceUuid::project_local(),
        SkillName::parse("security-auditor").unwrap(),
    );
    let mut expected = vec![
        review_key(),
        SkillKey::builtin(SkillName::parse("review-api-designer").unwrap()),
        fs_key.clone(),
    ];
    keys.sort();
    expected.sort();
    assert_eq!(keys, expected);
    let source_uuid = fs_key.source_uuid.to_string();
    let loaded = engine
        .load_from_source(&fs_key, Some(&source_uuid))
        .await
        .unwrap();
    assert_eq!(loaded.descriptor.key, fs_key);
    assert!(loaded.body.contains("You are a security auditor"));
    let health = engine.health_snapshot().await.unwrap();
    assert_eq!(health.invalid_count, 0);
    assert_eq!(health.state, SourceHealthState::Healthy);
}

#[test]
fn printed_skill_and_configuration_parse_through_real_types() {
    let shell = meerkat_skills::parser::parse_skill_md(
        SkillKey::builtin(SkillName::parse("shell-patterns").unwrap()),
        SkillScope::Project,
        SHELL_SKILL,
        Some("shell-patterns"),
    )
    .unwrap();
    assert!(shell.body.contains("Use explicit working directories"));
    let config: meerkat::Config = toml::from_str(SKILLS_CONFIG).unwrap();
    assert_eq!(config.skills.repositories.len(), 2);
    let fs = &config.skills.repositories[0];
    let git = &config.skills.repositories[1];
    assert_eq!(fs.name, "project-examples");
    assert_eq!(
        fs.source_uuid.to_string(),
        "11111111-1111-4111-8111-111111111111"
    );
    assert!(
        matches!(&fs.transport, SkillRepoTransport::Filesystem { path } if path == ".rkat/skills/")
    );
    assert_eq!(git.name, "team-examples");
    assert_eq!(
        git.source_uuid.to_string(),
        "22222222-2222-4222-8222-222222222222"
    );
    assert!(
        matches!(&git.transport, SkillRepoTransport::Git { url, .. } if url == "https://github.com/org/skills.git")
    );
}

#[tokio::test]
async fn typed_activation_reaches_model_with_canonical_key_and_event() {
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let engine = skill_engine(&root.path().join("skills")).unwrap();
    let client = Arc::new(CaptureClient::default());
    let store = Arc::new(JsonlStore::new(root.path().join("sessions")));
    store.init().await.unwrap();
    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-6")
        .with_skill_engine(Arc::new(SkillRuntime::new(Arc::new(engine))))
        .build(
            client.clone(),
            Arc::new(EmptyToolDispatcher),
            Arc::new(StoreAdapter::new(store)),
        )
        .await
        .unwrap();
    agent.pending_skill_references = Some(vec![review_key()]);
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    agent
        .run_with_events("synthetic Rust review".into(), tx)
        .await
        .unwrap();
    let messages = client.0.lock().unwrap();
    let blocks: Vec<_> = messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => Some(user.content.iter()),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::SkillContext { skill_key, text } => Some((skill_key, text)),
            _ => None,
        })
        .collect();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].0, &review_key());
    assert!(blocks[0].1.contains("Check ownership patterns"));
    let content = serde_json::to_string(&*messages).unwrap();
    assert!(!content.contains("You are an API design consultant"));
    assert!(!content.contains("You are a security auditor"));
    let mut resolved = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::SkillsResolved { skills, .. } => {
                assert_eq!(skills, vec![review_key()]);
                resolved = true;
            }
            AgentEvent::SkillResolutionFailed { .. } => panic!("skill activation failed"),
            _ => {}
        }
    }
    assert!(resolved);
}

#[tokio::test]
async fn documented_builtin_preload_survives_cli_precreated_session_shape() {
    use meerkat::surface::{
        build_runtime_backed_service, default_persistent_executor, materialize_session,
    };

    let slug = PRELOAD_COMMAND.split_whitespace().nth(3).unwrap();
    let key = SkillKey::builtin(SkillName::parse(slug).unwrap());
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let client = Arc::new(CaptureClient::default());
    let factory = AgentFactory::new(root.path().join("sessions"))
        .runtime_root(root.path().to_path_buf())
        .project_root(root.path())
        .builtins(true)
        .shell(false);
    let store = Arc::new(JsonlStore::new(root.path().join("sessions")));
    store.init().await.unwrap();
    let persistence = meerkat::PersistenceBundle::new(
        store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    );
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
    builder.default_llm_client = Some(client.clone());
    let (service, machine) = build_runtime_backed_service(builder, 4, persistence);
    let service = Arc::new(service);
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    // Use the surface's real materialization protocol: it binds the precreated
    // session and admits/stamps the first ContentTurn through machine authority.
    let result = materialize_session(
        &service,
        &machine,
        Session::new(),
        meerkat::CreateSessionRequest {
            injected_context: vec![],
            model: "claude-sonnet-4-6".into(),
            prompt: "Explain the builtin utility workflow.".into(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: Some(256),
            event_tx: Some(tx),
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions {
                preload_skills: Some(vec![key.clone()]),
                ..Default::default()
            }),
            labels: None,
        },
        {
            let service = Arc::clone(&service);
            let machine = Arc::clone(&machine);
            move |id| default_persistent_executor(service, machine, id)
        },
    )
    .await
    .unwrap();
    let session = service
        .export_live_session(&result.session_id)
        .await
        .unwrap();
    assert_eq!(
        session.session_metadata().unwrap().tooling.active_skills,
        Some(vec![key])
    );
    assert_eq!(result.text, "synthetic review");
    let body = serde_json::to_string(&*client.0.lock().unwrap()).unwrap();
    assert!(body.contains("builtin-utilities-workflow"));
    assert!(
        body.contains("Use `datetime` when relative dates"),
        "preload must include the skill's body: {body}"
    );
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(event.payload, AgentEvent::SkillResolutionFailed { .. }),
            "{event:?}"
        );
    }
    machine
        .unregister_session(&result.session_id)
        .await
        .unwrap();
}
