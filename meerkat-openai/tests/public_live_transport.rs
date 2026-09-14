#![cfg(all(not(target_arch = "wasm32"), feature = "live"))]

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use meerkat_core::live_execution::profile::{LiveClientRequestPolicy, LiveProfileId};
use meerkat_core::{
    AuthBindingRef, Config, ModelRegistry, Provider, RealmConnectionSet, SessionLlmIdentity,
};
use meerkat_llm_core::provider_runtime::{
    ProviderRuntimeRegistry, ResolvedLiveExecution, ResolvedLiveTarget, ResolverEnvironment,
};
use meerkat_openai::public_live::config::PublicLiveVoiceSettings;
use meerkat_openai::public_live::session::{
    OpenAiPublicLiveSessionFactory, PublicLiveSessionError, PublicLiveTransportLimits,
};
use oai_rt_rs::live::{ClientEvent, CloseReason, Command, ServerEvent};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn identity_frame(kind: &str, id: &str) -> Value {
    let mut frame = json!({
        "type": kind, "event_id": "identity-fixture",
        "session": {"id": id, "model": "gpt-live-1", "status": "active", "expires_at": 4102444800.0}
    });
    if kind == "session.closed" {
        frame["reason"] = json!(CloseReason::CloseRequested);
        frame["usage"] = json!({"seconds": 1.25});
    }
    frame
}

async fn scripted_adapter(
    server: &Server,
    webrtc: bool,
) -> TestResult<Arc<dyn meerkat_core::live_adapter::LiveAdapter>> {
    use meerkat_llm_core::live_adapter_factory::{ContinuousLiveOpenConfig, LiveAdapterOpenConfig};
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_openai::runtime::OpenAiProviderRuntime));
    let factory = registry.build_live_adapter_factory(target(&server.root, false).await?)?;
    let config = LiveAdapterOpenConfig::Continuous(ContinuousLiveOpenConfig {
        channel_id: meerkat_core::live_execution::LiveChannelId::new("identity"),
        voice: None,
        instructions: None,
        limits: PublicLiveTransportLimits {
            max_event_bytes: 128 * 1024,
            event_capacity: 8,
            command_capacity: 4,
            io_timeout: Duration::from_secs(2),
        },
    });
    if webrtc {
        factory
            .prepare_webrtc(&config, "v=0\r\noffer")
            .await?
            .attach_adapter()
            .await
            .map_err(Into::into)
    } else {
        factory.open_adapter(&config).await.map_err(Into::into)
    }
}

