//! Runtime-owned host for provider live adapter sessions.
//!
//! The host owns provider adapter mechanics and generation fencing. It does
//! not own turn admission, tool dispatch, canonical terminality, or mob roster
//! truth; those stay behind resolver/projection traits and existing Meerkat
//! APIs.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_contracts::RealtimeChannelTarget;
use meerkat_core::{SessionLlmIdentity, types::SessionId};
use meerkat_llm_core::LlmError;
use meerkat_llm_core::realtime_adapter::{
    RealtimeAdapter, RealtimeAdapterCommand, RealtimeAdapterGeneration, RealtimeAdapterObservation,
    RealtimeAdapterObservationEnvelope, RealtimeAdapterStatus, RealtimeProjectionSnapshot,
    RealtimeProjectionWatermark,
};
use thiserror::Error;

/// Domain handle for one runtime-hosted live adapter channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LiveChannelId(u64);

impl LiveChannelId {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for LiveChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "live-channel-{}", self.0)
    }
}

/// Current session identity resolved at the runtime authority boundary.
#[derive(Debug, Clone)]
pub struct LiveResolvedTarget {
    pub session_id: SessionId,
    pub identity: SessionLlmIdentity,
}

/// Resolve a public live target to the current canonical session identity.
///
/// Mob-member implementations must resolve through current `AgentIdentity`
/// ownership on every open/rebuild instead of caching a session id.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait LiveTargetResolver: Send + Sync {
    async fn resolve(
        &self,
        target: &RealtimeChannelTarget,
    ) -> Result<LiveResolvedTarget, LiveAdapterHostError>;
}

/// Build a provider projection from canonical Meerkat state.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait LiveProjectionBuilder: Send + Sync {
    async fn build_snapshot(
        &self,
        target: &LiveResolvedTarget,
    ) -> Result<RealtimeProjectionSnapshot, LiveAdapterHostError>;
}

/// Create an unopened provider adapter for a projection snapshot.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait LiveAdapterFactory: Send + Sync {
    async fn create_adapter(
        &self,
        snapshot: &RealtimeProjectionSnapshot,
    ) -> Result<Box<dyn RealtimeAdapter>, LiveAdapterHostError>;
}

#[derive(Debug, Error)]
pub enum LiveAdapterHostError {
    #[error("live target already has an active primary channel: {target}")]
    TargetBusy { target: String },
    #[error("unknown live channel: {channel}")]
    UnknownChannel { channel: LiveChannelId },
    #[error("live target resolution failed: {0}")]
    TargetResolution(String),
    #[error("live projection failed: {0}")]
    Projection(String),
    #[error("live adapter failed: {0}")]
    Adapter(#[from] LlmError),
}

/// Stable snapshot of a hosted channel's fencing state.
#[derive(Debug, Clone)]
pub struct LiveAdapterChannelSnapshot {
    pub channel: LiveChannelId,
    pub target: RealtimeChannelTarget,
    pub resolved_session_id: SessionId,
    pub generation: RealtimeAdapterGeneration,
    pub watermark: RealtimeProjectionWatermark,
    pub status: RealtimeAdapterStatus,
}

struct LiveAdapterSession {
    target: RealtimeChannelTarget,
    target_key: String,
    resolved: LiveResolvedTarget,
    generation: RealtimeAdapterGeneration,
    watermark: RealtimeProjectionWatermark,
    status: RealtimeAdapterStatus,
    adapter: Box<dyn RealtimeAdapter>,
}

/// Runtime-owned host for active provider live adapter sessions.
pub struct LiveAdapterHost {
    resolver: Arc<dyn LiveTargetResolver>,
    projection_builder: Arc<dyn LiveProjectionBuilder>,
    adapter_factory: Arc<dyn LiveAdapterFactory>,
    channels: BTreeMap<LiveChannelId, LiveAdapterSession>,
    primary_by_target: BTreeMap<String, LiveChannelId>,
    next_channel: u64,
    next_generation: RealtimeAdapterGeneration,
}

impl LiveAdapterHost {
    #[must_use]
    pub fn new(
        resolver: Arc<dyn LiveTargetResolver>,
        projection_builder: Arc<dyn LiveProjectionBuilder>,
        adapter_factory: Arc<dyn LiveAdapterFactory>,
    ) -> Self {
        Self {
            resolver,
            projection_builder,
            adapter_factory,
            channels: BTreeMap::new(),
            primary_by_target: BTreeMap::new(),
            next_channel: 1,
            next_generation: RealtimeAdapterGeneration(1),
        }
    }

