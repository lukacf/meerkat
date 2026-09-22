use mdm_tux::{
    rpc_client::{RpcClient, turn_params},
    runtime::ManagedRpcHost,
};
use meerkat::{AgentBuildConfig, SystemPromptOverride};
use meerkat_comms::ResolvedCommsConfig;
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::mpsc};

mod test_support {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/test_support.rs"));
}

fn comms_config(root: &Path) -> ResolvedCommsConfig {
    ResolvedCommsConfig {
        enabled: true,
        name: "synthetic".into(),
        inproc_namespace: Some(root.display().to_string()),
        listen_tcp: None,
        listen_uds: None,
        advertise_address: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        identity_dir: root.join("identity"),
        trusted_peers_path: root.join("peers.json"),
        comms_config: Default::default(),
        auth: Default::default(),
        require_peer_auth: true,
        allow_external_unauthenticated: false,
        pairing_password: None,
    }
}

fn build() -> AgentBuildConfig {
    let mut build = AgentBuildConfig::new("gpt-5.5");
    build.provider = Some(meerkat_core::Provider::OpenAI);
    build.system_prompt =
        SystemPromptOverride::Set("Synthetic managed system instructions.".into());
    build
}

async fn isolated_runtime(
    root: &Path,
    capacity: usize,
) -> Arc<meerkat_rpc::session_runtime::SessionRuntime> {
    let (manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "mdm",
        Some(meerkat_store::RealmBackend::Sqlite),
        None,
    )
    .await
    .unwrap();
    let factory = meerkat::AgentFactory::new(persistence.store_path().unwrap());
    let runtime = Arc::new(meerkat_rpc::session_runtime::SessionRuntime::new(
        factory,
        Default::default(),
        capacity,
        persistence,
        meerkat_rpc::router::NotificationSink::noop(),
    ));
    runtime.set_realm_context(
        Some(manifest.realm),
        None,
        Some(manifest.backend.as_str().into()),
    );
    runtime.set_default_llm_client(Some(Arc::new(meerkat_client::TestClient::for_provider(
        meerkat_core::Provider::OpenAI,
    ))));
    runtime
}

async fn client(
    host: &ManagedRpcHost,
) -> (
    RpcClient,
    mpsc::UnboundedReceiver<serde_json::Value>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let runtime = host.runtime.clone();
    let config = host.config_store.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        meerkat_rpc::serve_tcp_connection(stream, runtime, config, None)
            .await
            .unwrap();
    });
    let (tx, rx) = mpsc::unbounded_channel();
    let mut client = RpcClient::connect(&addr.to_string(), tx).await.unwrap();
    client.set_request_timeout(Duration::from_secs(15));
    client.request("initialize", json!({})).await.unwrap();
    (client, rx, server)
}