#[tokio::test]
async fn public_adapter_rejects_foreign_lifecycle_without_readiness_final_usage_or_alien_close()
-> TestResult {
    use meerkat_core::live_adapter::{
        LiveAdapterCommand, LiveAdapterObservation, LiveAdapterStatus,
    };
    use meerkat_core::live_execution::observation::ContinuousLiveObservation;
    let started = |id| identity_frame("session.started", id);
    let closed = |id| identity_frame("session.closed", id);
    for (webrtc, accepted_start, frames) in [
        (
            true,
            false,
            vec![started("alien"), started("provider-session")],
        ),
        (true, false, vec![started(""), started("provider-session")]),
        (true, false, vec![closed("provider-session")]),
        (
            true,
            true,
            vec![started("provider-session"), closed("alien")],
        ),
        (
            true,
            true,
            vec![
                started("provider-session"),
                identity_frame("session.updated", "alien"),
            ],
        ),
        (false, true, vec![started("ws-owned"), closed("alien")]),
        (
            false,
            true,
            vec![started("ws-owned"), started("alien"), started("ws-owned")],
        ),
    ] {
        let server = Server::with_state(ServerState {
            script: Some(Arc::new(frames)),
            ..Default::default()
        })
        .await?;
        let adapter = scripted_adapter(&server, webrtc).await?;
        if accepted_start {
            let expected = if webrtc {
                "provider-session"
            } else {
                "ws-owned"
            };
            assert!(matches!(
                adapter.next_observation().await?,
                Some(LiveAdapterObservation::Continuous {
                    event: ContinuousLiveObservation::ProviderStarted { provider_session },
                    ..
                }) if provider_session.as_str() == expected
            ));
        }
        let error = adapter
            .next_observation()
            .await
            .err()
            .ok_or("identity accepted")?;
        let rendered = format!("{error:?}");
        for private in ["alien", "provider-session", "ws-owned"] {
            assert!(!rendered.contains(private));
        }
        assert!(matches!(
            adapter.status(),
            LiveAdapterStatus::Degraded { .. }
        ));
        assert!(
            adapter
                .send_command(LiveAdapterCommand::Close)
                .await
                .is_err()
        );
        assert!(adapter.close().await.is_err());
        server.state.finish_script.notify_one();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), adapter.next_observation()).await??,
            Some(LiveAdapterObservation::Continuous {
                event: ContinuousLiveObservation::ObservationStreamEnded,
                ..
            })
        ));
        assert!(adapter.next_observation().await?.is_none());
        assert!(matches!(
            adapter.status(),
            LiveAdapterStatus::Degraded { .. }
        ));
        assert!(server.state.commands.lock().await.is_empty());
        assert!(server.state.errors.lock().await.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn public_websocket_binds_actual_identity_before_accepting_final_usage() -> TestResult {
    use meerkat_core::live_adapter::{LiveAdapterObservation, LiveAdapterStatus};
    use meerkat_core::live_execution::observation::{ContinuousLiveObservation, LiveUsageSnapshot};
    let server = Server::with_state(ServerState {
        script: Some(Arc::new(vec![
            identity_frame("session.started", "ws-owned"),
            identity_frame("session.started", "ws-owned"),
            identity_frame("session.closed", "ws-owned"),
        ])),
        ..Default::default()
    })
    .await?;
    let adapter = scripted_adapter(&server, false).await?;
    assert_ne!(adapter.status(), LiveAdapterStatus::Closed);
    assert!(matches!(
        adapter.next_observation().await?,
        Some(LiveAdapterObservation::Continuous {
            event: ContinuousLiveObservation::ProviderStarted { provider_session },
            ..
        }) if provider_session.as_str() == "ws-owned"
    ));
    assert!(matches!(
        adapter.next_observation().await?,
        Some(LiveAdapterObservation::Continuous {
            event: ContinuousLiveObservation::ProviderClosed {
                usage: LiveUsageSnapshot::SessionClosed { .. },
            },
            ..
        })
    ));
    assert_eq!(adapter.status(), LiveAdapterStatus::Closed);
    adapter.close().await?;
    server.state.finish_script.notify_one();
    assert!(server.state.commands.lock().await.is_empty());
    Ok(())
}

struct Capture {
    path: String,
    headers: HeaderMap,
    body: Value,
}

#[derive(Clone, Default)]
struct ServerState {
    captures: Arc<Mutex<Vec<Capture>>>,
    errors: Arc<Mutex<Vec<String>>>,
    advisories: bool,
    attach_failures: Arc<AtomicUsize>,
    script: Option<Arc<Vec<Value>>>,
    commands: Arc<Mutex<Vec<Value>>>,
    finish_script: Arc<tokio::sync::Notify>,
}

struct Server {
    root: String,
    state: ServerState,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn start() -> TestResult<Self> {
        Self::with_advisories(false).await
    }

    async fn with_advisories(advisories: bool) -> TestResult<Self> {
        Self::with_state(ServerState {
            advisories,
            ..Default::default()
        })
        .await
    }

    async fn with_state(state: ServerState) -> TestResult<Self> {
        let router = Router::new()
            .route("/v1/live/sessions", get(websocket).post(create))
            .route("/v1/live/sessions/provider-session/attach", get(websocket))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let root = format!("http://{}/v1", listener.local_addr()?);
        let errors = Arc::clone(&state.errors);
        let task = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, router).await {
                errors.lock().await.push(error.to_string());
            }
        });
        Ok(Self { root, state, task })
    }
}

async fn create(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    state.captures.lock().await.push(Capture {
        path: uri.to_string(),
        headers,
        body,
    });
    (
        StatusCode::CREATED,
        Json(json!({
            "session":{"id":"provider-session"},
            "transport":{"type":"webrtc","sdp":"v=0\r\nanswer"}
        })),
    )
}

