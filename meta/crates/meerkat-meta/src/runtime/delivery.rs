//! Message delivery subsystem — send, history, and module-backed dispatch.

use super::module_boundary::{
    CORE_MODULE_MCP_TIMEOUT, DELIVERY_SEND_MCP_TOOL, call_module_mcp_tool_json, mcp_required_error,
    module_uses_mcp,
};
use super::*;

impl MetaRuntimeHandle {
    fn parse_delivery_outcome_payload(
        payload: &serde_json::Map<String, Value>,
    ) -> DeliveryBoundaryOutcome {
        let mut outcome = DeliveryBoundaryOutcome::default();
        if let Some(adapter) = payload.get("adapter").and_then(Value::as_str) {
            let adapter = adapter.trim();
            if !adapter.is_empty() {
                outcome.sink_adapter = Some(adapter.to_string());
            }
        }
        outcome.force_fail = payload
            .get("force_fail")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        outcome
    }

    fn refresh_delivery_clocks_from_now(&mut self) {
        let observed_now_ms = current_time_ms().saturating_sub(self.delivery_runtime_epoch_ms);
        self.delivery_now_floor_ms = self.delivery_now_floor_ms.max(observed_now_ms);
        self.delivery_clock_ms = self.delivery_clock_ms.max(self.delivery_now_floor_ms);
    }

    pub(super) fn next_route_resolved_timestamp_ms(&mut self) -> u64 {
        self.refresh_delivery_clocks_from_now();
        let latest_merged_timestamp_ms = self
            .merged_events
            .last()
            .map_or(0, |event| event.timestamp_ms);
        let timestamp_ms = self.delivery_clock_ms.max(latest_merged_timestamp_ms);
        self.delivery_clock_ms = timestamp_ms;
        timestamp_ms
    }

    fn scoped_idempotency_key(route_id: &str, idempotency_key: &str) -> String {
        format!("{route_id}:{idempotency_key}")
    }
    fn trusted_resolution_for_delivery(
        &self,
        provided: &RoutingResolution,
    ) -> Result<RoutingResolution, DeliverySendError> {
        let route_id = provided.route_id.trim();
        if route_id.is_empty() {
            return Err(DeliverySendError::InvalidRouteId);
        }
        let Some(trusted) = self.routing_resolutions.get(route_id) else {
            return Err(DeliverySendError::UnknownRouteId(route_id.to_string()));
        };
        if trusted != provided {
            return Err(DeliverySendError::ForgedResolution);
        }
        Ok(trusted.clone())
    }

