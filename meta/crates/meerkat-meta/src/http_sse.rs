//! Server-Sent Events (SSE) streaming endpoints for agent and mob observation.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_stream::stream;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::{Json, Router};
use futures::StreamExt;
use meerkat_core::AgentEvent;
use meerkat_core::comms::EventStream;
use meerkat_core::event::agent_event_type;
use meerkat_mob::{MobEventRouterHandle, MobHandle};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::access::{
    ACTION_AGENT_VIEW, ACTION_MOB_OBSERVE, ACTION_WORKGRAPH_VIEW, AccessController, AccessView,
    AgentResourceAttributes, LiveAccessView,
};
use crate::auth::session_fence::ConsoleSessionFences;
use crate::console_aggregator::{
    AllowAllConsoleVisibilityPolicy, ConsoleCursor, ConsoleFrame, ConsoleFrameSource,
    ConsoleFrameSourceKind, ConsoleFrameStatus, ConsoleVisibilityPolicy, NewConsoleFrame,
};
use crate::runtime::{
    ConsoleCallerVerification, RuntimeDecisionState, extract_bearer_token_from_header,
};
use crate::unified_runtime::EventQuery;
use crate::unified_runtime::mob_events::{
    MOB_EVENTS_STREAM_PATH, MobEventsStore, ProjectedMobEvent,
};
use crate::workgraph_events::{WorkGraphFactHub, workgraph_resync_required_event};

use crate::mob_handle_runtime::{MobRuntime, MobRuntimeError};
use meerkat_core::comms::SendError;
use meerkat_core::service::SessionError;
use meerkat_mob::MobError;

pub(crate) const DEFAULT_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);
pub(crate) const KEEP_ALIVE_TEXT: &str = "keep-alive";

pub(crate) use crate::mob_handle_runtime::console_agent_event_payload;

/// Lossy WorkGraph wake stream. Durable WorkGraph pull methods remain the
/// only state and replay authority.
pub const WORKGRAPH_FACTS_STREAM_PATH: &str = "/meta/workgraph/facts/stream";

/// What woke a stream that follows the access policy and its caller's
/// authentication.
pub(crate) enum StreamWake<T> {
    /// The verifier no longer accepts the token the stream's caller opened
    /// it with; the stream ends.
    AuthenticationLapsed,
    /// The access owner committed a change; the stream's view is rebuilt.
    PolicyChanged,
    /// The stream's source produced this.
    Item(T),
}

/// Wait for the stream's next item, or, while there is none, for a
/// committed access change or the lapse of the caller's authentication.
/// Each is watched whether or not the other exists. `next` must be
/// cancellation-safe: a change or a lapse drops it.
pub(crate) async fn wake_or_policy_change<F: Future>(
    live_view: Option<&mut LiveAccessView>,
    authentication: Option<&mut StreamAuthentication>,
    next: F,
) -> StreamWake<F::Output> {
    let policy_changed = async {
        match live_view {
            Some(live_view) => live_view.changed().await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        biased;
        () = authentication_lapsed(authentication) => StreamWake::AuthenticationLapsed,
        () = policy_changed => StreamWake::PolicyChanged,
        item = next => StreamWake::Item(item),
    }
}

/// Completes once the caller's authentication lapsed; never for a stream
/// whose caller no token identified.
pub(crate) async fn authentication_lapsed(authentication: Option<&mut StreamAuthentication>) {
    match authentication {
        Some(authentication) => authentication.lapsed().await,
        None => std::future::pending().await,
    }
}

/// The gateway HTTP server's drain as its event streams see it. The server
/// adds it to every request it serves; when its HTTP drain begins (at
/// shutdown, after any in-flight drain), a stream that watches it ends, so
/// the bounded drain can join it. A server that only stops accepting
/// connections leaves its streams open. A stream served without one never
/// ends this way.
#[derive(Clone)]
pub(crate) struct StreamShutdown(tokio::sync::watch::Receiver<bool>);

impl StreamShutdown {
    pub(crate) fn new(stopping: tokio::sync::watch::Receiver<bool>) -> Self {
        Self(stopping)
    }
}

/// Completes once the HTTP drain of the gateway serving the stream began (or
/// its server is gone); never for a stream served without a
/// [`StreamShutdown`].
pub(crate) async fn stream_shutdown_begun(shutdown: Option<&mut StreamShutdown>) {
    match shutdown {
        Some(shutdown) => {
            let _ = shutdown.0.wait_for(|stopping| *stopping).await;
        }
        None => std::future::pending().await,
    }
}

/// `next`, or `None` once the HTTP drain of the gateway serving the stream
/// began. `next` must be cancellation-safe: the drain drops it.
pub(crate) async fn until_stream_shutdown<F: Future>(
    shutdown: Option<&mut StreamShutdown>,
    next: F,
) -> Option<F::Output> {
    tokio::select! {
        biased;
        () = stream_shutdown_begun(shutdown) => None,
        item = next => Some(item),
    }
}

/// [`until_stream_shutdown`] for a source that ends with `None`: the stream
/// ends either way.
pub(crate) async fn next_until_stream_shutdown<T, F: Future<Output = Option<T>>>(
    shutdown: Option<&mut StreamShutdown>,
    next: F,
) -> Option<T> {
    until_stream_shutdown(shutdown, next).await.flatten()
}

/// Whether the stream's caller is still authenticated. A stream checks it
/// after the last await of a frame's processing, before it decides the
/// frame. A caller no token identified always is.
pub(crate) fn still_authenticated(authentication: Option<&mut StreamAuthentication>) -> bool {
    authentication.is_none_or(StreamAuthentication::still_current)
}

/// The wall clock a stream reads its caller's token deadline against.
pub(crate) type WallClock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// The authentication of a stream's caller, held for as long as the stream
/// stays open. A stream whose caller a token identified ends once the
/// Console's verifier no longer accepts that token: the moment a request
/// with the token is refused, on its expiry or on a session fence set for
/// its principal. It is built from the verifier's retained verification and
/// asks the verifier itself, never reads the token, and does not depend on
/// an access controller.
pub(crate) struct StreamAuthentication {
    verification: ConsoleCallerVerification,
    /// The first instant the verifier refuses the token, from its latest
    /// answer; `None` when the token has no `exp`.
    refused_from: Option<SystemTime>,
    /// Changes after each change to the Console's session fence record;
    /// `None` once the record is gone and can change no more.
    fence_changes: Option<tokio::sync::watch::Receiver<u64>>,
    lapsed: bool,
    clock: WallClock,
}

impl StreamAuthentication {
    pub(crate) fn new(verification: ConsoleCallerVerification) -> Self {
        Self::with_clock(verification, Arc::new(SystemTime::now))
    }

    /// [`Self::new`] reading the wall clock through `clock`.
    pub(crate) fn with_clock(verification: ConsoleCallerVerification, clock: WallClock) -> Self {
        Self {
            refused_from: verification.refused_from(),
            fence_changes: Some(verification.session_fence_changes()),
            verification,
            lapsed: false,
            clock,
        }
    }

    /// Whether the caller is still authenticated now. After a change to the
    /// session fence record the verifier decides at once. Otherwise, before
    /// the verifier's deadline this only reads the clock; from the deadline
    /// on, the verifier decides, with the acceptance a request goes through.
    pub(crate) fn still_current(&mut self) -> bool {
        if self.lapsed {
            return false;
        }
        let now = (self.clock)();
        if self.take_fence_change() {
            return self.verify(now);
        }
        let Some(refused_from) = self.refused_from else {
            return true;
        };
        now < refused_from || self.verify(now)
    }

    /// Whether the session fence record changed since the verifier last
    /// answered; marks the change seen.
    fn take_fence_change(&mut self) -> bool {
        let Some(fence_changes) = self.fence_changes.as_mut() else {
            return false;
        };
        match fence_changes.has_changed() {
            Ok(true) => {
                fence_changes.mark_unchanged();
                true
            }
            Ok(false) => false,
            Err(_) => {
                self.fence_changes = None;
                false
            }
        }
    }

    /// Completes once the verifier no longer accepts the caller's token. It
    /// sleeps until the verifier's deadline (never, for a token without
    /// `exp`) or until the session fence record changes, whichever comes
    /// first, then asks the verifier. A wake the verifier still accepts
    /// (another principal's fence, or a wall clock set back while the stream
    /// slept) waits again, for the deadline of the verifier's fresh answer
    /// and the next fence change. Cancellation-safe: dropped while it waits,
    /// it changes nothing.
    pub(crate) async fn lapsed(&mut self) {
        let mut woken_at_deadline = false;
        while !self.lapsed {
            let wait = self.refused_from.map(|refused_from| {
                let wait = refused_from
                    .duration_since((self.clock)())
                    .unwrap_or_default();
                if woken_at_deadline && wait.is_zero() {
                    // The verifier accepted the token at an instant it names
                    // as refused: ask again a second later, never in a busy
                    // loop.
                    Duration::from_secs(1)
                } else {
                    wait
                }
            });
            let deadline = async {
                match wait {
                    Some(wait) if !wait.is_zero() => tokio::time::sleep(wait).await,
                    Some(_) => {}
                    None => std::future::pending().await,
                }
            };
            let fence_changed = async {
                match self.fence_changes.as_mut() {
                    Some(fence_changes) => fence_changes.changed().await.is_ok(),
                    None => std::future::pending().await,
                }
            };
            let woken_by_fence = tokio::select! {
                biased;
                changed = fence_changed => Some(changed),
                () = deadline => None,
            };
            match woken_by_fence {
                Some(true) => {
                    if let Some(fence_changes) = self.fence_changes.as_mut() {
                        fence_changes.mark_unchanged();
                    }
                }
                // The record is gone and can change no more.
                Some(false) => {
                    self.fence_changes = None;
                    continue;
                }
                None => woken_at_deadline = true,
            }
            self.verify((self.clock)());
        }
    }

    /// Ask the verifier at `now` and keep its answer: a fresh deadline, or
    /// the lapse.
    fn verify(&mut self, now: SystemTime) -> bool {
        match self.verification.current_at(now) {
            Some(current) => self.refused_from = current.refused_from,
            None => self.lapsed = true,
        }
        !self.lapsed
    }
}

/// Whether `view` lets its caller watch one agent's events, under the name
/// the request used and the member that name resolved to.
fn agent_stream_allowed(view: &AccessView, agent_id: &str, policy_identity: &str) -> bool {
    !view.enforced()
        || (view.allows_agent(ACTION_AGENT_VIEW, agent_id)
            && view.allows_agent(ACTION_AGENT_VIEW, policy_identity))
}

/// Whether `view` lets its caller watch the mob-wide streams.
fn mob_stream_allowed(view: &AccessView) -> bool {
    !view.enforced() || view.allows(ACTION_MOB_OBSERVE)
}

/// Whether `view` lets its caller watch WorkGraph facts.
fn workgraph_stream_allowed(view: &AccessView) -> bool {
    !view.enforced() || view.allows(ACTION_WORKGRAPH_VIEW)
}

pub fn agent_event_sse(interaction_id: &str, seq: u64, event: &AgentEvent) -> Event {
    let event_name = agent_event_name(event);
    let payload = serde_json::to_string(&console_agent_event_payload(event))
        .unwrap_or_else(|_| "{}".to_string());
    Event::default()
        .id(format!("{interaction_id}:{seq}"))
        .event(event_name)
        .data(payload)
}

fn agent_event_name(event: &AgentEvent) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|value| {
            value
                .as_object()
                .and_then(|object| object.get("type"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "agent_event".to_string())
}

fn http_error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(json!({
            "error": message
        })),
    )
}