#[test]
fn managed_runtime_rpc_stream_and_restart_share_durable_authority() {
    meerkat_runtime::host_stack::run_host("mdm-runtime-test", || async {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let mut first_id = None;
        let mut blob = None;
        let mut schedule = None;
        for epoch in 0..2 {
            let host =
                ManagedRpcHost::open(root.path(), Default::default(), comms_config(root.path()))
                    .await
                    .unwrap();
            let capture = Arc::new(test_support::CaptureClient::default());
            host.runtime.set_default_llm_client(Some(capture.clone()));
            let id = host
                .managed_session(
                    "synthetic",
                    build(),
                    vec![meerkat_core::mcp_config::McpServerConfig::stdio(
                        "local-fixture",
                        "python3",
                        vec![format!(
                            "{}/tests/fixture_mcp.py",
                            env!("CARGO_MANIFEST_DIR")
                        )],
                        Default::default(),
                    )],
                )
                .await
                .unwrap();
            if let Some(first) = &first_id {
                assert_eq!(&id, first);
            } else {
                first_id = Some(id.clone());
            }
            if epoch == 0 {
                blob = Some(
                    host.runtime
                        .blob_store()
                        .put_image("image/png", "c3ludGhldGlj")
                        .await
                        .unwrap(),
                );
                schedule = Some(
                    host.runtime
                        .schedule_service()
                        .create(meerkat::CreateScheduleRequest {
                            name: Some("durable synthetic schedule".into()),
                            description: None,
                            trigger: meerkat::TriggerSpec::Once {
                                due_at_utc: chrono::Utc::now() + chrono::Duration::hours(1),
                            },
                            target: meerkat::TargetBinding::session(
                                meerkat::SessionTargetBinding::ExactSession {
                                    session_id: id.clone(),
                                    action: meerkat::ScheduledSessionAction::Prompt {
                                        prompt: "Synthetic scheduled prompt".into(),
                                        system_prompt: None,
                                        render_metadata: None,
                                        skill_refs: Vec::new(),
                                        additional_instructions: Vec::new(),
                                    },
                                },
                            ),
                            misfire_policy: meerkat::MisfirePolicy::Skip,
                            overlap_policy: meerkat::OverlapPolicy::SkipIfRunning,
                            missing_target_policy: meerkat::MissingTargetPolicy::MarkMisfired,
                            labels: Default::default(),
                            planning_horizon_days: Some(1),
                            planning_horizon_occurrences: Some(1),
                        })
                        .await
                        .unwrap(),
                );
            } else {
                assert_eq!(
                    host.runtime
                        .blob_store()
                        .get(&blob.as_ref().unwrap().blob_id)
                        .await
                        .unwrap()
                        .data,
                    "c3ludGhldGlj",
                );
                assert_eq!(
                    host.runtime
                        .schedule_service()
                        .get(&schedule.as_ref().unwrap().schedule_id)
                        .await
                        .unwrap()
                        .target,
                    schedule.as_ref().unwrap().target,
                );
            }

            let (client, mut notifications, server) = client(&host).await;
            let list = client.request("session/list", json!({})).await.unwrap();
            assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
            assert_eq!(list["sessions"][0]["session_id"], id.to_string());
            let binding = client.bind_session(&id.to_string()).await.unwrap();
            client
                .request(
                    "turn/start",
                    turn_params(&id.to_string(), &format!("Synthetic epoch {epoch}")),
                )
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    let notification = notifications.recv().await.unwrap();
                    if let Some((received, params)) =
                        client.stream_notification(&notification).await
                    {
                        assert_eq!(received, binding);
                        if params["event"]["payload"]["type"] == "turn_completed" {
                            break;
                        }
                    }
                }
            })
            .await
            .expect("scoped terminal event");
            let tools = capture.tool_names();
            assert!(
                capture
                    .user_messages()
                    .iter()
                    .any(|message| message == &format!("Synthetic epoch {epoch}"))
            );
            for name in [
                "shell",
                "datetime",
                "send_message",
                "delegate",
                "meerkat_schedule_create",
            ] {
                assert!(
                    tools.iter().any(|tool| tool == name),
                    "missing {name}: {tools:?}"
                );
            }
            assert!(
                tools.iter().any(|tool| tool.contains("synthetic_probe")),
                "{tools:?}"
            );
            let repeated = client.bind_session(&id.to_string()).await.unwrap();
            assert_ne!(binding.stream_id, repeated.stream_id);
            assert!(
                client
                    .stream_notification(&json!({
                        "method": "session/stream_event",
                        "params": {"session_id": id, "stream_id": binding.stream_id, "event": {}}
                    }))
                    .await
                    .is_none()
            );
            let persisted = host
                .runtime
                .load_persisted_session(&id)
                .await
                .unwrap()
                .unwrap();
            let json = serde_json::to_string(&persisted).unwrap();
            assert!(json.contains("Synthetic managed system instructions."));
            assert!(json.contains("Synthetic epoch 0"));
            assert!(json.contains(&format!("Synthetic epoch {epoch}")));
            let second = host
                .runtime
                .create_session(build(), None, None, Vec::new())
                .await
                .unwrap();
            let replacement = client.bind_session(&second.to_string()).await.unwrap();
            assert_ne!(binding.stream_id, replacement.stream_id);
            assert!(
                client
                    .stream_notification(&json!({
                        "method": "session/stream_event",
                        "params": { "session_id": id, "stream_id": binding.stream_id, "event": {} }
                    }))
                    .await
                    .is_none()
            );
            assert!(client.stream_notification(&json!({
                "method": "session/event",
                "params": { "session_id": second, "stream_id": replacement.stream_id, "event": {} }
            })).await.is_none());
            host.runtime.archive_session(&second).await.unwrap();
            client.close().await;
            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap();
            let (reconnected, _, server) = self::client(&host).await;
            let rebound = reconnected.bind_session(&id.to_string()).await.unwrap();
            assert_ne!(rebound.connection_id, binding.connection_id);
            assert!(
                reconnected
                    .stream_notification(&json!({
                        "method": "session/stream_event",
                        "params": {"session_id": id, "stream_id": binding.stream_id, "event": {}}
                    }))
                    .await
                    .is_none()
            );
            if epoch == 1 {
                host.runtime
                    .schedule_service()
                    .update(
                        &schedule.as_ref().unwrap().schedule_id,
                        meerkat::UpdateScheduleRequest {
                            trigger: Some(meerkat::TriggerSpec::Once {
                                due_at_utc: chrono::Utc::now() + chrono::Duration::seconds(1),
                            }),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        let history = reconnected
                            .request("session/history", json!({"session_id": id}))
                            .await
                            .unwrap();
                        if history.to_string().contains("Synthetic scheduled prompt") {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                })
                .await
                .expect("persisted schedule resolves its target after full reopen");
            }
            reconnected.close().await;
            server.await.unwrap();
            host.shutdown().await.unwrap();
        }
    })
    .unwrap();
}

