//! A mob profile's `tools.deny` gates its members end to end.
//!
//! The HomeCore shape: a family stays enabled while named tools of it are
//! denied. A member built from a real factory composes the family, the deny
//! entry reaches the execution gate, and the model's call to the denied tool
//! comes back as an ordinary `access_denied` result without executing. A deny
//! entry naming a tool no enabled family provides fails the member's spawn
//! with a typed error naming the profile, the tool and the enabled families.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::HandlingMode;
use meerkat_core::{AgentEvent, Message};
use meerkat_mob::{
    AgentIdentity, MemberTurnOptions, MobBuilder, MobDefinition, MobId, MobRuntimeMode, MobStorage,
    Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};

const STEP: Duration = Duration::from_secs(30);
const MARKER: &str = "denied-shell-ran";

/// First request calls `shell` (which would create the marker file); the
/// request carrying the tool result ends the turn.
struct ShellThenDoneClient {
    marker: std::path::PathBuf,
}

fn done_events(
    request: &LlmRequest,
    mut events: Vec<LlmEvent>,
    stop: meerkat_core::StopReason,
) -> Vec<LlmEvent> {
    events.push(LlmEvent::UsageUpdate {
        usage: meerkat_core::TurnUsage::host_declared(
            meerkat_core::Provider::OpenAI,
            &request.model,
            meerkat_core::Usage::default(),
        ),
    });
    events.push(LlmEvent::Done {
        outcome: LlmDoneOutcome::Success { stop_reason: stop },
    });
    events
}

#[async_trait::async_trait]
impl LlmClient for ShellThenDoneClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let has_tool_result = request
            .messages
            .iter()
            .any(|message| matches!(message, Message::ToolResults { .. }));
        let events = if has_tool_result {
            done_events(
                request,
                vec![LlmEvent::TextDelta {
                    delta: "done".to_string(),
                    meta: None,
                }],
                meerkat_core::StopReason::EndTurn,
            )
        } else {
            done_events(
                request,
                vec![LlmEvent::ToolCallComplete {
                    id: "call-denied-shell".to_string(),
                    name: "shell".to_string(),
                    args: serde_json::json!({
                        "command": format!("touch '{}'", self.marker.display()),
                        "timeout_secs": 30,
                    }),
                    meta: None,
                }],
                meerkat_core::StopReason::ToolUse,
            )
        };
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
    client: Arc<ShellThenDoneClient>,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<meerkat_runtime::MeerkatMachine>,
) {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "profile-deny-realm",
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
        .shell(true)
        .comms(true);
    let mut config = Config::default();
    config.shell.program = "sh".to_string();
    config.shell.security_mode = meerkat_core::types::SecurityMode::Unrestricted;
    let mut builder = FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(client);
    meerkat::surface::build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        8,
        persistence,
        root.join("config-state.json"),
    )
}