fn map_runtime_error(error: MobRuntimeError) -> (StatusCode, Json<Value>) {
    match error {
        MobRuntimeError::InvalidInput(message) => http_error(StatusCode::BAD_REQUEST, message),
        MobRuntimeError::Mob(
            MobError::MemberNotFound(_)
            | MobError::SessionError(SessionError::NotFound { .. })
            | MobError::CommsError(SendError::PeerNotFound(_)),
        ) => http_error(StatusCode::NOT_FOUND, "member_not_found"),
        MobRuntimeError::Mob(MobError::SessionError(SessionError::Unsupported(_))) => {
            http_error(StatusCode::UNPROCESSABLE_ENTITY, "unsupported")
        }
        // An ambiguous delivery must not be rendered as a plain failure. The
        // envelope may already be on the receiver's queue, so "retry" - the
        // action a bare `internal_server_error` invites - is the one unsafe
        // move. HTTP has no status meaning "outcome unknown", so the status is
        // deliberately not the signal here: the body names the ambiguity and
        // carries the envelope id as correlation evidence for reconciliation.
        // `retry_safe` is intentionally absent rather than false: 0.8.24 comms
        // still folds connection-refused into this variant, where nothing left
        // the host, so asserting non-retryability would be unsubstantiated.
        MobRuntimeError::Mob(MobError::CommsError(SendError::AmbiguousDelivery {
            envelope_id,
            detail,
        })) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "delivery_outcome_ambiguous",
                "envelope_id": envelope_id.to_string(),
                "detail": detail,
                "required_action": "reconcile",
            })),
        ),
        _ => http_error(StatusCode::INTERNAL_SERVER_ERROR, "internal_server_error"),
    }
}

/// Apply the console's frame visibility/redaction contract to an SSE payload.
/// SSE remains a distinct transport, but it must not become a side door around
/// the policy already enforced by the console timeline.
fn project_sse_payload(
    visibility_policy: &dyn ConsoleVisibilityPolicy,
    identity: &str,
    kind: &str,
    timestamp_ms: u64,
    payload: Value,
) -> Option<Value> {
    let mut projected = NewConsoleFrame {
        id: None,
        dedupe_key: format!("sse:{identity}:{kind}:{timestamp_ms}"),
        timestamp_ms,
        runtime_key: "sse".to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: None,
        kind: kind.to_string(),
        status: ConsoleFrameStatus::Completed,
        payload,
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::Synthetic,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    };
    if let Some(redacted) = visibility_policy.redact_payload(&projected) {
        projected.payload = redacted;
        projected.status = ConsoleFrameStatus::Redacted;
    }
    let frame = ConsoleFrame {
        id: projected.dedupe_key.clone(),
        cursor: ConsoleCursor::from(projected.dedupe_key.as_str()),
        dedupe_key: projected.dedupe_key.clone(),
        timestamp_ms: projected.timestamp_ms,
        runtime_key: projected.runtime_key.clone(),
        identity: projected.identity.clone(),
        conversation_id: projected.conversation_id.clone(),
        session_id: projected.session_id.clone(),
        kind: projected.kind.clone(),
        status: projected.status,
        frame_version: 1,
        updated_at_ms: None,
        payload: projected.payload.clone(),
        source: projected.source.clone(),
        source_event_id: projected.source_event_id.clone(),
        interaction_id: projected.interaction_id.clone(),
        turn_id: projected.turn_id.clone(),
        run_id: projected.run_id.clone(),
        parent_frame_id: projected.parent_frame_id.clone(),
        caused_by_frame_id: projected.caused_by_frame_id.clone(),
    };
    visibility_policy
        .frame_visible(&frame)
        .then_some(projected.payload)
}

fn mob_event_authorization_alias(runtime_id: &meerkat_mob::ids::AgentRuntimeId) -> String {
    crate::member_comms_id::runtime_alias_str(runtime_id.identity.as_str()).into_owned()
}

// ---------------------------------------------------------------------------
// Tier 2: Per-agent persistent SSE  (MK-005)
// ---------------------------------------------------------------------------

pub type AgentEventSubscribeFuture =
    Pin<Box<dyn Future<Output = Result<EventStream, MobRuntimeError>> + Send>>;

pub type AgentEventSubscribeFn = Arc<dyn Fn(String) -> AgentEventSubscribeFuture + Send + Sync>;

/// A per-agent subscription that starts at a typed cursor and names the
/// sequence space of its envelopes (`None` for a live-only source).
pub(crate) struct AgentEventReplay {
    pub(crate) epoch: Option<meerkat_core::comms::SessionEventEpoch>,
    pub(crate) stream: EventStream,
}

pub(crate) type AgentEventReplaySubscribeFuture =
    Pin<Box<dyn Future<Output = Result<AgentEventReplay, MobRuntimeError>> + Send>>;

/// Replay-capable per-agent subscription, used by the unified runtime so the
/// stream resumes from an SSE `Last-Event-ID`.
pub(crate) type AgentEventReplaySubscribeFn = Arc<
    dyn Fn(String, meerkat_core::comms::SessionEventCursor) -> AgentEventReplaySubscribeFuture
        + Send
        + Sync,
>;

/// A live-only subscribe function as a replay-capable one: it ignores the
/// cursor and names no sequence space, so frame ids stay connection-local.
fn live_only_agent_subscribe(subscribe_fn: AgentEventSubscribeFn) -> AgentEventReplaySubscribeFn {
    Arc::new(move |agent_id, _cursor| {
        let subscribe_fn = Arc::clone(&subscribe_fn);
        Box::pin(async move {
            Ok(AgentEventReplay {
                epoch: None,
                stream: subscribe_fn(agent_id).await?,
            })
        })
    })
}