struct GatedClient {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    blocked: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl meerkat::LlmClient for GatedClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        use futures::StreamExt;
        Box::pin(
            futures::stream::once(async move {
                self.entered.notify_one();
                if self.blocked.load(std::sync::atomic::Ordering::Acquire) {
                    self.release.notified().await;
                }
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Synthetic gate".into(),
                    meta: None,
                })
            })
            .chain(futures::stream::once(async {
                Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                })
            })),
        )
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

#[test]
fn real_rpc_turn_start_uses_owner_admission_when_a_turn_is_active() {
    meerkat_runtime::host_stack::run_host("mdm-busy-test", || async {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let host = ManagedRpcHost::open(root.path(), Default::default(), comms_config(root.path()))
            .await
            .unwrap();
        let gate = Arc::new(GatedClient {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            blocked: std::sync::atomic::AtomicBool::new(true),
        });
        host.runtime.set_default_llm_client(Some(gate.clone()));
        let id = host
            .managed_session("busy", build(), Vec::new())
            .await
            .unwrap();
        let (client, _, server) = client(&host).await;
        let client = Arc::new(client);
        let first_client = client.clone();
        let first_id = id.clone();
        let first = tokio::spawn(async move {
            first_client
                .request(
                    "turn/start",
                    turn_params(&first_id.to_string(), "Synthetic first"),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .unwrap();
        let second_client = client.clone();
        let second_id = id.clone();
        let second = tokio::spawn(async move {
            second_client
                .request(
                    "turn/start",
                    turn_params(&second_id.to_string(), "Synthetic second"),
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !second.is_finished(),
            "default RPC admission queues behind the active turn"
        );
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            host.runtime.create_or_resume_session_without_turn(
                build(),
                Some(id.clone()),
                None,
                Default::default(),
            ),
        )
        .await
        .expect("busy preflight must not wait on the active turn boundary")
        .unwrap_err();
        assert_eq!(error.code, meerkat_rpc::error::SESSION_BUSY);
        gate.blocked
            .store(false, std::sync::atomic::Ordering::Release);
        gate.release.notify_one();
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        let history = client
            .request("session/history", json!({"session_id": id}))
            .await
            .unwrap();
        assert!(history.to_string().contains("Synthetic first"));
        assert!(history.to_string().contains("Synthetic second"));
        Arc::try_unwrap(client).ok().unwrap().close().await;
        server.await.unwrap();
        host.shutdown().await.unwrap();
    })
    .unwrap();
}

#[test]
fn deferred_seam_refuses_missing_archived_and_busy() {
    meerkat_runtime::host_stack::run_host("mdm-deferred-test", || async {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let host = ManagedRpcHost::open(root.path(), Default::default(), comms_config(root.path()))
            .await
            .unwrap();
        host.runtime.set_default_llm_client(Some(Arc::new(
            meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
        )));
        let missing = meerkat_core::types::SessionId::new();
        let error = host
            .runtime
            .create_or_resume_session_without_turn(build(), Some(missing), None, Default::default())
            .await
            .unwrap_err();
        assert_eq!(error.code, meerkat_rpc::error::SESSION_NOT_FOUND);
        let id = host
            .runtime
            .create_or_resume_session_without_turn(build(), None, None, Default::default())
            .await
            .unwrap();
        let error = host
            .runtime
            .create_or_resume_session_without_turn(
                build(),
                Some(id.clone()),
                None,
                Default::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, meerkat_rpc::error::SESSION_BUSY);
        let staged = host
            .runtime
            .create_session(build(), None, None, Vec::new())
            .await
            .unwrap();
        let error = host
            .runtime
            .create_or_resume_session_without_turn(
                build(),
                Some(staged.clone()),
                None,
                Default::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, meerkat_rpc::error::SESSION_BUSY);
        host.runtime.archive_session(&staged).await.unwrap();
        host.runtime.archive_session(&id).await.unwrap();
        let error = host
            .runtime
            .create_or_resume_session_without_turn(
                build(),
                Some(id.clone()),
                None,
                Default::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            meerkat_rpc::error::SESSION_NOT_FOUND,
            "archived identities must not be revived: {error:?}"
        );

        host.shutdown().await.unwrap();
    })
    .unwrap();
}

#[test]
fn failed_no_turn_creation_releases_anonymous_staging_and_capacity() {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let path = root.path().to_path_buf();
    meerkat_runtime::host_stack::run_host("mdm-rollback-test", move || async move {
        let runtime = isolated_runtime(&path, 1).await;
        let mut invalid = build();
        invalid.model_fallback = Some(meerkat_core::config::ModelFallbackConfig {
            enabled: Some(true),
            ..Default::default()
        });
        let failure = runtime
            .create_or_resume_session_without_turn(invalid, None, None, Default::default())
            .await
            .unwrap_err();
        assert!(
            failure.message.contains("nonempty explicit chain"),
            "{failure:?}"
        );
        let sessions = runtime.list_sessions(Default::default()).await.unwrap();
        let retry = runtime
            .create_or_resume_session_without_turn(build(), None, None, Default::default())
            .await;
        runtime.try_shutdown().await.unwrap();
        assert!(
            sessions.is_empty() && retry.is_ok(),
            "failed API call returned no id; retained: {sessions:?}; capacity-one retry: {retry:?}"
        );
    })
    .unwrap();
}

#[test]
fn cold_no_turn_resume_preserves_durable_build_state_without_resupply() {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let path = root.path().to_path_buf();
    meerkat_runtime::host_stack::run_host("mdm-build-state-test", move || async move {
        let runtime = isolated_runtime(&path, 1).await;
        let mut initial = build();
        initial.app_context = Some(json!({"durable_marker": "synthetic"}));
        initial.hooks_override.disable = vec![meerkat_core::HookId::new("synthetic-disabled")];
        initial.output_schema = Some(
            meerkat_core::OutputSchema::new(json!({
                "type": "object",
                "properties": {"ok": {"type": "boolean"}},
                "required": ["ok"],
                "additionalProperties": false
            }))
            .unwrap(),
        );
        initial.budget_limits = Some(meerkat_core::BudgetLimits {
            max_tokens: Some(1234),
            max_tool_calls: Some(7),
            ..Default::default()
        });
        initial.additional_instructions = Some(vec!["Synthetic retained instruction".into()]);
        initial.shell_env = Some(
            [("SYNTHETIC_STATE".into(), "retained".into())]
                .into_iter()
                .collect(),
        );
        initial.call_timeout_override =
            meerkat_core::CallTimeoutOverride::Value(Duration::from_secs(31));
        initial.llm_client_override = Some(Arc::new(meerkat_client::TestClient::new(vec![
            meerkat_client::LlmEvent::TextDelta {
                delta: "{\"ok\":true}".into(),
                meta: None,
            },
            meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            },
        ])));
        let id = runtime
            .create_or_resume_session_without_turn(initial, None, None, Default::default())
            .await
            .unwrap();
        let (events, _rx) = mpsc::channel(128);
        runtime
            .start_turn_via_runtime(
                &id,
                "Synthetic history seed".into(),
                Vec::new(),
                events,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        let session = runtime.load_persisted_session(&id).await.unwrap().unwrap();
        let user_index = session
            .messages()
            .iter()
            .position(|message| {
                matches!(message, meerkat_core::Message::User(user)
                if user.text_content() == "Synthetic history seed")
            })
            .unwrap();
        let rewrite = runtime
            .rewrite_session_transcript(
                &id,
                meerkat_core::service::SessionTranscriptRewriteRequest {
                    selection: meerkat_core::TranscriptRewriteSelection::MessageRange {
                        start: user_index,
                        end: user_index + 1,
                    },
                    replacement: vec![meerkat_core::Message::User(
                        meerkat_core::types::UserMessage::text("Synthetic revised history"),
                    )],
                    reason: meerkat_core::TranscriptRewriteReason::new("synthetic-history-edit"),
                    actor: Some("synthetic-editor".into()),
                    expected_parent_revision: None,
                    running_behavior: Default::default(),
                },
            )
            .await
            .unwrap();
        let revisions_before = serde_json::to_value(
            runtime
                .list_session_transcript_revisions_rich(&id, Default::default())
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(revisions_before.to_string().contains(&rewrite.revision));
        let historical_before = serde_json::to_value(
            runtime
                .read_session_transcript_revision_rich(
                    &id,
                    meerkat_core::service::SessionTranscriptRevisionQuery {
                        revision: rewrite.parent_revision.clone(),
                        offset: 0,
                        limit: None,
                    },
                )
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(
            historical_before
                .to_string()
                .contains("Synthetic history seed")
        );
        let before = runtime.load_persisted_session(&id).await.unwrap().unwrap();
        let messages_before = serde_json::to_value(before.messages()).unwrap();
        assert!(
            messages_before
                .to_string()
                .contains("Synthetic revised history")
        );
        let build_before =
            serde_json::to_value(before.try_build_state().unwrap().unwrap()).unwrap();
        runtime.try_shutdown().await.unwrap();
        drop(runtime);

        let reopened = isolated_runtime(&path, 1).await;
        let resumed = reopened
            .create_or_resume_session_without_turn(
                build(),
                Some(id.clone()),
                None,
                Default::default(),
            )
            .await
            .unwrap();
        assert_eq!(resumed, id);
        let after = reopened.load_persisted_session(&id).await.unwrap().unwrap();
        let messages_after = serde_json::to_value(after.messages()).unwrap();
        let build_after = serde_json::to_value(after.try_build_state().unwrap().unwrap()).unwrap();
        let revisions_after = serde_json::to_value(
            reopened
                .list_session_transcript_revisions_rich(&id, Default::default())
                .await
                .unwrap(),
        )
        .unwrap();
        let historical_after = serde_json::to_value(
            reopened
                .read_session_transcript_revision_rich(
                    &id,
                    meerkat_core::service::SessionTranscriptRevisionQuery {
                        revision: rewrite.parent_revision,
                        offset: 0,
                        limit: None,
                    },
                )
                .await
                .unwrap(),
        )
        .unwrap();
        reopened.try_shutdown().await.unwrap();
        assert_eq!(
            build_after, build_before,
            "cold resume must inherit the canonical SessionBuildState"
        );
        assert_eq!(messages_after, messages_before);
        assert_eq!(revisions_after, revisions_before);
        assert_eq!(historical_after, historical_before);
    })
    .unwrap();
}

#[test]
fn cold_no_turn_resume_obeys_recovery_override_admission() {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let path = root.path().to_path_buf();
    meerkat_runtime::host_stack::run_host("mdm-resume-admission-test", move || async move {
        let runtime = isolated_runtime(&path, 1).await;
        let id = runtime
            .create_or_resume_session_without_turn(build(), None, None, Default::default())
            .await
            .unwrap();
        let (events, _rx) = mpsc::channel(128);
        let completed = runtime
            .start_turn_via_runtime(
                &id,
                "Synthetic completed first turn".into(),
                Vec::new(),
                events,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(completed.text, "ok");
        let session = runtime.load_persisted_session(&id).await.unwrap().unwrap();
        runtime.try_shutdown().await.unwrap();
        drop(runtime);
        let overrides = meerkat_core::SurfaceSessionRecoveryOverrides {
            provider: Some(meerkat_core::Provider::Anthropic),
            ..Default::default()
        };
        let owner_verdict = meerkat_core::build_recovered_session(
            session,
            &overrides,
            meerkat_core::SurfaceSessionRecoveryContext::default(),
        );
        assert!(
            owner_verdict.is_err(),
            "canonical recovery must reject provider override without model"
        );
        let reopened = isolated_runtime(&path, 1).await;
        let result = reopened
            .create_or_resume_session_without_turn(build(), Some(id), None, overrides)
            .await;
        reopened.try_shutdown().await.unwrap();
        let error = result.expect_err("no-turn resume bypassed canonical override admission");
        assert_eq!(error.code, meerkat_rpc::error::INVALID_PARAMS, "{error:?}");
    })
    .unwrap();
}

#[test]
fn managed_peer_ingress_and_schedule_use_the_rpc_session_owner() {
    meerkat_runtime::host_stack::run_host("mdm-ingress-test", || async {
        use meerkat_core::agent::CommsRuntime as _;
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let mut hosts = Vec::new();
        let mut ids = Vec::new();
        let namespace = uuid::Uuid::new_v4().to_string();
        for name in ["managed-receiver", "managed-sender"] {
            let path = root.path().join(name);
            let mut config = comms_config(&path);
            config.name = name.into();
            config.inproc_namespace = Some(namespace.clone());
            let host = ManagedRpcHost::open(&path, Default::default(), config)
                .await
                .unwrap();
            host.runtime.set_default_llm_client(Some(Arc::new(
                meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            )));
            let id = host
                .managed_session(name, build(), Vec::new())
                .await
                .unwrap();
            ids.push(id);
            hosts.push(host);
        }
        for (owner, peer) in [(0, 1), (1, 0)] {
            let trust = mdm_tux::ExampleGeneratedCommsTrustRouter::new(
                hosts[owner].runtime.runtime_adapter(),
                ids[owner].clone(),
                hosts[owner].comms.clone(),
            );
            trust
                .add_trusted_peer(
                    if peer == 0 {
                        "managed-receiver"
                    } else {
                        "managed-sender"
                    },
                    hosts[peer].comms.public_key(),
                    &hosts[peer].comms.advertised_address(),
                )
                .await
                .unwrap();
            assert_eq!(
                hosts[owner]
                    .runtime
                    .runtime_adapter()
                    .direct_peer_endpoints(&ids[owner])
                    .await
                    .unwrap()
                    .len(),
                1
            );
        }
        let (client, _, server) = client(&hosts[0]).await;
        hosts[1]
            .comms
            .send(meerkat_core::comms::CommsCommand::PeerMessage {
                to: meerkat_core::comms::PeerRoute::new(hosts[0].comms.public_key().to_peer_id()),
                body: "Synthetic peer ingress".into(),
                blocks: None,
                content_taint: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                objective_id: None,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let session = client
                    .request("session/history", json!({"session_id": ids[0]}))
                    .await
                    .expect("peer history observation must succeed");
                if session.to_string().contains("Synthetic peer ingress") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("peer ingress committed to RPC-owned session");
        let schedule = hosts[0]
            .runtime
            .schedule_service()
            .create(meerkat::CreateScheduleRequest {
                name: None,
                description: None,
                trigger: meerkat::TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() + chrono::Duration::seconds(1),
                },
                target: meerkat::TargetBinding::session(
                    meerkat::SessionTargetBinding::ExactSession {
                        session_id: ids[0].clone(),
                        action: meerkat::ScheduledSessionAction::Prompt {
                            prompt: "Synthetic scheduled delivery".into(),
                            system_prompt: None,
                            render_metadata: None,
                            skill_refs: Vec::new(),
                            additional_instructions: Vec::new(),
                        },
                    },
                ),
                misfire_policy: meerkat::MisfirePolicy::Skip,
                overlap_policy: meerkat::OverlapPolicy::SkipIfRunning,
                missing_target_policy: meerkat::MissingTargetPolicy::MarkMisfired,
                labels: Default::default(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let session = client
                    .request("session/history", json!({"session_id": ids[0]}))
                    .await
                    .unwrap();
                if session.to_string().contains("Synthetic scheduled delivery") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("scheduled delivery visible through same RPC owner");
        assert_eq!(
            hosts[0]
                .runtime
                .schedule_service()
                .get(&schedule.schedule_id)
                .await
                .unwrap()
                .schedule_id,
            schedule.schedule_id
        );
        client.close().await;
        server.await.unwrap();
        for host in hosts {
            host.shutdown().await.unwrap();
        }
    })
    .unwrap();
}

#[tokio::test]
async fn actual_target_boot_and_restart_advertise_the_same_rpc_session() {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    std::fs::create_dir(root.path().join(".rkat")).unwrap();
    std::fs::write(root.path().join(".rkat/config.toml"), "").unwrap();
    let broker = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut first_id = None;
    for _ in 0..2 {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let rpc_addr = probe.local_addr().unwrap();
        drop(probe);
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mdm-target"))
            .args([
                "--name",
                "synthetic-target",
                "--model",
                "gpt-5.5",
                "--provider",
                "openai",
                "--advertise",
                "127.0.0.1",
                "--rpc-port",
            ])
            .arg(rpc_addr.port().to_string())
            .arg("--kennel")
            .arg(broker.local_addr().unwrap().to_string())
            .arg("--data-dir")
            .arg(root.path())
            .env("HOME", root.path())
            .env("OPENAI_API_KEY", "synthetic-never-called")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let client = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(client) = RpcClient::connect(&rpc_addr.to_string(), tx.clone()).await {
                    break client;
                }
                if let Some(status) = child.try_wait().unwrap() {
                    use tokio::io::AsyncReadExt;
                    let mut stderr = String::new();
                    child
                        .stderr
                        .take()
                        .unwrap()
                        .read_to_string(&mut stderr)
                        .await
                        .unwrap();
                    panic!("target startup {status}: {stderr}");
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("actual target RPC startup");
        client.request("initialize", json!({})).await.unwrap();
        let sessions = client.request("session/list", json!({})).await.unwrap();
        assert_eq!(sessions["sessions"].as_array().unwrap().len(), 1);
        let id = sessions["sessions"][0]["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        if let Some(first) = &first_id {
            assert_eq!(&id, first);
        } else {
            first_id = Some(id.clone());
        }
        client.bind_session(&id).await.unwrap();
        client.close().await;
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }
}
