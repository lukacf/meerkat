//! Provider-brokered WebRTC answer construction for an admitted live channel.
//!
//! This module deliberately does not mint, store, resolve, or consume browser
//! signaling tokens. MeerkatMachine remains the one owner of that authority.
//! The RPC answer path can construct a provider offer only by consuming the
//! opaque one-use admission seal minted from the generated
//! `ResolveLiveWebrtcAnswerAdmission` effect. Physical transport custody lives
//! in the composing facade, not in this provider-neutral carrier crate.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use meerkat_core::live_execution::{LiveChannelId, LiveResultDisposition};
use meerkat_core::types::SessionId;

/// Machine-projected runtime generation for one exact live transport binding.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LiveRuntimeBindingGeneration(u64);

impl LiveRuntimeBindingGeneration {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for LiveRuntimeBindingGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LiveRuntimeBindingGeneration([REDACTED])")
    }
}

/// Machine-projected runtime fence for one exact live transport binding.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LiveRuntimeBindingFence(u64);

impl LiveRuntimeBindingFence {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for LiveRuntimeBindingFence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LiveRuntimeBindingFence([REDACTED])")
    }
}

/// Provider-neutral projection of one exact runtime incarnation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveWebrtcRuntimeBinding {
    pub generation: u64,
    pub fence: u64,
}

/// Opaque one-use proof that generated MeerkatMachine authority admitted the
/// exact channel/session answer token. The runtime bridge is the only
/// production minter; transports can consume it but cannot inspect or clone a
/// fresh authority.
#[derive(Clone)]
pub struct LiveWebrtcAnswerAdmissionSeal {
    channel_id: LiveChannelId,
    session_id: SessionId,
    consumed: Arc<AtomicBool>,
}

impl PartialEq for LiveWebrtcAnswerAdmissionSeal {
    fn eq(&self, other: &Self) -> bool {
        self.channel_id == other.channel_id
            && self.session_id == other.session_id
            && Arc::ptr_eq(&self.consumed, &other.consumed)
    }
}

impl Eq for LiveWebrtcAnswerAdmissionSeal {}

impl fmt::Debug for LiveWebrtcAnswerAdmissionSeal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiveWebrtcAnswerAdmissionSeal")
            .field("channel_id", &"[REDACTED]")
            .field("session_id", &"[REDACTED]")
            .field("consumed", &self.consumed.load(Ordering::Relaxed))
            .finish()
    }
}

impl LiveWebrtcAnswerAdmissionSeal {
    /// Runtime-only bridge from a matching admitted generated effect.
    #[cfg(feature = "__meerkat-generated-authority-bridge")]
    #[doc(hidden)]
    #[must_use]
    pub fn __from_generated_admission(channel_id: LiveChannelId, session_id: SessionId) -> Self {
        Self {
            channel_id,
            session_id,
            consumed: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(feature = "webrtc")]
    pub(crate) fn consume_for(
        &self,
        channel_id: &LiveChannelId,
        session_id: &SessionId,
    ) -> Result<(), LiveWebrtcAdmissionSealError> {
        if self.channel_id != *channel_id || self.session_id != *session_id {
            return Err(LiveWebrtcAdmissionSealError::BindingMismatch);
        }
        self.consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| LiveWebrtcAdmissionSealError::AlreadyConsumed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveWebrtcAdmissionSealError {
    #[error("live WebRTC answer admission does not match the exact channel and session")]
    BindingMismatch,
    #[error("live WebRTC answer admission was already consumed")]
    AlreadyConsumed,
}

impl LiveWebrtcAdmissionSealError {
    #[must_use]
    pub const fn reason_code(self) -> &'static str {
        match self {
            Self::BindingMismatch => "binding_mismatch",
            Self::AlreadyConsumed => "already_consumed",
        }
    }
}

/// Exact authority tuple that fences provider-side physical transport custody.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProviderWebrtcBinding {
    channel_id: LiveChannelId,
    session_id: SessionId,
    runtime_generation: LiveRuntimeBindingGeneration,
    runtime_fence: LiveRuntimeBindingFence,
}

/// Opaque one-use evidence that the admitted provider sideband reached
/// SessionReady and acknowledged the exact canonical seed cursor.
#[derive(Clone)]
pub struct ProviderWebrtcBoundReadyReceipt {
    binding: ProviderWebrtcBinding,
    canonical_seed_cursor: u64,
    consumed: Arc<AtomicBool>,
}

impl fmt::Debug for ProviderWebrtcBoundReadyReceipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderWebrtcBoundReadyReceipt")
            .field("binding", &self.binding)
            .field("canonical_seed_cursor", &self.canonical_seed_cursor)
            .field("consumed", &self.consumed.load(Ordering::Relaxed))
            .finish()
    }
}

