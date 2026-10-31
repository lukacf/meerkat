//! Runtime lifecycle management — startup, shutdown, rediscovery, and periodic maintenance.

use std::future::Future;
use std::future::IntoFuture;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Bound on the mob actor's terminal teardown in [`UnifiedRuntime::shutdown`],
/// a phase of its own in the published shutdown horizon.
///
/// The teardown unregisters every member runtime concurrently and returns as
/// soon as the last one ends, so the bound only caps a slow teardown: a large
/// fleet's unregisters, each several durable commits through one runtime
/// store, finish inside it, and a member still pending at it really is stuck.
pub const MOB_TERMINAL_SHUTDOWN_BUDGET: Duration = Duration::from_secs(35);

/// Bound on standing durable child mobs down in [`UnifiedRuntime::shutdown`],
/// taken when the root mob's terminal teardown has returned, so a root that
/// spends its whole budget does not starve the child root. A phase of its own
/// in the published shutdown horizon.
///
/// It covers the agent mob-tool state's admission close, each child's own
/// shutdown and the state's stand-down. A child mob with a fleet too large to
/// unregister within it is reported Retained, and its root stays owned.
pub const CHILD_MOB_STAND_DOWN_BUDGET: Duration = Duration::from_secs(10);

/// Budget the shutdown horizon reserves for joining retired supervisor
/// cleanups. Meta runs no lease-renewal or continuity-repair
/// supervisor, so nothing is ever retired and the phase completes at once;
/// the budget stays in the advertised horizon until the deletion sweep
/// removes the phase from the report.
pub const RETIRED_SUPERVISOR_JOIN_BUDGET: Duration = Duration::from_secs(2);

use meerkat_mob::SpawnMemberSpec;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::watch;

use crate::mob_handle_runtime::MobRuntimeError;
use crate::runtime::RuntimeDecisionState;

use super::event_log::{EventLogIntakeEnded, EventLogRoom};
use super::types::{
    IdentityAuthorityReleaseOutcome, MemberMcpSettlement, MobStopOutcome,
    MobTerminalShutdownOutcome, RediscoverReport, RetiredSupervisorCleanupOutcome,
    ShutdownDrainReport, UnifiedRuntimeError, UnifiedRuntimeRunReport,
    UnifiedRuntimeShutdownReport,
};
use super::{MobEventIngress, UnifiedRuntime, discovery_spec_to_spawn_spec};

/// The `HostLoopCrash` detail for a `run_failed` agent event.
///
/// meerkat's `RunFailed` carries its failure truth as the typed
/// `error_report { class, reason, message }` and the projected payload keeps
/// it, so name it here: for a non-retryable provider failure (a 401 on a bad
/// key) meerkat itself logs nothing at warn or above, which makes this ERROR
/// line the only place an operator reading logs learns WHY the run failed.
/// A payload without a report (a producer that predates it, or a
/// payload-less envelope) falls back to the bare event id the line always
/// carried.
pub(super) fn run_failed_crash_error(
    event_id: &str,
    payload: Option<&serde_json::Value>,
) -> String {
    fn string_field(value: Option<&serde_json::Value>) -> Option<&str> {
        value.and_then(serde_json::Value::as_str)
    }
    let mut detail = format!("agent run failed (event_id: {event_id})");
    let Some(report) = payload.and_then(|payload| payload.get("error_report")) else {
        return detail;
    };
    if let Some(class) = string_field(report.get("class")) {
        detail.push_str(&format!(" class={class}"));
    }
    if let Some(reason_type) = string_field(
        report
            .get("reason")
            .and_then(|reason| reason.get("reason_type")),
    ) {
        detail.push_str(&format!(" reason={reason_type}"));
    }
    if let Some(message) = string_field(report.get("message")) {
        detail.push_str(&format!(": {message}"));
    }
    detail
}

impl UnifiedRuntime {
    /// Reset the mob and re-run discovery + edge reconciliation.
    ///
    /// Sequence:
    /// 1. `MobHandle::reset()` — retires all members, clears projections,
    ///    restarts MCP servers, returns mob to Running state
    /// 2. Re-runs the stored `Discovery` (with `Value::Null` context since
    ///    `PreSpawnHook` is consumed at boot and cannot be replayed)
    /// 3. Spawns discovered members via `spawn_many`
    /// 4. Clears managed dynamic edges (stale after reset)
    /// 5. Runs edge reconciliation if `EdgeDiscovery` is configured
    ///
    /// Returns `None` if no `Discovery` is configured (nothing to rediscover).
    pub async fn rediscover(&self) -> Result<Option<RediscoverReport>, MobRuntimeError> {
        match self.rediscover_inner().await {
            Ok(report) => Ok(report),
            Err(err) => {
                self.fire_error(super::types::ErrorEvent::RediscoverFailure {
                    error: format!("{err}"),
                });
                Err(err)
            }
        }
    }

    async fn rediscover_inner(&self) -> Result<Option<RediscoverReport>, MobRuntimeError> {
        let discovery = match &self.discovery {
            Some(d) => d,
            None => return Ok(None),
        };
        if self.identity_runtime_ref().is_some() {
            return Err(MobRuntimeError::InvalidConfig(
                "rediscover resets the whole mob and is unavailable with identity-first authority; use refresh_desired_topology"
                    .to_string(),
            ));
        }

        // 1. Reset the mob — retires all, clears state, returns to Running
        self.mob_runtime
            .handle()
            .reset()
            .await
            .map_err(MobRuntimeError::Mob)?;

        // 2. Re-run discovery (no pre-spawn context — PreSpawnHook is FnOnce)
        let specs = discovery.discover(serde_json::Value::Null).await;
        let spawn_specs: Vec<SpawnMemberSpec> =
            specs.iter().map(discovery_spec_to_spawn_spec).collect();
        let spawned: Vec<String> = spawn_specs.iter().map(|s| s.identity.to_string()).collect();

        // 3. Spawn discovered members (hook-aware variant fires post_spawn_hook)
        self.spawn_many(spawn_specs).await?;

        // 4. Clear stale managed edges (old topology is gone after reset)
        self.managed_dynamic_edges.write().await.clear();

        // 5. Reconcile edges
        let edges = self.reconcile_edges().await;

        Ok(Some(RediscoverReport { spawned, edges }))
    }

