//! Parameter parsing for routing and delivery RPC methods.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RoutingDeliveryParamsError {
    ParamsMustBeObject,
    RouteFieldMustBeObject,
    RouteKeyRequired,
    RecipientRequired,
    ChannelMustBeString,
    SinkMustBeString,
    TargetModuleMustBeString,
    RetryMaxMustBeInteger,
    RetryMaxOverflow,
    RetryMaxAboveCap { cap: u32 },
    BackoffMsMustBeInteger,
    RateLimitMustBeInteger,
    RateLimitMustBePositive,
    RateLimitOverflow,
    ResolutionRequired,
    PayloadRequired,
    IdempotencyKeyMustBeString,
    HistoryRecipientMustBeString,
    HistorySinkMustBeString,
    HistoryLimitOutOfRange,
    Routing(RoutingResolveError),
    Delivery(DeliverySendError),
    RouteMutation(RuntimeRouteMutationError),
}

impl RoutingDeliveryParamsError {
    pub(super) fn message(&self) -> String {
        match self {
            RoutingDeliveryParamsError::ParamsMustBeObject => {
                "params must be a JSON object".to_string()
            }
            RoutingDeliveryParamsError::RouteFieldMustBeObject => {
                "route must be an object".to_string()
            }
            RoutingDeliveryParamsError::RouteKeyRequired => {
                "route_key must be a non-empty string".to_string()
            }
            RoutingDeliveryParamsError::RecipientRequired => {
                "recipient must be a non-empty string".to_string()
            }
            RoutingDeliveryParamsError::ChannelMustBeString => {
                "channel must be a string".to_string()
            }
            RoutingDeliveryParamsError::SinkMustBeString => "sink must be a string".to_string(),
            RoutingDeliveryParamsError::TargetModuleMustBeString => {
                "target_module must be a string".to_string()
            }
            RoutingDeliveryParamsError::RetryMaxMustBeInteger => {
                "retry_max must be a non-negative integer".to_string()
            }
            RoutingDeliveryParamsError::RetryMaxOverflow => {
                "retry_max exceeds maximum supported integer range".to_string()
            }
            RoutingDeliveryParamsError::RetryMaxAboveCap { cap } => {
                format!("retry_max must be <= {cap}")
            }
            RoutingDeliveryParamsError::BackoffMsMustBeInteger => {
                "backoff_ms must be a non-negative integer".to_string()
            }
            RoutingDeliveryParamsError::RateLimitMustBeInteger => {
                "rate_limit_per_minute must be a non-negative integer".to_string()
            }
            RoutingDeliveryParamsError::RateLimitMustBePositive => {
                "rate_limit_per_minute must be greater than 0".to_string()
            }
            RoutingDeliveryParamsError::RateLimitOverflow => {
                "rate_limit_per_minute exceeds maximum supported integer range".to_string()
            }
            RoutingDeliveryParamsError::ResolutionRequired => {
                "resolution must be an object".to_string()
            }
            RoutingDeliveryParamsError::PayloadRequired => "payload is required".to_string(),
            RoutingDeliveryParamsError::IdempotencyKeyMustBeString => {
                "idempotency_key must be a non-empty string".to_string()
            }
            RoutingDeliveryParamsError::HistoryRecipientMustBeString => {
                "recipient filter must be a string".to_string()
            }
            RoutingDeliveryParamsError::HistorySinkMustBeString => {
                "sink filter must be a string".to_string()
            }
            RoutingDeliveryParamsError::HistoryLimitOutOfRange => {
                "limit must be an integer between 1 and 200".to_string()
            }
            RoutingDeliveryParamsError::Routing(RoutingResolveError::RouterModuleNotLoaded) => {
                "router module is not loaded".to_string()
            }
            RoutingDeliveryParamsError::Routing(RoutingResolveError::DeliveryModuleNotLoaded) => {
                "delivery module is not loaded".to_string()
            }
            RoutingDeliveryParamsError::Routing(RoutingResolveError::EmptyRecipient) => {
                "recipient must be a non-empty string".to_string()
            }
            RoutingDeliveryParamsError::Routing(RoutingResolveError::InvalidChannel) => {
                "channel must be a non-empty string".to_string()
            }
            RoutingDeliveryParamsError::Routing(RoutingResolveError::InvalidRateLimitPerMinute) => {
                "rate_limit_per_minute must be greater than 0".to_string()
            }
            RoutingDeliveryParamsError::Routing(RoutingResolveError::RetryMaxExceedsCap {
                cap,
                ..
            }) => {
                format!("retry_max must be <= {cap}")
            }
            RoutingDeliveryParamsError::Routing(RoutingResolveError::RouterBoundary(err)) => {
                format!("router boundary failed: {err:?}")
            }
            RoutingDeliveryParamsError::Delivery(DeliverySendError::DeliveryModuleNotLoaded) => {
                "delivery module is not loaded".to_string()
            }
            RoutingDeliveryParamsError::Delivery(DeliverySendError::InvalidRouteTarget(target)) => {
                format!("resolution.target_module must be 'delivery' (got '{target}')")
            }
            RoutingDeliveryParamsError::Delivery(DeliverySendError::InvalidRouteId) => {
                "resolution.route_id must be a non-empty string".to_string()
            }
            RoutingDeliveryParamsError::Delivery(DeliverySendError::UnknownRouteId(route_id)) => {
                format!("resolution.route_id '{route_id}' was not issued by routing/resolve")
            }
            RoutingDeliveryParamsError::Delivery(DeliverySendError::ForgedResolution) => {
                "resolution does not match the trusted route for route_id".to_string()
            }
            RoutingDeliveryParamsError::Delivery(DeliverySendError::InvalidRecipient) => {
                "resolution.recipient must be a non-empty string".to_string()
            }
            RoutingDeliveryParamsError::Delivery(DeliverySendError::InvalidSink) => {
                "resolution.sink must be a non-empty string".to_string()
            }
            RoutingDeliveryParamsError::Delivery(DeliverySendError::InvalidIdempotencyKey) => {
                "idempotency_key must be a non-empty string".to_string()
            }
            RoutingDeliveryParamsError::Delivery(DeliverySendError::IdempotencyPayloadMismatch) => {
                "idempotency_key replay payload does not match original request".to_string()
            }
            RoutingDeliveryParamsError::Delivery(DeliverySendError::RateLimited {
                sink,
                window_start_ms,
                limit,
            }) => format!(
                "rate limit exceeded for sink '{sink}' at window {window_start_ms} (limit={limit})"
            ),
            RoutingDeliveryParamsError::Delivery(DeliverySendError::DeliveryBoundary(err)) => {
                format!("delivery boundary failed: {err:?}")
            }
            RoutingDeliveryParamsError::RouteMutation(RuntimeRouteMutationError::EmptyRouteKey) => {
                "route_key must be a non-empty string".to_string()
            }
            RoutingDeliveryParamsError::RouteMutation(
                RuntimeRouteMutationError::EmptyRecipient,
            ) => "recipient must be a non-empty string".to_string(),
            RoutingDeliveryParamsError::RouteMutation(
                RuntimeRouteMutationError::InvalidChannel,
            ) => "channel must be a non-empty string when provided".to_string(),
            RoutingDeliveryParamsError::RouteMutation(RuntimeRouteMutationError::EmptySink) => {
                "sink must be a non-empty string".to_string()
            }
            RoutingDeliveryParamsError::RouteMutation(
                RuntimeRouteMutationError::EmptyTargetModule,
            ) => "target_module must be a non-empty string".to_string(),
            RoutingDeliveryParamsError::RouteMutation(
                RuntimeRouteMutationError::InvalidRateLimitPerMinute,
            ) => "rate_limit_per_minute must be greater than 0".to_string(),
            RoutingDeliveryParamsError::RouteMutation(
                RuntimeRouteMutationError::RetryMaxExceedsCap { cap, .. },
            ) => format!("retry_max must be <= {cap}"),
            RoutingDeliveryParamsError::RouteMutation(
                RuntimeRouteMutationError::RouteNotFound(route_key),
            ) => format!("route_key '{route_key}' was not found"),
        }
    }
}

