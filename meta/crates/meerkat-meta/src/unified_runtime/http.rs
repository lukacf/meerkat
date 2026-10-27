//! HTTP server and route assembly for the unified runtime.

use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderMap, header};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use futures::StreamExt;
use meerkat_core::comms::EventStream;

use crate::console_aggregator::{
    ConsoleVisibilityPolicy, HideImplicitDelegateMembersConsoleVisibilityPolicy,
    MetaConsoleAggregator,
};
use crate::http_console::{
    console_frontend_router, console_json_router_with_runtime_events_and_policy,
};
use crate::http_flow_editor::protected_flow_editor_router_with_runtime_catalog;
use crate::http_sse::{
    agent_events_sse_router_with_access_and_priming, mob_events_sse_router_with_access_and_priming,
    mob_structural_events_sse_router_with_access_and_priming,
    workgraph_facts_sse_router_under_session_fences,
};
use crate::runtime::RuntimeDecisionState;
use tokio::sync::Semaphore;
use tower::limit::GlobalConcurrencyLimitLayer;

use super::UnifiedRuntime;

/// The `/healthz` answer: `ok` as plain text, the liveness probe's answer,
/// and a JSON summary on request. The summary's `status` is `degraded` while
/// Core holds restored members broken (`broken_members` names each, with
/// Core's reason), as it is when the host's detached job projection says so,
/// and `unreadable` when that projection says so. Only the JSON summary
/// reads the broken members.
async fn healthz_response(
    headers: HeaderMap,
    job_health_projection: Arc<std::sync::RwLock<Option<serde_json::Value>>>,
    broken_members: impl std::future::Future<Output = Vec<(meerkat_mob::AgentIdentity, String)>>,
) -> axum::response::Response {
    let wants_json = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|item| item.trim().starts_with("application/json"))
        });
    if !wants_json {
        return "ok".into_response();
    }
    let broken_members = broken_members.await;
    let projection = job_health_projection
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let mut public_projection = projection.map_or_else(
        || {
            serde_json::json!({
                "status": "ok",
                "detached_jobs": null
            })
        },
        |projection| {
            serde_json::json!({
                "status": projection
                    .get("status")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!("ok")),
                "detached_jobs": projection
                    .get("detached_jobs")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null)
            })
        },
    );
    if !broken_members.is_empty() {
        // The worse of the two: a job health that is already unreadable
        // stays unreadable.
        if public_projection["status"] != "unreadable" {
            public_projection["status"] = serde_json::json!("degraded");
        }
        public_projection["broken_members"] = broken_members_json(&broken_members);
    }
    Json(public_projection).into_response()
}

/// Core's broken members on the wire: each member's public alias and
/// Core's reason.
pub(crate) fn broken_members_json(
    broken_members: &[(meerkat_mob::AgentIdentity, String)],
) -> serde_json::Value {
    serde_json::Value::Array(
        broken_members
            .iter()
            .map(|(member, reason)| {
                serde_json::json!({
                    "member_id": crate::member_comms_id::runtime_alias_str(member.as_str()),
                    "error": reason,
                })
            })
            .collect(),
    )
}

/// Default cap on the requests the stock reference app router serves at once.
///
/// The cap counts handlers in flight. A request holds its permit until its
/// response is produced, so an SSE stream runs without one once its headers
/// are sent, and a WebSocket upgrade holds one only until its 101 response. A
/// request over the cap waits for a permit, and nothing bounds how many
/// requests wait. The cap limits neither connections nor open SSE or
/// WebSocket streams, so a flood of connections still needs an outer limit or
/// timeout.
///
/// One set of permits covers every route and connection of a router the
/// public builders return, including the live WebSocket door that
/// `build_reference_app_router_with_live` mounts. The stock `rpc_gateway` puts
/// the host's routes and its live door under the same permits. Each built
/// router has its own permits, and routes an embedding host merges into a
/// returned router are outside them. Hosts that wrap Meta in their own axum
/// service may still choose a different outer limit, but the reference router
/// itself defaults to a ceiling high enough for real console usage.
pub const DEFAULT_REFERENCE_APP_MAX_CONCURRENT_REQUESTS: usize = 1024;

/// The reference app's bound on requests in flight: one set of permits for
/// every group of routes the app serves.
///
/// axum applies a `Router::layer` to every route on its own, and again for
/// every connection a handler route is served on. tower's
/// `ConcurrencyLimitLayer` makes new permits each time, so its bound held per
/// route and per connection; this bound shares its permits with every
/// application. Apply it to each group of routes before merging the group
/// into the app.
#[derive(Clone, Debug)]
pub(crate) struct ReferenceAppRequestBound {
    permits: Arc<Semaphore>,
}

impl ReferenceAppRequestBound {
    pub(crate) fn new(max_concurrent_requests: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_concurrent_requests)),
        }
    }

    /// Put `routes` under this bound.
    pub(crate) fn apply(&self, routes: Router) -> Router {
        routes.layer(GlobalConcurrencyLimitLayer::with_semaphore(Arc::clone(
            &self.permits,
        )))
    }
}

impl Default for ReferenceAppRequestBound {
    fn default() -> Self {
        Self::new(DEFAULT_REFERENCE_APP_MAX_CONCURRENT_REQUESTS)
    }
}

/// A runtime alias that is stale, or names no identity, has no member
/// stream to open: it answers as a missing member, not an internal error.
fn agent_event_target_error(
    agent_id: &str,
    error: crate::identity_first::IdentityRuntimeError,
) -> crate::MobRuntimeError {
    use crate::identity_first::IdentityRuntimeError;
    let missing = matches!(
        error,
        IdentityRuntimeError::StaleRuntimeAlias { .. } | IdentityRuntimeError::UnknownIdentity(_)
    ) || (crate::member_comms_id::is_reserved_generated_alias(agent_id)
        && matches!(error, IdentityRuntimeError::Internal(_)));
    if missing {
        crate::MobRuntimeError::Mob(meerkat_mob::MobError::MemberNotFound(
            meerkat_mob::AgentIdentity::from(agent_id),
        ))
    } else {
        crate::MobRuntimeError::InvalidConfig(error.to_string())
    }
}

