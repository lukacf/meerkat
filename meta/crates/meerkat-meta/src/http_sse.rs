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

/// The mob machine's own word that the mob of a stream is going away: the
/// admission state of the mob's actor, `None` while admission is open and
/// then, for good, the reason it closed (a destroy, a stand-down, an
/// unload). A stream of a mob other than the launch mob follows it.
type StreamMobGone = tokio::sync::watch::Receiver<Option<meerkat_mob::MobAdmissionCloseReason>>;

/// `next`, or `None` once the actor of the stream's mob closed its
/// admission: the stream ends with its mob. `gone` is `None` for a stream
/// of the launch mob and of a router built over one mob, which end as they
/// always did. `next` must be cancellation-safe: the end drops it.
async fn next_until_mob_gone<T, F: Future<Output = Option<T>>>(
    gone: Option<&mut StreamMobGone>,
    next: F,
) -> Option<T> {
    let Some(gone) = gone else {
        return next.await;
    };
    tokio::select! {
        biased;
        _ = gone.wait_for(Option::is_some) => None,
        item = next => item,
    }
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
/// its principal or its login (a sign-out). It is built from the verifier's
/// retained verification and asks the verifier itself, never reads the
/// token, and does not depend on an access controller.
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
/// the request used and the member that name resolved to: neither may be
/// hidden from the caller ([`agent_stream_hidden`]), and under enforced
/// access both need `agent.view`.
fn agent_stream_allowed(view: &AccessView, agent_id: &str, policy_identity: &str) -> bool {
    !agent_stream_hidden(view, agent_id, policy_identity)
        && (!view.enforced()
            || (view.allows_agent(ACTION_AGENT_VIEW, agent_id)
                && view.allows_agent(ACTION_AGENT_VIEW, policy_identity)))
}

/// Whether the agent a stream names is one its caller may not know of
/// ([`AccessView::hides_agent`]: private to another person), under the name
/// the request used or the member it resolved to. Such a stream is refused
/// as a stream of an unknown member, enforced access or not.
pub(crate) fn agent_stream_hidden(
    view: &AccessView,
    agent_id: &str,
    policy_identity: &str,
) -> bool {
    view.hides_agent(agent_id) || view.hides_agent(policy_identity)
}

/// Whether `view` lets its caller watch the mob-wide streams.
fn mob_stream_allowed(view: &AccessView) -> bool {
    !view.enforced() || view.allows(ACTION_MOB_OBSERVE)
}

/// Whether `view` lets its caller watch WorkGraph facts.
fn workgraph_stream_allowed(view: &AccessView) -> bool {
    !view.enforced() || view.allows(ACTION_WORKGRAPH_VIEW)
}

/// Whether the caller of a stream of the mob `other` (none: the launch
/// mob, which a route serves to every caller it lets in) may still address
/// that mob, by `view` as it is now. The mob a stream's request names is
/// judged with its caller when the stream opens ([`stream_mob_of_request`]);
/// a stream that stays open asks this again after each refresh of its
/// caller's view, beside its own grant, and ends when the answer is no: a
/// rule that listed the mob was withdrawn, the caller is no administrator
/// any more, or access came to be enforced. The client's reconnect is then
/// answered for the mob as for an id no mob has.
fn stream_mob_still_addressed(view: &AccessView, other: Option<&meerkat_mob::MobId>) -> bool {
    other.is_none_or(|mob_id| view.may_address_other_mob(mob_id))
}

/// [`stream_mob_still_addressed`] at a stream's opening, asked on the live
/// view the stream decides under. That view is built from the rules
/// committed when the stream opens, which may be later ones than the
/// request's view held when the mob was judged with the caller
/// ([`stream_mob_of_request`]): the resolve that loads a mob lies between.
/// A caller who may not address the mob by the live view is answered as
/// for an id no mob has, ahead of the stream's own grant, as at that
/// judgement.
fn stream_mob_addressed_at_its_opening(
    live: Option<&LiveAccessView>,
    other: Option<&meerkat_mob::MobId>,
) -> Result<(), (StatusCode, Json<Value>)> {
    match (live, other) {
        (Some(live), Some(mob_id)) => stream_mob_addressed_by(live.view(), mob_id),
        _ => Ok(()),
    }
}

/// Whether the caller whose view is `view` may address the mob `mob_id`, a
/// mob other than the launch mob, for a stream of it: a caller who may not
/// is answered as for an id no mob has.
fn stream_mob_addressed_by(
    view: &AccessView,
    mob_id: &meerkat_mob::MobId,
) -> Result<(), (StatusCode, Json<Value>)> {
    if stream_mob_still_addressed(view, Some(mob_id)) {
        Ok(())
    } else {
        Err(mob_refusal_over_http(
            crate::rpc::mob_addressing::mob_not_served(mob_id.as_str()),
        ))
    }
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

/// The mob an event stream request names in its `mob_id` query parameter
/// (`crate::rpc::mob_addressing`), for a route whose router was built over
/// ONE mob's handles: a router a host builds by hand from the public router
/// functions, which take that mob's own closures and handles. A request
/// without the parameter names no mob and reads that mob's stream.
///
/// Such a route can read no other mob, so it serves the mob it was built
/// over only, to every caller: the id is judged with the route's caller as
/// one who addresses that mob alone
/// ([`crate::rpc::mob_addressing::LaunchMobOnly`]), right after the route
/// knows its caller and before its own rules, and a request that names any
/// other id is refused before a stream opens and before anything is asked
/// about that id, an administrator's included. The refusal is the one
/// answer a door that has callers gives: HTTP 404 with the id the request
/// named and no other mob. The routers a [`crate::UnifiedRuntime`] builds
/// read the mob the request names instead ([`stream_mob_of_request`]).
///
/// A route whose router carries no resolver knows only the mob it was
/// built over (`own_mob`): it serves it by its id and by no id, refuses
/// every other id, and says so once in the log; a route that knows no mob
/// by its id refuses every id.
async fn refuse_stream_of_another_mob(
    uri: &Uri,
    resolver: Option<&crate::unified_runtime::MobResolver>,
    own_mob: Option<&meerkat_mob::MobId>,
    caller: SseCaller,
) -> Result<SseCaller, (StatusCode, Json<Value>)> {
    crate::rpc::mob_addressing::judge_served_mob(
        &mob_named_in_query(uri),
        resolver,
        own_mob,
        crate::rpc::mob_addressing::LaunchMobOnly(caller),
        "event stream",
    )
    .await
    .map(|(_, crate::rpc::mob_addressing::LaunchMobOnly(caller))| caller)
    .map_err(mob_refusal_over_http)
}

/// The mob a stream of a router the runtime built is read from: the mob
/// its request names, resolved.
pub(crate) struct StreamMob {
    /// The resolved mob: everything the stream reads is this mob's.
    resolved: crate::unified_runtime::ResolvedMob,
    /// The mob's id when it is not the launch mob: what the stream records
    /// and reads of an agent is keyed by it. A stream of the launch mob
    /// records and reads as a host of one mob does.
    other: Option<meerkat_mob::MobId>,
    /// The stream's owner lease on a mob other than the launch mob, held
    /// until the stream ends: the mob counts an open stream among its
    /// owners, so it is not closed as an idle mob while one is open. The
    /// launch mob is never unloaded and gets none.
    owner: Option<meerkat_mob::MobHandle>,
}

impl StreamMob {
    /// The mob's runtime wrapper.
    fn mob_runtime(&self) -> &MobRuntime {
        self.resolved.mob_runtime()
    }

    /// The mob's id when it is not the launch mob.
    fn other_mob(&self) -> Option<&meerkat_mob::MobId> {
        self.other.as_ref()
    }

    /// The stream's owner lease, to keep for the stream's life.
    fn take_owner(&mut self) -> Option<meerkat_mob::MobHandle> {
        self.owner.take()
    }
}

/// Resolve the mob a stream request on a router the runtime built names
/// in its `mob_id` query parameter, with the route's caller in hand. A
/// request without the parameter, or with the launch mob's id, is for the
/// launch mob and the caller is handed back as it came. Any other id is
/// judged with the caller first ([`crate::rpc::mob_addressing::judge_named_mob`]):
/// a caller who may not address it is refused before anything is asked
/// about that mob, with the answer of an id no mob has; a caller who may
/// is handed back seeing that mob, the mob is resolved (which loads a mob
/// that is not loaded) and the stream takes an owner lease on it.
///
/// Every refusal is the one answer a door that has callers gives: HTTP 404
/// with the id the request named and no other mob, also for a mob whose
/// admission closed between its resolve and the lease (a mob that is being
/// unloaded or destroyed).
async fn stream_mob_of_request(
    uri: &Uri,
    resolver: &crate::unified_runtime::MobResolver,
    caller: SseCaller,
) -> Result<(StreamMob, SseCaller), (StatusCode, Json<Value>)> {
    use crate::rpc::mob_addressing::{
        AddressedMob, judge_named_mob, launch_mob_of, mob_not_served, mob_refusal_for_a_caller,
    };

    let named = mob_named_in_query(uri);
    let launch_mob = launch_mob_of(resolver).await;
    let (addressed, caller) =
        judge_named_mob(&named, launch_mob.as_ref(), caller).map_err(mob_refusal_over_http)?;
    let refused = |error: &crate::unified_runtime::MobResolveError| {
        mob_refusal_over_http(mob_refusal_for_a_caller(error))
    };
    match addressed {
        AddressedMob::Launch => {
            let resolved = resolver
                .resolve(None)
                .await
                .map_err(|error| refused(&error))?;
            Ok((
                StreamMob {
                    resolved,
                    other: None,
                    owner: None,
                },
                caller,
            ))
        }
        AddressedMob::Other(mob_id) => {
            let resolved = resolver
                .resolve(Some(&mob_id))
                .await
                .map_err(|error| refused(&error))?;
            let owner = resolved
                .mob_handle()
                .acquire_owner()
                .map_err(|_| mob_refusal_over_http(mob_not_served(mob_id.as_str())))?;
            Ok((
                StreamMob {
                    resolved,
                    other: Some(mob_id),
                    owner: Some(owner),
                },
                caller,
            ))
        }
    }
}

/// The answer of a stream route of a router the runtime built when its
/// request carries no mob resolver. The runtime layers its resolver over
/// every router it builds, so no request reaches this.
fn stream_route_without_resolver() -> (StatusCode, Json<Value>) {
    http_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "the event stream route has no mob resolver",
    )
}

/// The mob a request names in its `mob_id` query parameter
/// (`crate::rpc::mob_addressing`); a request without the parameter names no
/// mob.
pub(crate) fn mob_named_in_query(uri: &Uri) -> crate::rpc::mob_addressing::NamedMob {
    let mob_id = uri.query().and_then(|query| {
        form_urlencoded::parse(query.as_bytes())
            .find(|(name, _)| name == "mob_id")
            .map(|(_, value)| value.into_owned())
    });
    crate::rpc::mob_addressing::NamedMob::from_query(mob_id)
}

/// The answer of an HTTP route for a mob id it does not serve to its
/// caller, from the refusal a door that has callers gives: 404 with the id
/// the request named and no other mob.
pub(crate) fn mob_refusal_over_http(
    refusal: crate::rpc::JsonRpcError,
) -> (StatusCode, Json<Value>) {
    let data = refusal.data.unwrap_or(Value::Null);
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": crate::rpc::mob_addressing::FOREIGN_MOB_TARGET_KIND,
            "requested_mob_id": data.get("requested_mob_id").cloned().unwrap_or(Value::Null),
        })),
    )
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
        mob_id: None,
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
        mob_id: projected.mob_id.clone(),
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
    /// For a subscription that attaches after the connection opened (its
    /// member was dormant, or its session unborn, when the request came):
    /// set when it attaches, before its first event, to the sequence space
    /// the attached subscription names (`None` for a live-only source), so
    /// every frame with a place in that space carries a resumable id. `None`
    /// for a subscription attached at once.
    pub(crate) attached_epoch:
        Option<Arc<std::sync::OnceLock<Option<meerkat_core::comms::SessionEventEpoch>>>>,
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
                attached_epoch: None,
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