async fn websocket(
    upgrade: WebSocketUpgrade,
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    let path = uri.to_string();
    if path.ends_with("/attach")
        && state
            .attach_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
    {
        state.captures.lock().await.push(Capture {
            path,
            headers,
            body: Value::Null,
        });
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    upgrade.on_upgrade(move |socket| async move {
        if let Err(error) = dialog(socket, &state, path, headers).await {
            state.errors.lock().await.push(error.to_string());
        }
    })
}

async fn read_command(socket: &mut WebSocket) -> TestResult<Value> {
    let message = socket.recv().await.ok_or("missing command")??;
    let Message::Text(text) = message else {
        return Err("command is not text".into());
    };
    Ok(serde_json::from_str(text.as_str())?)
}

async fn dialog(
    mut socket: WebSocket,
    state: &ServerState,
    path: String,
    headers: HeaderMap,
) -> TestResult {
    let primary = path == "/v1/live/sessions";
    let body = if primary {
        read_command(&mut socket).await?
    } else {
        Value::Null
    };
    state.captures.lock().await.push(Capture {
        path,
        headers,
        body,
    });
    if let Some(script) = &state.script {
        for frame in script.iter() {
            socket.send(Message::Text(frame.to_string().into())).await?;
        }
        loop {
            tokio::select! {
                () = state.finish_script.notified() => {
                    socket.send(Message::Close(None)).await?;
                    return Ok(());
                }
                command = socket.recv() => match command {
                    Some(Ok(Message::Text(text))) => state.commands.lock().await.push(serde_json::from_str(text.as_str())?),
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(error.into()),
                }
            }
        }
    }
    let session = json!({
        "id":"provider-session", "model":"gpt-live-1", "status":"active", "expires_at":4102444800.0
    });
    for frame in [
        json!({"type":"session.started","event_id":"start","session":session}),
        json!({"type":"session.input_transcript.delta","event_id":"input","delta":"","start_ms":0.25,"end_ms":0.25}),
        json!({"type":"session.output_transcript.delta","event_id":"output","delta":" \n\"exact\"\u{0000} ","start_ms":0.5,"end_ms":1.75}),
    ] {
        socket.send(Message::Text(frame.to_string().into())).await?;
    }
    if state.advisories {
        for frame in [
            json!({"type":"error","event_id":"error","error":{"code":null,"type":"provider","message":"provider-secret-not-for-clients"}}),
            json!({"type":"info","event_id":"info","code":"opaque","message":"provider-secret-not-for-clients"}),
            json!({"type":"session.input_transcript.delta","event_id":"bad-range","delta":"rejected text","start_ms":2.0,"end_ms":1.0}),
        ] {
            socket.send(Message::Text(frame.to_string().into())).await?;
        }
    }
    let command = read_command(&mut socket).await?;
    if command["type"] != "session.close" {
        return Err("expected close, not another start or a Realtime command".into());
    }
    socket
        .send(Message::Text(
            json!({
                "type":"session.closed","event_id":"closed","session":session,
                "reason":CloseReason::CloseRequested,"usage":{"seconds":1.25}
            })
            .to_string()
            .into(),
        ))
        .await?;
    Ok(())
}

async fn target(root: &str, function_mode: bool) -> TestResult<ResolvedLiveTarget> {
    let binding: AuthBindingRef =
        serde_json::from_value(json!({"realm":"voice","binding":"voice-key"}))?;
    let realm: RealmConnectionSet = serde_json::from_value(json!({
        "realm_id":"voice",
        "backends":{"voice-backend":{"id":"voice-backend","provider":"openai","backend_kind":"openai_api","base_url":root}},
        "auth_profiles":{"voice-auth":{"id":"voice-auth","provider":"openai","auth_method":"api_key",
            "source":{"kind":"inline_secret","secret":"public-live-fixture-key"}}},
        "bindings":{"voice-key":{"id":"voice-key","backend_profile":"voice-backend","auth_profile":"voice-auth"}}
    }))?;
    let connection = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_openai::runtime::OpenAiProviderRuntime))
        .resolve_live_connection(&realm, &binding, &ResolverEnvironment::testing())
        .await?;
    let registry = ModelRegistry::from_config(&Config::default(), meerkat_models::canonical())?;
    let execution = if function_mode {
        ResolvedLiveExecution::FunctionBridge {
            backend: registry
                .profile_witness_for_provider(Provider::OpenAI, "gpt-5.5")
                .ok_or("backend")?,
        }
    } else {
        ResolvedLiveExecution::ClientContext {
            request_policy: LiveClientRequestPolicy::SnapshotAtDelegation,
        }
    };
    Ok(ResolvedLiveTarget::new(
        LiveProfileId::parse("voice")?,
        SessionLlmIdentity {
            provider: Provider::OpenAI,
            model: "gpt-live-1".into(),
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(binding),
        },
        registry
            .profile_witness_for_provider(Provider::OpenAI, "gpt-live-1")
            .ok_or("voice")?,
        connection,
        execution,
    )?)
}