    pub async fn run<F>(
        &self,
        listener: tokio::net::TcpListener,
        decisions: RuntimeDecisionState,
        shutdown_signal: F,
    ) -> UnifiedRuntimeRunReport
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let app = self.build_reference_app_router(decisions);
        let serve = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal)
            .into_future();
        let serve_result = self.serve_while_draining(serve).await;
        let shutdown = self.shutdown().await;
        UnifiedRuntimeRunReport {
            serve_result,
            shutdown,
        }
    }

    pub async fn serve(
        &self,
        listener: tokio::net::TcpListener,
        decisions: RuntimeDecisionState,
    ) -> std::io::Result<()> {
        let app = self.build_reference_app_router(decisions);
        self.serve_while_draining(axum::serve(listener, app).into_future())
            .await
    }

    /// Drive `serve` to its end while the mob events drain every 25 ms, and
    /// at once after a drain that left events queued
    /// ([`pause_before_next_drain`]).
    ///
    /// A drain can wait for room in the event log, so it runs beside the
    /// server rather than between its polls: a slow or stalled store holds
    /// up neither new connections nor the graceful-shutdown signal. When
    /// serving ends during a drain, the drain is told to stop and awaited.
    /// It stops before it takes its next event, at once when it is waiting
    /// for room, and is never cut off in the middle of an event; the
    /// shutdown's drains take the rest.
    async fn serve_while_draining<T>(&self, serve: impl Future<Output = T>) -> T {
        tokio::pin!(serve);
        let (stop, stopping) = watch::channel(false);
        let mut more_queued = false;
        loop {
            tokio::select! {
                result = &mut serve => return result,
                () = pause_before_next_drain(more_queued) => {
                    let drain = self.drain_mob_agent_events_until(Some(&stopping));
                    tokio::pin!(drain);
                    tokio::select! {
                        result = &mut serve => {
                            stop.send_replace(true);
                            let _ = drain.await;
                            return result;
                        }
                        end = &mut drain => {
                            more_queued = matches!(end, Ok(DrainEnd::MoreQueued));
                        }
                    }
                }
            }
        }
    }

    /// Spawn a detached task that periodically drains mob agent events and
    /// projects them onto the ConsoleEventStore. Returns a [`tokio::task::JoinHandle`] -
    /// callers that manage graceful shutdown should abort it before stopping
    /// the runtime.
    ///
    /// Use this when embedding [`UnifiedRuntime`] inside a host-owned axum
    /// server (so [`Self::serve`]'s built-in drain loop isn't running).
    /// Without this task the mob event router fills up, agent turns never
    /// reach the console SSE stream, and event-log consumers miss events.
    /// It drains every 25 ms, and at once after a drain that took its whole
    /// backlog and left events queued, so a busy router is drained without
    /// a tick between router loads.
    pub fn spawn_event_drain_task(self: std::sync::Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut more_queued = false;
            loop {
                pause_before_next_drain(more_queued).await;
                if self.shutting_down.load(Ordering::SeqCst) {
                    break;
                }
                match self.drain_mob_agent_events_until(None).await {
                    Ok(end) => more_queued = end == DrainEnd::MoreQueued,
                    Err(err) => {
                        more_queued = false;
                        if matches!(err, UnifiedRuntimeError::RuntimeShuttingDown) {
                            break;
                        }
                        // Transient drain failures are logged but don't stop the
                        // task: the next tick will try again.
                        tracing::warn!(error = %err, "mob agent event drain tick failed");
                    }
                }
            }
        })
    }

    /// How long [`Self::shutdown`] may legitimately take.
    ///
    /// **Bound your wait on this, not on a generic stage timeout.** Shutdown
    /// covers two provider-callback windows, runtime event and mob drains,
    /// bounded RPC/HTTP/stdout phases, and process-reap margin. A caller that
    /// gives it less is not detecting a hang - it is interrupting work the
    /// runtime is bounded to finish, and abandoning the wait does not stop it:
    /// the shutdown keeps running while the caller proceeds to exit, which is
    /// how cleanup gets killed mid-flight.
    ///
    /// This exists because the number was already published, documented and
    /// arithmetically asserted - and two independent consumers still bounded it
    /// 4x and 11x too tightly on the same night (2026-08-31), each by applying
    /// an ordinary host stage timeout. A `pub const` in a module they had no
    /// reason to open was not discoverable at the point of use. This method is
    /// reachable from the value they already hold and are about to shut down.
    ///
    /// The stdio surface has always advertised its horizon over the wire via
    /// `stdio_shutdown_horizon_ms`; this is the embedder's equivalent.
    ///
    /// ```no_run
    /// # async fn example(runtime: meerkat_meta::UnifiedRuntime) {
    /// tokio::time::timeout(runtime.shutdown_horizon(), runtime.shutdown())
    ///     .await
    ///     .expect("shutdown exceeded its own published horizon");
    /// # }
    /// ```
    #[must_use]
    pub fn shutdown_horizon(&self) -> Duration {
        crate::gateway_composition::GATEWAY_RUNTIME_SHUTDOWN_TIMEOUT
    }

    /// Retained module cleanup as its process owners see it. `None` while the
    /// module runtime is busy (for example inside its own shutdown): an
    /// absent observation, never a settled one.
    pub fn try_module_cleanup_observation(
        &self,
    ) -> Option<crate::runtime::ModuleCleanupObservation> {
        self.module_runtime
            .try_lock()
            .ok()
            .map(|runtime| runtime.module_cleanup_observation())
    }

    pub async fn shutdown(&self) -> UnifiedRuntimeShutdownReport {
        self.shutdown_with_root_budget(MOB_TERMINAL_SHUTDOWN_BUDGET)
            .await
    }

    /// [`Self::shutdown`] with `root_budget` bounding the mob actor's
    /// terminal teardown in place of [`MOB_TERMINAL_SHUTDOWN_BUDGET`]. The
    /// child stand-down keeps [`CHILD_MOB_STAND_DOWN_BUDGET`].
    pub(super) async fn shutdown_with_root_budget(
        &self,
        root_budget: Duration,
    ) -> UnifiedRuntimeShutdownReport {
        // The close begins under the bootstrap completion's gate: a running
        // publication finishes first, and every later one is refused.
        self.close_gate.close();
        self.shutting_down.store(true, Ordering::SeqCst);
        // Nothing is delivered once teardown starts: stop the mob
        // announcement dispatcher first, whichever surface composed it.
        // Stopping is idempotent; a gateway may already have stopped it.
        if let Some(announcements) = self.mob_announcements() {
            announcements.stop();
        }
        // The liveness probe is observation only. Abort it before anything
        // can quiesce the mob actor so an intentional shutdown stall cannot
        // page a false ActorLoopStalled.
        if let Some(task) = self.actor_loop_probe_task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
        // The continuity mirror writes its last copy while the mob still
        // serves reads; the mirror's own deadline bounds the wait.
        let continuity_mirror = self.continuity_mirror.lock().await.take();
        if let Some(mirror) = continuity_mirror {
            let counts = mirror.stop().await;
            tracing::info!(
                writes = counts.writes.load(Ordering::Relaxed),
                failures = counts.failures.load(Ordering::Relaxed),
                "continuity mirror stopped"
            );
        }
        // The fact tail owns only a lossy wake projection, so shutdown does
        // not drain it. Abort and join the sole runtime-owned task before
        // taking down the authoritative WorkGraph-bearing mob runtime.
        if let Some(task) = self.workgraph_fact_tail_task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(observer) = self.agent_memory_observer_task.lock().await.take() {
            observer.abort_and_join().await;
        }
        if let Some(task) = self.agent_memory_steward_task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
        // Reconnect probes are observation accelerants only, but they own
        // sockets and may be mid-authentication. Stop and join the sole task
        // before closing the listener or quiescing member/session authority.
        if let Some(task) = self.remote_host_reconnect_task.lock().await.take() {
            abort_and_join_remote_host_reconnect(task).await;
        }
        // Stop accepting cross-mob control RPC before the mob quiesces so a
        // late inbound wire/inject cannot race member teardown.
        if let Some(task) = self.cross_mob_control_task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
        let identity_runtime = self.identity_runtime_ref().cloned();
        if let Some(identity_runtime) = identity_runtime.as_ref() {
            // Close request admission before background work is drained. A
            // caller may disappear while its lazy wake is in flight; the
            // runtime, not the caller, owns that task to its end.
            identity_runtime.close_foreground_operations();
        }
        if let Some(identity_runtime) = identity_runtime.as_ref() {
            // Background hydration owns concrete member creation/resume work;
            // stop and join it before quiescing the mob actor.
            identity_runtime.cancel_identity_bootstrap().await;
            // Foreground request tasks can share the same materialization and
            // lifecycle locks. Join them after the warmer has stopped and
            // before the mob actor is taken down.
            identity_runtime.join_foreground_operations().await;
        }
        if let Some(task) = self.implicit_delegate_retirement_task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }

        // A reset hands the outgoing session's memory capture to a
        // runtime-owned task, so the reset stays latency-neutral. Join those
        // tasks once foreground lifecycle admission is closed, then run any
        // capture a cancelled task left behind, before the mob stops.
        if let Some(identity_runtime) = identity_runtime.as_ref() {
            // The customizer `after_create` calls run outside any lifecycle
            // lock; closing foreground admission abandoned those still
            // waiting for their customizer.
            identity_runtime.join_after_create_notices().await;
            identity_runtime.join_reset_memory_captures().await;
            let ran = identity_runtime.drain_pending_reset_memory_captures().await;
            if ran > 0 {
                tracing::debug!(ran, "ran reset memory captures left by cancelled tasks");
            }
        }

        // Phase 1: Drain in-flight events
        let drain_start = std::time::Instant::now();
        // Phase 1 and the post-stop drain before Phase 3 share this window:
        // the published shutdown horizon counts one event drain.
        let drain_deadline = tokio::time::Instant::from_std(drain_start) + self.drain_timeout;
        // The console events this shutdown's drains project follow this
        // position; the post-stop drain makes each of them durable.
        let console_drain_position = self.console_events.latest_event_id().await;
        let mut drained_count = 0_usize;
        let drain_result = tokio::time::timeout(self.drain_timeout, async {
            loop {
                if self.drain_mob_agent_events().await.is_err() {
                    break;
                }
                let ingress = self.mob_event_ingress.lock().await;
                if ingress.is_none() {
                    break;
                }
                drop(ingress);
                drained_count += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
                if drained_count > 1 {
                    break;
                }
            }
        })
        .await;
        let drain = ShutdownDrainReport {
            drained_count,
            timed_out: drain_result.is_err(),
            drain_duration_ms: drain_start.elapsed().as_millis() as u64,
        };

        // Phase 2: Stop the mob actor while its router/module dependencies
        // are still alive. Closing them first can race Stop against an
        // already-dropped actor reply channel under teardown pressure.
        // The Stop holds the members' run starts and leaves the runs they are
        // in running: the terminal Shutdown below decides each one, keeping a
        // durable member's in-flight run and eligible queued input for its
        // next start and ending the others. Without a drain window this is
        // the drain's own Stop with a zero window; after a drain the mob is
        // already Stopped and this Stop is refused as such, which counts as
        // stopped.
        // A refusal stays Err: the gates below (detaching identity authority,
        // terminal teardown) are conservative on a mob that did not quiesce.
        // Phases 3 and 4 continue either way. A candidate launch stops
        // nothing ([`Self::stop_mob_unless_candidate`]).
        let mob_stop = self
            .stop_mob_unless_candidate(meerkat_mob::StopOptions::default().leave_runs_running())
            .await;
        // Shutdown is terminal: the delivery owner stops with the mob rather
        // than when the last runtime clone happens to drop. Undelivered rows
        // stay pending in the durable inbox.
        self.mob_runtime.stop_runtime_delivery();
        // Terminal as well: no member is built or admitted after Stop, and
        // the builder's tool slots otherwise keep the whole composition alive.
        self.mob_runtime.release_composition_slots();

        // Meta holds no identity lease authority: Core owns every
        // member binding, so there is no grant to release here.
        let identity_authority_release = IdentityAuthorityReleaseOutcome::NotConfigured;
        if mob_stop.is_ok() {
            // Break the MobRuntime <-> IdentityRuntime authority cycle only
            // after the mob is stopped; the runs it left running end, or are
            // kept, in the terminal teardown below. This is required for
            // failed builders to release persistent topology/store locks
            // before returning Err; on a failed mob stop the authority
            // deliberately stays attached.
            self.mob_runtime.clear_identity_runtime_authority();
            *self
                .implicit_delegate_identity_runtime
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }

        // `stop` quiesces members but keeps the mob actor - and its
        // `{mob_id}/__mob_supervisor__` in-proc route - alive for resume.
        // UnifiedRuntime shutdown is terminal, and meerkat 0.8.23 refuses
        // route displacement, so a same-process cold replacement for this mob
        // id must observe the name actually freed. Drive the mob's own
        // terminal teardown (which retires the supervisor route) only AFTER
        // identity authority is detached. Best-effort: a refusal leaves the
        // route registered and is reported loudly rather than failing the
        // report.
        let mob_terminal_shutdown = if mob_stop.is_ok() {
            self.shutdown_mob_actor(root_budget).await
        } else {
            MobTerminalShutdownOutcome::SkippedMobStopFailed
        };

        // Durable child mobs stand down after the root mob, which a failed
        // root stop skips because children may still be running. Their root
        // claim is released only after a proven stand-down. Their window
        // starts now: a root teardown that spent its whole budget must not
        // leave them an expired deadline.
        let child_mob_stand_down = if mob_stop.is_ok() {
            self.mob_runtime
                .stand_down_child_mobs(std::time::Instant::now() + CHILD_MOB_STAND_DOWN_BUDGET)
                .await
        } else if self.mob_runtime.owns_child_mob_root() {
            crate::child_mob_storage::ChildMobStandDownOutcome::SkippedMobStopFailed
        } else {
            crate::child_mob_storage::ChildMobStandDownOutcome::NotConfigured
        };

        // The member teardown above is bounded, so it can end before a stdio
        // close's own bound, and an agent's release of its MCP router is not a
        // cleanup receipt. Start every member MCP close in custody and wait
        // for each native outcome before any cleanup is reported: only a close
        // that reported settled counts as complete.
        let member_mcp_settlement = self.settle_member_mcp().await;

        // The terminal teardown above ends a member's in-flight run (or, for
        // a durable member, suspends it for its next start) after Phase 1
        // drained, so the run's terminal reaches the router only now. Drain
        // once more and make the console frames durable before the router
        // closes and the live projection detaches, or a turn the shutdown
        // interrupted never gets its terminal frame.
        self.drain_terminal_events_after_stop(drain_deadline, console_drain_position)
            .await;

        // Phase 3: Close event router
        self.close_event_router().await;
        // The aborted structural-events subscription's last cursor write may
        // still run on the metadata store's own task (cursor writes are
        // coalesced off the caller). It lands within what is left of the
        // drain window; dropped with the runtime it would poison the
        // cursor's host store scope in this process, and the next start
        // would replay the events after the previous cursor.
        let _ = tokio::time::timeout_at(
            drain_deadline,
            self.persistent_metadata.settle_detached_writes(),
        )
        .await;

        // The runtime owns the console projector. Detach its live source after
        // draining runtime events so closed runtimes cannot retain discovery tasks.
        if let Some(projection) = self.console_projection.get() {
            projection.unregister_runtime("default");
        }

        // Phase 4: Shutdown modules
        let module_shutdown = self.module_runtime.lock().await.shutdown();
        // No supervisor is ever retired (see
        // `RETIRED_SUPERVISOR_JOIN_BUDGET`).
        let retired_supervisor_cleanup = RetiredSupervisorCleanupOutcome::NothingPending;
        // The shutdown ran to its end: a later drop has nothing to hand over.
        // Entering shutdown alone never disarms the drop path.
        self.teardown_settled.store(true, Ordering::SeqCst);
        // A mob runtime that stood down cleanly released its share of the
        // state directory's claim. This runtime's own share ends with the
        // shutdown that covered it, so a successor may claim the directory;
        // otherwise the claim stays until the runtime drops.
        if !self.mob_runtime.holds_state_dir_claims() {
            self.state_dir_claims
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }
        UnifiedRuntimeShutdownReport {
            drain,
            module_shutdown,
            mob_stop,
            identity_authority_release,
            retired_supervisor_cleanup,
            mob_terminal_shutdown,
            child_mob_stand_down,
            member_mcp_settlement,
        }
    }

    /// Shutdown's post-stop drain: at most two drain passes 50 ms apart, as
    /// in Phase 1, then the console projection of every event this
    /// shutdown's drains appended after `console_drain_position`, all
    /// within `deadline`. Projection dedupes on the source event id, so
    /// frames the live projection already wrote are not written twice.
    async fn drain_terminal_events_after_stop(
        &self,
        deadline: tokio::time::Instant,
        console_drain_position: Option<String>,
    ) {
        let drained = tokio::time::timeout_at(deadline, async {
            for pass in 0..2 {
                if pass > 0 {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                if self.drain_mob_agent_events().await.is_err()
                    || self.mob_event_ingress.lock().await.is_none()
                {
                    break;
                }
            }
            let Some(projection) = self.console_projection.get() else {
                return;
            };
            let events = match self
                .console_events
                .replay_all(console_drain_position.as_deref())
                .await
            {
                Ok(events) => events,
                // The position aged out of the replay buffer: project all
                // the buffer still holds.
                Err(_) => self
                    .console_events
                    .replay_all(None)
                    .await
                    .unwrap_or_default(),
            };
            if let Err(error) = projection.project_runtime_events("default", events).await {
                tracing::warn!(
                    %error,
                    "shutdown could not persist every console frame of its final drain"
                );
            }
        })
        .await;
        if drained.is_err() {
            tracing::warn!(
                "shutdown's post-stop event drain reached the end of the event drain window"
            );
        }
    }

    /// The members' MCP servers, closed through the custody the gateway
    /// composed them under, with each close's native outcome. Each wait ends
    /// at the deadline Meerkat derives from the closes it started, within
    /// [`MEMBER_MCP_SETTLEMENT_BUDGET`](crate::member_mcp_custody::MEMBER_MCP_SETTLEMENT_BUDGET).
    /// Safe to repeat: a gateway's executor-exit boundary runs
    /// [`Self::settle_member_mcp_to_exit`] after it.
    pub(crate) async fn settle_member_mcp(&self) -> MemberMcpSettlement {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(custody) = self.mob_runtime.member_mcp_custody() {
            return custody.settle().await;
        }
        MemberMcpSettlement::NothingOwned
    }

    /// The member MCP settlement at a gateway's executor-exit boundary: as
    /// [`Self::settle_member_mcp`], except that a close still pending at its
    /// deadline is waited for until its native outcome reports. Nothing
    /// settles these closes after it, so no deadline ends its wait; a close
    /// that needed the wait still counts under `deadline`.
    pub(crate) async fn settle_member_mcp_to_exit(&self) -> MemberMcpSettlement {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(custody) = self.mob_runtime.member_mcp_custody() {
            return custody.settle_to_exit().await;
        }
        MemberMcpSettlement::NothingOwned
    }

    /// The mob actor's terminal teardown, once: meerkat 0.8.51's
    /// `shutdown_with_report` owns its convergence (stuck retirements, held
    /// unregisters and in-flight runs are settled or reported, not retried),
    /// bounded by `budget` ([`MOB_TERMINAL_SHUTDOWN_BUDGET`] in production).
    /// Its report is final: Meta never follows or rewrites a member's
    /// outcome after it. A candidate launch's teardown keeps every member's
    /// work as the store holds it
    /// ([`crate::MobRuntime::mob_shutdown_options`]).
    async fn shutdown_mob_actor(&self, budget: Duration) -> MobTerminalShutdownOutcome {
        let deadline = std::time::Instant::now() + budget;
        match self
            .mob_handle()
            .shutdown_with_report(self.mob_runtime.mob_shutdown_options(deadline))
            .await
        {
            Ok(report) => {
                if !report.is_clean() {
                    tracing::warn!(
                        members = ?report.members,
                        budget_ms = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX),
                        "mob terminal shutdown completed with members it could not settle \
                         within its budget"
                    );
                }
                MobTerminalShutdownOutcome::Completed(report)
            }
            // A closed command channel means the actor task has ended: the
            // mob is already shut down, not failing. A Shutdown sent while
            // another one drains joins it and gets that Shutdown's result,
            // so this call never sees `ShutdownInProgress` (meerkat answers
            // only other requests with it during a drain).
            Err(
                meerkat_mob::MobError::ActorCommandChannelClosed
                | meerkat_mob::MobError::ActorReplyChannelClosed,
            ) => MobTerminalShutdownOutcome::AlreadyShutDown,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "mob terminal shutdown after stop was refused; the supervisor in-proc \
                     route may remain registered until process exit"
                );
                MobTerminalShutdownOutcome::Refused {
                    error: error.to_string(),
                }
            }
        }
    }

    /// Stop the mob for teardown: [`MobStopOutcome::Stopped`], or the stop's
    /// refusal as [`MobStopOutcome::Failed`].
    ///
    /// This Stops the mob on any launch, a candidate launch
    /// (`CompositionAuthority::NonAuthoritative`) included: the Stop journals
    /// `MobStopped` in the shared mob store and fails pending spawns, cancels
    /// pending peer deliveries and closes kickoff origins. An embedder must
    /// not call it on a candidate launch; [`Self::shutdown`] is the
    /// candidate-safe teardown, which on a candidate stops nothing and runs
    /// the keep Shutdown alone.
    ///
    /// Since MobKit 0.8.46 this never returns
    /// [`MobStopOutcome::ProceededWithoutInterrupt`]: that degraded a
    /// `Runtime not ready: attached` refusal from meerkat's old stop
    /// interrupt path, which meerkat 0.8.51's Stop no longer takes. The
    /// variant (and [`super::types::ErrorEvent::MobStopProceededWithoutInterrupt`]) stays
    /// for wire and SDK compatibility.
    pub async fn stop_mob_for_teardown(&self) -> MobStopOutcome {
        let outcome = match self.stop_mob().await {
            Ok(()) => MobStopOutcome::Stopped,
            Err(error) => MobStopOutcome::Failed(error),
        };
        // Teardown: no delivery may be applied to the stopped mob.
        self.mob_runtime.stop_runtime_delivery();
        outcome
    }

    /// Stop the mob, settling its active flow runs first when they hold the
    /// Stop (see [`super::mob_stop`]). meerkat 0.8.51's Stop settles member
    /// work itself, so nothing here cancels member work or re-sends the stop
    /// on a timer; any other refusal is returned as is.
    async fn stop_mob(&self) -> Result<(), MobRuntimeError> {
        self.stop_mob_with(meerkat_mob::StopOptions::default())
            .await
    }

    /// [`Self::stop_mob_with`] for a shutdown's teardown, where a candidate
    /// launch stops nothing: a Stop, even one that leaves every run running,
    /// journals `MobStopped` in the shared mob store and fails pending
    /// spawns, cancels pending peer deliveries and closes kickoff origins.
    /// A candidate's teardown is its keep Shutdown alone, from Running
    /// ([`crate::MobRuntime::mob_shutdown_options`]); it counts as stopped.
    async fn stop_mob_unless_candidate(
        &self,
        options: meerkat_mob::StopOptions,
    ) -> Result<(), MobRuntimeError> {
        if !self.speaks_for_composition() {
            tracing::info!(
                "candidate launch: the mob is not stopped before its keep shutdown, so no stop \
                 is journaled in the shared mob store"
            );
            return Ok(());
        }
        self.stop_mob_with(options).await
    }

    /// [`Self::stop_mob`] with `options` for the mob's Stop.
    async fn stop_mob_with(
        &self,
        options: meerkat_mob::StopOptions,
    ) -> Result<(), MobRuntimeError> {
        super::mob_stop::settle_flow_runs_then_stop(
            &super::mob_stop::HandleStopTarget(&self.mob_handle(), options),
            super::mob_stop::MOB_STOP_FLOW_SETTLE_BUDGET,
        )
        .await
    }

    /// Let the mob's in-flight turns finish before a graceful shutdown:
    /// close this runtime's own ingress, stop the mob leaving every member's
    /// current run running, then wait until `until` for the run each member
    /// had ([`meerkat_mob::MobHandle::drain_stopped_runs`]). The
    /// [`Self::shutdown`] that must follow settles what is still running, as
    /// it does without a drain: its terminal Shutdown keeps a durable
    /// member's run and eligible queued input for the member's next start,
    /// and ends every other member's run.
    ///
    /// In order it closes cross-mob control inbound, identity foreground
    /// operations (admission closed, bootstrap cancelled, operations
    /// joined; a wait on a turn's outcome keeps waiting, so a drained turn's
    /// caller hears how it ended), the actor-loop probe and runtime delivery.
    /// Each is a step
    /// [`Self::shutdown`] repeats, so that shutdown finds it done. The
    /// identity steps are bounded by `until`: a held member add or a roster
    /// pass still settling at `until` cuts the drain before its Stop
    /// ([`InFlightDrainOutcome::IdentityForegroundBusy`]), and the shutdown
    /// settles them inside its own budget. The event router and console
    /// projection, the continuity mirror, memory, WorkGraph, member MCP and
    /// the session runtime keep running through the drain.
    ///
    /// [`InFlightDrainOutcome::IdentityForegroundBusy`]: super::InFlightDrainOutcome::IdentityForegroundBusy
    pub async fn drain_in_flight_turns(
        &self,
        until: std::time::Instant,
    ) -> super::InFlightDrainReport {
        let started = std::time::Instant::now();
        let report = |outcome| super::InFlightDrainReport {
            outcome,
            window: until.saturating_duration_since(started),
            waited: started.elapsed(),
        };
        // Cross-mob control inbound closes first, as in shutdown: no late
        // wire or inject reaches the stopping mob.
        if let Some(task) = self.cross_mob_control_task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(identity_runtime) = self.identity_runtime_ref().cloned() {
            // Waiters on a turn's outcome keep waiting: the drained turns'
            // waiters report how they ended.
            identity_runtime.close_foreground_operations_for_drain();
            // Both waits can outlast the window (a held add holds the
            // bootstrap controller until it ends). Both are cancel-safe: the
            // stop requests are set before the controller lock is awaited,
            // and the join resumes in the shutdown.
            let settled = tokio::time::timeout_at(tokio::time::Instant::from_std(until), async {
                identity_runtime.cancel_identity_bootstrap().await;
                identity_runtime.join_foreground_operations().await;
            })
            .await
            .is_ok();
            if !settled {
                return report(super::InFlightDrainOutcome::IdentityForegroundBusy);
            }
        }
        // The liveness probe is observation only: a Stopped mob with long
        // turns must not page a false ActorLoopStalled.
        if let Some(task) = self.actor_loop_probe_task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
        self.mob_runtime.stop_runtime_delivery();
        // A candidate launch holds every stored member, so no run is in
        // flight to drain, and it stops nothing
        // ([`Self::stop_mob_unless_candidate`]).
        if !self.speaks_for_composition() {
            return report(super::InFlightDrainOutcome::Drained(
                meerkat_mob::MobRunDrainReport::default(),
            ));
        }
        if let Err(error) = self
            .stop_mob_with(meerkat_mob::StopOptions::default().leave_runs_running())
            .await
        {
            return report(super::InFlightDrainOutcome::StopFailed {
                error: error.to_string(),
            });
        }
        tracing::info!(
            remaining_ms = u64::try_from(
                until
                    .saturating_duration_since(std::time::Instant::now())
                    .as_millis()
            )
            .unwrap_or(u64::MAX),
            "in-flight drain: the mob is stopped with its members' runs left running; waiting for them"
        );
        match self.mob_handle().drain_stopped_runs(until).await {
            Ok(drained) => report(super::InFlightDrainOutcome::Drained(drained)),
            Err(error) => report(super::InFlightDrainOutcome::DrainFailed {
                error: error.to_string(),
            }),
        }
    }

    /// Drain pending agent/module events from the mob event router and
    /// project them onto the ConsoleEventStore + event log. Callers that
    /// embed `UnifiedRuntime` inside their own axum server (rather than
    /// using `.serve()`) must poll this periodically — typically via
    /// [`UnifiedRuntime::spawn_event_drain_task`] — or console/event-log
    /// consumers will never see agent responses.
    ///
    /// A call takes the backlog it finds on entry; events that arrive while
    /// it runs wait for the next call. It takes an event only once the event
    /// log, when one is configured, has room for it, so this call (and
    /// [`UnifiedRuntime::subscribe_events`], which drains first) can wait on
    /// the event log store: a slow store slows the drain and the console
    /// projection it also feeds, and a store that never answers holds it
    /// until shutdown closes the log's intake (a shutdown whose drain window
    /// ends with events still queued reports `drain.timed_out`, and its
    /// cleanup is not attested). The wait is cancel-safe: a drain cut off
    /// while it waits for room has taken nothing. A host loop that awaits a
    /// drain inside a `select!` handler must race it against its own
    /// shutdown signal, or a stalled store keeps that loop from seeing the
    /// signal.
    pub async fn drain_mob_agent_events(&self) -> Result<(), UnifiedRuntimeError> {
        self.drain_mob_agent_events_until(None).await?;
        Ok(())
    }

    /// [`Self::drain_mob_agent_events`], stopping before it takes its next
    /// event once `stop` is raised: between events, never in the middle of
    /// one. It says whether it left events queued after taking its backlog.
    async fn drain_mob_agent_events_until(
        &self,
        stop: Option<&watch::Receiver<bool>>,
    ) -> Result<DrainEnd, UnifiedRuntimeError> {
        let mut disconnected = false;
        let mut end = DrainEnd::Done;
        let mut ingress_guard = match self.mob_event_ingress.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                // A previous drain tick may still be projecting a burst of
                // events. Skip this tick instead of killing the host-owned
                // background drain task.
                return Ok(DrainEnd::Done);
            }
        };
        let ingress = match ingress_guard.as_mut() {
            Some(i) => i,
            None => return Ok(DrainEnd::Done),
        };
        let events = Self::ingress_events(ingress);
        // The backlog found on entry is this call's work. Events that arrive
        // meanwhile wait for the next call, so a router refilled as fast as
        // a slow store lets the drain empty it cannot keep one call running.
        let mut backlog = events.len();

        loop {
            // Nothing queued: done, with no wait for room for nothing. Read
            // closure first: no event arrives once every sender is gone, so a
            // router closed and then empty stays empty.
            let closed = events.is_closed();
            if events.is_empty() {
                disconnected = closed;
                break;
            }
            if backlog == 0 {
                // The backlog is taken and more has arrived since.
                end = DrainEnd::MoreQueued;
                break;
            }
            // The event log's room for the next event, before that event is
            // taken from the router: a full log makes the drain wait here,
            // where stopping it takes nothing.
            let event_log_room = match self.ready_for_next_event(stop).await {
                NextEvent::Take(room) => room,
                NextEvent::Stop => break,
            };
            let forwarded = match events.try_recv() {
                Ok(forwarded) => forwarded,
                // Not seen: this drain holds the router, the only receiver,
                // so the queued event is still there.
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            };
            backlog -= 1;
            // Fire the typed alert the forwarder extracted at ingest, while
            // the member event was still typed: a member compaction
            // persistence rejection, or a failed member run as
            // `HostLoopCrash` (a cancelled run is not a crash). The hook is
            // read here, at drain time, so hooks installed via
            // `set_error_hook` after construction still fire.
            if let Some(alert) = forwarded.alert {
                self.fire_error(alert);
            }
            let unified_event = forwarded.envelope;
            if let Some(room) = event_log_room {
                room.ingest(unified_event.clone());
            }
            self.project_console_event_from_unified(&unified_event)
                .await;
            self.module_runtime
                .lock()
                .await
                .append_normalized_event(unified_event)?;
        }

        if disconnected {
            *ingress_guard = None;
        }

        Ok(end)
    }

    pub(super) async fn close_event_router(&self) {
        // A drain outside this shutdown (a host's drain task, a request that
        // drains) may still hold the router while it waits for room in an
        // event log whose store does not answer. Closing the log's intake
        // ends that wait, so the router is free to take. This shutdown's own
        // drains ran before, each within the drain window.
        if let Some(log) = self.event_log.as_ref() {
            log.close_intake();
        }
        let ingress = self.mob_event_ingress.lock().await.take();
        match ingress {
            Some(MobEventIngress::Forwarder(forwarder)) => {
                let task = forwarder.task;
                task.abort();
                let _ = task.await;
                let health_task = forwarder.identity_stream_health_task;
                health_task.abort();
                let _ = health_task.await;
                // What no drain took closes with the router: it reaches
                // neither the event log nor the console.
                let left = forwarder.event_rx.len();
                if left > 0 {
                    tracing::warn!(
                        left,
                        "the event router closed with forwarded member events no drain took; \
                         they reach neither the event log nor the console"
                    );
                }
            }
            None => {}
        }

        // Stop the structural mob-events subscription task as well.
        if let Some(task) = self.mob_events_subscriber_task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
    }

    /// The router's queue of forwarded member events.
    fn ingress_events(
        ingress: &mut MobEventIngress,
    ) -> &mut tokio::sync::mpsc::Receiver<super::ForwardedMemberEvent> {
        match ingress {
            MobEventIngress::Forwarder(forwarder) => &mut forwarder.event_rx,
        }
    }

    /// Wait, while the event log is full, for room for the next event a
    /// drain takes, or for `stop`. A raised stop wins, room or not.
    async fn ready_for_next_event(&self, stop: Option<&watch::Receiver<bool>>) -> NextEvent<'_> {
        let room = async {
            let Some(log) = self.event_log.as_ref() else {
                return NextEvent::Take(None);
            };
            match log.room().await {
                Ok(room) => NextEvent::Take(Some(room)),
                // Shutdown is closing the event router.
                Err(EventLogIntakeEnded::Closed) => NextEvent::Stop,
                // The log records nothing more. Say so once, and keep
                // feeding the console and the module events.
                Err(EventLogIntakeEnded::FlushTaskEnded) => {
                    if log.first_to_see_flush_task_end() {
                        self.fire_error(super::types::ErrorEvent::EventLogFlushFailure {
                            error: "event log flush task ended: the events it had buffered, the \
                                    batch it was writing and every later event are lost"
                                .to_string(),
                        });
                    }
                    NextEvent::Take(None)
                }
            }
        };
        tokio::select! {
            biased;
            () = until_raised(stop) => NextEvent::Stop,
            next = room => next,
        }
    }
}

