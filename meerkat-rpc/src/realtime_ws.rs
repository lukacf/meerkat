//! Sibling WebSocket host for realtime channel transport.
//!
//! This module owns only transport mechanics and bootstrap control-plane state.
//! It binds a dedicated websocket listener alongside the existing JSONL
//! stdio/TCP RPC host, issues single-use open tokens for `realtime/open_info`,
//! and validates the initial `channel.open` handshake.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    extract::{
        State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code},
    },
    response::IntoResponse,
    routing::get,
};
use chrono::{DateTime, Utc};
use meerkat_client::{
    RealtimeSessionEvent, RealtimeSessionFactory, realtime_session::RealtimeSessionOpenConfig,
};
use meerkat_contracts::{
    AudioFormatMismatchContext, RealtimeAudioFormat, RealtimeCapabilities,
    RealtimeChannelClosedFrame, RealtimeChannelErrorFrame, RealtimeChannelEventFrame,
    RealtimeChannelOpenFrame, RealtimeChannelOpenedFrame, RealtimeChannelState,
    RealtimeChannelStatus, RealtimeChannelStatusFrame, RealtimeClientFrame, RealtimeErrorCode,
    RealtimeErrorDetails, RealtimeEvent, RealtimeInputChunk, RealtimeOpenInfo, RealtimeOpenRequest,
    RealtimeProtocolVersion, RealtimeReconnectPolicy, RealtimeServerFrame,
};
use meerkat_core::{ConfigStore, SessionId};
use meerkat_runtime::{
    Input, PromptInput, RuntimeDriverError, RuntimeState, service_ext::SessionServiceRuntimeExt,
};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio::time::{Instant, MissedTickBehavior};
use uuid::Uuid;

use crate::session_runtime::SessionRuntime;

/// Canonical websocket path for realtime channels hosted by `rkat-rpc`.
pub const REALTIME_WS_PATH: &str = "/realtime/ws";
const DEFAULT_OPEN_TOKEN_TTL: Duration = Duration::from_secs(60);
const RECONNECT_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone)]
struct RealtimeWsState {
    host: Arc<RealtimeWsHost>,
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
}