#[tokio::test]
async fn registry_public_adapter_emits_internal_continuous_facts_and_rejects_legacy_controls()
-> TestResult {
    use meerkat_core::live_adapter::{
        LiveAdapterCommand, LiveAdapterError, LiveAdapterErrorCode, LiveAdapterObservation,
        LiveInputChunk,
    };
    use meerkat_core::live_execution::frontend::ContinuousLiveInputError;
    use meerkat_core::live_execution::observation::{ContinuousLiveObservation, LiveUsageSnapshot};
    use meerkat_llm_core::live_adapter_factory::{ContinuousLiveOpenConfig, LiveAdapterOpenConfig};
    let server = Server::with_advisories(true).await?;
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_openai::runtime::OpenAiProviderRuntime));
    let factory = registry.build_live_adapter_factory(target(&server.root, false).await?)?;
    let adapter = factory
        .open_adapter(&LiveAdapterOpenConfig::Continuous(
            ContinuousLiveOpenConfig {
                channel_id: meerkat_core::live_execution::LiveChannelId::new("channel"),
                voice: None,
                instructions: None,
                limits: PublicLiveTransportLimits {
                    max_event_bytes: 128 * 1024,
                    event_capacity: 8,
                    command_capacity: 4,
                    io_timeout: Duration::from_secs(2),
                },
            },
        ))
        .await?;
    assert!(matches!(
        adapter.next_observation().await?.ok_or("started")?,
        LiveAdapterObservation::Continuous {
            event: ContinuousLiveObservation::ProviderStarted { .. },
            receive: None
        }
    ));
    let first = adapter.next_observation().await?.ok_or("input")?;
    assert!(
        matches!(&first, LiveAdapterObservation::Continuous { event: ContinuousLiveObservation::Transcript(observation), receive: None }
        if observation.text().is_empty() && observation.direction() == meerkat_core::live_observation::LiveTranscriptDirection::Input)
    );
    assert!(serde_json::to_value(&first).is_err());
    assert!(matches!(adapter.next_observation().await?.ok_or("output")?,
        LiveAdapterObservation::Continuous { event: ContinuousLiveObservation::Transcript(observation), receive: None }
        if observation.text() == " \n\"exact\"\u{0000} " && observation.direction() == meerkat_core::live_observation::LiveTranscriptDirection::Output));
    for _ in 0..2 {
        let diagnostic = adapter.next_observation().await?.ok_or("diagnostic")?;
        assert!(matches!(
            &diagnostic,
            LiveAdapterObservation::Continuous {
                event: ContinuousLiveObservation::Diagnostic(_),
                receive: None
            }
        ));
        assert!(!format!("{diagnostic:?}").contains("provider-secret"));
        assert!(!adapter.status().is_terminal());
    }
    assert!(matches!(
        adapter.next_observation().await?.ok_or("rejected TEXT")?,
        LiveAdapterObservation::Continuous {
            event: ContinuousLiveObservation::TranscriptRejected(
                meerkat_core::live_observation::LiveObservationValueError::InvalidRange
            ),
            receive: None
        }
    ));
    for (command, expected) in [
        (
            LiveAdapterCommand::SendInput {
                chunk: LiveInputChunk::Text {
                    text: "not thinking".into(),
                },
            },
            ContinuousLiveInputError::UnsupportedInputKind,
        ),
        (
            LiveAdapterCommand::Interrupt,
            ContinuousLiveInputError::UnsupportedCapability,
        ),
    ] {
        assert!(matches!(adapter.send_command(command).await,
            Err(LiveAdapterError::ProviderError { code:LiveAdapterErrorCode::ContinuousInputRejected { reason }, .. }) if reason == expected));
    }
    adapter.send_command(LiveAdapterCommand::Close).await?;
    assert!(matches!(
        adapter.next_observation().await?.ok_or("closed")?,
        LiveAdapterObservation::Continuous {
            event: ContinuousLiveObservation::ProviderClosed {
                usage: LiveUsageSnapshot::SessionClosed { .. }
            },
            receive: None
        }
    ));
    adapter.close().await?;
    assert!(matches!(
        adapter.next_observation().await?,
        Some(LiveAdapterObservation::Continuous {
            event: ContinuousLiveObservation::ObservationStreamEnded,
            receive: None,
        })
    ));
    assert!(adapter.next_observation().await?.is_none());
    assert_eq!(server.state.captures.lock().await.len(), 1);
    assert!(server.state.errors.lock().await.is_empty());
    assert!(
        registry
            .build_live_adapter_factory(target(&server.root, true).await?)
            .is_err(),
        "managed function observations must not fall through to unscoped legacy dispatch"
    );
    Ok(())
}