/// The cursor an agent SSE connection resumes from: after the
/// `{epoch}:{seq}` its `Last-Event-ID` names, else the actor's earliest
/// retained event.
fn agent_sse_resume_cursor(headers: &HeaderMap) -> meerkat_core::comms::SessionEventCursor {
    headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().rsplit_once(':'))
        .and_then(|(epoch, seq)| Some((epoch.parse().ok()?, seq.parse().ok()?)))
        .map_or(
            meerkat_core::comms::SessionEventCursor::Earliest,
            |(epoch, seq)| meerkat_core::comms::SessionEventCursor::After { epoch, seq },
        )
}

#[derive(Clone)]
struct AgentSseState {
    subscribe_fn: AgentEventReplaySubscribeFn,
    decisions: Option<RuntimeDecisionState>,
    /// The Console's session fence record the route's token checks read.
    session_fences: ConsoleSessionFences,
    access: Option<AccessController>,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    /// Live mob runtime used to prime the access attribute cache at
    /// connection time (roster plus spawn-registered console metadata), so
    /// label/role/lineage rules resolve without a prior
    /// `/console/experience` call. `None` keeps the route behaviour unchanged.
    prime_runtime: Option<MobRuntime>,
    /// Identity authority whose current runtime aliases the access rules
    /// need to judge an alias as the member it names.
    prime_identity_runtime: Option<Arc<crate::identity_first::IdentityRuntime>>,
}

/// Applies no session fence; use the routers a [`crate::UnifiedRuntime`] builds,
/// or the `_under_session_fences` variant
/// ([`crate::runtime::validate_console_token_under_session_fences`]).
pub fn agent_events_sse_router(
    subscribe_fn: AgentEventSubscribeFn,
    decisions: Option<RuntimeDecisionState>,
) -> Router {
    agent_events_sse_router_with_access(subscribe_fn, decisions, None)
}

/// Applies no session fence; use the routers a [`crate::UnifiedRuntime`] builds,
/// or the `_under_session_fences` variant
/// ([`crate::runtime::validate_console_token_under_session_fences`]).
pub fn agent_events_sse_router_with_access(
    subscribe_fn: AgentEventSubscribeFn,
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
) -> Router {
    agent_events_sse_router_with_access_and_priming(
        live_only_agent_subscribe(subscribe_fn),
        decisions,
        access,
        None,
        None,
        Arc::new(AllowAllConsoleVisibilityPolicy),
        ConsoleSessionFences::detached(),
    )
}

pub(crate) fn agent_events_sse_router_with_access_and_priming(
    subscribe_fn: AgentEventReplaySubscribeFn,
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
    prime_runtime: Option<MobRuntime>,
    prime_identity_runtime: Option<Arc<crate::identity_first::IdentityRuntime>>,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    session_fences: ConsoleSessionFences,
) -> Router {
    Router::new()
        .route("/agents/{agent_id}/events", get(agent_events_sse_handler))
        .with_state(AgentSseState {
            subscribe_fn,
            decisions,
            session_fences,
            access,
            visibility_policy,
            prime_runtime,
            prime_identity_runtime,
        })
}

