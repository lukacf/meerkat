#![cfg(all(feature = "mcp", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real factory and agent-loop continuation after a refused local MCP launch.
//! This is host launch confinement, not a caller-authorization fixture.

use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use meerkat::{AgentBuildConfig, AgentFactory, LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_client::LlmClient;
use meerkat_core::confinement::{
    ConfinementRefusal, ConfinementSpec, FilesystemAccess, IpNetworkAccess, PathAccess,
    PlatformBaseline,
};
use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind};
use meerkat_core::{
    AgentToolDispatcher, Config, DynamicToolComposite, Message, Provider, ToolCallView, ToolDef,
    ToolDispatchOutcome, ToolError, ToolResult,
};
use meerkat_mcp::{McpConnection, McpError, McpRouter, McpRouterAdapter, McpStdioLaunchProfile};
use serde_json::json;

#[derive(Default)]
struct RecordingClient(Mutex<Vec<Vec<Message>>>);

#[async_trait::async_trait]
impl LlmClient for RecordingClient {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_llm_core::LlmStream<'a> {
        let mut requests = self.0.lock().unwrap();
        let first = requests.is_empty();
        requests.push(request.messages.clone());
        drop(requests);
        let mut events = Vec::new();
        let stop_reason = if first {
            assert!(
                request
                    .tools
                    .iter()
                    .any(|tool| tool.name == "permitted_sibling")
            );
            events.push(LlmEvent::ToolCallComplete {
                id: "sibling-call".into(),
                name: "permitted_sibling".into(),
                args: json!({}),
                meta: None,
            });
            meerkat_core::StopReason::ToolUse
        } else {
            events.push(LlmEvent::TextDelta {
                delta: "continued".into(),
                meta: None,
            });
            meerkat_core::StopReason::EndTurn
        };
        events.push(LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                Provider::Other,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        });
        events.push(LlmEvent::Done {
            outcome: LlmDoneOutcome::Success { stop_reason },
        });
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> Provider {
        Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

#[derive(Default)]
struct Sibling(AtomicUsize);

#[async_trait::async_trait]
impl AgentToolDispatcher for Sibling {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            "permitted_sibling",
            "A permitted local sibling",
            json!({"type":"object"}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::new(call.id.into(), "sibling completed".into(), false).into())
    }
}

fn failure_notices(messages: &[Message]) -> usize {
    messages.iter().filter(|message| match message {
        Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::Mcp => {
            notice.blocks.iter().any(|block| matches!(block,
                SystemNoticeBlock::Mcp { server_id: Some(server), phase: Some(meerkat_core::ExternalToolDeltaPhase::Failed), .. }
                if server == "refused-local-server"
            ))
        }
        _ => false,
    }).count()
}