#[derive(Debug, Clone)]
struct PendingOpenEntry {
    request: RealtimeOpenRequest,
    capabilities: RealtimeCapabilities,
    expires_at: DateTime<Utc>,
    /// Realm scope captured when the open_info was minted. The websocket
    /// accept path re-observes the current realm and must match this value;
    /// mismatches are rejected with `RealtimeErrorCode::UnauthorizedRealm` so a
    /// token minted in realm X cannot be redeemed inside realm Y.
    realm_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RealtimeTargetKey {
    Session(String),
    // Placeholder variant retained for pattern-exhaustiveness on binding
    // enum (T5i kept the empty variant for the SocketBinding pattern match);
    // never constructed post-T5i.
    #[allow(dead_code)]
    MobMember {
        mob_id: String,
        agent_identity: String,
    },
}

impl From<&meerkat_contracts::RealtimeChannelTarget> for RealtimeTargetKey {
    fn from(target: &meerkat_contracts::RealtimeChannelTarget) -> Self {
        match target {
            meerkat_contracts::RealtimeChannelTarget::SessionTarget { session_id } => {
                Self::Session(session_id.clone())
            }
        }
    }
}

#[derive(Debug, Default, Clone)]
struct ActiveTargetEntry {
    primary_connection: Option<String>,
    observer_connections: HashSet<String>,
    observer_fanout: Option<broadcast::Sender<RealtimeServerFrame>>,
}

#[derive(Debug, Clone)]
enum RealtimeSocketBinding {
    SessionPrimary { session_id: SessionId },
    SessionObserver { session_id: SessionId },
}

#[derive(Debug, Default)]
struct RealtimePendingTurn {
    staged_user_text: String,
    staged_assistant_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RealtimeTurnCompletionDisposition {
    Finalize,
    SuppressKeepStaged,
    SuppressDiscardStaged,
}

fn product_turn_completion_disposition(
    stop_reason: meerkat_core::types::StopReason,
) -> RealtimeTurnCompletionDisposition {
    match stop_reason {
        meerkat_core::types::StopReason::ToolUse => {
            RealtimeTurnCompletionDisposition::SuppressKeepStaged
        }
        meerkat_core::types::StopReason::Cancelled => {
            RealtimeTurnCompletionDisposition::SuppressDiscardStaged
        }
        _ => RealtimeTurnCompletionDisposition::Finalize,
    }
}

fn product_turn_completion_is_logically_terminal(
    stop_reason: meerkat_core::types::StopReason,
) -> bool {
    !matches!(
        product_turn_completion_disposition(stop_reason),
        RealtimeTurnCompletionDisposition::SuppressKeepStaged
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RealtimeBindingProjection {
    Unattached,
    IntentPresentUnbound,
    BindingNotReady,
    BindingReady,
    ReplacementPending,
    ReattachRequired,
}

/// Tiny splitmix64 PRNG. Used to produce a per-channel full-jitter stream for
/// reconnect backoff without pulling a crate-level `rand` dependency into the
/// realtime hot path. Deterministic per-overlay: tests can seed it by
/// constructing the overlay with a fixed seed.
#[derive(Debug, Clone)]
struct BackoffJitterRng {
    state: u64,
}

impl BackoffJitterRng {
    const fn new(seed: u64) -> Self {
        // splitmix64 refuses all-zero seeds; fall back to a fixed golden ratio.
        let effective = if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        };
        Self { state: effective }
    }

    fn next_u64(&mut self) -> u64 {
        // splitmix64 — a single well-known cycle that is cheap and has good
        // output distribution for deterministic jitter.
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Return a random value in `[0, ceiling_ms]` inclusive. When `ceiling_ms`
    /// is zero, returns zero.
    fn draw_ms(&mut self, ceiling_ms: u64) -> u64 {
        if ceiling_ms == 0 {
            0
        } else {
            self.next_u64() % (ceiling_ms + 1)
        }
    }
}

#[derive(Debug, Clone)]
struct RealtimeReconnectOverlay {
    policy: RealtimeReconnectPolicy,
    cycle_started_at: Option<Instant>,
    cycle_started_at_utc: Option<DateTime<Utc>>,
    attempt_count: u32,
    next_retry_deadline: Option<Instant>,
    next_retry_at: Option<DateTime<Utc>>,
    jitter: BackoffJitterRng,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RealtimeReconnectFailure {
    RetryScheduled(RealtimeChannelStatus),
    Exhausted {
        status: RealtimeChannelStatus,
        error: RealtimeChannelErrorFrame,
        close_reason: String,
    },
}

impl RealtimeReconnectOverlay {
    fn new(policy: RealtimeReconnectPolicy) -> Self {
        // Per-channel seed derived from the wall-clock plus an incrementing
        // counter. Tests use `new_with_seed` for determinism.
        let seed = {
            let now_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|dur| dur.as_nanos() as u64)
                .unwrap_or(0);
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let ticker = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            now_ns
                .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                .wrapping_add(ticker)
        };
        Self::new_with_seed(policy, seed)
    }

    fn new_with_seed(policy: RealtimeReconnectPolicy, jitter_seed: u64) -> Self {
        Self {
            policy: RealtimeReconnectPolicy {
                max_attempts: policy.max_attempts.max(1),
                initial_backoff_ms: policy.initial_backoff_ms,
                max_backoff_ms: policy.max_backoff_ms.max(policy.initial_backoff_ms),
                max_total_ms: policy.max_total_ms,
            },
            cycle_started_at: None,
            cycle_started_at_utc: None,
            attempt_count: 0,
            next_retry_deadline: None,
            next_retry_at: None,
            jitter: BackoffJitterRng::new(jitter_seed),
        }
    }

    fn default_policy() -> RealtimeReconnectPolicy {
        RealtimeReconnectPolicy {
            max_attempts: 3,
            initial_backoff_ms: 500,
            max_backoff_ms: 5_000,
            max_total_ms: 30_000,
        }
    }

    fn is_active(&self) -> bool {
        self.cycle_started_at.is_some()
    }

    fn deadline_at(&self) -> Option<String> {
        let started = self.cycle_started_at_utc?;
        if self.policy.max_total_ms == 0 {
            return None;
        }
        let max_total =
            chrono::TimeDelta::from_std(Duration::from_millis(self.policy.max_total_ms))
                .unwrap_or_else(|_| chrono::TimeDelta::zero());
        Some((started + max_total).to_rfc3339())
    }

    fn current_status(&self) -> Option<RealtimeChannelStatus> {
        self.is_active().then(|| RealtimeChannelStatus {
            state: RealtimeChannelState::Reconnecting,
            attempt_count: self.attempt_count,
            next_retry_at: self.next_retry_at.map(|deadline| deadline.to_rfc3339()),
            deadline_at: self.deadline_at(),
            reason: Some("realtime attachment requires reattach".to_string()),
        })
    }

    fn begin_if_needed(
        &mut self,
        now: Instant,
        now_utc: DateTime<Utc>,
    ) -> Option<RealtimeChannelStatus> {
        if self.is_active() {
            return None;
        }
        self.cycle_started_at = Some(now);
        self.cycle_started_at_utc = Some(now_utc);
        self.attempt_count = 1;
        self.schedule_next_retry(now, now_utc);
        self.current_status()
    }

    fn clear(&mut self) {
        self.cycle_started_at = None;
        self.cycle_started_at_utc = None;
        self.attempt_count = 0;
        self.next_retry_deadline = None;
        self.next_retry_at = None;
    }

    fn should_exhaust(&self, now: Instant) -> bool {
        let Some(started_at) = self.cycle_started_at else {
            return false;
        };
        self.policy.max_total_ms > 0
            && now.duration_since(started_at) >= Duration::from_millis(self.policy.max_total_ms)
    }

    fn attempt_due(&self, now: Instant) -> bool {
        self.next_retry_deadline
            .is_some_and(|deadline| now >= deadline)
    }

    fn on_attempt_failure(
        &mut self,
        now: Instant,
        now_utc: DateTime<Utc>,
        message: impl Into<String>,
    ) -> RealtimeReconnectFailure {
        let message = message.into();
        if self.attempt_count >= self.policy.max_attempts || self.should_exhaust(now) {
            self.clear();
            return RealtimeReconnectFailure::Exhausted {
                status: RealtimeChannelStatus {
                    state: RealtimeChannelState::Error,
                    attempt_count: 0,
                    next_retry_at: None,
                    deadline_at: None,
                    reason: Some("realtime reconnect attempts exhausted".to_string()),
                },
                error: RealtimeChannelErrorFrame {
                    code: RealtimeErrorCode::ReconnectExhausted,
                    message: format!("realtime reconnect attempts exhausted: {message}"),
                    details: None,
                },
                close_reason: "reconnect_exhausted".to_string(),
            };
        }

        self.attempt_count += 1;
        self.schedule_next_retry(now, now_utc);
        RealtimeReconnectFailure::RetryScheduled(RealtimeChannelStatus {
            state: RealtimeChannelState::Reconnecting,
            attempt_count: self.attempt_count,
            next_retry_at: self.next_retry_at.map(|deadline| deadline.to_rfc3339()),
            deadline_at: self.deadline_at(),
            reason: Some("realtime attachment requires reattach".to_string()),
        })
    }

    fn schedule_next_retry(&mut self, now: Instant, now_utc: DateTime<Utc>) {
        let backoff = self.backoff_for_attempt(self.attempt_count);
        self.next_retry_deadline = Some(now + backoff);
        self.next_retry_at = Some(
            now_utc
                + chrono::TimeDelta::from_std(backoff)
                    .unwrap_or_else(|_| chrono::TimeDelta::zero()),
        );
    }

    /// Exponential full-jitter backoff: each attempt picks a uniform random
    /// value in `[0, capped_exponential]` so retries from many hosts do not
    /// synchronise on the same moment. Seeded per-channel (deterministic for
    /// tests via `new_with_seed`), drawn fresh on every attempt.
    fn backoff_for_attempt(&mut self, attempt_count: u32) -> Duration {
        let factor = 1u64 << attempt_count.saturating_sub(1).min(20);
        let capped_ms = self
            .policy
            .initial_backoff_ms
            .saturating_mul(factor)
            .min(self.policy.max_backoff_ms);
        let jittered_ms = self.jitter.draw_ms(capped_ms);
        Duration::from_millis(jittered_ms)
    }
}

struct RealtimeProductSessionBridge {
    command_tx: mpsc::Sender<RealtimeProductSessionCommand>,
    update_rx: mpsc::Receiver<RealtimeProductSessionUpdate>,
}

enum RealtimeProductSessionCommand {
    // Transitional: retained for upcoming refresh-via-command wiring; not yet
    // exercised by any call site.
    #[allow(dead_code)]
    RefreshProjection {
        open_config: RealtimeSessionOpenConfig,
        respond: oneshot::Sender<Result<(), RealtimeChannelErrorFrame>>,
    },
    Input {
        chunk: RealtimeInputChunk,
        respond: oneshot::Sender<Result<(), RealtimeChannelErrorFrame>>,
    },
    CommitTurn {
        respond: oneshot::Sender<Result<(), RealtimeChannelErrorFrame>>,
    },
    Interrupt {
        respond: oneshot::Sender<Result<(), RealtimeChannelErrorFrame>>,
    },
    SubmitToolResult {
        result: meerkat_core::ToolResult,
        respond: oneshot::Sender<Result<(), RealtimeChannelErrorFrame>>,
    },
    SubmitToolError {
        call_id: String,
        error: String,
        respond: oneshot::Sender<Result<(), RealtimeChannelErrorFrame>>,
    },
    BargeInTruncate {
        item_id: String,
        content_index: u32,
        audio_played_ms: u64,
        respond: oneshot::Sender<Result<(), RealtimeChannelErrorFrame>>,
    },
    Close,
}

enum RealtimeProductSessionUpdate {
    Event(RealtimeSessionEvent),
    Closed,
    Error {
        error: RealtimeChannelErrorFrame,
        retryable: bool,
    },
}

/// Shared bootstrap/control-plane host for realtime websocket transport.
pub struct RealtimeWsHost {
    ws_url: String,
    supported_protocol_versions: Vec<String>,
    default_protocol_version: String,
    token_ttl: Duration,
    session_factory: Option<Arc<dyn RealtimeSessionFactory>>,
    pending_opens: Mutex<HashMap<String, PendingOpenEntry>>,
    active_targets: Mutex<HashMap<RealtimeTargetKey, ActiveTargetEntry>>,
}

/// Accepted `channel.open` metadata returned after token validation.
#[derive(Debug, Clone)]
pub struct AcceptedRealtimeOpen {
    pub request: RealtimeOpenRequest,
    pub capabilities: RealtimeCapabilities,
    pub protocol_version: String,
}

#[derive(Debug)]
pub struct RegisteredRealtimeOpen {
    connection_id: String,
    target: RealtimeTargetKey,
    role: meerkat_contracts::RealtimeChannelRole,
    observer_fanout_rx: Option<broadcast::Receiver<RealtimeServerFrame>>,
}

/// Open-time websocket handshake failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RealtimeOpenError {
    #[error("invalid open token")]
    InvalidOpenToken,
    #[error("open token expired")]
    OpenTokenExpired,
    #[error("requested role does not match the issued bootstrap token")]
    RoleMismatch,
    #[error("requested turning mode does not match the issued bootstrap token")]
    TurningModeMismatch,
    #[error("requested turning mode is not supported by this target")]
    UnsupportedTurningMode,
    #[error("target already has an active primary realtime channel")]
    TargetBusy,
    #[error("unsupported protocol version '{requested}'")]
    UnsupportedProtocolVersion {
        requested: String,
        supported: Vec<String>,
    },
    /// The open_token was minted under one realm and redeemed under another.
    /// Binding is resolved server-side; the token itself carries no realm
    /// claim, so a mismatch here is always a cross-realm attempt.
    #[error("open token belongs to a different realm and cannot be redeemed here")]
    UnauthorizedRealm,
}

impl RealtimeOpenError {
    /// Typed product-layer error code for websocket `channel.error`.
    #[must_use]
    pub fn code(&self) -> RealtimeErrorCode {
        match self {
            Self::InvalidOpenToken => RealtimeErrorCode::InvalidOpenToken,
            Self::OpenTokenExpired => RealtimeErrorCode::OpenTokenExpired,
            Self::RoleMismatch => RealtimeErrorCode::RoleMismatch,
            Self::TurningModeMismatch => RealtimeErrorCode::TurningModeMismatch,
            Self::UnsupportedTurningMode => RealtimeErrorCode::UnsupportedTurningMode,
            Self::TargetBusy => RealtimeErrorCode::TargetBusy,
            Self::UnsupportedProtocolVersion { .. } => {
                RealtimeErrorCode::UnsupportedProtocolVersion
            }
            Self::UnauthorizedRealm => RealtimeErrorCode::UnauthorizedRealm,
        }
    }

    fn into_error_frame(self) -> RealtimeChannelErrorFrame {
        let code = self.code();
        let message = self.to_string();
        let details = match &self {
            Self::UnsupportedProtocolVersion {
                requested,
                supported,
            } => Some(RealtimeErrorDetails::UnsupportedProtocolVersion {
                requested: requested.clone(),
                supported: supported.clone(),
            }),
            _ => None,
        };
        RealtimeChannelErrorFrame {
            code,
            message,
            details,
        }
    }
}

impl RealtimeWsHost {
    /// Create a new shared websocket bootstrap host for one websocket URL.
    pub fn new(ws_url: impl Into<String>) -> Self {
        let supported = RealtimeProtocolVersion::SUPPORTED
            .iter()
            .map(|version| version.as_str().to_string())
            .collect();
        Self {
            ws_url: ws_url.into(),
            supported_protocol_versions: supported,
            default_protocol_version: RealtimeProtocolVersion::CURRENT.as_str().to_string(),
            token_ttl: DEFAULT_OPEN_TOKEN_TTL,
            session_factory: None,
            pending_opens: Mutex::new(HashMap::new()),
            active_targets: Mutex::new(HashMap::new()),
        }
    }

    /// Override token TTL for deterministic tests or tighter deployments.
    pub fn with_token_ttl(mut self, token_ttl: Duration) -> Self {
        self.token_ttl = token_ttl;
        self
    }

    /// Attach the product-session factory used for provider-created realtime sessions.
    pub fn with_session_factory(
        mut self,
        session_factory: Arc<dyn RealtimeSessionFactory>,
    ) -> Self {
        self.session_factory = Some(session_factory);
        self
    }

    /// Return the configured product-session capability set, if any.
    pub fn session_factory_capabilities(&self) -> Option<RealtimeCapabilities> {
        self.session_factory
            .as_ref()
            .map(|factory| factory.capabilities())
    }

    fn session_factory(&self) -> Option<Arc<dyn RealtimeSessionFactory>> {
        self.session_factory.clone()
    }

    /// Issue bootstrap info for a validated realtime target.
    ///
    /// `realm_id` captures the realm scope at mint time. It is NOT encoded in
    /// the returned token (the token stays opaque); it is stored alongside the
    /// pending entry and compared to the realm observed at
    /// `accept_open_frame_with_realm` time. Pass `None` when the deployment
    /// does not carry a realm context — the token then accepts any realm, a
    /// single-tenant fallback.
    pub async fn issue_open_info(
        &self,
        request: RealtimeOpenRequest,
        capabilities: RealtimeCapabilities,
        realm_id: Option<String>,
    ) -> RealtimeOpenInfo {
        let open_token = Uuid::new_v4().to_string();
        let expires_at = Utc::now()
            + chrono::TimeDelta::from_std(self.token_ttl)
                .unwrap_or_else(|_| chrono::TimeDelta::seconds(60));
        let target = request.target.clone();
        self.pending_opens.lock().await.insert(
            open_token.clone(),
            PendingOpenEntry {
                request,
                capabilities: capabilities.clone(),
                expires_at,
                realm_id,
            },
        );

        RealtimeOpenInfo {
            ws_url: self.ws_url.clone(),
            open_token,
            expires_at: expires_at.to_rfc3339(),
            target,
            supported_protocol_versions: self.supported_protocol_versions.clone(),
            default_protocol_version: self.default_protocol_version.clone(),
            capabilities,
        }
    }

    /// Validate and consume a `channel.open` frame.
    ///
    /// Shorthand for [`accept_open_frame_with_realm`] without any realm check.
    /// Callers in multi-realm deployments should prefer the `with_realm`
    /// variant so a token minted under realm X cannot be redeemed under Y.
    pub async fn accept_open_frame(
        &self,
        frame: &RealtimeChannelOpenFrame,
    ) -> Result<AcceptedRealtimeOpen, RealtimeOpenError> {
        self.accept_open_frame_with_realm(frame, None).await
    }

    /// Validate and consume a `channel.open` frame, enforcing realm scope.
    ///
    /// `observed_realm_id` is the realm the websocket accept path currently
    /// resolves to. When the pending entry was minted under a specific realm
    /// and the observed realm differs, the entry is discarded and
    /// `UnauthorizedRealm` is returned — the opaque token is treated as
    /// cross-realm and cannot be retried. Passing `None` for the observed
    /// realm reduces the check to the legacy single-tenant shape.
    pub async fn accept_open_frame_with_realm(
        &self,
        frame: &RealtimeChannelOpenFrame,
        observed_realm_id: Option<&str>,
    ) -> Result<AcceptedRealtimeOpen, RealtimeOpenError> {
        if !self
            .supported_protocol_versions
            .iter()
            .any(|version| version == &frame.protocol_version)
        {
            return Err(RealtimeOpenError::UnsupportedProtocolVersion {
                requested: frame.protocol_version.clone(),
                supported: self.supported_protocol_versions.clone(),
            });
        }

        let mut pending = self.pending_opens.lock().await;
        let Some(entry) = pending.get(&frame.open_token).cloned() else {
            return Err(RealtimeOpenError::InvalidOpenToken);
        };

        if entry.expires_at < Utc::now() {
            pending.remove(&frame.open_token);
            return Err(RealtimeOpenError::OpenTokenExpired);
        }
        if frame.role != entry.request.role {
            return Err(RealtimeOpenError::RoleMismatch);
        }
        if frame.turning_mode != entry.request.turning_mode {
            return Err(RealtimeOpenError::TurningModeMismatch);
        }
        if !entry
            .capabilities
            .turning_modes
            .contains(&frame.turning_mode)
        {
            return Err(RealtimeOpenError::UnsupportedTurningMode);
        }

        // Realm scope: a token minted in realm X cannot be redeemed in realm
        // Y. Drop the pending entry on mismatch so replaying the token does
        // nothing.
        if entry.realm_id.as_deref() != observed_realm_id {
            pending.remove(&frame.open_token);
            return Err(RealtimeOpenError::UnauthorizedRealm);
        }

        let Some(entry) = pending.remove(&frame.open_token) else {
            return Err(RealtimeOpenError::InvalidOpenToken);
        };
        Ok(AcceptedRealtimeOpen {
            request: entry.request,
            capabilities: entry.capabilities,
            protocol_version: frame.protocol_version.clone(),
        })
    }

    /// Register an accepted open against the canonical target registry.
    pub async fn register_open(
        &self,
        accepted: &AcceptedRealtimeOpen,
    ) -> Result<RegisteredRealtimeOpen, RealtimeOpenError> {
        let target = RealtimeTargetKey::from(&accepted.request.target);
        let connection_id = Uuid::new_v4().to_string();
        let mut active_targets = self.active_targets.lock().await;
        let entry = active_targets.entry(target.clone()).or_default();
        let fanout_tx = entry
            .observer_fanout
            .get_or_insert_with(|| {
                let (tx, _rx) = broadcast::channel(128);
                tx
            })
            .clone();
        match accepted.request.role {
            meerkat_contracts::RealtimeChannelRole::Primary => {
                if entry.primary_connection.is_some() {
                    return Err(RealtimeOpenError::TargetBusy);
                }
                entry.primary_connection = Some(connection_id.clone());
            }
            meerkat_contracts::RealtimeChannelRole::Observer => {
                entry.observer_connections.insert(connection_id.clone());
            }
        }
        Ok(RegisteredRealtimeOpen {
            connection_id,
            target,
            role: accepted.request.role,
            observer_fanout_rx: matches!(
                accepted.request.role,
                meerkat_contracts::RealtimeChannelRole::Observer
            )
            .then(|| fanout_tx.subscribe()),
        })
    }

    /// Release a previously registered open from the canonical target registry.
    pub async fn release_open(&self, registered: &RegisteredRealtimeOpen) {
        let mut active_targets = self.active_targets.lock().await;
        let should_remove = if let Some(entry) = active_targets.get_mut(&registered.target) {
            match registered.role {
                meerkat_contracts::RealtimeChannelRole::Primary => {
                    if entry.primary_connection.as_deref()
                        == Some(registered.connection_id.as_str())
                    {
                        entry.primary_connection = None;
                    }
                }
                meerkat_contracts::RealtimeChannelRole::Observer => {
                    entry
                        .observer_connections
                        .remove(registered.connection_id.as_str());
                }
            }
            entry.primary_connection.is_none() && entry.observer_connections.is_empty()
        } else {
            false
        };

        if should_remove {
            active_targets.remove(&registered.target);
        }
    }

    async fn fanout_observer_frame(
        &self,
        registered: &RegisteredRealtimeOpen,
        frame: &RealtimeServerFrame,
    ) {
        if !matches!(
            registered.role,
            meerkat_contracts::RealtimeChannelRole::Primary
        ) {
            return;
        }
        let active_targets = self.active_targets.lock().await;
        if let Some(entry) = active_targets.get(&registered.target)
            && let Some(tx) = &entry.observer_fanout
        {
            let _ = tx.send(frame.clone());
        }
    }
}

/// Bind and serve the realtime websocket host on `addr`.
pub async fn serve_realtime_ws(
    addr: &str,
    host: Arc<RealtimeWsHost>,
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    serve_realtime_ws_listener(listener, host, runtime, config_store).await
}

/// Serve the realtime websocket host on a pre-bound listener.
pub async fn serve_realtime_ws_listener(
    listener: TcpListener,
    host: Arc<RealtimeWsHost>,
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
) -> std::io::Result<()> {
    let app = Router::new()
        .route(REALTIME_WS_PATH, get(realtime_ws_upgrade))
        .with_state(RealtimeWsState {
            host,
            runtime,
            config_store,
        });
    axum::serve(listener, app).await
}

async fn realtime_ws_upgrade(
    websocket: WebSocketUpgrade,
    State(state): State<RealtimeWsState>,
) -> impl IntoResponse {
    websocket.on_upgrade(move |socket| handle_realtime_socket(socket, state))
}

async fn handle_realtime_socket(mut socket: WebSocket, state: RealtimeWsState) {
    let _canonical_owners = (&state.runtime, &state.config_store);
    let Some(first_message) = socket.recv().await else {
        return;
    };

    let first_message = match first_message {
        Ok(message) => message,
        Err(_) => return,
    };

    match first_message {
        Message::Text(text) => {
            let frame = match serde_json::from_str::<RealtimeClientFrame>(text.as_str()) {
                Ok(frame) => frame,
                Err(error) => {
                    let _ = send_error_and_close(
                        &mut socket,
                        RealtimeChannelErrorFrame {
                            code: RealtimeErrorCode::InvalidFrame,
                            message: format!("failed to parse realtime frame: {error}"),
                            details: None,
                        },
                    )
                    .await;
                    return;
                }
            };

            let RealtimeClientFrame::ChannelOpen(open_frame) = frame else {
                let _ = send_error_and_close(
                    &mut socket,
                    RealtimeChannelErrorFrame {
                        code: RealtimeErrorCode::ExpectedChannelOpen,
                        message: "first realtime websocket frame must be channel.open".to_string(),
                        details: None,
                    },
                )
                .await;
                return;
            };

            let observed_realm_id = state.runtime.realm_id().map(str::to_string);
            match state
                .host
                .accept_open_frame_with_realm(&open_frame, observed_realm_id.as_deref())
                .await
            {
                Ok(accepted) => {
                    let mut registered = match state.host.register_open(&accepted).await {
                        Ok(registered) => registered,
                        Err(error) => {
                            let _ =
                                send_error_and_close(&mut socket, error.into_error_frame()).await;
                            return;
                        }
                    };
                    let (opened_status, binding, mut product_session) = match bind_realtime_target(
                        &state.runtime,
                        &accepted,
                        state.host.session_factory(),
                    )
                    .await
                    {
                        Ok(bound) => bound,
                        Err(error) => {
                            state.host.release_open(&registered).await;
                            let _ = send_error_and_close(&mut socket, error).await;
                            return;
                        }
                    };
                    let uses_product_session = product_session.is_some();
                    let expected_audio_input_format =
                        accepted.capabilities.audio_input_format.clone();
                    let opened = RealtimeServerFrame::ChannelOpened(RealtimeChannelOpenedFrame {
                        protocol_version: accepted.protocol_version,
                        status: opened_status.clone(),
                        capabilities: accepted.capabilities,
                        role: accepted.request.role,
                    });
                    let _ = send_server_frame(&mut socket, &opened).await;
                    let role = registered.role;
                    let turning_mode = accepted.request.turning_mode;
                    let tool_timeout_ms = accepted
                        .request
                        .channel_config
                        .clone()
                        .unwrap_or_default()
                        .tool_timeout_ms_or_default();
                    let mut observer_fanout_rx = registered.observer_fanout_rx.take();
                    let mut reconnect_overlay = RealtimeReconnectOverlay::new(
                        accepted
                            .request
                            .reconnect_policy
                            .clone()
                            .unwrap_or_else(RealtimeReconnectOverlay::default_policy),
                    );
                    let mut cleanup_performed = false;
                    let mut pending_turn = RealtimePendingTurn::default();
                    // Product-session projection discipline:
                    // the provider session is a derived, rebuildable projection
                    // of canonical Meerkat session truth, but it must not be
                    // torn down while the current provider-managed turn is
                    // still semantically in flight. A user transcript commit is
                    // itself a session mutation; if we used that mutation as an
                    // immediate refresh trigger after `TurnCommitted`, we would
                    // rebuild the provider session right before it emits the
                    // assistant response. The DSL-owned
                    // `realtime_product_turn_phase` (U9 / dogma #4) tracks the
                    // turn lifecycle; the shell reads
                    // `RealtimeProductTurnHandle::current_phase` for routing
                    // decisions instead of hand-tracking three booleans.
                    //
                    // Gates for proactive reconnect on clean provider-session
                    // close (`RealtimeProductSessionUpdate::Closed`). A clean
                    // close is treated as a mid-work disconnect worth
                    // proactively re-opening only when the client has issued
                    // work that is still in flight from the channel's
                    // perspective:
                    //   * `client_has_submitted_input` flips to true the
                    //     first time the client's input chunk is accepted by
                    //     the provider session. Before that point the
                    //     channel has no client-requested work to recover,
                    //     so reconnecting would race against callers that
                    //     observe `open_calls` immediately after the
                    //     terminal event (see the tool-call routing test).
                    //   * `last_turn_terminally_completed` flips to true on
                    //     a `TurnCompleted` with a terminal stop reason and
                    //     back to false when a new turn starts. A Close
                    //     after a terminal turn has already satisfied the
                    //     client's request, so the channel can stay idle
                    //     until the client next submits input.
                    // Error paths still reconnect unconditionally because
                    // Error is not a clean close.
                    let mut client_has_submitted_input = false;
                    let mut last_turn_terminally_completed = false;
                    let mut last_visible_status = Some(opened_status);
                    let mut poll_interval = tokio::time::interval(RECONNECT_POLL_INTERVAL);
                    poll_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

                    // W2-E + U9: install typed observers on the session's DSL
                    // handles so `SessionContextAdvanced` effects arrive as
                    // typed notifications instead of a hand-polled watch
                    // channel, and the product-turn lifecycle is owned by the
                    // MeerkatMachine DSL (no shell-local boolean triple). Both
                    // handles share the session's authority via
                    // `prepare_bindings`. `install_observer_with_baseline`
                    // captures the current watermark AND installs the observer
                    // in a single critical section under the DSL authority
                    // lock, so no tick can slip between the sampled baseline
                    // and the observer becoming visible. Separate
                    // `current_watermark_ms` + `install_observer` calls would
                    // re-introduce the original two-read race the handle
                    // abstraction is meant to eliminate.
                    let (projection_refresh_tx, mut projection_refresh_rx) =
                        mpsc::channel::<u64>(16);
                    let realtime_handles =
                        resolve_session_realtime_handles(&state.runtime, binding.as_ref()).await;
                    let session_context_handle =
                        realtime_handles.as_ref().map(|(ctx, _)| Arc::clone(ctx));
                    let product_turn_handle =
                        realtime_handles.as_ref().map(|(_, turn)| Arc::clone(turn));
                    let initial_baseline_ms = if let Some(handle) = session_context_handle.as_ref()
                    {
                        let observer: Arc<
                            dyn meerkat_core::handles::SessionContextAdvancedObserver,
                        > = Arc::new(ProjectionRefreshObserver {
                            notify_tx: projection_refresh_tx.clone(),
                        });
                        handle.install_observer_with_baseline(observer)
                    } else {
                        0
                    };
                    let mut projection_freshness = ProjectionFreshness::Clean {
                        baseline_ms: initial_baseline_ms,
                    };

                    loop {
                        tokio::select! {
                            next = socket.recv() => {
                                let Some(next) = next else {
                                    break;
                                };
                                match next {
                                    Ok(Message::Close(_)) | Err(_) => break,
                                    Ok(Message::Text(text)) => {
                                        let frame = match serde_json::from_str::<RealtimeClientFrame>(
                                            text.as_str(),
                                        ) {
                                            Ok(frame) => frame,
                                            Err(error) => {
                                                let _ = send_server_frame(
                                                    &mut socket,
                                                    &RealtimeServerFrame::ChannelError(
                                                        RealtimeChannelErrorFrame {
                                                            code: RealtimeErrorCode::InvalidFrame,
                                                            message: format!(
                                                                "failed to parse realtime frame: {error}"
                                                            ),
                                                            details: None,
                                                        },
                                                    ),
                                                )
                                                .await;
                                                continue;
                                            }
                                        };
                                        match frame {
                                            RealtimeClientFrame::ChannelClose => {
                                                if let Some(product_session) = product_session.as_mut() {
                                                    let _ = product_session
                                                        .command_tx
                                                        .send(RealtimeProductSessionCommand::Close)
                                                        .await;
                                                }
                                                if let Err(error) = cleanup_realtime_binding(
                                                    &state.runtime,
                                                    binding.as_ref(),
                                                )
                                                .await
                                                {
                                                    cleanup_performed = true;
                                                    let _ = send_server_frame(
                                                        &mut socket,
                                                        &RealtimeServerFrame::ChannelError(error),
                                                    )
                                                    .await;
                                                    let _ = send_server_frame(
                                                        &mut socket,
                                                        &RealtimeServerFrame::ChannelClosed(
                                                            RealtimeChannelClosedFrame {
                                                                reason: Some("close_failed".to_string()),
                                                            },
                                                        ),
                                                    )
                                                    .await;
                                                    break;
                                                }
                                                cleanup_performed = true;
                                                let _ = send_server_frame(
                                                    &mut socket,
                                                    &RealtimeServerFrame::ChannelClosed(
                                                        RealtimeChannelClosedFrame {
                                                            reason: Some("client_closed".to_string()),
                                                        },
                                                    ),
                                                )
                                                .await;
                                                break;
                                            }
                                            RealtimeClientFrame::ChannelInput(_)
                                            | RealtimeClientFrame::ChannelCommitTurn
                                            | RealtimeClientFrame::ChannelInterrupt
                                            | RealtimeClientFrame::ChannelBargeInTruncate(_)
                                                if matches!(
                                                    role,
                                                    meerkat_contracts::RealtimeChannelRole::Observer
                                                ) =>
                                            {
                                                let _ = send_protocol_error(
                                                    &mut socket,
                                                    RealtimeErrorCode::ObserverReadOnly,
                                                    "observer channels may not send input or control frames",
                                                )
                                                .await;
                                            }
                                            RealtimeClientFrame::ChannelOpen(_) => {
                                                let _ = send_protocol_error(
                                                    &mut socket,
                                                    RealtimeErrorCode::UnexpectedChannelOpen,
                                                    "channel.open is only valid as the first realtime websocket frame",
                                                )
                                                .await;
                                            }
                                            RealtimeClientFrame::ChannelCommitTurn
                                                if !matches!(
                                                    turning_mode,
                                                    meerkat_contracts::RealtimeTurningMode::ExplicitCommit
                                                ) =>
                                            {
                                                let _ = send_protocol_error(
                                                    &mut socket,
                                                    RealtimeErrorCode::CommitTurnUnavailable,
                                                    "channel.commit_turn is only valid for explicit_commit channels",
                                                )
                                                .await;
                                            }
                                            RealtimeClientFrame::ChannelBargeInTruncate(frame) => {
                                                // Barge-in must reach the
                                                // provider no matter what —
                                                // if the product session is
                                                // reconnecting or absent we
                                                // still acknowledge the client
                                                // by noting the intent in the
                                                // pending projection so the
                                                // canonical session can
                                                // project the truncation once
                                                // the session comes back.
                                                if let Some(product_session) = product_session.as_mut() {
                                                    let (respond_tx, respond_rx) = oneshot::channel();
                                                    let _ = product_session
                                                        .command_tx
                                                        .send(RealtimeProductSessionCommand::BargeInTruncate {
                                                            item_id: frame.item_id,
                                                            content_index: frame.content_index,
                                                            audio_played_ms: frame.audio_played_ms,
                                                            respond: respond_tx,
                                                        })
                                                        .await;
                                                    match respond_rx.await {
                                                        Ok(Ok(())) => {}
                                                        Ok(Err(error)) => {
                                                            let _ = send_server_frame(
                                                                &mut socket,
                                                                &RealtimeServerFrame::ChannelError(error),
                                                            )
                                                            .await;
                                                        }
                                                        Err(_) => {
                                                            let _ = send_protocol_error(
                                                                &mut socket,
                                                                RealtimeErrorCode::ProviderSessionClosed,
                                                                "realtime provider session closed before barge-in truncation completed",
                                                            )
                                                            .await;
                                                        }
                                                    }
                                                } else if uses_product_session {
                                                    let _ = send_protocol_error(
                                                        &mut socket,
                                                        RealtimeErrorCode::ChannelReconnecting,
                                                        "realtime provider session is reconnecting; retry barge-in once ready",
                                                    )
                                                    .await;
                                                }
                                                // Non-product-session paths
                                                // (bare session target without
                                                // an OpenAI session) silently
                                                // ignore — there is no
                                                // provider transcript to
                                                // truncate.
                                            }
                                            RealtimeClientFrame::ChannelInterrupt => {
                                                if let Some(product_session) = product_session.as_mut() {
                                                    let (respond_tx, respond_rx) = oneshot::channel();
                                                    let _ = product_session
                                                        .command_tx
                                                        .send(RealtimeProductSessionCommand::Interrupt {
                                                            respond: respond_tx,
                                                        })
                                                        .await;
                                                    match respond_rx.await {
                                                        Ok(Ok(())) => {}
                                                        Ok(Err(error)) => {
                                                            let _ = send_server_frame(
                                                                &mut socket,
                                                                &RealtimeServerFrame::ChannelError(error),
                                                            )
                                                            .await;
                                                        }
                                                        Err(_) => {
                                                            let _ = send_protocol_error(
                                                                &mut socket,
                                                                RealtimeErrorCode::ProviderSessionClosed,
                                                                "realtime provider session closed before interrupt completed",
                                                            )
                                                            .await;
                                                        }
                                                    }
                                                } else if uses_product_session {
                                                    let _ = send_protocol_error(
                                                        &mut socket,
                                                        RealtimeErrorCode::ChannelReconnecting,
                                                        "realtime provider session is reconnecting; wait for the channel to become ready",
                                                    )
                                                    .await;
                                                } else if let Some(RealtimeSocketBinding::SessionPrimary {
                                                    session_id,
                                                }) = binding.as_ref()
                                                {
                                                    if let Err(error) = state
                                                        .runtime
                                                        .runtime_adapter()
                                                        .interrupt_current_run(session_id)
                                                        .await
                                                    {
                                                        let _ = send_server_frame(
                                                            &mut socket,
                                                            &RealtimeServerFrame::ChannelError(
                                                                runtime_error_frame(error, "interrupt"),
                                                            ),
                                                        )
                                                        .await;
                                                    }
                                                } else {
                                                    let _ = send_protocol_error(
                                                        &mut socket,
                                                        RealtimeErrorCode::ChannelNotBound,
                                                        "realtime frame routing is not wired to the substrate yet",
                                                    )
                                                    .await;
                                                }
                                            }
                                            RealtimeClientFrame::ChannelInput(input_frame) => {
                                                if let Err(error) = validate_input_chunk_audio_format(
                                                    &input_frame.chunk,
                                                    &expected_audio_input_format,
                                                ) {
                                                    let _ = send_server_frame(
                                                        &mut socket,
                                                        &RealtimeServerFrame::ChannelError(error),
                                                    )
                                                    .await;
                                                    continue;
                                                }
                                                if let Some(product_session) = product_session.as_mut() {
                                                    let preempt = product_turn_handle
                                                        .as_ref()
                                                        .is_some_and(|h| h.should_preempt_on_input());
                                                    if preempt
                                                        && std::env::var_os(
                                                            "RKAT_OPENAI_REALTIME_TRACE_LIFECYCLE",
                                                        )
                                                        .is_some()
                                                    {
                                                        let phase = product_turn_handle
                                                            .as_ref()
                                                            .map(|h| h.current_phase());
                                                        eprintln!(
                                                            "[realtime-ws-input] preempt triggered: phase={phase:?}",
                                                        );
                                                    }
                                                    if preempt {
                                                        let (respond_tx, respond_rx) = oneshot::channel();
                                                        let _ = product_session
                                                            .command_tx
                                                            .send(RealtimeProductSessionCommand::Interrupt {
                                                                respond: respond_tx,
                                                            })
                                                            .await;
                                                        match respond_rx.await {
                                                            Ok(Ok(())) => {
                                                                if let Some(handle) =
                                                                    product_turn_handle.as_ref()
                                                                {
                                                                    let _ = handle.turn_terminal();
                                                                }
                                                            }
                                                            Ok(Err(error))
                                                                if preemptive_interrupt_can_be_ignored(&error) =>
                                                            {
                                                                if let Some(handle) =
                                                                    product_turn_handle.as_ref()
                                                                {
                                                                    let _ = handle.turn_terminal();
                                                                }
                                                            }
                                                            Ok(Err(error)) => {
                                                                let _ = send_server_frame(
                                                                    &mut socket,
                                                                    &RealtimeServerFrame::ChannelError(error),
                                                                )
                                                                .await;
                                                                continue;
                                                            }
                                                            Err(_) => {
                                                                let _ = send_protocol_error(
                                                                    &mut socket,
                                                                    RealtimeErrorCode::ProviderSessionClosed,
                                                                    "realtime provider session closed before turn preemption completed",
                                                                )
                                                                .await;
                                                                continue;
                                                            }
                                                        }
                                                    }
                                                    let turn_was_idle = product_turn_handle
                                                        .as_ref()
                                                        .is_none_or(|h| !h.is_in_flight());
                                                    // Barge-in continuity:
                                                    // after preempting the in-flight response, the
                                                    // caller is about to stream the audio/text for the
                                                    // new user turn onto the *same* provider session.
                                                    // Tearing the provider session down here would
                                                    // reopen a fresh session without the barge-in
                                                    // context and cause the stop audio to be processed
                                                    // as a brand-new, unrelated turn — which defeats
                                                    // the interruption semantics entirely. Refresh only
                                                    // when this input chunk is arriving on a cleanly
                                                    // idle provider session.
                                                    if !preempt
                                                        && turn_was_idle
                                                        && projection_freshness.is_stale_immediate()
                                                    {
                                                        // Derived provider projections should refresh
                                                        // only when canonical Meerkat state has
                                                        // actually changed since the last successful
                                                        // reconstruction. A freshly opened product
                                                        // session is already seeded from the current
                                                        // canonical state, so rebuilding again on the
                                                        // very first input chunk only widens the race
                                                        // surface for member bridge-session
                                                        // replacement without improving semantic
                                                        // correctness.
                                                        if let Err(error) = refresh_product_session_projection(
                                                            &state.runtime,
                                                            binding.as_ref(),
                                                            turning_mode,
                                                            state.host.session_factory.clone(),
                                                            product_session,
                                                        )
                                                        .await
                                                        {
                                                            let _ = send_server_frame(
                                                                &mut socket,
                                                                &RealtimeServerFrame::ChannelError(error),
                                                            )
                                                            .await;
                                                            continue;
                                                        }
                                                        // W2-E: refresh landed. Advance the typed
                                                        // freshness state to `Clean { baseline }`
                                                        // using the current DSL watermark. No
                                                        // hand-maintained dirty flag.
                                                        let watermark_ms = session_context_handle
                                                            .as_ref()
                                                            .map(|h| h.current_watermark_ms())
                                                            .unwrap_or(0);
                                                        projection_freshness =
                                                            projection_freshness.on_refreshed(watermark_ms);
                                                    }
                                                    let (respond_tx, respond_rx) = oneshot::channel();
                                                    let _ = product_session
                                                        .command_tx
                                                        .send(RealtimeProductSessionCommand::Input {
                                                            chunk: input_frame.chunk,
                                                            respond: respond_tx,
                                                        })
                                                        .await;
                                                    match respond_rx.await {
                                                        Ok(Ok(())) => {
                                                            // The DSL `ProductTurnInFlight`
                                                            // transition is guard-rejected when
                                                            // already non-idle, so this single
                                                            // fire handles both "starting new
                                                            // turn" and "continuing within the
                                                            // same turn" paths. Handle returns
                                                            // `Ok(false)` on the continuation case.
                                                            if let Some(handle) =
                                                                product_turn_handle.as_ref()
                                                            {
                                                                let _ = handle.turn_in_flight();
                                                            }
                                                            // Client-initiated work has now
                                                            // started on the provider session.
                                                            // Any subsequent clean close while
                                                            // this flag is set AND no terminal
                                                            // turn completion has cleared the
                                                            // mid-work gate is treated as a
                                                            // mid-work disconnect, so the poll
                                                            // loop can reconnect proactively.
                                                            client_has_submitted_input = true;
                                                            last_turn_terminally_completed = false;
                                                        }
                                                        Ok(Err(error)) => {
                                                            let _ = send_server_frame(
                                                                &mut socket,
                                                                &RealtimeServerFrame::ChannelError(error),
                                                            )
                                                            .await;
                                                        }
                                                        Err(_) => {
                                                            let _ = send_protocol_error(
                                                                &mut socket,
                                                                RealtimeErrorCode::ProviderSessionClosed,
                                                                "realtime provider session closed before input was accepted",
                                                            )
                                                            .await;
                                                        }
                                                    }
                                                } else if uses_product_session {
                                                    let _ = send_protocol_error(
                                                        &mut socket,
                                                        RealtimeErrorCode::ChannelReconnecting,
                                                        "realtime provider session is reconnecting; wait for the channel to become ready",
                                                    )
                                                    .await;
                                                } else {
                                                    match handle_channel_input(
                                                        &state.runtime,
                                                        binding.as_ref(),
                                                        turning_mode,
                                                        &mut pending_turn,
                                                        input_frame.chunk,
                                                    )
                                                    .await
                                                    {
                                                        Ok(frames) => {
                                                            for frame in frames {
                                                                let _ = send_server_frame_with_fanout(
                                                                    &mut socket,
                                                                    &frame,
                                                                    Some((state.host.as_ref(), &registered)),
                                                                )
                                                                .await;
                                                            }
                                                        }
                                                        Err(error) => {
                                                            let _ = send_server_frame(
                                                                &mut socket,
                                                                &RealtimeServerFrame::ChannelError(error),
                                                            )
                                                            .await;
                                                        }
                                                    }
                                                }
                                            }
                                            RealtimeClientFrame::ChannelCommitTurn => {
                                                if let Some(product_session) = product_session.as_mut() {
                                                    let (respond_tx, respond_rx) = oneshot::channel();
                                                    let _ = product_session
                                                        .command_tx
                                                        .send(RealtimeProductSessionCommand::CommitTurn {
                                                            respond: respond_tx,
                                                        })
                                                        .await;
                                                    match respond_rx.await {
                                                        Ok(Ok(())) => {}
                                                        Ok(Err(error)) => {
                                                            let _ = send_server_frame(
                                                                &mut socket,
                                                                &RealtimeServerFrame::ChannelError(error),
                                                            )
                                                            .await;
                                                        }
                                                        Err(_) => {
                                                            let _ = send_protocol_error(
                                                                &mut socket,
                                                                RealtimeErrorCode::ProviderSessionClosed,
                                                                "realtime provider session closed before commit_turn completed",
                                                            )
                                                            .await;
                                                        }
                                                    }
                                                } else if uses_product_session {
                                                    let _ = send_protocol_error(
                                                        &mut socket,
                                                        RealtimeErrorCode::ChannelReconnecting,
                                                        "realtime provider session is reconnecting; wait for the channel to become ready",
                                                    )
                                                    .await;
                                                } else {
                                                    match commit_pending_turn(
                                                        &state.runtime,
                                                        binding.as_ref(),
                                                        &mut pending_turn,
                                                    )
                                                    .await
                                                    {
                                                        Ok(frames) => {
                                                            for frame in frames {
                                                                let _ = send_server_frame_with_fanout(
                                                                    &mut socket,
                                                                    &frame,
                                                                    Some((state.host.as_ref(), &registered)),
                                                                )
                                                                .await;
                                                            }
                                                        }
                                                        Err(error) => {
                                                            let _ = send_server_frame(
                                                                &mut socket,
                                                                &RealtimeServerFrame::ChannelError(error),
                                                            )
                                                            .await;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Ok(_) => {}
                                }
                            }
                            refresh = projection_refresh_rx.recv() => {
                                // W2-E: typed `SessionContextAdvanced` effect
                                // arrived from the DSL handle's observer. Fold
                                // into `projection_freshness`, then drain any
                                // siblings that queued while we were awaiting
                                // (the DSL can emit back-to-back advances
                                // during a single summary publish burst, so
                                // coalesce to the highest watermark).
                                let Some(updated_at_ms) = refresh else {
                                    continue;
                                };
                                let mut max_notified = updated_at_ms;
                                while let Ok(next_notified) = projection_refresh_rx.try_recv() {
                                    if next_notified > max_notified {
                                        max_notified = next_notified;
                                    }
                                }
                                let turn_in_flight = product_turn_handle
                                    .as_ref()
                                    .is_some_and(|h| h.is_in_flight());
                                projection_freshness = projection_freshness
                                    .on_context_advanced(max_notified, turn_in_flight);
                                if let Some(product_session) = product_session.as_mut()
                                    && projection_freshness.is_stale_immediate()
                                {
                                    if let Err(error) = refresh_product_session_projection(
                                        &state.runtime,
                                        binding.as_ref(),
                                        turning_mode,
                                        state.host.session_factory.clone(),
                                        product_session,
                                    )
                                    .await
                                    {
                                        let _ = send_server_frame(
                                            &mut socket,
                                            &RealtimeServerFrame::ChannelError(error),
                                        )
                                        .await;
                                    } else {
                                        let watermark_ms = session_context_handle
                                            .as_ref()
                                            .map(|h| h.current_watermark_ms())
                                            .unwrap_or(0);
                                        projection_freshness =
                                            projection_freshness.on_refreshed(watermark_ms);
                                    }
                                }
                            }
                            update = async {
                                match product_session.as_mut() {
                                    Some(product_session) => product_session.update_rx.recv().await,
                                    None => std::future::pending().await,
                                }
                            } => {
                                match update {
                                    Some(RealtimeProductSessionUpdate::Event(event)) => {
                                        if std::env::var_os("RKAT_OPENAI_REALTIME_TRACE_LIFECYCLE").is_some() {
                                            let tag = match &event {
                                                RealtimeSessionEvent::TurnStarted => Some("turn_started"),
                                                RealtimeSessionEvent::TurnCommitted => Some("turn_committed"),
                                                RealtimeSessionEvent::TurnCompleted { stop_reason, .. } => {
                                                    eprintln!(
                                                        "[realtime-ws-update] turn_completed stop_reason={stop_reason:?}",
                                                    );
                                                    None
                                                }
                                                RealtimeSessionEvent::InputTranscriptFinal { .. } => Some("input_transcript_final"),
                                                RealtimeSessionEvent::Interrupted => Some("interrupted"),
                                                RealtimeSessionEvent::ToolCallRequested { .. } => Some("tool_call_requested"),
                                                _ => None,
                                            };
                                            if let Some(tag) = tag {
                                                eprintln!("[realtime-ws-update] {tag}");
                                            }
                                        }
                                        let lifecycle = classify_product_session_event(&event);
                                        match match event {
                                            RealtimeSessionEvent::ToolCallRequested {
                                                call_id,
                                                tool_name,
                                                arguments,
                                            } => {
                                                handle_product_session_tool_call(
                                                    &state.runtime,
                                                    binding.as_ref(),
                                                    product_session.as_mut(),
                                                    call_id,
                                                    tool_name,
                                                    arguments,
                                                    tool_timeout_ms,
                                                )
                                                .await
                                            }
                                            other => {
                                                handle_product_session_event(
                                                    &state.runtime,
                                                    binding.as_ref(),
                                                    &mut pending_turn,
                                                    other,
                                                )
                                                .await
                                            }
                                        } {
                                            Ok(frames) => {
                                                for frame in frames {
                                                    let _ = send_server_frame_with_fanout(
                                                        &mut socket,
                                                        &frame,
                                                        Some((state.host.as_ref(), &registered)),
                                                    )
                                                    .await;
                                                }
                                            }
                                            Err(error) => {
                                                if std::env::var_os(
                                                    "RKAT_OPENAI_REALTIME_TRACE_LIFECYCLE",
                                                )
                                                .is_some()
                                                {
                                                    eprintln!(
                                                        "[realtime-ws-emit] ERROR: {}",
                                                        error.code
                                                    );
                                                }
                                                let _ = send_server_frame(
                                                    &mut socket,
                                                    &RealtimeServerFrame::ChannelError(error),
                                                )
                                                .await;
                                            }
                                        }
                                        // W2-E: own-turn commits (user transcript append,
                                        // assistant-output append, tool-dispatch mutation) also
                                        // emit `SessionContextAdvanced` effects, same as external
                                        // mutations do. Clear any pending stale state those ticks
                                        // may have left behind — our own-turn commit is not an
                                        // external mutation the provider session needs to
                                        // absorb. Without this, a queued observer tick could
                                        // have transitioned state to `StaleDeferred` before the
                                        // `update` arm ran, and the subsequent
                                        // `logical_turn_completed` promotion would fire a
                                        // spurious provider-session rebuild for our own-turn
                                        // transcript commit (the fix the old
                                        // `known == current => clear dirty` check encoded).
                                        if lifecycle.advances_projection_known_state {
                                            let watermark_ms = session_context_handle
                                                .as_ref()
                                                .map(|h| h.current_watermark_ms())
                                                .unwrap_or(0);
                                            // Drain any queued observer notifies so they don't
                                            // land in the wrong phase. Pass `false` so they
                                            // don't transition into `StaleDeferred` — the DSL
                                            // already recorded this advance, and we're about to
                                            // consume it via `on_refreshed` below.
                                            while projection_refresh_rx.try_recv().is_ok() {
                                                // Intentionally discarded. The canonical DSL
                                                // watermark is authoritative; the observer just
                                                // signalled us to look.
                                            }
                                            // Reset to `Clean` at the current DSL watermark. This
                                            // supersedes any `StaleDeferred { new_at_ms }` that
                                            // might have been set by an observer notify that
                                            // arrived before this `update` arm ran.
                                            projection_freshness =
                                                projection_freshness.on_refreshed(watermark_ms);
                                        }
                                        if let Some(handle) = product_turn_handle.as_ref() {
                                            // U9: fire typed lifecycle inputs in the order the
                                            // event carries them. A provider-issued tool call is
                                            // both output_started (it triggers assistant-side
                                            // progress) and an in-flight turn marker; the DSL
                                            // handles both via two guard-rejected-idempotent
                                            // fires.
                                            if lifecycle.turn_committed {
                                                let _ = handle.turn_committed();
                                            }
                                            if lifecycle.tool_call_requested {
                                                // A provider-issued tool call is part of the
                                                // currently active provider-managed turn even if
                                                // the backend never emitted an explicit
                                                // TurnStarted marker. Fire in-flight so a
                                                // `ToolCallRequested`-only turn (no prior Input)
                                                // doesn't look idle to the projection gate.
                                                let _ = handle.turn_in_flight();
                                            }
                                            if lifecycle.output_started {
                                                let _ = handle.output_started();
                                            }
                                            if lifecycle.interrupted {
                                                let _ = handle.turn_interrupted();
                                            }
                                        }
                                        if lifecycle.tool_call_requested {
                                            // A provider-issued tool call is not yet a terminal
                                            // turn completion, so a subsequent clean close must
                                            // not treat the preceding turn as "finished" when
                                            // deciding whether to auto-reconnect.
                                            last_turn_terminally_completed = false;
                                        }
                                        if lifecycle.logical_turn_completed {
                                            if let Some(handle) = product_turn_handle.as_ref() {
                                                let _ = handle.turn_terminal();
                                            }
                                            // Record that the last in-flight turn reached a
                                            // terminal stop reason so a subsequent clean close
                                            // is treated as "session finished the requested
                                            // work" rather than a mid-turn drop.
                                            last_turn_terminally_completed = true;
                                            // W2-E: promote `StaleDeferred` → `StaleImmediate`
                                            // at turn end. This is the specific fix for the s71
                                            // turn-8 regression: a peer-response terminal
                                            // advanced the session mid-turn while the provider
                                            // turn was live, landing as `StaleDeferred`; the old
                                            // hand-maintained flag missed promoting it on turn
                                            // end because the drain gate checked
                                            // `projection_refresh_dirty` rather than the
                                            // typed stale state.
                                            //
                                            // Drain any queued sibling notifies before
                                            // transitioning so the promoted new_at carries the
                                            // highest observed watermark.
                                            while let Ok(notified_at) =
                                                projection_refresh_rx.try_recv()
                                            {
                                                projection_freshness = projection_freshness
                                                    .on_context_advanced(notified_at, true);
                                            }
                                            projection_freshness =
                                                projection_freshness.on_turn_completed();
                                            if projection_freshness.is_stale_immediate()
                                                && let Some(product_session) =
                                                    product_session.as_mut()
                                            {
                                                if let Err(error) =
                                                    refresh_product_session_projection(
                                                        &state.runtime,
                                                        binding.as_ref(),
                                                        turning_mode,
                                                        state.host.session_factory.clone(),
                                                        product_session,
                                                    )
                                                    .await
                                                {
                                                    let _ = send_server_frame(
                                                        &mut socket,
                                                        &RealtimeServerFrame::ChannelError(error),
                                                    )
                                                    .await;
                                                } else {
                                                    let watermark_ms = session_context_handle
                                                        .as_ref()
                                                        .map(|h| h.current_watermark_ms())
                                                        .unwrap_or(0);
                                                    projection_freshness = projection_freshness
                                                        .on_refreshed(watermark_ms);
                                                }
                                            }
                                        }
                                    }
                                    Some(RealtimeProductSessionUpdate::Closed) => {
                                        if let Some(handle) = product_turn_handle.as_ref() {
                                            let _ = handle.turn_terminal();
                                        }
                                        // W2-E: the observer remains installed on the
                                        // session's DSL handle — no separate task to abort.
                                        // The freshness state resets to the current
                                        // watermark so a fresh provider session opens
                                        // with a `Clean` baseline.
                                        let watermark_ms = session_context_handle
                                            .as_ref()
                                            .map(|h| h.current_watermark_ms())
                                            .unwrap_or(0);
                                        projection_freshness = ProjectionFreshness::Clean {
                                            baseline_ms: watermark_ms,
                                        };
                                        product_session = None;
                                        // Only flip the binding into
                                        // `ReattachRequired` — which kicks off
                                        // the poll-driven reconnect cycle —
                                        // when the client has actually issued
                                        // work against this session AND that
                                        // work has not yet reached a terminal
                                        // turn completion. Without any client
                                        // input, a clean close is just
                                        // "provider ended the session" with
                                        // nothing to resume, and proactively
                                        // re-opening would race against
                                        // callers that read `open_calls`
                                        // immediately after the terminal
                                        // event (e.g. the tool-call routing
                                        // coverage test). If the last turn
                                        // already completed terminally, the
                                        // provider delivered everything the
                                        // client asked for — a follow-up
                                        // clean close is the session
                                        // finishing, not a mid-turn drop, so
                                        // there is nothing to recover.
                                        let needs_reattach =
                                            client_has_submitted_input && !last_turn_terminally_completed;
                                        if needs_reattach
                                            && let Err(error) = require_product_session_reattach(
                                                &state.runtime,
                                                binding.as_ref(),
                                            )
                                            .await
                                        {
                                            let _ = send_server_frame(
                                                &mut socket,
                                                &RealtimeServerFrame::ChannelError(error),
                                            )
                                            .await;
                                            let _ = send_server_frame(
                                                &mut socket,
                                                &RealtimeServerFrame::ChannelClosed(
                                                    RealtimeChannelClosedFrame {
                                                        reason: Some(
                                                            RealtimeErrorCode::ProviderSessionClosed.to_string(),
                                                        ),
                                                    },
                                                ),
                                            )
                                            .await;
                                            break;
                                        }
                                    }
                                    Some(RealtimeProductSessionUpdate::Error { error, retryable }) => {
                                        if let Some(handle) = product_turn_handle.as_ref() {
                                            let _ = handle.turn_terminal();
                                        }
                                        // W2-E: observer lifetime is bound to the DSL
                                        // handle; no separate task to abort. Reset
                                        // freshness to a fresh `Clean` baseline.
                                        let watermark_ms = session_context_handle
                                            .as_ref()
                                            .map(|h| h.current_watermark_ms())
                                            .unwrap_or(0);
                                        projection_freshness = ProjectionFreshness::Clean {
                                            baseline_ms: watermark_ms,
                                        };
                                        product_session = None;
                                        if retryable {
                                            if let Err(error) = require_product_session_reattach(
                                                &state.runtime,
                                                binding.as_ref(),
                                            )
                                            .await
                                            {
                                                let _ = send_server_frame(
                                                    &mut socket,
                                                    &RealtimeServerFrame::ChannelError(error),
                                                )
                                                .await;
                                                let _ = send_server_frame(
                                                    &mut socket,
                                                    &RealtimeServerFrame::ChannelClosed(
                                                        RealtimeChannelClosedFrame {
                                                            reason: Some(
                                                                "provider_session_failed".to_string(),
                                                            ),
                                                        },
                                                    ),
                                                )
                                                .await;
                                                break;
                                            }
                                        } else {
                                            let close_reason = error.code.as_str().to_string();
                                            let _ = send_server_frame(
                                                &mut socket,
                                                &RealtimeServerFrame::ChannelError(error),
                                            )
                                            .await;
                                            let _ = send_server_frame(
                                                &mut socket,
                                                &RealtimeServerFrame::ChannelClosed(
                                                    RealtimeChannelClosedFrame {
                                                        reason: Some(close_reason),
                                                    },
                                                ),
                                            )
                                            .await;
                                            break;
                                        }
                                    }
                                    None => {}
                                }
                            }
                            observer_frame = async {
                                match observer_fanout_rx.as_mut() {
                                    Some(rx) => match rx.recv().await {
                                        Ok(frame) => Some(frame),
                                        Err(broadcast::error::RecvError::Lagged(_)) => None,
                                        Err(broadcast::error::RecvError::Closed) => None,
                                    },
                                    None => std::future::pending().await,
                                }
                            } => {
                                if let Some(frame) = observer_frame {
                                    let _ = send_server_frame(&mut socket, &frame).await;
                                }
                            }
                            _ = poll_interval.tick() => {
                                let now = Instant::now();
                                let now_utc = Utc::now();
                                match current_binding_projection(&state.runtime, binding.as_ref()).await {
                                    Ok(RealtimeBindingProjection::ReattachRequired)
                                        if matches!(role, meerkat_contracts::RealtimeChannelRole::Primary) =>
                                    {
                                        if let Some(status) = reconnect_overlay.begin_if_needed(now, now_utc) {
                                            let _ = emit_status_update(
                                                &mut socket,
                                                &mut last_visible_status,
                                                status,
                                                true,
                                                Some((state.host.as_ref(), &registered)),
                                            )
                                            .await;
                                        }

                                        if reconnect_overlay.should_exhaust(now) {
                                            let exhausted = reconnect_overlay.on_attempt_failure(
                                                now,
                                                now_utc,
                                                "realtime reconnect budget expired before the next retry",
                                            );
                                            if let RealtimeReconnectFailure::Exhausted {
                                                status,
                                                error,
                                                close_reason,
                                            } = exhausted
                                            {
                                                let _ = emit_status_update(
                                                    &mut socket,
                                                    &mut last_visible_status,
                                                    status,
                                                    false,
                                                    Some((state.host.as_ref(), &registered)),
                                                )
                                                .await;
                                                let _ = send_server_frame(
                                                    &mut socket,
                                                    &RealtimeServerFrame::ChannelError(error),
                                                )
                                                .await;
                                                let _ = send_server_frame(
                                                    &mut socket,
                                                    &RealtimeServerFrame::ChannelClosed(
                                                        RealtimeChannelClosedFrame {
                                                            reason: Some(close_reason),
                                                        },
                                                    ),
                                                )
                                                .await;
                                                break;
                                            }
                                        } else if reconnect_overlay.attempt_due(now) {
                                            match attempt_realtime_reconnect(
                                                &state.runtime,
                                                binding.as_ref(),
                                                turning_mode,
                                                state.host.session_factory(),
                                            )
                                            .await
                                            {
                                                Ok(new_product_session) => {
                                                    if let Some(new_product_session) = new_product_session {
                                                        product_session = Some(new_product_session);
                                                        // W2-E: observer is installed on the
                                                        // session's DSL handle for the
                                                        // socket's lifetime; reconnect
                                                        // doesn't require re-installing it.
                                                        // Reset the freshness baseline to
                                                        // the current DSL watermark so the
                                                        // rebuilt provider session starts
                                                        // `Clean`.
                                                        let watermark_ms = session_context_handle
                                                            .as_ref()
                                                            .map(|h| h.current_watermark_ms())
                                                            .unwrap_or(0);
                                                        projection_freshness = ProjectionFreshness::Clean {
                                                            baseline_ms: watermark_ms,
                                                        };
                                                    }
                                                    if let Ok(projection) = current_binding_projection(
                                                        &state.runtime,
                                                        binding.as_ref(),
                                                    )
                                                    .await
                                                        && projection != RealtimeBindingProjection::ReattachRequired
                                                    {
                                                        reconnect_overlay.clear();
                                                        let _ = emit_status_update(
                                                            &mut socket,
                                                            &mut last_visible_status,
                                                            projection_to_channel_status(projection),
                                                            false,
                                                            Some((state.host.as_ref(), &registered)),
                                                        )
                                                        .await;
                                                    }
                                                }
                                                Err(error) => {
                                                    match reconnect_overlay.on_attempt_failure(
                                                        now,
                                                        now_utc,
                                                        error.message.clone(),
                                                    ) {
                                                        RealtimeReconnectFailure::RetryScheduled(status) => {
                                                            let _ = emit_status_update(
                                                                &mut socket,
                                                                &mut last_visible_status,
                                                                status,
                                                                false,
                                                                Some((state.host.as_ref(), &registered)),
                                                            )
                                                            .await;
                                                        }
                                                        RealtimeReconnectFailure::Exhausted {
                                                            status,
                                                            error,
                                                            close_reason,
                                                        } => {
                                                            let _ = emit_status_update(
                                                                &mut socket,
                                                                &mut last_visible_status,
                                                                status,
                                                                false,
                                                                Some((state.host.as_ref(), &registered)),
                                                            )
                                                            .await;
                                                            let _ = send_server_frame(
                                                                &mut socket,
                                                                &RealtimeServerFrame::ChannelError(error),
                                                            )
                                                            .await;
                                                            let _ = send_server_frame(
                                                                &mut socket,
                                                                &RealtimeServerFrame::ChannelClosed(
                                                                    RealtimeChannelClosedFrame {
                                                                        reason: Some(close_reason),
                                                                    },
                                                                ),
                                                            )
                                                            .await;
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Ok(projection) => {
                                        reconnect_overlay.clear();
                                        let _ = emit_status_update(
                                            &mut socket,
                                            &mut last_visible_status,
                                            projection_to_channel_status(projection),
                                            false,
                                            Some((state.host.as_ref(), &registered)),
                                        )
                                        .await;
                                    }
                                    Err(error) => {
                                        let _ = send_server_frame(
                                            &mut socket,
                                            &RealtimeServerFrame::ChannelError(error),
                                        )
                                        .await;
                                        let _ = send_server_frame(
                                            &mut socket,
                                            &RealtimeServerFrame::ChannelClosed(
                                                RealtimeChannelClosedFrame {
                                                    reason: Some("status_failed".to_string()),
                                                },
                                            ),
                                        )
                                        .await;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if !cleanup_performed {
                        let _ = cleanup_realtime_binding(&state.runtime, binding.as_ref()).await;
                    }
                    // W2-E: observer lifetime is bound to `session_context_handle`
                    // which drops here. The DSL-side `install_observer` stores a
                    // `Weak`, so downgraded references become no-ops after this
                    // drop — no separate task to abort.
                    drop(session_context_handle);
                    state.host.release_open(&registered).await;
                }
                Err(error) => {
                    let _ = send_error_and_close(&mut socket, error.into_error_frame()).await;
                }
            }
        }
        Message::Close(_) => {}
        _ => {
            let _ = send_error_and_close(
                &mut socket,
                RealtimeChannelErrorFrame {
                    code: RealtimeErrorCode::ExpectedChannelOpen,
                    message: "first realtime websocket frame must be channel.open".to_string(),
                    details: None,
                },
            )
            .await;
        }
    }
}

async fn send_server_frame(socket: &mut WebSocket, frame: &RealtimeServerFrame) -> Result<(), ()> {
    let payload = serde_json::to_string(frame).map_err(|_| ())?;
    socket
        .send(Message::Text(payload.into()))
        .await
        .map_err(|_| ())
}

async fn send_server_frame_with_fanout(
    socket: &mut WebSocket,
    frame: &RealtimeServerFrame,
    fanout: Option<(&RealtimeWsHost, &RegisteredRealtimeOpen)>,
) -> Result<(), ()> {
    send_server_frame(socket, frame).await?;
    if matches!(
        frame,
        RealtimeServerFrame::ChannelStatus(_) | RealtimeServerFrame::ChannelEvent(_)
    ) && let Some((host, registered)) = fanout
    {
        host.fanout_observer_frame(registered, frame).await;
    }
    Ok(())
}

/// Validate an incoming realtime input chunk against the channel's negotiated
/// audio format, if any. Returns `Ok(())` when the chunk is non-audio or
/// matches; returns a typed channel error frame for any mismatch.
fn validate_input_chunk_audio_format(
    chunk: &RealtimeInputChunk,
    expected: &Option<RealtimeAudioFormat>,
) -> Result<(), RealtimeChannelErrorFrame> {
    let RealtimeInputChunk::AudioChunk(audio) = chunk else {
        return Ok(());
    };
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = audio.format();
    if actual.mime_type == expected.mime_type
        && actual.sample_rate_hz == expected.sample_rate_hz
        && actual.channels == expected.channels
    {
        return Ok(());
    }
    Err(RealtimeChannelErrorFrame {
        code: RealtimeErrorCode::AudioFormatMismatch,
        message: format!(
            "audio input format does not match provider negotiated format: \
             expected {} @ {} Hz / {} ch, got {} @ {} Hz / {} ch",
            expected.mime_type,
            expected.sample_rate_hz,
            expected.channels,
            actual.mime_type,
            actual.sample_rate_hz,
            actual.channels,
        ),
        details: Some(RealtimeErrorDetails::AudioFormatMismatch(
            AudioFormatMismatchContext {
                expected: expected.clone(),
                actual,
            },
        )),
    })
}

async fn send_protocol_error(
    socket: &mut WebSocket,
    code: RealtimeErrorCode,
    message: &str,
) -> Result<(), ()> {
    send_server_frame(
        socket,
        &RealtimeServerFrame::ChannelError(RealtimeChannelErrorFrame {
            code,
            message: message.to_string(),
            details: None,
        }),
    )
    .await
}

async fn bind_realtime_target(
    runtime: &SessionRuntime,
    accepted: &AcceptedRealtimeOpen,
    session_factory: Option<Arc<dyn RealtimeSessionFactory>>,
) -> Result<
    (
        RealtimeChannelStatus,
        Option<RealtimeSocketBinding>,
        Option<RealtimeProductSessionBridge>,
    ),
    RealtimeChannelErrorFrame,
> {
    match &accepted.request.target {
        meerkat_contracts::RealtimeChannelTarget::SessionTarget { session_id } => {
            let session_id =
                SessionId::parse(session_id).map_err(|err| RealtimeChannelErrorFrame {
                    code: RealtimeErrorCode::InvalidTarget,
                    message: format!("invalid session target: {err}"),
                    details: None,
                })?;
            if matches!(
                accepted.request.role,
                meerkat_contracts::RealtimeChannelRole::Primary
            ) {
                if let Some(session_factory) = session_factory {
                    let (status, bridge) = open_product_session_bridge(
                        runtime,
                        &session_id,
                        accepted.request.turning_mode,
                        session_factory,
                    )
                    .await?;
                    Ok((
                        status,
                        Some(RealtimeSocketBinding::SessionPrimary { session_id }),
                        Some(bridge),
                    ))
                } else {
                    runtime
                        .runtime_adapter()
                        .attach_live(&session_id)
                        .await
                        .map_err(|err| runtime_error_frame(err, "attach"))?;
                    let status = runtime
                        .runtime_adapter()
                        .realtime_attachment_status(&session_id)
                        .await
                        .map(session_projection_from_runtime)
                        .map(projection_to_channel_status)
                        .map_err(|err| runtime_error_frame(err, "status"))?;
                    Ok((
                        status,
                        Some(RealtimeSocketBinding::SessionPrimary { session_id }),
                        None,
                    ))
                }
            } else {
                let status = runtime
                    .runtime_adapter()
                    .realtime_attachment_status(&session_id)
                    .await
                    .map(session_projection_from_runtime)
                    .map(projection_to_channel_status)
                    .map_err(|err| runtime_error_frame(err, "status"))?;
                Ok((
                    status,
                    Some(RealtimeSocketBinding::SessionObserver { session_id }),
                    None,
                ))
            }
        }
    }
}

async fn open_product_session_bridge(
    runtime: &SessionRuntime,
    session_id: &SessionId,
    turning_mode: meerkat_contracts::RealtimeTurningMode,
    session_factory: Arc<dyn RealtimeSessionFactory>,
) -> Result<(RealtimeChannelStatus, RealtimeProductSessionBridge), RealtimeChannelErrorFrame> {
    let open_config = runtime
        .realtime_session_open_config(session_id, turning_mode)
        .await
        .map_err(session_error_frame)?;
    let authority = runtime
        .runtime_adapter()
        .attach_live(session_id)
        .await
        .map_err(|err| runtime_error_frame(err, "attach"))?;
    let session = match session_factory.open_session(&open_config).await {
        Ok(session) => session,
        Err(error) => {
            let _ = runtime.runtime_adapter().detach_live(session_id).await;
            return Err(realtime_client_error_frame(error, "open"));
        }
    };
    if let Err(error) = runtime
        .runtime_adapter()
        .publish_realtime_attachment_signal(
            authority.clone(),
            meerkat_runtime::RealtimeAttachmentStatus::BindingReady,
        )
        .await
    {
        let _ = runtime.runtime_adapter().detach_live(session_id).await;
        return Err(runtime_error_frame(error, "bind_ready"));
    }

    let (command_tx, command_rx) = mpsc::channel(32);
    let (update_tx, update_rx) = mpsc::channel(128);
    tokio::spawn(run_product_session_actor(session, command_rx, update_tx));

    let status = runtime
        .runtime_adapter()
        .realtime_attachment_status(session_id)
        .await
        .map(session_projection_from_runtime)
        .map(projection_to_channel_status)
        .map_err(|err| runtime_error_frame(err, "status"))?;
    Ok((
        status,
        RealtimeProductSessionBridge {
            command_tx,
            update_rx,
        },
    ))
}

async fn cleanup_realtime_binding(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
) -> Result<(), RealtimeChannelErrorFrame> {
    match binding {
        Some(RealtimeSocketBinding::SessionPrimary { session_id }) => runtime
            .runtime_adapter()
            .detach_live(session_id)
            .await
            .map_err(|err| runtime_error_frame(err, "detach")),
        Some(RealtimeSocketBinding::SessionObserver { session_id }) => {
            let _ = session_id;
            Ok(())
        }
        None => Ok(()),
    }
}

fn projection_to_channel_status(projection: RealtimeBindingProjection) -> RealtimeChannelStatus {
    match projection {
        RealtimeBindingProjection::Unattached => RealtimeChannelStatus {
            state: RealtimeChannelState::Closed,
            attempt_count: 0,
            next_retry_at: None,
            deadline_at: None,
            reason: Some("no realtime channel is open for this target".to_string()),
        },
        RealtimeBindingProjection::IntentPresentUnbound
        | RealtimeBindingProjection::BindingNotReady => RealtimeChannelStatus {
            state: RealtimeChannelState::Opening,
            attempt_count: 0,
            next_retry_at: None,
            deadline_at: None,
            reason: Some("realtime attachment is pending".to_string()),
        },
        RealtimeBindingProjection::BindingReady => RealtimeChannelStatus {
            state: RealtimeChannelState::Ready,
            attempt_count: 0,
            next_retry_at: None,
            deadline_at: None,
            reason: None,
        },
        RealtimeBindingProjection::ReplacementPending => RealtimeChannelStatus {
            state: RealtimeChannelState::Reconnecting,
            attempt_count: 0,
            next_retry_at: None,
            deadline_at: None,
            reason: Some("realtime attachment replacement is pending".to_string()),
        },
        RealtimeBindingProjection::ReattachRequired => RealtimeChannelStatus {
            state: RealtimeChannelState::Reconnecting,
            attempt_count: 0,
            next_retry_at: None,
            deadline_at: None,
            reason: Some("realtime attachment requires reattach".to_string()),
        },
    }
}

async fn current_binding_projection(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
) -> Result<RealtimeBindingProjection, RealtimeChannelErrorFrame> {
    match binding {
        Some(
            RealtimeSocketBinding::SessionPrimary { session_id }
            | RealtimeSocketBinding::SessionObserver { session_id },
        ) => runtime
            .runtime_adapter()
            .realtime_attachment_status(session_id)
            .await
            .map(session_projection_from_runtime)
            .map_err(|err| runtime_error_frame(err, "status")),
        None => Err(RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::ChannelNotBound,
            message: "realtime frame routing is not wired to the substrate yet".to_string(),
            details: None,
        }),
    }
}

fn session_projection_from_runtime(
    status: meerkat_runtime::RealtimeAttachmentStatus,
) -> RealtimeBindingProjection {
    match status {
        meerkat_runtime::RealtimeAttachmentStatus::Unattached => {
            RealtimeBindingProjection::Unattached
        }
        meerkat_runtime::RealtimeAttachmentStatus::IntentPresentUnbound => {
            RealtimeBindingProjection::IntentPresentUnbound
        }
        meerkat_runtime::RealtimeAttachmentStatus::BindingNotReady => {
            RealtimeBindingProjection::BindingNotReady
        }
        meerkat_runtime::RealtimeAttachmentStatus::BindingReady => {
            RealtimeBindingProjection::BindingReady
        }
        meerkat_runtime::RealtimeAttachmentStatus::ReplacementPending => {
            RealtimeBindingProjection::ReplacementPending
        }
        meerkat_runtime::RealtimeAttachmentStatus::ReattachRequired => {
            RealtimeBindingProjection::ReattachRequired
        }
    }
}

async fn emit_status_update(
    socket: &mut WebSocket,
    last_visible_status: &mut Option<RealtimeChannelStatus>,
    status: RealtimeChannelStatus,
    emit_needs_reattach: bool,
    fanout: Option<(&RealtimeWsHost, &RegisteredRealtimeOpen)>,
) -> Result<(), ()> {
    if last_visible_status.as_ref() == Some(&status) {
        return Ok(());
    }
    *last_visible_status = Some(status.clone());
    let status_frame = RealtimeServerFrame::ChannelStatus(RealtimeChannelStatusFrame {
        status: status.clone(),
    });
    let status_changed_frame = channel_event(RealtimeEvent::StatusChanged {
        status: status.clone(),
    });
    send_server_frame(socket, &status_frame).await?;
    send_server_frame(socket, &status_changed_frame).await?;
    if let Some((host, registered)) = fanout {
        host.fanout_observer_frame(registered, &status_frame).await;
        host.fanout_observer_frame(registered, &status_changed_frame)
            .await;
    }
    if emit_needs_reattach {
        let needs_reattach = channel_event(RealtimeEvent::NeedsReattach);
        send_server_frame(socket, &needs_reattach).await?;
        if let Some((host, registered)) = fanout {
            host.fanout_observer_frame(registered, &needs_reattach)
                .await;
        }
    }
    Ok(())
}

async fn attempt_realtime_reconnect(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
    turning_mode: meerkat_contracts::RealtimeTurningMode,
    session_factory: Option<Arc<dyn RealtimeSessionFactory>>,
) -> Result<Option<RealtimeProductSessionBridge>, RealtimeChannelErrorFrame> {
    match binding {
        Some(RealtimeSocketBinding::SessionPrimary { session_id }) => {
            if let Some(session_factory) = session_factory {
                let (_status, bridge) =
                    open_product_session_bridge(runtime, session_id, turning_mode, session_factory)
                        .await?;
                Ok(Some(bridge))
            } else {
                runtime
                    .runtime_adapter()
                    .attach_live(session_id)
                    .await
                    .map_err(|err| runtime_error_frame(err, "reattach"))?;
                Ok(None)
            }
        }
        Some(RealtimeSocketBinding::SessionObserver { .. }) | None => {
            Err(RealtimeChannelErrorFrame {
                code: RealtimeErrorCode::ChannelNotBound,
                message: "observer channels do not own realtime reconnect attempts".to_string(),
                details: None,
            })
        }
    }
}

fn channel_event(event: RealtimeEvent) -> RealtimeServerFrame {
    RealtimeServerFrame::ChannelEvent(RealtimeChannelEventFrame { event })
}

/// Typed classification of a [`RealtimeSessionEvent`] into the
/// lifecycle-meaningful aspects the realtime-WS dispatch loop consumes.
///
/// The three "meaningful for turn lifecycle" bits (committed /
/// output_started / interrupted / tool_call) are fed into the
/// [`RealtimeProductTurnHandle`] as typed DSL inputs; the projection-
/// refresh bit (`advances_projection_known_state`) feeds the freshness
/// state machine. Centralising the classification here keeps the dispatch
/// loop free of `matches!` folklore over provider-session event variants.
#[derive(Debug, Clone, Copy, Default)]
struct ProductSessionEventLifecycle {
    /// This event advances canonical session-context truth, so the
    /// realtime projection baseline should be re-seeded on observation.
    advances_projection_known_state: bool,
    /// Equivalent to the old `logical_turn_completed` — a `TurnCompleted`
    /// whose stop reason is logically terminal for the provider turn.
    logical_turn_completed: bool,
    /// The provider just issued a tool call.
    tool_call_requested: bool,
    /// `TurnCommitted` arrived.
    turn_committed: bool,
    /// An output delta / chunk / tool call arrived (any visible
    /// assistant-side progress).
    output_started: bool,
    /// The provider interrupted the in-flight turn.
    interrupted: bool,
}

fn classify_product_session_event(event: &RealtimeSessionEvent) -> ProductSessionEventLifecycle {
    let logical_turn_completed = matches!(
        event,
        RealtimeSessionEvent::TurnCompleted { stop_reason, .. }
            if product_turn_completion_is_logically_terminal(*stop_reason)
    );
    let advances_projection_known_state = matches!(
        event,
        RealtimeSessionEvent::TurnCommitted
            | RealtimeSessionEvent::ToolCallRequested { .. }
            // InputTranscriptFinal is the canonical-append point for audio
            // turns (the transcript-final event mutates the runtime session
            // history). The projection baseline must advance here or the
            // async mutation signal would set `StaleDeferred` for an own-
            // turn mutation and cause a spurious provider-session reopen at
            // turn end.
            | RealtimeSessionEvent::InputTranscriptFinal { .. }
    ) || logical_turn_completed;
    let tool_call_requested = matches!(event, RealtimeSessionEvent::ToolCallRequested { .. });
    let turn_committed = matches!(event, RealtimeSessionEvent::TurnCommitted);
    let output_started = matches!(
        event,
        RealtimeSessionEvent::OutputTextDelta { .. }
            | RealtimeSessionEvent::OutputAudioChunk { .. }
            | RealtimeSessionEvent::OutputVideoChunk { .. }
            | RealtimeSessionEvent::ToolCallRequested { .. }
    );
    let interrupted = matches!(event, RealtimeSessionEvent::Interrupted);
    ProductSessionEventLifecycle {
        advances_projection_known_state,
        logical_turn_completed,
        tool_call_requested,
        turn_committed,
        output_started,
        interrupted,
    }
}

async fn require_product_session_reattach(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
) -> Result<(), RealtimeChannelErrorFrame> {
    match binding {
        Some(RealtimeSocketBinding::SessionPrimary { session_id }) => runtime
            .runtime_adapter()
            .require_realtime_attachment_reattach(session_id)
            .await
            .map_err(|err| runtime_error_frame(err, "reattach")),
        Some(RealtimeSocketBinding::SessionObserver { .. }) | None => Ok(()),
    }
}

async fn run_product_session_actor(
    mut session: Box<dyn meerkat_client::RealtimeSession>,
    mut command_rx: mpsc::Receiver<RealtimeProductSessionCommand>,
    update_tx: mpsc::Sender<RealtimeProductSessionUpdate>,
) {
    loop {
        tokio::select! {
            command = command_rx.recv() => {
                let Some(command) = command else {
                    let _ = session.close().await;
                    break;
                };
                match command {
                    RealtimeProductSessionCommand::RefreshProjection {
                        open_config,
                        respond,
                    } => {
                        let _ = respond.send(
                            session
                                .refresh_projection(&open_config)
                                .await
                                .map_err(|error| realtime_client_error_frame(error, "refresh_projection"))
                        );
                    }
                    RealtimeProductSessionCommand::Input { chunk, respond } => {
                        let _ = respond.send(
                            session
                                .send_input(chunk)
                                .await
                                .map_err(|error| realtime_client_error_frame(error, "input"))
                        );
                    }
                    RealtimeProductSessionCommand::CommitTurn { respond } => {
                        let _ = respond.send(
                            session
                                .commit_turn()
                                .await
                                .map_err(|error| realtime_client_error_frame(error, "commit_turn"))
                        );
                    }
                    RealtimeProductSessionCommand::Interrupt { respond } => {
                        let _ = respond.send(
                            session
                                .interrupt()
                                .await
                                .map_err(|error| realtime_client_error_frame(error, "interrupt"))
                        );
                    }
                    RealtimeProductSessionCommand::SubmitToolResult { result, respond } => {
                        let _ = respond.send(
                            session
                                .submit_tool_result(result)
                                .await
                                .map_err(|error| realtime_client_error_frame(error, "tool_result"))
                        );
                    }
                    RealtimeProductSessionCommand::SubmitToolError {
                        call_id,
                        error,
                        respond,
                    } => {
                        let _ = respond.send(
                            session
                                .submit_tool_error(call_id, error)
                                .await
                                .map_err(|error| realtime_client_error_frame(error, "tool_error"))
                        );
                    }
                    RealtimeProductSessionCommand::BargeInTruncate {
                        item_id,
                        content_index,
                        audio_played_ms,
                        respond,
                    } => {
                        let _ = respond.send(
                            session
                                .truncate_assistant_output(item_id, content_index, audio_played_ms)
                                .await
                                .map_err(|error| {
                                    realtime_client_error_frame(error, "barge_in_truncate")
                                })
                        );
                    }
                    RealtimeProductSessionCommand::Close => {
                        let _ = session.close().await;
                        break;
                    }
                }
            }
            next_event = session.next_event() => {
                match next_event {
                    Ok(Some(event)) => {
                        if update_tx
                            .send(RealtimeProductSessionUpdate::Event(event))
                            .await
                            .is_err()
                        {
                            let _ = session.close().await;
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = update_tx.send(RealtimeProductSessionUpdate::Closed).await;
                        break;
                    }
                    Err(error) => {
                        let _ = update_tx
                            .send(RealtimeProductSessionUpdate::Error {
                                error: realtime_client_error_frame(error.clone(), "event_pump"),
                                retryable: error.is_retryable(),
                            })
                            .await;
                        break;
                    }
                }
            }
        }
    }
}

async fn handle_product_session_tool_call(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
    product_session: Option<&mut RealtimeProductSessionBridge>,
    call_id: String,
    tool_name: String,
    arguments: serde_json::Value,
    tool_timeout_ms: Option<u64>,
) -> Result<Vec<RealtimeServerFrame>, RealtimeChannelErrorFrame> {
    let mut frames = vec![channel_event(RealtimeEvent::ToolCallRequested {
        call_id: call_id.clone(),
        tool_name: tool_name.clone(),
    })];
    let session_id = resolve_primary_session_id(
        runtime,
        binding,
        "realtime product session is not wired to a session target",
    )
    .await?;
    let Some(product_session) = product_session else {
        return Err(RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::ProviderSessionClosed,
            message: "realtime provider session closed before tool continuation could run"
                .to_string(),
            details: None,
        });
    };

    let call = meerkat_core::ToolCall::new(call_id.clone(), tool_name, arguments);
    let dispatch = runtime.dispatch_external_tool_call(&session_id, call);
    let started_at = meerkat_core::time_compat::Instant::now();
    let outcome_result = match tool_timeout_ms {
        Some(limit_ms) => {
            match tokio::time::timeout(std::time::Duration::from_millis(limit_ms), dispatch).await {
                Ok(inner) => inner,
                Err(_elapsed) => {
                    // Budget exceeded: inject a synthetic tool-error result so
                    // the model observes a concrete failure, then emit the
                    // typed timeout event. Dropping the `dispatch` future
                    // cancels the underlying task so a late-finishing tool
                    // does not leak its result onto a cancelled channel.
                    let actual_elapsed_ms =
                        u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
                    let timeout_message = format!(
                        "tool exceeded budget after {actual_elapsed_ms}ms, continuing without result",
                    );
                    let _ = submit_product_session_tool_error(
                        runtime,
                        binding,
                        product_session,
                        call_id.clone(),
                        timeout_message.clone(),
                    )
                    .await;
                    frames.push(channel_event(RealtimeEvent::ToolCallTimedOut {
                        call_id,
                        elapsed_ms: actual_elapsed_ms,
                    }));
                    return Ok(frames);
                }
            }
        }
        None => dispatch.await,
    };

    match outcome_result {
        Ok(outcome) => match submit_product_session_tool_result(
            runtime,
            binding,
            product_session,
            outcome.result,
        )
        .await
        {
            Ok(()) => {
                frames.push(channel_event(RealtimeEvent::ToolCallCompleted { call_id }));
                Ok(frames)
            }
            Err(error) => {
                let error_message = error.message;
                frames.push(channel_event(RealtimeEvent::ToolCallFailed {
                    call_id,
                    error: error_message,
                }));
                Ok(frames)
            }
        },
        Err(error) => {
            let error_message = error.to_string();
            let _ = submit_product_session_tool_error(
                runtime,
                binding,
                product_session,
                call_id.clone(),
                error_message.clone(),
            )
            .await;
            frames.push(channel_event(RealtimeEvent::ToolCallFailed {
                call_id,
                error: error_message,
            }));
            Ok(frames)
        }
    }
}

async fn submit_product_session_tool_result(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
    product_session: &mut RealtimeProductSessionBridge,
    result: meerkat_core::ToolResult,
) -> Result<(), RealtimeChannelErrorFrame> {
    let (respond_tx, respond_rx) = oneshot::channel();
    product_session
        .command_tx
        .send(RealtimeProductSessionCommand::SubmitToolResult {
            result,
            respond: respond_tx,
        })
        .await
        .map_err(|_| RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::ProviderSessionClosed,
            message: "realtime provider session closed before the tool result could be submitted"
                .to_string(),
            details: None,
        })?;
    match respond_rx.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            let _ = require_product_session_reattach(runtime, binding).await;
            Err(error)
        }
        Err(_) => {
            let _ = require_product_session_reattach(runtime, binding).await;
            Err(RealtimeChannelErrorFrame {
                code: RealtimeErrorCode::ProviderSessionClosed,
                message: "realtime provider session closed before the tool result was accepted"
                    .to_string(),
                details: None,
            })
        }
    }
}

async fn submit_product_session_tool_error(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
    product_session: &mut RealtimeProductSessionBridge,
    call_id: String,
    error: String,
) -> Result<(), RealtimeChannelErrorFrame> {
    let (respond_tx, respond_rx) = oneshot::channel();
    product_session
        .command_tx
        .send(RealtimeProductSessionCommand::SubmitToolError {
            call_id,
            error,
            respond: respond_tx,
        })
        .await
        .map_err(|_| RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::ProviderSessionClosed,
            message: "realtime provider session closed before the tool failure could be submitted"
                .to_string(),
            details: None,
        })?;
    match respond_rx.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            let _ = require_product_session_reattach(runtime, binding).await;
            Err(error)
        }
        Err(_) => {
            let _ = require_product_session_reattach(runtime, binding).await;
            Err(RealtimeChannelErrorFrame {
                code: RealtimeErrorCode::ProviderSessionClosed,
                message: "realtime provider session closed before the tool failure was accepted"
                    .to_string(),
                details: None,
            })
        }
    }
}

async fn handle_product_session_event(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
    pending_turn: &mut RealtimePendingTurn,
    event: RealtimeSessionEvent,
) -> Result<Vec<RealtimeServerFrame>, RealtimeChannelErrorFrame> {
    match event {
        RealtimeSessionEvent::InputTranscriptPartial { text } => {
            pending_turn.staged_user_text = text.clone();
            Ok(vec![channel_event(RealtimeEvent::InputTranscriptPartial {
                text,
            })])
        }
        RealtimeSessionEvent::InputTranscriptFinal { text } => {
            // Canonical-history append on transcript finalization, not on
            // TurnCommitted. For OpenAI audio the provider emits
            // `input_audio_buffer.committed` *before* transcription
            // completes, so relying on TurnCommitted's staged text would
            // either leak the prior turn's transcript or drop this turn's
            // transcript entirely. The transcript-final event is the earliest
            // point where the full user utterance is known; use it as the
            // append trigger and clear the staging slot so the subsequent
            // TurnCommitted handler does not re-append.
            let session_id = resolve_primary_session_id(
                runtime,
                binding,
                "realtime product session is not wired to a session target",
            )
            .await?;
            runtime
                .append_external_user_content(
                    &session_id,
                    meerkat_core::types::ContentInput::Text(text.clone()),
                )
                .await
                .map_err(session_error_frame)?;
            pending_turn.staged_user_text.clear();
            Ok(vec![channel_event(RealtimeEvent::InputTranscriptFinal {
                text,
                prosody_hint: None,
            })])
        }
        RealtimeSessionEvent::TurnStarted => Ok(vec![channel_event(RealtimeEvent::TurnStarted)]),
        RealtimeSessionEvent::TurnCommitted => {
            let session_id = resolve_primary_session_id(
                runtime,
                binding,
                "realtime product session is not wired to a session target",
            )
            .await?;
            if pending_turn.staged_user_text.is_empty() {
                // Either the transcript-final handler already appended the
                // canonical user turn and cleared staging, or the provider
                // committed with no transcript at all.
                return Ok(vec![channel_event(RealtimeEvent::TurnCommitted)]);
            }
            // Fallback: transcript partials accumulated but no final event
            // arrived before the commit. Append the best-known staged text so
            // the canonical history still records *something* for this turn.
            let text = std::mem::take(&mut pending_turn.staged_user_text);
            append_external_user_transcript(runtime, &session_id, text, false).await
        }
        RealtimeSessionEvent::TurnCompleted { stop_reason, usage } => {
            match product_turn_completion_disposition(stop_reason) {
                RealtimeTurnCompletionDisposition::Finalize => {
                    let session_id = resolve_primary_session_id(
                        runtime,
                        binding,
                        "realtime product session is not wired to a session target",
                    )
                    .await?;
                    let assistant_text = std::mem::take(&mut pending_turn.staged_assistant_text);
                    append_external_assistant_output(
                        runtime,
                        &session_id,
                        assistant_text,
                        stop_reason,
                        usage,
                    )
                    .await
                }
                RealtimeTurnCompletionDisposition::SuppressKeepStaged => Ok(Vec::new()),
                RealtimeTurnCompletionDisposition::SuppressDiscardStaged => {
                    pending_turn.staged_assistant_text.clear();
                    Ok(Vec::new())
                }
            }
        }
        RealtimeSessionEvent::OutputTextDelta { delta } => {
            pending_turn.staged_assistant_text.push_str(&delta);
            Ok(vec![channel_event(RealtimeEvent::OutputTextDelta {
                delta,
            })])
        }
        RealtimeSessionEvent::OutputAudioChunk { chunk } => {
            Ok(vec![channel_event(RealtimeEvent::OutputAudioChunk {
                chunk,
            })])
        }
        RealtimeSessionEvent::OutputVideoChunk { chunk } => {
            Ok(vec![channel_event(RealtimeEvent::OutputVideoChunk {
                chunk,
            })])
        }
        RealtimeSessionEvent::Interrupted => {
            pending_turn.staged_assistant_text.clear();
            Ok(vec![channel_event(RealtimeEvent::Interrupted)])
        }
        RealtimeSessionEvent::ToolCallRequested {
            call_id, tool_name, ..
        } => Ok(vec![channel_event(RealtimeEvent::ToolCallRequested {
            call_id,
            tool_name,
        })]),
        RealtimeSessionEvent::AssistantTranscriptTruncated {
            item_id,
            audio_played_ms,
            truncated_text,
        } => {
            // Canonical-history projection: replace the staged assistant text
            // with the heard prefix so the next turn's projection seeds from
            // what the user actually heard. If the provider did not supply a
            // re-projected text, leave existing staging and let the next
            // TurnCompleted event finalize from whatever the provider
            // eventually surfaces.
            if let Some(text) = truncated_text.clone() {
                pending_turn.staged_assistant_text = text;
            }
            Ok(vec![channel_event(
                RealtimeEvent::AssistantTranscriptTruncated {
                    item_id,
                    audio_played_ms,
                    truncated_text,
                },
            )])
        }
    }
}

async fn resolve_primary_session_id(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
    not_bound_message: &str,
) -> Result<SessionId, RealtimeChannelErrorFrame> {
    let _ = runtime;
    match binding {
        Some(RealtimeSocketBinding::SessionPrimary { session_id }) => Ok(session_id.clone()),
        _ => Err(RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::ChannelNotBound,
            message: not_bound_message.to_string(),
            details: None,
        }),
    }
}

async fn refresh_product_session_projection(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
    turning_mode: meerkat_contracts::RealtimeTurningMode,
    session_factory: Option<Arc<dyn RealtimeSessionFactory>>,
    product_session: &mut RealtimeProductSessionBridge,
) -> Result<(), RealtimeChannelErrorFrame> {
    let session_factory = session_factory.ok_or_else(|| RealtimeChannelErrorFrame {
        code: RealtimeErrorCode::ProviderSessionUnavailable,
        message: "realtime provider session factory is not available for projection reconstruction"
            .to_string(),
        details: None,
    })?;
    let session_id = resolve_primary_session_id(
        runtime,
        binding,
        "realtime product session is not wired to a session target",
    )
    .await?;
    let open_config = runtime
        .realtime_session_open_config(&session_id, turning_mode)
        .await
        .map_err(session_error_frame)?;
    // Reconstruction, not patching:
    // OpenAI's realtime SessionUpdate can refresh instructions and tools, but
    // it does not rebuild the provider's accumulated conversation state from
    // canonical Meerkat history. When authoritative Meerkat state changes
    // asynchronously between turns (for example, terminal peer responses
    // accepted into runtime system context), the dogma-correct text-first
    // solution is to rebuild the provider session from canonical Meerkat
    // truth rather than carry a stale provider-side conversation cache.
    let _ = product_session
        .command_tx
        .send(RealtimeProductSessionCommand::Close)
        .await;
    let session = session_factory
        .open_session(&open_config)
        .await
        .map_err(|error| realtime_client_error_frame(error, "reconstruct_open"))?;
    let (command_tx, command_rx) = mpsc::channel(32);
    let (update_tx, update_rx) = mpsc::channel(128);
    tokio::spawn(run_product_session_actor(session, command_rx, update_tx));
    *product_session = RealtimeProductSessionBridge {
        command_tx,
        update_rx,
    };
    Ok(())
}

/// Typed projection-freshness state for the realtime socket (W2-E / issue #264).
///
/// Replaces the hand-wired `projection_refresh_dirty: bool` +
/// `projection_known_updated_at: SystemTime` pair with a single state that
/// carries the same information typed. Transitions are driven by three
/// inputs:
///   1. `SessionContextAdvanced { updated_at_ms }` effect arrival (observer
///      notify): advance `Clean` baseline if idle; mark `StaleDeferred` if
///      a turn is in flight; mark `StaleImmediate` otherwise.
///   2. Turn-lifecycle transition (`logical_turn_completed`): promote
///      `StaleDeferred` to `StaleImmediate` and drain.
///   3. Input-chunk arrival with an idle product-turn phase
///      (`!RealtimeProductTurnHandle::is_in_flight`): drain
///      `StaleImmediate` at the canonical refresh site.
///
/// The discriminant is always one of:
///   - `Clean { baseline }`: provider projection matches canonical session
///     state as of `baseline`. No refresh owed.
///   - `StaleDeferred { new_at }`: canonical state advanced to `new_at`
///     while the provider turn is live; refresh blocked until the turn
///     terminates so barge-in continuity isn't broken.
///   - `StaleImmediate { new_at }`: refresh owed at the next drain site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectionFreshness {
    Clean { baseline_ms: u64 },
    StaleDeferred { new_at_ms: u64 },
    StaleImmediate { new_at_ms: u64 },
}

impl ProjectionFreshness {
    fn baseline_ms(&self) -> u64 {
        match *self {
            Self::Clean { baseline_ms } => baseline_ms,
            Self::StaleDeferred { new_at_ms } | Self::StaleImmediate { new_at_ms } => new_at_ms,
        }
    }

    /// Transition on a `SessionContextAdvanced` observer tick.
    ///
    /// `turn_in_flight`: whether the provider-managed turn is currently
    /// running. When `true`, the advance is recorded as `StaleDeferred`
    /// so barge-in continuity is preserved. When `false`, it's recorded
    /// as `StaleImmediate` so the next drain site picks it up.
    ///
    /// Ticks below the current baseline are ignored (non-monotonic —
    /// shouldn't happen because the DSL guard filters them, but belt +
    /// suspenders at the consumer too).
    fn on_context_advanced(self, advanced_at_ms: u64, turn_in_flight: bool) -> Self {
        if advanced_at_ms <= self.baseline_ms() {
            return self;
        }
        if turn_in_flight {
            Self::StaleDeferred {
                new_at_ms: advanced_at_ms,
            }
        } else {
            // Promote any deferred pending-stale into immediate so the
            // consumer refreshes at the next available drain site.
            Self::StaleImmediate {
                new_at_ms: advanced_at_ms,
            }
        }
    }

    /// Transition on `logical_turn_completed`.
    ///
    /// `StaleDeferred` → `StaleImmediate` so the turn-end drain site
    /// picks up the pending refresh. This is the specific fix for the
    /// s71 turn-8 regression (peer-response terminal advanced the
    /// session mid-turn, which used to land as a dirty flag that
    /// never drained until the next input chunk).
    fn on_turn_completed(self) -> Self {
        match self {
            Self::StaleDeferred { new_at_ms } => Self::StaleImmediate { new_at_ms },
            other => other,
        }
    }

    /// Transition after a successful refresh drain.
    fn on_refreshed(self, now_ms: u64) -> Self {
        // `now_ms` should be the updated_at_ms read back from the session
        // immediately post-refresh (which advances on our own commit).
        // Fall back to the previous new_at if we didn't get one — the
        // important invariant is "no longer stale".
        let baseline_ms = match self {
            Self::Clean { baseline_ms } => baseline_ms.max(now_ms),
            Self::StaleDeferred { new_at_ms } | Self::StaleImmediate { new_at_ms } => {
                new_at_ms.max(now_ms)
            }
        };
        Self::Clean { baseline_ms }
    }

    fn is_stale_immediate(&self) -> bool {
        matches!(self, Self::StaleImmediate { .. })
    }
}

/// Observer installed on the session's [`SessionContextHandle`] to push
/// `SessionContextAdvanced` effect emissions into the realtime socket's
/// event loop. The loop consumes these as a `tokio::select` arm, same
/// place the old `projection_refresh_rx` sat.
struct ProjectionRefreshObserver {
    notify_tx: mpsc::Sender<u64>,
}

impl meerkat_core::handles::SessionContextAdvancedObserver for ProjectionRefreshObserver {
    fn on_session_context_advanced(&self, updated_at_ms: u64) {
        // `try_send`: the observer is invoked under the DSL authority
        // lock (see `HandleDslAuthority::apply_input_with_effects`). A
        // blocking send here would risk deadlock; dropping a tick on a
        // full queue is fine because the loop coalesces any backlog via
        // `projection_refresh_rx.try_recv()` before processing, and the
        // DSL guarantees monotonic watermarks so the next observed tick
        // supersedes any we missed.
        let _ = self.notify_tx.try_send(updated_at_ms);
    }
}

/// Resolve the session's realtime DSL handles (context-refresh +
/// product-turn lifecycle) for this socket by re-preparing bindings via
/// the runtime adapter.
///
/// Returns `None` when the socket has no bound session yet — the
/// observers are then never installed and the realtime socket runs in
/// "no projection refresh" mode (standalone / unbound sockets have
/// nothing to refresh and no turn lifecycle to track through the DSL).
async fn resolve_session_realtime_handles(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
) -> Option<(
    Arc<dyn meerkat_core::handles::SessionContextHandle>,
    Arc<dyn meerkat_core::handles::RealtimeProductTurnHandle>,
)> {
    let session_id = resolve_primary_session_id(
        runtime,
        binding,
        "realtime projection refresh is not wired to a primary session binding",
    )
    .await
    .ok()?;
    // `prepare_bindings` re-binds the session's DSL authority to a fresh
    // runtime epoch; the returned bindings expose the canonical handles
    // shared by every other consumer of this session's DSL authority.
    let bindings = runtime
        .runtime_adapter()
        .prepare_bindings(session_id)
        .await
        .ok()?;
    Some((
        Arc::clone(&bindings.session_context),
        Arc::clone(&bindings.realtime_product_turn),
    ))
}

async fn handle_channel_input(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
    turning_mode: meerkat_contracts::RealtimeTurningMode,
    pending_turn: &mut RealtimePendingTurn,
    chunk: RealtimeInputChunk,
) -> Result<Vec<RealtimeServerFrame>, RealtimeChannelErrorFrame> {
    let session_id = resolve_primary_session_id(
        runtime,
        binding,
        "realtime frame routing is not wired to the substrate yet",
    )
    .await?;

    match chunk {
        RealtimeInputChunk::TextChunk(text_chunk) => {
            let mut frames = Vec::new();
            if pending_turn.staged_user_text.is_empty() {
                frames.push(channel_event(RealtimeEvent::TurnStarted));
            }
            pending_turn.staged_user_text.push_str(&text_chunk.text);
            frames.push(channel_event(RealtimeEvent::InputTranscriptPartial {
                text: pending_turn.staged_user_text.clone(),
            }));
            if matches!(
                turning_mode,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged
            ) {
                let text = std::mem::take(&mut pending_turn.staged_user_text);
                frames.extend(commit_runtime_turn_text(runtime, &session_id, text, true).await?);
            }
            Ok(frames)
        }
        RealtimeInputChunk::AudioChunk(_) | RealtimeInputChunk::VideoChunk(_) => {
            Err(RealtimeChannelErrorFrame {
                code: RealtimeErrorCode::UnsupportedInputKind,
                message: "realtime media chunk routing is not wired to the substrate yet"
                    .to_string(),
                details: None,
            })
        }
    }
}

async fn commit_pending_turn(
    runtime: &SessionRuntime,
    binding: Option<&RealtimeSocketBinding>,
    pending_turn: &mut RealtimePendingTurn,
) -> Result<Vec<RealtimeServerFrame>, RealtimeChannelErrorFrame> {
    let session_id = resolve_primary_session_id(
        runtime,
        binding,
        "realtime frame routing is not wired to the substrate yet",
    )
    .await?;

    if pending_turn.staged_user_text.is_empty() {
        return Err(RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::NoPendingTurn,
            message: "channel.commit_turn requires staged realtime input".to_string(),
            details: None,
        });
    }

    let text = std::mem::take(&mut pending_turn.staged_user_text);
    commit_runtime_turn_text(runtime, &session_id, text, true).await
}

async fn commit_runtime_turn_text(
    runtime: &SessionRuntime,
    session_id: &SessionId,
    text: String,
    emit_transcript_final: bool,
) -> Result<Vec<RealtimeServerFrame>, RealtimeChannelErrorFrame> {
    let (outcome, completion_handle) = runtime
        .runtime_adapter()
        .accept_input_with_completion(
            session_id,
            Input::Prompt(PromptInput::new(text.clone(), None)),
        )
        .await
        .map_err(|err| runtime_error_frame(err, "commit"))?;

    let mut frames = Vec::new();
    if emit_transcript_final {
        frames.push(channel_event(RealtimeEvent::InputTranscriptFinal {
            text: text.clone(),
            prosody_hint: None,
        }));
    }
    frames.push(channel_event(RealtimeEvent::TurnCommitted));

    if outcome.is_accepted() || outcome.is_deduplicated() {
        if let Some(completion_handle) = completion_handle {
            match completion_handle.wait().await {
                meerkat_runtime::completion::CompletionOutcome::Completed(_)
                | meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult
                | meerkat_runtime::completion::CompletionOutcome::Abandoned(_)
                | meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(_) => {
                    frames.push(channel_event(RealtimeEvent::TurnCompleted));
                }
                meerkat_runtime::completion::CompletionOutcome::CallbackPending {
                    tool_name,
                    ..
                } => {
                    return Err(RealtimeChannelErrorFrame {
                        code: RealtimeErrorCode::RuntimeInternal,
                        message: format!(
                            "realtime websocket tool callback sequencing is not wired yet for '{tool_name}'"
                        ),
                        details: None,
                    });
                }
            }
        } else {
            frames.push(channel_event(RealtimeEvent::TurnCompleted));
        }
    }

    Ok(frames)
}

async fn append_external_user_transcript(
    runtime: &SessionRuntime,
    session_id: &SessionId,
    text: String,
    emit_transcript_final: bool,
) -> Result<Vec<RealtimeServerFrame>, RealtimeChannelErrorFrame> {
    runtime
        .append_external_user_content(
            session_id,
            meerkat_core::types::ContentInput::Text(text.clone()),
        )
        .await
        .map_err(session_error_frame)?;

    let mut frames = Vec::new();
    if emit_transcript_final {
        frames.push(channel_event(RealtimeEvent::InputTranscriptFinal {
            text,
            prosody_hint: None,
        }));
    }
    frames.push(channel_event(RealtimeEvent::TurnCommitted));
    Ok(frames)
}

async fn append_external_assistant_output(
    runtime: &SessionRuntime,
    session_id: &SessionId,
    text: String,
    stop_reason: meerkat_core::types::StopReason,
    usage: meerkat_core::types::Usage,
) -> Result<Vec<RealtimeServerFrame>, RealtimeChannelErrorFrame> {
    let blocks = if text.is_empty() {
        Vec::new()
    } else {
        vec![meerkat_core::types::AssistantBlock::Text { text, meta: None }]
    };
    runtime
        .append_external_assistant_output(session_id, blocks, stop_reason, usage)
        .await
        .map_err(session_error_frame)?;

    Ok(vec![channel_event(RealtimeEvent::TurnCompleted)])
}

fn runtime_error_frame(err: RuntimeDriverError, action: &str) -> RealtimeChannelErrorFrame {
    match err {
        RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        }
        | RuntimeDriverError::Destroyed => RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::InvalidTarget,
            message: format!("realtime {action} target is unavailable"),
            details: None,
        },
        RuntimeDriverError::NotReady { state } => RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::RuntimeNotReady,
            message: format!("realtime {action} requires a ready runtime: {state}"),
            details: None,
        },
        RuntimeDriverError::ValidationFailed { reason } => RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::InvalidTarget,
            message: reason,
            details: None,
        },
        RuntimeDriverError::Internal(message) => RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::RuntimeInternal,
            message,
            details: None,
        },
        other => RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::RuntimeInternal,
            message: other.to_string(),
            details: None,
        },
    }
}

fn session_error_frame(err: meerkat_core::service::SessionError) -> RealtimeChannelErrorFrame {
    match err {
        meerkat_core::service::SessionError::NotFound { .. } => RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::InvalidTarget,
            message: "realtime session target is unavailable".to_string(),
            details: None,
        },
        other => RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::RuntimeInternal,
            message: other.to_string(),
            details: None,
        },
    }
}

fn realtime_client_error_frame(
    err: meerkat_client::LlmError,
    action: &str,
) -> RealtimeChannelErrorFrame {
    match err {
        meerkat_client::LlmError::InvalidRequest { message }
        | meerkat_client::LlmError::AuthenticationFailed { message }
        | meerkat_client::LlmError::ContentFiltered { reason: message }
        | meerkat_client::LlmError::ModelNotFound { model: message } => RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::InvalidTarget,
            message: format!("realtime {action} failed: {message}"),
            details: None,
        },
        other => RealtimeChannelErrorFrame {
            code: RealtimeErrorCode::ProviderSessionFailed,
            message: format!("realtime {action} failed: {other}"),
            details: None,
        },
    }
}

fn preemptive_interrupt_can_be_ignored(error: &RealtimeChannelErrorFrame) -> bool {
    error.code == RealtimeErrorCode::InvalidTarget
        && error.message.contains("realtime interrupt failed")
}

async fn send_error_and_close(
    socket: &mut WebSocket,
    error: RealtimeChannelErrorFrame,
) -> Result<(), ()> {
    let close_reason = error.code.as_str().to_string();
    send_server_frame(socket, &RealtimeServerFrame::ChannelError(error)).await?;
    send_server_frame(
        socket,
        &RealtimeServerFrame::ChannelClosed(meerkat_contracts::RealtimeChannelClosedFrame {
            reason: Some(close_reason.clone()),
        }),
    )
    .await?;
    socket
        .send(Message::Close(Some(CloseFrame {
            code: close_code::POLICY,
            reason: close_reason.into(),
        })))
        .await
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

    use std::time::Duration;

    use chrono::Utc;
    use meerkat_contracts::{
        RealtimeCapabilities, RealtimeChannelOpenFrame, RealtimeChannelRole, RealtimeChannelState,
        RealtimeChannelTarget, RealtimeInputKind, RealtimeOpenInfo, RealtimeOpenRequest,
        RealtimeOutputKind, RealtimeReconnectPolicy, RealtimeTurningMode,
    };
    use meerkat_core::types::StopReason;

    use super::{
        RealtimeErrorCode, RealtimeOpenError, RealtimeProtocolVersion, RealtimeReconnectFailure,
        RealtimeReconnectOverlay, RealtimeSocketBinding, RealtimeTurnCompletionDisposition,
        RealtimeWsHost, product_turn_completion_disposition,
        product_turn_completion_is_logically_terminal,
    };
    use tokio::time::Instant;

    fn conservative_capabilities() -> RealtimeCapabilities {
        RealtimeCapabilities {
            input_kinds: vec![RealtimeInputKind::Text, RealtimeInputKind::Audio],
            output_kinds: vec![RealtimeOutputKind::Text, RealtimeOutputKind::Audio],
            turning_modes: vec![RealtimeTurningMode::ProviderManaged],
            interrupt_supported: true,
            transcript_supported: true,
            tool_lifecycle_events_supported: false,
            video_supported: false,
            audio_input_format: None,
            audio_output_format: None,
        }
    }

    #[test]
    fn product_turn_completion_suppresses_provider_tool_use_boundaries() {
        assert_eq!(
            product_turn_completion_disposition(StopReason::ToolUse),
            RealtimeTurnCompletionDisposition::SuppressKeepStaged
        );
    }

    #[test]
    fn product_turn_completion_suppresses_cancelled_boundaries() {
        assert_eq!(
            product_turn_completion_disposition(StopReason::Cancelled),
            RealtimeTurnCompletionDisposition::SuppressDiscardStaged
        );
    }

    #[test]
    fn product_turn_completion_finalizes_real_terminal_outputs() {
        assert_eq!(
            product_turn_completion_disposition(StopReason::EndTurn),
            RealtimeTurnCompletionDisposition::Finalize
        );
        assert_eq!(
            product_turn_completion_disposition(StopReason::MaxTokens),
            RealtimeTurnCompletionDisposition::Finalize
        );
    }

    #[test]
    fn product_turn_completion_keeps_tool_use_turns_semantically_in_flight() {
        assert!(
            !product_turn_completion_is_logically_terminal(StopReason::ToolUse),
            "tool-use subresponses must not reopen projection refresh or close the logical turn"
        );
        assert!(
            product_turn_completion_is_logically_terminal(StopReason::Cancelled),
            "cancelled provider turns are terminal even when assistant output is suppressed"
        );
        assert!(
            product_turn_completion_is_logically_terminal(StopReason::EndTurn),
            "ordinary provider turn completions remain terminal"
        );
    }

    #[test]
    fn realtime_socket_binding_has_no_mob_member_variants() {
        // Lock-in: after Phase 5G/T5i there are no mob-member realtime bindings.
        // The exhaustive match below won't compile if MemberPrimary or
        // MemberObserver variants ever reappear — absence of those arms is the
        // proof.
        fn _proof(binding: RealtimeSocketBinding) {
            match binding {
                RealtimeSocketBinding::SessionPrimary { .. } => {}
                RealtimeSocketBinding::SessionObserver { .. } => {}
            }
        }
    }

    #[test]
    fn product_turn_preemption_requires_visible_output_progress() {
        use meerkat_core::handles::{RealtimeProductTurnHandle, RealtimeProductTurnPhase};
        use meerkat_runtime::RuntimeRealtimeProductTurnHandle;

        // Committed-but-no-output: must not preempt. Post-commit chunks
        // from the same utterance can't be reclassified as a new turn
        // before assistant output starts.
        let handle = RuntimeRealtimeProductTurnHandle::ephemeral();
        assert!(handle.turn_in_flight().unwrap());
        assert!(handle.turn_committed().unwrap());
        assert_eq!(handle.current_phase(), RealtimeProductTurnPhase::Committed);
        assert!(!handle.should_preempt_on_input());

        // Committed + output_started: barge-in remains valid once the
        // committed turn has visible assistant-side progress.
        assert!(handle.output_started().unwrap());
        assert_eq!(
            handle.current_phase(),
            RealtimeProductTurnPhase::Preemptible
        );
        assert!(handle.should_preempt_on_input());

        // In-flight + output_started but not yet committed: input still
        // belongs to the current user utterance.
        let handle = RuntimeRealtimeProductTurnHandle::ephemeral();
        assert!(handle.turn_in_flight().unwrap());
        assert!(handle.output_started().unwrap());
        assert_eq!(
            handle.current_phase(),
            RealtimeProductTurnPhase::OutputStarted
        );
        assert!(!handle.should_preempt_on_input());
    }

    #[test]
    fn product_turn_phase_is_idempotent_under_guard_rejection() {
        use meerkat_core::handles::{RealtimeProductTurnHandle, RealtimeProductTurnPhase};
        use meerkat_runtime::RuntimeRealtimeProductTurnHandle;

        // Duplicate `turn_in_flight()` from `AwaitingProgress` is
        // guard-rejected and surfaces as `Ok(false)`.
        let handle = RuntimeRealtimeProductTurnHandle::ephemeral();
        assert!(handle.turn_in_flight().unwrap());
        assert!(!handle.turn_in_flight().unwrap());
        assert_eq!(
            handle.current_phase(),
            RealtimeProductTurnPhase::AwaitingProgress
        );

        // `turn_terminal()` from `Idle` is guard-rejected.
        let handle = RuntimeRealtimeProductTurnHandle::ephemeral();
        assert!(!handle.turn_terminal().unwrap());
        assert_eq!(handle.current_phase(), RealtimeProductTurnPhase::Idle);
    }

    #[test]
    fn product_turn_interrupt_clears_output_started_only() {
        use meerkat_core::handles::{RealtimeProductTurnHandle, RealtimeProductTurnPhase};
        use meerkat_runtime::RuntimeRealtimeProductTurnHandle;

        // Preemptible → Committed on interrupt.
        let handle = RuntimeRealtimeProductTurnHandle::ephemeral();
        assert!(handle.turn_in_flight().unwrap());
        assert!(handle.turn_committed().unwrap());
        assert!(handle.output_started().unwrap());
        assert_eq!(
            handle.current_phase(),
            RealtimeProductTurnPhase::Preemptible
        );
        assert!(handle.turn_interrupted().unwrap());
        assert_eq!(handle.current_phase(), RealtimeProductTurnPhase::Committed);

        // OutputStarted → AwaitingProgress on interrupt.
        let handle = RuntimeRealtimeProductTurnHandle::ephemeral();
        assert!(handle.turn_in_flight().unwrap());
        assert!(handle.output_started().unwrap());
        assert!(handle.turn_interrupted().unwrap());
        assert_eq!(
            handle.current_phase(),
            RealtimeProductTurnPhase::AwaitingProgress
        );
    }

    #[tokio::test]
    async fn issue_open_info_tracks_target_and_single_use_token_acceptance() {
        let host = RealtimeWsHost::new("ws://127.0.0.1:4900/realtime/ws");
        let request = RealtimeOpenRequest {
            target: RealtimeChannelTarget::SessionTarget {
                session_id: "01234567-89ab-cdef-0123-456789abcdef".to_string(),
            },
            role: RealtimeChannelRole::Primary,
            turning_mode: RealtimeTurningMode::ProviderManaged,
            reconnect_policy: None,
            channel_config: None,
        };

        let info = host
            .issue_open_info(request.clone(), conservative_capabilities(), None)
            .await;
        assert_eq!(info.target, request.target);
        let accepted_result = host
            .accept_open_frame(&RealtimeChannelOpenFrame {
                protocol_version: info.default_protocol_version.clone(),
                open_token: info.open_token.clone(),
                role: RealtimeChannelRole::Primary,
                turning_mode: RealtimeTurningMode::ProviderManaged,
            })
            .await;
        assert!(
            accepted_result.is_ok(),
            "first open should succeed: {accepted_result:?}"
        );
        let accepted = match accepted_result {
            Ok(accepted) => accepted,
            Err(_) => unreachable!("assert above ensures success"),
        };
        assert_eq!(accepted.request.target, request.target);
        assert_eq!(accepted.request.role, RealtimeChannelRole::Primary);

        let reused_result = host
            .accept_open_frame(&RealtimeChannelOpenFrame {
                protocol_version: info.default_protocol_version,
                open_token: info.open_token,
                role: RealtimeChannelRole::Primary,
                turning_mode: RealtimeTurningMode::ProviderManaged,
            })
            .await;
        assert!(
            reused_result.is_err(),
            "second open should reject reused token: {reused_result:?}"
        );
        let reused = match reused_result {
            Ok(_) => unreachable!("assert above ensures an error"),
            Err(error) => error,
        };
        assert_eq!(reused, RealtimeOpenError::InvalidOpenToken);
        assert_eq!(reused.code(), RealtimeErrorCode::InvalidOpenToken);
    }

    #[tokio::test]
    async fn accept_open_frame_rejects_expired_role_mismatch_and_unsupported_protocol() {
        let host = RealtimeWsHost::new("ws://127.0.0.1:4900/realtime/ws")
            .with_token_ttl(std::time::Duration::from_millis(1));
        let request = RealtimeOpenRequest {
            target: RealtimeChannelTarget::SessionTarget {
                session_id: "fedcba98-7654-3210-fedc-ba9876543210".to_string(),
            },
            role: RealtimeChannelRole::Primary,
            turning_mode: RealtimeTurningMode::ProviderManaged,
            reconnect_policy: None,
            channel_config: None,
        };
        let info = host
            .issue_open_info(request.clone(), conservative_capabilities(), None)
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let expired_result = host
            .accept_open_frame(&RealtimeChannelOpenFrame {
                protocol_version: RealtimeProtocolVersion::CURRENT.as_str().to_string(),
                open_token: info.open_token.clone(),
                role: RealtimeChannelRole::Primary,
                turning_mode: RealtimeTurningMode::ProviderManaged,
            })
            .await;
        assert!(
            expired_result.is_err(),
            "expired token should reject: {expired_result:?}"
        );
        let expired = match expired_result {
            Ok(_) => unreachable!("assert above ensures an error"),
            Err(error) => error,
        };
        assert_eq!(expired, RealtimeOpenError::OpenTokenExpired);
        assert_eq!(expired.code(), RealtimeErrorCode::OpenTokenExpired);

        let fresh_info = host
            .issue_open_info(request, conservative_capabilities(), None)
            .await;
        let role_error_result = host
            .accept_open_frame(&RealtimeChannelOpenFrame {
                protocol_version: RealtimeProtocolVersion::CURRENT.as_str().to_string(),
                open_token: fresh_info.open_token.clone(),
                role: RealtimeChannelRole::Observer,
                turning_mode: RealtimeTurningMode::ProviderManaged,
            })
            .await;
        assert!(
            role_error_result.is_err(),
            "role mismatch should reject: {role_error_result:?}"
        );
        let role_error = match role_error_result {
            Ok(_) => unreachable!("assert above ensures an error"),
            Err(error) => error,
        };
        assert_eq!(role_error, RealtimeOpenError::RoleMismatch);
        assert_eq!(role_error.code(), RealtimeErrorCode::RoleMismatch);

        let protocol_error_result = host
            .accept_open_frame(&RealtimeChannelOpenFrame {
                protocol_version: "999".to_string(),
                open_token: fresh_info.open_token,
                role: RealtimeChannelRole::Primary,
                turning_mode: RealtimeTurningMode::ProviderManaged,
            })
            .await;
        assert!(
            protocol_error_result.is_err(),
            "unsupported protocol should reject: {protocol_error_result:?}"
        );
        let protocol_error = match protocol_error_result {
            Ok(_) => unreachable!("assert above ensures an error"),
            Err(error) => error,
        };
        assert_eq!(
            protocol_error,
            RealtimeOpenError::UnsupportedProtocolVersion {
                requested: "999".to_string(),
                supported: vec![RealtimeProtocolVersion::CURRENT.as_str().to_string()],
            }
        );
        assert_eq!(
            protocol_error.code(),
            RealtimeErrorCode::UnsupportedProtocolVersion
        );
    }

    #[tokio::test]
    async fn register_open_rejects_second_primary_and_allows_multiple_observers() {
        let host = RealtimeWsHost::new("ws://127.0.0.1:4900/realtime/ws");
        let target = RealtimeChannelTarget::SessionTarget {
            session_id: "11111111-2222-3333-4444-555555555555".to_string(),
        };

        let primary_info = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target: target.clone(),
                    role: RealtimeChannelRole::Primary,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                conservative_capabilities(),
                None,
            )
            .await;
        let primary_result = host
            .accept_open_frame(&RealtimeChannelOpenFrame {
                protocol_version: primary_info.default_protocol_version,
                open_token: primary_info.open_token,
                role: RealtimeChannelRole::Primary,
                turning_mode: RealtimeTurningMode::ProviderManaged,
            })
            .await;
        assert!(
            primary_result.is_ok(),
            "primary token should validate: {primary_result:?}"
        );
        let primary = match primary_result {
            Ok(primary) => primary,
            Err(_) => unreachable!("assert above ensures success"),
        };
        let registered_primary_result = host.register_open(&primary).await;
        assert!(
            registered_primary_result.is_ok(),
            "first primary should register: {registered_primary_result:?}"
        );
        let registered_primary = match registered_primary_result {
            Ok(registered_primary) => registered_primary,
            Err(_) => unreachable!("assert above ensures success"),
        };

        let second_primary_info = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target: target.clone(),
                    role: RealtimeChannelRole::Primary,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                conservative_capabilities(),
                None,
            )
            .await;
        let second_primary_result = host
            .accept_open_frame(&RealtimeChannelOpenFrame {
                protocol_version: second_primary_info.default_protocol_version,
                open_token: second_primary_info.open_token,
                role: RealtimeChannelRole::Primary,
                turning_mode: RealtimeTurningMode::ProviderManaged,
            })
            .await;
        assert!(
            second_primary_result.is_ok(),
            "second primary token should validate: {second_primary_result:?}"
        );
        let second_primary = match second_primary_result {
            Ok(second_primary) => second_primary,
            Err(_) => unreachable!("assert above ensures success"),
        };
        let second_primary_result = host.register_open(&second_primary).await;
        assert!(
            second_primary_result.is_err(),
            "second primary should be rejected while the first is active: {second_primary_result:?}"
        );
        let second_primary_error = match second_primary_result {
            Ok(_) => unreachable!("assert above ensures an error"),
            Err(error) => error,
        };
        assert_eq!(second_primary_error, RealtimeOpenError::TargetBusy);