/// The per-agent subscription of a router the runtime built: the stream of
/// the agent of the given name in the resolved mob.
pub(crate) type AgentEventSubscribeInMobFn = Arc<
    dyn Fn(
            crate::unified_runtime::ResolvedMob,
            String,
            meerkat_core::comms::SessionEventCursor,
        ) -> AgentEventReplaySubscribeFuture
        + Send
        + Sync,
>;

/// What the per-agent stream route reads its streams from.
#[derive(Clone)]
enum AgentStreamSource {
    /// One mob's own closures and handles: a router a host built by hand.
    /// It serves that mob only.
    Own(Box<AgentStreamOwn>),
    /// The mob a request names, resolved by the host's resolver: a router
    /// the runtime built. The attributes are primed from the resolved
    /// mob's runtime and identity runtime.
    Resolved(AgentEventSubscribeInMobFn),
}

/// What a per-agent stream router built over one mob holds of that mob.
#[derive(Clone)]
struct AgentStreamOwn {
    subscribe_fn: AgentEventReplaySubscribeFn,
    /// Live mob runtime used to prime the access attribute cache at
    /// connection time (roster plus spawn-registered console metadata),
    /// so label/role/lineage rules resolve without a prior
    /// `/console/experience` call. `None` keeps the route behaviour
    /// unchanged.
    prime_runtime: Option<MobRuntime>,
    /// Identity authority whose current runtime aliases the access
    /// rules need to judge an alias as the member it names.
    prime_identity_runtime: Option<Arc<crate::identity_first::IdentityRuntime>>,
}