async fn run_feedback_case(wait_at_build: bool) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let requirement = ConfinementSpec {
        baseline: PlatformBaseline::CommandRuntimeV1,
        read: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.clone())]),
        write: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.clone())]),
        deny_read: vec![],
        deny_write: vec![],
        network: IpNetworkAccess::Denied,
        unix_connect: vec![],
        require_descendant_termination: true,
    }
    .try_into()
    .unwrap();
    let profile = McpStdioLaunchProfile::required(requirement, root.clone());
    let config = meerkat_core::McpServerConfig::stdio(
        "refused-local-server",
        "/bin/sh",
        vec!["-c".into(), "printf entered > \"$MARKER\"".into()],
        HashMap::from([(
            "MARKER".into(),
            root.join("private-launch-marker").display().to_string(),
        )]),
    );
    // A real typed pre-spawn control also excludes a malformed shell fixture.
    assert!(matches!(
        McpConnection::connect_with_stdio_profile(&config, &profile).await,
        Err(McpError::Confinement(
            ConfinementRefusal::UnsupportedRequirement
        ))
    ));
    let client = Arc::new(RecordingClient::default());
    let sibling = Arc::new(Sibling::default());
    let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
    build.llm_client_override = Some(client.clone());
    build.wait_for_mcp = wait_at_build;
    let mut adapter = None;
    if wait_at_build {
        build.mcp_servers = vec![config];
        build.external_tools = Some(sibling.clone());
    } else {
        // Complete the real failure before the model boundary, retaining its
        // undrained notice in the existing adapter. No timing-only sleep.
        let handle = Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral());
        let mut router =
            McpRouter::new_with_surface_handle_and_stdio_profile(handle, profile.clone());
        assert!(matches!(
            router.add_server(config).await,
            Err(McpError::Confinement(
                ConfinementRefusal::UnsupportedRequirement
            ))
        ));
        let mcp = Arc::new(McpRouterAdapter::new(router));
        build.external_tools = Some(Arc::new(DynamicToolComposite::new(vec![
            sibling.clone(),
            mcp.clone(),
        ])));
        adapter = Some(mcp);
    }
    let factory = AgentFactory::minimal()
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .with_mcp_stdio_launch_profile(profile);
    let mut agent = tokio::time::timeout(
        Duration::from_secs(15),
        factory.build_agent(build, &Config::default()),
    )
    .await
    .unwrap()
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        agent.run("Use the available sibling".to_string().into()),
    )
    .await;
    if let Some(adapter) = adapter {
        adapter.shutdown().await;
    }
    assert_eq!(result.unwrap().unwrap().text, "continued");
    assert_eq!(sibling.0.load(Ordering::SeqCst), 1);
    let requests = client.0.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "launch refusal must not terminate or retry the run"
    );
    for request in requests.iter() {
        assert_eq!(
            failure_notices(request),
            1,
            "one failed launch, one retained model notice"
        );
        let notice = request
            .iter()
            .find_map(|message| match message {
                Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::Mcp => {
                    Some(notice)
                }
                _ => None,
            })
            .expect("retained setup failure notice");
        let block = serde_json::to_value(&notice.blocks[0]).unwrap();
        assert_eq!(block["confinement_refusal"], "unsupported_requirement");
        assert!(
            notice
                .model_projection_text()
                .contains(&ConfinementRefusal::UnsupportedRequirement.to_string())
        );
        let serialized = serde_json::to_string(request).unwrap();
        assert!(!serialized.contains(root.to_str().unwrap()));
        assert!(!serialized.contains("private-launch-marker"));
        assert!(!serialized.contains("printf entered"));
    }
    assert_eq!(failure_notices(agent.session().messages()), 1);
    assert!(!root.join("private-launch-marker").exists());
}

#[tokio::test]
async fn factory_wait_preserves_refused_mcp_launch_feedback_and_permitted_sibling() {
    run_feedback_case(true).await;
}

#[tokio::test]
async fn agent_boundary_projects_refused_mcp_launch_once_and_continues() {
    run_feedback_case(false).await;
}

#[cfg(feature = "test-mcp-oauth-fixtures")]
mod factory_async {
    use super::*;
    use futures::StreamExt;
    use meerkat::test_fixtures::mcp_oauth::McpOAuthFixture;
    use meerkat_core::event::{
        AgentEvent, ExternalToolDeltaPhase, ToolConfigChangeOperation, ToolConfigChangeStatus,
    };
    use std::path::Path;
    use tokio::sync::{Notify, mpsc};

    const BOUND: Duration = Duration::from_secs(15);
    const SERVER: &str = "refused-local-server";

