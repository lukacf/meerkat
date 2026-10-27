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
    fn matching_runtime_route(&self, recipient: &str, channel: &str) -> Option<&RuntimeRoute> {
        // Priority 1: Exact recipient + exact channel
        let exact = self.runtime_routes.values().find(|route| {
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
        let exact_recip_wild_chan = self.runtime_routes.values().find(|route| {
            route.recipient != WILDCARD_ROUTE
                && route.recipient == recipient
                && route.channel.as_deref() == Some(WILDCARD_ROUTE)
        });
        if exact_recip_wild_chan.is_some() {
            return exact_recip_wild_chan;
        }
        // Priority 3: Wildcard recipient + exact channel
        let wild_recip_exact_chan = self.runtime_routes.values().find(|route| {
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
        self.runtime_routes.values().find(|route| {
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
            recipient,
            channel: route.channel.as_deref().and_then(require_non_empty),
            sink,
            target_module,
            retry_max: route.retry_max,
            backoff_ms: route.backoff_ms,
            rate_limit_per_minute: route.rate_limit_per_minute,
        };
        self.runtime_routes
            .insert(canonical.route_key.clone(), canonical.clone());
        Ok(canonical)
    }

    pub fn delete_runtime_route(
        &mut self,
        route_key: &str,
    ) -> Result<RuntimeRoute, RuntimeRouteMutationError> {
        let route_key = route_key.trim();
        if route_key.is_empty() {
            return Err(RuntimeRouteMutationError::EmptyRouteKey);
        }
        self.runtime_routes
            .remove(route_key)
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
        if let Some(route_override) = self.matching_runtime_route(&recipient, &resolved_channel) {
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
