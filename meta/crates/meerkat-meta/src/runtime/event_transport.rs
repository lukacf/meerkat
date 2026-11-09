//! Event normalization and transport — line parsing, source validation, and envelope construction.

use super::*;

pub fn normalize_event_line(line: &str) -> Result<EventEnvelope<UnifiedEvent>, NormalizationError> {
    if let Ok(envelope) = parse_unified_event_line(line) {
        return enforce_source_consistency(envelope);
    }

    let value: Value = serde_json::from_str(line).map_err(|_| NormalizationError::InvalidJson)?;
    let object = value.as_object().ok_or(NormalizationError::InvalidSchema)?;

    let event_id = required_string(object.get("event_id"), "event_id")?;
    let source = required_string(object.get("source"), "source")?;
    let timestamp_ms = required_u64(object.get("timestamp_ms"), "timestamp_ms")?;

    if let Some(module) = object.get("module") {
        let module = required_string(Some(module), "module")?;
        let event_type = required_string(object.get("event_type"), "event_type")?;
        let payload = object
            .get("payload")
            .ok_or(NormalizationError::MissingField("payload"))?
            .clone();
        return enforce_source_consistency(EventEnvelope {
            event_id,
            source,
            timestamp_ms,
            event: UnifiedEvent::Module(ModuleEvent {
                module,
                event_type,
                payload,
            }),
        });
    }

    let agent_id = required_string(object.get("agent_id"), "agent_id")?;
    let event_type = required_string(object.get("event_type"), "event_type")?;
    let payload = object.get("payload").cloned();

    enforce_source_consistency(EventEnvelope {
        event_id,
        source,
        timestamp_ms,
        event: UnifiedEvent::Agent {
            agent_id,
            event_type,
            payload,
        },
    })
}

impl MetaRuntimeHandle {
    pub(crate) fn append_normalized_event(
        &mut self,
        event: EventEnvelope<UnifiedEvent>,
    ) -> Result<(), NormalizationError> {
        let event = enforce_source_consistency(event)?;
        insert_event_sorted(&mut self.merged_events, event);
        Ok(())
    }

    pub fn merged_events(&self) -> &[EventEnvelope<UnifiedEvent>] {
        &self.merged_events
    }
    pub fn subscribe_events(
        &self,
        request: SubscribeRequest,
    ) -> Result<SubscribeResponse, SubscribeError> {
        if let Some(checkpoint) = request.last_event_id.as_ref()
            && checkpoint.trim().is_empty()
        {
            return Err(SubscribeError::EmptyCheckpoint);
        }

        if matches!(request.scope, SubscribeScope::Agent) {
            let agent_id = request
                .agent_id
                .as_deref()
                .ok_or(SubscribeError::MissingAgentId)?;
            if agent_id.trim().is_empty() {
                return Err(SubscribeError::InvalidAgentId);
            }
        }

        let scoped_events: Vec<_> = self
            .merged_events
            .iter()
            .filter(|event| event_matches_request(event, &request))
            .collect();
        let skip = scoped_events
            .len()
            .saturating_sub(SUBSCRIBE_REPLAY_EVENT_CAP);
        let bounded = &scoped_events[skip..];

        let replay_slice = match request.last_event_id.as_ref() {
            Some(checkpoint) => {
                let start_idx = bounded
                    .iter()
                    .position(|event| event.event_id == *checkpoint)
                    .ok_or_else(|| SubscribeError::UnknownCheckpoint(checkpoint.clone()))?;
                &bounded[start_idx..]
            }
            None => bounded,
        };
        let replay_events: Vec<_> = replay_slice.iter().map(|e| (*e).clone()).collect();
        let event_frames = replay_events
            .iter()
            .map(build_sse_event_frame)
            .collect::<Vec<_>>();

        Ok(SubscribeResponse {
            scope: request.scope,
            replay_from_event_id: request.last_event_id,
            keep_alive: SubscribeKeepAlive {
                interval_ms: SSE_KEEP_ALIVE_INTERVAL_MS,
                event: SSE_KEEP_ALIVE_EVENT_NAME.to_string(),
            },
            keep_alive_comment: SSE_KEEP_ALIVE_COMMENT_FRAME.to_string(),
            event_frames,
            events: replay_events,
        })
    }
}

