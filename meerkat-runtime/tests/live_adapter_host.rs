#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_contracts::{
    RealtimeAudioFormat, RealtimeCapabilities, RealtimeChannelTarget, RealtimeInputKind,
    RealtimeOutputKind, RealtimeTurningMode,
};
use meerkat_core::{Provider, SessionLlmIdentity, types::SessionId};
use meerkat_llm_core::LlmError;
use meerkat_llm_core::realtime_adapter::{
    RealtimeAdapter, RealtimeAdapterCommand, RealtimeAdapterContinuity, RealtimeAdapterObservation,
    RealtimeAdapterObservationEnvelope, RealtimeAdapterStatus, RealtimeProjectionSnapshot,
    RealtimeProjectionWatermark,
};
use meerkat_runtime::{
    LiveAdapterFactory, LiveAdapterHost, LiveProjectionBuilder, LiveResolvedTarget,
    LiveTargetResolver,
};
use tokio::sync::Mutex;

#[derive(Default)]
struct FakeResolver {
    sessions: Mutex<VecDeque<SessionId>>,
    resolved: Mutex<Vec<RealtimeChannelTarget>>,
}

#[async_trait]
impl LiveTargetResolver for FakeResolver {
    async fn resolve(
        &self,
        target: &RealtimeChannelTarget,
    ) -> Result<LiveResolvedTarget, meerkat_runtime::LiveAdapterHostError> {
        self.resolved.lock().await.push(target.clone());
        let session_id = self
            .sessions
            .lock()
            .await
            .pop_front()
            .expect("test resolver session");
        Ok(LiveResolvedTarget {
            session_id,
            identity: SessionLlmIdentity {
                model: "gpt-realtime-1.5".to_string(),
                provider: Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
        })
    }
}

struct FakeProjectionBuilder;

#[async_trait]
impl LiveProjectionBuilder for FakeProjectionBuilder {
    async fn build_snapshot(
        &self,
        target: &LiveResolvedTarget,
    ) -> Result<RealtimeProjectionSnapshot, meerkat_runtime::LiveAdapterHostError> {
        Ok(RealtimeProjectionSnapshot {
            identity: target.identity.clone(),
            turning_mode: RealtimeTurningMode::ProviderManaged,
            visible_tools: Vec::new(),
            seed_messages: Vec::new(),
            runtime_system_context: Vec::new(),
            watermark: RealtimeProjectionWatermark {
                version: 1,
                frontier: 10,
                digest: target.session_id.to_string(),
            },
            continuity: RealtimeAdapterContinuity::TranscriptOnly,
            response_nudge_timeout_ms: None,
            response_nudge_max_attempts: None,
        })
    }
}

#[derive(Clone, Default)]
struct FakeFactory {
    commands: Arc<Mutex<Vec<RealtimeAdapterCommand>>>,
    fail_on_close: bool,
}

#[async_trait]
impl LiveAdapterFactory for FakeFactory {
    async fn create_adapter(
        &self,
        _snapshot: &RealtimeProjectionSnapshot,
    ) -> Result<Box<dyn RealtimeAdapter>, meerkat_runtime::LiveAdapterHostError> {
        Ok(Box::new(FakeAdapter {
            commands: Arc::clone(&self.commands),
            fail_on_close: self.fail_on_close,
            capabilities: RealtimeCapabilities {
                input_kinds: vec![RealtimeInputKind::Audio, RealtimeInputKind::Text],
                output_kinds: vec![RealtimeOutputKind::Audio, RealtimeOutputKind::Text],
                turning_modes: vec![RealtimeTurningMode::ProviderManaged],
                interrupt_supported: true,
                transcript_supported: true,
                tool_lifecycle_events_supported: true,
                video_supported: false,
                audio_input_format: Some(RealtimeAudioFormat::pcm(24_000, 1)),
                audio_output_format: Some(RealtimeAudioFormat::pcm(24_000, 1)),
            },
        }))
    }
}

struct FakeAdapter {
    commands: Arc<Mutex<Vec<RealtimeAdapterCommand>>>,
    fail_on_close: bool,
    capabilities: RealtimeCapabilities,
}

#[async_trait]
impl RealtimeAdapter for FakeAdapter {
    fn capabilities(&self) -> &RealtimeCapabilities {
        &self.capabilities
    }

