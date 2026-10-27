//! Module supervisor — process lifecycle, health monitoring, and restart logic.

use super::module_boundary::{module_env_with_extra, module_uses_mcp, probe_module_mcp_tools};
use super::module_process::{FirstLine, ModuleProcess, read_first_line};
use super::*;

pub fn run_module_boundary_once(
    module: &ModuleConfig,
    pre_spawn: Option<&PreSpawnData>,
    timeout: Duration,
) -> Result<EventEnvelope<UnifiedEvent>, RuntimeBoundaryError> {
    run_module_boundary_with_env(module, pre_spawn, &[], timeout)
}

pub(super) fn run_module_boundary_with_env(
    module: &ModuleConfig,
    pre_spawn: Option<&PreSpawnData>,
    extra_env: &[(String, String)],
    timeout: Duration,
) -> Result<EventEnvelope<UnifiedEvent>, RuntimeBoundaryError> {
    let env = module_env_with_extra(module, pre_spawn, extra_env);
    let line = run_process_json_line(&module.command, &module.args, &env, timeout)
        .map_err(RuntimeBoundaryError::Process)?;
    normalize_event_line(&line).map_err(RuntimeBoundaryError::Normalize)
}

pub fn run_discovered_module_once(
    config: &MetaConfig,
    module_id: &str,
    timeout: Duration,
) -> Result<EventEnvelope<UnifiedEvent>, RuntimeFromConfigError> {
    let module = config
        .modules
        .iter()
        .find(|module| module.id == module_id)
        .ok_or_else(|| {
            RuntimeFromConfigError::Config(ConfigResolutionError::ModuleNotConfigured(
                module_id.to_string(),
            ))
        })?;

    if !config.discovery.modules.iter().any(|id| id == module_id) {
        return Err(RuntimeFromConfigError::Config(
            ConfigResolutionError::ModuleNotDiscovered(module_id.to_string()),
        ));
    }

    let pre_spawn = config
        .pre_spawn
        .iter()
        .find(|data| data.module_id == module_id);
    run_module_boundary_once(module, pre_spawn, timeout).map_err(RuntimeFromConfigError::Runtime)
}

impl MetaRuntimeHandle {
    pub(super) fn is_module_loaded(&self, module_id: &str) -> bool {
        self.loaded_modules.contains(module_id)
    }
    pub fn shutdown(&mut self) -> RuntimeShutdownReport {
        let mut seq = self
            .lifecycle_events
            .last()
            .map_or(0, |event| event.seq + 1);
        self.lifecycle_events.push(LifecycleEvent {
            seq,
            stage: LifecycleStage::ShutdownRequested,
        });
        seq += 1;
        self.lifecycle_events.push(LifecycleEvent {
            seq,
            stage: LifecycleStage::ShutdownComplete,
        });
        self.running = false;

        let terminated_modules: Vec<String> = self.loaded_modules.iter().cloned().collect();
        self.loaded_modules.clear();

        let mut orphan_processes = 0_u32;
        let mut reaped = Vec::new();
        let children = std::mem::take(&mut self.live_children);
        // Only the owner's confirmed wait counts as a reap. A child whose
        // termination was not confirmed stays owned here, retained for
        // `progress_module_cleanup` and reported as outstanding.
        for (_, mut child) in children {
            if child.terminate(false).is_ok() {
                reaped.push(child.reaped());
            } else {
                orphan_processes += 1;
                self.retained_cleanup.retain(child);
            }
        }
        // Earlier retained processes (an aborted respawn's replacement) get
        // one more non-blocking step; their confirmed reaps are reported too.
        let mut physical_cleanup = self.retained_cleanup.progress();
        reaped.append(&mut physical_cleanup.reaped);
        physical_cleanup.reaped = reaped;
        orphan_processes = orphan_processes.max(physical_cleanup.outstanding.len() as u32);

        RuntimeShutdownReport {
            terminated_modules,
            orphan_processes,
            physical_cleanup,
        }
    }

    /// Retained module cleanup as its owners see it, without advancing it.
    pub fn module_cleanup_observation(&self) -> ModuleCleanupObservation {
        self.retained_cleanup.observation(Vec::new())
    }