pub(super) fn merge_unified_events(
    mut module_events: Vec<EventEnvelope<UnifiedEvent>>,
    mut agent_events: Vec<EventEnvelope<UnifiedEvent>>,
) -> Vec<EventEnvelope<UnifiedEvent>> {
    let mut merged = Vec::with_capacity(module_events.len() + agent_events.len());
    merged.append(&mut module_events);
    merged.append(&mut agent_events);
    merged.sort_by(|left, right| {
        left.timestamp_ms
            .cmp(&right.timestamp_ms)
            .then_with(|| left.event_id.cmp(&right.event_id))
            .then_with(|| left.source.cmp(&right.source))
    });
    retain_newest_merged_events(&mut merged);
    merged
}

/// Keep at least the newest [`MERGED_EVENTS_RETAINED`] merged events: once
/// the buffer passes that by [`MERGED_EVENTS_TRIM_SLACK`], drop the oldest
/// down to it in one batch. The buffer is ordered oldest first, so the newest
/// event, whose timestamp the delivery clock reads, stays.
fn retain_newest_merged_events(events: &mut Vec<EventEnvelope<UnifiedEvent>>) {
    if events.len() > MERGED_EVENTS_RETAINED + MERGED_EVENTS_TRIM_SLACK {
        let excess = events.len() - MERGED_EVENTS_RETAINED;
        events.drain(..excess);
    }
}

/// True when a caller-supplied agent-scope filter matches a projected
/// `agent_id`. The projected id is the generation-suffixed public alias
/// (`{alias}:{generation}`, e.g. `worker:0` or `rt:review:singleton:0:1`), but
/// callers naturally pass the BARE member id / durable identity (the value every
/// sibling identity surface accepts). Accept either form: an exact match, or the
/// bare alias whose only difference is a trailing `:{generation}` segment.
fn agent_scope_filter_matches(selected: &str, agent_id: &str) -> bool {
    if selected == agent_id {
        return true;
    }
    // `agent_id == "{selected}:{generation}"` where generation is numeric.
    agent_id
        .strip_prefix(selected)
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|generation| {
            !generation.is_empty() && generation.bytes().all(|b| b.is_ascii_digit())
        })
}

fn event_matches_request(event: &EventEnvelope<UnifiedEvent>, request: &SubscribeRequest) -> bool {
    match request.scope {
        SubscribeScope::Mob => true,
        SubscribeScope::Agent => match &event.event {
            UnifiedEvent::Agent { agent_id, .. } => request
                .agent_id
                .as_deref()
                .map(|selected| agent_scope_filter_matches(selected, agent_id))
                .unwrap_or(false),
            UnifiedEvent::Module(_) => false,
        },
        SubscribeScope::Interaction => match &event.event {
            UnifiedEvent::Agent { event_type, .. } => event_type.starts_with("interaction"),
            UnifiedEvent::Module(module_event) => {
                module_event.event_type.starts_with("interaction")
            }
        },
    }
}

fn build_sse_event_frame(event: &EventEnvelope<UnifiedEvent>) -> String {
    let event_name = match &event.event {
        UnifiedEvent::Agent { event_type, .. } => event_type.as_str(),
        UnifiedEvent::Module(module_event) => module_event.event_type.as_str(),
    };
    let payload = serde_json::to_string(&event.event).unwrap_or_else(|_| "{}".to_string());
    format!(
        "id: {}\nevent: {}\ndata: {}\n\n",
        event.event_id, event_name, payload
    )
}

fn enforce_source_consistency(
    envelope: EventEnvelope<UnifiedEvent>,
) -> Result<EventEnvelope<UnifiedEvent>, NormalizationError> {
    let expected = match &envelope.event {
        UnifiedEvent::Agent { .. } => "agent",
        UnifiedEvent::Module(_) => "module",
    };
    if envelope.source != expected {
        return Err(NormalizationError::SourceMismatch {
            expected,
            got: envelope.source,
        });
    }
    Ok(envelope)
}