async fn resolve_agent_event_member_id(
    handle: &meerkat_mob::MobHandle,
    agent_id: &str,
) -> meerkat_mob::ids::AgentIdentity {
    let direct = crate::member_comms_id::mob_member_id(agent_id);
    if handle.get_member(&direct).await.ok().flatten().is_some() {
        return direct;
    }
    handle
        .list_members_including_retiring()
        .await
        .into_iter()
        .find(|entry| {
            crate::member_comms_id::durable_identity_label(&entry.labels)
                .is_some_and(|identity| identity == agent_id)
        })
        .map(|entry| entry.agent_identity)
        .unwrap_or(direct)
}

/// Whether a per-agent event subscription still belongs to the identity life
/// it was opened on: Meta still lists the identity, and Core's last binding of
/// it still names the session the subscription was opened on.
///
/// A retire keeps the binding (the next wake resumes the same session), so it
/// ends nothing. A reset, a delete or a respawn that mints a session moves the
/// binding to another session; a roster removal unregisters the identity; a
/// mob reset or destroy leaves no binding. Each of those ends the
/// subscription, and so does a read Core cannot answer.
async fn identity_event_session_is_current(
    identity_runtime: &crate::identity_first::IdentityRuntime,
    identity: &crate::identity_first::AgentIdentity,
    session: &meerkat_core::SessionId,
) -> bool {
    identity_runtime
        .status(identity)
        .await
        .is_ok_and(|status| status.session_id.as_ref() == Some(session))
}