#[tokio::test]
async fn neutral_webrtc_preparation_retains_resource_on_failed_attach_without_claiming_readiness()
-> TestResult {
    use meerkat_core::live_adapter::{
        LiveAdapterCommand, LiveAdapterError, LiveAdapterErrorCode, LiveAdapterObservation,
        LiveAdapterStatus, LiveInputChunk,
    };
    use meerkat_core::live_execution::frontend::ContinuousLiveInputError;
    use meerkat_core::live_execution::observation::ContinuousLiveObservation;
    use meerkat_llm_core::live_adapter_factory::{ContinuousLiveOpenConfig, LiveAdapterOpenConfig};
    let server = Server::start().await?;
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_openai::runtime::OpenAiProviderRuntime));
    let factory = registry.build_live_adapter_factory(target(&server.root, false).await?)?;
    let config = LiveAdapterOpenConfig::Continuous(ContinuousLiveOpenConfig {
        channel_id: meerkat_core::live_execution::LiveChannelId::new("rtc"),
        voice: None,
        instructions: None,
        limits: PublicLiveTransportLimits {
            max_event_bytes: 128 * 1024,
            event_capacity: 8,
            command_capacity: 4,
            io_timeout: Duration::from_secs(2),
        },
    });
    let mut pending = factory.prepare_webrtc(&config, "v=0\r\noffer").await?;
    assert_eq!(pending.answer_sdp(), "v=0\r\nanswer");
    server.state.attach_failures.store(1, Ordering::SeqCst);
    assert!(pending.attach_adapter().await.is_err());
    assert_eq!(pending.answer_sdp(), "v=0\r\nanswer");
    let adapter = pending.attach_adapter().await?;
    assert!(Arc::ptr_eq(&adapter, &pending.attach_adapter().await?));
    assert_eq!(adapter.status(), LiveAdapterStatus::Opening);
    assert!(!adapter.capabilities().audio_in && !adapter.capabilities().audio_out);
    assert!(matches!(
        adapter
            .send_command(LiveAdapterCommand::SendInput {
                chunk: LiveInputChunk::Audio {
                    data: vec![0, 0],
                    sample_rate_hz: 24_000,
                    channels: 1
                },
            })
            .await,
        Err(LiveAdapterError::ProviderError {
            code: LiveAdapterErrorCode::ContinuousInputRejected {
                reason: ContinuousLiveInputError::AudioUsesAnotherTransport
            },
            ..
        })
    ));
    assert!(matches!(
        adapter.next_observation().await?.ok_or("started")?,
        LiveAdapterObservation::Continuous {
            event: ContinuousLiveObservation::ProviderStarted { .. },
            ..
        }
    ));
    for _ in 0..2 {
        assert!(matches!(
            adapter.next_observation().await?.ok_or("text")?,
            LiveAdapterObservation::Continuous {
                event: ContinuousLiveObservation::Transcript(_),
                ..
            }
        ));
    }
    adapter.send_command(LiveAdapterCommand::Close).await?;
    assert!(matches!(
        adapter.next_observation().await?.ok_or("closed")?,
        LiveAdapterObservation::Continuous {
            event: ContinuousLiveObservation::ProviderClosed { .. },
            ..
        }
    ));
    adapter.close().await?;
    let captures = server.state.captures.lock().await;
    assert_eq!(captures.len(), 3);
    assert_eq!(
        captures
            .iter()
            .filter(|capture| capture.body.get("transport").is_some())
            .count(),
        1
    );
    assert!(server.state.errors.lock().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn public_transport_rejects_invalid_bounds_without_opening_a_socket() -> TestResult {
    let server = Server::start().await?;
    let target = target(&server.root, false).await?;
    let limits = PublicLiveTransportLimits {
        max_event_bytes: 128 * 1024,
        event_capacity: 8,
        command_capacity: 4,
        io_timeout: Duration::from_secs(2),
    };
    for invalid in [
        PublicLiveTransportLimits {
            max_event_bytes: 0,
            ..limits
        },
        PublicLiveTransportLimits {
            event_capacity: 0,
            ..limits
        },
        PublicLiveTransportLimits {
            command_capacity: usize::MAX,
            ..limits
        },
        PublicLiveTransportLimits {
            io_timeout: Duration::ZERO,
            ..limits
        },
    ] {
        assert!(matches!(
            OpenAiPublicLiveSessionFactory::new(
                &target,
                PublicLiveVoiceSettings::default(),
                invalid
            ),
            Err(PublicLiveSessionError::InvalidLimits)
        ));
    }
    let factory = OpenAiPublicLiveSessionFactory::new(
        &target,
        PublicLiveVoiceSettings {
            voice: None,
            instructions: Some("private voice guidance"),
        },
        limits,
    )?;
    assert!(!format!("{factory:?}").contains("guidance"));
    assert!(!format!("{factory:?}").contains("fixture-key"));
    assert!(factory.create_webrtc(String::new()).await.is_err());
    assert!(matches!(
        factory.create_webrtc("\0".repeat(128 * 1024 / 5)).await,
        Err(PublicLiveSessionError::RequestTooLarge)
    ));
    assert!(server.state.captures.lock().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn public_factory_uses_live_endpoints_and_keeps_websocket_and_webrtc_shapes_distinct()
-> TestResult {
    for function_mode in [false, true] {
        for webrtc in [false, true] {
            let server = Server::start().await?;
            let target = target(&server.root, function_mode).await?;
            let factory = OpenAiPublicLiveSessionFactory::new(
                &target,
                PublicLiveVoiceSettings {
                    voice: None,
                    instructions: Some("voice guidance only"),
                },
                PublicLiveTransportLimits {
                    max_event_bytes: 128 * 1024,
                    event_capacity: 8,
                    command_capacity: 4,
                    io_timeout: Duration::from_secs(2),
                },
            )?;
            let mut connection = if webrtc {
                let created = factory.create_webrtc("v=0\r\noffer".into()).await?;
                assert_eq!(created.transport.sdp(), "v=0\r\nanswer");
                factory.attach_sideband(&created.session.id).await?
            } else {
                factory.open_websocket().await?
            };
            assert!(matches!(
                connection.next_event().await?.ok_or("started")?.event,
                ServerEvent::Started { .. }
            ));
            assert!(
                matches!(connection.next_event().await?.ok_or("input")?.event,
                ServerEvent::InputTranscriptDelta { delta, start_ms, end_ms, .. }
                    if delta.is_empty() && start_ms.to_bits() == 0.25_f64.to_bits() && end_ms.to_bits() == 0.25_f64.to_bits())
            );
            assert!(
                matches!(connection.next_event().await?.ok_or("output")?.event,
                ServerEvent::OutputTranscriptDelta { delta, start_ms, end_ms, .. }
                    if delta == " \n\"exact\"\u{0000} " && start_ms.to_bits() == 0.5_f64.to_bits() && end_ms.to_bits() == 1.75_f64.to_bits())
            );
            connection.send(ClientEvent::new(Command::Close)).await?;
            let closed = connection.next_event().await?.ok_or("closed")?;
            assert!(
                matches!(closed.event, ServerEvent::Closed { usage, .. } if usage.seconds.to_bits() == 1.25_f64.to_bits())
            );
            let captures = server.state.captures.lock().await;
            assert_eq!(captures.len(), if webrtc { 2 } else { 1 });
            for capture in captures.iter() {
                assert_eq!(
                    capture
                        .headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer public-live-fixture-key")
                );
                assert!(capture.headers.get("openai-beta").is_none());
                assert!(capture.headers.get("chatgpt-account-id").is_none());
                assert!(matches!(
                    capture.path.as_str(),
                    "/v1/live/sessions" | "/v1/live/sessions/provider-session/attach"
                ));
            }
            let startup = &captures[0].body;
            if !webrtc {
                assert_eq!(startup["type"], "session.start");
            }
            let session = &startup["session"];
            assert_eq!(session["model"], "gpt-live-1");
            assert_eq!(session["instructions"], "voice guidance only");
            assert_eq!(session["audio"].get("format").is_none(), webrtc);
            if function_mode {
                assert_eq!(session["delegation"]["responses"]["model"], "gpt-5.5");
                assert_eq!(
                    session["delegation"]["responses"]["tools"][0]["name"],
                    "invoke_meerkat"
                );
            } else {
                assert_eq!(session["delegation"], json!({"type":"client"}));
            }
            assert!(server.state.errors.lock().await.is_empty());
        }
    }
    Ok(())
}