fn mob_definition(deny: &[&str]) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("peer"),
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
                shell: true,
                comms: true,
                deny: deny.iter().map(|name| (*name).to_string()).collect(),
                ..Default::default()
            },
            peer_description: "household peer".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        })),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "profile-deny-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles = profiles;
    definition
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_tool_of_an_enabled_family_is_gated_for_the_member() {
    let temp = tempfile::tempdir().expect("temp dir");
    let marker = temp.path().join(MARKER);
    let client = Arc::new(ShellThenDoneClient {
        marker: marker.clone(),
    });
    let (service, adapter) = build_service(temp.path(), Arc::clone(&client)).await;
    let storage = MobStorage::persistent(temp.path().join("mob.db")).expect("mob storage");
    let handle = MobBuilder::new(mob_definition(&["shell"]), storage)
        .with_session_service(service.clone())
        .with_runtime_adapter(adapter.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create mob");
    let peer = AgentIdentity::from("kitchen");
    handle
        .spawn_spec(SpawnMemberSpec::new("peer", peer.clone()))
        .await
        .expect("a deny entry naming a composed tool spawns");
    let member = handle.member(&peer).await.expect("member handle");

    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(256);
    let turn = member
        .start_turn(
            "run the shell tool",
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            Some(events_tx),
        )
        .await
        .expect("start the turn");
    tokio::time::timeout(STEP, turn.wait())
        .await
        .expect("the turn completes")
        .expect("a gate denial does not fail the turn");

    let mut shell_result = None;
    while let Ok(envelope) = events_rx.try_recv() {
        if let AgentEvent::ToolExecutionCompleted {
            name,
            is_error,
            content,
            ..
        } = envelope.payload
            && name == "shell"
        {
            shell_result = Some((is_error, meerkat_core::types::text_content(&content)));
        }
    }
    let (is_error, text) = shell_result.expect("the shell call completed");
    assert!(is_error, "the denied call is an error result");
    assert!(
        text.contains("\"error\":\"access_denied\""),
        "the denied call carries the canonical access_denied payload: {text}"
    );
    assert!(!marker.exists(), "the denied shell command never ran");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deny_entry_in_no_tool_vocabulary_fails_the_spawn_typed() {
    let temp = tempfile::tempdir().expect("temp dir");
    let client = Arc::new(ShellThenDoneClient {
        marker: temp.path().join(MARKER),
    });
    let (service, adapter) = build_service(temp.path(), Arc::clone(&client)).await;
    let storage = MobStorage::persistent(temp.path().join("mob.db")).expect("mob storage");
    // `mob_wier` is in no tool vocabulary: stale configuration, never a
    // silently inert entry.
    let handle = MobBuilder::new(mob_definition(&["mob_wier"]), storage)
        .with_session_service(service.clone())
        .with_runtime_adapter(adapter.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create mob");
    let err = handle
        .spawn_spec(SpawnMemberSpec::new("peer", AgentIdentity::from("kitchen")))
        .await
        .expect_err("an unknown deny entry must fail the spawn");
    let message = err.to_string();
    for needle in [
        "profile 'peer'",
        "'mob_wier'",
        "shell, comms",
        "agent mob tools",
    ] {
        assert!(
            message.contains(needle),
            "{needle} missing from the spawn error: {message}"
        );
    }
}

/// The failed spawn's rollback compensates a registration that spawn created,
/// so it joins that registration's unregister saga until terminal: a saga
/// outliving the plain unregister's caller grace is cleanup in progress, and
/// the typed build error still reaches the caller. The saga is held until the
/// rollback has dispatched its wait, and the machine witnesses which wait
/// that was, so no scheduling decides the outcome.
#[tokio::test(flavor = "multi_thread")]
async fn a_deny_entry_in_no_tool_vocabulary_fails_the_spawn_typed_past_a_slow_rollback() {
    let temp = tempfile::tempdir().expect("temp dir");
    let client = Arc::new(ShellThenDoneClient {
        marker: temp.path().join(MARKER),
    });
    let (service, adapter) = build_service(temp.path(), Arc::clone(&client)).await;
    let storage = MobStorage::persistent(temp.path().join("mob.db")).expect("mob storage");
    let handle = MobBuilder::new(mob_definition(&["mob_wier"]), storage)
        .with_session_service(service.clone())
        .with_runtime_adapter(adapter.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create mob");
    let (saga_entered, release_saga) = adapter.test_hold_next_unregister_saga();
    let rollback_wait = adapter.test_witness_next_unregister_wait();
    adapter.test_set_unregister_caller_wait_grace(Duration::ZERO);
    let release_after_dispatch = async {
        saga_entered
            .await
            .expect("the spawn rollback started an unregister saga");
        let wait = rollback_wait
            .await
            .expect("the spawn rollback dispatched its wait on the held saga");
        let _ = release_saga.send(());
        wait
    };
    let (spawned, rollback_wait) = tokio::join!(
        handle.spawn_spec(SpawnMemberSpec::new("peer", AgentIdentity::from("kitchen"))),
        release_after_dispatch,
    );
    assert_eq!(
        rollback_wait,
        meerkat_runtime::UnregisterTeardownWaitWitness::UntilTerminal,
        "the rollback joins its saga until terminal instead of answering within the caller grace"
    );
    let message = spawned
        .expect_err("an unknown deny entry must fail the spawn")
        .to_string();
    for needle in [
        "profile 'peer'",
        "'mob_wier'",
        "shell, comms",
        "agent mob tools",
    ] {
        assert!(
            message.contains(needle),
            "{needle} missing from the spawn error: {message}"
        );
    }
}

/// A known tool the member does not mount is an inert deny entry: with
/// `mob = false` the agent mob tools are not composed, yet `mob_wire` is in
/// the agent mob tool vocabulary, so the member builds.
#[tokio::test(flavor = "multi_thread")]
async fn a_known_but_unmounted_deny_entry_is_inert() {
    let temp = tempfile::tempdir().expect("temp dir");
    let client = Arc::new(ShellThenDoneClient {
        marker: temp.path().join(MARKER),
    });
    let (service, adapter) = build_service(temp.path(), Arc::clone(&client)).await;
    let storage = MobStorage::persistent(temp.path().join("mob.db")).expect("mob storage");
    let handle = MobBuilder::new(mob_definition(&["mob_wire"]), storage)
        .with_session_service(service.clone())
        .with_runtime_adapter(adapter.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn_spec(SpawnMemberSpec::new("peer", AgentIdentity::from("kitchen")))
        .await
        .expect("a known but unmounted deny entry builds");
}