/// Keep a per-agent event subscription bound to the identity life that was
/// current when the HTTP request was admitted: the session Core bound the
/// identity to ([`identity_event_session_is_current`]). Output from another
/// session, which the lower-plane stream can still deliver after a reset,
/// never reaches the subscriber under the identity's name.
fn session_authoritative_agent_event_stream(
    mut event_stream: EventStream,
    mut identity_events: tokio::sync::broadcast::Receiver<
        crate::identity_first::runtime::IdentityEvent,
    >,
    // The stream is handed to the HTTP client: it holds the owning view.
    identity_runtime: crate::identity_first::PublicIdentityRuntime,
    identity: crate::identity_first::AgentIdentity,
    session: meerkat_core::SessionId,
) -> EventStream {
    Box::pin(async_stream::stream! {
        // Every check reads and folds Core's whole member journal, so it runs
        // on the identity's lifecycle events and on this backstop tick, not
        // per payload: the tick also catches a change no verb of this
        // runtime announces (Core's reconciler, a mob reset), and it closes
        // an otherwise idle connection. Once a second is enough for that:
        // the per-payload session check below already stops another
        // session's output before it is forwarded.
        let mut authority_check = tokio::time::interval(Duration::from_secs(1));
        authority_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                // Prefer lifecycle invalidation and the due backstop over a
                // lower-plane event ready in the same scheduler turn.
                biased;
                lifecycle = identity_events.recv() => {
                    match lifecycle {
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            if !identity_event_session_is_current(
                                &identity_runtime,
                                &identity,
                                &session,
                            )
                            .await
                            {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = authority_check.tick() => {
                    if !identity_event_session_is_current(
                        &identity_runtime,
                        &identity,
                        &session,
                    )
                    .await
                    {
                        break;
                    }
                }
                event = event_stream.next() => {
                    let Some(event) = event else {
                        break;
                    };
                    // A payload another session emitted belongs to another
                    // life of the identity: the subscription ends before it.
                    if let meerkat_core::event::EventSourceIdentity::Session { session_id } =
                        &event.source
                        && *session_id != session
                    {
                        break;
                    }
                    yield event;
                }
            }
        }
    })
}

impl UnifiedRuntime {
    /// Establish the host-wide console payload policy before building views.
    /// Redaction applies before persistence and broadcast. Identity and member
    /// visibility remain per-view, so a restricted view cannot erase another
    /// authorized view's history. A later call cannot replace this owner policy.
    pub fn initialize_console_projection_with_policy(
        &self,
        policy: Arc<dyn ConsoleVisibilityPolicy>,
    ) -> Result<(), &'static str> {
        let mut initialized = false;
        self.console_projection.get_or_init(|| {
            initialized = true;
            self.create_console_projection(policy)
        });
        if initialized {
            Ok(())
        } else {
            Err("console projection policy is already initialized")
        }
    }

    fn create_console_projection(
        &self,
        policy: Arc<dyn ConsoleVisibilityPolicy>,
    ) -> MetaConsoleAggregator {
        struct HostPayloadPolicy(Arc<dyn ConsoleVisibilityPolicy>);
        impl ConsoleVisibilityPolicy for HostPayloadPolicy {
            fn redact_payload(
                &self,
                frame: &crate::console_aggregator::NewConsoleFrame,
            ) -> Option<serde_json::Value> {
                self.0.redact_payload(frame)
            }
        }
        // The runtime's own projection is a back-edge: it holds the observer
        // mob runtime and the non-owning identity runtime.
        let aggregator = MetaConsoleAggregator::new(self.console_log_store());
        aggregator.register_runtime_handles_with_policy(
            "default",
            "",
            self.mob_runtime.clone(),
            self.identity_first_context
                .as_ref()
                .map(|context| Arc::clone(&context.runtime)),
            self.console_events(),
            Arc::new(HostPayloadPolicy(policy)),
        );
        aggregator
    }

    /// Settle, from native admission evidence, the console sends a previous
    /// process accepted but did not see settle. Bounded and evidence only: a
    /// send whose input the runtime admitted is settled delivered; nothing
    /// is dispatched or failed. Gateways run it once at startup, after
    /// building the console router and before serving it. A runtime whose
    /// console projection was never built has nothing to settle. A candidate
    /// launch settles none: it logs how many it leaves to the authoritative
    /// launch.
    pub async fn reconcile_interrupted_console_sends(&self) {
        let Some(projection) = self.console_projection.get() else {
            return;
        };
        // A settlement writes the Console store the authoritative launch
        // reads, and that launch settles the sends at its own startup.
        if !self.speaks_for_composition() {
            let unsettled = projection.unsettled_send_count().await;
            tracing::warn!(
                unsettled,
                "candidate launch census: the console sends a previous process left unsettled \
                 stay so; the authoritative launch settles them at its startup"
            );
            return;
        }
        let outcome = projection.reconcile_interrupted_sends().await;
        if outcome.examined > 0 {
            tracing::info!(
                examined = outcome.examined,
                delivered = outcome.delivered,
                "reconciled interrupted console sends"
            );
        }
    }

    pub fn build_console_json_router(&self, decisions: RuntimeDecisionState) -> Router {
        self.build_console_json_router_with_policy(
            decisions,
            Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
        )
    }

    pub fn build_console_json_router_with_policy(
        &self,
        decisions: RuntimeDecisionState,
        visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    ) -> Router {
        // Existing single-policy hosts keep ingestion redaction. Hosts serving
        // different views can initialize an explicit host policy first.
        let projection = self
            .console_projection
            .get_or_init(|| self.create_console_projection(visibility_policy.clone()));
        let view = projection.policy_view(visibility_policy.clone());
        // The router is returned to the host: it holds the public owners.
        console_json_router_with_runtime_events_and_policy(
            decisions,
            self.mob_runtime().clone(),
            Some(self.module_runtime_handle()),
            self.contact_directory.clone(),
            self.event_log_store(),
            self.gateway_peer_keys().map(|keys| (*keys).clone()),
            Some(self.console_events()),
            Some(self.console_log_store()),
            Some(self.mob_events_store()),
            Some(Arc::clone(self.metadata_table())),
            self.identity_runtime(),
            visibility_policy,
            self.access_controller().cloned(),
            self.memory_panel_store(),
            self.console_operator_resolver(),
            self.console_identity_roster(),
            self.workgraph_service(),
            Some(self.topology_runtime_handle()),
            Some(Arc::clone(&self.job_health_projection)),
            Some(view),
            self.console_governed_ingress.clone(),
            self.console_session_fences().clone(),
            self.mob_announcements_slot(),
        )
    }

    /// Resolves Console viewers for host routes served beside the console,
    /// with the same decisions and access controller as the console router
    /// built from `decisions`.
    pub fn console_viewer_resolver(
        &self,
        decisions: &RuntimeDecisionState,
    ) -> crate::http_console::ConsoleViewerResolver {
        crate::http_console::ConsoleViewerResolver::new(
            decisions.clone(),
            self.access_controller().cloned(),
        )
        .with_session_fences(self.console_session_fences().clone())
    }

    /// The Console's stock send gate for a host route that prepares a
    /// Console send (see [`crate::http_console::ConsoleSendPrecheck`]): the
    /// same decisions, live roster and access controller as the console
    /// router built from `decisions`.
    pub fn console_send_precheck(
        &self,
        decisions: &RuntimeDecisionState,
    ) -> crate::http_console::ConsoleSendPrecheck {
        crate::http_console::ConsoleSendPrecheck::new(
            decisions.clone(),
            Some(self.mob_runtime().clone()),
            self.access_controller().cloned(),
        )
    }

    pub fn build_console_frontend_router(&self) -> Router {
        console_frontend_router()
    }

    pub fn build_reference_app_router(&self, decisions: RuntimeDecisionState) -> Router {
        self.reference_app_router_under(decisions, &ReferenceAppRequestBound::default())
    }

    /// [`Self::build_reference_app_router`] under `bound`, for a caller that
    /// merges more routes into the same app (see [`ReferenceAppRequestBound`]).
    pub(crate) fn reference_app_router_under(
        &self,
        decisions: RuntimeDecisionState,
        bound: &ReferenceAppRequestBound,
    ) -> Router {
        bound.apply(self.reference_app_routes(
            decisions,
            Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
        ))
    }

    /// The reference router plus the live doors the builder registered.
    ///
    /// Composes the live doors once ([`UnifiedRuntime::compose_live`]), then
    /// mounts them beside the console: the console voice controller as the
    /// request extension the console RPC reads, and the live WebSocket
    /// router (`/live/ws`) when the external channel is registered. An
    /// embedder that serves this router on its own listener gets the same
    /// surface the gateway binaries expose.
    ///
    /// Fails closed when console voice is composed but
    /// `decisions.console.require_app_auth` is false: the controller serves
    /// only authenticated principals, and an open console would report voice
    /// as unavailable with no diagnostic. Without a registered live door the
    /// result equals [`Self::build_reference_app_router`]. The live door
    /// shares the console's request bound.
    pub async fn build_reference_app_router_with_live(
        self: &Arc<Self>,
        decisions: RuntimeDecisionState,
    ) -> Result<Router, super::live_compose::LiveComposeError> {
        let composition = self.compose_live().await?;
        #[cfg(feature = "openai-live")]
        if composition
            .as_ref()
            .is_some_and(|composition| composition.console_voice().is_some())
            && !decisions.console.require_app_auth
        {
            return Err(super::live_compose::LiveComposeError::ConsoleVoiceRequiresAppAuth);
        }
        let bound = ReferenceAppRequestBound::default();
        let router = self.reference_app_router_under(decisions, &bound);
        let Some(composition) = composition else {
            return Ok(router);
        };
        let router = if composition.external_handler().is_some() {
            router.merge(bound.apply(meerkat_live::live_ws_router(Arc::clone(
                &composition.context().ws_state,
            ))))
        } else {
            router
        };
        #[cfg(feature = "openai-live")]
        let router = match composition.console_voice() {
            Some(controller) => router.layer(axum::Extension(controller.clone())),
            None => router,
        };
        Ok(router)
    }

    pub fn build_reference_app_router_with_console_visibility_policy(
        &self,
        decisions: RuntimeDecisionState,
        visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    ) -> Router {
        ReferenceAppRequestBound::default()
            .apply(self.reference_app_routes(decisions, visibility_policy))
    }

    /// The reference app's routes, before its request bound.
    fn reference_app_routes(
        &self,
        decisions: RuntimeDecisionState,
        visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    ) -> Router {
        // The router is returned to the host: it holds the public owners.
        let agent_runtime = self.mob_runtime().clone();
        let agent_identity_runtime = self.identity_runtime();
        let mob_runtime = self.mob_runtime().clone();
        // Every SSE route shares the same `RuntimeDecisionState` the
        // console RPC route uses: when `require_app_auth` is on, requests
        // must carry a valid bearer / auth_token. Pre-fix, only the
        // structural-events route gated; tier-2 (`/agents/{id}/events`)
        // and tier-3 (`/mob/events`) shipped unauthenticated.
        let flow_editor_decisions = decisions.clone();
        let flow_editor_runtime_catalog = self.mobpack_runtime_catalog_state_snapshot();
        let flow_editor_host_deploy = self.flow_editor_host_deploy;
        let sse_decisions_a = decisions.clone();
        let sse_decisions_b = decisions.clone();
        let sse_decisions_c = decisions.clone();
        let sse_decisions_d = decisions.clone();
        let access = self.access_controller().cloned();
        let agent_sse_visibility_policy = visibility_policy.clone();
        let mob_sse_visibility_policy = visibility_policy.clone();
        let structural_sse_visibility_policy = visibility_policy.clone();
        let job_health_projection = Arc::clone(&self.job_health_projection);
        let health_runtime = self.mob_runtime().clone();
        Router::new()
            .route(
                "/healthz",
                get(move |headers: HeaderMap| {
                    let job_health_projection = Arc::clone(&job_health_projection);
                    let health_runtime = health_runtime.clone();
                    async move {
                        let broken_members = async move {
                            crate::mob_handle_runtime::broken_members_of(&health_runtime.handle())
                                .await
                        };
                        healthz_response(headers, job_health_projection, broken_members).await
                    }
                }),
            )
            .merge(self.build_console_frontend_router())
            .merge(protected_flow_editor_router_with_runtime_catalog(
                flow_editor_decisions,
                flow_editor_runtime_catalog,
                access.clone(),
                flow_editor_host_deploy,
                self.console_session_fences().clone(),
                (!self.speaks_for_composition()).then(|| {
                    crate::http_flow_editor::FlowEditorCandidate {
                        state_dir: self
                            .resolved_storage()
                            .and_then(|storage| storage.state_dir),
                    }
                }),
                self.has_explicit_provider_credentials(),
            ))
            .merge(self.build_console_json_router_with_policy(decisions, visibility_policy))
            .merge(agent_events_sse_router_with_access_and_priming(
                Arc::new(move |agent_id, cursor| {
                    let runtime = agent_runtime.clone();
                    let identity_runtime = agent_identity_runtime.clone();
                    Box::pin(async move {
                        let agent_id =
                            crate::member_comms_id::runtime_alias_str(&agent_id).into_owned();
                        let handle = runtime.handle();
                        let target = if let Some(identity_runtime) = identity_runtime.as_ref() {
                            identity_runtime
                                .runtime()
                                .member_alias_lifecycle_target(&agent_id)
                                .await
                                .map_err(|error| agent_event_target_error(&agent_id, error))?
                        } else {
                            None
                        };
                        if let Some(target) = target {
                            let Some(authority_runtime) = identity_runtime.clone() else {
                                return Err(crate::MobRuntimeError::InvalidConfig(
                                    "identity event target resolved without identity runtime"
                                        .to_string(),
                                ));
                            };
                            let authority_identity = authority_runtime
                                .identity_for_member_mutation(&agent_id)
                                .await
                                .ok_or_else(|| {
                                    crate::MobRuntimeError::InvalidConfig(format!(
                                        "identity event target lost authority before subscription: {agent_id}"
                                    ))
                                })?;
                            return crate::identity_first::IdentityRuntime::run_member_alias_targets_operation_tracked(
                                vec![target],
                                move || async move {
                                    // The lifecycle guard pins the authorized identity while
                                    // the subscription opens: it belongs to the session Core
                                    // binds the live identity to now.
                                    let status = authority_runtime
                                        .status(&authority_identity)
                                        .await
                                        .map_err(|error| error.to_string())?;
                                    if status.agent_runtime_id.is_none() {
                                        return Err(
                                            "identity event target has no current runtime alias"
                                                .to_string(),
                                        );
                                    }
                                    let session = status.session_id.ok_or_else(|| {
                                        "identity event target has no current session".to_string()
                                    })?;
                                    let identity_events = authority_runtime
                                        .subscribe(&authority_identity)
                                        .await
                                        .map_err(|error| error.to_string())?;
                                    let member_id = resolve_agent_event_member_id(
                                        &handle,
                                        authority_identity.as_str(),
                                    )
                                    .await;
                                    let subscription =
                                        subscribe_agent_events_resuming(&handle, &member_id, cursor)
                                            .await
                                            .map_err(|error| error.to_string())?;
                                    Ok(crate::http_sse::AgentEventReplay {
                                        epoch: subscription.epoch,
                                        stream: session_authoritative_agent_event_stream(
                                            subscription.stream,
                                            identity_events,
                                            authority_runtime,
                                            authority_identity,
                                            session,
                                        ),
                                    })
                                },
                            )
                            .await
                            .map_err(|error| {
                                crate::MobRuntimeError::InvalidConfig(error.to_string())
                            });
                        }
                        let member_id = resolve_agent_event_member_id(&handle, &agent_id).await;
                        subscribe_agent_events_resuming(&handle, &member_id, cursor)
                            .await
                            .map(|subscription| crate::http_sse::AgentEventReplay {
                                epoch: subscription.epoch,
                                stream: subscription.stream,
                            })
                            .map_err(Into::into)
                    })
                }),
                Some(sse_decisions_a),
                access.clone(),
                Some(self.mob_runtime().clone()),
                self.identity_runtime_ref().cloned(),
                agent_sse_visibility_policy,
                self.console_session_fences().clone(),
            ))
            .merge(mob_events_sse_router_with_access_and_priming(
                Arc::new(move || {
                    let mob_runtime = mob_runtime.clone();
                    Box::pin(async move { mob_runtime.handle().subscribe_mob_events().await })
                }),
                Some(sse_decisions_b),
                access.clone(),
                Some(self.mob_runtime().clone()),
                mob_sse_visibility_policy,
                self.console_session_fences().clone(),
            ))
            .merge(mob_structural_events_sse_router_with_access_and_priming(
                self.mob_handle(),
                self.mob_events_store(),
                Some(sse_decisions_c),
                access.clone(),
                Some(self.mob_runtime().clone()),
                structural_sse_visibility_policy,
                self.console_session_fences().clone(),
            ))
            .merge(self.workgraph_fact_hub().map_or_else(Router::new, |hub| {
                workgraph_facts_sse_router_under_session_fences(
                    hub,
                    Some(sse_decisions_d),
                    access,
                    self.console_session_fences().clone(),
                )
            }))
    }
}