impl ProviderWebrtcBoundReadyReceipt {
    fn from_seed_ack(binding: ProviderWebrtcBinding, canonical_seed_cursor: u64) -> Self {
        Self {
            binding,
            canonical_seed_cursor,
            consumed: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(feature = "__meerkat-generated-authority-bridge")]
    #[doc(hidden)]
    pub fn __consume_for_generated_bind(
        &self,
        binding: &ProviderWebrtcBinding,
    ) -> Result<u64, LiveWebrtcAdmissionSealError> {
        if &self.binding != binding {
            return Err(LiveWebrtcAdmissionSealError::BindingMismatch);
        }
        self.consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| self.canonical_seed_cursor)
            .map_err(|_| LiveWebrtcAdmissionSealError::AlreadyConsumed)
    }
}

impl ProviderWebrtcBinding {
    #[must_use]
    pub fn new(
        channel_id: LiveChannelId,
        session_id: SessionId,
        runtime_generation: LiveRuntimeBindingGeneration,
        runtime_fence: LiveRuntimeBindingFence,
    ) -> Self {
        Self {
            channel_id,
            session_id,
            runtime_generation,
            runtime_fence,
        }
    }

    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn runtime_generation(&self) -> LiveRuntimeBindingGeneration {
        self.runtime_generation
    }

    #[must_use]
    pub const fn runtime_fence(&self) -> LiveRuntimeBindingFence {
        self.runtime_fence
    }
}

impl fmt::Debug for ProviderWebrtcBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderWebrtcBinding")
            .field("channel_id", &"[REDACTED]")
            .field("session_id", &"[REDACTED]")
            .field("runtime_generation", &"[REDACTED]")
            .field("runtime_fence", &"[REDACTED]")
            .finish()
    }
}

/// Provider-neutral input to the admitted provider signaling broker.
pub struct ProviderWebrtcOffer {
    binding: ProviderWebrtcBinding,
    offer_sdp: String,
}

impl ProviderWebrtcOffer {
    #[cfg(feature = "webrtc")]
    pub(crate) fn new(binding: ProviderWebrtcBinding, offer_sdp: String) -> Self {
        Self { binding, offer_sdp }
    }

    #[must_use]
    pub fn binding(&self) -> &ProviderWebrtcBinding {
        &self.binding
    }

    #[must_use]
    pub fn offer_sdp(&self) -> &str {
        &self.offer_sdp
    }

    /// Consume the admitted provider offer into a seeded answer. Only the
    /// broker holding this unforgeable offer can mint bound-ready evidence.
    #[must_use]
    pub fn into_seeded_answer(
        self,
        answer_sdp: String,
        sideband: Arc<dyn ProviderWebrtcSidebandSession>,
        canonical_seed_cursor: u64,
    ) -> ProviderWebrtcBrokerAnswer {
        ProviderWebrtcBrokerAnswer {
            answer_sdp,
            sideband,
            bound_ready: ProviderWebrtcBoundReadyReceipt::from_seed_ack(
                self.binding,
                canonical_seed_cursor,
            ),
        }
    }
}

impl fmt::Debug for ProviderWebrtcOffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderWebrtcOffer")
            .field("binding", &self.binding)
            .field("offer_sdp", &"[REDACTED]")
            .finish()
    }
}