pub(super) fn parse_routing_resolve_params(
    params: &Value,
) -> Result<RoutingResolveRequest, RoutingDeliveryParamsError> {
    let object = params
        .as_object()
        .ok_or(RoutingDeliveryParamsError::ParamsMustBeObject)?;
    let recipient = object
        .get("recipient")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(RoutingDeliveryParamsError::RecipientRequired)?;
    let channel = match object.get("channel") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(RoutingDeliveryParamsError::ChannelMustBeString)?
                .to_string(),
        ),
    };
    let retry_max = match object.get("retry_max") {
        None => None,
        Some(value) => {
            let raw = value
                .as_u64()
                .ok_or(RoutingDeliveryParamsError::RetryMaxMustBeInteger)?;
            let retry_max =
                u32::try_from(raw).map_err(|_| RoutingDeliveryParamsError::RetryMaxOverflow)?;
            if retry_max > ROUTING_RETRY_MAX_CAP {
                return Err(RoutingDeliveryParamsError::RetryMaxAboveCap {
                    cap: ROUTING_RETRY_MAX_CAP,
                });
            }
            Some(retry_max)
        }
    };
    let backoff_ms = match object.get("backoff_ms") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or(RoutingDeliveryParamsError::BackoffMsMustBeInteger)?,
        ),
    };
    let rate_limit_per_minute = match object.get("rate_limit_per_minute") {
        None => None,
        Some(value) => {
            let raw = value
                .as_u64()
                .ok_or(RoutingDeliveryParamsError::RateLimitMustBeInteger)?;
            let rate_limit_per_minute =
                u32::try_from(raw).map_err(|_| RoutingDeliveryParamsError::RateLimitOverflow)?;
            if rate_limit_per_minute == 0 {
                return Err(RoutingDeliveryParamsError::RateLimitMustBePositive);
            }
            Some(rate_limit_per_minute)
        }
    };

    Ok(RoutingResolveRequest {
        recipient: recipient.to_string(),
        channel,
        retry_max,
        backoff_ms,
        rate_limit_per_minute,
    })
}