        let observer_one = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target: target.clone(),
                    role: RealtimeChannelRole::Observer,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                conservative_capabilities(),
                None,
            )
            .await;
        let observer_one_result = host
            .accept_open_frame(&RealtimeChannelOpenFrame {
                protocol_version: observer_one.default_protocol_version,
                open_token: observer_one.open_token,
                role: RealtimeChannelRole::Observer,
                turning_mode: RealtimeTurningMode::ProviderManaged,
            })
            .await;
        assert!(
            observer_one_result.is_ok(),
            "observer one token should validate: {observer_one_result:?}"
        );
        let observer_one = match observer_one_result {
            Ok(observer_one) => observer_one,
            Err(_) => unreachable!("assert above ensures success"),
        };
        let observer_one_result = host.register_open(&observer_one).await;
        assert!(
            observer_one_result.is_ok(),
            "observer one should register: {observer_one_result:?}"
        );
        let observer_one = match observer_one_result {
            Ok(observer_one) => observer_one,
            Err(_) => unreachable!("assert above ensures success"),
        };

        let observer_two = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target: target.clone(),
                    role: RealtimeChannelRole::Observer,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                conservative_capabilities(),
                None,
            )
            .await;
        let observer_two_result = host
            .accept_open_frame(&RealtimeChannelOpenFrame {
                protocol_version: observer_two.default_protocol_version,
                open_token: observer_two.open_token,
                role: RealtimeChannelRole::Observer,
                turning_mode: RealtimeTurningMode::ProviderManaged,
            })
            .await;
        assert!(
            observer_two_result.is_ok(),
            "observer two token should validate: {observer_two_result:?}"
        );
        let observer_two = match observer_two_result {
            Ok(observer_two) => observer_two,
            Err(_) => unreachable!("assert above ensures success"),
        };
        let observer_two_result = host.register_open(&observer_two).await;
        assert!(
            observer_two_result.is_ok(),
            "observer two should register: {observer_two_result:?}"
        );
        let observer_two = match observer_two_result {
            Ok(observer_two) => observer_two,
            Err(_) => unreachable!("assert above ensures success"),
        };

        host.release_open(&observer_one).await;
        host.release_open(&observer_two).await;
        host.release_open(&registered_primary).await;

        let replacement_primary = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target,
                    role: RealtimeChannelRole::Primary,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                conservative_capabilities(),
                None,
            )
            .await;
        let replacement_primary_result = host
            .accept_open_frame(&RealtimeChannelOpenFrame {
                protocol_version: replacement_primary.default_protocol_version,
                open_token: replacement_primary.open_token,
                role: RealtimeChannelRole::Primary,
                turning_mode: RealtimeTurningMode::ProviderManaged,
            })
            .await;
        assert!(
            replacement_primary_result.is_ok(),
            "replacement primary token should validate: {replacement_primary_result:?}"
        );
        let replacement_primary = match replacement_primary_result {
            Ok(replacement_primary) => replacement_primary,
            Err(_) => unreachable!("assert above ensures success"),
        };
        let replacement_primary_result = host.register_open(&replacement_primary).await;
        assert!(
            replacement_primary_result.is_ok(),
            "primary slot should reopen after release: {replacement_primary_result:?}"
        );
        let replacement_primary = match replacement_primary_result {
            Ok(replacement_primary) => replacement_primary,
            Err(_) => unreachable!("assert above ensures success"),
        };
        host.release_open(&replacement_primary).await;
    }

    #[test]
    fn reconnect_overlay_schedules_exponential_backoff() {
        // Full-jitter backoff draws in `[0, capped_exponential]`; the pre-jitter
        // test pinned exact durations. After Item 4 we only check that each
        // draw is bounded by the exponential cap for that attempt.
        let policy = RealtimeReconnectPolicy {
            max_attempts: 3,
            initial_backoff_ms: 50,
            max_backoff_ms: 200,
            max_total_ms: 1_000,
        };
        let mut overlay = RealtimeReconnectOverlay::new_with_seed(policy, 0x5eed_face);
        let started_at = Instant::now();
        let started_status = overlay
            .begin_if_needed(started_at, Utc::now())
            .expect("initial reattach should start reconnect tracking");
        assert_eq!(started_status.state, RealtimeChannelState::Reconnecting);
        assert_eq!(started_status.attempt_count, 1);
        let first_deadline = overlay
            .next_retry_deadline
            .expect("first reconnect should have a retry deadline");
        let first_backoff = first_deadline.duration_since(started_at);
        // attempt=1 cap = initial_backoff_ms * 2^0 = 50ms
        assert!(
            first_backoff <= Duration::from_millis(50),
            "attempt=1 full-jitter draw must be in [0, 50ms], got {first_backoff:?}",
        );

        let first_failure_at = started_at + Duration::from_millis(60);
        let first_failure =
            overlay.on_attempt_failure(first_failure_at, Utc::now(), "transient failure");
        match first_failure {
            RealtimeReconnectFailure::RetryScheduled(status) => {
                assert_eq!(status.state, RealtimeChannelState::Reconnecting);
                assert_eq!(status.attempt_count, 2);
            }
            other => panic!("expected retry scheduling after first failure, got {other:?}"),
        }
        let second_deadline = overlay
            .next_retry_deadline
            .expect("second reconnect should have a retry deadline");
        let second_backoff = second_deadline.duration_since(first_failure_at);
        // attempt=2 cap = min(initial*2, max) = min(100, 200) = 100ms
        assert!(
            second_backoff <= Duration::from_millis(100),
            "attempt=2 full-jitter draw must be in [0, 100ms], got {second_backoff:?}",
        );
    }

    #[test]
    fn reconnect_overlay_exhausts_after_attempt_budget() {
        let mut overlay = RealtimeReconnectOverlay::new(RealtimeReconnectPolicy {
            max_attempts: 2,
            initial_backoff_ms: 10,
            max_backoff_ms: 40,
            max_total_ms: 1_000,
        });
        let started_at = Instant::now();
        overlay
            .begin_if_needed(started_at, Utc::now())
            .expect("initial reattach should start reconnect tracking");

        let first_failure = overlay.on_attempt_failure(
            started_at + Duration::from_millis(10),
            Utc::now(),
            "still disconnected",
        );
        assert!(
            matches!(first_failure, RealtimeReconnectFailure::RetryScheduled(_)),
            "first failure should schedule the second and final retry: {first_failure:?}"
        );

        let exhausted = overlay.on_attempt_failure(
            started_at + Duration::from_millis(30),
            Utc::now(),
            "still disconnected",
        );
        match exhausted {
            RealtimeReconnectFailure::Exhausted {
                status,
                error,
                close_reason,
            } => {
                assert_eq!(status.state, RealtimeChannelState::Error);
                assert_eq!(error.code, RealtimeErrorCode::ReconnectExhausted);
                assert_eq!(close_reason, "reconnect_exhausted");
            }
            other => panic!("expected reconnect exhaustion, got {other:?}"),
        }
        assert!(
            !overlay.is_active(),
            "exhaustion should clear reconnect state"
        );
    }

    fn realm_request(session_id: &str) -> RealtimeOpenRequest {
        RealtimeOpenRequest {
            target: RealtimeChannelTarget::SessionTarget {
                session_id: session_id.to_string(),
            },
            role: RealtimeChannelRole::Primary,
            turning_mode: RealtimeTurningMode::ProviderManaged,
            reconnect_policy: None,
            channel_config: None,
        }
    }

    fn realm_open_frame(info: &RealtimeOpenInfo) -> RealtimeChannelOpenFrame {
        RealtimeChannelOpenFrame {
            protocol_version: info.default_protocol_version.clone(),
            open_token: info.open_token.clone(),
            role: RealtimeChannelRole::Primary,
            turning_mode: RealtimeTurningMode::ProviderManaged,
        }
    }

    #[tokio::test]
    async fn token_for_session_a_cannot_open_session_b_across_realms() {
        // Mint a token under realm X for session A, then try to redeem the
        // same token against session B while the host observes realm Y. The
        // accept path must reject with UnauthorizedRealm and drop the
        // pending entry so replays also fail.
        let host = RealtimeWsHost::new("ws://127.0.0.1:4900/realtime/ws");
        let info_x = host
            .issue_open_info(
                realm_request("11111111-2222-3333-4444-555555555555"),
                conservative_capabilities(),
                Some("realm-x".to_string()),
            )
            .await;

        let cross_realm_result = host
            .accept_open_frame_with_realm(&realm_open_frame(&info_x), Some("realm-y"))
            .await;
        let error = match cross_realm_result {
            Ok(_) => unreachable!("cross-realm redemption must fail"),
            Err(err) => err,
        };
        assert_eq!(error, RealtimeOpenError::UnauthorizedRealm);
        assert_eq!(error.code(), RealtimeErrorCode::UnauthorizedRealm);

        // Pending entry must be discarded — a second attempt in the right
        // realm should now fall through to InvalidOpenToken (one-shot).
        let replay_result = host
            .accept_open_frame_with_realm(&realm_open_frame(&info_x), Some("realm-x"))
            .await;
        assert_eq!(
            replay_result.expect_err("pending entry must be discarded on realm mismatch"),
            RealtimeOpenError::InvalidOpenToken,
        );
    }

    #[tokio::test]
    async fn token_for_session_a_in_own_realm_opens() {
        // Regression guard: same-realm tokens keep working after the
        // realm-scoping check lands.
        let host = RealtimeWsHost::new("ws://127.0.0.1:4900/realtime/ws");
        let info = host
            .issue_open_info(
                realm_request("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
                conservative_capabilities(),
                Some("realm-x".to_string()),
            )
            .await;

        let accepted = host
            .accept_open_frame_with_realm(&realm_open_frame(&info), Some("realm-x"))
            .await
            .expect("same-realm token must succeed");
        assert_eq!(
            accepted.request.target,
            RealtimeChannelTarget::SessionTarget {
                session_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
            },
        );
    }

    #[tokio::test]
    async fn token_without_realm_still_accepts_when_observed_realm_is_none() {
        // Single-tenant fallback: when the mint side did not carry a realm
        // and the accept side observes no realm either, the token opens.
        let host = RealtimeWsHost::new("ws://127.0.0.1:4900/realtime/ws");
        let info = host
            .issue_open_info(
                realm_request("00000000-0000-0000-0000-000000000001"),
                conservative_capabilities(),
                None,
            )
            .await;
        let accepted = host
            .accept_open_frame_with_realm(&realm_open_frame(&info), None)
            .await
            .expect("none/none should open cleanly");
        assert_eq!(
            accepted.request.target,
            RealtimeChannelTarget::SessionTarget {
                session_id: "00000000-0000-0000-0000-000000000001".to_string(),
            },
        );
    }

    #[tokio::test]
    async fn token_minted_without_realm_rejects_observed_realm() {
        // A token minted in a pre-multitenant deployment must not become
        // accepted when the server is later upgraded to observe a realm.
        let host = RealtimeWsHost::new("ws://127.0.0.1:4900/realtime/ws");
        let info = host
            .issue_open_info(
                realm_request("00000000-0000-0000-0000-000000000002"),
                conservative_capabilities(),
                None,
            )
            .await;
        let error = host
            .accept_open_frame_with_realm(&realm_open_frame(&info), Some("realm-x"))
            .await
            .expect_err("realmless token must not open inside a realm");
        assert_eq!(error, RealtimeOpenError::UnauthorizedRealm);
    }

    #[test]
    fn reconnect_backoff_draws_distinct_jittered_values() {
        // Exponential full-jitter must not collapse to a single value across
        // attempts. Seed deterministically so the test is repeatable.
        let policy = RealtimeReconnectPolicy {
            max_attempts: 10_000,
            initial_backoff_ms: 100,
            max_backoff_ms: 5_000,
            max_total_ms: 0, // disable exhaustion budget so we can sample freely
        };
        let mut overlay =
            super::RealtimeReconnectOverlay::new_with_seed(policy, 0xfeed_d00d_f00d_beef);

        let mut draws = std::collections::BTreeSet::new();
        // Saturate the exponent quickly so later draws all share the same cap.
        let attempts: [u32; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
        for _ in 0..1_000 {
            for &attempt in &attempts {
                let backoff = overlay.backoff_for_attempt(attempt);
                draws.insert(backoff.as_millis() as u64);
            }
        }
        assert!(
            draws.len() > 500,
            "expected >500 distinct jittered backoff draws, got {}",
            draws.len(),
        );
    }

    #[test]
    fn reconnect_backoff_is_deterministic_for_same_seed() {
        // Regression guard: tests that rely on the jitter draws must be
        // reproducible given a fixed seed, so the policy layer can be verified
        // without hiding flakes behind randomness.
        let policy = RealtimeReconnectPolicy {
            max_attempts: 10,
            initial_backoff_ms: 100,
            max_backoff_ms: 5_000,
            max_total_ms: 0,
        };
        let seed = 0x1234_5678_9abc_def0;
        let mut a = super::RealtimeReconnectOverlay::new_with_seed(policy.clone(), seed);
        let mut b = super::RealtimeReconnectOverlay::new_with_seed(policy, seed);
        for attempt in 1..=8u32 {
            assert_eq!(
                a.backoff_for_attempt(attempt),
                b.backoff_for_attempt(attempt),
                "same-seed overlays must draw identical backoff for attempt {attempt}",
            );
        }
    }

    #[test]
    fn reconnect_status_emits_deadline_at_while_cycle_is_active() {
        // The status frame must surface the derived cycle deadline whenever a
        // reconnect budget is configured; clients use this to decide whether
        // it is worth waiting for the next retry.
        let policy = RealtimeReconnectPolicy {
            max_attempts: 3,
            initial_backoff_ms: 50,
            max_backoff_ms: 200,
            max_total_ms: 1_000,
        };
        let mut overlay = super::RealtimeReconnectOverlay::new_with_seed(policy, 0);
        let started_at = Instant::now();
        let started_utc = Utc::now();
        let status = overlay
            .begin_if_needed(started_at, started_utc)
            .expect("initial reattach should start reconnect tracking");
        assert_eq!(status.state, RealtimeChannelState::Reconnecting);
        assert!(
            status.deadline_at.is_some(),
            "reconnect status with a max_total_ms budget must carry a deadline_at",
        );

        let on_failure = overlay.on_attempt_failure(
            started_at + Duration::from_millis(25),
            started_utc,
            "transient failure",
        );
        match on_failure {
            super::RealtimeReconnectFailure::RetryScheduled(status) => {
                assert!(
                    status.deadline_at.is_some(),
                    "retry-scheduled status must carry deadline_at",
                );
                assert_eq!(status.deadline_at, overlay.deadline_at());
            }
            other => panic!("expected retry to be scheduled, got {other:?}"),
        }
    }

    #[test]
    fn reconnect_status_omits_deadline_at_when_budget_is_unset() {
        let policy = RealtimeReconnectPolicy {
            max_attempts: 3,
            initial_backoff_ms: 50,
            max_backoff_ms: 200,
            max_total_ms: 0,
        };
        let mut overlay = super::RealtimeReconnectOverlay::new_with_seed(policy, 0);
        let status = overlay
            .begin_if_needed(Instant::now(), Utc::now())
            .expect("reattach should start");
        assert_eq!(status.deadline_at, None);
    }

    // -----------------------------------------------------------------------
    // W2-E: ProjectionFreshness typed state machine tests.
    //
    // Invariant tests are paired: each transition has a positive case
    // (proves the transition advances as specified) and a negative case
    // (proves the transition refuses to fire on the wrong precondition).
    // This symmetry is the lesson from W1-B (admission-only trust cut
    // that silently broke send-resolution when only positive cases were
    // covered): both directions of change must be asserted.
    // -----------------------------------------------------------------------

    use super::ProjectionFreshness;

    #[test]
    fn projection_freshness_clean_advances_to_stale_immediate_when_idle() {
        // Positive: idle session, context advances, frontier transitions
        // to StaleImmediate carrying the new watermark.
        let fresh = ProjectionFreshness::Clean { baseline_ms: 100 };
        let advanced = fresh.on_context_advanced(200, /* turn_in_flight */ false);
        assert_eq!(
            advanced,
            ProjectionFreshness::StaleImmediate { new_at_ms: 200 }
        );
    }

    #[test]
    fn projection_freshness_clean_advances_to_stale_deferred_when_turn_in_flight() {
        // Positive: mid-turn advance records as `StaleDeferred` so the
        // refresh is held until turn end (preserves barge-in continuity).
        let fresh = ProjectionFreshness::Clean { baseline_ms: 100 };
        let advanced = fresh.on_context_advanced(200, /* turn_in_flight */ true);
        assert_eq!(
            advanced,
            ProjectionFreshness::StaleDeferred { new_at_ms: 200 }
        );
    }

    #[test]
    fn projection_freshness_non_monotonic_advance_is_ignored() {
        // Negative: an advance at or below the current baseline is a
        // no-op. The consumer must not transition into a stale state
        // on a tick that doesn't actually advance canonical truth.
        let fresh = ProjectionFreshness::Clean { baseline_ms: 300 };
        assert_eq!(
            fresh.on_context_advanced(300, false),
            ProjectionFreshness::Clean { baseline_ms: 300 },
        );
        assert_eq!(
            fresh.on_context_advanced(250, false),
            ProjectionFreshness::Clean { baseline_ms: 300 },
        );
    }

    #[test]
    fn projection_freshness_turn_completed_promotes_deferred_to_immediate() {
        // Positive: this is the specific s71 turn-8 fix. A StaleDeferred
        // entry (canonical truth advanced mid-turn, e.g. peer-response
        // terminal) must promote to StaleImmediate when the turn
        // completes so the drain site picks it up.
        let deferred = ProjectionFreshness::StaleDeferred { new_at_ms: 500 };
        let promoted = deferred.on_turn_completed();
        assert_eq!(
            promoted,
            ProjectionFreshness::StaleImmediate { new_at_ms: 500 }
        );
    }

    #[test]
    fn projection_freshness_turn_completed_does_not_alter_clean_state() {
        // Negative: turn completion on a Clean frontier is a no-op. The
        // drain must not re-fire against a session that has no pending
        // advance — this was the failure mode where own-turn commits
        // masqueraded as external advances under the old flag-based
        // state (D4 / issue #260).
        let clean = ProjectionFreshness::Clean { baseline_ms: 42 };
        assert_eq!(clean.on_turn_completed(), clean);
    }

    #[test]
    fn projection_freshness_turn_completed_is_idempotent_on_immediate() {
        // Negative: repeated turn-completion ticks against an already-
        // immediate stale entry must not regress (e.g. to Clean) or
        // duplicate the stale reason. The drain is the only transition
        // that should clear it.
        let immediate = ProjectionFreshness::StaleImmediate { new_at_ms: 500 };
        assert_eq!(immediate.on_turn_completed(), immediate);
    }

    #[test]
    fn projection_freshness_refresh_clears_stale_and_advances_baseline() {
        // Positive: a successful refresh drain resets the frontier to
        // Clean with the refreshed watermark.
        let stale = ProjectionFreshness::StaleImmediate { new_at_ms: 700 };
        let refreshed = stale.on_refreshed(720);
        assert_eq!(refreshed, ProjectionFreshness::Clean { baseline_ms: 720 });
    }

    #[test]
    fn projection_freshness_refresh_keeps_highest_observed_watermark() {
        // Negative: if the post-refresh DSL watermark is lower than the
        // stale `new_at_ms` we already saw, the refreshed baseline must
        // still reflect the highest observed watermark — regressing the
        // baseline would leave the next SessionContextAdvanced tick
        // misclassified as non-advancing and silently drop refreshes.
        let stale = ProjectionFreshness::StaleImmediate { new_at_ms: 700 };
        let refreshed = stale.on_refreshed(680);
        assert_eq!(refreshed, ProjectionFreshness::Clean { baseline_ms: 700 });
    }

    #[test]
    fn projection_freshness_is_stale_immediate_only_for_immediate_variant() {
        // Tight invariant used by the drain gate. Must not fire for
        // Clean or StaleDeferred.
        assert!(
            !ProjectionFreshness::Clean { baseline_ms: 0 }.is_stale_immediate(),
            "Clean must not be drain-eligible",
        );
        assert!(
            !ProjectionFreshness::StaleDeferred { new_at_ms: 0 }.is_stale_immediate(),
            "StaleDeferred must not be drain-eligible — turn-end pulls the trigger",
        );
        assert!(
            ProjectionFreshness::StaleImmediate { new_at_ms: 0 }.is_stale_immediate(),
            "StaleImmediate must be drain-eligible",
        );
    }

    #[test]
    fn projection_freshness_deferred_coalesces_highest_new_at() {
        // Multiple mid-turn advances (e.g. peer-response terminal, then
        // a tool-result append) must coalesce to the highest watermark,
        // not lose ticks.
        let mut state = ProjectionFreshness::Clean { baseline_ms: 100 };
        state = state.on_context_advanced(200, true);
        state = state.on_context_advanced(300, true);
        assert_eq!(state, ProjectionFreshness::StaleDeferred { new_at_ms: 300 });
    }

    #[test]
    fn projection_freshness_idle_advance_supersedes_previous_deferred() {
        // After a turn completion, the deferred promotes to immediate.
        // A subsequent advance while idle must keep the state immediate
        // and update the watermark — not regress to Deferred.
        let deferred = ProjectionFreshness::StaleDeferred { new_at_ms: 200 };
        let promoted = deferred.on_turn_completed();
        assert_eq!(
            promoted,
            ProjectionFreshness::StaleImmediate { new_at_ms: 200 }
        );
        let updated = promoted.on_context_advanced(400, false);
        assert_eq!(
            updated,
            ProjectionFreshness::StaleImmediate { new_at_ms: 400 }
        );
    }
}