/// Safe failure classes exposed by a provider signaling broker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderWebrtcBrokerError {
    #[error("provider WebRTC signaling is unavailable")]
    Unavailable,
    #[error("provider rejected the WebRTC signaling request")]
    Rejected,
    #[error("provider WebRTC signaling protocol drifted")]
    ProtocolDrift,
}

/// Opaque provider delegation identity. It is round-tripped to the provider
/// adapter but is never a Meerkat authority or diagnostic value.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct LiveSidebandDelegationRef {
    provider_adapter_key: String,
    provider_delegation_id: String,
}

impl LiveSidebandDelegationRef {
    /// Minted only by a provider adapter while lowering a private event.
    #[doc(hidden)]
    #[must_use]
    pub fn __from_provider_observation(
        provider_adapter_key: String,
        provider_delegation_id: String,
    ) -> Option<Self> {
        (!provider_adapter_key.trim().is_empty() && !provider_delegation_id.trim().is_empty())
            .then_some(Self {
                provider_adapter_key,
                provider_delegation_id,
            })
    }

    /// Borrow only for the provider adapter that must round-trip the opaque
    /// identity. Callers must not interpret or log this value.
    #[doc(hidden)]
    #[must_use]
    pub fn __provider_opaque_value(&self) -> &str {
        &self.provider_adapter_key
    }

    /// Stable sanitized adapter-local key for generated correlation joins.
    /// The opaque provider delegation identifier remains inaccessible.
    #[must_use]
    pub fn adapter_key(&self) -> &str {
        &self.provider_adapter_key
    }

    /// Exact comparison used only when generated release authority is sealed.
    #[cfg(feature = "__meerkat-generated-authority-bridge")]
    #[doc(hidden)]
    #[must_use]
    pub fn __matches_provider_delegation_id(&self, expected: &str) -> bool {
        self.provider_delegation_id == expected
    }
}

impl fmt::Debug for LiveSidebandDelegationRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LiveSidebandDelegationRef([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct LiveSidebandTranscriptItemRef {
    provider_adapter_key: String,
    provider_item_id: String,
}

impl LiveSidebandTranscriptItemRef {
    #[doc(hidden)]
    #[must_use]
    pub fn __from_provider_observation(
        provider_adapter_key: String,
        provider_item_id: String,
    ) -> Option<Self> {
        (!provider_adapter_key.trim().is_empty() && !provider_item_id.trim().is_empty()).then_some(
            Self {
                provider_adapter_key,
                provider_item_id,
            },
        )
    }

    #[must_use]
    pub fn adapter_key(&self) -> &str {
        &self.provider_adapter_key
    }

    #[doc(hidden)]
    #[must_use]
    pub fn __provider_opaque_value(&self) -> &str {
        &self.provider_item_id
    }
}

impl fmt::Debug for LiveSidebandTranscriptItemRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LiveSidebandTranscriptItemRef([REDACTED])")
    }
}

/// Channel-incarnation-qualified equality key passed to generated live-turn
/// authority. The provider-local turn ref may restart at `turn:1` whenever a
/// replacement adapter is created, so it is never globally canonical by
/// itself. This codec is injective over `(LiveChannelId, local_ref)` and is
/// deliberately write-only: shared runtime code compares the opaque result
/// and never parses provider meaning back out of it.
#[derive(Clone, PartialEq, Eq, Hash)]
struct LiveProviderTurnCorrelationKey(String);