#[derive(Clone)]
struct AgentSseState {
    source: AgentStreamSource,
    decisions: Option<RuntimeDecisionState>,
    /// The Console's session fence record the route's token checks read.
    session_fences: ConsoleSessionFences,
    access: Option<AccessController>,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
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
    agent_events_router(AgentSseState {
        source: AgentStreamSource::Own(Box::new(AgentStreamOwn {
            subscribe_fn,
            prime_runtime,
            prime_identity_runtime,
        })),
        decisions,
        session_fences,
        access,
        visibility_policy,
    })
}

/// The per-agent stream router of a [`crate::UnifiedRuntime`]: it serves
/// the mob a request names with `mob_id`, resolved by the resolver the
/// runtime layers over its routers, and the launch mob for a request that
/// names none. `subscribe_in` opens the stream of an agent of the resolved
/// mob.
pub(crate) fn agent_events_sse_router_of_resolved_mobs(
    subscribe_in: AgentEventSubscribeInMobFn,
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    session_fences: ConsoleSessionFences,
) -> Router {
    agent_events_router(AgentSseState {
        source: AgentStreamSource::Resolved(subscribe_in),
        decisions,
        session_fences,
        access,
        visibility_policy,
    })
}

/// The one mount of the per-agent stream route.
fn agent_events_router(state: AgentSseState) -> Router {
    Router::new()
        .route("/agents/{agent_id}/events", get(agent_events_sse_handler))
        .with_state(state)
}