/// What a drain finds before it takes its next event.
enum NextEvent<'a> {
    /// Take it, into this room in the event log when one is configured.
    Take(Option<EventLogRoom<'a>>),
    /// Stop before taking it: serving ended, or shutdown closed the event
    /// log's intake.
    Stop,
}

/// How a drain call ended, for a background loop pacing the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrainEnd {
    /// It took the whole backlog it found, and more has arrived since.
    MoreQueued,
    /// Anything else: nothing left queued, another drain holding the
    /// router, or a drain told to stop.
    Done,
}

/// The background drain loops' pause before their next drain: the 25 ms
/// tick, or only a yield to the other tasks after a drain that left events
/// queued. Each call still takes at most the backlog it finds, so a busy
/// router is drained call after call, without a tick between router loads.
async fn pause_before_next_drain(more_queued: bool) {
    if more_queued {
        tokio::task::yield_now().await;
    } else {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Resolves once `stop` is raised, or its sender is gone; never without a
/// `stop`.
async fn until_raised(stop: Option<&watch::Receiver<bool>>) {
    match stop {
        Some(stop) => {
            let mut stop = stop.clone();
            let _ = stop.wait_for(|raised| *raised).await;
        }
        None => std::future::pending::<()>().await,
    }
}

async fn abort_and_join_remote_host_reconnect(task: tokio::task::JoinHandle<()>) {
    task.abort();
    let _ = task.await;
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod remote_host_task_tests {
    use super::abort_and_join_remote_host_reconnect;

    struct DropNotify(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for DropNotify {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    #[tokio::test]
    async fn reconnect_task_abort_is_joined_before_shutdown_continues() {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _notify = DropNotify(Some(sender));
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;

        abort_and_join_remote_host_reconnect(task).await;
        receiver
            .await
            .expect("joined task must drop all owned reconnect state");
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
pub(crate) mod stop_degrade_tests {
    use super::*;
    use std::sync::Arc;

    pub(crate) async fn empty_runtime(mob_id: &str) -> UnifiedRuntime {
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"

[profiles.worker.tools]
comms = true
"#
        ))
        .expect("definition parses");
        UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(Arc::new(meerkat_client::TestClient::for_provider(
                meerkat_core::Provider::OpenAI,
            )))
            .build()
            .await
            .expect("runtime builds")
    }

    /// D67: entering `shutdown()` does not disarm the drop path. An explicit
    /// shutdown held in a real phase (module teardown, behind its lock) and
    /// abandoned by its caller has not settled; dropping the runtime then
    /// hands its state to the retained teardown, which runs the shutdown to
    /// its end and only then releases the state.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_abandoned_explicit_shutdown_hands_over_on_drop() {
        let runtime = empty_runtime("abandoned-shutdown").await;
        let modules = Arc::clone(&runtime.module_runtime);
        let state = Arc::downgrade(&modules);
        let settlement = Arc::clone(&runtime.retained_teardown_report);
        let held = modules.lock().await;
        let attempt =
            tokio::time::timeout(std::time::Duration::from_millis(500), runtime.shutdown()).await;
        assert!(attempt.is_err(), "the shutdown is held at module teardown");
        assert!(
            !runtime.teardown_settled.load(Ordering::SeqCst),
            "an entered but abandoned shutdown has not settled"
        );
        drop(held);
        drop(modules);
        drop(runtime);
        let settled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while settlement.get().is_none() || state.strong_count() > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            settled.is_ok(),
            "the retained teardown resumes and finishes"
        );
        // The cleanup resumed rather than the state merely dropping: the
        // retained teardown ran the shutdown through module teardown (the
        // phase the abandoned call never reached) and reported its outcome.
        let report = settlement.get().expect("the retained teardown reported");
        assert_eq!(
            report.module_shutdown.physical_cleanup.phase,
            crate::ModuleCleanupPhase::Settled,
            "module teardown ran to settlement"
        );
        // The abandoned call had already stopped the mob, so the resumed
        // shutdown reports that phase as it found it; module teardown above
        // is the phase that proves the cleanup resumed.
        tracing::debug!(terminal = ?report.mob_terminal_shutdown, "resumed teardown");
    }

    fn injected_refusal(detail: &str) -> MobRuntimeError {
        MobRuntimeError::Mob(meerkat_mob::MobError::Internal(detail.to_string()))
    }

    /// Teardown sends the mob's Stop once and returns the machine's refusal
    /// as is. A completed mob refuses Stop with `InvalidTransition`; the old
    /// quiescing loop answered that class by cancelling member work and
    /// re-sending the stop every 250 ms for 10 s. With the clock paused, any
    /// such wait advances virtual time, so the teardown must take none.
    #[tokio::test(flavor = "current_thread")]
    async fn teardown_sends_stop_once_and_returns_the_refusal() {
        let runtime = empty_runtime("stop-once-refusal").await;
        runtime
            .mob_handle()
            .complete()
            .await
            .expect("the mob completes");

        tokio::time::pause();
        let started = tokio::time::Instant::now();
        let outcome = runtime.stop_mob_for_teardown().await;
        let waited = started.elapsed();
        tokio::time::resume();

        match outcome {
            MobStopOutcome::Failed(MobRuntimeError::Mob(
                meerkat_mob::MobError::InvalidTransition { .. },
            )) => {}
            other => panic!("a completed mob refuses Stop with InvalidTransition: {other:?}"),
        }
        assert_eq!(
            waited,
            Duration::ZERO,
            "teardown re-sent or waited on the stop instead of returning the refusal"
        );
    }

    /// The outcome vocabulary is what callers branch on, and the two
    /// non-failure outcomes must stay TELLABLE APART: teardown may continue
    /// after proceeding without an interrupt, but that is not a clean stop.
    /// Collapsing them is the false-success shape this item refuses.
    #[test]
    fn proceeding_without_an_interrupt_is_never_reported_as_a_clean_stop() {
        let proceeded = MobStopOutcome::ProceededWithoutInterrupt {
            waited_ms: 10_000,
            member: Some("019e3c52-0f1b-73d3-a5c7-4b21c2bbf131".to_string()),
            error: "Runtime not ready: attached".to_string(),
        };

        assert!(MobStopOutcome::Stopped.teardown_may_proceed());
        assert!(MobStopOutcome::Stopped.stopped_cleanly());

        assert!(
            proceeded.teardown_may_proceed(),
            "teardown must not be blocked by a readiness state"
        );
        assert!(
            !proceeded.stopped_cleanly(),
            "proceeding without an interrupt must never read as a clean stop"
        );

        let failed = MobStopOutcome::Failed(injected_refusal("actor task dropped"));
        assert!(!failed.teardown_may_proceed());
        assert!(!failed.stopped_cleanly());
    }
}

#[cfg(test)]
mod shutdown_horizon_tests {
    /// The published shutdown horizon must not silently collapse.
    ///
    /// `shutdown_horizon()` returns
    /// `gateway_composition::GATEWAY_RUNTIME_SHUTDOWN_TIMEOUT` directly - one
    /// expression, so accessor-vs-constant divergence is visible in the diff
    /// rather than needing a test. What a diff does NOT make obvious is the
    /// constant shrinking, and that is what this guards.
    ///
    /// It matters because the failure is asymmetric. Two independent consumers
    /// bounded this budget 4x and 11x too tightly on the same night
    /// (2026-08-31) while it was merely a documented `pub const`. Now that the
    /// runtime advertises it, an embedder who sizes their wait on the
    /// advertised figure is stranded if it drops beneath what shutdown
    /// actually needs - and they would be MORE confident while being wrong,
    /// because they asked.
    #[test]
    fn the_published_shutdown_horizon_does_not_silently_collapse() {
        const ADVERTISED: std::time::Duration =
            crate::gateway_composition::GATEWAY_RUNTIME_SHUTDOWN_TIMEOUT;
        assert!(
            ADVERTISED.as_secs() >= 300,
            "the published shutdown horizon collapsed to {ADVERTISED:?}. It \
             covers two provider-callback windows plus drains and reap margin; \
             shrinking it below that strands every embedder that sized their \
             wait on the advertised figure"
        );
    }
}

#[cfg(test)]
mod run_failed_crash_error_tests {
    use super::run_failed_crash_error;

    /// The ERROR line names the typed failure: class, the stable
    /// `reason_type`, and meerkat's message, after the event id it always
    /// carried.
    #[test]
    fn names_class_reason_and_message_from_the_error_report() {
        let payload = serde_json::json!({
            "type": "run_failed",
            "session_id": "01a0-session",
            "error_report": {
                "class": "llm",
                "reason": { "reason_type": "llm_auth_error" },
                "message": "LLM error: authentication failed (401)"
            }
        });
        assert_eq!(
            run_failed_crash_error("evt-agent-7", Some(&payload)),
            "agent run failed (event_id: evt-agent-7) class=llm reason=llm_auth_error: \
             LLM error: authentication failed (401)"
        );
    }

    /// A report without a typed reason still names class and message.
    #[test]
    fn omits_the_reason_when_the_report_carries_none() {
        let payload = serde_json::json!({
            "error_report": { "class": "internal", "message": "runner gave up" }
        });
        assert_eq!(
            run_failed_crash_error("evt-1", Some(&payload)),
            "agent run failed (event_id: evt-1) class=internal: runner gave up"
        );
    }

    /// Without a report (a payload-less envelope, or a producer that
    /// predates it) the line is exactly what it was before.
    #[test]
    fn falls_back_to_the_bare_event_id_without_a_report() {
        assert_eq!(
            run_failed_crash_error("evt-2", None),
            "agent run failed (event_id: evt-2)"
        );
        let unrelated = serde_json::json!({ "type": "run_failed", "session_id": "s" });
        assert_eq!(
            run_failed_crash_error("evt-3", Some(&unrelated)),
            "agent run failed (event_id: evt-3)"
        );
    }
}

/// The drain waits for room in the event log instead of dropping events.
/// These rows hold that wait to its bounds: a drain waits only for an event
/// it is about to take, one call takes only the backlog it found (and the
/// drain task goes again at once while events stay queued), serving that
/// ends never cuts a drain off in the middle of an event, and a drain
/// waiting on a store that never answers keeps neither shutdown nor
/// `run()`'s shutdown signal from ending.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod event_log_intake_tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::{Arc, Mutex, OnceLock};

    use tokio::sync::mpsc::Sender;

    use crate::types::{EventEnvelope, ModuleEvent, UnifiedEvent};
    use crate::unified_runtime::{
        EventLogConfig, EventLogError, EventLogStore, EventQuery, ForwardedMemberEvent,
        PersistedEvent,
    };

    type AppendFuture<'a> = Pin<Box<dyn Future<Output = Result<(), EventLogError>> + Send + 'a>>;
    type QueryFuture<'a> =
        Pin<Box<dyn Future<Output = Result<Vec<PersistedEvent>, EventLogError>> + Send + 'a>>;

    /// An event log store whose writes never answer.
    struct StalledStore;

    impl EventLogStore for StalledStore {
        fn append_batch(&self, _events: Vec<PersistedEvent>) -> AppendFuture<'_> {
            Box::pin(std::future::pending())
        }

        fn query(&self, _query: EventQuery) -> QueryFuture<'_> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    /// An event log store that keeps the ids of the events it is given.
    #[derive(Clone, Default)]
    struct RecordingStore(Arc<Mutex<Vec<String>>>);

    impl RecordingStore {
        fn ids(&self) -> Vec<String> {
            self.0.lock().expect("recorded ids").clone()
        }
    }

    impl EventLogStore for RecordingStore {
        fn append_batch(&self, events: Vec<PersistedEvent>) -> AppendFuture<'_> {
            self.0
                .lock()
                .expect("recorded ids")
                .extend(events.into_iter().map(|event| event.id));
            Box::pin(async { Ok(()) })
        }

        fn query(&self, _query: EventQuery) -> QueryFuture<'_> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    /// An event log store that puts a new event on the router for each event
    /// it writes, while its refills last: a forwarder that refills the router
    /// as fast as a slow store lets the drain empty it.
    struct RefillingStore {
        router: Arc<OnceLock<Sender<ForwardedMemberEvent>>>,
        refills: AtomicUsize,
    }

    impl EventLogStore for RefillingStore {
        fn append_batch(&self, events: Vec<PersistedEvent>) -> AppendFuture<'_> {
            if let Some(router) = self.router.get() {
                for _ in &events {
                    let Ok(left) =
                        self.refills
                            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                                left.checked_sub(1)
                            })
                    else {
                        break;
                    };
                    let _ = router.try_send(forwarded(&format!("evt-refill-{left}")));
                }
            }
            Box::pin(async { Ok(()) })
        }

        fn query(&self, _query: EventQuery) -> QueryFuture<'_> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    /// Events queued on the router for the drain.
    const QUEUED: usize = 8;
    /// What a log of `batch_size: 1` over a stalled store holds: one event in
    /// the write that never answers and four in its intake buffer.
    const HELD_BY_THE_LOG: usize = 5;

    fn forwarded(id: &str) -> ForwardedMemberEvent {
        ForwardedMemberEvent {
            envelope: EventEnvelope {
                event_id: id.to_string(),
                source: "module".to_string(),
                timestamp_ms: 1,
                event: UnifiedEvent::Module(ModuleEvent {
                    module: "intake-test".into(),
                    event_type: "tick".into(),
                    payload: serde_json::Value::Null,
                }),
            },
            alert: None,
        }
    }

    /// A runtime whose event log (`batch_size: 1`) writes to `store`, with
    /// `drain_window` as its shutdown drain window, and the sender of the
    /// router its drains read.
    async fn runtime_over(
        mob_id: &str,
        store: Box<dyn EventLogStore>,
        drain_window: Duration,
    ) -> (UnifiedRuntime, Sender<ForwardedMemberEvent>) {
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n"
        ))
        .expect("definition parses");
        let runtime = UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(Arc::new(meerkat_client::TestClient::for_provider(
                meerkat_core::Provider::OpenAI,
            )))
            .event_log(EventLogConfig {
                store,
                filter: None,
                batch_size: 1,
                flush_interval: Duration::from_millis(50),
            })
            .drain_timeout(drain_window)
            .build()
            .await
            .expect("runtime builds");
        let router = runtime.install_test_event_ingress().await;
        (runtime, router)
    }

    /// Queue `count` events named `{prefix}-{n}` on the router.
    async fn queue(router: &Sender<ForwardedMemberEvent>, prefix: &str, count: usize) {
        for n in 0..count {
            router
                .send(forwarded(&format!("{prefix}-{n}")))
                .await
                .expect("queue an event for the drain");
        }
    }

    /// The events still queued on the router.
    fn queued(router: &Sender<ForwardedMemberEvent>) -> usize {
        router.max_capacity() - router.capacity()
    }

    /// A runtime over a stalled event log with [`QUEUED`] events on its
    /// router, and that router's sender.
    async fn runtime_over_a_stalled_event_log(
        mob_id: &str,
    ) -> (UnifiedRuntime, Sender<ForwardedMemberEvent>) {
        let (runtime, router) =
            runtime_over(mob_id, Box::new(StalledStore), Duration::from_millis(200)).await;
        queue(&router, "evt-stalled-log", QUEUED).await;
        (runtime, router)
    }

    /// Wait (bounded) until `condition` holds.
    async fn until(what: &str, condition: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(tokio::time::Instant::now() < deadline, "{what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Wait (bounded) until a drain has handed the log all it can hold and
    /// waits for room, holding the router, with the rest still queued.
    async fn until_a_drain_waits_for_room(router: &Sender<ForwardedMemberEvent>) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while queued(router) != QUEUED - HELD_BY_THE_LOG {
            assert!(
                tokio::time::Instant::now() < deadline,
                "a drain takes only what the stalled log can hold, then waits for room: \
                 {} of {QUEUED} events still queued",
                queued(router)
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The ids of the test's events among `events`.
    fn intake_ids(events: &[EventEnvelope<UnifiedEvent>]) -> Vec<String> {
        events
            .iter()
            .filter(|envelope| match &envelope.event {
                UnifiedEvent::Module(event) => event.module == "intake-test",
                UnifiedEvent::Agent { .. } => false,
            })
            .map(|envelope| envelope.event_id.clone())
            .collect()
    }

    /// A drain outside the shutdown (here a host's own drain task) waits for
    /// room in the stalled log while it holds the event router. Shutdown
    /// still ends within its drain window: before it takes the router, it
    /// closes the log's intake, which ends that wait. Its report shows the
    /// window ran out with events left behind.
    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_ends_while_a_drain_waits_on_a_stalled_event_log() {
        let (runtime, router) =
            runtime_over_a_stalled_event_log("event-log-stalled-shutdown").await;
        let runtime = Arc::new(runtime);
        let host_drain = tokio::spawn({
            let runtime = Arc::clone(&runtime);
            async move { runtime.drain_mob_agent_events().await }
        });
        until_a_drain_waits_for_room(&router).await;

        let report = tokio::time::timeout(Duration::from_secs(30), runtime.shutdown())
            .await
            .expect("shutdown ends while a drain waits on the stalled event log");
        assert!(
            report.drain.timed_out,
            "the drain window ran out with events the log could not take: {:?}",
            report.drain
        );
        let drained = tokio::time::timeout(Duration::from_secs(10), host_drain)
            .await
            .expect("the waiting drain ends once shutdown closes the log's intake")
            .expect("the drain task joins");
        assert!(
            drained.is_ok(),
            "the waiting drain ends cleanly: {drained:?}"
        );
    }

    /// `run()`'s own drain waits for room in the stalled log. The drain runs
    /// beside the server, so the server still sees the shutdown signal, and
    /// the shutdown that follows ends within its drain window.
    #[tokio::test(flavor = "multi_thread")]
    async fn run_sees_its_shutdown_signal_while_its_drain_waits_on_a_stalled_event_log() {
        let (runtime, router) = runtime_over_a_stalled_event_log("event-log-stalled-run").await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback listener");
        let decisions = RuntimeDecisionState::local_console(crate::ConsolePolicy::default(), None);
        let (signal, signalled) = tokio::sync::oneshot::channel::<()>();
        let run = tokio::spawn(async move {
            runtime
                .run(listener, decisions, async {
                    let _ = signalled.await;
                })
                .await
        });
        until_a_drain_waits_for_room(&router).await;

        signal.send(()).expect("run() is still serving");
        let report = tokio::time::timeout(Duration::from_secs(30), run)
            .await
            .expect("run() sees its signal while its drain waits on the stalled event log")
            .expect("the run task joins");
        assert!(
            report.serve_result.is_ok(),
            "serving ends cleanly: {:?}",
            report.serve_result
        );
        assert!(
            report.shutdown.drain.timed_out,
            "the drain window ran out with events the log could not take: {:?}",
            report.shutdown.drain
        );
    }

    /// A drain waits for room only for an event it is about to take. Here
    /// the router holds exactly what the stalled log can hold: the
    /// shutdown's drain hands the log all five and then finds nothing
    /// queued, so it returns instead of waiting for room the store never
    /// frees. The drain window does not run out, and the report attests
    /// cleanup.
    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_attests_cleanup_when_a_stalled_event_log_holds_all_that_was_queued() {
        let (runtime, router) = runtime_over(
            "event-log-stalled-nothing-queued",
            Box::new(StalledStore),
            Duration::from_secs(5),
        )
        .await;
        queue(&router, "evt-held", HELD_BY_THE_LOG).await;

        let report = tokio::time::timeout(Duration::from_secs(30), runtime.shutdown())
            .await
            .expect("shutdown ends over the stalled event log");
        assert_eq!(
            intake_ids(&runtime.module_events().await).len(),
            HELD_BY_THE_LOG,
            "the shutdown's drain took every queued event"
        );
        assert!(
            !report.drain.timed_out,
            "with nothing queued, the drain does not wait out its window: {:?}",
            report.drain
        );
        assert!(
            report.cleanup_completed(),
            "shutdown attests cleanup: {report:?}"
        );
    }

    /// Refills a drain may meet in one call; the row only needs more than
    /// the backlog.
    const REFILLS: usize = 32;

    /// One drain call takes the backlog it found on entry. The store puts
    /// an event back on the router for each event it writes, so the router
    /// never empties while the drain runs: a drain that ran until the router
    /// was empty would take every refill too, and against a store slower
    /// than arrivals it would never return.
    #[tokio::test(flavor = "multi_thread")]
    async fn one_drain_call_takes_the_backlog_it_found_while_the_router_refills() {
        let router_slot = Arc::new(OnceLock::new());
        let store = RefillingStore {
            router: Arc::clone(&router_slot),
            refills: AtomicUsize::new(REFILLS),
        };
        let (runtime, router) = runtime_over(
            "event-log-refilled-router",
            Box::new(store),
            Duration::from_millis(200),
        )
        .await;
        assert!(
            router_slot.set(router.clone()).is_ok(),
            "the store gets the router once"
        );
        queue(&router, "evt-backlog", QUEUED).await;

        tokio::time::timeout(Duration::from_secs(10), runtime.drain_mob_agent_events())
            .await
            .expect("one drain call returns while the router refills")
            .expect("the drain");
        let taken = intake_ids(&runtime.module_events().await);
        assert_eq!(
            taken.len(),
            QUEUED,
            "one call takes the backlog it found on entry and no refill: {taken:?}"
        );
        assert!(queued(&router) > 0, "the refills wait for the next call");
        let _ = runtime.shutdown().await;
    }

    /// Serving that ends while a drain is in the middle of an event lets
    /// that event finish, and the drain takes no event after it: the drain
    /// is told to stop and awaited, and stops before its next event. Here
    /// the drain has handed the first of two events to the log and waits for
    /// the module events' lock, which the row holds until serving has ended.
    /// Once the lock is free, the first event reaches the module events too,
    /// and the second stays queued for the shutdown's drains.
    #[tokio::test(flavor = "multi_thread")]
    async fn serving_that_ends_mid_event_lets_the_drain_finish_that_event() {
        let log = RecordingStore::default();
        let (runtime, router) = runtime_over(
            "event-log-serving-ends-mid-event",
            Box::new(log.clone()),
            Duration::from_millis(200),
        )
        .await;
        let runtime = Arc::new(runtime);
        queue(&router, "evt-mid-event", 2).await;
        let modules = Arc::clone(&runtime.module_runtime);
        let held = modules.lock().await;

        let (signal, signalled) = tokio::sync::oneshot::channel::<()>();
        let ended = Arc::new(AtomicBool::new(false));
        let serving = tokio::spawn({
            let runtime = Arc::clone(&runtime);
            let ended = Arc::clone(&ended);
            async move {
                runtime
                    .serve_while_draining(async move {
                        let _ = signalled.await;
                        ended.store(true, Ordering::SeqCst);
                    })
                    .await;
            }
        });
        until(
            "the drain hands the first event to the log, then waits for the module events' lock",
            || log.ids() == ["evt-mid-event-0"],
        )
        .await;
        signal.send(()).expect("still serving");
        until("serving ends", || ended.load(Ordering::SeqCst)).await;
        drop(held);

        tokio::time::timeout(Duration::from_secs(10), serving)
            .await
            .expect("serving returns once its drain has finished the event")
            .expect("the serving task joins");
        let module_ids = intake_ids(&runtime.module_events().await);
        assert_eq!(
            module_ids,
            ["evt-mid-event-0"],
            "the event the drain was in the middle of reaches the module events, \
             and the drain takes no event after it"
        );
        assert_eq!(queued(&router), 1, "the second event stays queued");
        let _ = runtime.shutdown().await;
    }

    /// Events a producer streams through the router in the drain task row:
    /// many router loads (the test router holds 16).
    const STREAM: usize = 200;

    /// The drain task drains again at once after a drain that took its whole
    /// backlog and left events queued, not a tick later. A producer streams
    /// many router loads through the router while the task drains it. The
    /// clock is paused, so it moves only while every task waits: the whole
    /// stream is taken within the task's first 25 ms tick, where a tick
    /// between router loads would take a tick per load.
    #[tokio::test(flavor = "current_thread")]
    async fn the_drain_task_takes_a_stream_of_router_loads_without_a_tick_between_them() {
        let (runtime, router) = runtime_over(
            "event-log-drain-task-stream",
            Box::new(RecordingStore::default()),
            Duration::from_millis(200),
        )
        .await;
        let runtime = Arc::new(runtime);
        tokio::time::pause();
        let started = tokio::time::Instant::now();
        let producer = tokio::spawn({
            let router = router.clone();
            async move { queue(&router, "evt-stream", STREAM).await }
        });
        let drain_task = Arc::clone(&runtime).spawn_event_drain_task();
        while intake_ids(&runtime.module_events().await).len() < STREAM {
            assert!(
                started.elapsed() < Duration::from_mins(1),
                "the drain task takes the stream"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let took = started.elapsed();
        drain_task.abort();
        let _ = drain_task.await;
        producer.await.expect("the producer joins");
        tokio::time::resume();
        assert!(
            took < Duration::from_millis(50),
            "the stream is taken within the drain task's first tick, not a tick per \
             router load: {took:?}"
        );
        let _ = runtime.shutdown().await;
    }
}