    fn status(&self) -> RealtimeAdapterStatus {
        RealtimeAdapterStatus::Ready
    }

    async fn handle_command(&mut self, command: RealtimeAdapterCommand) -> Result<(), LlmError> {
        if self.fail_on_close && matches!(command, RealtimeAdapterCommand::CloseSession { .. }) {
            return Err(LlmError::Unknown {
                message: "close failed".to_string(),
            });
        }
        self.commands.lock().await.push(command);
        Ok(())
    }

    async fn next_observation(&mut self) -> Result<Option<RealtimeAdapterObservation>, LlmError> {
        Ok(None)
    }
}

#[tokio::test]
async fn live_adapter_host_resolves_mob_identity_again_on_rebuild() {
    let resolver = Arc::new(FakeResolver::default());
    let before = SessionId::parse("00000000-0000-4000-8000-000000000001").unwrap();
    let after = SessionId::parse("00000000-0000-4000-8000-000000000002").unwrap();
    resolver
        .sessions
        .lock()
        .await
        .extend([before.clone(), after.clone()]);
    let factory = FakeFactory::default();
    let resolver_trait: Arc<dyn LiveTargetResolver> = resolver.clone();
    let mut host = LiveAdapterHost::new(
        resolver_trait,
        Arc::new(FakeProjectionBuilder),
        Arc::new(factory.clone()),
    );
    let target = RealtimeChannelTarget::MobMember {
        mob_id: "mob-1".to_string(),
        agent_identity: "lead".to_string(),
    };

    let channel = host.open_primary(target.clone()).await.unwrap();
    host.rebuild(channel, "model switch").await.unwrap();

    let resolved = resolver.resolved.lock().await;
    assert_eq!(resolved.as_slice(), &[target.clone(), target]);

    let commands = factory.commands.lock().await;
    assert!(matches!(
        commands.as_slice(),
        [
            RealtimeAdapterCommand::OpenSession { snapshot: first },
            RealtimeAdapterCommand::RebuildSession { snapshot: second, reason },
        ] if first.watermark.digest == before.to_string()
            && second.watermark.digest == after.to_string()
            && reason == "model switch"
    ));
}

#[tokio::test]
async fn live_adapter_host_rejects_stale_observation_envelopes() {
    let resolver = Arc::new(FakeResolver::default());
    resolver
        .sessions
        .lock()
        .await
        .extend([SessionId::parse("00000000-0000-4000-8000-000000000011").unwrap()]);
    let resolver_trait: Arc<dyn LiveTargetResolver> = resolver;
    let mut host = LiveAdapterHost::new(
        resolver_trait,
        Arc::new(FakeProjectionBuilder),
        Arc::new(FakeFactory::default()),
    );

    let channel = host
        .open_primary(RealtimeChannelTarget::SessionTarget {
            session_id: "session-1".to_string(),
        })
        .await
        .unwrap();
    let current = host.channel_snapshot(channel).unwrap();

    let stale = RealtimeAdapterObservationEnvelope {
        generation: current.generation.next(),
        watermark: current.watermark,
        observation: RealtimeAdapterObservation::ProviderInputCommitted,
    };

    assert_eq!(host.accept_observation_envelope(channel, stale), None);
}

#[tokio::test]
async fn live_adapter_host_close_error_does_not_pin_primary_target() {
    let resolver = Arc::new(FakeResolver::default());
    resolver.sessions.lock().await.extend([
        SessionId::parse("00000000-0000-4000-8000-000000000021").unwrap(),
        SessionId::parse("00000000-0000-4000-8000-000000000022").unwrap(),
    ]);
    let resolver_trait: Arc<dyn LiveTargetResolver> = resolver;
    let mut host = LiveAdapterHost::new(
        resolver_trait,
        Arc::new(FakeProjectionBuilder),
        Arc::new(FakeFactory {
            fail_on_close: true,
            ..FakeFactory::default()
        }),
    );
    let target = RealtimeChannelTarget::SessionTarget {
        session_id: "session-1".to_string(),
    };

    let channel = host.open_primary(target.clone()).await.unwrap();
    let err = host
        .close(channel, "provider close failed")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        meerkat_runtime::LiveAdapterHostError::Adapter(LlmError::Unknown { .. })
    ));

    let reopened = host.open_primary(target).await.unwrap();
    assert_ne!(channel, reopened);
}