/// Subscribe to a member's agent events for an SSE connection: from the
/// cursor its `Last-Event-ID` names, or from the actor's earliest retained
/// event (meerkat #1236), so a client that connects after a run started
/// still receives it from `run_started` and a reconnect resumes after the
/// last event it received. A cursor from another sequence space (a restart
/// since) or ahead of it replays the current space from its earliest event.
async fn subscribe_agent_events_resuming(
    handle: &meerkat_mob::MobHandle,
    member_id: &meerkat_mob::AgentIdentity,
    cursor: meerkat_core::comms::SessionEventCursor,
) -> Result<meerkat_mob::AgentEventSubscription, meerkat_mob::MobError> {
    match handle.subscribe_agent_events_from(member_id, cursor).await {
        Err(meerkat_mob::MobError::AgentEventCursorRejected { .. })
            if cursor != meerkat_core::comms::SessionEventCursor::Earliest =>
        {
            handle
                .subscribe_agent_events_from(
                    member_id,
                    meerkat_core::comms::SessionEventCursor::Earliest,
                )
                .await
        }
        result => result,
    }
}

/// Test support for the reference app's request bound: routes whose handlers
/// stay in flight until released, and requests that each open their own
/// connection to an app served the way the gateways serve it.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
pub(crate) mod request_bound_test_support {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use axum::Router;
    use axum::extract::State;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::watch;

