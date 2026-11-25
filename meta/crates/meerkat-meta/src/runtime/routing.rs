//! Routing subsystem — destination resolution and module-backed routing dispatch.

use super::module_boundary::{
    CORE_MODULE_MCP_TIMEOUT, ROUTER_RESOLVE_MCP_TOOL, call_module_mcp_tool_json,
    call_module_mcp_tool_text, mcp_required_error, module_uses_mcp,
};
use super::*;

fn require_non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// A route's key in the module runtime's route map: its mob (none for the
/// host's default mob) and its route key. Two mobs' routes of one route key
/// are two routes.
fn route_map_key(mob_id: Option<&str>, route_key: &str) -> (Option<String>, String) {
    (mob_id.and_then(require_non_empty), route_key.to_string())
}

const MCP_REQUIRED_CORE_MODULES: [&str; 3] = ["router", "delivery", "memory"];
pub const WILDCARD_ROUTE: &str = "*";

fn core_module_requires_mcp(module_id: &str) -> bool {
    MCP_REQUIRED_CORE_MODULES.contains(&module_id)
}

pub fn route_module_call(
    runtime: &MetaRuntimeHandle,
    request: &ModuleRouteRequest,
    timeout: Duration,
) -> Result<ModuleRouteResponse, ModuleRouteError> {
    if !runtime.loaded_modules.contains(&request.module_id) {
        return Err(ModuleRouteError::UnloadedModule(request.module_id.clone()));
    }

    let module = runtime
        .config
        .modules
        .iter()
        .find(|module| module.id == request.module_id)
        .ok_or_else(|| ModuleRouteError::UnloadedModule(request.module_id.clone()))?;
    let pre_spawn = runtime
        .config
        .pre_spawn
        .iter()
        .find(|data| data.module_id == request.module_id);
    let uses_mcp = module_uses_mcp(module, pre_spawn);

    if core_module_requires_mcp(module.id.as_str()) && !uses_mcp {
        return Err(ModuleRouteError::ModuleRuntime(mcp_required_error(
            &request.module_id,
            &request.method,
        )));
    }

    if uses_mcp {
        let response =
            call_module_mcp_tool_text(module, pre_spawn, &request.method, &request.params, timeout)
                .map_err(ModuleRouteError::ModuleRuntime)?;
        let payload = serde_json::from_str::<Value>(&response).unwrap_or(Value::String(response));
        return Ok(ModuleRouteResponse {
            module_id: request.module_id.clone(),
            method: request.method.clone(),
            payload,
        });
    }

    let envelope = run_module_boundary_once(module, pre_spawn, timeout)
        .map_err(ModuleRouteError::ModuleRuntime)?;

    match envelope.event {
        UnifiedEvent::Module(event) if event.module == request.module_id => {
            Ok(ModuleRouteResponse {
                module_id: request.module_id.clone(),
                method: request.method.clone(),
                payload: event.payload,
            })
        }
        _ => Err(ModuleRouteError::UnexpectedRouteResponse),
    }
}

impl MetaRuntimeHandle {
    fn parse_router_payload_overrides(
        payload: &serde_json::Map<String, Value>,
    ) -> RouterBoundaryOverrides {
        let mut overrides = RouterBoundaryOverrides::default();
        if let Some(channel) = payload.get("channel").and_then(Value::as_str) {
            let channel = channel.trim();
            if !channel.is_empty() {
                overrides.channel = Some(channel.to_string());
            }
        }
        if let Some(sink) = payload.get("sink").and_then(Value::as_str) {
            let sink = sink.trim();
            if !sink.is_empty() {
                overrides.sink = Some(sink.to_string());
            }
        }
        if let Some(target_module) = payload.get("target_module").and_then(Value::as_str) {
            let target_module = target_module.trim();
            if !target_module.is_empty() {
                overrides.target_module = Some(target_module.to_string());
            }
        }
        if let Some(retry_max) = payload
            .get("retry_max")
            .and_then(Value::as_u64)
            .and_then(|raw| u32::try_from(raw).ok())
        {
            overrides.retry_max = Some(retry_max);
        }
        if let Some(backoff_ms) = payload.get("backoff_ms").and_then(Value::as_u64) {
            overrides.backoff_ms = Some(backoff_ms);
        }
        if let Some(rate_limit_per_minute) = payload
            .get("rate_limit_per_minute")
            .and_then(Value::as_u64)
            .and_then(|raw| u32::try_from(raw).ok())
        {
            overrides.rate_limit_per_minute = Some(rate_limit_per_minute);
        }
        overrides
    }