impl LiveProviderTurnCorrelationKey {
    fn from_channel_and_local_ref(
        channel_id: &LiveChannelId,
        provider_adapter_local_key: String,
    ) -> Option<Self> {
        let channel = channel_id.as_str();
        (!channel.trim().is_empty() && !provider_adapter_local_key.trim().is_empty()).then(|| {
            Self(format!(
                "live-turn-v1:{}:{channel}{provider_adapter_local_key}",
                channel.len()
            ))
        })
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct LiveSidebandTurnRef {
    provider_correlation_key: LiveProviderTurnCorrelationKey,
    provider_turn_id: String,
}

impl LiveSidebandTurnRef {
    #[doc(hidden)]
    #[must_use]
    pub fn __from_provider_observation(
        channel_id: &LiveChannelId,
        provider_adapter_local_key: String,
        provider_turn_id: String,
    ) -> Option<Self> {
        if provider_turn_id.trim().is_empty() {
            return None;
        }
        Some(Self {
            provider_correlation_key: LiveProviderTurnCorrelationKey::from_channel_and_local_ref(
                channel_id,
                provider_adapter_local_key,
            )?,
            provider_turn_id,
        })
    }

    /// Stable channel-incarnation-qualified key for generated correlation
    /// joins. Provider-local refs are never exposed as global identity.
    #[must_use]
    pub fn adapter_key(&self) -> &str {
        self.provider_correlation_key.as_str()
    }

    #[doc(hidden)]
    #[must_use]
    pub fn __provider_opaque_value(&self) -> &str {
        &self.provider_turn_id
    }
}

impl fmt::Debug for LiveSidebandTurnRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LiveSidebandTurnRef([REDACTED])")
    }
}

/// Provider-observed conversational role for one exact turn lifecycle.
/// Unknown values remain typed and cannot be projected as user or assistant
/// transcript authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveSidebandTurnRole {
    User,
    Assistant,
    Unknown,
}

/// Opaque identity of one machine-authorized append attempt.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct LiveSidebandAppendAttempt(String);

impl LiveSidebandAppendAttempt {
    #[cfg(feature = "__meerkat-generated-authority-bridge")]
    #[doc(hidden)]
    #[must_use]
    pub fn __from_generated_append_id(append_id: String) -> Option<Self> {
        (!append_id.trim().is_empty()).then_some(Self(append_id))
    }
}

impl fmt::Debug for LiveSidebandAppendAttempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LiveSidebandAppendAttempt([REDACTED])")
    }
}

/// Non-forgeable append authority. No public constructor exists. The generated
/// machine bridge will be the only production minter once append authority is
/// added to the machine schema.
#[derive(Clone)]
pub struct LiveSidebandAppendAuthority {
    binding: ProviderWebrtcBinding,
    attempt: LiveSidebandAppendAttempt,
    cursor: u64,
    consumed: Arc<AtomicBool>,
}

impl fmt::Debug for LiveSidebandAppendAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiveSidebandAppendAuthority")
            .field("binding", &self.binding)
            .field("attempt", &self.attempt)
            .field("cursor", &"[REDACTED]")
            .field("consumed", &self.consumed.load(Ordering::Relaxed))
            .finish()
    }
}

impl LiveSidebandAppendAuthority {
    #[cfg(feature = "__meerkat-generated-authority-bridge")]
    #[doc(hidden)]
    #[must_use]
    pub fn __from_generated_authority(
        binding: ProviderWebrtcBinding,
        append_id: String,
        cursor: u64,
    ) -> Option<Self> {
        Some(Self {
            binding,
            attempt: LiveSidebandAppendAttempt::__from_generated_append_id(append_id)?,
            cursor,
            consumed: Arc::new(AtomicBool::new(false)),
        })
    }

    fn consume_once(&self) -> Result<(), LiveSidebandCommandError> {
        self.consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| LiveSidebandCommandError::AuthorityAlreadyConsumed)
    }
}

/// Non-forgeable release authority for executor result context. It remains
/// distinct from ordinary context append so a caller cannot publish a result
/// before generated disposition authority releases it.
#[derive(Clone)]
pub struct LiveSidebandReleaseAuthority {
    binding: ProviderWebrtcBinding,
    attempt: LiveSidebandAppendAttempt,
    disposition: LiveResultDisposition,
    _content_digest: String,
    consumed: Arc<AtomicBool>,
}

impl fmt::Debug for LiveSidebandReleaseAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiveSidebandReleaseAuthority")
            .field("binding", &self.binding)
            .field("attempt", &self.attempt)
            .field("disposition", &self.disposition)
            .field("content_digest", &"[REDACTED]")
            .field("consumed", &self.consumed.load(Ordering::Relaxed))
            .finish()
    }
}