pub(super) fn parse_routing_routes_list_params(
    params: &Value,
) -> Result<(), RoutingDeliveryParamsError> {
    if params.is_null() || params.is_object() {
        return Ok(());
    }
    Err(RoutingDeliveryParamsError::ParamsMustBeObject)
}

pub(super) fn parse_routing_route_add_params(
    params: &Value,
) -> Result<RuntimeRoute, RoutingDeliveryParamsError> {
    let object = params
        .as_object()
        .ok_or(RoutingDeliveryParamsError::ParamsMustBeObject)?;
    let route_value = object.get("route").unwrap_or(params);
    let route_object = route_value
        .as_object()
        .ok_or(RoutingDeliveryParamsError::RouteFieldMustBeObject)?;

    let route_key = route_object
        .get("route_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(RoutingDeliveryParamsError::RouteKeyRequired)?
        .to_string();
    let recipient = route_object
        .get("recipient")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(RoutingDeliveryParamsError::RecipientRequired)?
        .to_string();
    let channel = match route_object.get("channel") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(RoutingDeliveryParamsError::ChannelMustBeString)?
                .to_string(),
        ),
    };
    let sink = route_object
        .get("sink")
        .and_then(Value::as_str)
        .ok_or(RoutingDeliveryParamsError::SinkMustBeString)?
        .to_string();
    let target_module = route_object
        .get("target_module")
        .and_then(Value::as_str)
        .ok_or(RoutingDeliveryParamsError::TargetModuleMustBeString)?
        .to_string();
    let retry_max = match route_object.get("retry_max") {
        None => None,
        Some(value) => {
            let raw = value
                .as_u64()
                .ok_or(RoutingDeliveryParamsError::RetryMaxMustBeInteger)?;
            let retry_max =
                u32::try_from(raw).map_err(|_| RoutingDeliveryParamsError::RetryMaxOverflow)?;
            if retry_max > ROUTING_RETRY_MAX_CAP {
                return Err(RoutingDeliveryParamsError::RetryMaxAboveCap {
                    cap: ROUTING_RETRY_MAX_CAP,
                });
            }
            Some(retry_max)
        }
    };
    let backoff_ms = match route_object.get("backoff_ms") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or(RoutingDeliveryParamsError::BackoffMsMustBeInteger)?,
        ),
    };
    let rate_limit_per_minute = match route_object.get("rate_limit_per_minute") {
        None => None,
        Some(value) => {
            let raw = value
                .as_u64()
                .ok_or(RoutingDeliveryParamsError::RateLimitMustBeInteger)?;
            let rate_limit_per_minute =
                u32::try_from(raw).map_err(|_| RoutingDeliveryParamsError::RateLimitOverflow)?;
            if rate_limit_per_minute == 0 {
                return Err(RoutingDeliveryParamsError::RateLimitMustBePositive);
            }
            Some(rate_limit_per_minute)
        }
    };

    Ok(RuntimeRoute {
        route_key,
        recipient,
        channel,
        sink,
        target_module,
        retry_max,
        backoff_ms,
        rate_limit_per_minute,
    })
}