    /// Counts the hold routes' handlers in flight and the most seen at once;
    /// every handler waits until [`Self::release`].
    #[derive(Clone)]
    pub(crate) struct HeldRequests {
        in_flight: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        release: Arc<watch::Sender<bool>>,
    }

    struct InFlight(Arc<AtomicUsize>);

    impl Drop for InFlight {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl HeldRequests {
        pub(crate) fn new() -> Self {
            Self {
                in_flight: Arc::new(AtomicUsize::new(0)),
                peak: Arc::new(AtomicUsize::new(0)),
                release: Arc::new(watch::channel(false).0),
            }
        }

        pub(crate) fn peak(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }

        pub(crate) fn release(&self) {
            self.release.send_replace(true);
        }

        async fn hold(self) -> &'static str {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            let _in_flight = InFlight(Arc::clone(&self.in_flight));
            self.peak.fetch_max(now, Ordering::SeqCst);
            let mut released = self.release.subscribe();
            let _ = released.wait_for(|released| *released).await;
            "held"
        }

        /// `GET plain` is a handler route of a router without state, which
        /// axum layers again for every connection; `GET with_state` belongs
        /// to a router that already has its state, layered once per route.
        pub(crate) fn routes(&self, plain: &str, with_state: &str) -> Router {
            let held = self.clone();
            Router::new()
                .route(plain, get(move || held.clone().hold()))
                .merge(
                    Router::new()
                        .route(with_state, get(|State(held): State<Self>| held.hold()))
                        .with_state(self.clone()),
                )
        }

