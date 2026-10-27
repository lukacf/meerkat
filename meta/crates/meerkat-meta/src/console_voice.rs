//! Console request ownership around the shared Meerkat live host.
//!
//! This registry fences HTTP retries and cancellation, not live execution.
//! A host owns every channel, receipt, credential, and provider effect.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::{Mutex, Notify};

use crate::live_contracts::PendingLiveChannelHandle;
use crate::live_wiring::{
    LiveOwner, LiveOwnerArbiter, LiveOwnerCloser, LiveOwnerLease, LiveSupersededReason,
};

#[cfg(feature = "openai-live")]
mod auth;
#[cfg(all(test, feature = "openai-live"))]
pub(crate) mod body_read_counting_store;
#[cfg(feature = "openai-live")]
pub(crate) mod capabilities;
mod captions;
mod context_status;
#[cfg(feature = "openai-live")]
pub(crate) mod live_host;
pub(crate) mod readiness_trace;
mod summary;
#[cfg(feature = "openai-live")]
mod summary_window;

pub(crate) use captions::{VoiceCaptionBatch, VoiceCaptions, VoiceCaptionsRequest};
#[cfg(feature = "openai-live")]
pub(crate) use captions::{VoiceCaptionHub, VoiceCaptionRegistration};
pub(crate) use context_status::{
    VoiceContextPreparation, VoiceContextStatus, VoiceContextStatusRequest,
};