pub(super) fn parse_routing_route_delete_params(
    params: &Value,
) -> Result<String, RoutingDeliveryParamsError> {
    let object = params
        .as_object()
        .ok_or(RoutingDeliveryParamsError::ParamsMustBeObject)?;
    let route_key = object
        .get("route_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(RoutingDeliveryParamsError::RouteKeyRequired)?;
    Ok(route_key.to_string())
}

pub(super) fn parse_delivery_send_params(
    params: &Value,
) -> Result<DeliverySendRequest, RoutingDeliveryParamsError> {
    let object = params
        .as_object()
        .ok_or(RoutingDeliveryParamsError::ParamsMustBeObject)?;
    let resolution = object
        .get("resolution")
        .cloned()
        .ok_or(RoutingDeliveryParamsError::ResolutionRequired)?;
    let resolution = serde_json::from_value(resolution)
        .map_err(|_| RoutingDeliveryParamsError::ResolutionRequired)?;
    let payload = object
        .get("payload")
        .cloned()
        .ok_or(RoutingDeliveryParamsError::PayloadRequired)?;
    let idempotency_key = match object.get("idempotency_key") {
        None => None,
        Some(value) => {
            let key = value
                .as_str()
                .ok_or(RoutingDeliveryParamsError::IdempotencyKeyMustBeString)?
                .trim()
                .to_string();
            if key.is_empty() {
                return Err(RoutingDeliveryParamsError::IdempotencyKeyMustBeString);
            }
            Some(key)
        }
    };

    Ok(DeliverySendRequest {
        resolution,
        payload,
        idempotency_key,
    })
}

pub(super) fn parse_delivery_history_params(
    params: &Value,
) -> Result<DeliveryHistoryRequest, RoutingDeliveryParamsError> {
    if params.is_null() {
        return Ok(DeliveryHistoryRequest {
            recipient: None,
            sink: None,
            limit: 20,
        });
    }

    let object = params
        .as_object()
        .ok_or(RoutingDeliveryParamsError::ParamsMustBeObject)?;
    let recipient = match object.get("recipient") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(RoutingDeliveryParamsError::HistoryRecipientMustBeString)?
                .to_string(),
        ),
    };
    let sink = match object.get("sink") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(RoutingDeliveryParamsError::HistorySinkMustBeString)?
                .to_string(),
        ),
    };
    let limit = match object.get("limit") {
        None => 20,
        Some(value) => {
            let Some(limit) = value.as_u64() else {
                return Err(RoutingDeliveryParamsError::HistoryLimitOutOfRange);
            };
            if !(1..=200).contains(&limit) {
                return Err(RoutingDeliveryParamsError::HistoryLimitOutOfRange);
            }
            limit as usize
        }
    };

    Ok(DeliveryHistoryRequest {
        recipient,
        sink,
        limit,
    })
}