async fn agent_events_sse_handler(
    State(state): State<AgentSseState>,
    shutdown: Option<axum::Extension<StreamShutdown>>,
    headers: HeaderMap,
    uri: Uri,
    Path(agent_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let SseCaller {
        access_view,
        mut authentication,
    } = sse_access_context(
        state.decisions.as_ref(),
        &state.session_fences,
        state.access.as_ref(),
        &headers,
        &uri,
    )
    .map_err(|()| sse_unauthorized("agent events stream requires a valid auth token"))?;
    let agent_id = agent_id.trim().to_string();
    if agent_id.is_empty() {
        return Err(http_error(
            StatusCode::BAD_REQUEST,
            "agent_id must not be empty",
        ));
    }
    if let Err(message) =
        crate::member_comms_id::validate_public_member_alias("agent_id", &agent_id)
    {
        return Err(http_error(StatusCode::BAD_REQUEST, &message));
    }
    let policy_identity = if let Some(runtime) = state.prime_runtime.as_ref() {
        match crate::http_console::sse_agent_stream_identity_visibility(
            &runtime.handle(),
            state.visibility_policy.as_ref(),
            &agent_id,
        )
        .await
        {
            Some((identity, true)) => identity,
            Some((_, false)) => {
                return Err(http_error(StatusCode::NOT_FOUND, "member_not_found"));
            }
            // A runtime alias that names no live member is refused: it has
            // no member for the visibility policy to judge.
            None if crate::member_comms_id::is_reserved_generated_alias(&agent_id) => {
                return Err(http_error(StatusCode::NOT_FOUND, "member_not_found"));
            }
            None => agent_id.clone(),
        }
    } else {
        agent_id.clone()
    };
    prime_sse_access_cache(state.prime_runtime.as_ref(), state.access.as_ref()).await;
    if let (Some(identity_runtime), Some(controller)) =
        (state.prime_identity_runtime.as_ref(), state.access.as_ref())
    {
        crate::http_console::prime_current_runtime_aliases(identity_runtime, controller).await;
    }
    // The stream decides under the policy committed before each event, not
    // the one the request read; the gate below reads the same view.
    let mut live_view = access_view.map(LiveAccessView::new);
    // The access rules judge the member the request resolved to as well
    // as the spelling it used: a runtime alias is not a name the rules
    // describe, the member it names is.
    if live_view
        .as_ref()
        .is_some_and(|live| !agent_stream_allowed(live.view(), &agent_id, &policy_identity))
    {
        return Err(sse_access_denied(ACTION_AGENT_VIEW));
    }
    let reprime_runtime = state.prime_runtime.clone();
    let reprime_identity_runtime = state.prime_identity_runtime.clone();
    let reprime_access = state.access.clone();

    let AgentEventReplay {
        epoch,
        stream: event_stream,
    } = (state.subscribe_fn)(agent_id.clone(), agent_sse_resume_cursor(&headers))
        .await
        .map_err(map_runtime_error)?;
    let visibility_policy = state.visibility_policy;
    let mut shutdown = shutdown.map(|axum::Extension(shutdown)| shutdown);

    let stream = stream! {
        let mut seq = 0_u64;
        tokio::pin!(event_stream);
        loop {
            let (policy_changed, envelope) = match wake_or_policy_change(
                live_view.as_mut(),
                authentication.as_mut(),
                next_until_stream_shutdown(shutdown.as_mut(), event_stream.next()),
            )
            .await
            {
                StreamWake::AuthenticationLapsed | StreamWake::Item(None) => break,
                StreamWake::PolicyChanged => (true, None),
                StreamWake::Item(Some(envelope)) => (false, Some(envelope)),
            };
            if let Some(live) = live_view.as_mut() {
                // Read the roster again after each committed change, until
                // none lands during the reads: the gate below then reads the
                // configuration committed before the event is sent.
                let mut changed = policy_changed;
                while live.refresh() || changed {
                    changed = false;
                    prime_sse_access_cache(reprime_runtime.as_ref(), reprime_access.as_ref())
                        .await;
                    if let (Some(identity_runtime), Some(controller)) =
                        (reprime_identity_runtime.as_ref(), reprime_access.as_ref())
                    {
                        crate::http_console::prime_current_runtime_aliases(
                            identity_runtime,
                            controller,
                        )
                        .await;
                    }
                }
                // A revoked grant ends the stream, so the client reconnects
                // under the current policy.
                if !agent_stream_allowed(live.view(), &agent_id, &policy_identity) {
                    break;
                }
            }
            // A lapsed token ends the stream too, so the client reconnects
            // with a current one.
            if !still_authenticated(authentication.as_mut()) {
                break;
            }
            let Some(envelope) = envelope else {
                continue;
            };
            let event_name = agent_event_type(&envelope.payload).to_string();
            let Some(payload) = project_sse_payload(
                visibility_policy.as_ref(),
                &policy_identity,
                &event_name,
                envelope.timestamp_ms,
                console_agent_event_payload(&envelope.payload),
            ) else {
                continue;
            };
            let payload = serde_json::to_string(&payload)
                .unwrap_or_else(|_| "{}".to_string());
            // A replay-capable source ids frames by their place in the
            // member's sequence space, so a reconnect resumes after the last
            // one received. A typed gap marker has no place of its own and
            // carries no id, leaving the client's last event id unchanged.
            let id = match epoch {
                Some(epoch) => (envelope.seq != 0).then(|| format!("{epoch}:{}", envelope.seq)),
                None => Some(format!("{agent_id}:{seq}")),
            };
            let event = Event::default().event(event_name).data(payload);
            yield Ok::<Event, Infallible>(match id {
                Some(id) => event.id(id),
                None => event,
            });
            seq += 1;
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(DEFAULT_KEEP_ALIVE_INTERVAL)
            .text(KEEP_ALIVE_TEXT),
    ))
}

// ---------------------------------------------------------------------------
// Tier 3: Mob-merged SSE  (MK-006)
// ---------------------------------------------------------------------------

/// Meerkat 0.7: mob event-router subscription is fallible (machine command
/// faults surface as `MobError` instead of panicking inside the router).
pub type MobEventSubscribeFuture =
    Pin<Box<dyn Future<Output = Result<MobEventRouterHandle, meerkat_mob::MobError>> + Send>>;

pub type MobEventSubscribeFn = Arc<dyn Fn() -> MobEventSubscribeFuture + Send + Sync>;

#[derive(Clone)]
struct MobSseState {
    subscribe_fn: MobEventSubscribeFn,
    decisions: Option<RuntimeDecisionState>,
    /// See [`AgentSseState::session_fences`].
    session_fences: ConsoleSessionFences,
    access: Option<AccessController>,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    /// See [`AgentSseState::prime_runtime`].
    prime_runtime: Option<MobRuntime>,
}

/// Applies no session fence; use the routers a [`crate::UnifiedRuntime`] builds,
/// or the `_under_session_fences` variant
/// ([`crate::runtime::validate_console_token_under_session_fences`]).
pub fn mob_events_sse_router(
    subscribe_fn: MobEventSubscribeFn,
    decisions: Option<RuntimeDecisionState>,
) -> Router {
    mob_events_sse_router_with_access(subscribe_fn, decisions, None)
}

/// Applies no session fence; use the routers a [`crate::UnifiedRuntime`] builds,
/// or the `_under_session_fences` variant
/// ([`crate::runtime::validate_console_token_under_session_fences`]).
pub fn mob_events_sse_router_with_access(
    subscribe_fn: MobEventSubscribeFn,
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
) -> Router {
    mob_events_sse_router_with_access_and_priming(
        subscribe_fn,
        decisions,
        access,
        None,
        Arc::new(AllowAllConsoleVisibilityPolicy),
        ConsoleSessionFences::detached(),
    )
}

pub(crate) fn mob_events_sse_router_with_access_and_priming(
    subscribe_fn: MobEventSubscribeFn,
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
    prime_runtime: Option<MobRuntime>,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    session_fences: ConsoleSessionFences,
) -> Router {
    Router::new()
        .route("/mob/events", get(mob_events_sse_handler))
        .with_state(MobSseState {
            subscribe_fn,
            decisions,
            session_fences,
            access,
            visibility_policy,
            prime_runtime,
        })
}

/// Refresh the access attribute cache from the live roster and the spawn
/// registry before an SSE stream applies any per-agent filter, so
/// label/role/lineage rules resolve without depending on a prior
/// `/console/experience` call.
async fn prime_sse_access_cache(
    prime_runtime: Option<&MobRuntime>,
    access: Option<&AccessController>,
) {
    if let (Some(runtime), Some(controller)) = (
        prime_runtime,
        access.filter(|controller| controller.enabled()),
    ) {
        crate::http_console::prime_access_cache_from_runtime(runtime, controller).await;
    }
}

fn sse_access_allows_agent_attributes(
    view: Option<&AccessView>,
    attributes: &AgentResourceAttributes,
) -> bool {
    view.is_none_or(|view| {
        view.decide_agent_with_attributes(ACTION_AGENT_VIEW, attributes)
            .is_allow()
    })
}

/// Apply the shared authorization and console-visibility projection for one
/// structural event envelope. Callers must prime the access cache first; an
/// enforced view then fails closed for identities whose attributes remain
/// unknown. Attributed historical events also fail closed when no live member
/// projection exists to evaluate the configured console policy.
pub(crate) async fn project_structural_envelope_for_console(
    handle: &MobHandle,
    runtime: Option<&MobRuntime>,
    visibility_policy: &dyn ConsoleVisibilityPolicy,
    access_view: Option<&AccessView>,
    projection: ProjectedMobEvent,
    require_exact_authority: bool,
) -> Option<crate::MobStructuralEventEnvelope> {
    project_structural_member(
        handle,
        runtime,
        visibility_policy,
        projection,
        require_exact_authority,
    )
    .await
    .decide(visibility_policy, access_view)
}

/// One structural event with the member it is attributed to resolved: the
/// part of its projection that awaits, done before the access decision.
pub(crate) struct ProjectedStructuralEvent {
    envelope: crate::MobStructuralEventEnvelope,
    member: StructuralEventMember,
}

enum StructuralEventMember {
    /// A mob-level event, attributed to no member.
    Mob,
    /// The exact live member that emitted the event.
    Exact(crate::http_console::SseMemberAuthorizationProjection),
    /// No exact authority, only the member's console identity and
    /// visibility by alias, which an enforced view never accepts.
    ByAlias { identity: String, visible: bool },
    /// The member can no longer be resolved.
    Unresolved,
}

pub(crate) async fn project_structural_member(
    handle: &MobHandle,
    runtime: Option<&MobRuntime>,
    visibility_policy: &dyn ConsoleVisibilityPolicy,
    projection: ProjectedMobEvent,
    require_exact_authority: bool,
) -> ProjectedStructuralEvent {
    let envelope = projection.envelope;
    let member = if let Some(identity) = envelope.agent_identity.as_deref() {
        let exact = if let Some(authority) = projection.member_authority.as_ref() {
            crate::http_console::sse_member_authorization_projection(
                handle,
                runtime,
                visibility_policy,
                identity,
                &authority.runtime_id,
                authority.fence_token,
            )
            .await
        } else {
            None
        };
        if let Some(exact) = exact {
            StructuralEventMember::Exact(exact)
        } else if require_exact_authority {
            // Protected projections require a generation/fence-bound live
            // roster snapshot.
            StructuralEventMember::Unresolved
        } else {
            // The public low-level AllowAll router retains its legacy
            // behavior for structural variants that do not carry a fence
            // token on the upstream event.
            match crate::http_console::sse_member_identity_visibility(
                handle,
                visibility_policy,
                identity,
            )
            .await
            {
                Some((identity, visible)) => StructuralEventMember::ByAlias { identity, visible },
                None => StructuralEventMember::Unresolved,
            }
        }
    } else {
        StructuralEventMember::Mob
    };
    ProjectedStructuralEvent { envelope, member }
}

impl ProjectedStructuralEvent {
    /// Decide the event under `access_view` and project its payload. Nothing
    /// here awaits, so a stream decides under the view it settled after its
    /// last await.
    pub(crate) fn decide(
        self,
        visibility_policy: &dyn ConsoleVisibilityPolicy,
        access_view: Option<&AccessView>,
    ) -> Option<crate::MobStructuralEventEnvelope> {
        let access_view = access_view.filter(|view| view.enforced());
        let mut envelope = self.envelope;
        let policy_identity = match self.member {
            StructuralEventMember::Mob => {
                crate::console_contracts::SYSTEM_EVENT_IDENTITY.to_string()
            }
            StructuralEventMember::Exact(exact) => {
                if !sse_access_allows_agent_attributes(access_view, &exact.attributes)
                    || !exact.visible
                {
                    return None;
                }
                exact.identity
            }
            StructuralEventMember::ByAlias { identity, visible } => {
                if access_view.is_some() || !visible {
                    return None;
                }
                identity
            }
            StructuralEventMember::Unresolved => return None,
        };
        envelope.data = project_sse_payload(
            visibility_policy,
            &policy_identity,
            &envelope.kind,
            envelope.timestamp_ms,
            envelope.data,
        )?;
        Some(envelope)
    }
}

/// Read the live roster into the shared attribute cache for the structural
/// stream: through the runtime when there is one, else through the handle.
async fn prime_structural_access_cache(
    runtime: Option<&MobRuntime>,
    handle: &MobHandle,
    access: Option<&AccessController>,
) {
    if runtime.is_some() {
        prime_sse_access_cache(runtime, access).await;
    } else if let Some(controller) = access.filter(|controller| controller.enabled()) {
        crate::http_console::prime_access_cache_from_handle(handle, controller).await;
    }
}

async fn mob_events_sse_handler(
    State(state): State<MobSseState>,
    shutdown: Option<axum::Extension<StreamShutdown>>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let SseCaller {
        access_view,
        mut authentication,
    } = sse_access_context(
        state.decisions.as_ref(),
        &state.session_fences,
        state.access.as_ref(),
        &headers,
        &uri,
    )
    .map_err(|()| sse_unauthorized("mob events stream requires a valid auth token"))?;
    prime_sse_access_cache(state.prime_runtime.as_ref(), state.access.as_ref()).await;
    // The stream decides under the policy committed before each event, not
    // the one the request read; the gate below reads the same view.
    let mut live_view = access_view.map(LiveAccessView::new);
    // `mob.observe` gates access to the merged stream surface. The events
    // flowing through it carry the same rich per-agent payload as
    // `/agents/{id}/events`, so each one is still filtered by `agent.view`
    // on its source — otherwise a `mob.observe` grant would silently defeat
    // a per-agent view denial. "Observe everything" is expressed by also
    // granting `agent.view` on `*`.
    if live_view
        .as_ref()
        .is_some_and(|live| !mob_stream_allowed(live.view()))
    {
        return Err(sse_access_denied(ACTION_MOB_OBSERVE));
    }
    // Captured so the long-lived stream can re-prime the shared attribute cache
    // for members spawned AFTER the one-time subscribe prime (the view reads
    // attributes through the controller's shared cache).
    let reprime_runtime = state.prime_runtime.clone();
    let reprime_access = state.access.clone();
    let visibility_runtime = state.prime_runtime.clone();
    let visibility_policy = state.visibility_policy;
    let mut router_handle = (state.subscribe_fn)().await.map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("mob event subscription failed: {err}")})),
        )
    })?;

    let mut shutdown = shutdown.map(|axum::Extension(shutdown)| shutdown);
    let stream = stream! {
        let mut seq = 0_u64;
        // Roster role/labels are immutable for one concrete member generation.
        // Re-prime on the first event and whenever the same public alias moves
        // to a new generated runtime source, so a respawn cannot inherit stale
        // authorization attributes from the previous embodiment.
        let mut authorized_source_by_alias = std::collections::HashMap::<String, String>::new();
        loop {
            let (policy_changed, attributed) = match wake_or_policy_change(
                live_view.as_mut(),
                authentication.as_mut(),
                next_until_stream_shutdown(shutdown.as_mut(), router_handle.event_rx.recv()),
            )
            .await
            {
                StreamWake::AuthenticationLapsed | StreamWake::Item(None) => break,
                StreamWake::PolicyChanged => (true, None),
                StreamWake::Item(Some(attributed)) => (false, Some(attributed)),
            };
            if let Some(live) = live_view.as_mut() {
                let mut changed = policy_changed;
                while live.refresh() || changed {
                    changed = false;
                    prime_sse_access_cache(reprime_runtime.as_ref(), reprime_access.as_ref())
                        .await;
                }
                // A revoked `mob.observe` ends the stream, so the client
                // reconnects under the current policy.
                if !mob_stream_allowed(live.view()) {
                    break;
                }
            }
            let Some(attributed) = attributed else {
                continue;
            };
            let enforced = live_view
                .as_ref()
                .and_then(LiveAccessView::enforced)
                .is_some();
            // Decode the comms-safe roster member id back to the public
            // alias space: SDK `EventStream` consumers filter by alias, and
            // fail-closed per-agent ABAC view rules are written against
            // aliases — an encoded id would silently drop both.
            let source = crate::member_comms_id::runtime_event_alias(&attributed.source);
            // ABAC speaks the member/durable alias, not meerkat's upstream
            // `{member}:{generation}` runtime id. The latter is still kept in
            // the public event payload for stream ordering/debugging, but
            // policy evaluation must not append a second generation to an
            // identity-first runtime alias.
            let authorization_alias = mob_event_authorization_alias(&attributed.source);
            let concrete_source = attributed.source.to_string();
            let source_changed = authorized_source_by_alias
                .insert(authorization_alias.clone(), concrete_source.clone())
                .as_deref()
                != Some(concrete_source.as_str());
            if enforced && source_changed {
                prime_sse_access_cache(reprime_runtime.as_ref(), reprime_access.as_ref()).await;
            }
            let exact = if let Some(runtime) = visibility_runtime.as_ref() {
                let Some(exact) = crate::http_console::sse_member_authorization_projection(
                    &runtime.handle(),
                    Some(runtime),
                    visibility_policy.as_ref(),
                    &authorization_alias,
                    &attributed.source,
                    attributed.source_fence_token,
                )
                .await
                else {
                    // The alias may now name a newer incarnation. Never use
                    // that member's attributes or visibility as authority for
                    // this event's exact runtime/fence source.
                    continue;
                };
                Some(exact)
            } else {
                None
            };
            // Settled after the last await: the event is decided under the
            // configuration committed before it is sent.
            if let Some(live) = live_view.as_mut() {
                while live.refresh() {
                    prime_sse_access_cache(reprime_runtime.as_ref(), reprime_access.as_ref())
                        .await;
                }
                if !mob_stream_allowed(live.view()) {
                    break;
                }
            }
            if !still_authenticated(authentication.as_mut()) {
                break;
            }
            let stream_view = live_view.as_ref().and_then(LiveAccessView::enforced);
            let policy_identity = if let Some(exact) = exact {
                if !sse_access_allows_agent_attributes(stream_view, &exact.attributes)
                    || !exact.visible
                {
                    continue;
                }
                exact.identity
            } else {
                // The low-level AllowAll router predates runtime-aware
                // projection and has no roster handle. Preserve that default
                // compatibility, but any enforced access view without exact
                // roster authority must fail closed.
                if stream_view.is_some() {
                    continue;
                }
                authorization_alias.clone()
            };
            let event_name = agent_event_type(&attributed.envelope.payload).to_string();
            let Some(payload) = project_sse_payload(
                visibility_policy.as_ref(),
                &policy_identity,
                &event_name,
                attributed.envelope.timestamp_ms,
                console_agent_event_payload(&attributed.envelope.payload),
            ) else {
                continue;
            };
            let data = json!({
                "member_id": &source,
                "source": &source,
                "payload": payload,
            });
            yield Ok::<Event, Infallible>(
                Event::default()
                    .id(format!("mob:{seq}"))
                    .event(event_name)
                    .data(data.to_string()),
            );
            seq += 1;
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(DEFAULT_KEEP_ALIVE_INTERVAL)
            .text(KEEP_ALIVE_TEXT),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{AccessControlConfig, AccessEffect, AccessRule, AgentResourceAttributes};
    use std::collections::BTreeMap;

    struct RedactAndFilterPolicy;

    impl ConsoleVisibilityPolicy for RedactAndFilterPolicy {
        fn frame_visible(&self, frame: &ConsoleFrame) -> bool {
            frame.kind != "hidden"
        }

        fn redact_payload(&self, _frame: &NewConsoleFrame) -> Option<Value> {
            Some(json!({"redacted": true}))
        }
    }

    /// Agent SSE frames carry `{epoch}:{seq}` ids; a reconnect with
    /// `Last-Event-ID` resumes after that position instead of replaying the
    /// retained window again, a gap marker carries no id, and a connection
    /// without `Last-Event-ID` replays from the earliest retained event.
    #[tokio::test]
    async fn agent_sse_ids_are_sequence_positions_and_a_reconnect_resumes_after_them()
    -> Result<(), String> {
        use axum::body::Body;
        use axum::http::Request;
        use meerkat_core::comms::{SessionEventCursor, SessionEventEpoch};
        use tower::ServiceExt;

        let epoch = SessionEventEpoch::new();
        let cursors: Arc<std::sync::Mutex<Vec<SessionEventCursor>>> = Arc::default();
        let recorded = Arc::clone(&cursors);
        let subscribe: AgentEventReplaySubscribeFn = Arc::new(move |_agent_id, cursor| {
            recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(cursor);
            Box::pin(async move {
                let text = |seq: u64| {
                    meerkat_core::event::EventEnvelope::new(
                        "worker-1",
                        seq,
                        None,
                        AgentEvent::TextDelta {
                            delta: format!("delta {seq}"),
                            assistant_message_id: None,
                        },
                    )
                };
                let gap = meerkat_core::event::EventEnvelope::new(
                    "worker-1",
                    0,
                    None,
                    AgentEvent::StreamTruncated {
                        reason: meerkat_core::event::StreamTruncationReason::StreamLagged {
                            dropped: 2,
                        },
                    },
                );
                Ok(AgentEventReplay {
                    epoch: Some(epoch),
                    stream: Box::pin(futures::stream::iter(vec![gap, text(3), text(4)])),
                })
            })
        });
        let app = agent_events_sse_router_with_access_and_priming(
            subscribe,
            None,
            None,
            None,
            None,
            Arc::new(AllowAllConsoleVisibilityPolicy),
            ConsoleSessionFences::detached(),
        );
        let read_ids = |response: axum::response::Response| async move {
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .map_err(|error| error.to_string())?;
            let text = String::from_utf8(body.to_vec()).map_err(|error| error.to_string())?;
            let frames: Vec<Option<String>> = text
                .split("\n\n")
                .filter(|frame| frame.contains("data:"))
                .map(|frame| {
                    frame
                        .lines()
                        .find_map(|line| line.strip_prefix("id:"))
                        .map(|id| id.trim().to_string())
                })
                .collect();
            Ok::<_, String>(frames)
        };

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/agents/worker-1/events")
                    .body(Body::empty())
                    .map_err(|error| error.to_string())?,
            )
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(
            read_ids(first).await?,
            vec![None, Some(format!("{epoch}:3")), Some(format!("{epoch}:4")),],
            "the gap marker carries no id; events carry their sequence position"
        );

        let reconnect = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/agents/worker-1/events")
                    .header("last-event-id", format!("{epoch}:4"))
                    .body(Body::empty())
                    .map_err(|error| error.to_string())?,
            )
            .await
            .map_err(|error| error.to_string())?;
        read_ids(reconnect).await?;
        let malformed = app
            .oneshot(
                Request::builder()
                    .uri("/agents/worker-1/events")
                    .header("last-event-id", "worker-1:7")
                    .body(Body::empty())
                    .map_err(|error| error.to_string())?,
            )
            .await
            .map_err(|error| error.to_string())?;
        read_ids(malformed).await?;
        assert_eq!(
            *cursors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![
                SessionEventCursor::Earliest,
                SessionEventCursor::After { epoch, seq: 4 },
                SessionEventCursor::Earliest,
            ],
            "a reconnect resumes after its Last-Event-ID; anything else replays from the earliest"
        );
        Ok(())
    }

    #[test]
    fn sse_projection_applies_redaction_and_frame_filtering() {
        let policy = RedactAndFilterPolicy;
        assert_eq!(
            project_sse_payload(&policy, "agent", "visible", 1, json!({"secret": true})),
            Some(json!({"redacted": true}))
        );
        assert_eq!(
            project_sse_payload(&policy, "agent", "hidden", 1, json!({"secret": true})),
            None
        );
    }

    #[test]
    fn enforced_sse_access_uses_exact_attributes_instead_of_alias_cache()
    -> Result<(), crate::access::AccessConfigError> {
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                AccessRule {
                    id: "view-all".to_string(),
                    actions: vec![ACTION_AGENT_VIEW.to_string()],
                    agents: vec!["*".to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "deny-secret".to_string(),
                    effect: AccessEffect::Deny,
                    actions: vec![ACTION_AGENT_VIEW.to_string()],
                    match_labels: BTreeMap::from([("org".to_string(), "secret".to_string())]),
                    ..AccessRule::default()
                },
            ],
            ..AccessControlConfig::default()
        })?;
        let view = controller.view_for_subject(None);
        let secret = AgentResourceAttributes {
            identity: "historical-secret".to_string(),
            agent_id: Some("historical-secret".to_string()),
            role: Some("lead".to_string()),
            labels: BTreeMap::from([("org".to_string(), "secret".to_string())]),
        };
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "historical-secret".to_string(),
            agent_id: Some("historical-secret".to_string()),
            role: Some("lead".to_string()),
            labels: BTreeMap::from([("org".to_string(), "public".to_string())]),
        });
        assert!(view.can_view_agent("historical-secret"));
        assert!(!sse_access_allows_agent_attributes(Some(&view), &secret));
        Ok(())
    }

    #[test]
    fn mob_event_authorization_uses_decoded_alias_without_upstream_generation() {
        let identity = crate::member_comms_id::mob_member_id("rt:identity:secret:0");
        let runtime_id =
            meerkat_mob::ids::AgentRuntimeId::new(identity, meerkat_mob::ids::Generation::new(7));

        assert_eq!(
            mob_event_authorization_alias(&runtime_id),
            "rt:identity:secret:0"
        );
        assert_eq!(
            crate::member_comms_id::runtime_event_alias(&runtime_id),
            "rt:identity:secret:0:7"
        );
    }

    fn forced_workgraph_serialization_failure(
        _event: &crate::types::UnifiedEvent,
    ) -> Result<String, serde_json::Error> {
        serde_json::from_str::<String>("{")
    }

    fn forced_lag_resync_serialization_failure(
        event: &crate::types::UnifiedEvent,
    ) -> Result<String, serde_json::Error> {
        if let crate::types::UnifiedEvent::Module(module) = event
            && module.event_type == crate::workgraph_events::WORKGRAPH_RESYNC_REQUIRED_EVENT_TYPE
            && module.payload["reason"] == "lagged"
        {
            return serde_json::from_str::<String>("{");
        }
        serialize_workgraph_event(event)
    }

    #[tokio::test]
    async fn mandatory_workgraph_serialization_failure_closes_without_a_fallback_body() {
        let hub = WorkGraphFactHub::new();
        let receiver = hub.subscribe();
        let stream = workgraph_facts_event_stream(
            receiver,
            forced_workgraph_serialization_failure,
            None,
            None,
            None,
        );
        futures::pin_mut!(stream);

        assert!(
            stream.next().await.is_none(),
            "serialization failure must close before emitting any SSE event",
        );
    }

    #[tokio::test]
    async fn workgraph_lag_resync_serialization_failure_closes_before_the_retained_fact() {
        let (tx, receiver) = broadcast::channel(1);
        let fact_event = |seq| {
            crate::types::UnifiedEvent::Module(crate::types::ModuleEvent {
                module: crate::workgraph_events::WORKGRAPH_EVENT_MODULE.to_string(),
                event_type: crate::workgraph_events::WORKGRAPH_FACT_EVENT_TYPE.to_string(),
                payload: json!({"seq": seq}),
            })
        };
        let first = fact_event(1);
        let retained = fact_event(2);
        assert!(tx.send(first).is_ok(), "first synthetic fact is queued");
        assert!(
            tx.send(retained).is_ok(),
            "retained synthetic fact is queued",
        );

        let stream = workgraph_facts_event_stream(
            receiver,
            forced_lag_resync_serialization_failure,
            None,
            None,
            None,
        );
        futures::pin_mut!(stream);
        assert!(
            stream.next().await.is_some(),
            "initial_sync is serializable"
        );
        assert!(
            stream.next().await.is_none(),
            "failed mandatory lag resync must close before the retained fact",
        );
    }
}