impl LiveSidebandReleaseAuthority {
    #[cfg(feature = "__meerkat-generated-authority-bridge")]
    #[doc(hidden)]
    #[must_use]
    pub fn __from_generated_result_authority(
        binding: ProviderWebrtcBinding,
        delivery_id: String,
        disposition: LiveResultDisposition,
        content_digest: String,
    ) -> Option<Self> {
        if content_digest.trim().is_empty() {
            return None;
        }
        Some(Self {
            binding,
            attempt: LiveSidebandAppendAttempt::__from_generated_append_id(format!(
                "result:{delivery_id}"
            ))?,
            disposition,
            _content_digest: content_digest,
            consumed: Arc::new(AtomicBool::new(false)),
        })
    }

    fn consume_once(&self) -> Result<(), LiveSidebandCommandError> {
        self.consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| LiveSidebandCommandError::AuthorityAlreadyConsumed)
    }

    #[cfg(test)]
    fn from_test_machine(
        binding: ProviderWebrtcBinding,
        attempt: u64,
        disposition: LiveResultDisposition,
    ) -> Self {
        Self {
            binding,
            attempt: LiveSidebandAppendAttempt(attempt.to_string()),
            disposition,
            _content_digest: "test-content-digest".to_string(),
            consumed: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(PartialEq, Eq)]
enum LiveSidebandCommandKind {
    AppendSessionContext {
        binding: ProviderWebrtcBinding,
        attempt: LiveSidebandAppendAttempt,
        cursor: u64,
        text: String,
    },
    ReleaseDelegationContext {
        binding: ProviderWebrtcBinding,
        attempt: LiveSidebandAppendAttempt,
        delegation: LiveSidebandDelegationRef,
        disposition: LiveResultDisposition,
        text: String,
    },
}

/// A command that has already consumed one machine-minted authority. Its kind
/// and payload are private, so public enum construction cannot bypass the
/// authority-consuming constructors.
#[derive(PartialEq, Eq)]
pub struct LiveSidebandCommand {
    kind: LiveSidebandCommandKind,
}

/// Read-only provider adapter decomposition. Constructing this carrier cannot
/// create a sendable [`LiveSidebandCommand`]; only consuming an already
/// authorized command can produce it for provider lowering.
#[doc(hidden)]
pub enum LiveSidebandProviderCommand {
    AppendSessionContext {
        binding: ProviderWebrtcBinding,
        attempt: LiveSidebandAppendAttempt,
        cursor: u64,
        text: String,
    },
    ReleaseDelegationContext {
        binding: ProviderWebrtcBinding,
        attempt: LiveSidebandAppendAttempt,
        delegation: LiveSidebandDelegationRef,
        disposition: LiveResultDisposition,
        text: String,
    },
}

impl fmt::Debug for LiveSidebandCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.kind {
            LiveSidebandCommandKind::AppendSessionContext { .. } => "append_session_context",
            LiveSidebandCommandKind::ReleaseDelegationContext { .. } => {
                "release_delegation_context"
            }
        };
        formatter
            .debug_struct("LiveSidebandCommand")
            .field("kind", &kind)
            .field("payload", &"[REDACTED]")
            .finish()
    }
}

impl LiveSidebandCommand {
    pub fn append_session_context(
        authority: LiveSidebandAppendAuthority,
        text: impl Into<String>,
    ) -> Result<Self, LiveSidebandCommandError> {
        let text = require_sideband_text(text)?;
        authority.consume_once()?;
        Ok(Self {
            kind: LiveSidebandCommandKind::AppendSessionContext {
                binding: authority.binding,
                attempt: authority.attempt,
                cursor: authority.cursor,
                text,
            },
        })
    }

    pub fn release_delegation_context(
        authority: LiveSidebandReleaseAuthority,
        delegation: LiveSidebandDelegationRef,
        text: impl Into<String>,
    ) -> Result<Self, LiveSidebandCommandError> {
        let text = require_sideband_text(text)?;
        authority.consume_once()?;
        let LiveSidebandReleaseAuthority {
            binding,
            attempt,
            disposition,
            _content_digest: _,
            consumed: _,
        } = authority;
        Ok(Self {
            kind: LiveSidebandCommandKind::ReleaseDelegationContext {
                binding,
                attempt,
                delegation,
                disposition,
                text,
            },
        })
    }