    /// One non-blocking cleanup step for every retained module process.
    pub fn progress_module_cleanup(&mut self) -> ModuleCleanupObservation {
        self.retained_cleanup.progress()
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn loaded_modules(&self) -> Vec<String> {
        self.loaded_modules.iter().cloned().collect()
    }

    pub(super) fn module_and_prespawn(
        &self,
        module_id: &str,
    ) -> Option<(&ModuleConfig, Option<&PreSpawnData>)> {
        let module = self
            .config
            .modules
            .iter()
            .find(|module| module.id == module_id)?;
        let pre_spawn = self
            .config
            .pre_spawn
            .iter()
            .find(|data| data.module_id == module_id);
        Some((module, pre_spawn))
    }

    pub fn reconcile_modules(
        &mut self,
        modules: Vec<String>,
        timeout: Duration,
    ) -> Result<usize, RuntimeMutationError> {
        for module_id in &modules {
            if self
                .config
                .modules
                .iter()
                .all(|configured| configured.id != *module_id)
            {
                return Err(RuntimeMutationError::Config(
                    ConfigResolutionError::ModuleNotConfigured(module_id.clone()),
                ));
            }
        }

        self.config.discovery.modules = modules.clone();
        let mut added = 0_usize;
        for module_id in modules {
            if self.loaded_modules.contains(&module_id) {
                continue;
            }
            self.spawn_member(&module_id, timeout)?;
            added += 1;
        }
        Ok(added)
    }

    pub fn spawn_member(
        &mut self,
        module_id: &str,
        timeout: Duration,
    ) -> Result<(), RuntimeMutationError> {
        let module = self
            .config
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .ok_or_else(|| {
                RuntimeMutationError::Config(ConfigResolutionError::ModuleNotConfigured(
                    module_id.to_string(),
                ))
            })?;

        let pre_spawn = self
            .config
            .pre_spawn
            .iter()
            .find(|data| data.module_id == module_id);

        let mut result = supervise_module_start(module, pre_spawn, timeout, &self.runtime_options);
        self.supervisor_report
            .transitions
            .append(&mut result.transitions);
        for process in result.retained.drain(..) {
            self.retained_cleanup.retain(process);
        }

        if let Some(error) = result.terminal_error.clone() {
            insert_event_sorted(
                &mut self.merged_events,
                supervisor_warning_event(module_id, &error),
            );
        }

        let Some(event) = result.event else {
            return Err(RuntimeMutationError::Runtime(
                result
                    .terminal_error
                    .unwrap_or(RuntimeBoundaryError::Process(
                        ProcessBoundaryError::EmptyOutput,
                    )),
            ));
        };
        let module_is_mcp = module_uses_mcp(module, pre_spawn);

        if !self
            .config
            .discovery
            .modules
            .iter()
            .any(|configured| configured == module_id)
        {
            self.config.discovery.modules.push(module_id.to_string());
        }

        if !module_is_mcp {
            let Some(mut child) = result.child else {
                return Err(RuntimeMutationError::Runtime(
                    result
                        .terminal_error
                        .unwrap_or(RuntimeBoundaryError::Process(
                            ProcessBoundaryError::EmptyOutput,
                        )),
                ));
            };
            if let Some(mut existing_child) = self.live_children.remove(module_id)
                && let Err(err) = existing_child
                    .terminate(self.runtime_options.supervisor_test_force_terminate_failure)
            {
                self.live_children
                    .insert(module_id.to_string(), existing_child);

                let mut error_message =
                    format!("failed to terminate existing child before respawn: {err}");
                if let Err(replacement_err) = child.terminate(false) {
                    error_message.push_str(&format!(
                        "; failed to terminate replacement child after aborted respawn: {replacement_err}"
                    ));
                    // The old child stays live above; the unconfirmed
                    // replacement stays owned as retained cleanup.
                    self.retained_cleanup.retain(child);
                }
                let runtime_error =
                    RuntimeBoundaryError::Process(ProcessBoundaryError::Io(error_message));
                insert_event_sorted(
                    &mut self.merged_events,
                    supervisor_warning_event(module_id, &runtime_error),
                );
                return Err(RuntimeMutationError::Runtime(runtime_error));
            }
            self.loaded_modules.insert(module_id.to_string());
            self.live_children.insert(module_id.to_string(), child);
            insert_event_sorted(&mut self.merged_events, event);
            return Ok(());
        }

        if let Some(mut existing_child) = self.live_children.remove(module_id)
            && let Err(err) = existing_child
                .terminate(self.runtime_options.supervisor_test_force_terminate_failure)
        {
            self.live_children
                .insert(module_id.to_string(), existing_child);
            let runtime_error = RuntimeBoundaryError::Process(ProcessBoundaryError::Io(format!(
                "failed to terminate existing child before MCP respawn: {err}"
            )));
            insert_event_sorted(
                &mut self.merged_events,
                supervisor_warning_event(module_id, &runtime_error),
            );
            return Err(RuntimeMutationError::Runtime(runtime_error));
        }

        self.loaded_modules.insert(module_id.to_string());
        insert_event_sorted(&mut self.merged_events, event);
        Ok(())
    }
}

pub(super) struct SuperviseModuleStartResult {
    pub event: Option<EventEnvelope<UnifiedEvent>>,
    pub child: Option<ModuleProcess>,
    pub transitions: Vec<ModuleHealthTransition>,
    pub terminal_error: Option<RuntimeBoundaryError>,
    /// Children of failed start attempts whose termination was not
    /// confirmed. The caller retains them; they are never released as reaped.
    pub retained: Vec<ModuleProcess>,
}

pub(super) fn supervise_module_start(
    module: &ModuleConfig,
    pre_spawn: Option<&PreSpawnData>,
    timeout: Duration,
    options: &RuntimeOptions,
) -> SuperviseModuleStartResult {
    if module_uses_mcp(module, pre_spawn) {
        return supervise_mcp_module_start(module, pre_spawn, timeout);
    }

    let mut transitions = vec![ModuleHealthTransition {
        module_id: module.id.clone(),
        from: None,
        to: ModuleHealthState::Starting,
        attempt: 0,
    }];

    let mut attempts = 0_u32;
    let mut state = ModuleHealthState::Starting;
    let mut retained = Vec::new();

    loop {
        attempts += 1;
        let result = spawn_module_capture_first_event(
            module,
            pre_spawn,
            timeout,
            options.supervisor_test_force_terminate_failure,
        );

        match result {
            Ok((event, mut child)) => {
                transitions.push(ModuleHealthTransition {
                    module_id: module.id.clone(),
                    from: Some(state.clone()),
                    to: ModuleHealthState::Healthy,
                    attempt: attempts,
                });

                let should_restart = match module.restart_policy {
                    RestartPolicy::Always => attempts <= options.always_restart_budget,
                    _ => false,
                };

                if should_restart {
                    transitions.push(ModuleHealthTransition {
                        module_id: module.id.clone(),
                        from: Some(ModuleHealthState::Healthy),
                        to: ModuleHealthState::Restarting,
                        attempt: attempts,
                    });
                    if let Err(err) =
                        child.terminate(options.supervisor_test_force_terminate_failure)
                    {
                        transitions.push(ModuleHealthTransition {
                            module_id: module.id.clone(),
                            from: Some(ModuleHealthState::Restarting),
                            to: ModuleHealthState::Failed,
                            attempt: attempts,
                        });
                        transitions.push(ModuleHealthTransition {
                            module_id: module.id.clone(),
                            from: Some(ModuleHealthState::Failed),
                            to: ModuleHealthState::Healthy,
                            attempt: attempts,
                        });
                        return SuperviseModuleStartResult {
                            event: Some(event),
                            child: Some(child),
                            transitions,
                            terminal_error: Some(RuntimeBoundaryError::Process(
                                ProcessBoundaryError::Io(format!(
                                    "terminate child failed during restart: {err}"
                                )),
                            )),
                            retained: std::mem::take(&mut retained),
                        };
                    }
                    apply_restart_backoff(options);
                    state = ModuleHealthState::Restarting;
                    continue;
                }

                return SuperviseModuleStartResult {
                    event: Some(event),
                    child: Some(child),
                    transitions,
                    terminal_error: None,
                    retained: std::mem::take(&mut retained),
                };
            }
            Err(failed) => {
                let (err, unconfirmed) = *failed;
                retained.extend(unconfirmed);
                transitions.push(ModuleHealthTransition {
                    module_id: module.id.clone(),
                    from: Some(state.clone()),
                    to: ModuleHealthState::Failed,
                    attempt: attempts,
                });

                let should_retry = match module.restart_policy {
                    RestartPolicy::Never => false,
                    RestartPolicy::OnFailure => attempts <= options.on_failure_retry_budget,
                    RestartPolicy::Always => attempts <= options.always_restart_budget,
                };

                if should_retry {
                    transitions.push(ModuleHealthTransition {
                        module_id: module.id.clone(),
                        from: Some(ModuleHealthState::Failed),
                        to: ModuleHealthState::Restarting,
                        attempt: attempts,
                    });
                    apply_restart_backoff(options);
                    state = ModuleHealthState::Restarting;
                    continue;
                }

                transitions.push(ModuleHealthTransition {
                    module_id: module.id.clone(),
                    from: Some(ModuleHealthState::Failed),
                    to: ModuleHealthState::Stopped,
                    attempt: attempts,
                });
                return SuperviseModuleStartResult {
                    event: None,
                    child: None,
                    transitions,
                    terminal_error: Some(err),
                    retained: std::mem::take(&mut retained),
                };
            }
        }
    }
}

fn supervise_mcp_module_start(
    module: &ModuleConfig,
    pre_spawn: Option<&PreSpawnData>,
    timeout: Duration,
) -> SuperviseModuleStartResult {
    let mut transitions = vec![ModuleHealthTransition {
        module_id: module.id.clone(),
        from: None,
        to: ModuleHealthState::Starting,
        attempt: 0,
    }];

    match probe_module_mcp_tools(module, pre_spawn, timeout) {
        Ok(tools) => {
            transitions.push(ModuleHealthTransition {
                module_id: module.id.clone(),
                from: Some(ModuleHealthState::Starting),
                to: ModuleHealthState::Healthy,
                attempt: 1,
            });
            SuperviseModuleStartResult {
                event: Some(mcp_ready_event(module, tools)),
                child: None,
                transitions,
                terminal_error: None,
                retained: Vec::new(),
            }
        }
        Err(error) => {
            transitions.push(ModuleHealthTransition {
                module_id: module.id.clone(),
                from: Some(ModuleHealthState::Starting),
                to: ModuleHealthState::Failed,
                attempt: 1,
            });
            transitions.push(ModuleHealthTransition {
                module_id: module.id.clone(),
                from: Some(ModuleHealthState::Failed),
                to: ModuleHealthState::Stopped,
                attempt: 1,
            });
            SuperviseModuleStartResult {
                event: None,
                child: None,
                transitions,
                terminal_error: Some(error),
                retained: Vec::new(),
            }
        }
    }
}

fn mcp_ready_event(module: &ModuleConfig, tools: Vec<String>) -> EventEnvelope<UnifiedEvent> {
    let timestamp_ms = current_time_ms();
    EventEnvelope {
        event_id: format!("evt-mcp-ready-{}-{timestamp_ms}", module.id),
        source: "module".to_string(),
        timestamp_ms,
        event: UnifiedEvent::Module(ModuleEvent {
            module: module.id.clone(),
            event_type: "mcp.ready".to_string(),
            payload: serde_json::json!({
                "tools": tools,
            }),
        }),
    }
}

fn apply_restart_backoff(options: &RuntimeOptions) {
    if options.supervisor_restart_backoff_ms == 0 {
        return;
    }
    std::thread::sleep(Duration::from_millis(options.supervisor_restart_backoff_ms));
}

/// A failed start attempt and its child, when that child's termination was
/// not confirmed and it must stay owned by the caller.
type FailedModuleStart = (RuntimeBoundaryError, Option<ModuleProcess>);

fn spawn_module_capture_first_event(
    module: &ModuleConfig,
    pre_spawn: Option<&PreSpawnData>,
    timeout: Duration,
    force_terminate_failure: bool,
) -> Result<(EventEnvelope<UnifiedEvent>, ModuleProcess), Box<FailedModuleStart>> {
    let env = module_env_with_extra(module, pre_spawn, &[]);
    let mut command = Command::new(&module.command);
    command
        .args(&module.args)
        .envs(env.iter().map(|(k, v)| (k, v)));
    let (mut child, stdout) = ModuleProcess::spawn(&module.id, command)
        .map_err(|error| Box::new((RuntimeBoundaryError::Process(error), None)))?;

    // Every failure below terminates the child; if that is not confirmed, the
    // child is handed back to the caller's retained cleanup, still owned.
    let failure = match read_first_line(stdout, timeout) {
        FirstLine::Line(mut line) => {
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            match normalize_event_line(&line) {
                Ok(event) => return Ok((event, child)),
                Err(err) => (
                    format!("normalize error: {err:?}"),
                    RuntimeBoundaryError::Normalize(err),
                ),
            }
        }
        // Closing stdout does not mean the module exited.
        FirstLine::Closed => (
            "empty output".to_string(),
            RuntimeBoundaryError::Process(ProcessBoundaryError::EmptyOutput),
        ),
        FirstLine::ReadFailed(err) => (
            format!("io read error: {err}"),
            RuntimeBoundaryError::Process(ProcessBoundaryError::Io(err)),
        ),
        FirstLine::TimedOut => {
            let timeout_ms = timeout.as_millis() as u64;
            (
                format!("timeout({timeout_ms}ms)"),
                RuntimeBoundaryError::Process(ProcessBoundaryError::Timeout { timeout_ms }),
            )
        }
    };
    let (context, error) = failure;
    if let Err(terminate_err) = child.terminate(force_terminate_failure) {
        return Err(Box::new((
            RuntimeBoundaryError::Process(ProcessBoundaryError::Io(format!(
                "cleanup terminate failed after {context}: {terminate_err}"
            ))),
            Some(child),
        )));
    }
    Err(Box::new((error, None)))
}

fn supervisor_warning_event(
    module_id: &str,
    error: &RuntimeBoundaryError,
) -> EventEnvelope<UnifiedEvent> {
    let timestamp_ms = current_time_ms();
    EventEnvelope {
        event_id: format!("evt-supervisor-warning-{module_id}-{timestamp_ms}"),
        source: "module".to_string(),
        timestamp_ms,
        event: UnifiedEvent::Module(ModuleEvent {
            module: module_id.to_string(),
            event_type: "supervisor.warning".to_string(),
            payload: serde_json::json!({
                "error": format!("{error:?}")
            }),
        }),
    }
}