pub(crate) const VOICE_OPEN_METHOD: &str = "meta/console/voice/open";
pub(crate) const VOICE_READINESS_METHOD: &str = "meta/console/voice/readiness";
pub(crate) const VOICE_CLOSE_METHOD: &str = "meta/console/voice/close";
pub(crate) const VOICE_ANSWER_RECEIVED_METHOD: &str = "meta/console/voice/answer_received";
pub(crate) const VOICE_REPLACEMENT_METHOD: &str = "meta/console/voice/replacement";
pub(crate) const VOICE_ACTIVITY_METHOD: &str = "meta/console/voice/activity";
pub(crate) const VOICE_CONTEXT_STATUS_METHOD: &str = "meta/console/voice/context_status";
pub(crate) const VOICE_CAPTIONS_METHOD: &str = "meta/console/voice/captions";
const SILENCE_LIMIT: Duration = Duration::from_mins(15);
const PENDING_SETUP_LIMIT: Duration = Duration::from_mins(2);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoiceActivity {
    pub identity: String,
    pub request_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoiceReadiness {
    pub identity: String,
}

pub(crate) fn is_channel_method(method: &str) -> bool {
    matches!(
        method,
        "meta/live/playback_owner/register"
            | "meta/live/playback_owner/revoke"
            | "meta/live/status"
            | "meta/live/close"
            | "meta/live/refresh"
            | "meta/live/interrupt"
            | "live/webrtc/answer"
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoiceAnswerReceived {
    pub identity: String,
    pub request_id: String,
    pub channel_id: String,
}
const MAX_REQUESTS: usize = 4096;
/// Closed slots stay observable for this long so a delayed request for the
/// same id sees the closed disposition instead of resurrecting old work.
const CLOSED_RETENTION: Duration = Duration::from_mins(10);
/// Closed slots one principal may retain at once. Beyond this the oldest are
/// reaped first, so no principal can consume the shared capacity by itself.
const MAX_CLOSED_PER_PRINCIPAL: usize = 32;
const CLOSE_WAIT: Duration = Duration::from_secs(10);

fn elapsed_ms(started: tokio::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
pub const CONSOLE_VOICE_SHUTDOWN_TIMEOUT: Duration = CLOSE_WAIT;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoiceRequest {
    pub identity: String,
    pub request_id: String,
}

impl VoiceRequest {
    fn validate(&self) -> Result<(), VoiceError> {
        validate_identity(&self.identity)?;
        if !valid_request_atom(&self.request_id) {
            return Err(VoiceError::InvalidRequest);
        }
        Ok(())
    }
}

fn valid_request_atom(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.trim() == value
}

fn validate_identity(identity: &str) -> Result<(), VoiceError> {
    if !valid_request_atom(identity)
        || crate::member_comms_id::is_reserved_generated_alias(identity)
    {
        return Err(VoiceError::InvalidRequest);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VoiceError {
    Unavailable,
    Unauthorized,
    InvalidRequest,
    RequestConflict,
    RequestCapacity,
    Cancelled,
    Closed,
    Busy,
    /// Another live open on this gateway was still being prepared, so this
    /// open was refused before anything opened. A new open succeeds once
    /// that open is prepared.
    #[cfg_attr(not(feature = "openai-live"), allow(dead_code))]
    OpenBusy,
    HostFailed,
    ContextReadFailed,
    /// The gateway's single live voice path was taken by another owner
    /// ("latest engaged wins"); this call was closed for that reason.
    Superseded(LiveSupersededReason),
    /// The readiness check did not finish within the server budget. This is
    /// not an answer: the console keeps what it knew and retries.
    ReadinessTimedOut,
}

impl VoiceError {
    pub(crate) fn rpc_error(self) -> crate::rpc::JsonRpcError {
        let (code, kind, message) = match self {
            Self::Unavailable => (-32050, "voice_unavailable", "Console voice is unavailable"),
            Self::Unauthorized => (
                -32030,
                "access_denied",
                "Console voice requires authorization",
            ),
            Self::InvalidRequest => (-32602, "invalid_params", "Invalid voice request"),
            Self::RequestConflict => (
                -32000,
                "voice_request_conflict",
                "Voice request conflicts with its existing owner",
            ),
            Self::RequestCapacity => (
                -32000,
                "voice_request_capacity",
                "Voice request capacity reached",
            ),
            Self::Cancelled => (-32000, "voice_cancelled", "Voice request was cancelled"),
            Self::Closed => (-32000, "voice_closed", "Voice conversation is closed"),
            Self::Busy => (
                -32000,
                "voice_busy",
                "Voice teardown is still pending; retry the same request",
            ),
            Self::OpenBusy => (
                -32000,
                "voice_open_busy",
                "Another live call is starting on this gateway; open again shortly",
            ),
            Self::HostFailed => (-32000, "voice_host_failed", "Voice host operation failed"),
            Self::ContextReadFailed => (
                -32000,
                "voice_context_read_failed",
                "Voice context status could not be read",
            ),
            Self::Superseded(_) => (
                -32000,
                "voice_superseded",
                "Voice conversation was superseded by another live owner",
            ),
            Self::ReadinessTimedOut => (
                -32000,
                "voice_readiness_timed_out",
                "Voice readiness could not be checked in time; retry",
            ),
        };
        let mut data = serde_json::json!({ "kind": kind });
        if let Self::Superseded(reason) = self {
            data["reason"] = serde_json::Value::String(reason.as_str().to_string());
        }
        crate::rpc::JsonRpcError {
            code,
            message: message.to_string(),
            data: Some(data),
        }
    }
}

/// These methods must delegate to shared live authority. A successful close
/// means no later activation can emerge from this exact host-owned open.
#[async_trait]
pub(crate) trait ConsoleVoiceSession: Send + Sync {
    fn pending(&self) -> PendingLiveChannelHandle;
    async fn close(&self) -> Result<(), VoiceError>;
    async fn dispatch(
        &self,
        _method: &str,
        _params: serde_json::Value,
    ) -> Result<serde_json::Value, VoiceError> {
        Err(VoiceError::Unavailable)
    }
    async fn answer_received(&self, _channel: &str) -> Result<(), VoiceError> {
        Err(VoiceError::Unavailable)
    }
    async fn replacement_required(&self) -> Result<serde_json::Value, VoiceError> {
        Err(VoiceError::Unavailable)
    }
    async fn context_preparation(
        &self,
        _channel: &str,
    ) -> Result<VoiceContextPreparation, VoiceError> {
        Err(VoiceError::Unavailable)
    }
    /// Provisional assistant captions of `channel` newer than `after`,
    /// waiting up to `wait` for one to arrive.
    async fn captions(
        &self,
        _channel: &str,
        _after: u64,
        _wait: Duration,
    ) -> Result<VoiceCaptionBatch, VoiceError> {
        Err(VoiceError::Unavailable)
    }
}

#[async_trait]
pub(crate) trait ConsoleVoiceHost: Send + Sync {
    /// Validate current target authorization and the configured OpenAI
    /// credential using the same authority as open, without opening a provider.
    async fn ready(&self, principal: &str, identity: &str) -> Result<bool, VoiceError>;
    async fn open(
        &self,
        principal: &str,
        identity: &str,
    ) -> Result<Arc<dyn ConsoleVoiceSession>, VoiceError>;
}

#[derive(Default)]
struct RequestState {
    opening: bool,
    cancelled: bool,
    closing: bool,
    closed: bool,
    session: Option<Arc<dyn ConsoleVoiceSession>>,
    open_error: Option<VoiceError>,
    close_error: Option<VoiceError>,
    last_activity: Option<tokio::time::Instant>,
    setup_deadline: Option<tokio::time::Instant>,
    activated: bool,
    /// First moment the registry observed this slot closed; reaping is
    /// measured from here, never from the close request itself.
    retired_at: Option<tokio::time::Instant>,
    /// Set when the voice-path arbiter closed this call for another owner.
    /// Every later request for the slot reports it instead of a bare close.
    superseded: Option<LiveSupersededReason>,
    /// The slot's engagement of the gateway's live voice path.
    lease: Option<LiveOwnerLease>,
}

/// Drop closed slots that are past retention, then enforce the per-principal
/// and global bounds by dropping the oldest closed slots first. Live slots
/// (opening, active, or failing to close) are never dropped here.
async fn reap_closed_requests(
    requests: &mut HashMap<RequestKey, Arc<RequestSlot>>,
    now: tokio::time::Instant,
) {
    let mut closed: Vec<(RequestKey, tokio::time::Instant)> = Vec::new();
    for (key, slot) in requests.iter() {
        let mut state = slot.state.lock().await;
        if !state.closed {
            continue;
        }
        let retired_at = *state.retired_at.get_or_insert(now);
        closed.push((key.clone(), retired_at));
    }
    closed.sort_by_key(|(_, retired_at)| *retired_at);
    let mut retained = Vec::new();
    for (key, retired_at) in closed {
        if now.saturating_duration_since(retired_at) >= CLOSED_RETENTION {
            requests.remove(&key);
        } else {
            retained.push(key);
        }
    }
    // Per-principal bound, keeping each principal's newest closed slots.
    let mut kept_per_principal: HashMap<&str, usize> = HashMap::new();
    let mut surviving = Vec::with_capacity(retained.len());
    for key in retained.iter().rev() {
        let kept = kept_per_principal.entry(key.0.as_str()).or_insert(0);
        if *kept >= MAX_CLOSED_PER_PRINCIPAL {
            requests.remove(key);
        } else {
            *kept += 1;
            surviving.push(key.clone());
        }
    }
    // Global bound: closed slots yield, oldest first, before capacity refuses.
    for key in surviving.iter().rev() {
        if requests.len() < MAX_REQUESTS {
            break;
        }
        requests.remove(key);
    }
}

struct RequestSlot {
    identity: String,
    state: Mutex<RequestState>,
    changed: Notify,
    arbiter: Option<Arc<LiveOwnerArbiter>>,
}

impl RequestSlot {
    fn supervise(self: &Arc<Self>) {
        let slot = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let changed = slot.changed.notified();
                let mut state = slot.state.lock().await;
                if state.closed {
                    return;
                }
                if state.cancelled {
                    drop(state);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    slot.cancel().await;
                    continue;
                }
                let deadline = if state.activated {
                    state.last_activity.map(|activity| activity + SILENCE_LIMIT)
                } else {
                    state.setup_deadline
                };
                let Some(deadline) = deadline else {
                    drop(state);
                    changed.await;
                    continue;
                };
                if tokio::time::Instant::now() >= deadline {
                    state.cancelled = true;
                    drop(state);
                    slot.cancel().await;
                    continue;
                }
                drop(state);
                tokio::select! {
                    () = changed => {},
                    () = tokio::time::sleep_until(deadline) => {},
                }
            }
        });
    }

    fn new(identity: String, state: RequestState, arbiter: Option<Arc<LiveOwnerArbiter>>) -> Self {
        Self {
            identity,
            state: Mutex::new(state),
            changed: Notify::new(),
            arbiter,
        }
    }

    /// The typed error a closed slot reports: a supersession when the
    /// arbiter closed it for another live owner, otherwise `fallback`.
    fn closed_error(state: &RequestState, fallback: VoiceError) -> VoiceError {
        state.superseded.map_or(fallback, VoiceError::Superseded)
    }

    /// The close the voice-path arbiter runs when another owner wins: mark
    /// the reason, cancel through the ordinary teardown path, and wait for it.
    fn owner_closer(self: &Arc<Self>) -> LiveOwnerCloser {
        let slot = Arc::clone(self);
        Arc::new(move |reason, _channel| {
            let slot = Arc::clone(&slot);
            Box::pin(async move {
                let has_channel = {
                    let mut state = slot.state.lock().await;
                    state.superseded = Some(reason);
                    state.session.is_some() || !state.opening
                };
                // `cancel` marks the slot and spawns the teardown that closes
                // whatever the provider open yields, now or later.
                slot.cancel().await;
                if !has_channel {
                    // Nothing is live yet: the in-flight open will find the
                    // slot cancelled and close its own channel on completion.
                    // Waiting here would block the winner on the loser's
                    // provider handshake.
                    return Ok(());
                }
                tokio::time::timeout(CLOSE_WAIT, slot.wait_closed())
                    .await
                    .map_err(|_| "console voice teardown is still pending".to_string())?
                    .map_err(|error| format!("console voice close failed: {error:?}"))
            })
        })
    }

    async fn result(&self) -> Result<PendingLiveChannelHandle, VoiceError> {
        loop {
            let changed = self.changed.notified();
            let state = self.state.lock().await;
            if state.cancelled {
                return Err(Self::closed_error(&state, VoiceError::Cancelled));
            }
            if let Some(error) = state.open_error {
                return Err(error);
            }
            if let Some(session) = state.session.as_ref() {
                return Ok(session.pending());
            }
            drop(state);
            changed.await;
        }
    }

    /// End this request's voice session once the authentication of the
    /// caller that opened it lapses: the Console's verifier no longer
    /// accepts the opening token, on its expiry or on a session fence set
    /// for its principal. The same rule ends an open stream. A session that
    /// ends first stops the watch.
    fn end_when_authentication_lapses(
        self: &Arc<Self>,
        mut authentication: crate::http_sse::StreamAuthentication,
    ) {
        let slot = Arc::clone(self);
        tokio::spawn(async move {
            let ended = async {
                loop {
                    let changed = slot.changed.notified();
                    if slot.state.lock().await.closed {
                        return;
                    }
                    changed.await;
                }
            };
            tokio::select! {
                () = authentication.lapsed() => {
                    tracing::info!(
                        identity = %slot.identity,
                        "console voice session ended: the sign-in that opened it is no longer accepted"
                    );
                    slot.cancel().await;
                }
                () = ended => {}
            }
        });
    }

    async fn wait_closed(&self) -> Result<(), VoiceError> {
        loop {
            let changed = self.changed.notified();
            let state = self.state.lock().await;
            if state.closed {
                return Ok(());
            }
            if let Some(error) = state.close_error {
                return Err(error);
            }
            drop(state);
            changed.await;
        }
    }

    async fn cancel(self: &Arc<Self>) {
        let mut state = self.state.lock().await;
        state.cancelled = true;
        self.changed.notify_waiters();
        if state.closed || state.closing {
            return;
        }
        state.closing = true;
        state.close_error = None;
        let slot = Arc::clone(self);
        // The request task does not own cleanup: dropping its HTTP response
        // must not cancel teardown or strand a late successful provider open.
        tokio::spawn(async move {
            let session = loop {
                let changed = slot.changed.notified();
                let state = slot.state.lock().await;
                if !state.opening {
                    break state.session.clone();
                }
                drop(state);
                changed.await;
            };
            let result = match session {
                Some(session) => session.close().await,
                None => Ok(()),
            };
            let mut state = slot.state.lock().await;
            state.closing = false;
            let released_lease = match result {
                Ok(()) => {
                    state.closed = true;
                    state.session = None;
                    state.lease.take()
                }
                Err(error) => {
                    state.close_error = Some(error);
                    None
                }
            };
            slot.changed.notify_waiters();
            drop(state);
            // Only after the slot is observably closed: the arbiter may be
            // mid-engagement waiting on exactly that, and a stale lease is
            // ignored, so this can never evict the owner that replaced us.
            if let (Some(arbiter), Some(lease)) = (slot.arbiter.as_ref(), released_lease) {
                arbiter.release(lease);
            }
        });
    }
}

type RequestKey = (String, String);

#[derive(Clone, Default)]
pub struct ConsoleVoiceController {
    host: Option<Arc<dyn ConsoleVoiceHost>>,
    requests: Arc<Mutex<HashMap<RequestKey, Arc<RequestSlot>>>>,
    stopped: Arc<AtomicBool>,
    /// The gateway's single live voice path, shared with the external
    /// `meta/live/*` door when both are registered. `None` means console
    /// voice is the only door and needs no arbitration.
    arbiter: Option<Arc<LiveOwnerArbiter>>,
    /// In-flight readiness checks by `(principal, identity)`. Concurrent
    /// checks for one target (the console's poll and a click, two tabs)
    /// share one check instead of each paying for the full probe.
    readiness_flights: Arc<std::sync::Mutex<HashMap<(String, String), ReadinessFlightEntry>>>,
}

type ReadinessResult = Result<VoiceReadinessReport, VoiceError>;
type SharedReadiness =
    futures::future::Shared<futures::future::BoxFuture<'static, ReadinessResult>>;

/// Weak so the map never keeps a check alive: when every caller has given
/// up (budget expired, client disconnected), the check is dropped.
struct ReadinessFlightEntry {
    id: u64,
    trace: Arc<readiness_trace::VoiceReadinessTrace>,
    result: futures::future::WeakShared<futures::future::BoxFuture<'static, ReadinessResult>>,
}

static NEXT_READINESS_FLIGHT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One caller's hold on a readiness check. Dropping the last hold cancels
/// the check.
pub(crate) struct ReadinessFlight {
    pub trace: Arc<readiness_trace::VoiceReadinessTrace>,
    /// `true` when this caller joined a check another caller started.
    pub joined: bool,
    pub result: SharedReadiness,
}

/// What `meta/console/voice/readiness` reports for one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VoiceReadinessReport {
    pub available: bool,
    /// Typed reason when unavailable because another owner holds the live
    /// path. Plain unavailability (no host, no credential, unauthorized
    /// target) carries no reason, as before.
    pub reason: Option<&'static str>,
    /// The owner holding the live path, when `reason` names one.
    pub holder: Option<LiveOwner>,
}

impl VoiceReadinessReport {
    pub(crate) const EXTERNAL_LIVE_ACTIVE: &'static str = "external_live_active";

    pub(crate) fn to_wire(&self, identity: &str) -> serde_json::Value {
        let mut wire = serde_json::json!({ "identity": identity, "available": self.available });
        if let Some(reason) = self.reason {
            wire["reason"] = serde_json::Value::String(reason.to_string());
        }
        if let Some(holder) = self.holder.as_ref() {
            let mut holder_wire = serde_json::json!({ "identity": holder.identity() });
            if let Some(channel_id) = holder.channel_id() {
                holder_wire["channel_id"] = serde_json::Value::String(channel_id.to_string());
            }
            wire["holder"] = holder_wire;
        }
        wire
    }
}

impl ConsoleVoiceController {
    pub(crate) async fn context_status(
        &self,
        principal: &str,
        request: VoiceContextStatusRequest,
    ) -> Result<VoiceContextStatus, VoiceError> {
        if !valid_request_atom(&request.channel_id) {
            return Err(VoiceError::InvalidRequest);
        }
        let slot = self
            .request_slot(
                principal,
                &VoiceRequest {
                    identity: request.identity.clone(),
                    request_id: request.request_id.clone(),
                },
            )
            .await?;
        let session = {
            let state = slot.state.lock().await;
            if state.cancelled || state.closed {
                return Err(RequestSlot::closed_error(&state, VoiceError::Closed));
            }
            state.session.clone().ok_or(VoiceError::Busy)?
        };
        if session.pending().channel_id != request.channel_id {
            return Err(VoiceError::RequestConflict);
        }
        // Never hold the request lock over a custody read. Activation, audio
        // activity and cancellation must proceed even if the read is delayed.
        let context_preparation = session.context_preparation(&request.channel_id).await?;
        let state = slot.state.lock().await;
        if state.cancelled || state.closed {
            return Err(RequestSlot::closed_error(&state, VoiceError::Closed));
        }
        if session.pending().channel_id != request.channel_id {
            return Err(VoiceError::RequestConflict);
        }
        Ok(VoiceContextStatus {
            identity: request.identity,
            request_id: request.request_id,
            channel_id: request.channel_id,
            context_preparation,
        })
    }

    /// Read the owned call's provisional assistant captions. The scope is
    /// the exact `{identity, request_id, channel_id}` of the current channel,
    /// like `context_status`; the request lock is never held over the wait.
    pub(crate) async fn captions(
        &self,
        principal: &str,
        request: VoiceCaptionsRequest,
    ) -> Result<VoiceCaptions, VoiceError> {
        if !valid_request_atom(&request.channel_id) {
            return Err(VoiceError::InvalidRequest);
        }
        let session = self
            .owned_session(
                principal,
                &VoiceRequest {
                    identity: request.identity.clone(),
                    request_id: request.request_id.clone(),
                },
            )
            .await?;
        if session.pending().channel_id != request.channel_id {
            return Err(VoiceError::RequestConflict);
        }
        let wait = Duration::from_millis(request.wait_ms.min(captions::MAX_CAPTION_WAIT_MS));
        let batch = session
            .captions(&request.channel_id, request.after, wait)
            .await?;
        Ok(VoiceCaptions {
            identity: request.identity,
            request_id: request.request_id,
            channel_id: request.channel_id,
            batch,
        })
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        let drain = async {
            let requests = self.requests.lock().await;
            self.stopped.store(true, Ordering::SeqCst);
            let slots = requests.values().cloned().collect::<Vec<_>>();
            drop(requests);
            for slot in &slots {
                slot.cancel().await;
            }
            for result in
                futures::future::join_all(slots.iter().map(|slot| slot.wait_closed())).await
            {
                result.map_err(|_| "console voice cleanup failed".to_string())?;
            }
            Ok(())
        };
        tokio::time::timeout(CONSOLE_VOICE_SHUTDOWN_TIMEOUT, drain)
            .await
            .map_err(|_| "console voice cleanup remains pending".to_string())?
    }

    pub(crate) async fn note_activity(
        &self,
        principal: &str,
        request: VoiceActivity,
    ) -> Result<(), VoiceError> {
        let slot = self
            .request_slot(
                principal,
                &VoiceRequest {
                    identity: request.identity,
                    request_id: request.request_id,
                },
            )
            .await?;
        let mut state = slot.state.lock().await;
        if state.cancelled || state.closed {
            return Err(RequestSlot::closed_error(&state, VoiceError::Cancelled));
        }
        if state.session.is_none() || !state.activated {
            return Err(VoiceError::Busy);
        }
        state.last_activity = Some(tokio::time::Instant::now());
        tracing::trace!("console voice audio activity accepted");
        slot.changed.notify_waiters();
        Ok(())
    }

    /// Authentication readiness is independent of whether a voice request
    /// already owns the target; it is not permission to open a second channel.
    pub(crate) async fn ready(&self, principal: &str, identity: &str) -> Result<bool, VoiceError> {
        validate_identity(identity)?;
        if principal.trim().is_empty() {
            return Err(VoiceError::Unauthorized);
        }
        if self.stopped.load(Ordering::SeqCst) {
            tracing::warn!(
                identity,
                "console voice readiness refused: controller stopped"
            );
            return Ok(false);
        }
        match &self.host {
            Some(host) => host.ready(principal, identity).await,
            None => {
                tracing::warn!(
                    identity,
                    "console voice readiness refused: no live host composed"
                );
                Ok(false)
            }
        }
    }

    /// Readiness with the typed reason and holder when the external live
    /// channel holds the gateway's voice path. Never opens a provider. The
    /// HTTP route holds the flight itself (see [`Self::readiness_flight`]).
    #[cfg(test)]
    pub(crate) async fn readiness(
        &self,
        principal: &str,
        identity: &str,
    ) -> Result<VoiceReadinessReport, VoiceError> {
        self.readiness_flight(principal, identity)?.result.await
    }

    /// Start, or join, the readiness check for `(principal, identity)`.
    /// The check runs while at least one returned flight is held; it is
    /// cancelled when the last one is dropped.
    pub(crate) fn readiness_flight(
        &self,
        principal: &str,
        identity: &str,
    ) -> Result<ReadinessFlight, VoiceError> {
        validate_identity(identity)?;
        if principal.trim().is_empty() {
            return Err(VoiceError::Unauthorized);
        }
        let key = (principal.to_string(), identity.to_string());
        let mut flights = self
            .readiness_flights
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = flights.get(&key)
            && let Some(result) = entry.result.upgrade()
        {
            return Ok(ReadinessFlight {
                trace: Arc::clone(&entry.trace),
                joined: true,
                result,
            });
        }
        let id = NEXT_READINESS_FLIGHT.fetch_add(1, Ordering::Relaxed);
        let trace = Arc::new(readiness_trace::VoiceReadinessTrace::new());
        let controller = self.clone();
        let registry = Arc::clone(&self.readiness_flights);
        let flight_key = key.clone();
        let flight_trace = Arc::clone(&trace);
        let check: futures::future::BoxFuture<'static, ReadinessResult> =
            Box::pin(readiness_trace::scope(Arc::clone(&trace), async move {
                let result = controller
                    .check_readiness(&flight_key.0, &flight_key.1)
                    .await;
                flight_trace.finish();
                // A finished check is never joined: the next caller starts
                // a fresh one.
                let mut flights = registry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if flights.get(&flight_key).is_some_and(|entry| entry.id == id) {
                    flights.remove(&flight_key);
                }
                result
            }));
        let result = futures::FutureExt::shared(check);
        let Some(weak) = result.downgrade() else {
            return Err(VoiceError::HostFailed);
        };
        flights.retain(|_, entry| entry.result.upgrade().is_some());
        flights.insert(
            key,
            ReadinessFlightEntry {
                id,
                trace: Arc::clone(&trace),
                result: weak,
            },
        );
        Ok(ReadinessFlight {
            trace,
            joined: false,
            result,
        })
    }

    async fn check_readiness(
        &self,
        principal: &str,
        identity: &str,
    ) -> Result<VoiceReadinessReport, VoiceError> {
        readiness_trace::enter(readiness_trace::VoiceReadinessStage::Arbiter);
        let holder = match self.arbiter.as_ref() {
            Some(arbiter) => arbiter.holder().await,
            None => None,
        };
        if let Some(holder @ LiveOwner::ExternalLive { .. }) = holder {
            tracing::info!(
                identity,
                holder = ?holder,
                "console voice readiness refused: external live channel holds the voice path"
            );
            return Ok(VoiceReadinessReport {
                available: false,
                reason: Some(VoiceReadinessReport::EXTERNAL_LIVE_ACTIVE),
                holder: Some(holder),
            });
        }
        Ok(VoiceReadinessReport {
            available: self.ready(principal, identity).await?,
            reason: None,
            holder: None,
        })
    }

    /// Share the gateway's live voice path with the external live door.
    #[cfg_attr(not(feature = "openai-live"), allow(dead_code))]
    #[must_use]
    pub(crate) fn with_arbiter(mut self, arbiter: Arc<LiveOwnerArbiter>) -> Self {
        self.arbiter = Some(arbiter);
        self
    }

    pub(crate) fn configured(&self) -> bool {
        self.host.is_some() && !self.stopped.load(Ordering::SeqCst)
    }

    async fn owned_session(
        &self,
        principal: &str,
        request: &VoiceRequest,
    ) -> Result<Arc<dyn ConsoleVoiceSession>, VoiceError> {
        let slot = self.request_slot(principal, request).await?;
        let state = slot.state.lock().await;
        if state.cancelled || state.closed {
            return Err(RequestSlot::closed_error(&state, VoiceError::Closed));
        }
        state.session.clone().ok_or(VoiceError::Busy)
    }

    pub(crate) async fn answer_received(
        &self,
        principal: &str,
        request: VoiceRequest,
        channel: &str,
    ) -> Result<(), VoiceError> {
        self.owned_session(principal, &request)
            .await?
            .answer_received(channel)
            .await?;
        let slot = self.request_slot(principal, &request).await?;
        let mut state = slot.state.lock().await;
        if state.cancelled || state.closed {
            return Err(RequestSlot::closed_error(&state, VoiceError::Closed));
        }
        if !state.activated {
            state.activated = true;
            state.last_activity = Some(tokio::time::Instant::now());
            state.setup_deadline = None;
            slot.changed.notify_waiters();
        }
        Ok(())
    }

    pub(crate) async fn replacement_required(
        &self,
        principal: &str,
        request: VoiceRequest,
    ) -> Result<serde_json::Value, VoiceError> {
        self.owned_session(principal, &request)
            .await?
            .replacement_required()
            .await
    }

    pub(crate) async fn dispatch_channel(
        &self,
        principal: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, VoiceError> {
        let identity = params
            .get("identity")
            .and_then(serde_json::Value::as_str)
            .ok_or(VoiceError::InvalidRequest)?;
        let channel = params
            .get("channel_id")
            .and_then(serde_json::Value::as_str)
            .ok_or(VoiceError::InvalidRequest)?;
        let requests = self.requests.lock().await;
        let slots = requests
            .iter()
            .filter(|((owner, _), slot)| owner == principal && slot.identity == identity)
            .map(|(_, slot)| Arc::clone(slot))
            .collect::<Vec<_>>();
        drop(requests);
        for slot in slots {
            let state = slot.state.lock().await;
            if !state.cancelled
                && let Some(session) = state.session.as_ref()
                && session.pending().channel_id == channel
            {
                let session = Arc::clone(session);
                drop(state);
                return session.dispatch(method, params).await;
            }
        }
        Err(VoiceError::RequestConflict)
    }

    async fn request_slot(
        &self,
        principal: &str,
        request: &VoiceRequest,
    ) -> Result<Arc<RequestSlot>, VoiceError> {
        request.validate()?;
        if principal.trim().is_empty() {
            return Err(VoiceError::Unauthorized);
        }
        let requests = self.requests.lock().await;
        let slot = requests
            .get(&(principal.to_string(), request.request_id.clone()))
            .ok_or(VoiceError::RequestConflict)?;
        if slot.identity != request.identity {
            return Err(VoiceError::RequestConflict);
        }
        Ok(Arc::clone(slot))
    }

    // No production host is installed until the upstream summary,
    // existing-member execution and authenticated readiness seams are composed.
    #[allow(dead_code)]
    pub(crate) fn new(host: Arc<dyn ConsoleVoiceHost>) -> Self {
        Self {
            host: Some(host),
            requests: Arc::default(),
            stopped: Arc::default(),
            arbiter: None,
            readiness_flights: Arc::default(),
        }
    }
}

impl ConsoleVoiceController {
    /// [`Self::open_while_authenticated`] for a session no authentication
    /// ends.
    #[cfg(test)]
    pub(crate) async fn open(
        &self,
        principal: &str,
        request: VoiceRequest,
    ) -> Result<PendingLiveChannelHandle, VoiceError> {
        self.open_while_authenticated(principal, request, None)
            .await
    }

    /// [`Self::open`] for a caller a token identified: the session it opens
    /// ends once `authentication` lapses (the Console's verifier no longer
    /// accepts the token, on its expiry or on a session fence), as an open
    /// stream does. A retried open of the same request keeps the first
    /// open's watch.
    pub(crate) async fn open_while_authenticated(
        &self,
        principal: &str,
        request: VoiceRequest,
        authentication: Option<crate::http_sse::StreamAuthentication>,
    ) -> Result<PendingLiveChannelHandle, VoiceError> {
        request.validate()?;
        if principal.trim().is_empty() {
            return Err(VoiceError::Unauthorized);
        }
        // The console already confirmed readiness for this click through
        // `meta/console/voice/readiness`; the host's open resolves the same
        // target authorization and credential once more inside Meerkat's
        // open admission, so a second probe here only repeated that work.
        let host = self.host.as_ref().ok_or(VoiceError::Unavailable)?;
        let key = (principal.to_string(), request.request_id.clone());
        let mut requests = self.requests.lock().await;
        if self.stopped.load(Ordering::SeqCst) {
            return Err(VoiceError::Unavailable);
        }
        let slot = if let Some(slot) = requests.get(&key) {
            if slot.identity != request.identity {
                return Err(VoiceError::RequestConflict);
            }
            Arc::clone(slot)
        } else {
            reap_closed_requests(&mut requests, tokio::time::Instant::now()).await;
            if requests.len() >= MAX_REQUESTS {
                return Err(VoiceError::RequestCapacity);
            }
            for ((owner, _), slot) in requests.iter() {
                let state = slot.state.lock().await;
                if owner == principal && !state.closed && state.open_error.is_none() {
                    return Err(VoiceError::Busy);
                }
            }
            let slot = Arc::new(RequestSlot::new(
                request.identity.clone(),
                RequestState {
                    opening: true,
                    ..RequestState::default()
                },
                self.arbiter.clone(),
            ));
            requests.insert(key, Arc::clone(&slot));
            slot.supervise();
            if let Some(authentication) = authentication {
                slot.end_when_authentication_lapses(authentication);
            }
            let host = Arc::clone(host);
            let owner = principal.to_string();
            let pending = Arc::clone(&slot);
            tokio::spawn(async move {
                let arbiter_started = tokio::time::Instant::now();
                // Take the gateway's live voice path first ("latest engaged
                // wins"): an active external channel is closed with a typed
                // reason before this call opens. A close that fails keeps the
                // external owner and fails this call closed.
                let lease = if let Some(arbiter) = pending.arbiter.as_ref() {
                    let engaged = arbiter
                        .engage(
                            LiveOwner::ConsoleVoice {
                                principal: owner.clone(),
                                identity: request.identity.clone(),
                                channel_id: None,
                            },
                            pending.owner_closer(),
                        )
                        .await;
                    match engaged {
                        Ok((lease, preempted)) => {
                            if let Some(previous) = preempted {
                                tracing::info!(
                                    identity = %request.identity,
                                    superseded = ?previous,
                                    "console voice took the live voice path"
                                );
                            }
                            Some(lease)
                        }
                        Err(error) => {
                            tracing::warn!(%error, "console voice could not take the live voice path");
                            let mut state = pending.state.lock().await;
                            state.opening = false;
                            state.open_error = Some(VoiceError::HostFailed);
                            state.closed = true;
                            pending.changed.notify_waiters();
                            return;
                        }
                    }
                } else {
                    None
                };
                let arbiter_ms = elapsed_ms(arbiter_started);
                let host_open_started = tokio::time::Instant::now();
                let result = host.open(&owner, &request.identity).await;
                tracing::info!(
                    target: "meerkat_meta::console_voice::timing",
                    identity = %request.identity,
                    arbiter_ms,
                    host_open_ms = elapsed_ms(host_open_started),
                    ok = result.is_ok(),
                    "console voice open stages"
                );
                let mut state = pending.state.lock().await;
                state.opening = false;
                match result {
                    Ok(session) => {
                        let mut lost_to = None;
                        if let (Some(arbiter), Some(lease)) = (pending.arbiter.as_ref(), lease) {
                            arbiter.bind_channel(lease, &session.pending().channel_id);
                            lost_to = arbiter.take_superseded(lease);
                        }
                        state.lease = lease;
                        state.session = Some(session);
                        state.setup_deadline =
                            Some(tokio::time::Instant::now() + PENDING_SETUP_LIMIT);
                        if let Some(reason) = lost_to {
                            // Another owner took the path while this open was
                            // in flight. The channel just opened must not stay
                            // live beside the winner: close it through the
                            // ordinary teardown and report the reason.
                            state.superseded = Some(reason);
                            drop(state);
                            pending.cancel().await;
                            return;
                        }
                    }
                    Err(error) => {
                        if let (Some(arbiter), Some(lease)) = (pending.arbiter.as_ref(), lease) {
                            arbiter.release(lease);
                        }
                        state.open_error = Some(error);
                        state.closed = true;
                    }
                }
                pending.changed.notify_waiters();
            });
            slot
        };
        drop(requests);
        slot.result().await
    }

    pub(crate) async fn close(
        &self,
        principal: &str,
        request: VoiceRequest,
    ) -> Result<(), VoiceError> {
        request.validate()?;
        if principal.trim().is_empty() {
            return Err(VoiceError::Unauthorized);
        }
        let key = (principal.to_string(), request.request_id);
        let mut requests = self.requests.lock().await;
        let slot = if let Some(slot) = requests.get(&key) {
            if slot.identity != request.identity {
                return Err(VoiceError::RequestConflict);
            }
            Arc::clone(slot)
        } else {
            reap_closed_requests(&mut requests, tokio::time::Instant::now()).await;
            if requests.len() >= MAX_REQUESTS {
                return Err(VoiceError::RequestCapacity);
            }
            // Retain cancellation even when close wins the race with open.
            // The tombstone stays for CLOSED_RETENTION (bounded per
            // principal) so a delayed request cannot resurrect old work,
            // and is reaped afterwards so capacity recovers.
            let slot = Arc::new(RequestSlot::new(
                request.identity,
                RequestState {
                    cancelled: true,
                    closed: true,
                    retired_at: Some(tokio::time::Instant::now()),
                    ..RequestState::default()
                },
                None,
            ));
            requests.insert(key, Arc::clone(&slot));
            // The bound holds after this insertion as well, so a principal
            // issuing closes for unknown ids never exceeds its share.
            reap_closed_requests(&mut requests, tokio::time::Instant::now()).await;
            slot
        };
        drop(requests);
        slot.cancel().await;
        tokio::time::timeout(CLOSE_WAIT, slot.wait_closed())
            .await
            .map_err(|_| VoiceError::Busy)?
    }
}

/// A host whose sessions open at once and count their closes, for tests of
/// when a session ends.
#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) mod closing_test_support {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    pub(crate) struct ClosingVoiceSession {
        pub close_calls: AtomicUsize,
    }

    #[async_trait]
    impl ConsoleVoiceSession for ClosingVoiceSession {
        fn pending(&self) -> PendingLiveChannelHandle {
            PendingLiveChannelHandle {
                channel_id: "closing-channel".to_string(),
                target_identity: "agent-a".to_string(),
                execution_mode: crate::live_contracts::LiveExecutionMode::ClientContext,
                pending_receipt: "opaque-pending".to_string(),
                transport: meerkat_contracts::WireLiveTransportBootstrap::Webrtc {
                    token: "opaque-token".to_string(),
                    answer_method: "live/webrtc/answer".to_string(),
                    http_url: None,
                },
                capabilities: meerkat_contracts::WireLiveChannelCapabilities {
                    audio_in: true,
                    audio_out: true,
                    text_in: false,
                    text_out: false,
                    image_in: false,
                    video_in: false,
                    transcript_supported: true,
                    barge_in_supported: true,
                    provider_native_resume: false,
                },
                continuity: meerkat_contracts::WireLiveContinuityMode::TranscriptOnly,
                advisories: Vec::new(),
            }
        }

        async fn close(&self) -> Result<(), VoiceError> {
            self.close_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    pub(crate) struct ClosingVoiceHost {
        pub session: Arc<ClosingVoiceSession>,
    }

    impl ClosingVoiceHost {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self {
                session: Arc::new(ClosingVoiceSession {
                    close_calls: AtomicUsize::new(0),
                }),
            })
        }

        pub(crate) fn controller(self: &Arc<Self>) -> ConsoleVoiceController {
            ConsoleVoiceController {
                host: Some(Arc::clone(self) as Arc<dyn ConsoleVoiceHost>),
                ..ConsoleVoiceController::default()
            }
        }

        /// How many times the session was closed.
        pub(crate) fn closes(&self) -> usize {
            self.session.close_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ConsoleVoiceHost for ClosingVoiceHost {
        async fn ready(&self, _principal: &str, _identity: &str) -> Result<bool, VoiceError> {
            Ok(true)
        }

        async fn open(
            &self,
            _principal: &str,
            _identity: &str,
        ) -> Result<Arc<dyn ConsoleVoiceSession>, VoiceError> {
            Ok(Arc::clone(&self.session) as Arc<dyn ConsoleVoiceSession>)
        }
    }
}

/// A host whose readiness check parks in a chosen stage until released, and
/// records whether its future was dropped (cancelled) before finishing.
#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) mod readiness_test_support {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    pub(crate) struct ParkedReadyHost {
        pub calls: AtomicUsize,
        pub cancelled: AtomicUsize,
        pub started: Notify,
        pub release: tokio::sync::Semaphore,
        pub park_in: readiness_trace::VoiceReadinessStage,
    }

    impl ParkedReadyHost {
        pub(crate) fn new(park_in: readiness_trace::VoiceReadinessStage) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                cancelled: AtomicUsize::new(0),
                started: Notify::new(),
                release: tokio::sync::Semaphore::new(0),
                park_in,
            })
        }

        pub(crate) fn controller(self: &Arc<Self>) -> ConsoleVoiceController {
            ConsoleVoiceController {
                host: Some(Arc::clone(self) as Arc<dyn ConsoleVoiceHost>),
                ..ConsoleVoiceController::default()
            }
        }
    }

    struct CancelGuard<'a> {
        host: &'a ParkedReadyHost,
        finished: bool,
    }

    impl Drop for CancelGuard<'_> {
        fn drop(&mut self) {
            if !self.finished {
                self.host.cancelled.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    #[async_trait]
    impl ConsoleVoiceHost for ParkedReadyHost {
        async fn ready(&self, _principal: &str, _identity: &str) -> Result<bool, VoiceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut guard = CancelGuard {
                host: self,
                finished: false,
            };
            readiness_trace::enter(readiness_trace::VoiceReadinessStage::Target);
            readiness_trace::enter(self.park_in);
            self.started.notify_one();
            self.release.acquire().await.expect("release").forget();
            guard.finished = true;
            Ok(true)
        }

        async fn open(
            &self,
            _principal: &str,
            _identity: &str,
        ) -> Result<Arc<dyn ConsoleVoiceSession>, VoiceError> {
            Err(VoiceError::Unavailable)
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_contracts::{
        WireLiveChannelCapabilities, WireLiveContinuityMode, WireLiveTransportBootstrap,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::Semaphore;

    struct Session {
        close_calls: AtomicUsize,
        fail_close: AtomicBool,
        fail_context_read: AtomicBool,
        channel: std::sync::RwLock<String>,
        block_context: AtomicBool,
        context_started: Notify,
        release_context: Notify,
    }

    #[async_trait]
    impl ConsoleVoiceSession for Session {
        fn pending(&self) -> PendingLiveChannelHandle {
            PendingLiveChannelHandle {
                channel_id: self.channel.read().expect("channel").clone(),
                target_identity: "agent-a".to_string(),
                execution_mode: crate::live_contracts::LiveExecutionMode::ClientContext,
                pending_receipt: "opaque-pending".to_string(),
                transport: WireLiveTransportBootstrap::Webrtc {
                    token: "opaque-token".to_string(),
                    answer_method: "live/webrtc/answer".to_string(),
                    http_url: None,
                },
                capabilities: WireLiveChannelCapabilities {
                    audio_in: true,
                    audio_out: true,
                    text_in: false,
                    text_out: false,
                    image_in: false,
                    video_in: false,
                    transcript_supported: true,
                    barge_in_supported: true,
                    provider_native_resume: false,
                },
                continuity: WireLiveContinuityMode::TranscriptOnly,
                advisories: Vec::new(),
            }
        }

        async fn close(&self) -> Result<(), VoiceError> {
            self.close_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_close.load(Ordering::SeqCst) {
                Err(VoiceError::HostFailed)
            } else {
                Ok(())
            }
        }

        async fn answer_received(&self, channel: &str) -> Result<(), VoiceError> {
            if channel != self.pending().channel_id {
                return Err(VoiceError::RequestConflict);
            }
            Ok(())
        }

        async fn context_preparation(
            &self,
            channel: &str,
        ) -> Result<VoiceContextPreparation, VoiceError> {
            if self.fail_context_read.load(Ordering::SeqCst) {
                return Err(VoiceError::ContextReadFailed);
            }
            if channel != self.pending().channel_id {
                return Err(VoiceError::RequestConflict);
            }
            if self.block_context.load(Ordering::SeqCst) {
                self.context_started.notify_one();
                self.release_context.notified().await;
            }
            Ok(VoiceContextPreparation::NotRequested)
        }
    }

    struct Host {
        ready: AtomicBool,
        checked_identities: Mutex<Vec<String>>,
        opens: AtomicUsize,
        started: Notify,
        permit: Semaphore,
        session: Arc<Session>,
    }

    impl Host {
        fn new(ready: bool, blocked: bool) -> Arc<Self> {
            Arc::new(Self {
                ready: AtomicBool::new(ready),
                checked_identities: Mutex::new(Vec::new()),
                opens: AtomicUsize::new(0),
                started: Notify::new(),
                permit: Semaphore::new(usize::from(!blocked)),
                session: Arc::new(Session {
                    close_calls: AtomicUsize::new(0),
                    fail_close: AtomicBool::new(false),
                    fail_context_read: AtomicBool::new(false),
                    channel: std::sync::RwLock::new("test-channel".to_string()),
                    block_context: AtomicBool::new(false),
                    context_started: Notify::new(),
                    release_context: Notify::new(),
                }),
            })
        }
    }

    #[async_trait]
    impl ConsoleVoiceHost for Host {
        async fn ready(&self, _principal: &str, identity: &str) -> Result<bool, VoiceError> {
            self.checked_identities
                .lock()
                .await
                .push(identity.to_string());
            Ok(self.ready.load(Ordering::SeqCst))
        }

        async fn open(
            &self,
            _principal: &str,
            _identity: &str,
        ) -> Result<Arc<dyn ConsoleVoiceSession>, VoiceError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            // Like the live host, open resolves the target itself and
            // refuses an unavailable one; readiness is not consulted.
            if !self.ready.load(Ordering::SeqCst) {
                return Err(VoiceError::Unavailable);
            }
            self.started.notify_one();
            self.permit.acquire().await.expect("open permit").forget();
            Ok(Arc::clone(&self.session) as Arc<dyn ConsoleVoiceSession>)
        }
    }

    fn request() -> VoiceRequest {
        VoiceRequest {
            identity: "agent-a".to_string(),
            request_id: "request-a".to_string(),
        }
    }

    fn before_silence_expiry() -> Duration {
        SILENCE_LIMIT
            .checked_sub(Duration::from_secs(1))
            .expect("silence limit exceeds one second")
    }

    fn context_request() -> VoiceContextStatusRequest {
        VoiceContextStatusRequest {
            identity: "agent-a".to_string(),
            request_id: "request-a".to_string(),
            channel_id: "test-channel".to_string(),
        }
    }

    fn external_owner(identity: &str) -> LiveOwner {
        LiveOwner::ExternalLive {
            identity: identity.to_string(),
            channel_id: Some(format!("{identity}-channel")),
        }
    }

    fn noop_closer() -> LiveOwnerCloser {
        Arc::new(|_, _| Box::pin(async { Ok(()) }))
    }

    #[tokio::test]
    async fn external_live_engagement_supersedes_the_console_call_with_a_typed_reason() {
        let host = Host::new(true, false);
        let arbiter = Arc::new(LiveOwnerArbiter::default());
        let controller =
            ConsoleVoiceController::new(host.clone()).with_arbiter(Arc::clone(&arbiter));
        controller.open("alice", request()).await.expect("open");
        assert_eq!(
            arbiter.holder().await,
            Some(LiveOwner::ConsoleVoice {
                principal: "alice".to_string(),
                identity: "agent-a".to_string(),
                channel_id: Some("test-channel".to_string()),
            }),
            "the console call binds its channel to its engagement"
        );

        // The external live door takes the path: the console call is closed
        // through its ordinary teardown and every later request says why.
        let (_, preempted) = arbiter
            .engage(external_owner("agent-b"), noop_closer())
            .await
            .expect("external engagement");
        assert_eq!(preempted.map(|owner| owner.kind()), Some("console_voice"));
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 1);
        let superseded = VoiceError::Superseded(LiveSupersededReason::SupersededByExternalLive);
        assert_eq!(
            controller.context_status("alice", context_request()).await,
            Err(superseded)
        );
        assert_eq!(
            controller.replacement_required("alice", request()).await,
            Err(superseded)
        );
        assert_eq!(
            controller
                .note_activity(
                    "alice",
                    VoiceActivity {
                        identity: "agent-a".to_string(),
                        request_id: "request-a".to_string(),
                    },
                )
                .await,
            Err(superseded)
        );
        let error = superseded.rpc_error();
        assert_eq!(error.code, -32000);
        assert_eq!(
            error.data,
            Some(serde_json::json!({
                "kind": "voice_superseded",
                "reason": "superseded_by_external_live",
            }))
        );
        // Closing the superseded call is still an ordinary, idempotent close.
        controller
            .close("alice", request())
            .await
            .expect("close superseded");
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            arbiter.holder().await.map(|owner| owner.kind()),
            Some("external_live"),
            "a superseded console close never evicts the owner that replaced it"
        );
    }

    #[tokio::test]
    async fn console_open_takes_the_voice_path_back_from_an_external_channel() {
        let host = Host::new(true, false);
        let arbiter = Arc::new(LiveOwnerArbiter::default());
        let controller =
            ConsoleVoiceController::new(host.clone()).with_arbiter(Arc::clone(&arbiter));
        let closes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = Arc::clone(&closes);
        let closer: LiveOwnerCloser = Arc::new(move |reason, channel| {
            let recorder = Arc::clone(&recorder);
            Box::pin(async move {
                recorder.lock().expect("closes").push((reason, channel));
                Ok(())
            })
        });
        arbiter
            .engage(external_owner("agent-b"), closer)
            .await
            .expect("external engagement");

        let readiness = controller
            .readiness("alice", "agent-a")
            .await
            .expect("readiness");
        assert!(!readiness.available);
        assert_eq!(readiness.reason, Some("external_live_active"));
        assert_eq!(
            readiness.to_wire("agent-a"),
            serde_json::json!({
                "identity": "agent-a",
                "available": false,
                "reason": "external_live_active",
                "holder": { "identity": "agent-b", "channel_id": "agent-b-channel" },
            })
        );
        assert!(
            host.checked_identities.lock().await.is_empty(),
            "readiness held by the external channel must not probe the console host"
        );

        controller
            .open("alice", request())
            .await
            .expect("open preempts");
        assert_eq!(
            closes.lock().expect("closes").as_slice(),
            &[(
                LiveSupersededReason::SupersededByConsoleVoice,
                Some("agent-b-channel".to_string())
            )]
        );
        assert_eq!(
            arbiter.close_reason("agent-b-channel"),
            Some(LiveSupersededReason::SupersededByConsoleVoice)
        );
        assert_eq!(
            arbiter.holder().await.map(|owner| owner.kind()),
            Some("console_voice")
        );
        let readiness = controller
            .readiness("alice", "agent-a")
            .await
            .expect("readiness");
        assert!(readiness.available);
        assert_eq!(
            readiness.to_wire("agent-a"),
            serde_json::json!({"identity":"agent-a","available":true})
        );

        // Ending the call frees the path again.
        controller.close("alice", request()).await.expect("close");
        assert!(arbiter.holder().await.is_none());
    }

    #[tokio::test]
    async fn a_console_open_that_loses_the_race_while_opening_closes_what_it_opened() {
        // The host blocks inside open until the test releases it.
        let host = Host::new(true, true);
        let arbiter = Arc::new(LiveOwnerArbiter::default());
        let controller =
            ConsoleVoiceController::new(host.clone()).with_arbiter(Arc::clone(&arbiter));
        let opening = {
            let controller = controller.clone();
            tokio::spawn(async move { controller.open("alice", request()).await })
        };
        host.started.notified().await;
        assert_eq!(
            arbiter.holder().await,
            Some(LiveOwner::ConsoleVoice {
                principal: "alice".to_string(),
                identity: "agent-a".to_string(),
                channel_id: None,
            }),
            "the console engaged before its provider open completed"
        );
        // The external door wins while the console open is still in flight.
        let (_, preempted) = arbiter
            .engage(external_owner("agent-b"), noop_closer())
            .await
            .expect("external engagement");
        assert_eq!(preempted.map(|owner| owner.kind()), Some("console_voice"));
        assert_eq!(
            host.session.close_calls.load(Ordering::SeqCst),
            0,
            "nothing to close yet"
        );
        // The console open now completes: its channel must not stay live
        // beside the external owner.
        host.permit.add_permits(1);
        let result = opening.await.expect("open task");
        assert_eq!(
            result,
            Err(VoiceError::Superseded(
                LiveSupersededReason::SupersededByExternalLive
            ))
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while host.session.close_calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the late console channel is closed");
        assert_eq!(
            arbiter.holder().await.map(|owner| owner.kind()),
            Some("external_live")
        );
        assert_eq!(
            controller.replacement_required("alice", request()).await,
            Err(VoiceError::Superseded(
                LiveSupersededReason::SupersededByExternalLive
            ))
        );
    }

    #[tokio::test]
    async fn a_dead_external_channel_never_locks_the_console_out() {
        let host = Host::new(true, false);
        let arbiter = Arc::new(LiveOwnerArbiter::default());
        let live: Arc<std::sync::Mutex<std::collections::HashSet<String>>> = Arc::default();
        let probe = Arc::clone(&live);
        arbiter.set_liveness(Arc::new(move |channel| {
            let probe = Arc::clone(&probe);
            Box::pin(async move { probe.lock().expect("live").contains(&channel) })
        }));
        let controller =
            ConsoleVoiceController::new(host.clone()).with_arbiter(Arc::clone(&arbiter));
        // The console's own channel is live once it opens.
        live.lock()
            .expect("live")
            .insert("test-channel".to_string());
        // The external closer fails the way the real one would for a channel
        // the machine no longer knows.
        let failing: LiveOwnerCloser =
            Arc::new(|_, _| Box::pin(async { Err("BindingMismatch".to_string()) }));
        arbiter
            .engage(external_owner("agent-b"), failing)
            .await
            .expect("external engagement");
        live.lock()
            .expect("live")
            .insert("agent-b-channel".to_string());
        assert_eq!(
            controller
                .readiness("alice", "agent-a")
                .await
                .expect("readiness")
                .reason,
            Some("external_live_active")
        );
        // the device daemon's socket drops; meerkat-live closes the channel by itself.
        live.lock().expect("live").remove("agent-b-channel");
        let readiness = controller
            .readiness("alice", "agent-a")
            .await
            .expect("readiness");
        assert!(readiness.available, "{readiness:?}");
        assert!(readiness.reason.is_none());
        controller
            .open("alice", request())
            .await
            .expect("console open over a dead channel");
        assert_eq!(
            arbiter.holder().await.map(|owner| owner.kind()),
            Some("console_voice")
        );
    }

    #[tokio::test]
    async fn a_failed_external_close_fails_the_console_open_closed() {
        let host = Host::new(true, false);
        let arbiter = Arc::new(LiveOwnerArbiter::default());
        let controller =
            ConsoleVoiceController::new(host.clone()).with_arbiter(Arc::clone(&arbiter));
        let failing: LiveOwnerCloser =
            Arc::new(|_, _| Box::pin(async { Err("provider close hung".to_string()) }));
        arbiter
            .engage(external_owner("agent-b"), failing)
            .await
            .expect("external engagement");
        assert_eq!(
            controller.open("alice", request()).await,
            Err(VoiceError::HostFailed)
        );
        assert_eq!(
            host.opens.load(Ordering::SeqCst),
            0,
            "no provider open behind a live owner"
        );
        assert_eq!(
            arbiter.holder().await.map(|owner| owner.kind()),
            Some("external_live")
        );
    }

    #[tokio::test]
    async fn context_read_errors_are_rpc_errors_not_preparation_failure_or_ack() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        controller.open("alice", request()).await.expect("open");
        host.session.fail_context_read.store(true, Ordering::SeqCst);
        let result = controller.context_status("alice", context_request()).await;
        assert_eq!(result, Err(VoiceError::ContextReadFailed));
        let error = VoiceError::ContextReadFailed.rpc_error();
        assert_eq!(error.code, -32000);
        assert_eq!(
            error.data,
            Some(serde_json::json!({"kind":"voice_context_read_failed"}))
        );
        controller.close("alice", request()).await.expect("close");
    }

    #[tokio::test(start_paused = true)]
    async fn context_status_is_exact_request_owned_and_does_not_extend_audio_activity() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host);
        controller.open("alice", request()).await.expect("pending");
        let slot = controller
            .request_slot("alice", &request())
            .await
            .expect("slot");
        assert_eq!(
            controller.context_status("", context_request()).await,
            Err(VoiceError::Unauthorized)
        );
        assert_eq!(
            controller.context_status("bob", context_request()).await,
            Err(VoiceError::RequestConflict)
        );
        for request in [
            VoiceContextStatusRequest {
                identity: "agent-b".to_string(),
                ..context_request()
            },
            VoiceContextStatusRequest {
                request_id: "other".to_string(),
                ..context_request()
            },
            VoiceContextStatusRequest {
                channel_id: "other".to_string(),
                ..context_request()
            },
        ] {
            assert_eq!(
                controller.context_status("alice", request).await,
                Err(VoiceError::RequestConflict)
            );
        }

        assert_eq!(
            controller
                .context_status("alice", context_request())
                .await
                .expect("read")
                .context_preparation,
            VoiceContextPreparation::NotRequested,
        );
        assert!(!slot.state.lock().await.activated);
        assert!(slot.state.lock().await.last_activity.is_none());
        controller
            .answer_received("alice", request(), "test-channel")
            .await
            .expect("activate");
        let activity = slot.state.lock().await.last_activity;
        tokio::time::advance(before_silence_expiry()).await;
        controller
            .context_status("alice", context_request())
            .await
            .expect("active read");
        assert_eq!(slot.state.lock().await.last_activity, activity);
        tokio::time::advance(Duration::from_secs(2)).await;
        slot.wait_closed().await.expect("silence expiry");
        assert_eq!(
            controller.context_status("alice", context_request()).await,
            Err(VoiceError::Closed)
        );
    }

    #[tokio::test]
    async fn delayed_context_read_does_not_block_activation_close_or_channel_fences() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        controller.open("alice", request()).await.expect("open");
        host.session.block_context.store(true, Ordering::SeqCst);
        let reader = controller.clone();
        let pending =
            tokio::spawn(async move { reader.context_status("alice", context_request()).await });
        host.session.context_started.notified().await;
        controller
            .answer_received("alice", request(), "test-channel")
            .await
            .expect("activation while read pending");
        *host.session.channel.write().expect("channel") = "replacement-channel".to_string();
        host.session.release_context.notify_one();
        assert_eq!(
            pending.await.expect("read task"),
            Err(VoiceError::RequestConflict)
        );
        assert_eq!(
            controller.context_status("alice", context_request()).await,
            Err(VoiceError::RequestConflict)
        );
        let reader = controller.clone();
        let pending = tokio::spawn(async move {
            reader
                .context_status(
                    "alice",
                    VoiceContextStatusRequest {
                        channel_id: "replacement-channel".to_string(),
                        ..context_request()
                    },
                )
                .await
        });
        host.session.context_started.notified().await;
        controller
            .close("alice", request())
            .await
            .expect("close while read pending");
        host.session.release_context.notify_one();
        assert_eq!(pending.await.expect("read task"), Err(VoiceError::Closed));
    }

    #[test]
    fn context_status_rejects_extra_or_missing_scope_fields() {
        for value in [
            serde_json::json!({"identity":"agent-a","request_id":"request-a"}),
            serde_json::json!({"identity":"agent-a","channel_id":"test-channel"}),
            serde_json::json!({"request_id":"request-a","channel_id":"test-channel"}),
            serde_json::json!({"identity":"agent-a","request_id":"request-a","channel_id":"test-channel",
                "pending_receipt":"caller-cannot-supply-authority"}),
        ] {
            assert!(serde_json::from_value::<VoiceContextStatusRequest>(value).is_err());
        }
    }

    #[tokio::test]
    async fn open_does_not_probe_readiness_again_and_surfaces_the_host_open_error() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        controller.open("alice", request()).await.expect("open");
        assert!(
            host.checked_identities.lock().await.is_empty(),
            "the click's readiness RPC is the one probe; open must not repeat it"
        );
        assert_eq!(host.opens.load(Ordering::SeqCst), 1);
        controller.close("alice", request()).await.expect("close");
        host.ready.store(false, Ordering::SeqCst);
        let denied = VoiceRequest {
            identity: "agent-a".to_string(),
            request_id: "request-b".to_string(),
        };
        assert_eq!(
            controller.open("alice", denied.clone()).await,
            Err(VoiceError::Unavailable),
            "the host's own target resolution refuses an unavailable target"
        );
        assert_eq!(host.opens.load(Ordering::SeqCst), 2);
        assert!(host.checked_identities.lock().await.is_empty());
        // A refused open leaves no live slot behind: the same principal may
        // try again at once instead of being reported busy.
        host.ready.store(true, Ordering::SeqCst);
        host.permit.add_permits(1);
        controller
            .open(
                "alice",
                VoiceRequest {
                    identity: "agent-a".to_string(),
                    request_id: "request-c".to_string(),
                },
            )
            .await
            .expect("reopen after refusal");
    }

    #[tokio::test]
    async fn configuration_discovery_does_not_probe_and_readiness_targets_one_identity() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        assert!(controller.configured());
        assert!(host.checked_identities.lock().await.is_empty());
        assert!(
            controller
                .ready("alice", "agent-a")
                .await
                .expect("target readiness")
        );
        assert_eq!(
            *host.checked_identities.lock().await,
            vec!["agent-a".to_string()]
        );
        assert_eq!(host.opens.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn missing_host_and_missing_auth_never_open() {
        assert_eq!(
            ConsoleVoiceController::default()
                .open("alice", request())
                .await,
            Err(VoiceError::Unavailable)
        );
        let host = Host::new(false, false);
        let controller = ConsoleVoiceController::new(host.clone());
        assert_eq!(
            controller.open("alice", request()).await,
            Err(VoiceError::Unavailable)
        );
        // The host's own target resolution is the one validation on the open
        // path; it refused this open and left no slot behind.
        assert_eq!(host.opens.load(Ordering::SeqCst), 1);
        host.ready.store(true, Ordering::SeqCst);
        assert_eq!(
            controller.open("", request()).await,
            Err(VoiceError::Unauthorized)
        );
        assert_eq!(
            host.opens.load(Ordering::SeqCst),
            1,
            "a missing principal is refused before the host is contacted"
        );
    }

    #[tokio::test]
    async fn repeated_open_is_idempotent_and_request_cannot_retarget() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        let pending = controller.open("alice", request()).await.expect("open");
        assert_eq!(
            controller.open("alice", request()).await.expect("retry"),
            pending
        );
        let mut retargeted = request();
        retargeted.identity = "agent-b".to_string();
        assert_eq!(
            controller.open("alice", retargeted.clone()).await,
            Err(VoiceError::RequestConflict)
        );
        assert_eq!(
            controller.close("alice", retargeted).await,
            Err(VoiceError::RequestConflict)
        );
        assert_eq!(host.opens.load(Ordering::SeqCst), 1);
        controller.close("alice", request()).await.expect("close");
    }

    #[tokio::test]
    async fn close_before_open_retains_cancellation_fence() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        controller.close("alice", request()).await.expect("fence");
        assert_eq!(
            controller.open("alice", request()).await,
            Err(VoiceError::Cancelled)
        );
        assert_eq!(host.opens.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn late_open_is_closed_even_after_both_http_waiters_disconnect() {
        let host = Host::new(true, true);
        let controller = ConsoleVoiceController::new(host.clone());
        let opener = controller.clone();
        let open = tokio::spawn(async move { opener.open("alice", request()).await });
        host.started.notified().await;
        open.abort();
        let slot = controller
            .requests
            .lock()
            .await
            .get(&("alice".to_string(), "request-a".to_string()))
            .expect("slot")
            .clone();
        slot.cancel().await;
        let closer = controller.clone();
        let close = tokio::spawn(async move { closer.close("alice", request()).await });
        close.abort();
        assert!(!slot.state.lock().await.closed);
        host.permit.add_permits(1);
        tokio::time::timeout(Duration::from_secs(1), slot.wait_closed())
            .await
            .expect("cleanup completes")
            .expect("closed");
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            controller.open("alice", request()).await,
            Err(VoiceError::Cancelled)
        );
    }

    #[tokio::test]
    async fn text_cannot_be_reported_as_voice_activity() {
        assert!(
            serde_json::from_value::<VoiceActivity>(serde_json::json!({
                "identity":"agent-a", "request_id":"request-a"
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<VoiceActivity>(serde_json::json!({
                "identity":"agent-a", "request_id":"request-a", "kind":"text"
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn readiness_does_not_open_and_remains_available_for_an_active_voice_target() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        assert!(
            controller
                .ready("alice", "agent-a")
                .await
                .expect("readiness")
        );
        assert_eq!(host.opens.load(Ordering::SeqCst), 0);
        controller.open("alice", request()).await.expect("open");
        controller
            .answer_received("alice", request(), "test-channel")
            .await
            .expect("activation");
        assert!(
            controller
                .request_slot("alice", &request())
                .await
                .expect("slot")
                .state
                .lock()
                .await
                .activated
        );
        assert!(
            controller
                .ready("alice", "agent-a")
                .await
                .expect("active readiness")
        );
        assert_eq!(host.opens.load(Ordering::SeqCst), 1);
        host.ready.store(false, Ordering::SeqCst);
        assert!(
            !controller
                .ready("alice", "agent-a")
                .await
                .expect("revoked readiness")
        );
        // Re-opening the same active request stays idempotent (its existing
        // handle, no second channel); a new request for the same principal
        // is refused because the active call still owns the target.
        assert_eq!(
            controller
                .open("alice", request())
                .await
                .expect("idempotent retry"),
            host.session.pending()
        );
        let second = VoiceRequest {
            identity: "agent-a".to_string(),
            request_id: "request-b".to_string(),
        };
        assert_eq!(
            controller.open("alice", second.clone()).await,
            Err(VoiceError::Busy)
        );
        assert_eq!(
            host.opens.load(Ordering::SeqCst),
            1,
            "active readiness must not open a second channel"
        );
        controller.close("alice", request()).await.expect("close");
        assert_eq!(
            controller.replacement_required("alice", request()).await,
            Err(VoiceError::Closed),
        );
        // With the call ended, the revoked target is refused by the host's
        // own resolution on the open path, exactly as readiness reports it.
        assert_eq!(
            controller.open("alice", second).await,
            Err(VoiceError::Unavailable)
        );
        assert_eq!(host.opens.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn expensive_open_does_not_consume_the_voice_silence_window() {
        let host = Host::new(true, true);
        let controller = ConsoleVoiceController::new(host.clone());
        let opener = controller.clone();
        let open = tokio::spawn(async move { opener.open("alice", request()).await });
        host.started.notified().await;
        let slot = controller
            .request_slot("alice", &request())
            .await
            .expect("slot");
        tokio::time::advance(SILENCE_LIMIT + Duration::from_secs(1)).await;
        assert!(
            !slot.state.lock().await.cancelled,
            "setup is not voice silence"
        );
        host.permit.add_permits(1);
        open.await.expect("open task").expect("pending handle");
        assert!(slot.state.lock().await.last_activity.is_none());
        controller
            .answer_received("alice", request(), "test-channel")
            .await
            .expect("activation");
        tokio::time::advance(before_silence_expiry()).await;
        assert!(!slot.state.lock().await.cancelled);
        controller.close("alice", request()).await.expect("close");
    }

    #[tokio::test(start_paused = true)]
    async fn abandoned_pending_setup_has_a_separate_cleanup_deadline() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        controller
            .open("alice", request())
            .await
            .expect("pending handle");
        let slot = controller
            .request_slot("alice", &request())
            .await
            .expect("slot");
        assert!(slot.state.lock().await.last_activity.is_none());
        tokio::time::advance(PENDING_SETUP_LIMIT).await;
        slot.wait_closed().await.expect("abandoned setup cleaned");
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn activation_starts_silence_window_but_replacement_ack_does_not_extend_it() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        controller.open("alice", request()).await.expect("open");
        tokio::time::advance(Duration::from_secs(30)).await;
        controller
            .answer_received("alice", request(), "test-channel")
            .await
            .expect("initial activation");
        tokio::time::advance(before_silence_expiry()).await;
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 0);
        controller
            .answer_received("alice", request(), "test-channel")
            .await
            .expect("repeated activation");
        tokio::time::advance(Duration::from_secs(1)).await;
        controller
            .request_slot("alice", &request())
            .await
            .expect("slot")
            .wait_closed()
            .await
            .expect("closed");
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn server_silence_watchdog_uses_only_explicit_audio_activity() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        controller.open("alice", request()).await.expect("open");
        controller
            .answer_received("alice", request(), "test-channel")
            .await
            .expect("activation acknowledgement");
        let slot = controller
            .request_slot("alice", &request())
            .await
            .expect("slot");
        tokio::time::advance(before_silence_expiry()).await;
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 0);
        controller
            .note_activity(
                "alice",
                VoiceActivity {
                    identity: request().identity,
                    request_id: request().request_id,
                },
            )
            .await
            .expect("actual model audio");
        tokio::time::advance(before_silence_expiry()).await;
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 0);
        tokio::time::advance(Duration::from_secs(1)).await;
        slot.wait_closed()
            .await
            .expect("silence closes through host");
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn close_is_principal_scoped_and_survives_readiness_revocation() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        controller.open("alice", request()).await.expect("open");
        controller
            .close("bob", request())
            .await
            .expect("other principal tombstone");
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 0);
        host.ready.store(false, Ordering::SeqCst);
        controller
            .close("alice", request())
            .await
            .expect("owner close");
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn closed_requests_are_reaped_after_retention_and_capacity_recovers() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        controller.open("alice", request()).await.expect("open");
        controller.close("alice", request()).await.expect("close");
        assert_eq!(controller.requests.lock().await.len(), 1);
        let mut foreign = request();
        foreign.request_id = "bob-late-close".to_string();
        controller.close("bob", foreign).await.expect("tombstone");
        {
            let mut requests = controller.requests.lock().await;
            assert_eq!(requests.len(), 2, "closed slots are retained for a while");
            let now = tokio::time::Instant::now();
            reap_closed_requests(&mut requests, now).await;
            assert_eq!(requests.len(), 2, "retention keeps recent closed slots");
            reap_closed_requests(
                &mut requests,
                now + CLOSED_RETENTION + Duration::from_secs(1),
            )
            .await;
            assert_eq!(requests.len(), 0, "expired closed slots are reaped");
        }
        let mut third = request();
        third.request_id = "third-request".to_string();
        // The fake host grants one open per permit; allow the reopen.
        host.permit.add_permits(1);
        controller
            .open("alice", third)
            .await
            .expect("open after retention is not refused by capacity or busy");
        let requests = controller.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert!(requests.contains_key(&("alice".to_string(), "third-request".to_string())));
    }

    #[tokio::test]
    async fn foreign_close_tombstones_are_bounded_per_principal() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        controller.open("alice", request()).await.expect("open");
        for index in 0..(MAX_CLOSED_PER_PRINCIPAL * 3) {
            let mut foreign = request();
            foreign.request_id = format!("bob-{index}");
            controller
                .close("bob", foreign)
                .await
                .expect("foreign close leaves a bounded tombstone");
        }
        let requests = controller.requests.lock().await;
        let bob = requests.keys().filter(|(owner, _)| owner == "bob").count();
        assert!(
            bob <= MAX_CLOSED_PER_PRINCIPAL,
            "bob retains at most {MAX_CLOSED_PER_PRINCIPAL} closed slots, found {bob}"
        );
        assert!(
            requests.contains_key(&("alice".to_string(), request().request_id)),
            "the live call of another principal is untouched"
        );
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn close_failure_remains_retryable_and_blocks_another_open() {
        let host = Host::new(true, false);
        let controller = ConsoleVoiceController::new(host.clone());
        controller.open("alice", request()).await.expect("open");
        host.session.fail_close.store(true, Ordering::SeqCst);
        assert_eq!(
            controller.close("alice", request()).await,
            Err(VoiceError::HostFailed)
        );
        let mut next = request();
        next.request_id = "next-request".to_string();
        assert_eq!(controller.open("alice", next).await, Err(VoiceError::Busy));
        host.session.fail_close.store(false, Ordering::SeqCst);
        controller
            .close("alice", request())
            .await
            .expect("retry closes");
        controller
            .close("alice", request())
            .await
            .expect("close idempotent");
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn close_timeout_never_claims_closed_and_eventual_cleanup_is_retained() {
        let host = Host::new(true, true);
        let controller = ConsoleVoiceController::new(host.clone());
        let opener = controller.clone();
        let open = tokio::spawn(async move { opener.open("alice", request()).await });
        host.started.notified().await;
        assert_eq!(
            controller.close("alice", request()).await,
            Err(VoiceError::Busy)
        );
        assert_eq!(open.await.expect("open waiter"), Err(VoiceError::Cancelled));
        host.permit.add_permits(1);
        controller
            .close("alice", request())
            .await
            .expect("eventual close");
        assert_eq!(host.session.close_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn voice_request_rejects_authority_fields_and_ambiguous_identifiers() {
        assert!(
            serde_json::from_value::<VoiceRequest>(serde_json::json!({
                "identity": "agent-a", "request_id": "r", "principal": "administrator"
            }))
            .is_err()
        );
        for identity in ["", " agent-a", "agent-a ", "rt:other"] {
            let mut invalid = request();
            invalid.identity = identity.to_string();
            assert_eq!(invalid.validate(), Err(VoiceError::InvalidRequest));
        }
    }

    #[tokio::test]
    async fn concurrent_readiness_checks_share_one_host_check_and_a_finished_one_is_never_joined() {
        use readiness_test_support::ParkedReadyHost;
        use readiness_trace::VoiceReadinessStage;
        let host = ParkedReadyHost::new(VoiceReadinessStage::DurableSource);
        let controller = host.controller();
        let first = controller
            .readiness_flight("alice", "agent-a")
            .expect("first flight");
        let second = controller
            .readiness_flight("alice", "agent-a")
            .expect("second flight");
        let other_principal = controller
            .readiness_flight("bob", "agent-a")
            .expect("other principal flight");
        assert!(!first.joined);
        assert!(second.joined, "a check in flight is joined");
        assert!(!other_principal.joined, "flights are per principal");
        let first_result = tokio::spawn(first.result.clone());
        let second_result = tokio::spawn(second.result.clone());
        let other_result = tokio::spawn(other_principal.result.clone());
        tokio::time::timeout(Duration::from_secs(5), async {
            while host.calls.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both checks started");
        let running = first.trace.snapshot();
        assert_eq!(running.active, Some(VoiceReadinessStage::DurableSource));
        assert!(Arc::ptr_eq(&first.trace, &second.trace));
        host.release.add_permits(2);
        for answer in [first_result, second_result, other_result] {
            assert!(answer.await.expect("join").expect("ready").available);
        }
        assert_eq!(
            host.calls.load(Ordering::SeqCst),
            2,
            "alice's two concurrent checks ran the host once"
        );
        assert_eq!(first.trace.snapshot().active, None, "finished");
        drop((first, second, other_principal));
        let fresh = controller
            .readiness_flight("alice", "agent-a")
            .expect("fresh flight");
        assert!(!fresh.joined, "a finished check is never joined");
        host.release.add_permits(1);
        assert!(fresh.result.await.expect("ready").available);
        assert_eq!(host.calls.load(Ordering::SeqCst), 3);
        assert_eq!(host.cancelled.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn dropping_every_hold_cancels_the_readiness_check() {
        use readiness_test_support::ParkedReadyHost;
        use readiness_trace::VoiceReadinessStage;
        let host = ParkedReadyHost::new(VoiceReadinessStage::Probe);
        let controller = host.controller();
        let first = controller
            .readiness_flight("alice", "agent-a")
            .expect("first");
        let joined = controller
            .readiness_flight("alice", "agent-a")
            .expect("joined");
        let started = host.started.notified();
        let polling = tokio::spawn(first.result.clone());
        started.await;
        polling.abort();
        let _ = polling.await;
        drop(first);
        assert_eq!(
            host.cancelled.load(Ordering::SeqCst),
            0,
            "a remaining hold keeps the check alive"
        );
        drop(joined);
        assert_eq!(
            host.cancelled.load(Ordering::SeqCst),
            1,
            "the last hold dropped cancels the host check"
        );
        let retry = controller
            .readiness_flight("alice", "agent-a")
            .expect("retry");
        assert!(!retry.joined, "a cancelled check is not joined");
        host.release.add_permits(1);
        assert!(retry.result.await.expect("ready").available);
        assert_eq!(host.calls.load(Ordering::SeqCst), 2);
    }
}