    #[must_use]
    pub fn binding(&self) -> &ProviderWebrtcBinding {
        match &self.kind {
            LiveSidebandCommandKind::AppendSessionContext { binding, .. }
            | LiveSidebandCommandKind::ReleaseDelegationContext { binding, .. } => binding,
        }
    }

    #[must_use]
    pub fn attempt(&self) -> LiveSidebandAppendAttempt {
        match &self.kind {
            LiveSidebandCommandKind::AppendSessionContext { attempt, .. }
            | LiveSidebandCommandKind::ReleaseDelegationContext { attempt, .. } => attempt.clone(),
        }
    }

    /// Consume this authorized command into a provider-readable carrier. The
    /// returned enum is not accepted by the sideband send API and therefore
    /// cannot be used to forge a command in the opposite direction.
    #[doc(hidden)]
    #[must_use]
    pub fn __into_provider_command(self) -> LiveSidebandProviderCommand {
        match self.kind {
            LiveSidebandCommandKind::AppendSessionContext {
                binding,
                attempt,
                cursor,
                text,
            } => LiveSidebandProviderCommand::AppendSessionContext {
                binding,
                attempt,
                cursor,
                text,
            },
            LiveSidebandCommandKind::ReleaseDelegationContext {
                binding,
                attempt,
                delegation,
                disposition,
                text,
            } => LiveSidebandProviderCommand::ReleaseDelegationContext {
                binding,
                attempt,
                delegation,
                disposition,
                text,
            },
        }
    }
}

fn require_sideband_text(text: impl Into<String>) -> Result<String, LiveSidebandCommandError> {
    let text = text.into();
    if text.trim().is_empty() {
        Err(LiveSidebandCommandError::EmptyText)
    } else {
        Ok(text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveSidebandCommandError {
    #[error("live sideband context text must not be empty")]
    EmptyText,
    #[error("live sideband command authority was already consumed")]
    AuthorityAlreadyConsumed,
}

/// Outcome of handing one authorized command to the opaque provider session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveSidebandCommandDelivery {
    Accepted,
    AmbiguousTerminal,
}

/// Sanitized provider-neutral observation kind emitted by the sideband actor.
#[derive(Clone, PartialEq, Eq)]
pub enum LiveSidebandObservationKind {
    SessionReady,
    /// Provider input-transcript transport detail. This is not canonical user
    /// transcript authority; only an exact user-role turn terminal can emit
    /// the parent-session final.
    UserTranscriptFragment {
        item: LiveSidebandTranscriptItemRef,
        text: String,
    },
    TurnStarted {
        turn: LiveSidebandTurnRef,
        role: LiveSidebandTurnRole,
    },
    /// Noncanonical partial snapshot for one provider turn. Gate0 may inspect
    /// this control fact, but the live adapter must never project it as
    /// assistant authored output until its semantics are qualified.
    TurnSnapshotDelta {
        turn: LiveSidebandTurnRef,
        delta: String,
    },
    TurnFinished {
        turn: LiveSidebandTurnRef,
        role: LiveSidebandTurnRole,
        transcript: String,
    },
    /// Provider output-transcript transport detail. Canonical assistant text
    /// is emitted only from an exact assistant turn lifecycle, so fragments
    /// cannot become duplicate parent messages.
    AssistantTranscriptFragment {
        item: LiveSidebandTranscriptItemRef,
        text: String,
    },
    DelegationRequested {
        turn: LiveSidebandTurnRef,
        delegation: LiveSidebandDelegationRef,
        actionable_input: String,
    },
    /// A client-context delegation whose provider payload cannot establish a
    /// normalized prose handoff. This is not a Responses function call and
    /// must never be converted into one by the provider-neutral layer.
    DelegationActionableInputUnsupported {
        delegation: LiveSidebandDelegationRef,
    },
    AppendAcknowledged {
        attempt: LiveSidebandAppendAttempt,
    },
    AppendDeliveryAmbiguousTerminal {
        attempt: LiveSidebandAppendAttempt,
    },
    UnsupportedProviderEvent,
}

impl fmt::Debug for LiveSidebandObservationKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::SessionReady => "session_ready",
            Self::UserTranscriptFragment { .. } => "user_transcript_fragment",
            Self::TurnStarted { .. } => "turn_started",
            Self::TurnSnapshotDelta { .. } => "turn_snapshot_delta",
            Self::TurnFinished { .. } => "turn_finished",
            Self::AssistantTranscriptFragment { .. } => "assistant_transcript_fragment",
            Self::DelegationRequested { .. } => "delegation_requested",
            Self::DelegationActionableInputUnsupported { .. } => {
                "delegation_actionable_input_unsupported"
            }
            Self::AppendAcknowledged { .. } => "append_acknowledged",
            Self::AppendDeliveryAmbiguousTerminal { .. } => "append_delivery_ambiguous_terminal",
            Self::UnsupportedProviderEvent => "unsupported_provider_event",
        };
        formatter
            .debug_struct("LiveSidebandObservationKind")
            .field("kind", &kind)
            .field("payload", &"[REDACTED]")
            .finish()
    }
}