async fn agent_events_sse_handler(
    State(state): State<AgentSseState>,
    shutdown: Option<axum::Extension<StreamShutdown>>,
    resolver: Option<axum::Extension<crate::unified_runtime::MobResolver>>,
    headers: HeaderMap,
    uri: Uri,
    Path(agent_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let caller = sse_access_context(
        state.decisions.as_ref(),
        &state.session_fences,
        state.access.as_ref(),
        &headers,
        &uri,
    )
    .map_err(|()| sse_unauthorized("agent events stream requires a valid auth token"))?;
    let resolver = resolver.as_ref().map(|axum::Extension(resolver)| resolver);
    // What the stream reads, and the mob it reads it of: the router's own
    // mob, or the mob the request names. `owner` is the stream's lease on a
    // mob other than the launch mob, kept until the stream ends.
    let (subscribe_fn, prime_runtime, prime_identity_runtime, other_mob, owner, caller) =
        match state.source {
            AgentStreamSource::Own(own) => {
                let AgentStreamOwn {
                    subscribe_fn,
                    prime_runtime,
                    prime_identity_runtime,
                } = *own;
                let caller = refuse_stream_of_another_mob(
                    &uri,
                    resolver,
                    prime_runtime.as_ref().map(MobRuntime::mob_id),
                    caller,
                )
                .await?;
                (
                    subscribe_fn,
                    prime_runtime,
                    prime_identity_runtime,
                    None,
                    None,
                    caller,
                )
            }
            AgentStreamSource::Resolved(subscribe_in) => {
                let resolver = resolver.ok_or_else(stream_route_without_resolver)?;
                let (mut mob, caller) = stream_mob_of_request(&uri, resolver, caller).await?;
                let owner = mob.take_owner();
                let other_mob = mob.other_mob().cloned();
                let prime_runtime = Some(mob.mob_runtime().clone());
                let prime_identity_runtime = mob.resolved.identity_runtime_ref();
                let resolved = mob.resolved;
                let subscribe_fn: AgentEventReplaySubscribeFn =
                    Arc::new(move |agent_id, cursor| {
                        subscribe_in(resolved.clone(), agent_id, cursor)
                    });
                (
                    subscribe_fn,
                    prime_runtime,
                    prime_identity_runtime,
                    other_mob,
                    owner,
                    caller,
                )
            }
        };
    let SseCaller {
        audience,
        mut authentication,
        ..
    } = caller;
    // The mob's address by the rules committed now, ahead of the name the
    // request gives: what the route answers about a name in the mob (a
    // member the console does not show, a runtime alias no live member
    // holds) it answers to a caller who may address the mob. The mob was
    // judged with the request's view; the resolve that loads a mob lies
    // between, and a rule may have been withdrawn in it.
    if let Some(mob_id) = other_mob.as_ref() {
        stream_mob_addressed_by(&audience.refreshed(), mob_id)?;
    }
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
    let policy_identity = if let Some(runtime) = prime_runtime.as_ref() {
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
    // The stream's audience controller: the access controller when one is
    // wired, else the stream's own, primed either way.
    let audience_access = audience.controller();
    prime_sse_access_cache(
        prime_runtime.as_ref(),
        Some(&audience_access),
        other_mob.as_ref(),
    )
    .await;
    if let Some(identity_runtime) = prime_identity_runtime.as_ref() {
        crate::http_console::prime_current_runtime_aliases_of(
            identity_runtime,
            &audience_access,
            other_mob.as_ref(),
        )
        .await;
    }
    // The stream decides under the policy committed before each event, not
    // the one the request read; the gate below reads the same view.
    let mut live_view = Some(LiveAccessView::new(audience));
    // The mob's address by the live view, ahead of the stream's own grant.
    stream_mob_addressed_at_its_opening(live_view.as_ref(), other_mob.as_ref())?;
    // The attributes were read just above; the stream reads them again
    // before it decides an item once that read is as old as a kept
    // record's bound.
    let mut prime_age = StreamPrimeAge::primed(std::time::Instant::now());
    // An agent private to another person is answered as a name no member
    // holds, by what this view answers such a name: a refusal where the
    // access rules refuse a name they know nothing of (a deny the name
    // cannot rule out, or no allow that reaches it), a member the console
    // does not hold otherwise. A runtime alias that names no member is
    // answered so above, whatever the rules say.
    if let Some(live) = live_view.as_ref()
        && agent_stream_hidden(live.view(), &agent_id, &policy_identity)
    {
        let absent = crate::http_console::ConsoleAbsentNames::fresh().absent_spelling(&agent_id);
        if !crate::member_comms_id::is_reserved_generated_alias(&agent_id)
            && !agent_stream_allowed(live.view(), &absent, &absent)
        {
            return Err(sse_access_denied(ACTION_AGENT_VIEW));
        }
        return Err(http_error(StatusCode::NOT_FOUND, "member_not_found"));
    }
    // The access rules judge the member the request resolved to as well
    // as the spelling it used: a runtime alias is not a name the rules
    // describe, the member it names is.
    if live_view
        .as_ref()
        .is_some_and(|live| !agent_stream_allowed(live.view(), &agent_id, &policy_identity))
    {
        return Err(sse_access_denied(ACTION_AGENT_VIEW));
    }
    let reprime_runtime = prime_runtime;
    let reprime_identity_runtime = prime_identity_runtime;
    let reprime_access = Some(audience_access);
    let reprime_mob = other_mob;

    let AgentEventReplay {
        epoch,
        attached_epoch,
        stream: event_stream,
    } = subscribe_fn(agent_id.clone(), agent_sse_resume_cursor(&headers))
        .await
        .map_err(map_runtime_error)?;
    let visibility_policy = state.visibility_policy;
    let mut shutdown = shutdown.map(|axum::Extension(shutdown)| shutdown);

    let stream = stream! {
        // The stream's owner lease on its mob, given back when the stream
        // ends; the stream ends when that mob's actor closes its admission.
        let owner = owner;
        let mut mob_gone = owner.as_ref().map(meerkat_mob::MobHandle::admission_closed);
        let mut seq = 0_u64;
        tokio::pin!(event_stream);
        loop {
            let (policy_changed, envelope) = match wake_or_policy_change(
                live_view.as_mut(),
                authentication.as_mut(),
                next_until_mob_gone(
                    mob_gone.as_mut(),
                    next_until_stream_shutdown(shutdown.as_mut(), event_stream.next()),
                ),
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
                let mut changed =
                    policy_changed || prime_age.due(std::time::Instant::now());
                while live.refresh() || changed {
                    changed = false;
                    prime_sse_access_cache(
                        reprime_runtime.as_ref(),
                        reprime_access.as_ref(),
                        reprime_mob.as_ref(),
                    )
                        .await;
                    if let (Some(identity_runtime), Some(controller)) =
                        (reprime_identity_runtime.as_ref(), reprime_access.as_ref())
                    {
                        crate::http_console::prime_current_runtime_aliases_of(
                            identity_runtime,
                            controller,
                            reprime_mob.as_ref(),
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
            // carries no id, leaving the client's last event id unchanged. A
            // subscription that attached after the connection opened names
            // its space once attached, before its first frame.
            let frame_epoch = epoch.or_else(|| {
                attached_epoch
                    .as_ref()
                    .and_then(|attached| attached.get().copied().flatten())
            });
            let id = match frame_epoch {
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

/// What the mob-wide stream route reads its stream from.
#[derive(Clone)]
enum MobStreamSource {
    /// One mob's own closure: a router a host built by hand. It serves
    /// that mob only.
    Own(Box<MobStreamOwn>),
    /// The mob a request names, resolved by the host's resolver: a router
    /// the runtime built. The stream is the resolved mob's merged agent
    /// events, primed from that mob's runtime.
    Resolved,
}

/// What a mob-wide stream router built over one mob holds of that mob.
#[derive(Clone)]
struct MobStreamOwn {
    subscribe_fn: MobEventSubscribeFn,
    /// See [`AgentStreamOwn::prime_runtime`].
    prime_runtime: Option<MobRuntime>,
}

#[derive(Clone)]
struct MobSseState {
    source: MobStreamSource,
    decisions: Option<RuntimeDecisionState>,
    /// See [`AgentSseState::session_fences`].
    session_fences: ConsoleSessionFences,
    access: Option<AccessController>,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
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
    mob_events_router(MobSseState {
        source: MobStreamSource::Own(Box::new(MobStreamOwn {
            subscribe_fn,
            prime_runtime,
        })),
        decisions,
        session_fences,
        access,
        visibility_policy,
    })
}

/// The mob-wide stream router of a [`crate::UnifiedRuntime`]: it serves the
/// merged agent events of the mob a request names with `mob_id`, resolved
/// by the resolver the runtime layers over its routers, and the launch
/// mob's for a request that names none.
pub(crate) fn mob_events_sse_router_of_resolved_mobs(
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    session_fences: ConsoleSessionFences,
) -> Router {
    mob_events_router(MobSseState {
        source: MobStreamSource::Resolved,
        decisions,
        session_fences,
        access,
        visibility_policy,
    })
}

/// The one mount of the mob-wide stream route.
fn mob_events_router(state: MobSseState) -> Router {
    Router::new()
        .route("/mob/events", get(mob_events_sse_handler))
        .with_state(state)
}

/// When a stream last read the roster and the agents' records into the
/// access attributes its decisions read.
///
/// A stream reads them when it opens. An agent's record can change while
/// the stream is open, and nothing wakes a stream for it, so a stream reads
/// them again before it decides its next item once its last read is as old
/// as the bound a kept record has
/// ([`crate::identity_first::runtime::UNSEATED_RECORD_MAX_AGE`]): a stream
/// opened before a change decides on the record as it was for that long at
/// most. A stream with no item to decide reads nothing.
pub(crate) struct StreamPrimeAge {
    primed_at: std::time::Instant,
}

impl StreamPrimeAge {
    /// A stream that read the attributes at `now`.
    pub(crate) fn primed(now: std::time::Instant) -> Self {
        Self { primed_at: now }
    }

    /// Whether the stream's last read is as old as the bound at `now`. It
    /// then counts as read at `now`: the caller reads the attributes again
    /// before it decides.
    pub(crate) fn due(&mut self, now: std::time::Instant) -> bool {
        if now.saturating_duration_since(self.primed_at)
            < crate::identity_first::runtime::UNSEATED_RECORD_MAX_AGE
        {
            return false;
        }
        self.primed_at = now;
        true
    }
}

/// Refresh the access attribute cache from the live roster and the spawn
/// registry before an SSE stream applies any per-agent filter, so
/// label/role/lineage rules resolve without depending on a prior
/// `/console/experience` call. `access` is the stream's audience controller
/// ([`SseCaller::audience`]), primed whether or not access is enforced: the
/// private-owner rule reads it too. `mob` is the mob of the stream's
/// request when it is not the launch mob: the attributes are recorded as
/// that mob's agents'; a stream of the launch mob passes none and records
/// as a host of one mob does.
async fn prime_sse_access_cache(
    prime_runtime: Option<&MobRuntime>,
    access: Option<&AccessController>,
    mob: Option<&meerkat_mob::MobId>,
) {
    if let (Some(runtime), Some(controller)) = (prime_runtime, access) {
        crate::http_console::prime_access_cache_from_runtime_of(runtime, controller, mob).await;
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
    /// The members the event names besides the one it is attributed to: the
    /// endpoints of a mob-level event, and the members of the run it belongs
    /// to (its bound members and every member a step dispatched to).
    endpoints: Vec<String>,
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
    let mut member = if let Some(identity) = envelope.agent_identity.as_deref() {
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
    let mut endpoints = projection.endpoints;
    if let Some(run_id) = envelope
        .run_id
        .as_deref()
        .and_then(|run_id| run_id.parse::<meerkat_mob::RunId>().ok())
        && let Ok(Some(run)) = handle.flow_status(run_id).await
    {
        // The members of the run, from its role targets and the mob's
        // per-target dispatch records. Who a run reached that cannot be read
        // leaves the event unresolved: it is shown to no one.
        match crate::run_members::run_members(runtime, handle, &run).await {
            Ok(members) => endpoints.extend(members),
            Err(_) => member = StructuralEventMember::Unresolved,
        }
    }
    ProjectedStructuralEvent {
        envelope,
        member,
        endpoints,
    }
}

impl ProjectedStructuralEvent {
    /// Whether the event names, besides the member it is attributed to, a
    /// member `view`'s attributes do not know that `asked` does not hold
    /// yet. Each such member is added to `asked`.
    fn names_member_unknown_to(
        &self,
        view: &AccessView,
        asked: &mut std::collections::HashSet<String>,
    ) -> bool {
        let mut unknown = false;
        for endpoint in &self.endpoints {
            if !view.knows_agent(endpoint) && asked.insert(endpoint.clone()) {
                unknown = true;
            }
        }
        unknown
    }

    /// Decide the event under `access_view` and project its payload. Nothing
    /// here awaits, so a stream decides under the view it settled after its
    /// last await. Any view decides the agents private to another person; an
    /// enforcing one also decides its rules.
    pub(crate) fn decide(
        self,
        visibility_policy: &dyn ConsoleVisibilityPolicy,
        access_view: Option<&AccessView>,
    ) -> Option<crate::MobStructuralEventEnvelope> {
        // An event that names a member hidden from the caller (a wiring edge
        // to it, an instruction for it, a run that reached it) is shown to no
        // one but that member's audience.
        if access_view.is_some_and(|view| {
            self.endpoints
                .iter()
                .any(|endpoint| view.hides_agent(endpoint))
        }) {
            return None;
        }
        let enforced_view = access_view.filter(|view| view.enforced());
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
                if enforced_view.is_some()
                    || access_view.is_some_and(|view| view.hides_agent(&identity))
                    || !visible
                {
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
/// `access` is the stream's audience controller, primed whether or not
/// access is enforced. `mob` is the mob of the stream's request when it is
/// not the launch mob ([`prime_sse_access_cache`]); the roster read through
/// a handle alone is of a router built over one mob, which names none.
async fn prime_structural_access_cache(
    runtime: Option<&MobRuntime>,
    handle: &MobHandle,
    access: Option<&AccessController>,
    mob: Option<&meerkat_mob::MobId>,
) {
    if runtime.is_some() {
        prime_sse_access_cache(runtime, access, mob).await;
    } else if let Some(controller) = access {
        crate::http_console::prime_access_cache_from_handle(handle, controller).await;
    }
}

async fn mob_events_sse_handler(
    State(state): State<MobSseState>,
    shutdown: Option<axum::Extension<StreamShutdown>>,
    resolver: Option<axum::Extension<crate::unified_runtime::MobResolver>>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let caller = sse_access_context(
        state.decisions.as_ref(),
        &state.session_fences,
        state.access.as_ref(),
        &headers,
        &uri,
    )
    .map_err(|()| sse_unauthorized("mob events stream requires a valid auth token"))?;
    let resolver = resolver.as_ref().map(|axum::Extension(resolver)| resolver);
    // What the stream reads, and the mob it reads it of: the router's own
    // mob, or the mob the request names. `owner` is the stream's lease on a
    // mob other than the launch mob, kept until the stream ends.
    let (subscribe_fn, prime_runtime, other_mob, owner, caller) = match state.source {
        MobStreamSource::Own(own) => {
            let MobStreamOwn {
                subscribe_fn,
                prime_runtime,
            } = *own;
            let caller = refuse_stream_of_another_mob(
                &uri,
                resolver,
                prime_runtime.as_ref().map(MobRuntime::mob_id),
                caller,
            )
            .await?;
            (subscribe_fn, prime_runtime, None, None, caller)
        }
        MobStreamSource::Resolved => {
            let resolver = resolver.ok_or_else(stream_route_without_resolver)?;
            let (mut mob, caller) = stream_mob_of_request(&uri, resolver, caller).await?;
            let owner = mob.take_owner();
            let other_mob = mob.other_mob().cloned();
            let mob_runtime = mob.mob_runtime().clone();
            let subscribing = mob_runtime.clone();
            let subscribe_fn: MobEventSubscribeFn = Arc::new(move || {
                let mob_runtime = subscribing.clone();
                Box::pin(async move { mob_runtime.handle().subscribe_mob_events().await })
            });
            (subscribe_fn, Some(mob_runtime), other_mob, owner, caller)
        }
    };
    let SseCaller {
        audience,
        mut authentication,
        ..
    } = caller;
    // The stream's audience controller: the access controller when one is
    // wired, else the stream's own, primed either way.
    let audience_access = audience.controller();
    prime_sse_access_cache(
        prime_runtime.as_ref(),
        Some(&audience_access),
        other_mob.as_ref(),
    )
    .await;
    // The stream decides under the policy committed before each event, not
    // the one the request read; the gate below reads the same view.
    let mut live_view = Some(LiveAccessView::new(audience));
    // The mob's address by the live view, ahead of the stream's own grant.
    stream_mob_addressed_at_its_opening(live_view.as_ref(), other_mob.as_ref())?;
    // The attributes were read just above; the stream reads them again
    // before it decides an item once that read is as old as a kept
    // record's bound.
    let mut prime_age = StreamPrimeAge::primed(std::time::Instant::now());
    // `mob.observe` gates access to the merged stream surface. The events
    // flowing through it carry the same rich per-agent payload as
    // `/agents/{id}/events`, so each one is still filtered by `agent.view`
    // on its source — otherwise a `mob.observe` grant would silently defeat
    // a per-agent view denial. "Observe everything" is expressed by also
    // granting `agent.view` on `*`. The events of an agent private to
    // another person are filtered out whether or not access is enforced.
    if live_view
        .as_ref()
        .is_some_and(|live| !mob_stream_allowed(live.view()))
    {
        return Err(sse_access_denied(ACTION_MOB_OBSERVE));
    }
    // Captured so the long-lived stream can re-prime the shared attribute cache
    // for members spawned AFTER the one-time subscribe prime (the view reads
    // attributes through the controller's shared cache).
    let reprime_runtime = prime_runtime.clone();
    let reprime_access = Some(audience_access);
    let reprime_mob = other_mob;
    let visibility_runtime = prime_runtime;
    let visibility_policy = state.visibility_policy;
    let mut router_handle = subscribe_fn().await.map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("mob event subscription failed: {err}")})),
        )
    })?;

    let mut shutdown = shutdown.map(|axum::Extension(shutdown)| shutdown);
    let stream = stream! {
        // The stream's owner lease on its mob, given back when the stream
        // ends; the stream ends when that mob's actor closes its admission.
        let owner = owner;
        let mut mob_gone = owner.as_ref().map(meerkat_mob::MobHandle::admission_closed);
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
                next_until_mob_gone(
                    mob_gone.as_mut(),
                    next_until_stream_shutdown(shutdown.as_mut(), router_handle.event_rx.recv()),
                ),
            )
            .await
            {
                StreamWake::AuthenticationLapsed | StreamWake::Item(None) => break,
                StreamWake::PolicyChanged => (true, None),
                StreamWake::Item(Some(attributed)) => (false, Some(attributed)),
            };
            if let Some(live) = live_view.as_mut() {
                let mut changed =
                    policy_changed || prime_age.due(std::time::Instant::now());
                while live.refresh() || changed {
                    changed = false;
                    prime_sse_access_cache(
                        reprime_runtime.as_ref(),
                        reprime_access.as_ref(),
                        reprime_mob.as_ref(),
                    )
                        .await;
                }
                // A revoked `mob.observe` ends the stream, so the client
                // reconnects under the current policy. So does a mob the
                // caller may no longer address.
                if !mob_stream_allowed(live.view())
                    || !stream_mob_still_addressed(live.view(), reprime_mob.as_ref())
                {
                    break;
                }
            }
            let Some(attributed) = attributed else {
                continue;
            };
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
            // Every stream decides per agent (the private-owner rule binds
            // without enforced access too), so a new source is primed.
            if source_changed {
                prime_sse_access_cache(
                        reprime_runtime.as_ref(),
                        reprime_access.as_ref(),
                        reprime_mob.as_ref(),
                    ).await;
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
            // configuration committed before it is sent. The stream's grant
            // and its mob's address are both asked here once more: a change
            // of the rules that lands between the wake above and this send
            // ends the stream ahead of the event.
            if let Some(live) = live_view.as_mut() {
                while live.refresh() {
                    prime_sse_access_cache(
                        reprime_runtime.as_ref(),
                        reprime_access.as_ref(),
                        reprime_mob.as_ref(),
                    )
                        .await;
                }
                if !mob_stream_allowed(live.view())
                    || !stream_mob_still_addressed(live.view(), reprime_mob.as_ref())
                {
                    break;
                }
            }
            if !still_authenticated(authentication.as_mut()) {
                break;
            }
            let stream_view = live_view.as_ref().and_then(LiveAccessView::enforced);
            let audience_view = live_view.as_ref().map(LiveAccessView::view);
            let policy_identity = if let Some(exact) = exact {
                if !sse_access_allows_agent_attributes(audience_view, &exact.attributes)
                    || !exact.visible
                {
                    continue;
                }
                exact.identity
            } else {
                // The low-level AllowAll router predates runtime-aware
                // projection and has no roster handle. Preserve that default
                // compatibility, but any enforced access view without exact
                // roster authority must fail closed, and an agent private to
                // another person stays hidden.
                if stream_view.is_some()
                    || audience_view.is_some_and(|view| view.hides_agent(&authorization_alias))
                {
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

    /// A stream reads the agents' attributes again once its last read is as
    /// old as the bound a kept record has, and not before: right below the
    /// bound it decides on what it read, at the bound it reads again, and
    /// that read starts a new bound.
    #[test]
    fn a_stream_reads_the_attributes_again_once_its_last_read_is_as_old_as_the_bound() {
        let bound = crate::identity_first::runtime::UNSEATED_RECORD_MAX_AGE;
        assert_eq!(bound, Duration::from_secs(20));
        let below = bound.saturating_sub(Duration::from_millis(1));
        let opened = std::time::Instant::now();
        let mut age = StreamPrimeAge::primed(opened);
        assert!(!age.due(opened));
        assert!(!age.due(opened + below));
        assert!(age.due(opened + bound), "the read is as old as the bound");
        // The read at the bound counts: the next one is a bound later.
        assert!(!age.due(opened + bound + Duration::from_millis(1)));
        assert!(!age.due(opened + bound + below));
        assert!(age.due(opened + bound + bound));
        // An instant before the last read reads nothing.
        assert!(!age.due(opened));
    }

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
                    attached_epoch: None,
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
            mob_incarnation: None,
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

/// What the WorkGraph fact stream route reads its facts from.
#[derive(Clone)]
enum WorkGraphFactsSource {
    /// One fact hub: a router a host built by hand. It holds no mob, so it
    /// refuses every id a request names unless the host's resolver says
    /// the id is the launch mob's.
    Own(WorkGraphFactHub),
    /// The fact hub of the mob a request names, resolved by the host's
    /// resolver: a router the runtime built.
    Resolved,
}

#[derive(Clone)]
struct WorkGraphFactsSseState {
    source: WorkGraphFactsSource,
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
    workgraph_facts_router(WorkGraphFactsSseState {
        source: WorkGraphFactsSource::Own(hub),
        decisions,
        session_fences,
        access,
    })
}

/// The WorkGraph fact stream router of a [`crate::UnifiedRuntime`]: it
/// serves the facts of the mob a request names with `mob_id`, resolved by
/// the resolver the runtime layers over its routers, and the launch mob's
/// for a request that names none. A mob that has no WorkGraph view answers
/// a caller who may address it that it has none.
pub(crate) fn workgraph_facts_sse_router_of_resolved_mobs(
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
    session_fences: ConsoleSessionFences,
) -> Router {
    workgraph_facts_router(WorkGraphFactsSseState {
        source: WorkGraphFactsSource::Resolved,
        decisions,
        session_fences,
        access,
    })
}

/// The one mount of the WorkGraph fact stream route.
fn workgraph_facts_router(state: WorkGraphFactsSseState) -> Router {
    Router::new()
        .route(
            WORKGRAPH_FACTS_STREAM_PATH,
            get(workgraph_facts_sse_handler),
        )
        .with_state(state)
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

/// The WorkGraph fact stream of the hub `receiver` subscribes to.
/// `other_mob` is the stream's mob when it is not the launch mob: the
/// stream ends once its caller may no longer address it
/// ([`stream_mob_still_addressed`]).
fn workgraph_facts_event_stream(
    mut receiver: broadcast::Receiver<crate::types::UnifiedEvent>,
    serializer: WorkGraphEventSerializer,
    mut live_view: Option<LiveAccessView>,
    other_mob: Option<meerkat_mob::MobId>,
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
            // reconnects under the current policy; so does a lapsed token,
            // and so does a mob the caller may no longer address.
            if let Some(live) = live_view.as_mut()
                && (live.refresh() || policy_changed)
                && !(workgraph_stream_allowed(live.view())
                    && stream_mob_still_addressed(live.view(), other_mob.as_ref()))
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
    resolver: Option<axum::Extension<crate::unified_runtime::MobResolver>>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<Value>)>
{
    let caller = sse_access_context(
        state.decisions.as_ref(),
        &state.session_fences,
        state.access.as_ref(),
        &headers,
        &uri,
    )
    .map_err(|()| sse_unauthorized("workgraph facts stream requires a valid auth token"))?;
    let resolver = resolver.as_ref().map(|axum::Extension(resolver)| resolver);
    // The mob the request names is judged with the caller before the
    // stream opens. A router built over one fact hub holds no mob: without
    // the host's resolver it knows no mob by its id and refuses every id a
    // request names. The runtime's router reads the hub of the mob the
    // request names; `owner` is the stream's lease on a mob other than the
    // launch mob, kept until the stream ends.
    let (hub, owner, other_mob, caller) = match state.source {
        WorkGraphFactsSource::Own(hub) => {
            let caller = refuse_stream_of_another_mob(&uri, resolver, None, caller).await?;
            (Some(hub), None, None, caller)
        }
        WorkGraphFactsSource::Resolved => {
            let resolver = resolver.ok_or_else(stream_route_without_resolver)?;
            let (mut mob, caller) = stream_mob_of_request(&uri, resolver, caller).await?;
            let other_mob = mob.other_mob().cloned();
            (
                mob.resolved.workgraph_fact_hub(),
                mob.take_owner(),
                other_mob,
                caller,
            )
        }
    };
    let SseCaller {
        access_view,
        authentication,
        ..
    } = caller;
    // The stream decides under the policy committed before each fact, not
    // the one the request read; the gate below reads the same view.
    let live_view = access_view.map(LiveAccessView::new);
    // The mob's address by the live view, ahead of the stream's own grant.
    stream_mob_addressed_at_its_opening(live_view.as_ref(), other_mob.as_ref())?;
    if live_view
        .as_ref()
        .is_some_and(|live| !workgraph_stream_allowed(live.view()))
    {
        return Err(sse_access_denied(ACTION_WORKGRAPH_VIEW));
    }
    // A mob that has no WorkGraph view has no facts to stream: a caller
    // who may address the mob and may watch WorkGraph facts is told so.
    let Some(hub) = hub else {
        return Err(http_error(
            StatusCode::NOT_FOUND,
            WORKGRAPH_UNAVAILABLE_KIND,
        ));
    };

    let receiver = hub.subscribe();
    let facts = workgraph_facts_event_stream(
        receiver,
        serialize_workgraph_event,
        live_view,
        other_mob,
        authentication,
        shutdown.map(|axum::Extension(shutdown)| shutdown),
    );
    let stream = stream! {
        // The stream's owner lease on its mob, given back when the stream
        // ends; the stream ends when that mob's actor closes its admission.
        let owner = owner;
        let mut mob_gone = owner.as_ref().map(meerkat_mob::MobHandle::admission_closed);
        tokio::pin!(facts);
        while let Some(event) = next_until_mob_gone(mob_gone.as_mut(), facts.next()).await {
            yield event;
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(DEFAULT_KEEP_ALIVE_INTERVAL)
            .text(KEEP_ALIVE_TEXT),
    ))
}

/// `error` of the WorkGraph fact stream's answer for a mob that has no
/// WorkGraph view: the kind the request methods answer for a runtime
/// without one.
const WORKGRAPH_UNAVAILABLE_KIND: &str = "workgraph_unavailable";

/// What the structural stream route reads its ledger from.
#[derive(Clone)]
enum StructuralStreamSource {
    /// One mob's own handle and event store: a router a host built by
    /// hand. It serves that mob only.
    Own(Box<StructuralStreamOwn>),
    /// The ledger of the mob a request names, resolved by the host's
    /// resolver: a router the runtime built. The handle, the event store
    /// and the runtime the attributes are primed from are that mob's.
    Resolved,
}

/// What a structural stream router built over one mob holds of that mob.
#[derive(Clone)]
struct StructuralStreamOwn {
    handle: MobHandle,
    store: MobEventsStore,
}

#[derive(Clone)]
struct MobStructuralSseState {
    source: StructuralStreamSource,
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
        StructuralStreamSource::Own(Box::new(StructuralStreamOwn { handle, store })),
        decisions,
        access,
        Arc::new(AllowAllConsoleVisibilityPolicy),
        false,
        ConsoleSessionFences::detached(),
    )
}

/// The structural stream router of a [`crate::UnifiedRuntime`]: it serves
/// the ledger of the mob a request names with `mob_id`, resolved by the
/// resolver the runtime layers over its routers, and the launch mob's for
/// a request that names none. It requires an exact roster authority, as
/// the runtime's router over one mob does.
pub(crate) fn mob_structural_events_sse_router_of_resolved_mobs(
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    session_fences: ConsoleSessionFences,
) -> Router {
    mob_structural_events_sse_router_configured(
        StructuralStreamSource::Resolved,
        decisions,
        access,
        visibility_policy,
        true,
        session_fences,
    )
}

fn mob_structural_events_sse_router_configured(
    source: StructuralStreamSource,
    decisions: Option<RuntimeDecisionState>,
    access: Option<AccessController>,
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
            source,
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
///
/// The caller the route did not identify is told by its console, as the
/// Console's own routes tell it ([`crate::runtime::console_identifies_people`]):
/// kept apart from the console's people where it can identify a person, the
/// one operator where it identifies nobody. A route with no decision state
/// has no console to ask, and keeps its caller apart.
pub(crate) fn sse_access_context(
    decisions: Option<&RuntimeDecisionState>,
    fences: &ConsoleSessionFences,
    access: Option<&AccessController>,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<SseCaller, ()> {
    let Some(decisions) = decisions else {
        let audience = sse_audience_view(access, None, None);
        return Ok(SseCaller {
            access_view: access.map(|_| audience.clone()),
            audience,
            authentication: None,
        });
    };
    let identifies_people = crate::runtime::console_identifies_people(decisions, access);
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
        // per-user ABAC grants apply; everyone else is anonymous. A token is
        // resolved only where the console can identify a person: without an
        // access controller a token grants nothing, and without a key no
        // token verifies.
        token.filter(|_| identifies_people).and_then(|token| {
            crate::runtime::resolve_authorized_console_caller_from_token(decisions, fences, &token)
        })
    };
    let private_owner_key = caller
        .as_ref()
        .and_then(|caller| caller.human.as_ref())
        .map(crate::http_console::console_person_private_owner_key);
    let audience = sse_audience_view(
        access,
        caller.as_ref().map(|caller| caller.auth.email.as_str()),
        private_owner_key.as_deref(),
    )
    .on_console(identifies_people);
    Ok(SseCaller {
        access_view: access.map(|_| audience.clone()),
        audience,
        authentication: caller.map(|caller| StreamAuthentication::new(caller.verification)),
    })
}

/// The agent audience of a stream's caller: `access`'s view when an access
/// controller is wired, else a view with no access configuration that only
/// the private-owner rule decides ([`crate::access::PRIVATE_TO_LABEL`]),
/// one per stream.
fn sse_audience_view(
    access: Option<&AccessController>,
    subject: Option<&str>,
    private_owner_key: Option<&str>,
) -> AccessView {
    access
        .cloned()
        .unwrap_or_else(AccessController::audience_only)
        .view_for_caller(subject, private_owner_key)
}

/// The caller of an SSE route, as its request authenticated it.
pub(crate) struct SseCaller {
    /// The caller's [`AccessView`] when an [`AccessController`] is wired
    /// (an anonymous view on open routes).
    pub(crate) access_view: Option<AccessView>,
    /// The caller's agent audience, always present: `access_view` when an
    /// access controller is wired, else a view with no access configuration.
    /// Every per-agent decision of a stream reads it: it hides the agents
    /// private to another person, enforced access or not.
    pub(crate) audience: AccessView,
    /// The caller's authentication when a token identified the caller: a
    /// stream ends once the verifier no longer accepts that token.
    pub(crate) authentication: Option<StreamAuthentication>,
}

impl crate::rpc::mob_addressing::DoorCaller for SseCaller {
    fn access_view(&self) -> Option<&AccessView> {
        Some(&self.audience)
    }

    fn seeing_mob(self, mob_id: &meerkat_mob::MobId) -> Self {
        Self {
            access_view: self.access_view.map(|view| view.in_mob(mob_id)),
            audience: self.audience.in_mob(mob_id),
            authentication: self.authentication,
        }
    }
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
    resolver: Option<axum::Extension<crate::unified_runtime::MobResolver>>,
    headers: HeaderMap,
    uri: Uri,
    Query(params): Query<MobStructuralStreamQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<Value>)>
{
    let caller = sse_access_context(
        state.decisions.as_ref(),
        &state.session_fences,
        state.access.as_ref(),
        &headers,
        &uri,
    )
    .map_err(|()| sse_unauthorized("mob_events stream requires a valid auth token"))?;
    let resolver = resolver.as_ref().map(|axum::Extension(resolver)| resolver);
    // The `mob_id` of this route names the mob whose ledger is read: it is
    // judged with the caller, and an id the host does not serve is refused
    // before the stream opens. The runtime's router reads the ledger of the
    // mob the request names (`owner` is the stream's lease on a mob other
    // than the launch mob, kept until the stream ends); a router built over
    // one mob's handle serves that mob alone.
    let (handle, store, prime_runtime, other_mob, owner, caller) = match state.source {
        StructuralStreamSource::Resolved => {
            let resolver = resolver.ok_or_else(stream_route_without_resolver)?;
            let (mut mob, caller) = stream_mob_of_request(&uri, resolver, caller).await?;
            let owner = mob.take_owner();
            let other_mob = mob.other_mob().cloned();
            (
                mob.resolved.mob_handle(),
                mob.resolved.mob_events_store(),
                Some(mob.mob_runtime().clone()),
                other_mob,
                owner,
                caller,
            )
        }
        // A router built over one mob's handle primes from the roster the
        // handle answers: it holds no runtime.
        StructuralStreamSource::Own(own) => {
            let StructuralStreamOwn { handle, store } = *own;
            let caller =
                refuse_stream_of_another_mob(&uri, resolver, Some(handle.mob_id()), caller).await?;
            (handle, store, None, None, None, caller)
        }
    };
    let SseCaller {
        audience,
        mut authentication,
        ..
    } = caller;
    // The stream's audience controller: the access controller when one is
    // wired, else the stream's own, primed either way.
    let audience_access = audience.controller();
    prime_structural_access_cache(
        prime_runtime.as_ref(),
        &handle,
        Some(&audience_access),
        other_mob.as_ref(),
    )
    .await;
    // The stream decides under the policy committed before each event, not
    // the one the request read; the gate below reads the same view.
    let mut live_view = Some(LiveAccessView::new(audience));
    // The mob's address by the live view, ahead of the stream's own grant.
    stream_mob_addressed_at_its_opening(live_view.as_ref(), other_mob.as_ref())?;
    // The attributes were read just above; the stream reads them again
    // before it decides an item once that read is as old as a kept
    // record's bound.
    let mut prime_age = StreamPrimeAge::primed(std::time::Instant::now());
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
    let events_view = handle.events();

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

    // Captured so the long-lived stream can re-prime the shared attribute cache
    // for members spawned AFTER the one-time subscribe prime.
    let reprime_runtime = prime_runtime;
    let reprime_handle = handle.clone();
    let reprime_access = Some(audience_access);
    let reprime_mob = other_mob;
    let visibility_handle = handle;
    let visibility_policy = state.visibility_policy;
    let require_exact_authority = state.require_exact_authority;
    // The members a mob-level event named that the attributes did not
    // know: each is read for once.
    let mut primed_unknown = std::collections::HashSet::<String>::new();
    let mut shutdown = shutdown.map(|axum::Extension(shutdown)| shutdown);
    let stream = stream! {
        // The stream's owner lease on its mob, given back when the stream
        // ends; the stream ends when that mob's actor closes its admission.
        let owner = owner;
        let mut mob_gone = owner.as_ref().map(meerkat_mob::MobHandle::admission_closed);
        loop {
            let (policy_changed, event) = match wake_or_policy_change(
                live_view.as_mut(),
                authentication.as_mut(),
                next_until_mob_gone(
                    mob_gone.as_mut(),
                    next_until_stream_shutdown(shutdown.as_mut(), subscription.event_rx.recv()),
                ),
            )
            .await
            {
                StreamWake::AuthenticationLapsed | StreamWake::Item(None) => break,
                StreamWake::PolicyChanged => (true, None),
                StreamWake::Item(Some(event)) => (false, Some(event)),
            };
            if let Some(live) = live_view.as_mut() {
                let mut changed =
                    policy_changed || prime_age.due(std::time::Instant::now());
                while live.refresh() || changed {
                    changed = false;
                    prime_structural_access_cache(
                        reprime_runtime.as_ref(),
                        &reprime_handle,
                        reprime_access.as_ref(),
                        reprime_mob.as_ref(),
                    )
                    .await;
                }
                // A revoked `mob.observe` ends the stream, so the client
                // reconnects under the current policy. So does a mob the
                // caller may no longer address.
                if !mob_stream_allowed(live.view())
                    || !stream_mob_still_addressed(live.view(), reprime_mob.as_ref())
                {
                    break;
                }
            }
            let Some(event) = event else {
                continue;
            };
            let projection = store.project_event_with_authority(&event).await;
            let mob_level = projection.envelope.agent_identity.is_none();
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
                // Every stream decides per agent (the private-owner rule
                // binds without enforced access too).
                prime_structural_access_cache(
                    reprime_runtime.as_ref(),
                    &reprime_handle,
                    reprime_access.as_ref(),
                    reprime_mob.as_ref(),
                )
                .await;
            }
            let projected = project_structural_member(
                &visibility_handle,
                reprime_runtime.as_ref(),
                visibility_policy.as_ref(),
                projection,
                require_exact_authority,
            )
            .await;
            // A mob-level event reads nothing above. One that names a
            // member the attributes do not know (a record that appeared
            // since they were last read) reads them first, once for each
            // such member: a member they do not know names no owner.
            if mob_level
                && let Some(view) = live_view.as_ref().map(LiveAccessView::view)
                && projected.names_member_unknown_to(view, &mut primed_unknown)
            {
                prime_structural_access_cache(
                    reprime_runtime.as_ref(),
                    &reprime_handle,
                    reprime_access.as_ref(),
                    reprime_mob.as_ref(),
                )
                .await;
            }
            // Settled after the last await: the event is decided under the
            // configuration committed before it is sent. The stream's grant
            // and its mob's address are both asked here once more: a change
            // of the rules that lands between the wake above and this send
            // ends the stream ahead of the event.
            if let Some(live) = live_view.as_mut() {
                while live.refresh() {
                    prime_structural_access_cache(
                        reprime_runtime.as_ref(),
                        &reprime_handle,
                        reprime_access.as_ref(),
                        reprime_mob.as_ref(),
                    )
                    .await;
                }
                if !mob_stream_allowed(live.view())
                    || !stream_mob_still_addressed(live.view(), reprime_mob.as_ref())
                {
                    break;
                }
            }
            if !still_authenticated(authentication.as_mut()) {
                break;
            }
            let Some(envelope) = projected.decide(
                visibility_policy.as_ref(),
                live_view.as_ref().map(LiveAccessView::view),
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