// ---------------------------------------------------------------------------
// Structural mob events: per-client meerkat ledger subscription
// ---------------------------------------------------------------------------

/// Query parameters for `/meta/mob_events/stream`. Mirrors
/// [`EventQuery`] for the field filters; cursor pagination is `after_seq`.
#[derive(Debug, Default, Deserialize)]
pub struct MobStructuralStreamQuery {
    #[serde(default)]
    pub after_seq: Option<u64>,
    #[serde(default)]
    pub mob_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub step_id: Option<String>,
    #[serde(default)]
    pub identity: Option<String>,
    #[serde(default)]
    pub member_id: Option<String>,
    /// Comma-separated list of event-kind labels to keep
    /// (e.g. `flow_started,step_completed`). Empty / absent = all.
    #[serde(default)]
    pub event_types: Option<String>,
    #[serde(default)]
    pub since_ms: Option<u64>,
    #[serde(default)]
    pub until_ms: Option<u64>,
}

impl MobStructuralStreamQuery {
    fn into_event_query(self) -> EventQuery {
        EventQuery {
            since_ms: self.since_ms,
            until_ms: self.until_ms,
            member_id: self.member_id,
            identity: self.identity,
            mob_id: self.mob_id,
            run_id: self.run_id,
            step_id: self.step_id,
            event_types: self
                .event_types
                .map(|raw| {
                    raw.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            limit: None,
            after_seq: self.after_seq,
        }
    }
}

#[derive(Clone)]
struct WorkGraphFactsSseState {
    hub: WorkGraphFactHub,
    decisions: Option<RuntimeDecisionState>,
    /// See [`AgentSseState::session_fences`].
    session_fences: ConsoleSessionFences,
    access: Option<AccessController>,
}

/// Route the runtime-owned WorkGraph wake hub over SSE.
///
/// This transport intentionally has no SSE `id`, `Last-Event-ID`, catch-up,
/// or replay contract. Every connection begins with
/// `workgraph.resync_required` and `reason=initial_sync`; clients establish
/// state through the durable WorkGraph pull methods, then treat subsequent
/// facts as wake accelerants only.
///
/// Applies no session fence; use the routers a [`crate::UnifiedRuntime`] builds,
/// or the `_under_session_fences` variant
/// ([`crate::runtime::validate_console_token_under_session_fences`]).
pub fn workgraph_facts_sse_router(
    hub: WorkGraphFactHub,
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
) -> Router {
    workgraph_facts_sse_router_under_session_fences(
        hub,
        decisions,
        access,
        ConsoleSessionFences::detached(),
    )
}

/// [`workgraph_facts_sse_router`] whose token checks read the Console's
/// session fence record.
pub(crate) fn workgraph_facts_sse_router_under_session_fences(
    hub: WorkGraphFactHub,
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
    session_fences: ConsoleSessionFences,
) -> Router {
    Router::new()
        .route(
            WORKGRAPH_FACTS_STREAM_PATH,
            get(workgraph_facts_sse_handler),
        )
        .with_state(WorkGraphFactsSseState {
            hub,
            decisions,
            session_fences,
            access,
        })
}

type WorkGraphEventSerializer =
    fn(&crate::types::UnifiedEvent) -> Result<String, serde_json::Error>;

fn serialize_workgraph_event(
    event: &crate::types::UnifiedEvent,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(event)
}

fn workgraph_module_sse_event(
    event: &crate::types::UnifiedEvent,
    serializer: WorkGraphEventSerializer,
) -> Option<Event> {
    let crate::types::UnifiedEvent::Module(module) = event else {
        tracing::error!("workgraph fact hub yielded a non-module event; dropping wake");
        return None;
    };
    match serializer(event) {
        Ok(data) => Some(Event::default().event(module.event_type.clone()).data(data)),
        Err(error) => {
            tracing::error!(
                %error,
                event_type = %module.event_type,
                "workgraph module event serialization failed; dropping wake",
            );
            None
        }
    }
}

fn workgraph_facts_event_stream(
    mut receiver: broadcast::Receiver<crate::types::UnifiedEvent>,
    serializer: WorkGraphEventSerializer,
    mut live_view: Option<LiveAccessView>,
    mut authentication: Option<StreamAuthentication>,
    mut shutdown: Option<StreamShutdown>,
) -> impl futures::Stream<Item = Result<Event, Infallible>> {
    stream! {
        // Subscription is a state-machine transition, not a claim that this
        // lossy transport has synchronized anything. Failure to serialize
        // this mandatory transition closes the stream instead of inventing
        // an untyped fallback body.
        let initial_sync = workgraph_resync_required_event("initial_sync", None);
        let Some(initial_sync) = workgraph_module_sse_event(&initial_sync, serializer) else {
            tracing::error!(
                "mandatory workgraph initial_sync serialization failed; closing SSE stream",
            );
            return;
        };
        yield Ok::<Event, Infallible>(initial_sync);
        loop {
            let (policy_changed, received) = match wake_or_policy_change(
                live_view.as_mut(),
                authentication.as_mut(),
                until_stream_shutdown(shutdown.as_mut(), receiver.recv()),
            )
            .await
            {
                // The gateway's HTTP drain ends the stream.
                StreamWake::AuthenticationLapsed | StreamWake::Item(None) => return,
                StreamWake::PolicyChanged => (true, None),
                StreamWake::Item(Some(received)) => (false, Some(received)),
            };
            // A revoked `workgraph.view` ends the stream, so the client
            // reconnects under the current policy; so does a lapsed token.
            if let Some(live) = live_view.as_mut()
                && (live.refresh() || policy_changed)
                && !workgraph_stream_allowed(live.view())
            {
                return;
            }
            if !still_authenticated(authentication.as_mut()) {
                return;
            }
            let Some(received) = received else {
                continue;
            };
            match received {
                Ok(event) => {
                    if let Some(event) = workgraph_module_sse_event(&event, serializer) {
                        yield Ok(event);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    let resync = workgraph_resync_required_event("lagged", Some(skipped));
                    let Some(event) = workgraph_module_sse_event(&resync, serializer) else {
                        tracing::error!(
                            "mandatory workgraph lag resync serialization failed; closing SSE stream",
                        );
                        return;
                    };
                    yield Ok(event);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

async fn workgraph_facts_sse_handler(
    State(state): State<WorkGraphFactsSseState>,
    shutdown: Option<axum::Extension<StreamShutdown>>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<Value>)>
{
    let SseCaller {
        access_view,
        authentication,
    } = sse_access_context(
        state.decisions.as_ref(),
        &state.session_fences,
        state.access.as_ref(),
        &headers,
        &uri,
    )
    .map_err(|()| sse_unauthorized("workgraph facts stream requires a valid auth token"))?;
    // The stream decides under the policy committed before each fact, not
    // the one the request read; the gate below reads the same view.
    let live_view = access_view.map(LiveAccessView::new);
    if live_view
        .as_ref()
        .is_some_and(|live| !workgraph_stream_allowed(live.view()))
    {
        return Err(sse_access_denied(ACTION_WORKGRAPH_VIEW));
    }

    let receiver = state.hub.subscribe();
    let stream = workgraph_facts_event_stream(
        receiver,
        serialize_workgraph_event,
        live_view,
        authentication,
        shutdown.map(|axum::Extension(shutdown)| shutdown),
    );

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(DEFAULT_KEEP_ALIVE_INTERVAL)
            .text(KEEP_ALIVE_TEXT),
    ))
}

#[derive(Clone)]
struct MobStructuralSseState {
    handle: MobHandle,
    store: MobEventsStore,
    /// See [`AgentSseState::prime_runtime`]. `None` falls back to a
    /// roster-only prime from `handle`.
    prime_runtime: Option<MobRuntime>,
    /// Optional auth context. When `Some`, requests are gated by the
    /// same `require_app_auth` toggle the console RPC route uses; when
    /// `None`, the route is unauthenticated (in-process or trusted
    /// embedding).
    decisions: Option<RuntimeDecisionState>,
    /// See [`AgentSseState::session_fences`].
    session_fences: ConsoleSessionFences,
    access: Option<AccessController>,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    /// Reference/custom-policy surfaces require a generation/fence-bound
    /// roster snapshot. The public low-level AllowAll constructor keeps the
    /// historical projection behavior for variants without exact authority.
    require_exact_authority: bool,
}

/// Per-client SSE subscription to the meerkat structural-event ledger.
///
/// Applies no session fence; use the routers a [`crate::UnifiedRuntime`] builds,
/// or the `_under_session_fences` variant
/// ([`crate::runtime::validate_console_token_under_session_fences`]).
///
/// Each connection opens its own `MobEventsView::subscribe_after` so
/// catch-up and live tail share the same ordered stream and there is
/// no race window between snapshot and live subscription. Stale
/// cursors are rejected with HTTP 410 Gone before any SSE handshake.
/// Filtering matches the `meta/mob_events/query` predicate; the
/// per-client `MobEventsSubscription` is dropped when the client
/// disconnects, which cancels the upstream forwarder automatically.
///
/// When `decisions` is `Some` and `decisions.console.require_app_auth`
/// is on, every request must carry a valid bearer token (Authorization
/// header or `auth_token` query param) — same gate the console RPC
/// route uses. `None` opts out of auth (e.g. trusted local embedding).
pub fn mob_structural_events_sse_router(
    handle: MobHandle,
    store: MobEventsStore,
    decisions: Option<RuntimeDecisionState>,
) -> Router {
    mob_structural_events_sse_router_with_access(handle, store, decisions, None)
}

/// Applies no session fence; use the routers a [`crate::UnifiedRuntime`] builds,
/// or the `_under_session_fences` variant
/// ([`crate::runtime::validate_console_token_under_session_fences`]).
pub fn mob_structural_events_sse_router_with_access(
    handle: MobHandle,
    store: MobEventsStore,
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
) -> Router {
    mob_structural_events_sse_router_configured(
        handle,
        store,
        decisions,
        access,
        None,
        Arc::new(AllowAllConsoleVisibilityPolicy),
        false,
        ConsoleSessionFences::detached(),
    )
}

pub(crate) fn mob_structural_events_sse_router_with_access_and_priming(
    handle: MobHandle,
    store: MobEventsStore,
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
    prime_runtime: Option<MobRuntime>,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    session_fences: ConsoleSessionFences,
) -> Router {
    mob_structural_events_sse_router_configured(
        handle,
        store,
        decisions,
        access,
        prime_runtime,
        visibility_policy,
        true,
        session_fences,
    )
}

#[allow(clippy::too_many_arguments)]
fn mob_structural_events_sse_router_configured(
    handle: MobHandle,
    store: MobEventsStore,
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
    prime_runtime: Option<MobRuntime>,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    require_exact_authority: bool,
    session_fences: ConsoleSessionFences,
) -> Router {
    Router::new()
        .route(
            MOB_EVENTS_STREAM_PATH,
            get(mob_structural_events_sse_handler),
        )
        .with_state(MobStructuralSseState {
            handle,
            store,
            prime_runtime,
            decisions,
            session_fences,
            access,
            visibility_policy,
            require_exact_authority,
        })
}

/// Shared auth gate for every SSE route in meta. When `decisions` is
/// `Some(_)` and `decisions.console.require_app_auth` is on, the
/// request must carry a valid bearer / `auth_token` token; otherwise
/// the route is open. Used by `mob_structural_events_sse_router`,
/// `interaction_stream_router`, and the agent-/mob-event tier 2/3
/// routers.
///
/// On success returns the caller: its [`AccessView`] when an
/// [`AccessController`] is wired (anonymous view on open routes), so the
/// SSE handlers can apply per-agent ABAC checks, and its authentication
/// when a token identified it. `Err(())` means 401. A token is checked
/// against the Console's session fence record `fences` as it is now.
pub(crate) fn sse_access_context(
    decisions: Option<&RuntimeDecisionState>,
    fences: &ConsoleSessionFences,
    access: Option<&AccessController>,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<SseCaller, ()> {
    let Some(decisions) = decisions else {
        return Ok(SseCaller {
            access_view: access.map(|controller| controller.view_for_subject(None)),
            authentication: None,
        });
    };
    let bearer_token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_bearer_token_from_header)
        .map(String::from);
    // Parse the query string with `form_urlencoded` so percent-encoded
    // tokens (e.g. base64 padding `=` re-encoded as `%3D`) decode
    // correctly, and so substring-shadowing values like `xauth_token=`
    // don't masquerade as `auth_token=`.
    let query_token = uri.query().and_then(|q| {
        form_urlencoded::parse(q.as_bytes())
            .find(|(key, _)| key == "auth_token")
            .map(|(_, value)| value.into_owned())
    });
    let token = bearer_token.or(query_token);
    let caller = if decisions.console.require_app_auth {
        let token = token.ok_or(())?;
        Some(
            crate::runtime::resolve_authorized_console_caller_from_token(decisions, fences, &token)
                .ok_or(())?,
        )
    } else {
        // Open route: identify callers that volunteered a valid token so
        // per-user ABAC grants apply; everyone else is anonymous. Without an
        // access controller a token grants nothing, so it identifies nobody.
        token.filter(|_| access.is_some()).and_then(|token| {
            crate::runtime::resolve_authorized_console_caller_from_token(decisions, fences, &token)
        })
    };
    let access_view = access.map(|controller| {
        controller.view_for_subject(caller.as_ref().map(|caller| caller.auth.email.as_str()))
    });
    Ok(SseCaller {
        access_view,
        authentication: caller.map(|caller| StreamAuthentication::new(caller.verification)),
    })
}

/// The caller of an SSE route, as its request authenticated it.
pub(crate) struct SseCaller {
    /// The caller's [`AccessView`] when an [`AccessController`] is wired
    /// (an anonymous view on open routes).
    pub(crate) access_view: Option<AccessView>,
    /// The caller's authentication when a token identified the caller: a
    /// stream ends once the verifier no longer accepts that token.
    pub(crate) authentication: Option<StreamAuthentication>,
}

fn sse_unauthorized(reason: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "unauthorized",
            "reason": reason,
        })),
    )
}

fn sse_access_denied(action: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": "access_denied",
            "action": action,
        })),
    )
}