    fn remember_routing_resolution(&mut self, resolution: RoutingResolution) {
        let route_id = resolution.route_id.clone();
        self.routing_resolutions
            .insert(route_id.clone(), resolution);
        self.routing_resolution_order.push(route_id);
        while self.routing_resolution_order.len() > ROUTING_RESOLUTION_LIMIT_MAX {
            let oldest_route_id = self.routing_resolution_order.remove(0);
            self.routing_resolutions.remove(&oldest_route_id);
        }
    }
    fn next_routing_sequence(&mut self) -> u64 {
        Self::next_sequence(&mut self.routing_sequence)
    }
    fn parse_router_mcp_overrides(response: &Value) -> RouterBoundaryOverrides {
        let Some(payload) = response.as_object() else {
            return RouterBoundaryOverrides::default();
        };
        Self::parse_router_payload_overrides(payload)
    }
    /// The routes of the mob `mob_id` names (none for the default mob).
    fn mob_routes<'a>(
        &'a self,
        mob_id: Option<&str>,
    ) -> impl Iterator<Item = &'a RuntimeRoute> + Clone + use<'a> {
        let mob_id = mob_id.map(meerkat_mob::MobId::from);
        self.runtime_routes
            .values()
            .filter(move |route| route.mob_id == mob_id)
    }

    fn matching_runtime_route(
        &self,
        mob_id: Option<&str>,
        recipient: &str,
        channel: &str,
    ) -> Option<&RuntimeRoute> {
        let routes = self.mob_routes(mob_id);
        // Priority 1: Exact recipient + exact channel
        let exact = routes.clone().find(|route| {
            route.recipient != WILDCARD_ROUTE
                && route.recipient == recipient
                && route
                    .channel
                    .as_deref()
                    .is_none_or(|c| c != WILDCARD_ROUTE && c == channel)
        });
        if exact.is_some() {
            return exact;
        }
        // Priority 2: Exact recipient + wildcard channel
        let exact_recip_wild_chan = routes.clone().find(|route| {
            route.recipient != WILDCARD_ROUTE
                && route.recipient == recipient
                && route.channel.as_deref() == Some(WILDCARD_ROUTE)
        });
        if exact_recip_wild_chan.is_some() {
            return exact_recip_wild_chan;
        }
        // Priority 3: Wildcard recipient + exact channel
        let wild_recip_exact_chan = routes.clone().find(|route| {
            route.recipient == WILDCARD_ROUTE
                && route
                    .channel
                    .as_deref()
                    .is_none_or(|c| c != WILDCARD_ROUTE && c == channel)
        });
        if wild_recip_exact_chan.is_some() {
            return wild_recip_exact_chan;
        }
        // Priority 4: Wildcard recipient + wildcard channel
        routes.clone().find(|route| {
            route.recipient == WILDCARD_ROUTE && route.channel.as_deref() == Some(WILDCARD_ROUTE)
        })
    }

    pub fn list_runtime_routes(&self) -> Vec<RuntimeRoute> {
        self.runtime_routes.values().cloned().collect()
    }

    pub fn add_runtime_route(
        &mut self,
        route: RuntimeRoute,
    ) -> Result<RuntimeRoute, RuntimeRouteMutationError> {
        let route_key =
            require_non_empty(&route.route_key).ok_or(RuntimeRouteMutationError::EmptyRouteKey)?;
        // A blank mob names no mob. It is refused, never stored as the
        // default mob's route: the default mob is named by leaving the mob
        // out.
        if route
            .mob_id
            .as_ref()
            .is_some_and(|mob_id| !crate::member_key::names_a_mob(mob_id))
        {
            return Err(RuntimeRouteMutationError::EmptyMobId);
        }
        let recipient =
            require_non_empty(&route.recipient).ok_or(RuntimeRouteMutationError::EmptyRecipient)?;
        if route
            .channel
            .as_ref()
            .is_some_and(|channel| require_non_empty(channel).is_none())
        {
            return Err(RuntimeRouteMutationError::InvalidChannel);
        }
        let sink = require_non_empty(&route.sink).ok_or(RuntimeRouteMutationError::EmptySink)?;
        let target_module = require_non_empty(&route.target_module)
            .ok_or(RuntimeRouteMutationError::EmptyTargetModule)?;
        if route
            .retry_max
            .is_some_and(|retry_max| retry_max > ROUTING_RETRY_MAX_CAP)
        {
            return Err(RuntimeRouteMutationError::RetryMaxExceedsCap {
                provided: route.retry_max.unwrap_or_default(),
                cap: ROUTING_RETRY_MAX_CAP,
            });
        }
        if route.rate_limit_per_minute == Some(0) {
            return Err(RuntimeRouteMutationError::InvalidRateLimitPerMinute);
        }

        let canonical = RuntimeRoute {
            route_key,
            mob_id: route
                .mob_id
                .as_ref()
                .map(meerkat_mob::MobId::as_str)
                .and_then(require_non_empty)
                .map(meerkat_mob::MobId::from),
            recipient,
            channel: route.channel.as_deref().and_then(require_non_empty),
            sink,
            target_module,
            retry_max: route.retry_max,
            backoff_ms: route.backoff_ms,
            rate_limit_per_minute: route.rate_limit_per_minute,
        };
        self.runtime_routes.insert(
            route_map_key(
                canonical.mob_id.as_ref().map(meerkat_mob::MobId::as_str),
                &canonical.route_key,
            ),
            canonical.clone(),
        );
        Ok(canonical)
    }

    /// Delete the default mob's route `route_key`.
    pub fn delete_runtime_route(
        &mut self,
        route_key: &str,
    ) -> Result<RuntimeRoute, RuntimeRouteMutationError> {
        self.delete_mob_runtime_route(None, route_key)
    }

    /// Delete the route `route_key` of the mob `mob_id` names (none for the
    /// host's default mob). A route of the same key in another mob stays. A
    /// blank mob names no mob and deletes nothing: it is refused, never
    /// read as the default mob.
    pub fn delete_mob_runtime_route(
        &mut self,
        mob_id: Option<&str>,
        route_key: &str,
    ) -> Result<RuntimeRoute, RuntimeRouteMutationError> {
        let route_key = route_key.trim();
        if route_key.is_empty() {
            return Err(RuntimeRouteMutationError::EmptyRouteKey);
        }
        if mob_id.is_some_and(|mob_id| {
            !crate::member_key::names_a_mob(&meerkat_mob::MobId::from(mob_id))
        }) {
            return Err(RuntimeRouteMutationError::EmptyMobId);
        }
        self.runtime_routes
            .remove(&route_map_key(mob_id, route_key))
            .ok_or_else(|| RuntimeRouteMutationError::RouteNotFound(route_key.to_string()))
    }

    pub fn resolve_routing(
        &mut self,
        request: RoutingResolveRequest,
    ) -> Result<RoutingResolution, RoutingResolveError> {
        if !self.is_module_loaded("router") {
            return Err(RoutingResolveError::RouterModuleNotLoaded);
        }
        if !self.is_module_loaded("delivery") {
            return Err(RoutingResolveError::DeliveryModuleNotLoaded);
        }

        let recipient =
            require_non_empty(&request.recipient).ok_or(RoutingResolveError::EmptyRecipient)?;
        // A blank mob names no mob and is never read as the default mob:
        // it is kept as it is and matches no route of any mob.
        let mob_id = request
            .mob_id
            .as_ref()
            .map(|mob_id| mob_id.as_str().trim().to_string());
        let request_value = serde_json::to_value(&request).unwrap_or(Value::Null);

        let channel = require_non_empty(
            &request
                .channel
                .unwrap_or_else(|| "notification".to_string()),
        )
        .ok_or(RoutingResolveError::InvalidChannel)?;
        let mut retry_max = request.retry_max.unwrap_or(1);
        if retry_max > ROUTING_RETRY_MAX_CAP {
            return Err(RoutingResolveError::RetryMaxExceedsCap {
                provided: retry_max,
                cap: ROUTING_RETRY_MAX_CAP,
            });
        }
        let mut rate_limit_per_minute = request.rate_limit_per_minute.unwrap_or(2);
        if rate_limit_per_minute == 0 {
            return Err(RoutingResolveError::InvalidRateLimitPerMinute);
        }
        let mut resolved_channel = channel;
        let mut backoff_ms = request.backoff_ms.unwrap_or(250);

        let mut sink = if recipient.contains('@') {
            "email"
        } else if recipient.starts_with('+') {
            "sms"
        } else {
            "webhook"
        }
        .to_string();
        let mut target_module = "delivery".to_string();

        let Some((router_module, pre_spawn)) = self.module_and_prespawn("router") else {
            return Err(RoutingResolveError::RouterBoundary(mcp_required_error(
                "router",
                ROUTER_RESOLVE_MCP_TOOL,
            )));
        };
        if !module_uses_mcp(router_module, pre_spawn) {
            return Err(RoutingResolveError::RouterBoundary(mcp_required_error(
                "router",
                ROUTER_RESOLVE_MCP_TOOL,
            )));
        }

        let mcp_response = call_module_mcp_tool_json(
            router_module,
            pre_spawn,
            ROUTER_RESOLVE_MCP_TOOL,
            &request_value,
            CORE_MODULE_MCP_TIMEOUT,
        )
        .map_err(RoutingResolveError::RouterBoundary)?;
        let overrides = Self::parse_router_mcp_overrides(&mcp_response);

        if let Some(override_channel) = overrides.channel {
            resolved_channel = override_channel;
        }
        if let Some(override_sink) = overrides.sink {
            sink = override_sink;
        }
        if let Some(override_target_module) = overrides.target_module {
            target_module = override_target_module;
        }
        if let Some(override_retry_max) = overrides.retry_max {
            retry_max = override_retry_max;
        }
        if let Some(override_backoff_ms) = overrides.backoff_ms {
            backoff_ms = override_backoff_ms;
        }
        if let Some(override_rate_limit) = overrides.rate_limit_per_minute {
            rate_limit_per_minute = override_rate_limit;
        }
        if retry_max > ROUTING_RETRY_MAX_CAP {
            return Err(RoutingResolveError::RetryMaxExceedsCap {
                provided: retry_max,
                cap: ROUTING_RETRY_MAX_CAP,
            });
        }
        if rate_limit_per_minute == 0 {
            return Err(RoutingResolveError::InvalidRateLimitPerMinute);
        }
        if let Some(route_override) =
            self.matching_runtime_route(mob_id.as_deref(), &recipient, &resolved_channel)
        {
            sink = route_override.sink.clone();
            target_module = route_override.target_module.clone();
            retry_max = route_override.retry_max.unwrap_or(retry_max);
            backoff_ms = route_override.backoff_ms.unwrap_or(backoff_ms);
            rate_limit_per_minute = route_override
                .rate_limit_per_minute
                .unwrap_or(rate_limit_per_minute);
        }
        if retry_max > ROUTING_RETRY_MAX_CAP {
            return Err(RoutingResolveError::RetryMaxExceedsCap {
                provided: retry_max,
                cap: ROUTING_RETRY_MAX_CAP,
            });
        }
        if rate_limit_per_minute == 0 {
            return Err(RoutingResolveError::InvalidRateLimitPerMinute);
        }

        let route_sequence = self.next_routing_sequence();
        let route_id = format!("route-{route_sequence:06}");
        let resolution = RoutingResolution {
            route_id,
            // The resolution names the mob whose routes it was resolved
            // over, so that a send can tell whose it is.
            mob_id: mob_id.as_deref().map(meerkat_mob::MobId::from),
            recipient,
            channel: resolved_channel,
            sink,
            target_module,
            retry_max,
            backoff_ms,
            rate_limit_per_minute,
        };
        self.remember_routing_resolution(resolution.clone());
        let event_id = format!("evt-routing-{route_sequence:06}");
        let resolved_timestamp_ms = self.next_route_resolved_timestamp_ms();
        insert_event_sorted(
            &mut self.merged_events,
            EventEnvelope {
                event_id,
                source: "module".to_string(),
                timestamp_ms: resolved_timestamp_ms,
                event: UnifiedEvent::Module(ModuleEvent {
                    module: "router".to_string(),
                    event_type: "resolved".to_string(),
                    payload: serde_json::to_value(&resolution).unwrap_or(Value::Null),
                }),
            },
        );

        Ok(resolution)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod member_keyed_route_tests {
    use super::*;

    fn owner() -> MetaRuntimeHandle {
        crate::start_meta_runtime(
            crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: "member-keyed-routes".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            vec![],
            Duration::from_secs(1),
        )
        .expect("runtime starts")
    }

    fn route(mob_id: Option<&str>, recipient: &str, sink: &str) -> RuntimeRoute {
        RuntimeRoute {
            route_key: format!("roster:{recipient}"),
            mob_id: mob_id.map(meerkat_mob::MobId::from),
            recipient: recipient.to_string(),
            channel: Some("notification".to_string()),
            sink: sink.to_string(),
            target_module: "delivery".to_string(),
            retry_max: None,
            backoff_ms: None,
            rate_limit_per_minute: None,
        }
    }

    /// Two mobs of one host each have a member named `alice`, and each has
    /// a route for her under the same route key. The routes are two: a
    /// resolve in one mob takes that mob's route, a wildcard route of one
    /// mob applies to no other, and deleting one mob's route leaves the
    /// other's.
    #[test]
    fn member_keyed_routes_two_mobs_recipients_of_one_name_keep_their_own_routes() {
        let mut owner = owner();
        owner
            .add_runtime_route(route(None, "alice", "launch-sink"))
            .expect("default mob's route");
        owner
            .add_runtime_route(route(Some("second"), "alice", "second-sink"))
            .expect("second mob's route");
        owner
            .add_runtime_route(route(Some("second"), WILDCARD_ROUTE, "second-any"))
            .expect("second mob's wildcard route");
        assert_eq!(owner.list_runtime_routes().len(), 3, "same key, two mobs");
        let sink = |owner: &MetaRuntimeHandle, mob_id: Option<&str>, recipient: &str| {
            owner
                .matching_runtime_route(mob_id, recipient, "notification")
                .map(|route| route.sink.clone())
        };
        assert_eq!(sink(&owner, None, "alice").as_deref(), Some("launch-sink"));
        assert_eq!(
            sink(&owner, Some("second"), "alice").as_deref(),
            Some("second-sink")
        );
        assert_eq!(sink(&owner, Some("third"), "alice"), None);
        assert_eq!(
            sink(&owner, Some("second"), "bob").as_deref(),
            Some("second-any")
        );
        assert_eq!(
            sink(&owner, None, "bob"),
            None,
            "a wildcard route of the second mob is not the default mob's"
        );

        owner
            .delete_mob_runtime_route(Some("second"), "roster:alice")
            .expect("the second mob's route goes");
        assert_eq!(sink(&owner, None, "alice").as_deref(), Some("launch-sink"));
        assert_eq!(
            sink(&owner, Some("second"), "alice").as_deref(),
            Some("second-any")
        );
        owner
            .delete_runtime_route("roster:alice")
            .expect("the default mob's route goes");
        assert_eq!(sink(&owner, None, "alice"), None);
    }

    /// A route that names a blank mob (an empty id, or only white space)
    /// names no mob, and is never the default mob's: adding it is refused
    /// and stores nothing, deleting by it deletes no route of the default
    /// mob, and matching by it reads no route of any mob. The default mob
    /// is named by leaving the mob out.
    #[test]
    fn member_keyed_routes_a_blank_mob_is_refused_on_a_write_and_never_the_default_mobs() {
        for blank in ["", "  ", "\t"] {
            let mut owner = owner();
            let added = owner.add_runtime_route(route(Some(blank), "alice", "blank-sink"));
            assert!(added.is_err(), "{blank:?}: added {added:?}");
            assert!(
                owner.list_runtime_routes().is_empty(),
                "{blank:?}: a route was stored: {:?}",
                owner.list_runtime_routes()
            );

            owner
                .add_runtime_route(route(None, "alice", "launch-sink"))
                .expect("default mob's route");
            let deleted = owner.delete_mob_runtime_route(Some(blank), "roster:alice");
            assert!(deleted.is_err(), "{blank:?}: deleted {deleted:?}");
            assert_eq!(
                owner.list_runtime_routes().len(),
                1,
                "{blank:?}: the default mob's route was deleted"
            );
            assert!(
                owner
                    .matching_runtime_route(Some(blank), "alice", "notification")
                    .is_none(),
                "{blank:?}: a blank mob matched the default mob's route"
            );
        }
    }

    /// A route that names no mob is the default mob's, and is written as it
    /// always was.
    #[test]
    fn member_keyed_routes_a_route_without_a_mob_is_the_default_mobs() {
        let stored = serde_json::json!({
            "route_key": "roster:alice",
            "recipient": "alice",
            "channel": "notification",
            "sink": "launch-sink",
            "target_module": "delivery",
            "retry_max": null,
            "backoff_ms": null,
            "rate_limit_per_minute": null,
        });
        let parsed: RuntimeRoute = serde_json::from_value(stored.clone()).expect("route reads");
        assert_eq!(parsed.mob_id, None);
        let mut owner = owner();
        let added = owner.add_runtime_route(parsed).expect("route");
        assert_eq!(serde_json::to_value(&added).expect("route writes"), stored);
        assert!(
            owner
                .matching_runtime_route(None, "alice", "notification")
                .is_some()
        );
        assert!(
            owner
                .matching_runtime_route(Some("second"), "alice", "notification")
                .is_none()
        );
        let request = RoutingResolveRequest {
            mob_id: None,
            recipient: "alice".to_string(),
            channel: None,
            retry_max: None,
            backoff_ms: None,
            rate_limit_per_minute: None,
        };
        assert!(
            !serde_json::to_string(&request)
                .expect("request writes")
                .contains("mob_id"),
            "a resolve in the default mob reaches the router module as it always did"
        );
    }
}