    fn refused_launch(root: &Path) -> (McpStdioLaunchProfile, meerkat_core::McpServerConfig) {
        let requirement = ConfinementSpec {
            baseline: PlatformBaseline::CommandRuntimeV1,
            read: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.to_path_buf())]),
            write: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.to_path_buf())]),
            deny_read: vec![],
            deny_write: vec![],
            network: IpNetworkAccess::Denied,
            unix_connect: vec![],
            require_descendant_termination: true,
        }
        .try_into()
        .unwrap();
        let profile = McpStdioLaunchProfile::required(requirement, root.to_path_buf());
        let config = meerkat_core::McpServerConfig::stdio(
            SERVER,
            "/bin/sh",
            vec!["-c".into(), "printf entered > \"$MARKER\"".into()],
            HashMap::from([(
                "MARKER".into(),
                root.join("private-launch-marker").display().to_string(),
            )]),
        );
        (profile, config)
    }

    fn observed_factory(
        profile: McpStdioLaunchProfile,
        captured: Arc<Mutex<Vec<Arc<McpRouterAdapter>>>>,
    ) -> AgentFactory {
        AgentFactory::minimal()
            .session_store(Arc::new(meerkat_store::MemoryStore::new()))
            .with_mcp_stdio_launch_profile(profile)
            .with_mcp_router_observer_for_test(move |adapter| {
                captured.lock().unwrap().push(adapter);
            })
    }

    fn only_adapter(captured: &Mutex<Vec<Arc<McpRouterAdapter>>>) -> Arc<McpRouterAdapter> {
        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1, "observe the one factory-created router");
        Arc::clone(&captured[0])
    }

    fn assert_safe_failed_notice(
        messages: &[Message],
        operation: ToolConfigChangeOperation,
        root: &Path,
    ) {
        assert_eq!(failure_notices(messages), 1);
        let block_and_notice = messages.iter().find_map(|message| match message {
            Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::Mcp => {
                notice.blocks.iter().find_map(|block| match block {
                    SystemNoticeBlock::Mcp {
                        server_id: Some(server),
                        operation: Some(actual),
                        phase: Some(ExternalToolDeltaPhase::Failed),
                        confinement_refusal: Some(ConfinementRefusal::UnsupportedRequirement),
                        ..
                    } if server == SERVER && actual == &operation => Some(notice),
                    _ => None,
                })
            }
            _ => None,
        });
        let notice = block_and_notice.expect("exact accepted operation and typed refusal");
        let text = notice.model_projection_text();
        assert!(text.contains(&ConfinementRefusal::UnsupportedRequirement.to_string()));
        assert!(text.len() < 512, "bounded audience-safe setup feedback");
        let wire = serde_json::to_string(messages).unwrap();
        assert!(!wire.contains(root.to_str().unwrap()));
        assert!(!wire.contains("private-launch-marker"));
        assert!(!wire.contains("printf entered"));
    }

    fn assert_successful_sibling_result(messages: &[Message]) {
        let results = messages
            .iter()
            .filter_map(|message| match message {
                Message::ToolResults { results, .. } => Some(results.as_slice()),
                _ => None,
            })
            .flatten()
            .filter(|result| result.tool_use_id == "sibling-call")
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 1, "exactly one settled sibling result");
        assert!(!results[0].is_error);
        assert_eq!(results[0].text_content(), "sibling completed");
    }

    fn assert_one_failure_event_and_completed_run(
        events: &mut mpsc::Receiver<AgentEvent>,
        operation: ToolConfigChangeOperation,
    ) {
        let mut failed = Vec::new();
        let mut completed_runs = Vec::new();
        let mut failed_runs = 0;
        while let Ok(event) = events.try_recv() {
            match event {
                AgentEvent::RunCompleted { result, .. } => completed_runs.push(result),
                AgentEvent::RunFailed { .. } => failed_runs += 1,
                AgentEvent::ToolConfigChanged { payload }
                    if payload.target == SERVER
                        && matches!(
                            payload.status_info(),
                            ToolConfigChangeStatus::ExternalToolDelta {
                                phase: ExternalToolDeltaPhase::Failed,
                                ..
                            }
                        ) =>
                {
                    failed.push(payload);
                }
                _ => {}
            }
        }
        assert_eq!(
            failed_runs, 0,
            "local launch refusal cannot emit run failure"
        );
        assert_eq!(
            completed_runs,
            ["continued"],
            "exactly one successful terminal event"
        );
        assert_eq!(
            failed.len(),
            1,
            "the Agent accepts and projects failure once"
        );
        assert_eq!(failed[0].operation, operation);
        assert!(matches!(
            failed[0].status_info(),
            ToolConfigChangeStatus::ExternalToolDelta {
                confinement_refusal: Some(ConfinementRefusal::UnsupportedRequirement),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn factory_nonwaiting_startup_projects_real_refusal_once_and_continues() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (profile, config) = refused_launch(&root);
        let captured = Arc::new(Mutex::new(Vec::new()));
        let factory = observed_factory(profile, Arc::clone(&captured));
        let client = Arc::new(RecordingClient::default());
        let sibling = Arc::new(Sibling::default());
        let (events_tx, mut events) = mpsc::channel(256);
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.llm_client_override = Some(client.clone());
        build.external_tools = Some(sibling.clone());
        build.mcp_servers = vec![config];
        build.wait_for_mcp = false;
        build.event_tx = Some(events_tx);
        let mut agent = tokio::time::timeout(BOUND, factory.build_agent(build, &Config::default()))
            .await
            .unwrap()
            .unwrap();
        let adapter = only_adapter(&captured);
        // The observer neither supplies a prefailed adapter nor drains results.
        // This wait only proves that the factory's real task has delivered its
        // result; the actual Agent boundary must still accept and project it.
        // The immediate refusal may finish during build, so this does not claim
        // that build returned before the background connect task completed.
        let delivered = adapter.wait_connect_results_delivered(BOUND).await;
        let pending_before_run = adapter.pending_catalog_sources();
        let notices_before_run = failure_notices(agent.session().messages());
        let calls_before_run = client.0.lock().unwrap().len();
        let result = tokio::time::timeout(
            BOUND,
            agent.run("Use the available sibling".to_string().into()),
        )
        .await;
        let repeated = adapter.poll_external_updates().await;
        tokio::time::timeout(BOUND, adapter.shutdown())
            .await
            .unwrap();

        assert!(delivered, "bounded completion barrier for the factory task");
        assert_eq!(pending_before_run.as_ref(), &[SERVER.to_string()]);
        assert_eq!(
            notices_before_run, 0,
            "nonwaiting build leaves owner acceptance to the Agent"
        );
        assert_eq!(calls_before_run, 0);
        assert_eq!(result.unwrap().unwrap().text, "continued");
        assert_eq!(sibling.0.load(Ordering::SeqCst), 1);
        let requests = client.0.lock().unwrap().clone();
        assert_eq!(requests.len(), 2, "no retry or premature turn termination");
        for request in &requests {
            assert_safe_failed_notice(request, ToolConfigChangeOperation::Add, &root);
        }
        assert_safe_failed_notice(
            agent.session().messages(),
            ToolConfigChangeOperation::Add,
            &root,
        );
        assert_successful_sibling_result(&requests[1]);
        assert_successful_sibling_result(agent.session().messages());
        assert_one_failure_event_and_completed_run(&mut events, ToolConfigChangeOperation::Add);
        assert!(
            repeated.notices.is_empty(),
            "already accepted completion is not replayed"
        );
        assert!(repeated.pending.is_empty());
        assert!(!root.join("private-launch-marker").exists());
    }

    #[derive(Default)]
    struct HeldFirstResponseClient {
        recording: RecordingClient,
        calls: AtomicUsize,
        entered: Notify,
        release: Notify,
    }

    #[async_trait::async_trait]
    impl LlmClient for HeldFirstResponseClient {
        fn project_replay_messages(
            &self,
            messages: &[Message],
        ) -> Result<Vec<Message>, meerkat_client::LlmError> {
            self.recording.project_replay_messages(messages)
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_llm_core::LlmStream<'a> {
            let stream = self.recording.stream(request);
            if self.calls.fetch_add(1, Ordering::SeqCst) != 0 {
                return stream;
            }
            Box::pin(
                futures::stream::once(async move {
                    self.entered.notify_one();
                    self.release.notified().await;
                    stream
                })
                .flatten(),
            )
        }

        fn provider(&self) -> Provider {
            Provider::Other
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn factory_owned_in_run_reload_refusal_reaches_next_model_once_and_continues() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (profile, refused_config) = refused_launch(&root);
        // An actual ready HTTP server establishes a legal Reload transition.
        // The retained Required profile is then exercised by its stdio replacement.
        let server = McpOAuthFixture::spawn().await.unwrap();
        let ready_config = meerkat_core::McpServerConfig::streamable_http(
            SERVER,
            server.public_mcp_url(),
            HashMap::new(),
        );
        let captured = Arc::new(Mutex::new(Vec::new()));
        let factory = observed_factory(profile, Arc::clone(&captured));
        let client = Arc::new(HeldFirstResponseClient::default());
        let sibling = Arc::new(Sibling::default());
        let (events_tx, mut events) = mpsc::channel(256);
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.llm_client_override = Some(client.clone());
        build.external_tools = Some(sibling.clone());
        build.mcp_servers = vec![ready_config];
        build.wait_for_mcp = true;
        build.event_tx = Some(events_tx);
        let mut agent = tokio::time::timeout(BOUND, factory.build_agent(build, &Config::default()))
            .await
            .unwrap()
            .unwrap();
        let adapter = only_adapter(&captured);
        let ready_tools = adapter.tools();
        let ready_pending = adapter.pending_catalog_sources();
        let calls_before_run = client.calls.load(Ordering::SeqCst);
        let notices_before_run = failure_notices(agent.session().messages());
        let reload = async {
            let outcome = tokio::time::timeout(BOUND, async {
                client.entered.notified().await;
                adapter.stage_reload(refused_config).await?;
                let applied = adapter.apply_staged().await?;
                if !applied.delta.rejected_boundaries.is_empty() || applied.pending_count != 1 {
                    return Err("reload was not admitted as one pending replacement".to_string());
                }
                if !adapter.wait_connect_results_delivered(BOUND).await {
                    return Err("replacement did not deliver its setup result".to_string());
                }
                Ok::<(), String>(())
            })
            .await;
            // Release even on a failed precondition so the run and its owner
            // can terminate before assertions; no sleep or detached controller.
            client.release.notify_one();
            outcome
        };
        let (result, reloaded) = tokio::join!(
            tokio::time::timeout(
                BOUND + BOUND,
                agent.run("Use the available sibling".to_string().into())
            ),
            reload,
        );
        let repeated = adapter.poll_external_updates().await;
        tokio::time::timeout(BOUND, adapter.shutdown())
            .await
            .unwrap();

        assert!(
            !ready_tools.is_empty(),
            "real MCP initialize and tools/list completed"
        );
        assert!(ready_pending.is_empty());
        assert!(server.request_paths().iter().any(|path| path == "/public"));
        assert_eq!(calls_before_run, 0);
        assert_eq!(notices_before_run, 0);
        reloaded.unwrap().unwrap();
        assert_eq!(result.unwrap().unwrap().text, "continued");
        assert_eq!(sibling.0.load(Ordering::SeqCst), 1);
        let requests = client.recording.0.lock().unwrap().clone();
        assert_eq!(
            requests.len(),
            2,
            "refused replacement permits the next model request"
        );
        assert_eq!(
            failure_notices(&requests[0]),
            0,
            "reload happens after this request was prepared"
        );
        assert_safe_failed_notice(&requests[1], ToolConfigChangeOperation::Reload, &root);
        assert_safe_failed_notice(
            agent.session().messages(),
            ToolConfigChangeOperation::Reload,
            &root,
        );
        assert_successful_sibling_result(&requests[1]);
        assert_successful_sibling_result(agent.session().messages());
        assert_one_failure_event_and_completed_run(&mut events, ToolConfigChangeOperation::Reload);
        assert!(repeated.notices.is_empty());
        assert!(repeated.pending.is_empty());
        assert!(!root.join("private-launch-marker").exists());
    }
}