/// One observation fenced to the exact transport binding that produced it.
/// Delayed callbacks from a replaced runtime generation or fence therefore
/// carry their stale identity and cannot be mistaken for current truth.
#[derive(Clone, PartialEq, Eq)]
pub struct LiveSidebandObservation {
    binding: ProviderWebrtcBinding,
    kind: LiveSidebandObservationKind,
}

impl LiveSidebandObservation {
    #[must_use]
    pub fn new(binding: ProviderWebrtcBinding, kind: LiveSidebandObservationKind) -> Self {
        Self { binding, kind }
    }

    #[must_use]
    pub fn binding(&self) -> &ProviderWebrtcBinding {
        &self.binding
    }

    #[must_use]
    pub fn kind(&self) -> &LiveSidebandObservationKind {
        &self.kind
    }

    #[must_use]
    pub fn into_kind(self) -> LiveSidebandObservationKind {
        self.kind
    }
}

impl fmt::Debug for LiveSidebandObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiveSidebandObservation")
            .field("binding", &self.binding)
            .field("kind", &self.kind)
            .finish()
    }
}

/// Opaque provider sideband session. The answer strategy is its sole physical
/// owner; semantic callers interact only through authorized commands and
/// sanitized observations.
#[async_trait]
pub trait ProviderWebrtcSidebandSession: Send + Sync {
    async fn send_command(
        &self,
        command: LiveSidebandCommand,
    ) -> Result<LiveSidebandCommandDelivery, ProviderWebrtcBrokerError>;

    async fn next_observation(
        &self,
    ) -> Result<Option<LiveSidebandObservation>, ProviderWebrtcBrokerError>;

    /// Mechanical cleanup invoked by the answer strategy. This is not a
    /// semantic context release and therefore carries no release authority.
    async fn close(&self) -> Result<(), ProviderWebrtcBrokerError>;
}

/// Mechanical result of provider answer construction.
pub struct ProviderWebrtcBrokerAnswer {
    answer_sdp: String,
    sideband: Arc<dyn ProviderWebrtcSidebandSession>,
    bound_ready: ProviderWebrtcBoundReadyReceipt,
}

impl ProviderWebrtcBrokerAnswer {
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        String,
        Arc<dyn ProviderWebrtcSidebandSession>,
        ProviderWebrtcBoundReadyReceipt,
    ) {
        (self.answer_sdp, self.sideband, self.bound_ready)
    }
}

impl fmt::Debug for ProviderWebrtcBrokerAnswer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderWebrtcBrokerAnswer")
            .field("answer_sdp", &"[REDACTED]")
            .field("sideband", &"[OPAQUE]")
            .finish()
    }
}