    /// Open the primary live channel for a target.
    ///
    /// The host enforces local primary occupancy, but the target/session facts
    /// are resolved outside the host so mob identity truth stays canonical.
    pub async fn open_primary(
        &mut self,
        target: RealtimeChannelTarget,
    ) -> Result<LiveChannelId, LiveAdapterHostError> {
        let target_key = target_key(&target);
        if self.primary_by_target.contains_key(&target_key) {
            return Err(LiveAdapterHostError::TargetBusy { target: target_key });
        }

        let resolved = self.resolver.resolve(&target).await?;
        let snapshot = self.projection_builder.build_snapshot(&resolved).await?;
        let mut adapter = self.adapter_factory.create_adapter(&snapshot).await?;
        adapter
            .handle_command(RealtimeAdapterCommand::OpenSession {
                snapshot: snapshot.clone(),
            })
            .await?;

        let channel = self.allocate_channel_id();
        let generation = self.allocate_generation();
        let status = adapter.status();
        self.channels.insert(
            channel,
            LiveAdapterSession {
                target,
                target_key: target_key.clone(),
                resolved,
                generation,
                watermark: snapshot.watermark,
                status,
                adapter,
            },
        );
        self.primary_by_target.insert(target_key, channel);
        Ok(channel)
    }

    /// Rebuild a live channel from a fresh target resolution and projection.
    pub async fn rebuild(
        &mut self,
        channel: LiveChannelId,
        reason: impl Into<String>,
    ) -> Result<(), LiveAdapterHostError> {
        let target = self
            .channels
            .get(&channel)
            .ok_or(LiveAdapterHostError::UnknownChannel { channel })?
            .target
            .clone();
        let resolved = self.resolver.resolve(&target).await?;
        let snapshot = self.projection_builder.build_snapshot(&resolved).await?;
        let mut adapter = self.adapter_factory.create_adapter(&snapshot).await?;
        adapter
            .handle_command(RealtimeAdapterCommand::RebuildSession {
                snapshot: snapshot.clone(),
                reason: reason.into(),
            })
            .await?;

        let generation = self.allocate_generation();
        let session = self
            .channels
            .get_mut(&channel)
            .ok_or(LiveAdapterHostError::UnknownChannel { channel })?;
        session.resolved = resolved;
        session.generation = generation;
        session.watermark = snapshot.watermark;
        session.status = adapter.status();
        session.adapter = adapter;
        Ok(())
    }

    /// Send a command to an active adapter.
    pub async fn command(
        &mut self,
        channel: LiveChannelId,
        command: RealtimeAdapterCommand,
    ) -> Result<(), LiveAdapterHostError> {
        let session = self
            .channels
            .get_mut(&channel)
            .ok_or(LiveAdapterHostError::UnknownChannel { channel })?;
        session.adapter.handle_command(command).await?;
        session.status = session.adapter.status();
        Ok(())
    }

    /// Poll and fence the next provider observation for a channel.
    pub async fn next_observation(
        &mut self,
        channel: LiveChannelId,
    ) -> Result<Option<RealtimeAdapterObservationEnvelope>, LiveAdapterHostError> {
        let session = self
            .channels
            .get_mut(&channel)
            .ok_or(LiveAdapterHostError::UnknownChannel { channel })?;
        let Some(observation) = session.adapter.next_observation().await? else {
            return Ok(None);
        };
        if let RealtimeAdapterObservation::ProviderStatus { status } = &observation {
            session.status = status.clone();
        }
        Ok(Some(RealtimeAdapterObservationEnvelope {
            generation: session.generation,
            watermark: session.watermark.clone(),
            observation,
        }))
    }

    /// Accept an already-enveloped observation only if it still belongs to the
    /// active channel generation and projection watermark.
    #[must_use]
    pub fn accept_observation_envelope(
        &self,
        channel: LiveChannelId,
        envelope: RealtimeAdapterObservationEnvelope,
    ) -> Option<RealtimeAdapterObservation> {
        let session = self.channels.get(&channel)?;
        if envelope.generation == session.generation && envelope.watermark == session.watermark {
            Some(envelope.observation)
        } else {
            None
        }
    }

    #[must_use]
    pub fn channel_snapshot(&self, channel: LiveChannelId) -> Option<LiveAdapterChannelSnapshot> {
        let session = self.channels.get(&channel)?;
        Some(LiveAdapterChannelSnapshot {
            channel,
            target: session.target.clone(),
            resolved_session_id: session.resolved.session_id.clone(),
            generation: session.generation,
            watermark: session.watermark.clone(),
            status: session.status.clone(),
        })
    }

    pub async fn close(
        &mut self,
        channel: LiveChannelId,
        reason: impl Into<String>,
    ) -> Result<(), LiveAdapterHostError> {
        let mut session = self
            .channels
            .remove(&channel)
            .ok_or(LiveAdapterHostError::UnknownChannel { channel })?;
        self.primary_by_target.remove(&session.target_key);
        session
            .adapter
            .handle_command(RealtimeAdapterCommand::CloseSession {
                reason: reason.into(),
            })
            .await?;
        Ok(())
    }

    fn allocate_channel_id(&mut self) -> LiveChannelId {
        let channel = LiveChannelId::new(self.next_channel);
        self.next_channel += 1;
        channel
    }

    fn allocate_generation(&mut self) -> RealtimeAdapterGeneration {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.next();
        generation
    }
}

fn target_key(target: &RealtimeChannelTarget) -> String {
    match target {
        RealtimeChannelTarget::SessionTarget { session_id } => {
            format!("session:{session_id}")
        }
        RealtimeChannelTarget::MobMember {
            mob_id,
            agent_identity,
        } => format!("mob:{mob_id}:agent:{agent_identity}"),
    }
}