async fn mob_structural_events_sse_handler(
    State(state): State<MobStructuralSseState>,
    shutdown: Option<axum::Extension<StreamShutdown>>,
    headers: HeaderMap,
    uri: Uri,
    Query(params): Query<MobStructuralStreamQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<Value>)>
{
    let SseCaller {
        access_view,
        mut authentication,
    } = sse_access_context(
        state.decisions.as_ref(),
        &state.session_fences,
        state.access.as_ref(),
        &headers,
        &uri,
    )
    .map_err(|()| sse_unauthorized("mob_events stream requires a valid auth token"))?;
    if state.prime_runtime.is_some() {
        prime_sse_access_cache(state.prime_runtime.as_ref(), state.access.as_ref()).await;
    } else if let Some(controller) = state
        .access
        .as_ref()
        .filter(|controller| controller.enabled())
    {
        crate::http_console::prime_access_cache_from_handle(&state.handle, controller).await;
    }
    // The stream decides under the policy committed before each event, not
    // the one the request read; the gate below reads the same view.
    let mut live_view = access_view.map(LiveAccessView::new);
    // Structural events span the whole mob: require the mob-wide
    // observation grant, mirroring `meta/mob_events/query`. Envelopes
    // attributed to a specific agent are additionally filtered by
    // `agent.view` on that agent (below), so `mob.observe` cannot surface
    // the lifecycle of an agent the caller is denied. Mob-level envelopes
    // with no agent attribution flow under `mob.observe` alone.
    if live_view
        .as_ref()
        .is_some_and(|live| !mob_stream_allowed(live.view()))
    {
        return Err(sse_access_denied(ACTION_MOB_OBSERVE));
    }
    let query = params.into_event_query();
    let events_view = state.handle.events();

    let latest = events_view.latest_cursor().await.map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "events_view_unavailable",
                "detail": err.to_string(),
            })),
        )
    })?;

    if let Some(after_seq) = query.after_seq
        && after_seq > latest
    {
        return Err((
            StatusCode::GONE,
            Json(json!({
                "error": "event_query_stale",
                "after_cursor": after_seq,
                "latest_cursor": latest,
            })),
        ));
    }

    let after_cursor = query.after_seq.unwrap_or(latest);
    let mut subscription = events_view
        .subscribe_after(after_cursor)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "subscribe_failed",
                    "detail": err.to_string(),
                })),
            )
        })?;

    let store = state.store;
    // Captured so the long-lived stream can re-prime the shared attribute cache
    // for members spawned AFTER the one-time subscribe prime.
    let reprime_runtime = state.prime_runtime.clone();
    let reprime_handle = state.handle.clone();
    let reprime_access = state.access.clone();
    let visibility_handle = state.handle.clone();
    let visibility_policy = state.visibility_policy;
    let require_exact_authority = state.require_exact_authority;
    let mut shutdown = shutdown.map(|axum::Extension(shutdown)| shutdown);
    let stream = stream! {
        loop {
            let (policy_changed, event) = match wake_or_policy_change(
                live_view.as_mut(),
                authentication.as_mut(),
                next_until_stream_shutdown(shutdown.as_mut(), subscription.event_rx.recv()),
            )
            .await
            {
                StreamWake::AuthenticationLapsed | StreamWake::Item(None) => break,
                StreamWake::PolicyChanged => (true, None),
                StreamWake::Item(Some(event)) => (false, Some(event)),
            };
            if let Some(live) = live_view.as_mut() {
                let mut changed = policy_changed;
                while live.refresh() || changed {
                    changed = false;
                    prime_structural_access_cache(
                        reprime_runtime.as_ref(),
                        &reprime_handle,
                        reprime_access.as_ref(),
                    )
                    .await;
                }
                // A revoked `mob.observe` ends the stream, so the client
                // reconnects under the current policy.
                if !mob_stream_allowed(live.view()) {
                    break;
                }
            }
            let Some(event) = event else {
                continue;
            };
            let enforced = live_view
                .as_ref()
                .and_then(LiveAccessView::enforced)
                .is_some();
            let projection = store.project_event_with_authority(&event).await;
            if !crate::unified_runtime::mob_events::envelope_matches(&projection.envelope, &query) {
                continue;
            }
            // Agent-attributed structural events are gated by `agent.view`
            // on their agent; mob-level events (no attribution) pass on the
            // `mob.observe` grant alone.
            if projection.envelope.agent_identity.is_some() {
                // Structural envelopes carry the durable alias but not the
                // concrete member generation. Refresh the live projection for
                // every attributed event before deciding; the subsequent
                // visibility lookup fails closed if the alias disappeared.
                if enforced {
                    prime_structural_access_cache(
                        reprime_runtime.as_ref(),
                        &reprime_handle,
                        reprime_access.as_ref(),
                    )
                    .await;
                }
            }
            let projected = project_structural_member(
                &visibility_handle,
                reprime_runtime.as_ref(),
                visibility_policy.as_ref(),
                projection,
                require_exact_authority,
            )
            .await;
            // Settled after the last await: the event is decided under the
            // configuration committed before it is sent.
            if let Some(live) = live_view.as_mut() {
                while live.refresh() {
                    prime_structural_access_cache(
                        reprime_runtime.as_ref(),
                        &reprime_handle,
                        reprime_access.as_ref(),
                    )
                    .await;
                }
                if !mob_stream_allowed(live.view()) {
                    break;
                }
            }
            if !still_authenticated(authentication.as_mut()) {
                break;
            }
            let Some(envelope) = projected.decide(
                visibility_policy.as_ref(),
                live_view.as_ref().and_then(LiveAccessView::enforced),
            ) else {
                continue;
            };
            let payload = serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".to_string());
            yield Ok::<Event, Infallible>(
                Event::default()
                    .id(format!("mob-evt-{}", envelope.cursor))
                    .event(envelope.kind.clone())
                    .data(payload),
            );
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(DEFAULT_KEEP_ALIVE_INTERVAL)
            .text(KEEP_ALIVE_TEXT),
    ))
}