/// Admitted provider signaling implementation.
///
/// Admission of credentials, provider policy, and model capability happens
/// before this broker is installed. The broker receives transport material
/// only and must not make Meerkat semantic decisions.
#[async_trait]
pub trait ProviderWebrtcBroker: Send + Sync {
    async fn answer(
        &self,
        offer: ProviderWebrtcOffer,
    ) -> Result<ProviderWebrtcBrokerAnswer, ProviderWebrtcBrokerError>;
}

/// Mechanical provider answer or cleanup failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderWebrtcSignalingError {
    #[error("provider WebRTC broker failed")]
    Broker(ProviderWebrtcBrokerError),
    #[error("provider WebRTC broker returned an empty SDP answer")]
    EmptyAnswer,
    #[error("provider WebRTC sideband cleanup failed")]
    SidebandClose(ProviderWebrtcBrokerError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> ProviderWebrtcBinding {
        ProviderWebrtcBinding::new(
            LiveChannelId::new("result-context-channel"),
            SessionId::new(),
            LiveRuntimeBindingGeneration::new(3),
            LiveRuntimeBindingFence::new(5),
        )
    }

    #[test]
    fn provider_turn_correlation_key_is_namespaced_by_channel_incarnation() {
        let channel_a = LiveChannelId::new("channel-a");
        let channel_b = LiveChannelId::new("channel-b");
        let turn_a = LiveSidebandTurnRef::__from_provider_observation(
            &channel_a,
            "turn:1".to_string(),
            "private-provider-turn-a".to_string(),
        )
        .expect("channel A turn ref");
        let turn_b = LiveSidebandTurnRef::__from_provider_observation(
            &channel_b,
            "turn:1".to_string(),
            "private-provider-turn-b".to_string(),
        )
        .expect("channel B turn ref");

        assert_ne!(turn_a.adapter_key(), turn_b.adapter_key());
        assert!(!format!("{turn_a:?}").contains("private-provider-turn-a"));
        assert!(!format!("{turn_a:?}").contains(channel_a.as_str()));
    }

    #[test]
    fn result_release_is_context_only_without_canonical_cursor_and_consumes_once() {
        let authority = LiveSidebandReleaseAuthority::from_test_machine(
            binding(),
            17,
            LiveResultDisposition::DeferredContext,
        );
        let duplicate = authority.clone();
        let delegation = LiveSidebandDelegationRef::__from_provider_observation(
            "delegation:1".to_string(),
            "provider-delegation-secret".to_string(),
        )
        .expect("opaque provider delegation");

        let command = LiveSidebandCommand::release_delegation_context(
            authority,
            delegation.clone(),
            "executor result for model context only",
        )
        .expect("one result-context delivery");
        assert!(matches!(
            command.__into_provider_command(),
            LiveSidebandProviderCommand::ReleaseDelegationContext {
                disposition: LiveResultDisposition::DeferredContext,
                text,
                ..
            } if text == "executor result for model context only"
        ));
        assert_eq!(
            LiveSidebandCommand::release_delegation_context(
                duplicate,
                delegation,
                "executor result for model context only",
            ),
            Err(LiveSidebandCommandError::AuthorityAlreadyConsumed)
        );
    }

    #[test]
    fn result_release_debug_redacts_content_digest_and_provider_payload() {
        let authority = LiveSidebandReleaseAuthority::from_test_machine(
            binding(),
            19,
            LiveResultDisposition::OpenTurn,
        );
        let authority_debug = format!("{authority:?}");
        assert!(!authority_debug.contains("test-content-digest"));
        let delegation = LiveSidebandDelegationRef::__from_provider_observation(
            "delegation:2".to_string(),
            "provider-delegation-secret".to_string(),
        )
        .expect("opaque provider delegation");
        let command = LiveSidebandCommand::release_delegation_context(
            authority,
            delegation,
            "private executor result",
        )
        .expect("one result-context delivery");
        let command_debug = format!("{command:?}");
        assert!(!command_debug.contains("private executor result"));
        assert!(!command_debug.contains("provider-delegation-secret"));
    }
}