        /// Wait until at least `count` handlers are in flight.
        pub(crate) async fn wait_in_flight(&self, count: usize) {
            tokio::time::timeout(Duration::from_secs(30), async {
                while self.in_flight.load(Ordering::SeqCst) < count {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the hold routes never had this many handlers in flight");
        }
    }

    /// Serve `app` on a loopback listener with `axum::serve`, as the
    /// gateways do.
    pub(crate) async fn serve(app: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        (address, server)
    }

    /// `GET path` on a connection of its own; returns the response status.
    pub(crate) async fn get_on_own_connection(address: SocketAddr, path: String) -> u16 {
        let mut stream = tokio::net::TcpStream::connect(address)
            .await
            .expect("connect");
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .expect("write request");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .expect("read response");
        String::from_utf8_lossy(&response)
            .split(' ')
            .nth(1)
            .and_then(|status| status.parse().ok())
            .unwrap_or_else(|| panic!("no status line in the response to {path}"))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;

    use axum::http::{HeaderMap, header};
    use futures::StreamExt;
    use meerkat_client::TestClient;
    use meerkat_core::comms::EventStream;
    use meerkat_mob::{MobDefinition, MobRuntimeMode, ProfileName};

    use super::{
        DEFAULT_REFERENCE_APP_MAX_CONCURRENT_REQUESTS, healthz_response,
        session_authoritative_agent_event_stream,
    };
    use crate::identity_first::{
        AgentAddressability, AgentIdentity, DurableAgentSpec, LocalContinuityStore,
        LocalLeaseProvider, MutableRosterProvider,
    };
    use crate::unified_runtime::UnifiedRuntimeBuilder;

    #[tokio::test]
    async fn host_console_payload_policy_is_explicit_and_cannot_be_replaced_by_later_views() {
        struct RedactHost;
        impl crate::console_aggregator::ConsoleVisibilityPolicy for RedactHost {
            fn redact_payload(
                &self,
                _: &crate::console_aggregator::NewConsoleFrame,
            ) -> Option<serde_json::Value> {
                Some(serde_json::json!({"host":"redacted"}))
            }
        }
        let definition = MobDefinition::from_toml(
            "[mob]\nid = \"host-policy-owner\"\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        )
        .expect("definition");
        let runtime = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .default_llm_client(Arc::new(TestClient::default()))
            .build()
            .await
            .expect("runtime");
        runtime
            .initialize_console_projection_with_policy(Arc::new(RedactHost))
            .expect("initial policy");
        assert!(
            runtime
                .initialize_console_projection_with_policy(Arc::new(
                    crate::console_aggregator::AllowAllConsoleVisibilityPolicy,
                ))
                .is_err(),
            "a later view must not replace the ingestion policy"
        );
        let first = runtime
            .console_projection
            .get()
            .expect("projection")
            .store();
        let decisions = crate::RuntimeDecisionState::local_console(
            crate::ConsolePolicy {
                require_app_auth: false,
                ..Default::default()
            },
            None,
        );
        drop(runtime.build_console_json_router_with_policy(
            decisions,
            Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
        ));
        assert!(Arc::ptr_eq(
            &first,
            &runtime
                .console_projection
                .get()
                .expect("same owner")
                .store()
        ));
        let mut stream = runtime.console_projection.get().expect("owner").subscribe();
        // A runtime-plane event: no member's event takes the system identity.
        let source = runtime
            .console_events()
            .append_runtime_plane_event(
                "text_delta",
                serde_json::json!({"delta":"host-private-token"}),
            )
            .await;
        let projected = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let crate::console_aggregator::ConsoleTimelineEvent::ConsoleFrame { frame } =
                    stream.recv().await.expect("owner stream")
                    && frame.source_event_id.as_deref() == Some(source.event_id.as_str())
                {
                    break frame;
                }
            }
        })
        .await
        .expect("source reaches canonical owner");
        assert_eq!(projected.payload, serde_json::json!({"host":"redacted"}));
        assert_eq!(projected.status, crate::ConsoleFrameStatus::Redacted);
        let stored = first
            .frame_by_dedupe_key(&projected.dedupe_key)
            .await
            .expect("raw store")
            .expect("persisted");
        assert_eq!(
            stored.payload, projected.payload,
            "host redaction precedes both persistence and raw broadcast"
        );
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn multiple_console_routers_reuse_one_runtime_projection() {
        let definition = MobDefinition::from_toml(
            r#"
[mob]
id = "shared-console-router-owner"
[profiles.worker]
model = "gpt-5.5"
"#,
        )
        .expect("definition");
        let runtime = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .default_llm_client(Arc::new(TestClient::default()))
            .build()
            .await
            .expect("runtime");
        let decisions = crate::runtime::RuntimeDecisionState::local_console(
            crate::ConsolePolicy {
                require_app_auth: false,
                ..Default::default()
            },
            None,
        );
        let _first = runtime.build_console_json_router(decisions.clone());
        let first = runtime
            .console_projection
            .get()
            .expect("projection")
            .store();
        let _second = runtime.build_console_json_router(decisions);
        let second = runtime
            .console_projection
            .get()
            .expect("same projection")
            .store();
        assert!(Arc::ptr_eq(&first, &second));
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn console_projection_tasks_release_replaced_and_shutdown_stores() {
        async fn assert_released(
            store: std::sync::Weak<crate::console_aggregator::InMemoryConsoleLogStore>,
        ) {
            tokio::time::timeout(Duration::from_secs(10), async {
                while store.upgrade().is_some() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("terminated console tasks must release their old store");
        }

        let definition = MobDefinition::from_toml(
            r#"
[mob]
id = "console-projector-lifecycle"
[profiles.worker]
model = "gpt-5.5"
"#,
        )
        .expect("definition");
        let mut runtime = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .default_llm_client(Arc::new(TestClient::default()))
            .build()
            .await
            .expect("runtime");
        let decisions = crate::runtime::RuntimeDecisionState::local_console(
            crate::ConsolePolicy {
                require_app_auth: false,
                ..Default::default()
            },
            None,
        );
        let old_store = Arc::new(crate::console_aggregator::InMemoryConsoleLogStore::new());
        let old_weak = Arc::downgrade(&old_store);
        runtime.set_console_log_store(old_store.clone());
        drop(runtime.build_console_json_router(decisions.clone()));
        drop(old_store);

        let new_store = Arc::new(crate::console_aggregator::InMemoryConsoleLogStore::new());
        let new_weak = Arc::downgrade(&new_store);
        runtime.set_console_log_store(new_store.clone());
        assert!(runtime.console_projection.get().is_none());
        assert_released(old_weak).await;
        drop(runtime.build_console_json_router(decisions.clone()));
        drop(runtime.build_console_json_router(decisions));
        drop(new_store);
        runtime.shutdown().await;
        drop(runtime);
        assert_released(new_weak).await;
    }

    #[test]
    fn reference_router_default_concurrency_allows_sse_fanout() {
        assert_eq!(DEFAULT_REFERENCE_APP_MAX_CONCURRENT_REQUESTS, 1024);
    }

    /// The reference app's request bound is app-wide: no more handlers run at
    /// once than it allows, across routes and connections, and a route merged
    /// in later under the same bound (as the live door and the gateway's host
    /// routes are) shares it. axum applies a `Router::layer` to every route on
    /// its own, and again for every connection a handler route is served on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reference_app_request_bound_holds_across_routes_and_connections() {
        use super::ReferenceAppRequestBound;
        use super::request_bound_test_support::{HeldRequests, get_on_own_connection, serve};

        const LIMIT: usize = 2;
        let definition = MobDefinition::from_toml(
            "[mob]\nid = \"reference-app-request-bound\"\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        )
        .expect("definition");
        let runtime = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .default_llm_client(Arc::new(TestClient::default()))
            .build()
            .await
            .expect("runtime");
        let decisions = crate::RuntimeDecisionState::local_console(
            crate::ConsolePolicy {
                require_app_auth: false,
                ..Default::default()
            },
            None,
        );
        let bound = ReferenceAppRequestBound::new(LIMIT);
        let held = HeldRequests::new();
        let app = runtime
            .reference_app_router_under(decisions, &bound)
            .merge(bound.apply(held.routes("/test/held/plain", "/test/held/with-state")));
        let (address, server) = serve(app).await;

        let requests = (0..6)
            .map(|index| {
                let path = if index % 2 == 0 {
                    "/test/held/plain"
                } else {
                    "/test/held/with-state"
                };
                tokio::spawn(get_on_own_connection(address, path.to_string()))
            })
            .collect::<Vec<_>>();
        held.wait_in_flight(LIMIT).await;
        // A route of the reference app itself, on yet another connection.
        let healthz = tokio::spawn(get_on_own_connection(address, "/healthz".to_string()));
        // Time for every outstanding request to start, if the bound let it.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            held.peak(),
            LIMIT,
            "more handlers ran at once than the bound allows"
        );
        assert!(
            !healthz.is_finished(),
            "/healthz was served while the bound was full"
        );

        held.release();
        for request in requests {
            assert_eq!(request.await.expect("held request"), 200);
        }
        assert_eq!(healthz.await.expect("healthz request"), 200);
        assert_eq!(held.peak(), LIMIT);
        server.abort();
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn healthz_preserves_text_probe_and_negotiates_structured_jobs() {
        let projection = Arc::new(std::sync::RwLock::new(Some(serde_json::json!({
            "status": "ok",
            "detached_jobs": {
                "queued": 1,
                "running": 2,
                "awaiting_members": 1,
                "stale_leases": 0,
                "needs_attention": 0,
                "pending_outbox_jobs": 0,
                "runtime_inbox_backlog": 0
            }
        }))));
        let text = healthz_response(
            HeaderMap::new(),
            Arc::clone(&projection),
            std::future::ready(Vec::new()),
        )
        .await;
        assert_eq!(text.status(), axum::http::StatusCode::OK);
        assert_eq!(
            axum::body::to_bytes(text.into_body(), usize::MAX)
                .await
                .expect("text body"),
            "ok"
        );

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, "application/json".parse().expect("accept"));
        let json = healthz_response(headers, projection, std::future::ready(Vec::new())).await;
        let body = axum::body::to_bytes(json.into_body(), usize::MAX)
            .await
            .expect("json body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(body["detached_jobs"]["running"], 2);
        assert_eq!(body["status"], "ok");
        assert!(body.get("broken_members").is_none(), "{body}");
    }

    /// A job health that is already unreadable stays unreadable while Core
    /// holds members broken: the summary keeps the worse of the two.
    #[tokio::test]
    async fn healthz_keeps_an_unreadable_job_health_with_broken_members() {
        let projection = Arc::new(std::sync::RwLock::new(Some(serde_json::json!({
            "status": "unreadable",
            "detached_jobs": null
        }))));
        let broken = vec![(
            crate::member_comms_id::roster_member_id_for_identity("personal:alice"),
            "restored member cannot publish its comms endpoint".to_string(),
        )];
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, "application/json".parse().expect("accept"));
        let json = healthz_response(headers, projection, std::future::ready(broken)).await;
        let body = axum::body::to_bytes(json.into_body(), usize::MAX)
            .await
            .expect("json body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(body["status"], "unreadable", "{body}");
        assert_eq!(
            body["broken_members"][0]["member_id"], "personal:alice",
            "{body}"
        );
    }

    /// While Core holds restored members broken, the JSON summary reads
    /// degraded and names each member with Core's reason; the plain-text
    /// liveness answer stays `ok`.
    #[tokio::test]
    async fn healthz_reads_degraded_and_names_the_broken_members() {
        let projection = Arc::new(std::sync::RwLock::new(None));
        let broken = vec![(
            crate::member_comms_id::roster_member_id_for_identity("personal:alice"),
            "restored member cannot publish its comms endpoint".to_string(),
        )];
        // The plain-text answer never reads the broken members.
        let read = std::sync::atomic::AtomicBool::new(false);
        let text = healthz_response(HeaderMap::new(), Arc::clone(&projection), async {
            read.store(true, std::sync::atomic::Ordering::SeqCst);
            broken.clone()
        })
        .await;
        assert_eq!(text.status(), axum::http::StatusCode::OK);
        assert!(
            !read.load(std::sync::atomic::Ordering::SeqCst),
            "the plain-text answer reads no broken member"
        );

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, "application/json".parse().expect("accept"));
        let json = healthz_response(headers, projection, std::future::ready(broken)).await;
        let body = axum::body::to_bytes(json.into_body(), usize::MAX)
            .await
            .expect("json body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(body["status"], "degraded", "{body}");
        assert_eq!(
            body["broken_members"],
            serde_json::json!([{
                "member_id": "personal:alice",
                "error": "restored member cannot publish its comms endpoint",
            }]),
            "{body}"
        );
    }

    /// An identity-first runtime over the in-memory test client with one
    /// roster identity, `lead`, and the roster it reads.
    async fn identity_event_fixture(
        mob_id: &str,
    ) -> (
        crate::unified_runtime::UnifiedRuntime,
        Arc<MutableRosterProvider>,
        AgentIdentity,
        tempfile::TempDir,
    ) {
        let definition = MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#
        ))
        .expect("definition parses");
        let identity = AgentIdentity::parse("lead").expect("identity parses");
        let roster = Arc::new(MutableRosterProvider::new(vec![DurableAgentSpec {
            identity: identity.clone(),
            profile: ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
            backend: None,
            binding: None,
            placement: None,
        }]));
        let scratch = tempfile::tempdir().expect("scratch dir");
        let runtime = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .continuity_store(Arc::new(
                LocalContinuityStore::in_memory().expect("continuity store"),
            ))
            .lease_provider(Arc::new(LocalLeaseProvider::new()))
            .roster_provider(Arc::<MutableRosterProvider>::clone(&roster))
            .scratch_dir(scratch.path())
            .ephemeral_runtime_store(true)
            .identity_runtime_instance_id(mob_id)
            .default_llm_client(Arc::new(TestClient::default()))
            .build()
            .await
            .expect("identity-first runtime builds");
        (runtime, roster, identity, scratch)
    }

    /// The gated per-agent stream over a lower plane that never ends, opened
    /// on the session Core binds `identity` to now: only the gate can end it.
    async fn session_gated_stream(
        runtime: &crate::unified_runtime::UnifiedRuntime,
        identity: &AgentIdentity,
    ) -> EventStream {
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime installed");
        let session = identity_runtime
            .status(identity)
            .await
            .expect("identity status")
            .session_id
            .expect("the live identity has a session");
        let identity_events = identity_runtime
            .subscribe(identity)
            .await
            .expect("identity event subscription");
        // A pending lower-plane stream proves termination comes from the
        // session gate, not from old-member channel teardown.
        let lower_plane: EventStream = Box::pin(futures::stream::pending());
        session_authoritative_agent_event_stream(
            lower_plane,
            identity_events,
            identity_runtime.clone(),
            identity.clone(),
            session,
        )
    }

    #[tokio::test]
    async fn durable_agent_event_stream_terminates_when_reset_replaces_the_session() {
        let (runtime, _roster, identity, _scratch) =
            identity_event_fixture("session-authoritative-agent-events").await;
        let mut guarded = session_gated_stream(&runtime, &identity).await;

        let replacement = runtime
            .identity_runtime()
            .expect("identity runtime installed")
            .reset(&identity)
            .await
            .expect("identity reset succeeds");
        assert_eq!(replacement.generation().get(), 1);
        let next = tokio::time::timeout(Duration::from_secs(5), guarded.next())
            .await
            .expect("session gate terminates promptly");
        assert!(
            next.is_none(),
            "stream opened on the prior session must terminate after reset"
        );

        let _ = runtime.mob_handle().stop().await;
    }

    /// A retire ends nothing: the identity keeps its session, which its next
    /// wake resumes. The gate keeps the stream open past two backstop checks
    /// and the retire's own lifecycle event; a reset then ends it.
    #[tokio::test]
    async fn durable_agent_event_stream_survives_a_retire_of_its_session() {
        let (runtime, _roster, identity, _scratch) =
            identity_event_fixture("session-gate-survives-retire").await;
        let mut guarded = session_gated_stream(&runtime, &identity).await;
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime installed");

        identity_runtime
            .retire(&identity)
            .await
            .expect("identity retire succeeds");
        assert!(
            tokio::time::timeout(Duration::from_millis(2500), guarded.next())
                .await
                .is_err(),
            "a retire keeps the session, so the stream stays open"
        );

        identity_runtime
            .reset(&identity)
            .await
            .expect("identity reset succeeds");
        let next = tokio::time::timeout(Duration::from_secs(5), guarded.next())
            .await
            .expect("session gate terminates promptly");
        assert!(
            next.is_none(),
            "a reset moves the identity to a fresh session and ends the stream"
        );

        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test]
    async fn durable_agent_event_stream_terminates_when_the_roster_drops_the_identity() {
        let (runtime, roster, identity, _scratch) =
            identity_event_fixture("session-gate-roster-removal").await;
        let mut guarded = session_gated_stream(&runtime, &identity).await;

        assert!(roster.remove(&identity), "the roster listed the identity");
        runtime
            .refresh_desired_topology()
            .await
            .expect("roster compile succeeds");
        let next = tokio::time::timeout(Duration::from_secs(5), guarded.next())
            .await
            .expect("session gate terminates promptly");
        assert!(
            next.is_none(),
            "an identity the roster dropped ends its subscriptions"
        );

        let _ = runtime.mob_handle().stop().await;
    }

    /// Identity-first materialization creates no member under a runtime-plane
    /// identity: the console treats `_system` frames as the runtime's own. The
    /// refusal is typed on every attempt, and a later materialization builds
    /// nothing.
    #[tokio::test]
    async fn identity_first_materialization_refuses_runtime_plane_identities() {
        let definition = MobDefinition::from_toml(
            r#"
[mob]
id = "runtime-plane-identity-first"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#,
        )
        .expect("definition parses");
        let identity = AgentIdentity::parse("_system").expect("identity parses");
        let roster = Arc::new(MutableRosterProvider::new(vec![DurableAgentSpec {
            identity: identity.clone(),
            profile: ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
            backend: None,
            binding: None,
            placement: None,
        }]));
        let scratch = tempfile::tempdir().expect("scratch dir");
        let built = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .continuity_store(Arc::new(
                LocalContinuityStore::in_memory().expect("continuity store"),
            ))
            .lease_provider(Arc::new(LocalLeaseProvider::new()))
            .roster_provider(roster)
            .scratch_dir(scratch.path())
            .ephemeral_runtime_store(true)
            .identity_runtime_instance_id("runtime-plane-identity-first")
            .default_llm_client(Arc::new(TestClient::default()))
            .build()
            .await;
        let runtime = built.expect("the refusal parks one identity; the runtime comes up");
        assert!(
            runtime
                .mob_handle()
                .list_members()
                .await
                .iter()
                .all(|member| member.agent_identity.as_str() != "_system"),
            "no member took `_system`"
        );

        // The refusal surfaces typed: the identity's bootstrap entry is
        // Broken with a reason that names the reservation.
        let identities = runtime
            .identity_runtime_ref()
            .expect("identity-first runtime")
            .clone();
        let bootstrap = identities.identity_bootstrap_status();
        let entry = bootstrap
            .identities
            .get(&identity)
            .expect("the identity is tracked");
        assert_eq!(
            entry.state,
            crate::identity_first::IdentityBootstrapState::Broken,
            "{entry:?}"
        );
        assert!(
            entry
                .error
                .as_deref()
                .is_some_and(|error| error.contains("reserved")),
            "{entry:?}"
        );

        // No build is retried: another materialization is refused typed with
        // the reservation before anything is built, and creates no member.
        let again = identities.materialize(&identity).await;
        assert!(
            matches!(
                &again,
                Err(crate::identity_first::IdentityRuntimeError::HostRejectedBuild {
                    reason, ..
                }) if reason.contains("reserved")
            ),
            "the reserved identity is refused typed: {again:?}"
        );
        assert!(
            runtime
                .mob_handle()
                .list_members()
                .await
                .iter()
                .all(|member| member.agent_identity.as_str() != "_system"),
            "a retry creates no `_system` member"
        );
        let _ = runtime.mob_handle().stop().await;
    }

    /// The per-agent event stream answers member_not_found for a runtime alias
    /// that is not the identity's current incarnation, as for any member it
    /// cannot find, rather than an internal error.
    #[tokio::test]
    async fn a_stale_runtime_alias_has_no_member_event_stream() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let definition = MobDefinition::from_toml(
            r#"
[mob]
id = "stale-alias-agent-events"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#,
        )
        .expect("definition parses");
        let identity = AgentIdentity::parse("lead").expect("identity parses");
        let roster = Arc::new(MutableRosterProvider::new(vec![DurableAgentSpec {
            identity: identity.clone(),
            profile: ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
            backend: None,
            binding: None,
            placement: None,
        }]));
        let scratch = tempfile::tempdir().expect("scratch dir");
        let runtime = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .continuity_store(Arc::new(
                LocalContinuityStore::in_memory().expect("continuity store"),
            ))
            .lease_provider(Arc::new(LocalLeaseProvider::new()))
            .roster_provider(roster)
            .scratch_dir(scratch.path())
            .ephemeral_runtime_store(true)
            .identity_runtime_instance_id("stale-alias-agent-events")
            .default_llm_client(Arc::new(TestClient::default()))
            .build()
            .await
            .expect("identity-first runtime builds");
        let current = runtime
            .identity_runtime()
            .expect("identity runtime installed")
            .status(&identity)
            .await
            .expect("identity active")
            .agent_runtime_id
            .expect("active runtime alias")
            .as_str()
            .to_string();
        let decisions = crate::runtime::RuntimeDecisionState::local_console(
            crate::ConsolePolicy {
                require_app_auth: false,
                ..Default::default()
            },
            None,
        );
        let app = runtime.build_reference_app_router(decisions);
        let status = async |agent_id: &str| {
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/agents/{agent_id}/events"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response")
                .status()
        };

        assert_eq!(
            (
                status(&current).await,
                status("rt:lead:7").await,
                status("rt:nobody:0").await,
            ),
            (StatusCode::OK, StatusCode::NOT_FOUND, StatusCode::NOT_FOUND),
            "(current alias {current}, stale alias, alias of no identity)"
        );
        let _ = runtime.mob_handle().stop().await;
    }
}