    fn prune_delivery_rate_window_counts(&mut self, current_window_start_ms: u64) {
        let earliest_window_start_ms = current_window_start_ms.saturating_sub(
            DELIVERY_RATE_WINDOW_MS.saturating_mul(DELIVERY_RATE_WINDOWS_RETAINED - 1),
        );
        self.delivery_rate_window_counts
            .retain(|key, _| key.window_start_ms >= earliest_window_start_ms);
    }
    fn next_delivery_sequence(&mut self) -> u64 {
        Self::next_sequence(&mut self.delivery_sequence)
    }
    fn replay_delivery_for_scoped_idempotency(
        &mut self,
        provided_resolution: &RoutingResolution,
        idempotency_key: &str,
        payload: &Value,
    ) -> Result<Option<DeliveryRecord>, DeliverySendError> {
        let route_id = provided_resolution.route_id.trim();
        let scoped_key = Self::scoped_idempotency_key(route_id, idempotency_key);
        let Some(entry) = self.delivery_idempotency.get(&scoped_key).cloned() else {
            return Ok(None);
        };

        if entry.payload != *payload {
            return Err(DeliverySendError::IdempotencyPayloadMismatch);
        }
        if let Some(trusted_resolution) = self.routing_resolutions.get(route_id) {
            if trusted_resolution != provided_resolution {
                return Err(DeliverySendError::ForgedResolution);
            }
        } else if entry.canonical_resolution != *provided_resolution {
            return Err(DeliverySendError::ForgedResolution);
        }
        if let Some(existing) = self
            .delivery_history
            .iter()
            .find(|record| record.delivery_id == entry.delivery_id)
        {
            return Ok(Some(existing.clone()));
        }

        self.delivery_idempotency.remove(&scoped_key);
        let mut remove_reverse_index = false;
        if let Some(scoped_keys) = self
            .delivery_idempotency_by_delivery
            .get_mut(&entry.delivery_id)
        {
            scoped_keys.retain(|candidate| candidate != &scoped_key);
            remove_reverse_index = scoped_keys.is_empty();
        }
        if remove_reverse_index {
            self.delivery_idempotency_by_delivery
                .remove(&entry.delivery_id);
        }

        Ok(None)
    }
    fn parse_delivery_mcp_outcome(response: &Value) -> DeliveryBoundaryOutcome {
        let Some(payload) = response.as_object() else {
            return DeliveryBoundaryOutcome::default();
        };
        Self::parse_delivery_outcome_payload(payload)
    }
    pub fn send_delivery(
        &mut self,
        request: DeliverySendRequest,
    ) -> Result<DeliveryRecord, DeliverySendError> {
        if !self.is_module_loaded("delivery") {
            return Err(DeliverySendError::DeliveryModuleNotLoaded);
        }
        if let Some(idempotency_key) = request.idempotency_key.as_ref()
            && idempotency_key.trim().is_empty()
        {
            return Err(DeliverySendError::InvalidIdempotencyKey);
        }

        if let Some(idempotency_key) = request.idempotency_key.as_deref()
            && let Some(existing) = self.replay_delivery_for_scoped_idempotency(
                &request.resolution,
                idempotency_key,
                &request.payload,
            )?
        {
            return Ok(existing);
        }

        let trusted_resolution = self.trusted_resolution_for_delivery(&request.resolution)?;
        if trusted_resolution.target_module != "delivery" {
            return Err(DeliverySendError::InvalidRouteTarget(
                trusted_resolution.target_module,
            ));
        }
        if trusted_resolution.recipient.trim().is_empty() {
            return Err(DeliverySendError::InvalidRecipient);
        }
        if trusted_resolution.sink.trim().is_empty() {
            return Err(DeliverySendError::InvalidSink);
        }

        let scoped_idempotency_key = request.idempotency_key.as_ref().map(|idempotency_key| {
            Self::scoped_idempotency_key(&trusted_resolution.route_id, idempotency_key)
        });
        self.refresh_delivery_clocks_from_now();
        let rate_window_now_ms = self.delivery_now_floor_ms;
        let window_start_ms = rate_window_now_ms - (rate_window_now_ms % DELIVERY_RATE_WINDOW_MS);
        self.prune_delivery_rate_window_counts(window_start_ms);
        let rate_key = DeliveryRateWindowKey {
            route_id: trusted_resolution.route_id.clone(),
            recipient: trusted_resolution.recipient.clone(),
            sink: trusted_resolution.sink.clone(),
            window_start_ms,
        };
        let current_count = self
            .delivery_rate_window_counts
            .get(&rate_key)
            .copied()
            .unwrap_or(0);
        if current_count >= trusted_resolution.rate_limit_per_minute {
            return Err(DeliverySendError::RateLimited {
                sink: trusted_resolution.sink.clone(),
                window_start_ms,
                limit: trusted_resolution.rate_limit_per_minute,
            });
        }
        let first_attempt_ms = self
            .delivery_clock_ms
            .saturating_add(DELIVERY_CLOCK_STEP_MS);
        self.delivery_clock_ms = first_attempt_ms;

        let boundary_outcome =
            if let Some((delivery_module, pre_spawn)) = self.module_and_prespawn("delivery") {
                if !module_uses_mcp(delivery_module, pre_spawn) {
                    return Err(DeliverySendError::DeliveryBoundary(mcp_required_error(
                        "delivery",
                        DELIVERY_SEND_MCP_TOOL,
                    )));
                }
                let mcp_response = call_module_mcp_tool_json(
                    delivery_module,
                    pre_spawn,
                    DELIVERY_SEND_MCP_TOOL,
                    &serde_json::to_value(&request).unwrap_or(Value::Null),
                    CORE_MODULE_MCP_TIMEOUT,
                )
                .map_err(DeliverySendError::DeliveryBoundary)?;
                Self::parse_delivery_mcp_outcome(&mcp_response)
            } else {
                return Err(DeliverySendError::DeliveryBoundary(mcp_required_error(
                    "delivery",
                    DELIVERY_SEND_MCP_TOOL,
                )));
            };

        // Charge rate limit only after the boundary call succeeded — transient
        // MCP/module failures should not burn quota.
        self.delivery_rate_window_counts
            .insert(rate_key, current_count.saturating_add(1));

        let should_force_fail = request
            .payload
            .get("force_fail")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || boundary_outcome.force_fail;
        let mut attempts = Vec::new();
        let status = if should_force_fail {
            "failed".to_string()
        } else {
            "sent".to_string()
        };
        let total_attempts = trusted_resolution.retry_max.saturating_add(1);
        for attempt in 1..=total_attempts {
            let is_final_attempt = attempt == total_attempts;
            attempts.push(DeliveryAttempt {
                attempt,
                status: if is_final_attempt {
                    status.clone()
                } else {
                    "transient_failure".to_string()
                },
                backoff_ms: if is_final_attempt {
                    0
                } else {
                    trusted_resolution.backoff_ms
                },
            });
        }

        let delivery_sequence = self.next_delivery_sequence();
        let delivery_id = format!("delivery-{delivery_sequence:06}");
        let final_attempt_ms = first_attempt_ms.saturating_add(
            trusted_resolution
                .backoff_ms
                .saturating_mul(u64::from(trusted_resolution.retry_max)),
        );
        let record = DeliveryRecord {
            delivery_id: delivery_id.clone(),
            route_id: trusted_resolution.route_id.clone(),
            recipient: trusted_resolution.recipient.clone(),
            sink: trusted_resolution.sink.clone(),
            target_module: trusted_resolution.target_module.clone(),
            payload: request.payload.clone(),
            status,
            attempts,
            first_attempt_ms,
            final_attempt_ms,
            idempotency_key: request.idempotency_key.clone(),
            sink_adapter: boundary_outcome.sink_adapter,
        };

        insert_event_sorted(
            &mut self.merged_events,
            EventEnvelope {
                event_id: format!("evt-delivery-{delivery_sequence:06}"),
                source: "module".to_string(),
                timestamp_ms: final_attempt_ms,
                event: UnifiedEvent::Module(ModuleEvent {
                    module: "delivery".to_string(),
                    event_type: "send".to_string(),
                    payload: serde_json::json!({
                        "delivery_id": record.delivery_id,
                        "route_id": record.route_id,
                        "recipient": record.recipient,
                        "sink": record.sink,
                        "status": record.status,
                        "attempts": record.attempts,
                    }),
                }),
            },
        );
        self.delivery_clock_ms = self.delivery_clock_ms.max(final_attempt_ms);

        if let Some(scoped_key) = scoped_idempotency_key {
            self.delivery_idempotency.insert(
                scoped_key.clone(),
                DeliveryIdempotencyEntry {
                    delivery_id: delivery_id.clone(),
                    payload: request.payload.clone(),
                    canonical_resolution: trusted_resolution,
                },
            );
            self.delivery_idempotency_by_delivery
                .entry(delivery_id)
                .or_default()
                .push(scoped_key);
        }
        self.delivery_history.push(record.clone());
        while self.delivery_history.len() > DELIVERY_HISTORY_LIMIT_MAX {
            let evicted = self.delivery_history.remove(0);
            if let Some(scoped_keys) = self
                .delivery_idempotency_by_delivery
                .remove(&evicted.delivery_id)
            {
                for scoped_key in scoped_keys {
                    self.delivery_idempotency.remove(&scoped_key);
                }
            }
        }

        Ok(record)
    }

    pub fn delivery_history(&self, request: DeliveryHistoryRequest) -> DeliveryHistoryResponse {
        let recipient_filter = request
            .recipient
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        let sink_filter = request
            .sink
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());

        let deliveries = self
            .delivery_history
            .iter()
            .filter(|record| {
                recipient_filter
                    .as_ref()
                    .is_none_or(|recipient| record.recipient == **recipient)
            })
            .filter(|record| {
                sink_filter
                    .as_ref()
                    .is_none_or(|sink| record.sink == **sink)
            })
            .rev()
            .take(request.limit)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();

        DeliveryHistoryResponse { deliveries }
    }
    pub fn delivery_rate_window_count_entries(&self) -> usize {
        self.delivery_rate_window_counts.len()
    }
}