fn required_string(
    value: Option<&Value>,
    field: &'static str,
) -> Result<String, NormalizationError> {
    let value = value.ok_or(NormalizationError::MissingField(field))?;
    let text = value
        .as_str()
        .ok_or(NormalizationError::InvalidFieldType(field))?;
    Ok(text.to_string())
}

fn required_u64(value: Option<&Value>, field: &'static str) -> Result<u64, NormalizationError> {
    let value = value.ok_or(NormalizationError::MissingField(field))?;
    value
        .as_u64()
        .ok_or(NormalizationError::InvalidFieldType(field))
}

pub(super) fn insert_event_sorted(
    events: &mut Vec<EventEnvelope<UnifiedEvent>>,
    event: EventEnvelope<UnifiedEvent>,
) {
    let insertion_index = events
        .binary_search_by(|existing| {
            existing
                .timestamp_ms
                .cmp(&event.timestamp_ms)
                .then_with(|| existing.event_id.cmp(&event.event_id))
                .then_with(|| existing.source.cmp(&event.source))
        })
        .unwrap_or_else(|index| index);
    events.insert(insertion_index, event);
    retain_newest_merged_events(events);
}

#[cfg(test)]
mod tests {
    use super::agent_scope_filter_matches;

    #[test]
    fn agent_scope_filter_accepts_bare_and_generation_suffixed_ids() {
        // Real events carry the generation-suffixed alias. A caller filtering by
        // the bare member id / durable identity (what every sibling identity
        // surface accepts) must match — the old exact-compare returned zero
        // events silently. Regression for the agent-scope filter id mismatch.

        // Bare alias matches the generation-suffixed projected id.
        assert!(agent_scope_filter_matches("worker", "worker:0"));
        assert!(agent_scope_filter_matches("worker", "worker:12"));
        // Identity-first alias with embedded colons + a trailing generation.
        assert!(agent_scope_filter_matches(
            "rt:review:singleton:0",
            "rt:review:singleton:0:1"
        ));
        // Exact match still works (caller passed the full suffixed id).
        assert!(agent_scope_filter_matches("worker:0", "worker:0"));

        // Must NOT over-match a different member that merely shares a prefix.
        assert!(!agent_scope_filter_matches("worker", "worker-2:0"));
        assert!(!agent_scope_filter_matches("work", "worker:0"));
        // The trailing segment must be a numeric generation, not arbitrary text.
        assert!(!agent_scope_filter_matches("worker", "worker:abc"));
        assert!(!agent_scope_filter_matches("worker", "worker:"));
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod merged_event_buffer_tests {
    use super::*;

    fn runtime_with(agent_events: Vec<EventEnvelope<UnifiedEvent>>) -> MetaRuntimeHandle {
        crate::start_meta_runtime(
            crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: "merged-event-buffer".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            agent_events,
            Duration::from_secs(1),
        )
        .expect("runtime starts")
    }

    fn agent_event(index: u64, agent_id: &str, event_type: &str) -> EventEnvelope<UnifiedEvent> {
        EventEnvelope {
            event_id: format!("evt-{agent_id}-{index:06}"),
            source: "agent".to_string(),
            timestamp_ms: 1_000 + index,
            event: UnifiedEvent::Agent {
                agent_id: agent_id.to_string(),
                event_type: event_type.to_string(),
                payload: Some(serde_json::json!({"delta": "chunk"})),
            },
        }
    }

    fn event_ids(events: &[EventEnvelope<UnifiedEvent>]) -> Vec<String> {
        events.iter().map(|event| event.event_id.clone()).collect()
    }

    fn retained() -> u64 {
        u64::try_from(MERGED_EVENTS_RETAINED).expect("cap fits u64")
    }

    /// The most the buffer holds before it trims.
    fn trim_point() -> u64 {
        retained() + u64::try_from(MERGED_EVENTS_TRIM_SLACK).expect("slack fits u64")
    }

    #[test]
    fn the_buffer_keeps_only_the_newest_events() {
        let mut runtime = runtime_with(vec![]);
        // Up to the trim point nothing goes; one more trims back to the cap.
        for index in 0..trim_point() {
            runtime
                .append_normalized_event(agent_event(index, "worker", "text_delta"))
                .expect("append");
        }
        assert_eq!(
            runtime.merged_events().len(),
            MERGED_EVENTS_RETAINED + MERGED_EVENTS_TRIM_SLACK
        );
        let extra = trim_point() + 1 - retained();
        let total = retained() + extra;
        for index in trim_point()..total {
            runtime
                .append_normalized_event(agent_event(index, "worker", "text_delta"))
                .expect("append");
        }
        let expected: Vec<String> = (extra..total)
            .map(|index| agent_event(index, "worker", "text_delta").event_id)
            .collect();
        assert_eq!(event_ids(runtime.merged_events()), expected);
    }

    #[test]
    fn the_startup_merge_keeps_only_the_newest_events() {
        let total = trim_point() + 5;
        let extra = total - retained();
        let runtime = runtime_with(
            (0..total)
                .map(|index| agent_event(index, "worker", "text_delta"))
                .collect(),
        );
        let merged = runtime.merged_events();
        assert_eq!(merged.len(), MERGED_EVENTS_RETAINED);
        assert_eq!(
            merged.first().map(|event| event.event_id.clone()),
            Some(agent_event(extra, "worker", "text_delta").event_id)
        );
        assert_eq!(
            merged.last().map(|event| event.event_id.clone()),
            Some(agent_event(total - 1, "worker", "text_delta").event_id)
        );
    }

    #[test]
    fn a_checkpoint_older_than_the_buffer_is_an_expired_checkpoint() {
        let mut runtime = runtime_with(vec![]);
        let quiet = agent_event(0, "quiet", "interaction_complete");
        runtime
            .append_normalized_event(quiet.clone())
            .expect("append");
        let request = || SubscribeRequest {
            scope: SubscribeScope::Agent,
            last_event_id: Some(quiet.event_id.clone()),
            agent_id: Some("quiet".to_string()),
        };
        let replayed = runtime
            .subscribe_events(request())
            .expect("checkpoint replays");
        assert_eq!(event_ids(&replayed.events), vec![quiet.event_id.clone()]);

        for index in 1..=trim_point() {
            runtime
                .append_normalized_event(agent_event(index, "busy", "text_delta"))
                .expect("append");
        }

        assert_eq!(
            runtime.subscribe_events(request()),
            Err(SubscribeError::UnknownCheckpoint(quiet.event_id))
        );
        let fresh = runtime
            .subscribe_events(SubscribeRequest {
                scope: SubscribeScope::Agent,
                last_event_id: None,
                agent_id: Some("quiet".to_string()),
            })
            .expect("a read without a checkpoint never fails");
        assert!(fresh.events.is_empty());
    }

    #[test]
    fn a_subscriber_that_keeps_up_misses_no_event_across_trims() {
        let mut runtime = runtime_with(vec![]);
        let total = trim_point() + 64;
        let mut checkpoint: Option<String> = None;
        let mut seen = Vec::new();
        for index in 0..total {
            runtime
                .append_normalized_event(agent_event(index, "worker", "text_delta"))
                .expect("append");
            let response = runtime
                .subscribe_events(SubscribeRequest {
                    scope: SubscribeScope::Mob,
                    last_event_id: checkpoint.clone(),
                    agent_id: None,
                })
                .expect("the latest checkpoint stays in the window");
            // Replay includes the checkpoint itself.
            seen.extend(
                response
                    .events
                    .into_iter()
                    .map(|event| event.event_id)
                    .filter(|event_id| Some(event_id) != checkpoint.as_ref()),
            );
            checkpoint = seen.last().cloned();
        }
        let expected: Vec<String> = (0..total)
            .map(|index| agent_event(index, "worker", "text_delta").event_id)
            .collect();
        assert_eq!(seen, expected);
        let held = runtime.merged_events().len();
        assert!(
            (MERGED_EVENTS_RETAINED..=MERGED_EVENTS_RETAINED + MERGED_EVENTS_TRIM_SLACK)
                .contains(&held),
            "{held} events held"
        );
    }
}
