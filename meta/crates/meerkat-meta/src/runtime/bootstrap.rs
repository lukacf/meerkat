//! Runtime bootstrap — config resolution, module startup, and event loop initialization.

use super::*;

pub fn start_meta_runtime(
    config: MetaConfig,
    agent_events: Vec<EventEnvelope<UnifiedEvent>>,
    timeout: Duration,
) -> Result<MetaRuntimeHandle, MetaRuntimeError> {
    start_meta_runtime_with_options(config, agent_events, timeout, RuntimeOptions::default())
}

pub fn start_meta_runtime_with_options(
    config: MetaConfig,
    agent_events: Vec<EventEnvelope<UnifiedEvent>>,
    timeout: Duration,
    options: RuntimeOptions,
) -> Result<MetaRuntimeHandle, MetaRuntimeError> {
    // Pay reqwest's one-time process init (the macOS system-proxy scan,
    // ~700ms observed since the realtime feature unification enabled
    // reqwest's `system-proxy`) at startup, not inside the first HTTP-backed
    // RPC's latency window (the BigQuery adapter's timeout contract measures
    // that window).
    let _ = crate::runtime::session_store::shared_bigquery_http_client();
    let delivery_runtime_epoch_ms = current_time_ms();
    let mut lifecycle_events = Vec::new();
    let mut seq = 0_u64;
    lifecycle_events.push(LifecycleEvent {
        seq,
        stage: LifecycleStage::MobStarted,
    });
    seq += 1;

    let mut supervisor_transitions = Vec::new();
    let mut module_events = Vec::new();
    let mut loaded_modules = BTreeSet::new();
    let mut live_children = BTreeMap::new();

    // Every fallible step that does not need a module runs before any module
    // process starts, so a refusal here never leaves started processes behind.
    for module_id in &config.discovery.modules {
        if !config.modules.iter().any(|module| &module.id == module_id) {
            return Err(MetaRuntimeError::Config(
                ConfigResolutionError::ModuleNotConfigured(module_id.clone()),
            ));
        }
    }
    let memory_backend = match options.memory_backend.as_ref() {
        Some(MemoryBackendConfig::LocalJson(config)) => Some(
            LocalJsonMemoryStoreAdapter::from_config(config)
                .map_err(MetaRuntimeError::MemoryBackend)?,
        ),
        None => None,
    };
    let persisted_memory = match memory_backend.as_ref() {
        Some(backend) => backend
            .read_state()
            .map_err(MetaRuntimeError::MemoryBackend)?,
        None => PersistedMemoryState::default(),
    };
    let mut memory_assertions = persisted_memory
        .assertions
        .into_iter()
        .filter_map(|assertion| {
            let entity = MetaRuntimeHandle::canonical_memory_token(&assertion.entity)?;
            let topic = MetaRuntimeHandle::canonical_memory_token(&assertion.topic)?;
            let store = MetaRuntimeHandle::canonical_memory_store(&assertion.store)?;
            let fact = assertion.fact.trim();
            if fact.is_empty() {
                return None;
            }
            Some(MemoryAssertion {
                assertion_id: assertion.assertion_id,
                entity,
                topic,
                store,
                fact: fact.to_string(),
                metadata: assertion.metadata,
                indexed_at_ms: assertion.indexed_at_ms,
            })
        })
        .collect::<Vec<_>>();
    while memory_assertions.len() > MEMORY_ASSERTIONS_MAX_RETAINED {
        memory_assertions.remove(0);
    }
    let mut memory_conflicts = BTreeMap::new();
    for signal in persisted_memory.conflicts {
        let Some(entity) = MetaRuntimeHandle::canonical_memory_token(&signal.entity) else {
            continue;
        };
        let Some(topic) = MetaRuntimeHandle::canonical_memory_token(&signal.topic) else {
            continue;
        };
        let Some(store) = MetaRuntimeHandle::canonical_memory_store(&signal.store) else {
            continue;
        };
        let normalized_signal = MemoryConflictSignal {
            entity: entity.clone(),
            topic: topic.clone(),
            store: store.clone(),
            reason: signal
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
            updated_at_ms: signal.updated_at_ms,
        };
        memory_conflicts.insert(
            MemoryConflictKey {
                entity,
                topic,
                store,
            },
            normalized_signal,
        );
    }
    let memory_sequence = memory_assertions
        .iter()
        .filter_map(|assertion| parse_memory_assertion_sequence(&assertion.assertion_id))
        .max()
        .map(|last_sequence| last_sequence.saturating_add(1))
        .unwrap_or(memory_assertions.len() as u64);

    let mut retained_cleanup = module_process::RetainedModuleCleanup::default();
    for module_id in &config.discovery.modules {
        let module = config
            .modules
            .iter()
            .find(|module| &module.id == module_id)
            .ok_or_else(|| {
                MetaRuntimeError::Config(ConfigResolutionError::ModuleNotConfigured(
                    module_id.clone(),
                ))
            })?;

        let pre_spawn = config
            .pre_spawn
            .iter()
            .find(|data| data.module_id == *module_id);

        let mut start_result = supervise_module_start(module, pre_spawn, timeout, &options);
        supervisor_transitions.append(&mut start_result.transitions);
        for process in start_result.retained.drain(..) {
            retained_cleanup.retain(process);
        }
        if let Some(error) = start_result.terminal_error.as_ref() {
            let timestamp_ms = current_time_ms();
            module_events.push(EventEnvelope {
                event_id: format!("evt-supervisor-warning-{}-{timestamp_ms}", module.id),
                source: "module".to_string(),
                timestamp_ms,
                event: UnifiedEvent::Module(ModuleEvent {
                    module: module.id.clone(),
                    event_type: "supervisor.warning".to_string(),
                    payload: serde_json::json!({
                        "error": format!("{error:?}")
                    }),
                }),
            });
        }
        if let Some(event) = start_result.event {
            loaded_modules.insert(module_id.clone());
            if let Some(child) = start_result.child {
                live_children.insert(module_id.clone(), child);
            }
            module_events.push(event);
        }
    }

    lifecycle_events.push(LifecycleEvent {
        seq,
        stage: LifecycleStage::ModulesStarted,
    });
    seq += 1;

    let merged_events = merge_unified_events(module_events, agent_events);
    lifecycle_events.push(LifecycleEvent {
        seq,
        stage: LifecycleStage::MergedStreamStarted,
    });

    Ok(MetaRuntimeHandle {
        config,
        runtime_options: options,
        loaded_modules,
        live_children,
        retained_cleanup,
        lifecycle_events,
        supervisor_report: SupervisorReport {
            transitions: supervisor_transitions,
        },
        merged_events,
        routing_sequence: 0,
        routing_resolutions: BTreeMap::new(),
        routing_resolution_order: Vec::new(),
        runtime_routes: BTreeMap::new(),
        delivery_sequence: 0,
        delivery_runtime_epoch_ms,
        delivery_now_floor_ms: 0,
        delivery_clock_ms: 0,
        delivery_history: Vec::new(),
        delivery_idempotency: BTreeMap::new(),
        delivery_idempotency_by_delivery: BTreeMap::new(),
        delivery_rate_window_counts: BTreeMap::new(),
        gating_epoch: new_gating_epoch(),
        gating_sequence: 0,
        gating_pending: BTreeMap::new(),
        gating_pending_order: Vec::new(),
        gating_audit: Vec::new(),
        gating_resolution_observers: GatingResolutionObservers::default(),
        gating_notice_queue: Vec::new(),
        gating_escalation_routes: GatingEscalationRoutes::default(),
        gating_changes: tokio::sync::broadcast::channel(64).0,
        memory_sequence,
        memory_assertions,
        memory_conflicts,
        memory_backend,
        running: true,
    })
}

fn parse_memory_assertion_sequence(assertion_id: &str) -> Option<u64> {
    assertion_id
        .strip_prefix("memory-assert-")
        .and_then(|suffix| suffix.parse::<u64>().ok())
}
