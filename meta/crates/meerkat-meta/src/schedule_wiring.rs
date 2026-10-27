//! Wire meerkat's per-session schedule tools + firing host into the gateways.
//!
//! A mob profile's `tools.schedule = true` maps (in `meerkat-mob`) to
//! `override_schedule = Enable`, which already resolves on. But the schedule
//! tools only compose if the agent factory's `default_schedule_tools` slot
//! holds a dispatcher — and the meta gateways never populated it, so the
//! `meerkat_schedule_*` surface was silently absent even when the capability
//! was declared and allow-listed.
//!
//! [`attach_schedule_tools`] fills that slot (members can author schedules) and
//! [`spawn_schedule_host`] runs the runtime-backed driver (authored schedules
//! actually fire: at due time it materializes a session and runs the prompt as
//! a real agent turn). Both back onto a single durable [`ScheduleService`].
//!
//! [`steward_dream_runnable_host`] + [`ensure_steward_dream_schedule`] register
//! the memory steward's dream as a host-runnable schedule target (§8.5 /
//! upstream ask 7), so the dream fires as a durable, misfire-aware occurrence
//! through the same host instead of a bare in-process interval loop.
//!
//! This is Meta's only scheduling authority. It owns durable definitions,
//! occurrence claims, firing records, and target delivery.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use meerkat::surface::{
    NoopScheduleMobHost, ScheduleHostHandle, SurfaceScheduleMobHost, immediate_completed_dispatch,
    immediate_delivery_failure, parse_mob_member_schedule_identity,
    spawn_runtime_backed_schedule_host_with_mobs,
};
use meerkat::{
    Config, CreateScheduleRequest, FactoryAgentBuilder, HostRunnable, HostRunnableError,
    HostRunnableInvocation, HostRunnableName, HostRunnableOutcome, HostRunnableRegistry,
    HostRunnableTargetBinding, IntervalTriggerSpec, MobTargetBinding, Occurrence, OccurrenceFilter,
    OccurrencePhase, PersistentSessionService, Schedule, ScheduleFilter, SchedulePhase,
    ScheduleRunnableHost, ScheduleService, ScheduleStore, ScheduleToolDispatcher,
    ScheduledMobAction, ScheduledSessionAction, SessionAgentBuilder, SessionTargetBinding,
    SqliteScheduleStore, TargetBinding, TriggerSpec, UpdateScheduleRequest,
};
use meerkat_core::service::SessionBuildOptions;
use meerkat_mob::runtime::MobHandle;
use meerkat_mob_mcp::{MobMcpScheduleHost, MobMcpState};
use meerkat_runtime::MeerkatMachine;
use serde_json::{Map, Value};

use crate::identity_first::gateway_bridges::CallbackBridge;
use crate::memory::steward::StewardEngine;
use crate::mob_announcements::{AnnouncementSource, SourceRunners, SourceState};

/// File name for the durable schedule store, kept beside the runtime DB so a
/// gateway and a library-mode runtime pointed at the same dir share state.
pub const SCHEDULE_STORE_FILE: &str = "schedule.sqlite";

/// JSON-RPC callback method the gateway sends to the SDK host when a
/// host-runnable schedule target registered via
/// `runtime_options.host_runnables` fires. Params:
/// `{runnable, occurrence: {schedule_id, occurrence_id, due_at, payload?}}`.
pub const SCHEDULE_FIRE_CALLBACK_METHOD: &str = "callback/schedule_fire";

/// Reserved host-runnable name for the memory steward's scheduled dream
/// (§8.5 / upstream ask 7). Stable across boots: the find-or-create schedule
/// keys idempotency on a target that names this runnable.
pub const STEWARD_DREAM_RUNNABLE: &str = "meta.memory.steward.dream";

/// Schedule name for the steward dream, used only for operator legibility in
/// schedule listings — idempotency keys on the host-runnable target, not this.
pub(crate) const STEWARD_DREAM_SCHEDULE_NAME: &str = "meta-memory-steward-dream";

/// A [`HostRunnable`] that runs one guarded steward dream attempt. Registered
/// with the schedule host so the dream fires as a durable, misfire-aware
/// schedule occurrence instead of a bare in-process interval loop.
struct StewardDreamRunnable {
    engine: Arc<StewardEngine>,
}

#[async_trait]
impl HostRunnable for StewardDreamRunnable {
    async fn run(
        &self,
        _invocation: HostRunnableInvocation,
    ) -> Result<HostRunnableOutcome, HostRunnableError> {
        // `dream_now` is fully self-gating (enabled / min-signals / budget)
        // and never returns an error — a skipped dream is a normal outcome,
        // not an occurrence failure. Always report completion so the
        // occurrence lands terminal-complete.
        self.engine.dream_now().await;
        Ok(HostRunnableOutcome::completed())
    }
}

/// Build a host-runnable registry exposing the steward's dream, when a steward
/// engine is present. Returned as `Arc<dyn ScheduleRunnableHost>` for
/// [`spawn_schedule_host`]'s `runnable_host`; `None` when there is no steward
/// to drive (the schedule host then serves session/mob targets only).
#[must_use]
// The runnable name is a non-empty const and registration into a fresh
// registry cannot duplicate — both expects are structurally infallible.
#[allow(clippy::expect_used)]
pub fn steward_dream_runnable_host(
    steward: Option<Arc<StewardEngine>>,
) -> Option<Arc<dyn ScheduleRunnableHost>> {
    let engine = steward?;
    let mut registry = HostRunnableRegistry::new();
    let name = HostRunnableName::parse(STEWARD_DREAM_RUNNABLE)
        .expect("STEWARD_DREAM_RUNNABLE is a non-empty runnable name");
    registry
        .register(name, Arc::new(StewardDreamRunnable { engine }))
        .expect("first registration into a fresh registry cannot duplicate");
    Some(Arc::new(registry))
}

/// A [`HostRunnable`] whose fire forwards over the SDK callback bridge as
/// [`SCHEDULE_FIRE_CALLBACK_METHOD`], so the app process (Python/TypeScript)
/// runs the occurrence deterministically — no LLM turn is involved.
///
/// A bridge error (handler raised, no handler registered for the name, host
/// gone, callback timeout) is reported as a typed runnable FAILURE so the
/// schedule store records the occurrence attempt as failed rather than
/// silently completing. The app's JSON result is logged and dropped:
/// upstream's `HostRunnableOutcome` deliberately carries no success payload
/// (occurrence completion consumes no detail).
struct CallbackHostRunnable {
    bridge: Arc<dyn CallbackBridge>,
}

#[async_trait]
impl HostRunnable for CallbackHostRunnable {
    async fn run(
        &self,
        invocation: HostRunnableInvocation,
    ) -> Result<HostRunnableOutcome, HostRunnableError> {
        let mut occurrence = Map::new();
        occurrence.insert(
            "schedule_id".to_string(),
            Value::String(invocation.schedule_id.to_string()),
        );
        occurrence.insert(
            "occurrence_id".to_string(),
            Value::String(invocation.occurrence_id.to_string()),
        );
        occurrence.insert(
            "due_at".to_string(),
            Value::String(invocation.trigger_time.to_rfc3339()),
        );
        if let Some(params) = invocation.params.as_deref() {
            match serde_json::from_str::<Value>(params.get()) {
                Ok(payload) => {
                    occurrence.insert("payload".to_string(), payload);
                }
                Err(error) => {
                    // The binding canonicalizes params at every ingress, so a
                    // non-JSON payload here is store corruption — fail the
                    // occurrence rather than firing with silently dropped params.
                    return Err(HostRunnableError::Failed {
                        detail: format!("host-runnable params are not valid JSON: {error}"),
                    });
                }
            }
        }
        let params = serde_json::json!({
            "runnable": invocation.runnable.as_str(),
            "occurrence": Value::Object(occurrence),
        });
        match self
            .bridge
            .call(SCHEDULE_FIRE_CALLBACK_METHOD, params)
            .await
        {
            Ok(result) => {
                tracing::debug!(
                    runnable = %invocation.runnable,
                    occurrence_id = %invocation.occurrence_id,
                    %result,
                    "callback schedule fire completed"
                );
                Ok(HostRunnableOutcome::completed())
            }
            Err(detail) => Err(HostRunnableError::Failed { detail }),
        }
    }
}

/// Compose the gateway's schedule runnable host: the steward dream (when a
/// steward engine is present) plus SDK-registered callback runnables
/// (`runtime_options.host_runnables`), each forwarding its fire over the
/// callback bridge as [`SCHEDULE_FIRE_CALLBACK_METHOD`].
///
/// `Ok(None)` when there is nothing to register (the schedule host then
/// serves session/mob targets only). `Err` on a duplicate runnable name —
/// including a callback runnable colliding with the reserved
/// [`STEWARD_DREAM_RUNNABLE`] name.
pub fn gateway_runnable_host(
    steward: Option<Arc<StewardEngine>>,
    callback_runnables: Option<(Arc<dyn CallbackBridge>, Vec<HostRunnableName>)>,
) -> Result<Option<Arc<dyn ScheduleRunnableHost>>, String> {
    gateway_runnable_host_with(steward, callback_runnables, Vec::new())
}

/// A Rust host's own schedule runnables, each under the name a
/// host-runnable target calls it by.
pub type HostRunnables = Vec<(HostRunnableName, Arc<dyn HostRunnable>)>;

/// [`gateway_runnable_host`], with the Rust host's own runnables
/// (`GatewayHostComposition::host_runnables`) registered beside the
/// steward's and the SDK's callback runnables. Core hands each fire's
/// [`HostRunnableInvocation`] to such a runnable unchanged, its delivery
/// idempotency key included. `Err` on a duplicate name, as for the others,
/// and on the steward's reserved name.
// The steward runnable name is a non-empty const, so its parse is infallible.
#[allow(clippy::expect_used)]
pub fn gateway_runnable_host_with(
    steward: Option<Arc<StewardEngine>>,
    callback_runnables: Option<(Arc<dyn CallbackBridge>, Vec<HostRunnableName>)>,
    host_runnables: HostRunnables,
) -> Result<Option<Arc<dyn ScheduleRunnableHost>>, String> {
    let mut registry = HostRunnableRegistry::new();
    let mut registered = false;
    if let Some(engine) = steward {
        let name = HostRunnableName::parse(STEWARD_DREAM_RUNNABLE)
            .expect("STEWARD_DREAM_RUNNABLE is a non-empty runnable name");
        registry
            .register(name, Arc::new(StewardDreamRunnable { engine }))
            .map_err(|error| error.to_string())?;
        registered = true;
    }
    if let Some((bridge, names)) = callback_runnables {
        for name in names {
            registry
                .register(
                    name,
                    Arc::new(CallbackHostRunnable {
                        bridge: Arc::clone(&bridge),
                    }),
                )
                .map_err(|error| error.to_string())?;
            registered = true;
        }
    }
    for (name, runnable) in host_runnables {
        // The steward's name is Meta's, with or without a steward here.
        if name.as_str() == STEWARD_DREAM_RUNNABLE {
            return Err(format!(
                "host runnable name '{name}' is reserved for the memory steward"
            ));
        }
        registry
            .register(name, runnable)
            .map_err(|error| error.to_string())?;
        registered = true;
    }
    Ok(registered.then(|| Arc::new(registry) as Arc<dyn ScheduleRunnableHost>))
}

/// The label every source runner schedule Meta makes carries. A schedule
/// without it is never Meta's to change or end, whatever it targets: a
/// host's own schedule of its collector stays the host's.
pub const SOURCE_RUNNER_LABEL: &str = "mob_announcements.source_runner";

/// The params a source runner's schedule carries: the source its collector
/// reads.
fn source_runner_params(source: &AnnouncementSource) -> Value {
    serde_json::json!({
        "mob_id": source.mob_id.as_str(),
        "source_id": source.source_id.as_str(),
    })
}

/// A host-runnable target's runnable and parsed params, if it is one.
fn host_runnable_target(target: &TargetBinding) -> Option<(HostRunnableName, Value)> {
    let TargetBinding::HostRunnable(binding) = target else {
        return None;
    };
    let params = serde_json::from_str::<Value>(binding.params.as_ref()?.get()).ok()?;
    Some((binding.runnable.clone(), params))
}

/// A source runner schedule Meta made, with its runnable and the params
/// naming its source.
type RunnerSchedule = (Schedule, HostRunnableName, Value);

/// Every source runner schedule Meta made on `service`.
async fn runner_schedules(service: &ScheduleService) -> Result<Vec<RunnerSchedule>, String> {
    Ok(service
        .list()
        .await
        .map_err(|error| format!("list schedules: {error}"))?
        .into_iter()
        .filter(|schedule| schedule.config.labels.contains_key(SOURCE_RUNNER_LABEL))
        .filter_map(|schedule| {
            let (runnable, params) = host_runnable_target(&schedule.target)?;
            Some((schedule, runnable, params))
        })
        .collect())
}

/// Bring `source`'s runner schedule in line with its record, given the
/// runner schedules Meta made for it (`theirs`). A source with a runner has
/// exactly one, keyed on the runner's host runnable AND the params naming
/// the source (never the runnable alone, so sources sharing one collector
/// keep separate schedules), firing every `every_seconds` from `now_utc` on
/// (a source starts collecting when it is declared, not one interval
/// later), under Core's default overlap (skip while the previous fire runs)
/// and misfire (skip a late fire: the next one reads from the cursor anyway)
/// policies. A changed cadence updates that schedule in place, from
/// `now_utc`; a changed runnable replaces it; no runner, or an ended source,
/// ends it. A paused source keeps its schedule: the collector reads the
/// pause and does nothing.
async fn apply_source_runner(
    service: &ScheduleService,
    source: &AnnouncementSource,
    theirs: Vec<RunnerSchedule>,
    now_utc: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let wanted = source
        .runner
        .as_ref()
        .filter(|_| source.state != SourceState::Ended);
    let runnable = wanted
        .map(|runner| HostRunnableName::parse(runner.host_runnable.as_str()))
        .transpose()
        .map_err(|error| format!("source runner host runnable: {error}"))?;
    let mut kept: Option<Schedule> = None;
    for (schedule, target_runnable, _) in theirs {
        if kept.is_none() && runnable.as_ref() == Some(&target_runnable) {
            kept = Some(schedule);
            continue;
        }
        service
            .delete(&schedule.schedule_id)
            .await
            .map_err(|error| format!("end a source runner schedule: {error}"))?;
    }
    let (Some(runner), Some(runnable)) = (wanted, runnable) else {
        return Ok(());
    };
    let every_seconds = runner.every_seconds;
    let trigger = TriggerSpec::Interval(IntervalTriggerSpec {
        start_at_utc: now_utc,
        every_seconds,
        end_at_utc: None,
    });
    match kept {
        Some(schedule) => {
            if matches!(
                &schedule.trigger,
                TriggerSpec::Interval(spec) if spec.every_seconds == every_seconds
            ) {
                return Ok(());
            }
            service
                .update(
                    &schedule.schedule_id,
                    UpdateScheduleRequest {
                        trigger: Some(trigger),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|error| format!("update a source runner schedule: {error}"))?;
        }
        None => {
            let params = meerkat::HostRunnableParams::try_from(source_runner_params(source))
                .map_err(|error| format!("source runner params: {error}"))?;
            service
                .create(CreateScheduleRequest {
                    name: Some(format!(
                        "mob announcement source {} {}",
                        source.mob_id, source.source_id
                    )),
                    description: Some(
                        "Runs a mob announcement source's collector (host-runnable target)"
                            .to_string(),
                    ),
                    trigger,
                    target: TargetBinding::host_runnable(HostRunnableTargetBinding {
                        runnable,
                        params: Some(params),
                    }),
                    misfire_policy: meerkat::MisfirePolicy::default(),
                    overlap_policy: meerkat::OverlapPolicy::default(),
                    missing_target_policy: meerkat::MissingTargetPolicy::default(),
                    labels: BTreeMap::from([(SOURCE_RUNNER_LABEL.to_string(), "1".to_string())]),
                    planning_horizon_days: None,
                    planning_horizon_occurrences: None,
                })
                .await
                .map_err(|error| format!("create a source runner schedule: {error}"))?;
        }
    }
    Ok(())
}

/// Bring the durable schedule of `source`'s runner in line with its record
/// (see [`apply_source_runner`] for the rules). Only schedules carrying
/// [`SOURCE_RUNNER_LABEL`] are read, changed or ended.
pub async fn reconcile_source_runner(
    service: &ScheduleService,
    source: &AnnouncementSource,
    now_utc: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let params = source_runner_params(source);
    let theirs = runner_schedules(service)
        .await?
        .into_iter()
        .filter(|(_, _, target_params)| *target_params == params)
        .collect();
    apply_source_runner(service, source, theirs, now_utc).await
}

/// Bring every source runner schedule on `service` in line with `live`, the
/// sources that are not ended, from one read of the schedules: each live
/// source's is kept, made or changed as [`reconcile_source_runner`] would,
/// and a runner schedule naming no live source (its source ended, or is
/// gone) ends. Every source is tried; the failures come back together.
pub async fn reconcile_source_runners(
    service: &ScheduleService,
    live: &[AnnouncementSource],
    now_utc: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let mut left = runner_schedules(service).await?;
    let mut failures = Vec::new();
    for source in live {
        let params = source_runner_params(source);
        let (theirs, others): (Vec<RunnerSchedule>, Vec<RunnerSchedule>) = left
            .into_iter()
            .partition(|(_, _, target_params)| *target_params == params);
        left = others;
        if let Err(error) = apply_source_runner(service, source, theirs, now_utc).await {
            failures.push(format!("{} {}: {error}", source.mob_id, source.source_id));
        }
    }
    for (schedule, _, _) in left {
        if let Err(error) = service.delete(&schedule.schedule_id).await {
            failures.push(format!(
                "end runner schedule {}: {error}",
                schedule.schedule_id
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// Sources' runners kept on this gateway's schedule service.
pub struct ScheduleSourceRunners {
    service: ScheduleService,
}

impl ScheduleSourceRunners {
    pub fn new(service: ScheduleService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl SourceRunners for ScheduleSourceRunners {
    async fn reconcile(&self, source: &AnnouncementSource) -> Result<(), String> {
        reconcile_source_runner(&self.service, source, chrono::Utc::now()).await
    }

    async fn reconcile_all(&self, live: &[AnnouncementSource]) -> Result<(), String> {
        reconcile_source_runners(&self.service, live, chrono::Utc::now()).await
    }
}

/// Find-or-create the durable schedule that drives the steward dream runnable
/// at `cadence`. Idempotent across boots against the persistent schedule
/// store: keyed on the reserved host-runnable target, so a restart reuses the
/// existing schedule (and its planning cursor) rather than stacking duplicate
/// dreams. When the configured cadence changed, the existing schedule's
/// interval is updated in place; otherwise it is left untouched.
// The runnable name is a non-empty const, so its parse is infallible.
#[allow(clippy::expect_used)]
pub async fn ensure_steward_dream_schedule(
    service: &ScheduleService,
    cadence: Duration,
    now_utc: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let every_seconds = cadence.as_secs().max(1);
    let runnable = HostRunnableName::parse(STEWARD_DREAM_RUNNABLE)
        .expect("STEWARD_DREAM_RUNNABLE is a non-empty runnable name");

    let schedules = service
        .list()
        .await
        .map_err(|error| format!("list schedules: {error}"))?;
    let existing = schedules.into_iter().find(|schedule| {
        matches!(
            &schedule.target,
            TargetBinding::HostRunnable(binding) if binding.runnable == runnable
        )
    });

    // First occurrence one interval out (parity with the old loop's initial
    // sleep-then-dream) so a fresh gateway does not dream immediately on boot.
    let start_at_utc = now_utc + chrono::Duration::seconds(every_seconds as i64);
    let trigger = TriggerSpec::Interval(IntervalTriggerSpec {
        start_at_utc,
        every_seconds,
        end_at_utc: None,
    });
    let target = TargetBinding::host_runnable(HostRunnableTargetBinding {
        runnable,
        params: None,
    });

    match existing {
        Some(schedule) => {
            let cadence_unchanged = matches!(
                &schedule.trigger,
                TriggerSpec::Interval(spec) if spec.every_seconds == every_seconds
            );
            if cadence_unchanged {
                return Ok(());
            }
            service
                .update(
                    &schedule.schedule_id,
                    UpdateScheduleRequest {
                        trigger: Some(trigger),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|error| format!("update steward dream schedule cadence: {error}"))?;
            tracing::info!(
                schedule_id = %schedule.schedule_id,
                every_seconds,
                "updated steward dream schedule cadence"
            );
            Ok(())
        }
        None => {
            let created = service
                .create(CreateScheduleRequest {
                    name: Some(STEWARD_DREAM_SCHEDULE_NAME.to_string()),
                    description: Some(
                        "Meta memory steward dream (host-runnable target)".to_string(),
                    ),
                    trigger,
                    target,
                    misfire_policy: meerkat::MisfirePolicy::default(),
                    overlap_policy: meerkat::OverlapPolicy::default(),
                    missing_target_policy: meerkat::MissingTargetPolicy::default(),
                    labels: std::collections::BTreeMap::new(),
                    planning_horizon_days: None,
                    planning_horizon_occurrences: None,
                })
                .await
                .map_err(|error| format!("create steward dream schedule: {error}"))?;
            tracing::info!(
                schedule_id = %created.schedule_id,
                every_seconds,
                "created steward dream schedule (host-runnable target)"
            );
            Ok(())
        }
    }
}

#[derive(Clone, Default)]
pub struct ScheduleMobTargetRegistry {
    state: Arc<RwLock<AdoptedMobState>>,
}

/// The mob state a registry resolves bridge sessions against, held weakly:
/// the runtime's mob runtime owns it, and the schedule tools a session
/// service keeps must not keep a stopped host's mob state alive.
#[derive(Default)]
enum AdoptedMobState {
    /// No mob state was adopted: the runtime has no mobs.
    #[default]
    Unset,
    /// The adopted mob state. It no longer upgrades once its owner dropped
    /// it, when its host stopped.
    Adopted(Weak<MobMcpState>),
}

/// What a registry resolved a bridge session to.
enum MobSessionResolution {
    /// The session's mob and member, or none: no mob state was adopted, or
    /// none of its mobs has the session.
    Resolved(Option<(String, String)>),
    /// The adopted mob state is gone: its host stopped.
    Stopped,
}

impl ScheduleMobTargetRegistry {
    /// Adopt the mob state bridge sessions resolve against, or forget it
    /// with `None`. The registry keeps only a weak reference to it.
    pub fn set_mob_state(&self, state: Option<Arc<MobMcpState>>) {
        if let Ok(mut guard) = self.state.write() {
            *guard = match state {
                Some(state) => AdoptedMobState::Adopted(Arc::downgrade(&state)),
                None => AdoptedMobState::Unset,
            };
        }
    }

    async fn resolve_bridge_session(&self, session_id: &str) -> Option<(String, String)> {
        match self.resolve_mob_session(session_id).await {
            MobSessionResolution::Resolved(found) => found,
            MobSessionResolution::Stopped => None,
        }
    }

    /// Resolve a bridge session against the adopted mob state, holding the
    /// state only for this call's roster walk.
    async fn resolve_mob_session(&self, session_id: &str) -> MobSessionResolution {
        let adopted = match self.state.read().as_deref() {
            Ok(AdoptedMobState::Adopted(state)) => Some(state.upgrade()),
            Ok(AdoptedMobState::Unset) => None,
            // A writer panicked while it adopted a mob state, so which state
            // it adopted is unknown: refuse as for a stopped host rather
            // than let a rewrite candidate through unrewritten.
            Err(_) => return MobSessionResolution::Stopped,
        };
        let state = match adopted {
            None => return MobSessionResolution::Resolved(None),
            Some(None) => return MobSessionResolution::Stopped,
            Some(Some(state)) => state,
        };
        MobSessionResolution::Resolved(bridge_session_member(&state, session_id).await)
    }
}

/// The mob and member a bridge session belongs to in `state`'s mobs.
async fn bridge_session_member(state: &MobMcpState, session_id: &str) -> Option<(String, String)> {
    let parsed_session_id = meerkat::SessionId::parse(session_id).ok()?;
    let handles = state.mob_handles_snapshot().await.ok()?;
    for (mob_id, handle) in handles {
        let roster = handle.roster().await;
        if let Some(entry) = roster.find_by_bridge_session_id(&parsed_session_id) {
            return Some((mob_id.to_string(), entry.agent_identity.to_string()));
        }
    }
    None
}

pub struct AttachedScheduleTools {
    pub service: ScheduleService,
    pub mob_target_registry: ScheduleMobTargetRegistry,
    /// Firing-host liveness the write gate consults (Bug C stopgap: no store
    /// may accept a firing-intent write that no driver drains). Gateway
    /// compositions start this `pending` and [`ScheduleFiringHostBinding::bind`]
    /// it once their in-process host spawns; the caller-injected store seam
    /// starts it `external_driver` because firing responsibility rides with
    /// the injector and meta must not rule on a store it does not own.
    pub firing_host_binding: ScheduleFiringHostBinding,
}

/// Shared firing state between a schedule-tools attachment and the firing
/// host that drains its store.
///
/// Bug C (2026-07-08) was a schedule write accepted into a store no driver
/// read: 30 occurrences rotted silently while healthz stayed green. The
/// meta-side stopgap turns that class into a typed refusal at write time:
/// firing-intent tool verbs (`create`, `update`, `resume`) refuse while the
/// binding is `pending`. Reads and rot-reducing verbs (`pause`, `delete`,
/// `get`, `list`, `occurrences`) always pass.
///
/// A candidate launch quiesces the binding instead. It runs no firing host and
/// boots against the state its promoted, authoritative boot adopts, so a
/// schedule change it accepted would land in that state without any authority
/// to make it: every mutating verb (`create`, `update`, `resume`, `pause`,
/// `delete`) refuses, and only reads pass.
#[derive(Clone)]
pub struct ScheduleFiringHostBinding(Arc<std::sync::atomic::AtomicU8>);

const FIRING_HOST_PENDING: u8 = 0;
const FIRING_HOST_BOUND: u8 = 1;
const FIRING_HOST_QUIESCED_CANDIDATE: u8 = 2;

impl ScheduleFiringHostBinding {
    /// An in-process firing host is expected but has not spawned yet.
    /// Firing-intent writes refuse until [`Self::bind`] is called.
    pub fn pending_in_process_host() -> Self {
        Self(Arc::new(std::sync::atomic::AtomicU8::new(
            FIRING_HOST_PENDING,
        )))
    }

    /// The store's driver lives outside this process (caller-injected store,
    /// library mode). Never gates: whether that driver exists is the
    /// injector's ruling to make, not meta's.
    pub fn external_driver() -> Self {
        Self(Arc::new(std::sync::atomic::AtomicU8::new(
            FIRING_HOST_BOUND,
        )))
    }

    /// Record that a firing host now drains the store. A quiesced candidate
    /// binding stays quiesced: a candidate never becomes its store's executor.
    pub fn bind(&self) {
        let _ = self.0.compare_exchange(
            FIRING_HOST_PENDING,
            FIRING_HOST_BOUND,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        );
    }

    /// Record that this attachment belongs to a candidate launch, which takes
    /// no firing authority over its store. It holds for the process:
    /// promotion is a fresh authoritative boot, never an in-process switch.
    pub(crate) fn quiesce_for_candidate(&self) {
        self.0.store(
            FIRING_HOST_QUIESCED_CANDIDATE,
            std::sync::atomic::Ordering::Release,
        );
    }

    pub fn is_bound(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire) == FIRING_HOST_BOUND
    }

    /// Whether a candidate launch quiesced this binding.
    pub(crate) fn is_quiesced_candidate(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire) == FIRING_HOST_QUIESCED_CANDIDATE
    }
}

/// Schedule tool verbs that create or revive firing intent. These are the
/// writes that rot when no driver drains the store; everything else either
/// reads or reduces rot.
const FIRING_INTENT_SCHEDULE_TOOLS: &[&str] = &[
    "meerkat_schedule_create",
    "meerkat_schedule_update",
    "meerkat_schedule_resume",
];

/// Every schedule tool verb that changes durable schedule state. A candidate
/// launch refuses all of them; `get`, `list` and `occurrences` read.
const MUTATING_SCHEDULE_TOOLS: &[&str] = &[
    "meerkat_schedule_create",
    "meerkat_schedule_update",
    "meerkat_schedule_resume",
    "meerkat_schedule_pause",
    "meerkat_schedule_delete",
];

/// Write gate over the schedule tool dispatcher: firing-intent verbs refuse
/// typed while no firing host is bound to the target store, and every
/// mutating verb refuses on a candidate launch.
struct FiringHostGatedScheduleTools {
    inner: Arc<dyn meerkat_core::AgentToolDispatcher>,
    binding: ScheduleFiringHostBinding,
}

impl FiringHostGatedScheduleTools {
    fn refusal(&self, name: &str) -> Option<meerkat_core::ToolError> {
        if self.binding.is_quiesced_candidate() {
            if !MUTATING_SCHEDULE_TOOLS.contains(&name) {
                return None;
            }
            return Some(meerkat_core::ToolError::execution_failed(format!(
                "{name} refused: this gateway is a candidate launch, which takes no firing \
                 authority over the schedule store and changes no durable schedule. Schedule \
                 changes belong to the authoritative gateway. Get, list, and occurrences \
                 remain available."
            )));
        }
        if !FIRING_INTENT_SCHEDULE_TOOLS.contains(&name) || self.binding.is_bound() {
            return None;
        }
        Some(meerkat_core::ToolError::execution_failed(format!(
            "{name} refused: no schedule firing host is bound to this store, so the \
             schedule would be accepted durably and then never fire (this gateway \
             is running without its schedule host - ephemeral mode or a failed \
             host spawn). Reads, pause, and delete remain available."
        )))
    }
}

#[async_trait]
impl meerkat_core::AgentToolDispatcher for FiringHostGatedScheduleTools {
    fn bind_session_mob_tool_authority(
        &self,
        authority: Option<Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>>,
    ) {
        self.inner.bind_session_mob_tool_authority(authority);
    }

    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        self.inner.tools()
    }

    /// The gate refuses calls at dispatch time; it advertises the inner
    /// surface unchanged, so the inner catalog declarations are forwarded
    /// verbatim rather than downgraded to the non-exact trait default.
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        self.inner.tool_catalog()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        if let Some(refusal) = self.refusal(call.name) {
            return Err(refusal);
        }
        self.inner.dispatch(call).await
    }

    async fn dispatch_with_context(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        if let Some(refusal) = self.refusal(call.name) {
            return Err(refusal);
        }
        // Forward AS the context entry point — the trait default drops the
        // ToolDispatchContext (verified witness-loss class, see tool_compose).
        self.inner.dispatch_with_context(call, context).await
    }
}

type MobSessionResolver =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = MobSessionResolution> + Send>> + Send + Sync>;

/// A rewrite candidate met a stopped mob state.
struct MobStateStopped;

struct MobIdentityScheduleToolDispatcher {
    inner: Arc<dyn meerkat_core::AgentToolDispatcher>,
    resolve_mob_session: MobSessionResolver,
}

impl MobIdentityScheduleToolDispatcher {
    fn new(
        inner: Arc<dyn meerkat_core::AgentToolDispatcher>,
        mob_target_registry: ScheduleMobTargetRegistry,
    ) -> Self {
        let resolve_mob_session = Arc::new(move |session_id: String| {
            let mob_target_registry = mob_target_registry.clone();
            Box::pin(async move { mob_target_registry.resolve_mob_session(&session_id).await })
                as Pin<Box<dyn Future<Output = MobSessionResolution> + Send>>
        });
        Self {
            inner,
            resolve_mob_session,
        }
    }

    #[cfg(test)]
    fn new_for_test<F, Fut>(inner: Arc<dyn meerkat_core::AgentToolDispatcher>, resolve: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<(String, String)>> + Send + 'static,
    {
        let resolve = Arc::new(resolve);
        let resolve_mob_session = Arc::new(move |session_id: String| {
            let resolve = resolve.clone();
            Box::pin(async move { MobSessionResolution::Resolved(resolve(session_id).await) })
                as Pin<Box<dyn Future<Output = MobSessionResolution> + Send>>
        });
        Self {
            inner,
            resolve_mob_session,
        }
    }

    /// Rewrite a create or update whose resumable-session target is a mob
    /// member's bridge session to that member. A rewrite candidate whose mob
    /// state is gone is refused: it is neither judged by the dead state nor
    /// let through unrewritten.
    async fn rewrite_args(&self, call_name: &str, args: Value) -> Result<Value, MobStateStopped> {
        if call_name != "meerkat_schedule_create" && call_name != "meerkat_schedule_update" {
            return Ok(args);
        }
        let stopped = AtomicBool::new(false);
        let rewritten = rewrite_resumable_session_target_to_mob_member(args, |session_id| {
            let stopped = &stopped;
            async move {
                match (self.resolve_mob_session)(session_id).await {
                    MobSessionResolution::Resolved(found) => found,
                    MobSessionResolution::Stopped => {
                        stopped.store(true, Ordering::Relaxed);
                        None
                    }
                }
            }
        })
        .await;
        if stopped.load(Ordering::Relaxed) {
            return Err(MobStateStopped);
        }
        Ok(rewritten)
    }
}

#[async_trait]
impl meerkat_core::AgentToolDispatcher for MobIdentityScheduleToolDispatcher {
    fn bind_session_mob_tool_authority(
        &self,
        authority: Option<Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>>,
    ) {
        self.inner.bind_session_mob_tool_authority(authority);
    }

    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        self.inner.tools()
    }

    /// Argument rewriting does not change the advertised surface; forward the
    /// inner catalog declarations verbatim.
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        self.inner.tool_catalog()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.dispatch_with_context(call, &meerkat_core::ToolDispatchContext::default())
            .await
    }

    /// The rewrite keeps the turn's dispatch context for the inner dispatcher,
    /// rewritten or not: the trait default would drop it and call `dispatch`.
    async fn dispatch_with_context(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        if call.name != "meerkat_schedule_create" && call.name != "meerkat_schedule_update" {
            return self.inner.dispatch_with_context(call, context).await;
        }

        let args: Value = serde_json::from_str(call.args.get()).map_err(|error| {
            meerkat_core::ToolError::invalid_arguments(
                call.name,
                format!("invalid schedule tool-call arguments JSON: {error}"),
            )
        })?;
        // A stopped mob state is Meta's stopped driver: refused as
        // temporarily unavailable, as Core refuses a stopped driver.
        let rewritten = self
            .rewrite_args(call.name, args)
            .await
            .map_err(|MobStateStopped| {
                meerkat_core::ToolError::unavailable(
                    call.name,
                    meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
                )
            })?;
        let rewritten_raw = serde_json::value::RawValue::from_string(rewritten.to_string())
            .map_err(|error| {
                meerkat_core::ToolError::invalid_arguments(
                    call.name,
                    format!("failed to encode rewritten schedule arguments: {error}"),
                )
            })?;
        self.inner
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: call.id,
                    name: call.name,
                    args: &rewritten_raw,
                },
                context,
            )
            .await
    }
}

async fn rewrite_resumable_session_target_to_mob_member<F, Fut>(
    mut args: Value,
    mut resolve: F,
) -> Value
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Option<(String, String)>>,
{
    let Some(target) = args
        .as_object_mut()
        .and_then(|object| object.get_mut("target"))
        .and_then(Value::as_object_mut)
    else {
        return args;
    };

    let is_resumable_session_target = target
        .get("target_kind")
        .and_then(Value::as_str)
        .is_some_and(|value| value == "session")
        && target
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "resumable_session");
    if !is_resumable_session_target {
        return args;
    }
    let Some(session_id) = target
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return args;
    };
    let Some(action) = target.get("action").and_then(Value::as_object) else {
        return args;
    };
    if action
        .get("type")
        .and_then(Value::as_str)
        .is_none_or(|value| value != "prompt")
    {
        return args;
    }
    if action_value_is_non_empty(action.get("system_prompt"))
        || action_value_is_non_empty(action.get("skill_refs"))
        || action_value_is_non_empty(action.get("additional_instructions"))
    {
        return args;
    }
    let Some(prompt) = action.get("prompt").cloned() else {
        return args;
    };
    let Some((mob_id, member_id)) = resolve(session_id).await else {
        return args;
    };

    let mut mob_action = Map::new();
    mob_action.insert("type".to_string(), Value::String("send".to_string()));
    mob_action.insert("content".to_string(), prompt);
    if let Some(render_metadata) = action.get("render_metadata").cloned() {
        mob_action.insert("render_metadata".to_string(), render_metadata);
    }

    let mut rewritten = Map::new();
    rewritten.insert("target_kind".to_string(), Value::String("mob".to_string()));
    rewritten.insert("type".to_string(), Value::String("member".to_string()));
    rewritten.insert("mob_id".to_string(), Value::String(mob_id));
    rewritten.insert("member_id".to_string(), Value::String(member_id));
    rewritten.insert("action".to_string(), Value::Object(mob_action));
    *target = rewritten;
    args
}

fn action_value_is_non_empty(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Array(values)) => !values.is_empty(),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Object(values)) => !values.is_empty(),
        Some(Value::Bool(_) | Value::Number(_)) => true,
    }
}

async fn rewrite_target_binding_to_mob_member<F, Fut>(
    target: &TargetBinding,
    resolve: &mut F,
) -> Option<TargetBinding>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Option<(String, String)>>,
{
    let TargetBinding::Session(binding) = target else {
        return None;
    };
    let SessionTargetBinding::ResumableSession { session_id, action } = binding.as_ref() else {
        return None;
    };
    let ScheduledSessionAction::Prompt {
        prompt,
        system_prompt,
        render_metadata,
        skill_refs,
        additional_instructions,
    } = action
    else {
        return None;
    };
    if system_prompt.is_some() || !skill_refs.is_empty() || !additional_instructions.is_empty() {
        return None;
    }
    let (mob_id, member_id) = resolve(session_id.to_string()).await?;
    Some(TargetBinding::Mob(Box::new(MobTargetBinding::Member {
        mob_id,
        member_id,
        action: ScheduledMobAction::Send {
            content: prompt.clone(),
            render_metadata: render_metadata.clone(),
        },
    })))
}

async fn repair_resumable_session_targets_with_resolver<F, Fut>(
    service: &ScheduleService,
    mut resolve: F,
) -> Result<usize, String>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Option<(String, String)>>,
{
    let schedules = service
        .list()
        .await
        .map_err(|error| format!("list schedules: {error}"))?;
    let mut repaired = 0usize;
    for schedule in schedules {
        let Some(target) =
            rewrite_target_binding_to_mob_member(&schedule.target, &mut resolve).await
        else {
            continue;
        };
        match service
            .update(
                &schedule.schedule_id,
                UpdateScheduleRequest {
                    target: Some(target),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(_) => repaired += 1,
            Err(error) => {
                tracing::warn!(
                    schedule_id = %schedule.schedule_id,
                    error = %error,
                    "failed to repair legacy resumable_session schedule target; continuing",
                );
            }
        }
    }
    Ok(repaired)
}

/// Best-effort migration for schedules authored before Meta preserved the
/// identity target. If the old session id still maps to a mob member, rewrite
/// simple resumable-session prompts to durable mob/member delivery before the
/// schedule host starts firing due occurrences.
pub async fn repair_resumable_session_targets_to_mob_members(
    service: &ScheduleService,
    mob_target_registry: &ScheduleMobTargetRegistry,
) -> Result<usize, String> {
    repair_resumable_session_targets_with_resolver(service, |session_id| async move {
        mob_target_registry
            .resolve_bridge_session(&session_id)
            .await
    })
    .await
}

/// Build a durable (Sqlite) [`ScheduleService`] under `state_dir` and attach its
/// tool dispatcher to `builder`'s default schedule-tools slot.
///
/// After this, any member whose profile resolves schedule tools on
/// (`tools.schedule = true` → `override_schedule = Enable`) is built with the
/// `meerkat_schedule_{create,get,list,update,pause,resume,delete,occurrences}`
/// surface. Returns the service so the same instance can back the firing host.
///
/// Call this on the `FactoryAgentBuilder` BEFORE it is consumed into a session
/// service. Returns `None` (with a warning) if the store cannot be opened — the
/// gateway then boots without schedule tools rather than failing closed.
#[must_use]
pub fn attach_schedule_tools(
    builder: &FactoryAgentBuilder,
    state_dir: &Path,
) -> Option<ScheduleService> {
    attach_schedule_tools_with_identity_targets(builder, state_dir).map(|attached| attached.service)
}

#[must_use]
pub fn attach_schedule_tools_with_identity_targets(
    builder: &FactoryAgentBuilder,
    state_dir: &Path,
) -> Option<AttachedScheduleTools> {
    attach_schedule_tools_with_identity_targets_reporting(builder, state_dir).ok()
}

/// [`attach_schedule_tools_with_identity_targets`] with the open failure
/// reported instead of swallowed, so composition sites can record the
/// sanctioned boot-without posture as a health-visible storage-slot
/// degradation (M4) rather than a warn line alone.
pub fn attach_schedule_tools_with_identity_targets_reporting(
    builder: &FactoryAgentBuilder,
    state_dir: &Path,
) -> Result<AttachedScheduleTools, String> {
    let path = state_dir.join(SCHEDULE_STORE_FILE);
    let store = match SqliteScheduleStore::open(&path) {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to open schedule store; schedule tools disabled for this gateway",
            );
            return Err(format!("{}: {error}", path.display()));
        }
    };
    // Gateway-owned local store: the only driver that can ever drain it is
    // this process's schedule host. Firing-intent writes refuse until the
    // composition marks the host bound (Bug C stopgap) — the alternative is
    // a durably accepted schedule that silently never fires.
    Ok(attach_schedule_tools_with_store_and_binding(
        builder,
        Arc::new(store),
        ScheduleFiringHostBinding::pending_in_process_host(),
    ))
}

/// Attach the agent-facing schedule tools over a caller-supplied
/// [`ScheduleStore`] (the public store seam, M4): the store no longer has to
/// be the bundled SQLite file — a downstream backend (BigQuery, Postgres)
/// injects its own implementation and gets the identical tool surface.
///
/// Injection is a *foundation*: any shadow-scheduler deletion downstream
/// requires a feature-parity audit first (multi-replica claims, timezone
/// cron, jitter, delivery-policy handoffs).
pub fn attach_schedule_tools_with_store(
    builder: &FactoryAgentBuilder,
    store: Arc<dyn ScheduleStore>,
) -> AttachedScheduleTools {
    // Caller-injected store (library mode): firing responsibility rides with
    // the injector — the embedder's own driver, or a gateway pointed at the
    // same store, drains it. Whether that driver exists is the injector's
    // ruling; meta never gates a store it does not own.
    attach_schedule_tools_with_store_and_binding(
        builder,
        store,
        ScheduleFiringHostBinding::external_driver(),
    )
}

fn attach_schedule_tools_with_store_and_binding(
    builder: &FactoryAgentBuilder,
    store: Arc<dyn ScheduleStore>,
    firing_host_binding: ScheduleFiringHostBinding,
) -> AttachedScheduleTools {
    let service = ScheduleService::new(store);
    let mob_target_registry = ScheduleMobTargetRegistry::default();
    let dispatcher = MobIdentityScheduleToolDispatcher::new(
        Arc::new(ScheduleToolDispatcher::new(service.clone())),
        mob_target_registry.clone(),
    );
    let gated = FiringHostGatedScheduleTools {
        inner: Arc::new(dispatcher),
        binding: firing_host_binding.clone(),
    };
    meerkat::surface::set_default_schedule_tools(builder, Some(Arc::new(gated)));
    AttachedScheduleTools {
        service,
        mob_target_registry,
        firing_host_binding,
    }
}

/// Spawn the runtime-backed schedule host so authored schedules fire: at due
/// time the driver materializes a session and runs the scheduled prompt as a
/// real agent turn. Session targets are served by meerkat's runtime-backed
/// host; mob targets by `meerkat-mob-mcp`'s host when a mob runtime is present.
///
/// Requires a persistent session service + the runtime adapter (the firing
/// driver is not available for ephemeral sessions). The returned handle MUST be
/// kept alive for the gateway's lifetime — dropping it shuts the host down.
/// Returns `None` if the store kind cannot host.
///
/// Generic over the session builder `B` so both gateways can drive firing: the
/// console gateway with `FactoryAgentBuilder`, and the SDK gateway with its
/// `StdioCallbackAgentBuilder` (so scheduled sessions are materialized through
/// the SDK build callback and keep their identity-scoped tools).
/// Mob host wrapper: mob-member schedule deliveries are INTERNAL addressing.
///
/// A schedule targeting a member of this mob was authored inside the mob (the
/// agent tool rewrite or delivery-time identity recovery produced it), so its
/// delivery is mob coordination — `WorkOrigin::Internal` — not external
/// ingress. The stock meerkat-mob-mcp host routes `member_send` through the
/// EXTERNAL work door, which rejects members whose profile is
/// `external_addressable = false` ("mob member is not externally
/// addressable") — the downstream app's domain agents are internal-only by design, and
/// flipping them externally addressable to receive their own schedules would
/// be the wrong fix. This wrapper delivers member-addressed prompts through
/// the same internal work lane the identity bridge uses
/// (`submit_work_with_mode` + `WorkOrigin::Internal`); everything else
/// (flows, helpers, probes) delegates to the wrapped host.
struct InternalDeliveryScheduleMobHost {
    inner: Arc<dyn SurfaceScheduleMobHost>,
    /// The host is handed to the gateway, so this is the caller's public
    /// (owning) handle; it keeps the mob owned while the host lives, and
    /// `identity_runtime` beside it is the plain runtime.
    handle: MobHandle,
    identity_runtime: Option<Arc<crate::identity_first::IdentityRuntime>>,
}

impl InternalDeliveryScheduleMobHost {
    async fn deliver_internal_member_prompt(
        &self,
        occurrence: &meerkat::Occurrence,
        delivery_identity: &meerkat::ScheduleDeliveryIdentity,
        mob_id: &str,
        member_id: &str,
        content: &meerkat_core::ContentInput,
    ) -> Option<meerkat::DeliveryDispatch> {
        if self.handle.definition().id.as_str() != mob_id {
            return None;
        }
        // Binding member ids arrive in ROSTER space (the authoring rewrite
        // stores `entry.agent_identity`, and meerkat's delivery-time identity
        // recovery stamps the same roster id from the session's mob-member
        // binding) or occasionally in alias space. Roster ids of
        // identity-first members are already `mk--`-encoded, and the codec
        // deliberately RE-encodes marker-prefixed input — encoding the raw
        // binding id double-encodes those and misses the roster (the 0.7.28
        // Downstream app field failure: the miss fell through to the external
        // door). Decode-then-encode canonicalizes both spaces to the roster
        // key; it is the identity on plain member names.
        let member_alias = crate::member_comms_id::runtime_alias_str(member_id).into_owned();
        if let Some(identity_runtime) = self.identity_runtime.as_ref()
            && let Some(identity) = identity_runtime
                .identity_for_member_mutation(&member_alias)
                .await
        {
            // meerkat 0.8.15: the driver replays this exact identity on every
            // lease-expiry reclaim, and the identity dispatch path now
            // consumes BOTH halves at admission - the runtime threads them
            // into the internal work lane's deduplicating submit
            // (`submit_work_with_mode_and_delivery_identity`, WorkRef derived
            // from mob + member identity + idempotency key), so a crash
            // redelivery of the same occurrence resolves to the SAME work
            // instead of a duplicate turn. The correlation is the occurrence
            // UUID by construction (canonical non-nil, as upstream requires).
            let input = crate::identity_first::DispatchInput {
                content: content.clone(),
                origin: crate::identity_first::DispatchOrigin::Scheduler,
                correlation_id: Some(crate::identity_first::CorrelationId::new(
                    delivery_identity.correlation_id.clone(),
                )),
                idempotency_key: Some(crate::identity_first::DispatchIdempotencyKey::new(
                    delivery_identity.idempotency_key.clone(),
                )),
            };
            return Some(
                match identity_runtime
                    .dispatch_member_alias_with_session_tracked(&identity, &member_alias, &input)
                    .await
                {
                    Ok(Some(_)) => immediate_completed_dispatch(
                        occurrence,
                        Some(delivery_identity.correlation_id.clone()),
                    ),
                    Ok(None) => immediate_delivery_failure(
                        occurrence,
                        format!(
                            "identity schedule delivery has no bound session bridge \
                             (member {member_id})"
                        ),
                        meerkat::DeliveryFailureReason::RuntimeRejected,
                        Some(delivery_identity.correlation_id.clone()),
                        None,
                    ),
                    Err(error) => immediate_delivery_failure(
                        occurrence,
                        format!("identity schedule delivery failed (member {member_id}): {error}"),
                        meerkat::DeliveryFailureReason::RuntimeRejected,
                        Some(delivery_identity.correlation_id.clone()),
                        None,
                    ),
                },
            );
        }
        if crate::member_comms_id::is_reserved_generated_alias(&member_alias) {
            return Some(immediate_delivery_failure(
                occurrence,
                format!(
                    "generated member alias requires current identity authority: {member_alias}"
                ),
                meerkat::DeliveryFailureReason::RuntimeRejected,
                Some(delivery_identity.correlation_id.clone()),
                None,
            ));
        }
        let member = crate::member_comms_id::mob_member_id(&member_alias);
        let entry = match self.handle.get_member(&member).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return None,
            Err(error) => {
                return Some(immediate_delivery_failure(
                    occurrence,
                    format!("member lookup failed (member {member_id}): {error}"),
                    meerkat::DeliveryFailureReason::RuntimeRejected,
                    Some(delivery_identity.correlation_id.clone()),
                    None,
                ));
            }
        };
        let spec = meerkat_mob::WorkSpec::new(content.clone(), meerkat_mob::WorkOrigin::Internal);
        // meerkat 0.8.15 deduplicating internal work door: the WorkRef
        // derives from mob + member identity + idempotency key, so replaying
        // this occurrence after a crash resolves to the SAME work. The
        // correlation is the occurrence UUID by construction; a validation
        // refusal is a real bug in that construction and fails the delivery
        // typed rather than silently downgrading to at-least-once.
        let mob_delivery_identity = match meerkat_mob::MobDeliveryIdentity::new(
            delivery_identity.idempotency_key.clone(),
            delivery_identity.correlation_id.clone(),
        ) {
            Ok(identity) => identity,
            Err(error) => {
                return Some(immediate_delivery_failure(
                    occurrence,
                    format!(
                        "schedule delivery identity rejected upstream validation \
                         (member {member_id}): {error}"
                    ),
                    meerkat::DeliveryFailureReason::RuntimeRejected,
                    Some(delivery_identity.correlation_id.clone()),
                    None,
                ));
            }
        };
        match self
            .handle
            .submit_work_with_mode_and_delivery_identity(
                entry.agent_runtime_id.clone(),
                entry.fence_token,
                spec,
                meerkat_core::types::HandlingMode::Queue,
                mob_delivery_identity,
            )
            .await
        {
            Ok(receipt) => {
                let mut dispatch = immediate_completed_dispatch(
                    occurrence,
                    Some(delivery_identity.correlation_id.clone()),
                );
                // Retain the lower admission identity in the schedule receipt.
                // This is both useful diagnostics and a durable-carrier proof:
                // a replay of one occurrence must expose the same derived work
                // reference instead of minting a second unit of work.
                dispatch.receipt.detail = Some(format!("work_ref={}", receipt.work_ref));
                Some(dispatch)
            }
            Err(error) => Some(immediate_delivery_failure(
                occurrence,
                format!("internal schedule delivery failed (member {member_id}): {error}"),
                meerkat::DeliveryFailureReason::RuntimeRejected,
                Some(delivery_identity.correlation_id.clone()),
                None,
            )),
        }
    }
}

#[async_trait]
impl SurfaceScheduleMobHost for InternalDeliveryScheduleMobHost {
    async fn probe_mob_target(
        &self,
        binding: &MobTargetBinding,
    ) -> Result<meerkat::TargetProbeOutcome, meerkat::ScheduleDomainError> {
        self.inner.probe_mob_target(binding).await
    }

    async fn deliver_mob_target(
        &self,
        occurrence: &meerkat::Occurrence,
        identity: &meerkat::ScheduleDeliveryIdentity,
        binding: &MobTargetBinding,
    ) -> Result<meerkat::DeliveryDispatch, meerkat::ScheduleDomainError> {
        if let MobTargetBinding::Member {
            mob_id,
            member_id,
            action: ScheduledMobAction::Send { content, .. },
        } = binding
            && let Some(dispatch) = self
                .deliver_internal_member_prompt(occurrence, identity, mob_id, member_id, content)
                .await
        {
            return Ok(dispatch);
        }
        self.inner
            .deliver_mob_target(occurrence, identity, binding)
            .await
    }

    async fn probe_identity_target(
        &self,
        binding: &meerkat::IdentityTargetBinding,
    ) -> Result<Option<meerkat::TargetProbeOutcome>, meerkat::ScheduleDomainError> {
        self.inner.probe_identity_target(binding).await
    }

    async fn deliver_identity_target(
        &self,
        occurrence: &meerkat::Occurrence,
        delivery_identity: &meerkat::ScheduleDeliveryIdentity,
        binding: &meerkat::IdentityTargetBinding,
    ) -> Result<Option<meerkat::DeliveryDispatch>, meerkat::ScheduleDomainError> {
        if let Some(identity) = parse_mob_member_schedule_identity(binding.identity())
            && let meerkat::ScheduledSessionAction::Prompt {
                prompt,
                system_prompt: None,
                skill_refs,
                additional_instructions,
                ..
            } = binding.action()
            && skill_refs.is_empty()
            && additional_instructions.is_empty()
            && let Some(dispatch) = self
                .deliver_internal_member_prompt(
                    occurrence,
                    delivery_identity,
                    &identity.mob_id,
                    &identity.member,
                    prompt,
                )
                .await
        {
            return Ok(Some(dispatch));
        }
        self.inner
            .deliver_identity_target(occurrence, delivery_identity, binding)
            .await
    }
}

#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn spawn_schedule_host<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
    adapter: Arc<MeerkatMachine>,
    schedule_service: ScheduleService,
    mob_state: Option<Arc<MobMcpState>>,
    mob_handle: MobHandle,
    runnable_host: Option<Arc<dyn meerkat::ScheduleRunnableHost>>,
    workgraph_service: Option<meerkat::WorkGraphService>,
    owner_id: impl Into<String>,
) -> Option<ScheduleHostHandle> {
    spawn_schedule_host_with_identity_runtime(
        service,
        adapter,
        schedule_service,
        mob_state,
        mob_handle,
        None,
        runnable_host,
        workgraph_service,
        owner_id,
    )
}

/// Identity-aware schedule host. Generated member aliases are dispatched
/// through the durable runtime instead of the raw member plane.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn spawn_schedule_host_with_identity_runtime<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
    adapter: Arc<MeerkatMachine>,
    schedule_service: ScheduleService,
    mob_state: Option<Arc<MobMcpState>>,
    mob_handle: MobHandle,
    identity_runtime: Option<Arc<crate::identity_first::IdentityRuntime>>,
    runnable_host: Option<Arc<dyn meerkat::ScheduleRunnableHost>>,
    workgraph_service: Option<meerkat::WorkGraphService>,
    owner_id: impl Into<String>,
) -> Option<ScheduleHostHandle> {
    let inner_mob_host: Arc<dyn SurfaceScheduleMobHost> = match mob_state {
        Some(state) => Arc::new(MobMcpScheduleHost::new(state)),
        None => Arc::new(NoopScheduleMobHost::new(
            "scheduled mob targets are not supported: no mob runtime",
        )),
    };
    // Member-addressed deliveries take the INTERNAL work lane (schedule
    // delivery to a mob member is mob coordination, not external ingress) —
    // internal-only members receive their own schedules.
    let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(InternalDeliveryScheduleMobHost {
        inner: inner_mob_host,
        handle: mob_handle,
        identity_runtime,
    });
    // meerkat 0.7.13 (upstream ask 10): thread a host-runnable registry so
    // host-registered runnables (e.g. the memory steward's dream) can be
    // driven as schedule occurrences. `None` keeps mob/session targets only.
    // meerkat 0.7.23: the runtime-backed session host injects the WorkGraph
    // attention projection at apply time when given a WorkGraphService, so
    // scheduled turns on goal-bound sessions carry the scoped tool overlay.
    spawn_runtime_backed_schedule_host_with_mobs(
        service,
        adapter,
        Config::default(),
        schedule_service,
        SessionBuildOptions::default(),
        mob_host,
        runnable_host,
        workgraph_service,
        owner_id,
    )
}

/// Cadence and sensitivity for [`spawn_schedule_claim_watchdog`].
#[derive(Debug, Clone, Copy)]
pub struct ScheduleClaimWatchdogConfig {
    /// Delay between probes.
    pub poll_interval: Duration,
    /// A pending occurrence under an active schedule due longer ago than this
    /// counts as stalled.
    pub overdue_threshold: Duration,
    /// While unhealthy with an unchanged report, re-log every Nth poll.
    pub heartbeat_polls: u32,
}

impl Default for ScheduleClaimWatchdogConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_mins(1),
            overdue_threshold: Duration::from_mins(2),
            heartbeat_polls: 10,
        }
    }
}

/// Verdict from one firing-pipeline probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleFiringProbe {
    Healthy,
    /// Active overdue work is not delivering.
    Stalled {
        report: String,
    },
    /// Reads or parent attribution failed. This is neither a confirmed
    /// active-work stall nor evidence of health.
    Incomplete {
        report: String,
    },
}

impl ScheduleFiringProbe {
    pub(crate) fn log_at_boot(&self, config: ScheduleClaimWatchdogConfig) {
        match self {
            Self::Healthy => tracing::info!("schedule firing pipeline healthy at boot"),
            Self::Stalled { report } => tracing::warn!(
                %report,
                poll_interval_secs = config.poll_interval.as_secs(),
                "schedule firing pipeline is not delivering at boot; the resident claim \
                 watchdog re-probes on its cadence and escalates to ERROR if this persists \
                 (a restart backlog clears on the first host ticks)"
            ),
            Self::Incomplete { report } => tracing::error!(
                %report,
                "schedule firing pipeline observation incomplete at boot"
            ),
        }
    }
}

/// One read-only projection, rebuilt each probe. Only the schedule owner's
/// typed phase attributes work; an absent parent is never a tombstone.
#[derive(Default)]
struct PendingScheduleAttribution<'a> {
    active: Vec<&'a Occurrence>,
    paused: usize,
    deleted: usize,
    unresolved: Vec<&'a Occurrence>,
}

impl<'a> PendingScheduleAttribution<'a> {
    fn observe(
        schedules: &[Schedule],
        occurrences: &'a [Occurrence],
        overdue_before: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        let phases: BTreeMap<_, _> = schedules
            .iter()
            .map(|schedule| (&schedule.schedule_id, schedule.phase))
            .collect();
        let mut attribution = Self::default();
        for occurrence in occurrences {
            // The store's due-before filter is inclusive; the watchdog's
            // threshold promises strictly more than this much lateness.
            if occurrence.phase != OccurrencePhase::Pending
                || occurrence.due_at_utc >= overdue_before
            {
                continue;
            }
            match phases.get(&occurrence.schedule_id) {
                Some(SchedulePhase::Active) => attribution.active.push(occurrence),
                Some(SchedulePhase::Paused) => attribution.paused += 1,
                Some(SchedulePhase::Deleted) => attribution.deleted += 1,
                None => attribution.unresolved.push(occurrence),
            }
        }
        attribution
    }
}

impl std::fmt::Display for PendingScheduleAttribution<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "overdue pending attribution: active={}, paused={}, deleted={}, unresolved={}",
            self.active.len(),
            self.paused,
            self.deleted,
            self.unresolved.len(),
        )?;
        for occurrence in self.unresolved.iter().take(5) {
            write!(
                f,
                "; occurrence {}: parent schedule {} missing from the authoritative \
                 listing; attribution unresolved",
                occurrence.occurrence_id, occurrence.schedule_id,
            )?;
        }
        if self.unresolved.len() > 5 {
            f.write_str("; further unresolved parent rows elided")?;
        }
        Ok(())
    }
}

/// The store's own answer to "does any host currently hold this realm's
/// singular schedule firing authority?"
///
/// meerkat 0.8.22 fences `claim_due_occurrences` behind a store-issued
/// executor lease, and `observe_executor_lease` is its deliberately
/// NON-AUTHORIZING read: the observation carries no bearer token, so taking
/// it can never grant, steal, or disturb the live driver's claim authority.
/// Meta uses it to STATE durable firing-host liveness instead of inferring
/// it from "nothing got claimed, so the driver must be dead".
///
/// This is evidence about the whole realm store, not about this process: the
/// lease is singular per store, so a held lease can belong to another
/// gateway. It is therefore not a substitute for the process-local firing
/// host binding, which answers a different question (does a driver live in
/// THIS process) for a different purpose (the firing-intent write gate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleFiringAuthority {
    /// A host holds the executor lease at the store's own clock.
    ///
    /// A BOUNDED fact, not proof of liveness: it says the holder renewed
    /// inside its lease window (60s by default), so a host that died within
    /// the last lease period still reads `Held` until that lease expires.
    /// Callers that report this to a human must say so - a crashed
    /// predecessor and a healthy driver are indistinguishable here.
    Held {
        /// Executor incarnation identity, not a stable host name: a
        /// restarted process mints a new one.
        owner_id: String,
        /// Monotonic liveness epoch the store advances on every takeover.
        fencing_token: u64,
        /// Lease remaining, measured entirely on the store's clock (the
        /// observation carries the store's `now`, so this never mixes clocks
        /// with the observing process).
        expires_in_secs: i64,
    },
    /// No host holds the lease at the store's own clock. Instantaneous: a
    /// host that has only just spawned acquires on its FIRST tick, so a
    /// vacancy read at boot is not yet evidence of a dead driver.
    Vacant,
    /// The store cannot answer: a pre-lease schedule store, a custom store
    /// that does not implement the read, or a transient store failure. This
    /// is evidence for neither side and must never be reported as vacancy.
    Unobservable { detail: String },
}

/// Read durable firing-host liveness straight from the store.
///
/// Never fails: an unanswerable store is a typed [`ScheduleFiringAuthority`]
/// variant rather than an error, because every caller here is a diagnostic
/// that must keep reporting the rest of its verdict.
pub async fn observe_schedule_firing_authority(
    schedule_service: &ScheduleService,
) -> ScheduleFiringAuthority {
    match schedule_service.firing_host_status().await {
        Ok(observation) => {
            // The store's clock, captured before the active holder is moved
            // out: expiry is computed store-clock against store-clock, never
            // against this process's wall time.
            let store_now_utc = observation.store_now_utc;
            match observation.active {
                Some(active) => ScheduleFiringAuthority::Held {
                    fencing_token: active.fencing_token,
                    expires_in_secs: (active.expires_at_utc - store_now_utc).num_seconds(),
                    owner_id: active.owner_id,
                },
                None => ScheduleFiringAuthority::Vacant,
            }
        }
        Err(error) => ScheduleFiringAuthority::Unobservable {
            detail: error.to_string(),
        },
    }
}

fn triage_poisoned_rows(
    store_path: &Path,
    table: &str,
    id_column: &str,
    json_column: &str,
    parse: impl Fn(&[u8]) -> Result<(), String>,
) -> Vec<String> {
    // Passive diagnostic read of meerkat-schedule's tables: the `ReadOnly`
    // profile never creates, never converts the journal mode, and cannot
    // write.
    let conn = match meerkat_sqlite::open(store_path, meerkat_sqlite::ConnectionProfile::ReadOnly) {
        Ok(conn) => conn,
        Err(err) => return vec![format!("(row triage unavailable: {err})")],
    };
    let sql = format!("SELECT {id_column}, {json_column} FROM {table}");
    let mut stmt = match conn.prepare(&sql) {
        Ok(stmt) => stmt,
        Err(err) => return vec![format!("(row triage query failed: {err})")],
    };
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    });
    let mut poisoned = Vec::new();
    if let Ok(rows) = rows {
        for row in rows.flatten() {
            let (id, bytes) = row;
            if let Err(err) = parse(&bytes) {
                poisoned.push(format!("{table}.{id_column}={id}: {err}"));
                if poisoned.len() >= 5 {
                    poisoned.push(format!("(further poisoned {table} rows elided)"));
                    break;
                }
            }
        }
    }
    poisoned
}

/// Probe whether the schedule firing pipeline can actually deliver, and if
/// not, name the reason — down to the poisoned row where possible.
///
/// Born on a downstream app on 0.7.24, where a tick aborted wholesale on the FIRST
/// poisoned row anywhere in the store and the host discarded the error: 31
/// pending occurrences sat with `lease_expires_at_ms=NULL` and nothing in
/// any log. meerkat 0.8.11 fixed the starvation half — the driver's listing
/// (`list_with_row_faults`) and claim scan now skip poisoned rows as typed
/// per-row faults and the host logs the fault fingerprint — but the loss
/// half remains: a poisoned row can never be claimed or listed, so ITS work
/// silently never fires, and the plain all-or-nothing reads every other
/// surface uses (`service.list()`, `store.list_occurrences()`) still fail
/// wholesale on it. This probe stays as the read-only belt-and-suspenders
/// that names the row.
///
/// meerkat 0.8.22 removes the guesswork from the OTHER half of the verdict:
/// when due work is unclaimed, the probe asks the store who holds the
/// singular executor lease ([`observe_schedule_firing_authority`]) instead of
/// inferring a dead driver. Everything here remains a read - the lease
/// observation carries no bearer token, so this probe still cannot claim,
/// transition, or race the real driver.
pub async fn probe_schedule_firing_pipeline(
    schedule_service: &ScheduleService,
    store_path: &Path,
    overdue_threshold: Duration,
) -> ScheduleFiringProbe {
    let now = match schedule_service.store().get_store_time_utc().await {
        Ok(now) => now,
        Err(err) => {
            return ScheduleFiringProbe::Incomplete {
                report: format!("schedule store clock read failed: {err}; observation incomplete"),
            };
        }
    };
    probe_schedule_firing_pipeline_at(schedule_service, store_path, overdue_threshold, now).await
}

async fn probe_schedule_firing_pipeline_at(
    schedule_service: &ScheduleService,
    store_path: &Path,
    overdue_threshold: Duration,
    now: chrono::DateTime<chrono::Utc>,
) -> ScheduleFiringProbe {
    // 1. One poisoned schedule row fails the whole all-or-nothing list. The
    // 0.8.11 driver tolerates it per-row (its own listing skips the row
    // typed), but the poisoned schedule itself can never fire and every
    // list-backed surface (tools, RPC, console) is blind until it is
    // repaired.
    // Include tombstones: list() hides Deleted parents, which would make
    // their leftover pending rows indistinguishable from missing evidence.
    let schedules = match schedule_service
        .list_filtered(ScheduleFilter {
            include_deleted: true,
            ..ScheduleFilter::default()
        })
        .await
    {
        Ok(schedules) => schedules,
        Err(err) => {
            let mut report = format!(
                "schedule list is failing (one poisoned schedule row fails the whole read); \
             the firing driver skips the row as a typed fault, so that schedule will \
             never fire and list-backed surfaces are blind until it is repaired: {err}; \
             parent attribution unresolved; observation incomplete"
            );
            let poisoned = triage_poisoned_rows(
                store_path,
                "schedule_schedules",
                "schedule_id",
                "schedule_json",
                |bytes| {
                    serde_json::from_slice::<Schedule>(bytes)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                },
            );
            if !poisoned.is_empty() {
                report.push_str(&format!("; poisoned rows: {}", poisoned.join("; ")));
            }
            return ScheduleFiringProbe::Incomplete { report };
        }
    };

    // 2. The claim scan's poison surface: it deserializes every occurrence.
    let store = schedule_service.store();
    let overdue_before = now
        - chrono::Duration::from_std(overdue_threshold)
            .unwrap_or_else(|_| chrono::Duration::seconds(120));
    let pending_overdue = match store
        .list_occurrences(OccurrenceFilter {
            phase: Some(OccurrencePhase::Pending),
            include_terminal: false,
            due_before_utc: Some(overdue_before),
            ..OccurrenceFilter::default()
        })
        .await
    {
        Ok(occurrences) => occurrences,
        Err(err) => {
            let mut report = format!(
                "occurrence scan is failing (one poisoned due-window occurrence row fails \
                 the whole read); the firing driver's claim skips the row as a typed \
                 fault, so that occurrence will never fire until it is repaired: {err}; \
                 overdue parent attribution unresolved; observation incomplete"
            );
            let poisoned = triage_poisoned_rows(
                store_path,
                "schedule_occurrences",
                "occurrence_id",
                "occurrence_json",
                |bytes| {
                    serde_json::from_slice::<Occurrence>(bytes)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                },
            );
            if !poisoned.is_empty() {
                report.push_str(&format!("; poisoned rows: {}", poisoned.join("; ")));
            }
            return ScheduleFiringProbe::Incomplete { report };
        }
    };

    let attribution =
        PendingScheduleAttribution::observe(&schedules, &pending_overdue, overdue_before);
    tracing::debug!(
        active = attribution.active.len(),
        paused = attribution.paused,
        deleted = attribution.deleted,
        unresolved = attribution.unresolved.len(),
        "schedule overdue pending parent attribution (paused/deleted work is intentionally non-runnable)"
    );
    let Some(oldest_due) = attribution.active.iter().map(|o| o.due_at_utc).min() else {
        return if attribution.unresolved.is_empty() {
            ScheduleFiringProbe::Healthy
        } else {
            ScheduleFiringProbe::Incomplete {
                report: format!("{attribution}; observation incomplete"),
            }
        };
    };

    // 3. Only active-parent overdue work supports a claim-stall diagnosis.
    // Paused/deleted rows remain untouched, and unresolved evidence remains
    // in the report even when some active work can be attributed.
    let mut classify_errors = Vec::new();
    for occurrence in &attribution.active {
        // This probe only reports occurrences whose due action cannot be
        // classified, and it derives no admission evidence from receipts. So
        // it passes the default, `AdmissionEvidence::None` (no earlier
        // attempt's admission proven), which keeps the classification this
        // call made before Meerkat added the argument.
        if let Err(err) = occurrence.classify_due_action(now, Default::default()) {
            classify_errors.push(format!(
                "occurrence {} (due {}): {err}",
                occurrence.occurrence_id, occurrence.due_at_utc
            ));
            if classify_errors.len() >= 5 {
                break;
            }
        }
    }
    let mut report = format!(
        "{} active-parent pending occurrence(s) overdue by more than {}s and never claimed \
         (oldest due {}); {attribution}",
        attribution.active.len(),
        overdue_threshold.as_secs(),
        oldest_due.to_rfc3339(),
    );
    // Do not reconstruct a fact the store will tell you. Before 0.8.22 this
    // sentence could only INFER "the firing driver's claim loop is failing
    // silently" from unclaimed work. meerkat 0.8.22 fences claiming behind a
    // store-issued executor lease, so the store can now name whether any host
    // holds firing authority at all - which splits the one symptom into two
    // very different diagnoses (nothing is draining vs. a lease-holding host
    // that cannot claim). The read is non-authorizing and cannot race the
    // driver.
    //
    // Deliberately NOT in this string: the remaining lease seconds. The
    // watchdog dedupes by exact report text (ERROR on a new or changed
    // report, WARN heartbeat while unchanged), so a per-poll countdown would
    // make every poll of a persistent stall look like a new incident. Owner
    // and fencing token are stable for as long as one host holds authority;
    // the countdown belongs in a structured log field, not in the identity of
    // the report.
    match observe_schedule_firing_authority(schedule_service).await {
        ScheduleFiringAuthority::Held {
            owner_id,
            fencing_token,
            ..
        } => report.push_str(&format!(
            "; the store's firing authority IS held (executor {owner_id}, fencing token \
             {fencing_token}), so the stall is downstream of the lease, in the claim itself. \
             Held is a bounded fact, not proof of liveness: it says the holder renewed inside \
             its lease window, so a host that died within the last lease period still reads \
             held until that lease expires"
        )),
        ScheduleFiringAuthority::Vacant => report.push_str(
            "; the store's firing authority is vacant AT THIS INSTANT - no host anywhere holds \
             the executor lease. A host that has just spawned acquires it on its first tick, so \
             at boot this clears within one tick; a lease still vacant at watchdog cadence means \
             nothing is draining this store",
        ),
        ScheduleFiringAuthority::Unobservable { detail } => report.push_str(&format!(
            "; the firing driver's claim loop is failing silently (the store's firing authority \
             is not observable here, so vacancy is neither proven nor ruled out: {detail})"
        )),
    }
    if classify_errors.is_empty() {
        report.push_str(
            "; every active-parent overdue occurrence classifies cleanly, so the abort is in the claim/lease transaction or a row outside the active overdue set",
        );
    } else {
        report.push_str(&format!(
            "; rows failing due-classification: {}",
            classify_errors.join("; ")
        ));
    }
    ScheduleFiringProbe::Stalled { report }
}

#[derive(Default)]
struct ScheduleClaimWatchdogState {
    last_probe: Option<ScheduleFiringProbe>,
    unhealthy_polls: u32,
}

impl ScheduleClaimWatchdogState {
    fn observe(&mut self, probe: ScheduleFiringProbe, config: ScheduleClaimWatchdogConfig) {
        if probe == ScheduleFiringProbe::Healthy {
            match self.last_probe.take() {
                Some(ScheduleFiringProbe::Stalled { .. }) => {
                    tracing::info!("schedule firing pipeline recovered");
                }
                Some(ScheduleFiringProbe::Incomplete { .. }) => {
                    tracing::info!("schedule firing pipeline observation recovered");
                }
                _ => {}
            }
            self.unhealthy_polls = 0;
            return;
        }
        self.unhealthy_polls = self.unhealthy_polls.saturating_add(1);
        let changed = self.last_probe.as_ref() != Some(&probe);
        let heartbeat = config.heartbeat_polls > 0
            && self.unhealthy_polls.is_multiple_of(config.heartbeat_polls);
        match &probe {
            ScheduleFiringProbe::Stalled { report } if changed => {
                tracing::error!(%report, "schedule firing pipeline is stalled");
            }
            ScheduleFiringProbe::Stalled { report } if heartbeat => {
                tracing::warn!(%report, "schedule firing pipeline is still stalled");
            }
            ScheduleFiringProbe::Incomplete { report } if changed || heartbeat => {
                tracing::error!(%report, "schedule firing pipeline observation incomplete");
            }
            _ => {}
        }
        self.last_probe = Some(probe);
    }
}

/// Watchdog for the silent-stall failure mode above: probes the firing
/// pipeline and logs LOUDLY when due work is not being claimed, naming the
/// poisoned row when one is identifiable. Purely read-only — it never claims
/// or transitions occurrences, so it cannot race the real driver.
///
/// Logs an ERROR when the stall report first appears or changes, a WARN
/// heartbeat every `heartbeat_polls` while it persists, and an INFO when the
/// pipeline recovers. Incomplete observations retain their own ERROR diagnostic
/// without asserting an active-work stall or recovery.
pub fn spawn_schedule_claim_watchdog(
    schedule_service: ScheduleService,
    store_path: std::path::PathBuf,
    config: ScheduleClaimWatchdogConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut state = ScheduleClaimWatchdogState::default();
        loop {
            tokio::time::sleep(config.poll_interval).await;
            let probe = probe_schedule_firing_pipeline(
                &schedule_service,
                &store_path,
                config.overdue_threshold,
            )
            .await;
            state.observe(probe, config);
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Both schedule wrappers pass the session's mob operator authority to
    /// the tools they wrap.
    #[test]
    fn the_schedule_wrappers_bind_the_session_mob_authority_on_their_inner_tools() {
        use crate::tool_application_test_support::{MobAuthorityBindProbe, session_mob_authority};
        let inner = Arc::new(MobAuthorityBindProbe::default());
        let identity_tools =
            MobIdentityScheduleToolDispatcher::new_for_test(inner.clone(), |_| async { None });
        let gated = FiringHostGatedScheduleTools {
            inner: Arc::new(identity_tools),
            binding: ScheduleFiringHostBinding::external_driver(),
        };
        let authority = session_mob_authority();
        meerkat_core::AgentToolDispatcher::bind_session_mob_tool_authority(
            &gated,
            Some(Arc::clone(&authority)),
        );
        assert!(inner.last_bound_is(Some(&authority)));
        meerkat_core::AgentToolDispatcher::bind_session_mob_tool_authority(&gated, None);
        assert!(inner.last_bound_is(None));
    }

    use meerkat::{
        AgentFactory, MemoryScheduleStore, MobTargetBinding,
        ScheduleToolDispatcher as InnerScheduleToolDispatcher, ScheduledMobAction, TargetBinding,
    };
    use meerkat_core::AgentToolDispatcher;
    use meerkat_core::types::{SessionId, ToolCallView};
    use meerkat_schedule::CurrentSessionScheduleToolDispatcher;
    use serde_json::json;
    use serde_json::value::RawValue;

    #[test]
    fn attach_schedule_tools_populates_the_builder_slot_and_opens_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let factory = AgentFactory::new(dir.path());
        let builder = FactoryAgentBuilder::new(factory, Config::default());

        // The default schedule-tools slot starts empty — this is exactly why
        // `profile.tools.schedule = true` was inert before this wiring.
        assert!(
            builder.default_schedule_tools.read().unwrap().is_none(),
            "slot should start empty"
        );

        let service = attach_schedule_tools(&builder, dir.path());

        assert!(service.is_some(), "a durable schedule service is created");
        assert!(
            builder.default_schedule_tools.read().unwrap().is_some(),
            "the dispatcher is installed so override_schedule=Enable members compose schedule tools",
        );
        assert!(
            dir.path().join(SCHEDULE_STORE_FILE).exists(),
            "the durable store file is created",
        );
    }

    #[tokio::test]
    async fn schedule_wrappers_forward_the_inner_catalog_declarations() {
        struct ExactInner;
        #[async_trait]
        impl meerkat_core::AgentToolDispatcher for ExactInner {
            fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
                vec![Arc::new(meerkat_core::types::ToolDef {
                    audience: Default::default(),
                    name: "meerkat_schedule_list".into(),
                    description: String::new(),
                    input_schema: serde_json::json!({"type": "object"}),
                    provenance: None,
                })]
                .into()
            }
            fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
                meerkat_core::ToolCatalogCapabilities {
                    exact_catalog: true,
                    may_require_catalog_control_plane: true,
                }
            }
            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                Err(meerkat_core::ToolError::not_found(call.name))
            }
        }

        let inner: Arc<dyn meerkat_core::AgentToolDispatcher> = Arc::new(ExactInner);
        let rewriting = Arc::new(MobIdentityScheduleToolDispatcher::new_for_test(
            Arc::clone(&inner),
            |_session| async { None },
        ));
        let gated = FiringHostGatedScheduleTools {
            inner: Arc::clone(&rewriting) as Arc<dyn meerkat_core::AgentToolDispatcher>,
            binding: ScheduleFiringHostBinding::pending_in_process_host(),
        };
        for dispatcher in [
            Arc::clone(&rewriting) as Arc<dyn meerkat_core::AgentToolDispatcher>,
            Arc::new(gated) as Arc<dyn meerkat_core::AgentToolDispatcher>,
        ] {
            assert_eq!(
                dispatcher.tool_catalog_capabilities(),
                inner.tool_catalog_capabilities(),
                "schedule wrappers must not downgrade the inner exactness"
            );
            assert_eq!(
                dispatcher
                    .tool_catalog()
                    .iter()
                    .map(|entry| entry.tool.name.to_string())
                    .collect::<Vec<_>>(),
                vec!["meerkat_schedule_list".to_string()]
            );
        }
    }

    /// Bug C stopgap: a gateway-owned store with no spawned firing host must
    /// refuse firing-intent writes typed instead of accepting rows nothing
    /// drains (30 occurrences rotted silently in the field). Reads and
    /// rot-reducing verbs always pass; `bind()` opens the gate.
    #[tokio::test]
    async fn firing_intent_writes_refuse_until_a_firing_host_is_bound() {
        struct CountingInner(std::sync::atomic::AtomicUsize);
        #[async_trait]
        impl meerkat_core::AgentToolDispatcher for CountingInner {
            fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
                Vec::new().into()
            }
            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(meerkat_core::ToolError::not_found(call.name))
            }
        }

        let inner = Arc::new(CountingInner(std::sync::atomic::AtomicUsize::new(0)));
        let binding = ScheduleFiringHostBinding::pending_in_process_host();
        let gated = FiringHostGatedScheduleTools {
            inner: Arc::clone(&inner) as Arc<dyn meerkat_core::AgentToolDispatcher>,
            binding: binding.clone(),
        };
        let args = RawValue::from_string("{}".to_string()).expect("raw");
        let call = |name: &'static str| ToolCallView {
            id: "call-1",
            name,
            args: &args,
        };

        // Firing-intent verbs refuse while pending and never reach the inner
        // dispatcher.
        for verb in FIRING_INTENT_SCHEDULE_TOOLS {
            let err = gated
                .dispatch(call(verb))
                .await
                .expect_err("pending binding must refuse firing-intent writes");
            assert!(
                err.to_string().contains("no schedule firing host is bound"),
                "refusal must name the missing firing host: {err}"
            );
        }
        assert_eq!(inner.0.load(std::sync::atomic::Ordering::SeqCst), 0);

        // Reads and rot-reducing verbs pass through unbound.
        for verb in [
            "meerkat_schedule_get",
            "meerkat_schedule_list",
            "meerkat_schedule_occurrences",
            "meerkat_schedule_pause",
            "meerkat_schedule_delete",
        ] {
            let _ = gated.dispatch(call(verb)).await;
        }
        assert_eq!(inner.0.load(std::sync::atomic::Ordering::SeqCst), 5);

        // Binding the firing host opens the gate for firing-intent verbs.
        binding.bind();
        let _ = gated.dispatch(call("meerkat_schedule_create")).await;
        assert_eq!(inner.0.load(std::sync::atomic::Ordering::SeqCst), 6);
    }

    /// A candidate launch takes no firing authority, so its schedule tools
    /// refuse every verb that changes durable schedule state (the
    /// rot-reducing `pause` and `delete` included, which the pending binding
    /// lets through) and never reach the store. Reads still pass, and a late
    /// `bind()` cannot reopen the gate.
    #[tokio::test]
    async fn candidate_binding_refuses_every_mutating_schedule_verb() {
        struct CountingInner(std::sync::atomic::AtomicUsize);
        #[async_trait]
        impl meerkat_core::AgentToolDispatcher for CountingInner {
            fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
                Vec::new().into()
            }
            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(meerkat_core::ToolError::not_found(call.name))
            }
        }

        let inner = Arc::new(CountingInner(std::sync::atomic::AtomicUsize::new(0)));
        let binding = ScheduleFiringHostBinding::pending_in_process_host();
        binding.quiesce_for_candidate();
        let gated = FiringHostGatedScheduleTools {
            inner: Arc::clone(&inner) as Arc<dyn meerkat_core::AgentToolDispatcher>,
            binding: binding.clone(),
        };
        let args = RawValue::from_string("{}".to_string()).expect("raw");
        let call = |name: &'static str| ToolCallView {
            id: "call-1",
            name,
            args: &args,
        };

        assert!(binding.is_quiesced_candidate());
        assert!(!binding.is_bound());
        for verb in MUTATING_SCHEDULE_TOOLS {
            let err = gated
                .dispatch(call(verb))
                .await
                .expect_err("a candidate launch must refuse every schedule change");
            assert!(
                err.to_string().contains("candidate launch"),
                "refusal must name the candidate launch: {err}"
            );
        }
        assert_eq!(inner.0.load(std::sync::atomic::Ordering::SeqCst), 0);

        for verb in [
            "meerkat_schedule_get",
            "meerkat_schedule_list",
            "meerkat_schedule_occurrences",
        ] {
            let _ = gated.dispatch(call(verb)).await;
        }
        assert_eq!(inner.0.load(std::sync::atomic::Ordering::SeqCst), 3);

        binding.bind();
        assert!(binding.is_quiesced_candidate() && !binding.is_bound());
        for verb in MUTATING_SCHEDULE_TOOLS {
            assert!(gated.dispatch(call(verb)).await.is_err());
        }
        assert_eq!(inner.0.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    /// The caller-injected store seam (library mode, the production shape) never
    /// gates: firing responsibility rides with the injector, and ruling on a
    /// store meta does not own is exactly the seam violation the
    /// dual-authority invariant forbids.
    #[test]
    fn caller_injected_store_attaches_ungated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let factory = AgentFactory::new(dir.path());
        let builder = FactoryAgentBuilder::new(factory, Config::default());
        let attached =
            attach_schedule_tools_with_store(&builder, Arc::new(MemoryScheduleStore::default()));
        assert!(
            attached.firing_host_binding.is_bound(),
            "library mode presumes an external driver; the write gate must not engage"
        );
    }

    /// The gateway state-dir seam starts pending: writes refuse until the
    /// composition binds its in-process host.
    #[test]
    fn gateway_state_dir_store_attaches_with_a_pending_binding() {
        let dir = tempfile::tempdir().expect("tempdir");
        let factory = AgentFactory::new(dir.path());
        let builder = FactoryAgentBuilder::new(factory, Config::default());
        let attached = attach_schedule_tools_with_identity_targets_reporting(&builder, dir.path())
            .expect("store opens");
        assert!(
            !attached.firing_host_binding.is_bound(),
            "gateway-owned store must refuse firing-intent writes until its host spawns"
        );
    }

    #[tokio::test]
    async fn current_session_resumable_prompt_rewrites_to_identity_mob_member_target() {
        let args = json!({
            "name": "daily-digest",
            "trigger": {
                "type": "calendar",
                "timezone": "Europe/Stockholm",
                "hour": { "kind": "values", "values": [7] },
                "minute": { "kind": "values", "values": [0] }
            },
            "target": {
                "target_kind": "session",
                "type": "resumable_session",
                "session_id": "019ee0a7-a594-7670-b530-97e7c9e263b7",
                "action": {
                    "type": "prompt",
                    "prompt": "Send the morning digest.",
                    "render_metadata": {
                        "source": "daily-digest"
                    }
                }
            }
        });

        let rewritten =
            rewrite_resumable_session_target_to_mob_member(args, |session_id| async move {
                assert_eq!(session_id, "019ee0a7-a594-7670-b530-97e7c9e263b7");
                Some(("example".to_string(), "domain:security".to_string()))
            })
            .await;

        assert_eq!(
            rewritten["target"],
            json!({
                "target_kind": "mob",
                "type": "member",
                "mob_id": "example",
                "member_id": "domain:security",
                "action": {
                    "type": "send",
                    "content": "Send the morning digest.",
                    "render_metadata": {
                        "source": "daily-digest"
                    }
                }
            })
        );
    }

    #[tokio::test]
    async fn resumable_session_target_stays_session_when_not_mob_owned() {
        let args = json!({
            "target": {
                "target_kind": "session",
                "type": "resumable_session",
                "session_id": "019ee0a7-a594-7670-b530-97e7c9e263b7",
                "action": {
                    "type": "prompt",
                    "prompt": "Keep this pinned."
                }
            }
        });

        let rewritten =
            rewrite_resumable_session_target_to_mob_member(args.clone(), |_session_id| async {
                None
            })
            .await;

        assert_eq!(rewritten, args);
    }

    #[tokio::test]
    async fn resumable_session_target_with_session_only_prompt_options_is_not_rewritten() {
        let args = json!({
            "target": {
                "target_kind": "session",
                "type": "resumable_session",
                "session_id": "019ee0a7-a594-7670-b530-97e7c9e263b7",
                "action": {
                    "type": "prompt",
                    "prompt": "Run with custom context.",
                    "system_prompt": "Use a one-off persona."
                }
            }
        });

        let rewritten = rewrite_resumable_session_target_to_mob_member(args.clone(), |_| async {
            Some(("example".to_string(), "domain:security".to_string()))
        })
        .await;

        assert_eq!(rewritten, args);
    }

    #[tokio::test]
    async fn empty_session_prompt_options_still_rewrite_to_identity_mob_member_target() {
        let args = json!({
            "target": {
                "target_kind": "session",
                "type": "resumable_session",
                "session_id": "019ee0a7-a594-7670-b530-97e7c9e263b7",
                "action": {
                    "type": "prompt",
                    "prompt": "Run with explicit defaults.",
                    "system_prompt": null,
                    "skill_refs": [],
                    "additional_instructions": []
                }
            }
        });

        let rewritten = rewrite_resumable_session_target_to_mob_member(args, |_| async {
            Some(("example".to_string(), "domain:security".to_string()))
        })
        .await;

        assert_eq!(
            rewritten["target"],
            json!({
                "target_kind": "mob",
                "type": "member",
                "mob_id": "example",
                "member_id": "domain:security",
                "action": {
                    "type": "send",
                    "content": "Run with explicit defaults."
                }
            })
        );
    }

    /// The mob-identity rewrite keeps the turn's dispatch context: a
    /// rewritten create and a pass-through list both reach the inner
    /// dispatcher through `dispatch_with_context` with the caller's context.
    #[tokio::test]
    async fn mob_identity_rewrite_forwards_the_dispatch_context() {
        struct ContextProbe {
            /// Tool name, objective id from the context, and arguments, per call.
            seen: std::sync::Mutex<Vec<(String, Option<String>, String)>>,
        }

        #[async_trait]
        impl meerkat_core::AgentToolDispatcher for ContextProbe {
            fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
                Vec::new().into()
            }

            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                Err(meerkat_core::ToolError::execution_failed(format!(
                    "plain dispatch dropped the context for {}",
                    call.name
                )))
            }

            async fn dispatch_with_context(
                &self,
                call: ToolCallView<'_>,
                context: &meerkat_core::ToolDispatchContext,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                self.seen
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((
                        call.name.to_string(),
                        context
                            .turn_metadata(meerkat_core::agent::TOOL_DISPATCH_OBJECTIVE_ID_KEY)
                            .and_then(Value::as_str)
                            .map(ToString::to_string),
                        call.args.get().to_string(),
                    ));
                Ok(meerkat_core::ToolDispatchOutcome::sync_result(
                    meerkat_core::types::ToolResult::new(
                        call.id.to_string(),
                        "{}".to_string(),
                        false,
                    ),
                ))
            }
        }

        let probe = Arc::new(ContextProbe {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let dispatcher = MobIdentityScheduleToolDispatcher::new_for_test(
            Arc::clone(&probe) as Arc<dyn meerkat_core::AgentToolDispatcher>,
            |session_id| async move {
                assert_eq!(session_id, "019ee0a7-a594-7670-b530-97e7c9e263b7");
                Some(("example".to_string(), "domain:security".to_string()))
            },
        );
        let objective_id = uuid::Uuid::new_v4().to_string();
        let context =
            meerkat_core::ToolDispatchContext::default().with_turn_metadata(BTreeMap::from([(
                meerkat_core::agent::TOOL_DISPATCH_OBJECTIVE_ID_KEY.to_string(),
                Value::String(objective_id.clone()),
            )]));
        let create_args = RawValue::from_string(
            json!({
                "name": "daily-digest",
                "trigger": {
                    "type": "interval",
                    "start_at_utc": "2026-07-01T05:00:00Z",
                    "every_seconds": 86400
                },
                "target": {
                    "target_kind": "session",
                    "type": "resumable_session",
                    "session_id": "019ee0a7-a594-7670-b530-97e7c9e263b7",
                    "action": {
                        "type": "prompt",
                        "prompt": "Send the morning digest."
                    }
                }
            })
            .to_string(),
        )
        .expect("create args");
        let list_args = RawValue::from_string("{}".to_string()).expect("list args");
        for (name, args) in [
            ("meerkat_schedule_create", &create_args),
            ("meerkat_schedule_list", &list_args),
        ] {
            dispatcher
                .dispatch_with_context(
                    ToolCallView {
                        id: "call-with-context",
                        name,
                        args,
                    },
                    &context,
                )
                .await
                .expect("the call reaches the context-aware inner dispatcher");
        }

        let seen = probe
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            seen.iter()
                .map(|(name, objective, _)| (name.as_str(), objective.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("meerkat_schedule_create", Some(objective_id.as_str())),
                ("meerkat_schedule_list", Some(objective_id.as_str())),
            ]
        );
        let created: Value = serde_json::from_str(&seen[0].2).expect("rewritten create args");
        assert_eq!(
            created["target"],
            json!({
                "target_kind": "mob",
                "type": "member",
                "mob_id": "example",
                "member_id": "domain:security",
                "action": {
                    "type": "send",
                    "content": "Send the morning digest."
                }
            }),
            "the create was rewritten to the member before it reached the inner dispatcher"
        );
    }

    #[tokio::test]
    async fn current_session_dispatch_chain_creates_and_updates_identity_mob_targets() {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let schedule_dispatcher = Arc::new(InnerScheduleToolDispatcher::new(service.clone()));
        let mob_dispatcher = Arc::new(MobIdentityScheduleToolDispatcher::new_for_test(
            schedule_dispatcher,
            |session_id| async move {
                assert_eq!(session_id, "019ee0a7-a594-7670-b530-97e7c9e263b7");
                Some(("example".to_string(), "domain:security".to_string()))
            },
        ));
        let session_id =
            SessionId::parse("019ee0a7-a594-7670-b530-97e7c9e263b7").expect("session id");
        let dispatcher = CurrentSessionScheduleToolDispatcher::new(mob_dispatcher, session_id);

        let create_args = RawValue::from_string(
            json!({
                "name": "daily-digest",
                "trigger": {
                    "type": "interval",
                    "start_at_utc": "2026-07-01T05:00:00Z",
                    "every_seconds": 86400
                },
                "target": {
                    "target_kind": "session",
                    "type": "current_session",
                    "action": {
                        "type": "prompt",
                        "prompt": "Send the morning digest.",
                        "render_metadata": {
                            "class": "external_event",
                            "salience": "important"
                        }
                    }
                },
                "planning_horizon_occurrences": 1
            })
            .to_string(),
        )
        .expect("create args");
        let create_call = ToolCallView {
            id: "create-digest",
            name: "meerkat_schedule_create",
            args: &create_args,
        };
        dispatcher
            .dispatch(create_call)
            .await
            .expect("create schedule through dispatch chain");

        let schedules = service.list().await.expect("list schedules");
        assert_eq!(schedules.len(), 1);
        let schedule_id = schedules[0].schedule_id.to_string();
        assert_mob_member_target(
            &schedules[0].target,
            "example",
            "domain:security",
            "Send the morning digest.",
            "external_event",
        );

        let update_args = RawValue::from_string(
            json!({
                "schedule_id": schedule_id,
                "target": {
                    "target_kind": "session",
                    "type": "current_session",
                    "action": {
                        "type": "prompt",
                        "prompt": "Send the updated digest.",
                        "render_metadata": {
                            "class": "peer_request",
                            "salience": "urgent"
                        }
                    }
                }
            })
            .to_string(),
        )
        .expect("update args");
        let update_call = ToolCallView {
            id: "update-digest",
            name: "meerkat_schedule_update",
            args: &update_args,
        };
        dispatcher
            .dispatch(update_call)
            .await
            .expect("update schedule through dispatch chain");

        let updated = service.list().await.expect("list updated schedules");
        assert_eq!(updated.len(), 1);
        assert_mob_member_target(
            &updated[0].target,
            "example",
            "domain:security",
            "Send the updated digest.",
            "peer_request",
        );
    }

    fn assert_mob_member_target(
        target: &TargetBinding,
        expected_mob_id: &str,
        expected_member_id: &str,
        expected_content: &str,
        expected_class: &str,
    ) {
        assert!(
            matches!(target, TargetBinding::Mob(_)),
            "expected mob target, got {target:?}"
        );
        let TargetBinding::Mob(binding) = target else {
            return;
        };
        assert!(
            matches!(binding.as_ref(), MobTargetBinding::Member { .. }),
            "expected member mob target, got {binding:?}"
        );
        let MobTargetBinding::Member {
            mob_id,
            member_id,
            action,
        } = binding.as_ref()
        else {
            return;
        };
        assert_eq!(mob_id, expected_mob_id);
        assert_eq!(member_id, expected_member_id);
        let ScheduledMobAction::Send {
            content,
            render_metadata,
        } = action;
        assert_eq!(content.text_content(), expected_content);
        let metadata = render_metadata.as_ref().expect("render metadata");
        let metadata = serde_json::to_value(metadata).expect("metadata value");
        assert_eq!(metadata["class"], expected_class);
    }

    #[test]
    fn steward_dream_runnable_host_is_none_without_a_steward() {
        assert!(steward_dream_runnable_host(None).is_none());
    }

    /// Records bridge calls and answers each with a scripted result, standing
    /// in for the SDK host on the far side of the stdio callback bridge.
    struct ScriptedCallbackBridge {
        calls: std::sync::Mutex<Vec<(String, Value)>>,
        result: Result<Value, String>,
    }

    impl ScriptedCallbackBridge {
        fn new(result: Result<Value, String>) -> Arc<Self> {
            Arc::new(Self {
                calls: std::sync::Mutex::new(Vec::new()),
                result,
            })
        }
    }

    #[async_trait]
    impl CallbackBridge for ScriptedCallbackBridge {
        async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
            self.calls
                .lock()
                .expect("calls lock")
                .push((method.to_string(), params));
            self.result.clone()
        }
    }

    fn runnable_name(value: &str) -> HostRunnableName {
        HostRunnableName::parse(value).expect("valid runnable name")
    }

    fn callback_invocation(runnable: &str, params_json: Option<&str>) -> HostRunnableInvocation {
        HostRunnableInvocation {
            occurrence_id: meerkat::OccurrenceId::new(),
            schedule_id: meerkat::ScheduleId::new(),
            delivery_idempotency_key: "conformance-callback-key".to_string(),
            runnable: runnable_name(runnable),
            trigger_time: chrono::Utc::now(),
            params: params_json
                .map(|text| RawValue::from_string(text.to_string()).expect("valid raw params")),
        }
    }

    #[tokio::test]
    async fn callback_runnable_fires_over_the_bridge_with_the_occurrence_shape() {
        let bridge = ScriptedCallbackBridge::new(Ok(json!({"digest": "sent"})));
        let host = gateway_runnable_host(
            None,
            Some((
                Arc::clone(&bridge) as Arc<dyn CallbackBridge>,
                vec![runnable_name("digest")],
            )),
        )
        .expect("compose runnable host")
        .expect("callback runnables registered");

        assert_eq!(
            host.probe_runnable(&runnable_name("digest")),
            meerkat::RunnableProbe::Registered
        );
        assert_eq!(
            host.probe_runnable(&runnable_name("unknown")),
            meerkat::RunnableProbe::Unknown
        );

        let invocation = callback_invocation("digest", Some(r#"{"depth":3}"#));
        let outcome = host
            .run_occurrence(invocation.clone())
            .await
            .expect("bridge success completes the occurrence");
        assert_eq!(outcome, HostRunnableOutcome::completed());

        let calls = bridge.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        let (method, params) = &calls[0];
        assert_eq!(method, SCHEDULE_FIRE_CALLBACK_METHOD);
        assert_eq!(params["runnable"], json!("digest"));
        let occurrence = params["occurrence"].as_object().expect("occurrence object");
        assert_eq!(
            occurrence["schedule_id"],
            json!(invocation.schedule_id.to_string())
        );
        assert_eq!(
            occurrence["occurrence_id"],
            json!(invocation.occurrence_id.to_string())
        );
        assert_eq!(
            occurrence["due_at"],
            json!(invocation.trigger_time.to_rfc3339())
        );
        assert_eq!(occurrence["payload"], json!({"depth": 3}));
    }

    #[tokio::test]
    async fn callback_runnable_omits_payload_when_the_binding_has_no_params() {
        let bridge = ScriptedCallbackBridge::new(Ok(Value::Null));
        let host = gateway_runnable_host(
            None,
            Some((
                Arc::clone(&bridge) as Arc<dyn CallbackBridge>,
                vec![runnable_name("digest")],
            )),
        )
        .expect("compose runnable host")
        .expect("callback runnables registered");

        host.run_occurrence(callback_invocation("digest", None))
            .await
            .expect("fire without params");
        let calls = bridge.calls.lock().expect("calls lock");
        assert!(
            calls[0].1["occurrence"].get("payload").is_none(),
            "no payload key without binding params: {}",
            calls[0].1
        );
    }

    #[tokio::test]
    async fn callback_runnable_maps_bridge_error_to_runnable_failure() {
        let bridge = ScriptedCallbackBridge::new(Err("callback error: handler raised".to_string()));
        let host = gateway_runnable_host(
            None,
            Some((
                Arc::clone(&bridge) as Arc<dyn CallbackBridge>,
                vec![runnable_name("digest")],
            )),
        )
        .expect("compose runnable host")
        .expect("callback runnables registered");

        let error = host
            .run_occurrence(callback_invocation("digest", None))
            .await
            .expect_err("bridge failure must fail the occurrence");
        assert!(
            matches!(&error, HostRunnableError::Failed { detail } if detail.contains("handler raised")),
            "expected typed Failed with the bridge detail, got {error:?}"
        );
    }

    #[test]
    fn gateway_runnable_host_is_none_with_nothing_to_register() {
        assert!(
            gateway_runnable_host(None, None)
                .expect("empty composition is not an error")
                .is_none()
        );
    }

    #[test]
    fn gateway_runnable_host_rejects_duplicate_callback_names() {
        let bridge = ScriptedCallbackBridge::new(Ok(Value::Null));
        let error = match gateway_runnable_host(
            None,
            Some((
                bridge as Arc<dyn CallbackBridge>,
                vec![runnable_name("digest"), runnable_name("digest")],
            )),
        ) {
            Err(error) => error,
            Ok(_) => panic!("duplicate names must be rejected"),
        };
        assert!(error.contains("digest"), "{error}");
    }

    /// The agent-facing schedule tools and any generic RPC pass-through
    /// deserialize targets through `TargetBinding`'s serde — pin that the
    /// host-runnable wire form is accepted and creation lands in the store.
    #[tokio::test]
    async fn schedule_creation_accepts_the_host_runnable_target_wire_form() {
        let target: TargetBinding = serde_json::from_value(json!({
            "target_kind": "host_runnable",
            "runnable": "digest",
            "params": {"depth": 3}
        }))
        .expect("host_runnable target wire form deserializes");
        assert!(
            matches!(&target, TargetBinding::HostRunnable(binding) if binding.runnable == runnable_name("digest")),
            "expected host-runnable binding, got {target:?}"
        );

        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::new()));
        let created = service
            .create(CreateScheduleRequest {
                name: Some("sdk-digest".to_string()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: chrono::Utc::now() + chrono::Duration::seconds(60),
                    every_seconds: 3600,
                    end_at_utc: None,
                }),
                target,
                misfire_policy: meerkat::MisfirePolicy::default(),
                overlap_policy: meerkat::OverlapPolicy::default(),
                missing_target_policy: meerkat::MissingTargetPolicy::default(),
                labels: std::collections::BTreeMap::new(),
                planning_horizon_days: None,
                planning_horizon_occurrences: None,
            })
            .await
            .expect("create schedule with host_runnable target");
        assert!(
            matches!(&created.target, TargetBinding::HostRunnable(_)),
            "persisted schedule keeps the host-runnable target"
        );
    }

    fn dream_target_count(schedules: &[meerkat::Schedule]) -> usize {
        let runnable = HostRunnableName::parse(STEWARD_DREAM_RUNNABLE).expect("name");
        schedules
            .iter()
            .filter(|schedule| {
                matches!(
                    &schedule.target,
                    TargetBinding::HostRunnable(binding) if binding.runnable == runnable
                )
            })
            .count()
    }

    fn dream_interval_seconds(schedule: &meerkat::Schedule) -> u64 {
        match &schedule.trigger {
            TriggerSpec::Interval(spec) => spec.every_seconds,
            other => panic!("expected interval trigger, got {other:?}"),
        }
    }

    /// Author an interval schedule (host-runnable target: no session machinery
    /// needed) against a REAL sqlite store and plan its horizon, mirroring what
    /// the firing driver's tick preflight sees.
    async fn seed_sqlite_schedule(
        dir: &std::path::Path,
        start_at_utc: chrono::DateTime<chrono::Utc>,
    ) -> (ScheduleService, std::path::PathBuf) {
        let store_path = dir.join(SCHEDULE_STORE_FILE);
        let store = SqliteScheduleStore::open(&store_path).expect("open sqlite schedule store");
        let service = ScheduleService::new(Arc::new(store));
        let created = service
            .create(CreateScheduleRequest {
                name: Some("watchdog-fixture".to_string()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc,
                    every_seconds: 3600,
                    end_at_utc: None,
                }),
                target: TargetBinding::host_runnable(HostRunnableTargetBinding {
                    runnable: HostRunnableName::parse("watchdog.fixture").expect("runnable name"),
                    params: None,
                }),
                misfire_policy: meerkat::MisfirePolicy::default(),
                overlap_policy: meerkat::OverlapPolicy::default(),
                missing_target_policy: meerkat::MissingTargetPolicy::default(),
                labels: std::collections::BTreeMap::new(),
                planning_horizon_days: None,
                planning_horizon_occurrences: None,
            })
            .await
            .expect("create schedule");
        service
            .refill_horizon(&created.schedule_id)
            .await
            .expect("plan occurrences");
        (service, store_path)
    }

    fn corrupt_first_row(store_path: &std::path::Path, table: &str, json_column: &str) {
        let conn = rusqlite::Connection::open(store_path).expect("open store for corruption");
        let changed = conn
            .execute(
                &format!(
                    "UPDATE {table} SET {json_column} = X'7B22706F69736F6E22' \
                     WHERE rowid = (SELECT MIN(rowid) FROM {table})"
                ),
                [],
            )
            .expect("corrupt row");
        assert_eq!(changed, 1, "one {table} row corrupted");
    }

    /// Rewrite the first planned occurrence to be due at `overdue` — both the
    /// projection's `due_at_utc` and the machine state's `due_at_utc_ms`
    /// (they are recovery-checked against each other), plus the indexed
    /// ordering column the SQL prefilters scan. The misfire deadline shifts
    /// by the same amount so due-classification still answers ClaimEligible.
    /// Returns the aged row's rowid so callers can stage neighbors around it.
    fn age_first_occurrence(
        store_path: &std::path::Path,
        overdue: chrono::DateTime<chrono::Utc>,
    ) -> i64 {
        let conn = rusqlite::Connection::open(store_path).expect("open store");
        let (rowid, bytes): (i64, Vec<u8>) = conn
            .query_row(
                "SELECT rowid, occurrence_json FROM schedule_occurrences \
                 ORDER BY rowid LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read first occurrence");
        let mut json: serde_json::Value = serde_json::from_slice(&bytes).expect("occurrence json");
        let old_due_ms = json["machine_state"]["due_at_utc_ms"]
            .as_i64()
            .expect("machine due ms");
        let shift = old_due_ms - overdue.timestamp_millis();
        json["due_at_utc"] = serde_json::Value::String(overdue.to_rfc3339());
        json["machine_state"]["due_at_utc_ms"] =
            serde_json::Value::from(overdue.timestamp_millis());
        if let Some(deadline) = json["machine_state"]["misfire_deadline_utc_ms"].as_i64() {
            json["machine_state"]["misfire_deadline_utc_ms"] =
                serde_json::Value::from(deadline - shift);
        }
        let updated = serde_json::to_vec(&json).expect("serialize occurrence");
        conn.execute(
            "UPDATE schedule_occurrences SET occurrence_json = ?1, due_at_ms = ?2 \
             WHERE rowid = ?3",
            rusqlite::params![updated, overdue.timestamp_millis(), rowid],
        )
        .expect("age occurrence");
        rowid
    }

    /// 0.8.22 semantics change: claiming due work is fenced behind a
    /// store-issued executor lease. `ScheduleStore::claim_due_occurrences` now
    /// takes the exact witness as argument #1 and there is deliberately no
    /// unfenced compatibility method, so any test that dequeues schedule work
    /// must first acquire the realm's singular firing authority (the driver
    /// does the same for its whole incarnation). Panics on `Busy`: these tests
    /// own a private tempdir store with no firing host, so a busy lease would
    /// be a real defect rather than a race worth tolerating.
    ///
    /// Note the two lease durations are different clocks: this one bounds the
    /// EXECUTOR incarnation, while `ClaimDueRequest::lease_duration` bounds the
    /// per-OCCURRENCE claim.
    async fn acquire_test_executor_lease(
        store: &dyn ScheduleStore,
        owner_id: &str,
    ) -> meerkat_schedule::ScheduleExecutorLease {
        match store
            .acquire_executor_lease(meerkat_schedule::AcquireScheduleExecutorLeaseRequest {
                owner_id: owner_id.to_string(),
                lease_duration: chrono::Duration::minutes(5),
            })
            .await
            .expect("acquire schedule executor lease")
        {
            meerkat_schedule::AcquireScheduleExecutorLeaseOutcome::Acquired(lease) => lease,
            meerkat_schedule::AcquireScheduleExecutorLeaseOutcome::Busy {
                current_owner_id,
                ..
            } => panic!(
                "a private test store must not report a busy executor lease \
                 (held by {current_owner_id})"
            ),
        }
    }

    /// meerkat 0.7.19 carry guard (asks 16-19): a poisoned occurrence row no
    /// longer starves the whole claim — the sqlite claim scan skips it as a
    /// typed row fault and claims healthy due neighbors. This is the exact
    /// Downstream app shape: one stale row, everything else due.
    #[tokio::test]
    async fn schedule_claim_tolerates_poisoned_neighbor_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Schedule A: healthy, due in the past via the aged-occurrence
        // rewrite used by the watchdog tests. Schedule B: poisoned row.
        let (service, store_path) =
            seed_sqlite_schedule(dir.path(), chrono::Utc::now() + chrono::Duration::hours(1)).await;
        // Age schedule A's first occurrence JUST past due (inside any misfire
        // window, so it classifies as claimable rather than misfired).
        let overdue = chrono::Utc::now() - chrono::Duration::seconds(5);
        let rowid = age_first_occurrence(&store_path, overdue);
        {
            let conn = rusqlite::Connection::open(&store_path).expect("open store");
            // Poison a NEIGHBOR row: insert a corrupt occurrence for the same
            // schedule so the claim scan meets it.
            conn.execute(
                "INSERT INTO schedule_occurrences \
                 (occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, \
                  phase, due_at_ms, occurrence_json) \
                 SELECT 'poisoned-row', schedule_id, schedule_revision, \
                        occurrence_ordinal + 999, phase, ?1, X'7B22706F69736F6E22' \
                 FROM schedule_occurrences WHERE rowid = ?2",
                rusqlite::params![overdue.timestamp_millis() - 1000, rowid],
            )
            .expect("insert poisoned row");
        }

        let schedule_store = service.store();
        let executor_lease =
            acquire_test_executor_lease(schedule_store.as_ref(), "carry-test-executor").await;
        let claimed = schedule_store
            .claim_due_occurrences(
                &executor_lease,
                meerkat::ClaimDueRequest {
                    owner_id: "carry-test".to_string(),
                    limit: 8,
                    lease_duration: chrono::Duration::seconds(60),
                },
            )
            .await
            .expect("claim must tolerate the poisoned neighbor (meerkat >= 0.7.19)");
        // Release before asserting so a failing assertion cannot leave the
        // realm's firing authority held for the rest of the lease window.
        schedule_store
            .release_executor_lease(executor_lease)
            .await
            .expect("release schedule executor lease");
        assert_eq!(
            claimed
                .transitions
                .iter()
                .filter(|commit| matches!(
                    commit.successor().phase,
                    meerkat::OccurrencePhase::Claimed
                ))
                .count(),
            1,
            "the healthy due occurrence must be claimed despite the poisoned neighbor"
        );
    }

    struct CountingLlmClient {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl meerkat_client::LlmClient for CountingLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }
        fn stream<'a>(
            &'a self,
            request: &'a meerkat_client::LlmRequest,
        ) -> std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<meerkat_client::LlmEvent, meerkat_client::LlmError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // meerkat 0.8.22 rejects a turn whose stream carried no normalized
            // provider accounting, so the terminal `Done` never travels alone.
            let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
                request,
                meerkat_core::Provider::OpenAI,
                meerkat_core::types::StopReason::EndTurn,
            );
            Box::pin(futures::stream::iter(vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "done".to_string(),
                    meta: None,
                }),
                Ok(usage),
                Ok(done),
            ]))
        }
        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }
        fn health_check<'life0, 'async_trait>(
            &'life0 self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<(), meerkat_client::LlmError>>
                    + Send
                    + 'async_trait,
            >,
        >
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async { Ok(()) })
        }
    }

    /// Task #50 carrier regression, DIRECT-DOOR half: redelivering the SAME
    /// occurrence through two independently-created Meta schedule hosts
    /// (no identity runtime bound, so the host's direct member-plane
    /// fallback submits) must reach Meerkat's deduplicating internal door
    /// with the SAME derived work reference. The identity-runtime door has
    /// its own crash-redelivery regression below
    /// (`internal_schedule_crash_redelivery_through_identity_runtime_executes_once`).
    #[tokio::test]
    async fn internal_schedule_redelivery_preserves_stable_work_receipt() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let llm_calls = Arc::new(AtomicUsize::new(0));
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let factory = AgentFactory::new(temp_dir.path()).comms(true);
        let session_service = Arc::new(meerkat::build_ephemeral_service(
            factory,
            Config::default(),
            4,
        ));
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "schedule-dedup"

[profiles.general]
model = "gpt-5.5"
external_addressable = true

[profiles.general.tools]
comms = true
"#,
        )
        .expect("schedule dedup mob definition");
        let mob_spec = crate::mob_handle_runtime::MobBootstrapSpec::new(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            session_service,
        )
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(CountingLlmClient {
                calls: llm_calls.clone(),
            })),
        });
        let runtime = crate::UnifiedRuntime::bootstrap(
            mob_spec,
            crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: "schedule-dedup".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            Duration::from_secs(2),
        )
        .await
        .expect("bootstrap schedule dedup runtime");
        let handle = runtime.mob_handle();
        runtime
            .spawn_many(vec![meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("general"),
                meerkat_mob::ids::AgentIdentity::from("digest-owner"),
            )])
            .await
            .expect("spawn member");
        // The member's autonomous kickoff turn also hits the counting client;
        // settle it and baseline the counter so the once-only assertion below
        // counts SCHEDULED executions, not the kickoff (the false-red shape
        // the identity-runtime twin already fixed).
        handle
            .wait_for_members_kickoff_complete(
                std::slice::from_ref(&meerkat_mob::ids::AgentIdentity::from("digest-owner")),
                // A settle bound only: the assertions below check state, not elapsed time.
                Some(Duration::from_secs(30)),
            )
            .await
            .expect("member kickoff should settle");
        let baseline_llm_calls = llm_calls.load(Ordering::SeqCst);
        let member_id = crate::member_comms_id::mob_member_id("digest-owner").to_string();
        let content = meerkat_core::ContentInput::Text("run the digest".to_string());
        let schedule = Schedule::new(CreateScheduleRequest {
            name: Some("schedule-dedup-carrier".to_string()),
            description: None,
            trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc: chrono::Utc::now(),
                every_seconds: 60,
                end_at_utc: None,
            }),
            target: TargetBinding::Mob(Box::new(MobTargetBinding::Member {
                mob_id: "schedule-dedup".to_string(),
                member_id: member_id.clone(),
                action: ScheduledMobAction::Send {
                    content: content.clone(),
                    render_metadata: None,
                },
            })),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("schedule");
        let occurrence = Occurrence::planned_from_schedule(
            &schedule,
            meerkat::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("occurrence");
        let delivery_identity = meerkat::ScheduleDeliveryIdentity::for_occurrence(&occurrence);

        let first_host = InternalDeliveryScheduleMobHost {
            inner: Arc::new(NoopScheduleMobHost::new("unused fallback")),
            handle: handle.clone(),
            identity_runtime: None,
        };
        let first = first_host
            .deliver_internal_member_prompt(
                &occurrence,
                &delivery_identity,
                "schedule-dedup",
                &member_id,
                &content,
            )
            .await
            .expect("first schedule host must own the local member delivery");
        let first_terminal = first.completion.await.expect("first terminal");
        let first_work_ref = first.receipt.detail.clone().unwrap_or_else(|| {
            panic!(
                "first receipt carries derived work ref: receipt={:?}, terminal={first_terminal:?}",
                first.receipt
            )
        });
        drop(first_host);

        // Recreate the Meta host boundary and replay the exact occurrence
        // identity, matching a driver reclaim after the previous host dies.
        let replacement_host = InternalDeliveryScheduleMobHost {
            inner: Arc::new(NoopScheduleMobHost::new("unused fallback")),
            handle,
            identity_runtime: None,
        };
        let second = replacement_host
            .deliver_internal_member_prompt(
                &occurrence,
                &delivery_identity,
                "schedule-dedup",
                &member_id,
                &content,
            )
            .await
            .expect("replacement schedule host must own the local member redelivery");
        let second_work_ref = second
            .receipt
            .detail
            .clone()
            .expect("redelivery receipt carries derived work ref");
        second.completion.await.expect("redelivery terminal");
        assert_eq!(
            first_work_ref, second_work_ref,
            "host replacement must preserve the exact derived work authority"
        );
        assert_eq!(
            second.receipt.correlation_id,
            Some(delivery_identity.correlation_id.clone())
        );

        // The stable authority above is the seam owned here. Keep a direct
        // end-to-end sanity check that the admitted work executes, then let
        // runtime shutdown drain before checking Meerkat's dedup result.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while llm_calls.load(Ordering::SeqCst) <= baseline_llm_calls {
            assert!(
                std::time::Instant::now() < deadline,
                "the deduplicated work must still execute once"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        runtime.shutdown().await;
        assert_eq!(
            llm_calls.load(Ordering::SeqCst),
            baseline_llm_calls + 1,
            "the shared work authority must execute once beyond the kickoff baseline"
        );
    }

    /// Task #50 carrier regression, IDENTITY-RUNTIME half: crash redelivery
    /// of one occurrence through the FULL Meta chain - schedule host
    /// (`deliver_mob_target`) -> `DispatchInput` (both identity halves) ->
    /// identity-runtime admission matrix -> `BridgeDelivery` ->
    /// `SessionBridge::deliver_admitted` (production `MobSessionBridge`) ->
    /// `submit_work_with_mode_and_delivery_identity` - executes the member
    /// turn exactly once. The identity is minted the way the driver mints it
    /// (`ScheduleDeliveryIdentity::for_occurrence`), the first host is
    /// dropped BEFORE the admitted work reaches a terminal (the driver
    /// reclaim shape: delivery lease expires while the work is in flight),
    /// and the replacement host replays the exact identity. If ANY hop drops
    /// the carrier, the replay mints a second unit of work and the count
    /// goes to 2; if a hop half-drops it, the typed admission refusal fails
    /// the delivery; if the identity door is bypassed, the receipt grows the
    /// direct-door work_ref stamp. All three defects turn this red.
    #[tokio::test(flavor = "multi_thread")]
    async fn internal_schedule_crash_redelivery_through_identity_runtime_executes_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let llm_calls = Arc::new(AtomicUsize::new(0));
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let factory = AgentFactory::new(temp_dir.path()).comms(true);
        let session_service = Arc::new(meerkat::build_ephemeral_service(
            factory,
            Config::default(),
            4,
        ));
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "schedule-dedup-identity"

[profiles.general]
model = "gpt-5.5"
external_addressable = false

[profiles.general.tools]
comms = true
"#,
        )
        .expect("schedule dedup identity mob definition");
        let mob_spec = crate::mob_handle_runtime::MobBootstrapSpec::new(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            session_service,
        )
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(CountingLlmClient {
                calls: llm_calls.clone(),
            })),
        });
        let runtime = crate::UnifiedRuntime::bootstrap(
            mob_spec,
            crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: "schedule-dedup-identity".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            Duration::from_secs(2),
        )
        .await
        .expect("bootstrap schedule dedup identity runtime");
        let handle = runtime.mob_handle();

        // An identity-first member: the roster id is the mk--encoded
        // generated runtime alias, exactly the shape a downstream app schedules
        // address (the binding stores the roster id).
        let roster_id = crate::member_comms_id::mob_member_id_str("digest:main").into_owned();
        assert!(
            roster_id.starts_with("mk--"),
            "precondition: identity-first roster ids are marker-encoded"
        );
        let roster_identity = meerkat_mob::ids::AgentIdentity::from(roster_id.clone());
        handle
            .ensure_member(meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("general"),
                roster_identity.clone(),
            ))
            .await
            .expect("spawn identity-first member");
        handle
            .wait_for_members_kickoff_complete(
                std::slice::from_ref(&roster_identity),
                // A settle bound only: the assertions below check state, not elapsed time.
                Some(Duration::from_secs(30)),
            )
            .await
            .expect("identity-first member kickoff should settle");
        let baseline_llm_calls = llm_calls.load(Ordering::SeqCst);

        // Durable identity authority over that member, bridged to the mob by
        // the PRODUCTION MobSessionBridge (the gateway wiring). The decoded
        // roster id IS the durable identity, and Core's journal binds the
        // member spawned above: the identity runtime reads it from there.
        let public_member_alias =
            crate::member_comms_id::runtime_alias_str(&roster_id).into_owned();
        let identity = crate::identity_first::AgentIdentity::parse(&public_member_alias)
            .expect("roster id must decode to a durable identity");
        let continuity_store = Arc::new(
            crate::identity_first::LocalContinuityStore::in_memory().expect("identity store"),
        );
        let lease_provider = Arc::new(crate::identity_first::LocalLeaseProvider::new());
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store,
                lease_provider,
                runtime_instance_id: "schedule-dedup-identity".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(crate::identity_first::MobSessionBridge::new(
                    handle.clone(),
                ))),
                default_timeout: None,
            },
        ));
        identity_runtime
            .register(crate::identity_first::DurableAgentSpec {
                identity,
                profile: meerkat_mob::ProfileName::from("general"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await;

        let schedule = Schedule::new(CreateScheduleRequest {
            name: Some("schedule-dedup-identity-carrier".to_string()),
            description: None,
            trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc: chrono::Utc::now(),
                every_seconds: 60,
                end_at_utc: None,
            }),
            target: TargetBinding::Mob(Box::new(MobTargetBinding::Member {
                mob_id: "schedule-dedup-identity".to_string(),
                member_id: roster_id,
                action: ScheduledMobAction::Send {
                    content: meerkat_core::ContentInput::Text("run the digest".to_string()),
                    render_metadata: None,
                },
            })),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("schedule");
        let occurrence = Occurrence::planned_from_schedule(
            &schedule,
            meerkat::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("occurrence");
        let TargetBinding::Mob(binding) = &schedule.target else {
            unreachable!("test schedule is mob-targeted")
        };
        // Minted exactly the way the driver mints it at delivery time.
        let delivery_identity = meerkat::ScheduleDeliveryIdentity::for_occurrence(&occurrence);

        let first_host = InternalDeliveryScheduleMobHost {
            inner: Arc::new(NoopScheduleMobHost::new("unused fallback")),
            handle: handle.clone(),
            identity_runtime: Some(identity_runtime.clone()),
        };
        let first = first_host
            .deliver_mob_target(&occurrence, &delivery_identity, binding)
            .await
            .expect("first host delivery");
        assert!(
            first.receipt.detail.is_none(),
            "the identity-runtime door must handle this member (the direct \
             fallback door stamps a work_ref detail): {:?}",
            first.receipt.detail
        );
        assert_eq!(
            first.receipt.correlation_id,
            Some(delivery_identity.correlation_id.clone())
        );
        let first_terminal = first.completion.await.expect("first delivery terminal");
        assert_eq!(
            first_terminal.phase,
            OccurrencePhase::Completed,
            "identity-door delivery must be admitted: {:?}",
            first_terminal.detail
        );

        // Kill the host boundary BEFORE the admitted work reaches a
        // terminal (no settle wait), then replay the exact occurrence
        // identity through a replacement host - the driver's lease-expiry
        // reclaim shape.
        drop(first_host);
        let replacement_host = InternalDeliveryScheduleMobHost {
            inner: Arc::new(NoopScheduleMobHost::new("unused fallback")),
            handle,
            identity_runtime: Some(identity_runtime),
        };
        let second = replacement_host
            .deliver_mob_target(&occurrence, &delivery_identity, binding)
            .await
            .expect("replacement host redelivery");
        assert!(
            second.receipt.detail.is_none(),
            "redelivery must also take the identity-runtime door: {:?}",
            second.receipt.detail
        );
        assert_eq!(
            second.receipt.correlation_id,
            Some(delivery_identity.correlation_id.clone())
        );
        let second_terminal = second.completion.await.expect("redelivery terminal");
        assert_eq!(
            second_terminal.phase,
            OccurrencePhase::Completed,
            "redelivery of the same identity must dedup, not refuse: {:?}",
            second_terminal.detail
        );

        // Exactly one member-turn execution across both deliveries.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while llm_calls.load(Ordering::SeqCst) <= baseline_llm_calls {
            assert!(
                std::time::Instant::now() < deadline,
                "the admitted work must still execute once"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        runtime.shutdown().await;
        assert_eq!(
            llm_calls.load(Ordering::SeqCst),
            baseline_llm_calls + 1,
            "crash redelivery of one occurrence identity must execute the \
             member turn exactly once end to end"
        );
    }

    #[tokio::test]
    async fn identity_dispatch_without_session_bridge_is_terminal_runtime_rejected() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let factory = AgentFactory::new(temp_dir.path()).comms(true);
        let session_service = Arc::new(meerkat::build_ephemeral_service(
            factory,
            Config::default(),
            4,
        ));
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "schedule-none"

[profiles.general]
model = "gpt-5.5"
external_addressable = true
"#,
        )
        .expect("schedule test mob definition");
        let mob_spec = crate::mob_handle_runtime::MobBootstrapSpec::new(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            session_service,
        )
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            // `TestClient::default()` DOES synthesize accounting under 0.8.22,
            // but under `Provider::Other` - and every profile here is
            // `gpt-5.5`, whose canonical owner is `Provider::OpenAI`, so the
            // turn would fail closed with
            // `normalized_provider_accounting_identity_mismatch`. See the rule
            // in `tests/support/llm_usage.rs`.
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::for_provider(
                meerkat_core::Provider::OpenAI,
            ))),
        });
        let runtime = crate::UnifiedRuntime::bootstrap(
            mob_spec,
            crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: "schedule-none".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            Duration::from_secs(2),
        )
        .await
        .expect("bootstrap schedule test runtime");

        let identity =
            crate::identity_first::AgentIdentity::parse("domain:scheduled").expect("identity");
        let alias = "rt:domain:scheduled:0";
        let lease_provider = Arc::new(crate::identity_first::LocalLeaseProvider::new());
        // No session bridge: the registered identity has no Core to read, so
        // it is Dormant and its scheduled delivery has nothing to wake.
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()
                        .expect("identity store"),
                ),
                lease_provider,
                runtime_instance_id: "schedule-none".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: None,
                default_timeout: None,
            },
        ));
        identity_runtime
            .register(crate::identity_first::DurableAgentSpec {
                identity,
                profile: meerkat_mob::ProfileName::from("general"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await;

        let schedule = Schedule::new(CreateScheduleRequest {
            name: Some("identity-no-bridge".to_string()),
            description: None,
            trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc: chrono::Utc::now(),
                every_seconds: 60,
                end_at_utc: None,
            }),
            target: TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id: SessionId::new(),
                action: ScheduledSessionAction::Prompt {
                    prompt: meerkat_core::ContentInput::Text("deliver".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("schedule");
        let occurrence = Occurrence::planned_from_schedule(
            &schedule,
            meerkat::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("occurrence");
        let host = InternalDeliveryScheduleMobHost {
            inner: Arc::new(NoopScheduleMobHost::new("unused fallback")),
            handle: runtime.mob_handle(),
            identity_runtime: Some(identity_runtime),
        };

        let dispatch = host
            .deliver_internal_member_prompt(
                &occurrence,
                &meerkat::ScheduleDeliveryIdentity::for_occurrence(&occurrence),
                "schedule-none",
                alias,
                &meerkat_core::ContentInput::Text("deliver".to_string()),
            )
            .await
            .expect("identity dispatch must be handled terminally");
        let terminal = dispatch.completion.await.expect("delivery terminal");
        assert_eq!(terminal.phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            terminal.delivery_failure_reason,
            Some(meerkat::DeliveryFailureReason::RuntimeRejected)
        );
        // Without a session bridge Core holds no member for the identity, so
        // the generated alias the delivery names is not current: the identity
        // runtime refuses it as a stale alias, typed, before anything is
        // delivered, and the occurrence records that refusal as its detail.
        assert!(
            terminal.detail.as_deref().is_some_and(|detail| {
                detail.contains("identity schedule delivery failed (member rt:domain:scheduled:0)")
                    && detail.contains("stale runtime alias for domain:scheduled")
            }),
            "{:?}",
            terminal.detail
        );

        runtime.shutdown().await;
    }

    /// A downstream app on 0.7.26 "last link" e2e: an agent-authored one-shot must
    /// DELIVER through the real schedule host — the full rpc_gateway chain
    /// (attach_schedule_tools_with_identity_targets → agent tool dispatch →
    /// planning → claim → delivery). `register_mob_state` toggles whether
    /// the mob-target registry can resolve the authoring session at create
    /// time (the field failure fired on the unresolved path).
    async fn one_shot_delivery_e2e(register_mob_state: bool) -> (String, String) {
        one_shot_delivery_e2e_with_addressability(register_mob_state, true).await
    }

    async fn one_shot_delivery_e2e_with_addressability(
        register_mob_state: bool,
        external_addressable: bool,
    ) -> (String, String) {
        one_shot_delivery_e2e_with_member(register_mob_state, external_addressable, "digest-owner")
            .await
    }

    async fn one_shot_delivery_e2e_with_member(
        register_mob_state: bool,
        external_addressable: bool,
        member_identity: &str,
    ) -> (String, String) {
        use meerkat_core::AgentToolDispatcher;
        use serde_json::value::RawValue;

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let state = temp_dir.path().join("state");
        std::fs::create_dir_all(&state).expect("state dir");
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
            meerkat_store::SqliteSessionStore::open(state.join("sessions.db"))
                .expect("session store"),
        );
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))
                .expect("runtime store"),
        );
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let factory = meerkat::AgentFactory::new(&state).comms(true);
        let mut inner_builder = FactoryAgentBuilder::new(factory, Config::default());
        inner_builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
            session_store.clone(),
        )));
        inner_builder.default_blob_store = Some(blob_store.clone());
        let jobs: Arc<dyn meerkat::DetachedJobStore> =
            Arc::new(meerkat::MemoryDetachedJobStore::new());
        inner_builder.default_detached_job_store = Some(Arc::clone(&jobs));
        let delivery =
            crate::mob_handle_runtime::MobRuntimeDelivery::new(Arc::clone(&runtime_store), jobs);
        let attached = attach_schedule_tools_with_identity_targets(&inner_builder, &state)
            .expect("schedule tools attach");
        let inner_builder_mob_tools_slot = Arc::clone(&inner_builder.default_mob_tools);
        let adapter = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(
                Arc::clone(&runtime_store),
                Arc::clone(&blob_store),
            )
            .expect("acquire the schedule delivery fixture runtime machine"),
        );
        let concrete = Arc::new(PersistentSessionService::new(
            inner_builder,
            16,
            session_store,
            runtime_store,
            blob_store,
        ));
        static NEXT_DELIVERY_MOB: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        // Per-call mob id: 0.8.23's fail-closed in-proc registration means
        // concurrently running tests must not share a supervisor route.
        let delivery_mob_id = format!(
            "delivery-e2e-{}",
            NEXT_DELIVERY_MOB.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{delivery_mob_id}"

[profiles.general]
model = "gpt-5.5"
external_addressable = {external_addressable}

[profiles.general.tools]
comms = true
schedule = true
"#
        ))
        .expect("definition");
        let agent_mob_tools_slot = Arc::clone(&inner_builder_mob_tools_slot);
        let mob_spec = crate::mob_handle_runtime::MobBootstrapSpec::new(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            concrete.clone(),
        )
        .with_session_runtime_adapter(adapter.clone())
        .expect("acquire the fixture session runtime owner")
        .with_runtime_delivery(delivery)
        .with_agent_mob_tools(agent_mob_tools_slot)
        .expect("install the schedule delivery fixture's agent mob tools")
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            // `TestClient::default()` DOES synthesize accounting under 0.8.22,
            // but under `Provider::Other` - and every profile here is
            // `gpt-5.5`, whose canonical owner is `Provider::OpenAI`, so the
            // turn would fail closed with
            // `normalized_provider_accounting_identity_mismatch`. See the rule
            // in `tests/support/llm_usage.rs`.
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::for_provider(
                meerkat_core::Provider::OpenAI,
            ))),
        });
        let runtime = crate::UnifiedRuntime::bootstrap(
            mob_spec,
            crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: "delivery-e2e".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            std::time::Duration::from_secs(2),
        )
        .await
        .expect("bootstrap");

        let handle = runtime.mob_handle();
        handle
            .ensure_member(meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("general"),
                meerkat_mob::ids::AgentIdentity::from(member_identity),
            ))
            .await
            .expect("ensure schedule author member");
        let owner_session = handle
            .resolve_bridge_session_id_observation(&meerkat_mob::ids::AgentIdentity::from(
                member_identity,
            ))
            .await
            .expect("owner session id");

        // A generated runtime alias is not merely a funny-looking raw member
        // id: give the schedule host the same durable authority the gateway
        // carries in production. This keeps the encoded-roster regression
        // realistic while ensuring an authority-less host remains fail-closed.
        let public_member_alias =
            crate::member_comms_id::runtime_alias_str(member_identity).into_owned();
        let identity_runtime =
            if crate::member_comms_id::is_reserved_generated_alias(&public_member_alias) {
                let identity =
                    crate::identity_first::IdentityRuntime::identity_for_generated_member_alias(
                        &public_member_alias,
                    )
                    .expect("generated alias identity");
                let continuity_store = Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()
                        .expect("identity continuity store"),
                );
                let lease_provider = Arc::new(crate::identity_first::LocalLeaseProvider::new());
                let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
                    crate::identity_first::IdentityRuntimeConfig {
                        continuity_store,
                        lease_provider,
                        runtime_instance_id: "delivery-e2e".to_string(),
                        has_runtime_store: true,
                        durability_policy:
                            crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                        bridge: Some(Arc::new(crate::identity_first::MobSessionBridge::new(
                            handle.clone(),
                        ))),
                        default_timeout: None,
                    },
                ));
                identity_runtime
                    .register(crate::identity_first::DurableAgentSpec {
                        identity,
                        profile: meerkat_mob::ProfileName::from("general"),
                        addressability: crate::identity_first::AgentAddressability::Addressable,
                        display_name: None,
                        labels: std::collections::BTreeMap::new(),
                        context: None,
                        additional_instructions: Vec::new(),
                        initial_message: None,
                        runtime_mode_override: None,
                        backend: None,
                        binding: None,
                        placement: None,
                    })
                    .await;
                Some(identity_runtime)
            } else {
                None
            };

        let mob_state = runtime.mob_runtime().agent_mob_mcp_state();
        assert!(
            mob_state.is_some(),
            "with_agent_mob_tools must install the mob authority"
        );
        if register_mob_state {
            attached
                .mob_target_registry
                .set_mob_state(mob_state.clone());
        }

        // Author AS THE AGENT: through the installed dispatcher chain, bound
        // to the member's session, current-session target, due just-past.
        let mob_dispatcher = MobIdentityScheduleToolDispatcher::new(
            Arc::new(ScheduleToolDispatcher::new(attached.service.clone())),
            attached.mob_target_registry.clone(),
        );
        let dispatcher = meerkat_schedule::CurrentSessionScheduleToolDispatcher::new(
            Arc::new(mob_dispatcher),
            owner_session.clone(),
        );
        let due = chrono::Utc::now() + chrono::Duration::seconds(2);
        let create_args = RawValue::from_string(
            serde_json::json!({
                "name": "delivery-e2e-oneshot",
                "trigger": { "type": "once", "due_at_utc": due.to_rfc3339() },
                "target": {
                    "target_kind": "session",
                    "type": "current_session",
                    "action": {
                        "type": "prompt",
                        "prompt": "Fire the e2e one-shot.",
                        "render_metadata": { "class": "external_event", "salience": "important", "source": "delivery-e2e" }
                    }
                }
            })
            .to_string(),
        )
        .expect("create args");
        dispatcher
            .dispatch(meerkat_core::types::ToolCallView {
                id: "create-e2e",
                name: "meerkat_schedule_create",
                args: &create_args,
            })
            .await
            .expect("author one-shot through the agent dispatcher");
        let authored_target = attached
            .service
            .list()
            .await
            .expect("list")
            .first()
            .map(|s| s.target.clone());
        if register_mob_state {
            assert!(
                matches!(
                    &authored_target,
                    Some(TargetBinding::Mob(binding))
                        if matches!(binding.as_ref(), MobTargetBinding::Member { .. })
                ),
                "with the mob authority registered, agent-authored current-session \
                 schedules must rewrite to mob-member targets: {authored_target:?}"
            );
        }

        // Ensure mob state is registered for DELIVERY either way (rpc_gateway
        // always sets it before spawning the host).
        attached
            .mob_target_registry
            .set_mob_state(mob_state.clone());
        // A workgraph service on the host exercises upstream arg-8
        // pass-through on every delivery e2e (overlay injection is inert
        // without attention bindings).
        let _host = spawn_schedule_host_with_identity_runtime(
            concrete,
            adapter,
            attached.service.clone(),
            mob_state,
            handle.clone(),
            identity_runtime,
            None,
            Some(crate::workgraph_wiring::ephemeral_workgraph_service(
                "delivery-e2e",
            )),
            "delivery-e2e",
        )
        .expect("spawn schedule host");

        // Poll receipts until one lands (or timeout) — receipts are keyed by
        // occurrence, so read raw rows from the store file.
        let receipts_path = state.join(SCHEDULE_STORE_FILE);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let row: Option<Vec<u8>> = {
                let conn = rusqlite::Connection::open(&receipts_path).expect("open receipts store");
                conn.query_row(
                    "SELECT receipt_json FROM schedule_receipts ORDER BY rowid DESC LIMIT 1",
                    [],
                    |r| r.get::<_, Vec<u8>>(0),
                )
                .ok()
            };
            if let Some(receipt_bytes) = row {
                let receipt_json = String::from_utf8_lossy(&receipt_bytes).to_string();
                let receipt: serde_json::Value =
                    serde_json::from_str(&receipt_json).unwrap_or_default();
                let stage = receipt["stage"].as_str().unwrap_or_default().to_string();
                // Only a TERMINAL delivery verdict counts — planner
                // bookkeeping (superseded) and in-flight stages
                // (dispatch_started/accepted) keep the poll waiting.
                if matches!(
                    stage.as_str(),
                    "completed" | "delivery_failed" | "misfired" | "skipped"
                ) {
                    runtime.shutdown().await;
                    return (stage, receipt_json);
                }
            }
            if std::time::Instant::now() >= deadline {
                let conn = rusqlite::Connection::open(&receipts_path).expect("open store for dump");
                let mut stmt = conn
                    .prepare(
                        "SELECT phase, due_at_ms, lease_expires_at_ms FROM schedule_occurrences",
                    )
                    .expect("prep");
                let rows: Vec<(String, i64, Option<i64>)> = stmt
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .expect("q")
                    .filter_map(Result::ok)
                    .collect();
                panic!("no delivery receipt within 20s; occurrences: {rows:?}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    /// Registry-resolved authoring (rpc_gateway steady state): the one-shot
    /// rewrites to a mob-member target at authoring and DELIVERS.
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_authored_one_shot_delivers_to_mob_member() {
        let (stage, detail) = one_shot_delivery_e2e(true).await;
        assert_eq!(
            stage, "completed",
            "one-shot must deliver to the member: {detail}"
        );
    }

    /// Downstream app field case: domain agents are internal_only by design (only
    /// person identities are externally addressable). A schedule firing back
    /// into ITS OWN AUTHOR's session is internal addressing — the external
    /// addressability posture must not block self-delivery.
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_authored_one_shot_delivers_to_internal_only_author() {
        let (stage, detail) = one_shot_delivery_e2e_with_addressability(true, false).await;
        assert_eq!(
            stage, "completed",
            "self-delivery to an internal_only author must succeed: {detail}"
        );
    }

    /// A downstream app on 0.7.28 field case: identity-first bridge members' ROSTER ids
    /// are comms-ENCODED (`mk--domain_cops` for the durable identity
    /// `domain:ops` - bridge.rs member_id_for_spawn_spec), and the
    /// authoring rewrite stores that roster id in the binding. The internal
    /// lane must resolve it WITHOUT re-encoding (the codec re-encodes
    /// marker-prefixed input by design); before the canonicalization fix the
    /// lookup missed and delivery fell through to the external door:
    /// "mob member is not externally addressable: mk--rt_cdomain_cops_c0".
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_authored_one_shot_delivers_to_internal_only_identity_bridge_member() {
        let roster_id = crate::member_comms_id::mob_member_id_str("domain:ops").into_owned();
        assert!(
            roster_id.starts_with("mk--"),
            "repro precondition: the roster id must be marker-encoded"
        );
        let (stage, detail) = one_shot_delivery_e2e_with_member(true, false, &roster_id).await;
        assert_eq!(
            stage, "completed",
            "self-delivery to an internal_only identity-bridge member (roster-space \
             binding id) must take the internal lane: {detail}"
        );
    }

    /// Unresolved-at-authoring shape (the downstream app's 0.7.26 field path): the
    /// target stays a resumable session, and DELIVERY-TIME recovery resolves
    /// the mob-member identity through the (now installed) mob authority.
    /// Before the with_agent_mob_tools fix this failed with
    /// "scheduled identity targets are not supported by this session host".
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_authored_one_shot_delivers_via_identity_recovery() {
        let (stage, detail) = one_shot_delivery_e2e(false).await;
        assert_eq!(
            stage, "completed",
            "delivery-time identity recovery must deliver: {detail}"
        );
    }

    /// Ask-22 upgrade-carry guard: a just-past ONE-SHOT must converge —
    /// bounded occurrences over repeated refill+claim rounds.
    ///
    /// Root cause (found by Luka, fix targeted at meerkat 0.7.20):
    /// sub-millisecond precision loss in the planning-cursor round-trip —
    /// the cursor is machine-owned at ms precision (truncate_ms(due)) while
    /// `next_due_after(Once, cursor)` compares at ns precision, so
    /// `due > cursor` stays true forever and the planner re-yields the same
    /// due each tick (one spawn per tick ≈ the field's ~1/sec, unbounded).
    /// Reproduces with PURE meerkat service APIs (refill_horizon +
    /// claim_due_occurrences) — no meta code in the loop; the claim
    /// watchdog is read-only and ticks at 60s, exonerated twice over.
    /// Fixed in meerkat 0.7.20: trigger yields/compares ms-truncated dues,
    /// and the ScheduleLifecycleMachine owns a planning-monotonicity
    /// invariant so future representation bugs converge as refill faults.
    #[tokio::test]
    async fn one_shot_misfire_must_not_regenerate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store_path = dir.path().join(SCHEDULE_STORE_FILE);
        let store = SqliteScheduleStore::open(&store_path).expect("open store");
        let service = ScheduleService::new(Arc::new(store));
        // One-shot due WELL past (beyond any catch-up window → misfires).
        let created = service
            .create(CreateScheduleRequest {
                name: Some("oneshot-runaway-probe".to_string()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() - chrono::Duration::seconds(30),
                },
                target: TargetBinding::host_runnable(HostRunnableTargetBinding {
                    runnable: HostRunnableName::parse("runaway.probe").expect("name"),
                    params: None,
                }),
                misfire_policy: meerkat::MisfirePolicy::default(),
                overlap_policy: meerkat::OverlapPolicy::default(),
                missing_target_policy: meerkat::MissingTargetPolicy::default(),
                labels: std::collections::BTreeMap::new(),
                planning_horizon_days: None,
                planning_horizon_occurrences: None,
            })
            .await
            .expect("create one-shot");

        let count_occurrences = |path: std::path::PathBuf| -> i64 {
            let conn = rusqlite::Connection::open(path).expect("open");
            conn.query_row("SELECT COUNT(*) FROM schedule_occurrences", [], |r| {
                r.get(0)
            })
            .expect("count")
        };

        // DEBUG: dump schedule + occurrence bookkeeping after create.
        {
            let conn = rusqlite::Connection::open(&store_path).expect("open");
            let (rev, cursor): (i64, Option<i64>) = conn
                .query_row(
                    "SELECT revision, planning_cursor_at_ms FROM schedule_schedules LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .expect("schedule row");
            eprintln!("after create: schedule revision={rev} planning_cursor_ms={cursor:?}");
            let mut stmt = conn
                .prepare(
                    "SELECT occurrence_ordinal, schedule_revision, phase FROM schedule_occurrences",
                )
                .expect("prep");
            let rows: Vec<(i64, i64, String)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .expect("q")
                .filter_map(Result::ok)
                .collect();
            eprintln!("occurrences after create: {rows:?}");
        }
        // Simulate driver ticks: refill + claim, several rounds.
        // 0.8.22: claiming is fenced behind a store-issued executor lease, so
        // the simulated driver holds ONE executor incarnation across all ticks
        // (a real driver does the same); 5 minutes covers six fast rounds, so
        // no renewal is needed.
        let schedule_store = service.store();
        let executor_owner = "runaway-probe-executor";
        let executor_lease =
            acquire_test_executor_lease(schedule_store.as_ref(), executor_owner).await;
        for round in 0..6 {
            let _ = service.refill_horizon(&created.schedule_id).await;
            let _ = schedule_store
                .claim_due_occurrences(
                    &executor_lease,
                    meerkat::ClaimDueRequest {
                        owner_id: "runaway-probe".to_string(),
                        limit: 8,
                        lease_duration: chrono::Duration::seconds(60),
                    },
                )
                .await;
            {
                let conn = rusqlite::Connection::open(&store_path).expect("open");
                let (rev, cursor): (i64, Option<i64>) = conn
                    .query_row(
                        "SELECT revision, planning_cursor_at_ms FROM schedule_schedules LIMIT 1",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .expect("schedule row");
                eprintln!(
                    "round {round}: occurrences = {} schedule_rev={rev} cursor_ms={cursor:?}",
                    count_occurrences(store_path.clone())
                );
            }
        }
        // The per-round claim is deliberately `let _ =` (a benign claim error
        // must not fail this planner-convergence guard). Under 0.8.22 that
        // swallow also hides `ExecutorLeaseStale`, so the simulated driver
        // could silently degrade to "refill only" and still go green. Observe
        // the store's firing authority to prove every round claimed under a
        // live lease. Test 1 needs no such guard: its claim is `.expect`-
        // checked, so a stale lease already fails it loudly.
        let observation = schedule_store
            .observe_executor_lease()
            .await
            .expect("observe schedule executor lease");
        // Release before asserting so a failing assertion cannot leave the
        // realm's firing authority held for the rest of the lease window.
        schedule_store
            .release_executor_lease(executor_lease)
            .await
            .expect("release schedule executor lease");
        let active = observation
            .active
            .expect("the simulated driver's executor lease must still be live after six rounds");
        assert_eq!(
            active.owner_id.as_str(),
            executor_owner,
            "every round's claim must have run under this test's own firing authority"
        );
        let total = count_occurrences(store_path.clone());
        assert!(
            total <= 1,
            "a one-shot must never regenerate after misfire; got {total} occurrences"
        );
    }

    fn watchdog_time(value: &str) -> chrono::DateTime<chrono::Utc> {
        value.parse().expect("fixed watchdog fixture time")
    }

    fn watchdog_calendar(hours: Vec<u32>, minute: u32) -> TriggerSpec {
        use meerkat_schedule::{CalendarFieldSpec, CalendarTriggerSpec};
        TriggerSpec::Calendar(CalendarTriggerSpec {
            timezone: "UTC".to_string(),
            minute: CalendarFieldSpec::Values(vec![minute]),
            hour: CalendarFieldSpec::Values(hours),
            day_of_month: CalendarFieldSpec::Any,
            month: CalendarFieldSpec::Any,
            day_of_week: CalendarFieldSpec::Any,
            year: None,
        })
    }

    /// Persist only owner-issued schedule/occurrence writes. Delaying the
    /// planned occurrence writes until after a parent transition stages
    /// historical leftover Pending rows, not scheduling execution policy.
    async fn seed_watchdog_parent(
        store: &dyn ScheduleStore,
        trigger: TriggerSpec,
        revision: u64,
        phase: SchedulePhase,
        at_utc: chrono::DateTime<chrono::Utc>,
        dues: &[chrono::DateTime<chrono::Utc>],
    ) -> Schedule {
        use meerkat_schedule::{OccurrenceOrdinal, ScheduleLifecycleInput};
        let write = Schedule::apply(
            None,
            ScheduleLifecycleInput::Create(CreateScheduleRequest {
                name: Some("watchdog-parent-fixture".to_string()),
                description: None,
                trigger,
                target: TargetBinding::host_runnable(HostRunnableTargetBinding {
                    runnable: HostRunnableName::parse("watchdog.fixture").expect("runnable"),
                    params: None,
                }),
                misfire_policy: meerkat::MisfirePolicy::default(),
                overlap_policy: meerkat::OverlapPolicy::default(),
                missing_target_policy: meerkat::MissingTargetPolicy::default(),
                labels: BTreeMap::new(),
                planning_horizon_days: None,
                planning_horizon_occurrences: None,
            }),
        )
        .expect("owner-issued schedule")
        .into_authorized_write();
        let mut schedule = store
            .commit_schedule_write(write)
            .await
            .expect("persist schedule")
            .into_parts()
            .1;
        while schedule.revision.0 < revision {
            let request = UpdateScheduleRequest {
                misfire_policy: Some(meerkat::MisfirePolicy::CatchUpWithin {
                    window_seconds: 86400 + schedule.revision.0,
                }),
                ..UpdateScheduleRequest::default()
            };
            let write = Schedule::apply(
                Some(schedule),
                ScheduleLifecycleInput::Update { request, at_utc },
            )
            .expect("owner-issued revision")
            .into_authorized_write();
            schedule = store
                .commit_schedule_mutation(write, Vec::new())
                .await
                .expect("persist revision")
                .into_parts()
                .0
                .into_parts()
                .1;
        }
        let pending: Vec<_> = dues
            .iter()
            .enumerate()
            .map(|(ordinal, due)| {
                Occurrence::planned_write_from_schedule(
                    &schedule,
                    OccurrenceOrdinal(ordinal as u64),
                    *due,
                )
                .expect("owner-issued pending occurrence")
            })
            .collect();
        let transition = match phase {
            SchedulePhase::Active => None,
            SchedulePhase::Paused => Some(ScheduleLifecycleInput::Pause { at_utc }),
            SchedulePhase::Deleted => Some(ScheduleLifecycleInput::Delete { at_utc }),
        };
        if let Some(transition) = transition {
            schedule = store
                .commit_schedule_mutation(
                    Schedule::apply(Some(schedule), transition)
                        .expect("owner-issued parent phase")
                        .into_authorized_write(),
                    Vec::new(),
                )
                .await
                .expect("persist parent phase")
                .into_parts()
                .0
                .into_parts()
                .1;
        }
        store
            .commit_occurrence_writes(pending)
            .await
            .expect("persist historical pending rows");
        assert_eq!(schedule.phase, phase);
        schedule
    }

    #[derive(Clone, Default)]
    struct WatchdogLogCapture(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for WatchdogLogCapture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log capture").extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for WatchdogLogCapture {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    async fn capture_watchdog_probe_at(
        service: &ScheduleService,
        path: &Path,
        now: chrono::DateTime<chrono::Utc>,
    ) -> (ScheduleFiringProbe, String) {
        use tracing::instrument::WithSubscriber;
        let capture = WatchdogLogCapture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .without_time()
            .finish();
        // A second registered Dispatch keeps callsite interest from being
        // cached `never` by threads without a subscriber.
        let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
        let probe = async {
            let config = ScheduleClaimWatchdogConfig::default();
            let probe =
                probe_schedule_firing_pipeline_at(service, path, config.overdue_threshold, now)
                    .await;
            probe.log_at_boot(config);
            ScheduleClaimWatchdogState::default().observe(probe.clone(), config);
            probe
        }
        .with_subscriber(subscriber)
        .await;
        let log =
            String::from_utf8(capture.0.lock().expect("log capture").clone()).expect("UTF-8 logs");
        (probe, log)
    }

    #[tokio::test]
    async fn schedule_claim_watchdog_nine_paused_rows_are_not_a_stall() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(SCHEDULE_STORE_FILE);
        let store = Arc::new(SqliteScheduleStore::open(&path).expect("store"));
        let now = watchdog_time("2026-09-04T20:00:00Z");
        let paused_at = watchdog_time("2026-08-30T22:06:00Z");
        let a = seed_watchdog_parent(
            store.as_ref(),
            watchdog_calendar(vec![6, 18], 45),
            4,
            SchedulePhase::Paused,
            paused_at,
            &[
                watchdog_time("2026-08-31T06:45:00Z"),
                watchdog_time("2026-08-31T18:45:00Z"),
            ],
        )
        .await;
        let b = seed_watchdog_parent(
            store.as_ref(),
            watchdog_calendar(vec![4], 0),
            5,
            SchedulePhase::Paused,
            paused_at,
            &[watchdog_time("2026-09-01T04:00:00Z")],
        )
        .await;
        let c = seed_watchdog_parent(
            store.as_ref(),
            TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc: watchdog_time("2026-08-20T06:00:00Z"),
                every_seconds: 43200,
                end_at_utc: Some(watchdog_time("2026-09-03T18:00:00Z")),
            }),
            2,
            SchedulePhase::Paused,
            watchdog_time("2026-09-01T05:21:00Z"),
            &[
                watchdog_time("2026-09-01T06:00:00Z"),
                watchdog_time("2026-09-01T18:00:00Z"),
                watchdog_time("2026-09-02T06:00:00Z"),
                watchdog_time("2026-09-02T18:00:00Z"),
                watchdog_time("2026-09-03T06:00:00Z"),
                watchdog_time("2026-09-03T18:00:00Z"),
            ],
        )
        .await;
        assert_eq!((a.revision.0, b.revision.0, c.revision.0), (4, 5, 2));
        assert_eq!(a.config.updated_at_utc, paused_at);
        assert_eq!(b.config.updated_at_utc, paused_at);
        assert_eq!(
            c.config.updated_at_utc,
            watchdog_time("2026-09-01T05:21:00Z")
        );
        let service = ScheduleService::new(store.clone());
        let before = store
            .list_occurrences(OccurrenceFilter::default())
            .await
            .expect("rows before probe");
        assert_eq!(before.len(), 9);
        assert!(before.iter().all(|row| {
            row.phase == OccurrencePhase::Pending
                && row.lease_expires_at_utc.is_none()
                && row.due_at_utc < now - chrono::Duration::seconds(120)
        }));
        let parents_before = service.list().await.expect("parents before probe");

        let (probe, log) = capture_watchdog_probe_at(&service, &path, now).await;
        assert_eq!(probe, ScheduleFiringProbe::Healthy);
        assert!(
            log.contains("active=0 paused=9 deleted=0 unresolved=0"),
            "{log}"
        );
        assert!(!log.contains("WARN") && !log.contains("ERROR"), "{log}");
        assert_eq!(
            store
                .list_occurrences(OccurrenceFilter::default())
                .await
                .expect("rows after probe"),
            before
        );
        assert_eq!(
            service.list().await.expect("parents after probe"),
            parents_before
        );
    }

    #[tokio::test]
    async fn schedule_claim_watchdog_deleted_leftover_is_not_missing_or_stalled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(SCHEDULE_STORE_FILE);
        let store = Arc::new(SqliteScheduleStore::open(&path).expect("store"));
        let now = watchdog_time("2026-09-04T20:00:00Z");
        let due = now - chrono::Duration::hours(1);
        seed_watchdog_parent(
            store.as_ref(),
            TriggerSpec::Once { due_at_utc: due },
            1,
            SchedulePhase::Deleted,
            now,
            &[due],
        )
        .await;
        let service = ScheduleService::new(store.clone());
        assert!(
            service
                .list()
                .await
                .expect("ordinary list hides deleted")
                .is_empty()
        );
        let before = store
            .list_occurrences(OccurrenceFilter::default())
            .await
            .expect("rows");
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].phase, OccurrencePhase::Pending);
        let (probe, log) = capture_watchdog_probe_at(&service, &path, now).await;
        assert_eq!(probe, ScheduleFiringProbe::Healthy);
        assert!(
            log.contains("active=0 paused=0 deleted=1 unresolved=0"),
            "{log}"
        );
        assert!(!log.contains("WARN") && !log.contains("ERROR"), "{log}");
        assert_eq!(
            store
                .list_occurrences(OccurrenceFilter::default())
                .await
                .expect("rows"),
            before
        );
    }

    #[tokio::test]
    async fn schedule_claim_watchdog_mixed_parents_count_and_oldest_only_active() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(SCHEDULE_STORE_FILE);
        let store = Arc::new(SqliteScheduleStore::open(&path).expect("store"));
        let now = watchdog_time("2026-09-04T20:00:00Z");
        for phase in [SchedulePhase::Paused, SchedulePhase::Deleted] {
            let due = now - chrono::Duration::days(2);
            seed_watchdog_parent(
                store.as_ref(),
                TriggerSpec::Once { due_at_utc: due },
                1,
                phase,
                now,
                &[due],
            )
            .await;
        }
        let oldest_active = now - chrono::Duration::hours(1);
        seed_watchdog_parent(
            store.as_ref(),
            TriggerSpec::Once {
                due_at_utc: oldest_active,
            },
            1,
            SchedulePhase::Active,
            now,
            &[
                now - chrono::Duration::minutes(5),
                oldest_active,
                now - chrono::Duration::seconds(120),
                now + chrono::Duration::hours(1),
            ],
        )
        .await;
        let service = ScheduleService::new(store.clone());
        // Both the held and vacant observations must leave attribution unchanged.
        let lease = acquire_test_executor_lease(store.as_ref(), "watchdog-active-control").await;
        let (probe, log) = capture_watchdog_probe_at(&service, &path, now).await;
        store
            .release_executor_lease(lease)
            .await
            .expect("release test lease");
        let ScheduleFiringProbe::Stalled { report } = probe else {
            panic!("active overdue work must stall");
        };
        assert!(
            report.starts_with("2 active-parent pending occurrence(s)"),
            "{report}"
        );
        assert!(
            report.contains(&format!("oldest due {}", oldest_active.to_rfc3339())),
            "{report}"
        );
        assert!(
            report.contains("active=2, paused=1, deleted=1, unresolved=0"),
            "{report}"
        );
        assert!(report.contains("firing authority IS held"), "{report}");
        assert!(
            log.lines()
                .any(|line| line.contains("WARN") && line.contains("not delivering at boot")),
            "{log}"
        );
        assert!(
            log.lines()
                .any(|line| line.contains("ERROR") && line.contains("pipeline is stalled")),
            "{log}"
        );
        let (vacant_probe, _) = capture_watchdog_probe_at(&service, &path, now).await;
        assert!(
            matches!(vacant_probe, ScheduleFiringProbe::Stalled { report }
            if report.contains("firing authority is vacant")
                && report.starts_with("2 active-parent pending occurrence(s)"))
        );
    }

    #[tokio::test]
    async fn schedule_claim_watchdog_missing_parent_is_unresolved_even_without_active_work() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(SCHEDULE_STORE_FILE);
        let store = Arc::new(SqliteScheduleStore::open(&path).expect("store"));
        let now = watchdog_time("2026-09-04T20:00:00Z");
        let due = now - chrono::Duration::hours(2);
        let parent = seed_watchdog_parent(
            store.as_ref(),
            TriggerSpec::Once { due_at_utc: due },
            1,
            SchedulePhase::Active,
            now,
            &[due],
        )
        .await;
        // Simulate corrupt/orphaned evidence that normal foreign-key-enforced
        // writes cannot create. Disable constraints only on this fault injector.
        {
            let conn = rusqlite::Connection::open(&path).expect("fault connection");
            conn.pragma_update(None, "foreign_keys", false)
                .expect("allow orphan fixture");
            assert_eq!(
                conn.execute(
                    "DELETE FROM schedule_schedules WHERE schedule_id = ?1",
                    [parent.schedule_id.to_string()],
                )
                .expect("remove parent"),
                1
            );
        }
        let service = ScheduleService::new(store.clone());
        let (probe, log) = capture_watchdog_probe_at(&service, &path, now).await;
        let ScheduleFiringProbe::Incomplete { report } = probe else {
            panic!("absent parent cannot prove health or a stall");
        };
        assert!(
            report.contains("active=0, paused=0, deleted=0, unresolved=1"),
            "{report}"
        );
        assert!(report.contains(&parent.schedule_id.to_string()), "{report}");
        assert!(!report.contains("firing authority"), "{report}");
        assert!(
            log.contains("ERROR") && log.contains("observation incomplete at boot"),
            "{log}"
        );
        assert!(
            !log.contains("WARN")
                && !log.contains("pipeline is stalled")
                && !log.contains("healthy at boot"),
            "{log}"
        );

        let active_due = now - chrono::Duration::minutes(10);
        seed_watchdog_parent(
            store.as_ref(),
            TriggerSpec::Once {
                due_at_utc: active_due,
            },
            1,
            SchedulePhase::Active,
            now,
            &[active_due],
        )
        .await;
        let (mixed, _) = capture_watchdog_probe_at(&service, &path, now).await;
        let ScheduleFiringProbe::Stalled { report } = mixed else {
            panic!("unresolved evidence must not hide known active overdue work");
        };
        assert!(
            report.contains("active=1, paused=0, deleted=0, unresolved=1"),
            "{report}"
        );
        assert!(
            report.contains(&format!("oldest due {}", active_due.to_rfc3339())),
            "{report}"
        );
        assert!(report.contains(&parent.schedule_id.to_string()), "{report}");
    }

    #[tokio::test]
    async fn schedule_claim_watchdog_poison_remains_incomplete_with_no_active_rows() {
        for (phase, corrupt_parent) in [
            (SchedulePhase::Paused, true),
            (SchedulePhase::Deleted, true),
            (SchedulePhase::Paused, false),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join(SCHEDULE_STORE_FILE);
            let store = Arc::new(SqliteScheduleStore::open(&path).expect("store"));
            let now = watchdog_time("2026-09-04T20:00:00Z");
            let due = now - chrono::Duration::hours(1);
            seed_watchdog_parent(
                store.as_ref(),
                TriggerSpec::Once { due_at_utc: due },
                1,
                phase,
                now,
                &[due],
            )
            .await;
            let (table, column) = if corrupt_parent {
                ("schedule_schedules", "schedule_json")
            } else {
                ("schedule_occurrences", "occurrence_json")
            };
            corrupt_first_row(&path, table, column);
            let service = ScheduleService::new(store);
            let (probe, log) = capture_watchdog_probe_at(&service, &path, now).await;
            let ScheduleFiringProbe::Incomplete { report } = probe else {
                panic!("poison must remain visible with no active rows");
            };
            assert!(
                report.contains(table) && report.contains("attribution unresolved"),
                "{report}"
            );
            assert!(
                log.contains("ERROR") && log.contains("observation incomplete"),
                "{log}"
            );
            assert!(
                !log.contains("WARN")
                    && !log.contains("pipeline is stalled")
                    && !log.contains("healthy at boot"),
                "{log}"
            );
        }
    }

    #[tokio::test]
    async fn schedule_claim_watchdog_unreadable_parent_list_is_not_healthy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(SCHEDULE_STORE_FILE);
        let store = Arc::new(SqliteScheduleStore::open(&path).expect("store"));
        // A query failure is not a successful empty parent list.
        rusqlite::Connection::open(&path)
            .expect("fault connection")
            .execute("DROP TABLE schedule_schedules", [])
            .expect("make schedule listing unreadable");
        let (probe, log) = capture_watchdog_probe_at(
            &ScheduleService::new(store),
            &path,
            watchdog_time("2026-09-04T20:00:00Z"),
        )
        .await;
        let ScheduleFiringProbe::Incomplete { report } = probe else {
            panic!("unreadable parents cannot prove health");
        };
        assert!(report.contains("schedule list is failing"), "{report}");
        assert!(report.contains("no such table"), "{report}");
        assert!(report.contains("attribution unresolved"), "{report}");
        assert!(log.contains("observation incomplete at boot"), "{log}");
        assert!(!log.contains("healthy at boot"), "{log}");
    }

    #[tokio::test]
    async fn schedule_claim_watchdog_unreadable_clock_is_incomplete() {
        let service = ScheduleService::new(Arc::new(meerkat_schedule::DisabledScheduleStore));
        let probe = probe_schedule_firing_pipeline(
            &service,
            Path::new("unused-diagnostic-path"),
            Duration::from_mins(2),
        )
        .await;
        assert!(matches!(probe, ScheduleFiringProbe::Incomplete { report }
            if report.contains("schedule store clock read failed")));
    }

    #[test]
    fn schedule_claim_watchdog_dedup_heartbeat_and_incomplete_recovery() {
        let capture = WatchdogLogCapture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_ansi(false)
            .without_time()
            .finish();
        // A second registered Dispatch keeps callsite interest from being
        // cached `never` by threads without a subscriber.
        let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
        tracing::subscriber::with_default(subscriber, || {
            let config = ScheduleClaimWatchdogConfig::default();
            let mut state = ScheduleClaimWatchdogState::default();
            let stalled = ScheduleFiringProbe::Stalled {
                report: "active overdue control".to_string(),
            };
            for _ in 0..10 {
                state.observe(stalled.clone(), config);
            }
            let incomplete = ScheduleFiringProbe::Incomplete {
                report: "parent evidence unavailable".to_string(),
            };
            state.observe(incomplete.clone(), config);
            state.observe(incomplete, config);
            state.observe(ScheduleFiringProbe::Healthy, config);
        });
        let log =
            String::from_utf8(capture.0.lock().expect("log capture").clone()).expect("UTF-8 logs");
        assert_eq!(log.matches("pipeline is stalled").count(), 1, "{log}");
        assert_eq!(log.matches("pipeline is still stalled").count(), 1, "{log}");
        assert_eq!(
            log.matches("pipeline observation incomplete").count(),
            1,
            "{log}"
        );
        assert_eq!(
            log.matches("pipeline observation recovered").count(),
            1,
            "{log}"
        );
        assert!(!log.contains("pipeline recovered"), "{log}");
    }

    /// Healthy pipeline: future work only, nothing overdue → the watchdog has
    /// nothing to say.
    #[tokio::test]
    async fn schedule_claim_watchdog_probe_is_healthy_on_future_work() {
        let dir = tempfile::tempdir().expect("tempdir");
        let start = chrono::Utc::now() + chrono::Duration::hours(1);
        let (service, store_path) = seed_sqlite_schedule(dir.path(), start).await;

        let probe =
            probe_schedule_firing_pipeline(&service, &store_path, Duration::from_secs(0)).await;
        assert_eq!(probe, ScheduleFiringProbe::Healthy);
    }

    /// Downstream app Observation A: due occurrences sit pending with no lease and
    /// the driver says nothing. The probe must call that out loudly.
    ///
    /// 0.8.22: the WHY half of that report is no longer inferred. This store
    /// has no firing host, so nothing ever acquires the store-issued executor
    /// lease and the probe's direct `observe_executor_lease` read is
    /// deterministically vacant. Pinned here because the vacancy clause is the
    /// difference between "nothing is draining this store" and "a live host
    /// cannot claim" - two diagnoses the old single sentence conflated.
    #[tokio::test]
    async fn schedule_claim_watchdog_probe_flags_overdue_unclaimed_occurrences() {
        let dir = tempfile::tempdir().expect("tempdir");
        let start = chrono::Utc::now() + chrono::Duration::hours(1);
        let (service, store_path) = seed_sqlite_schedule(dir.path(), start).await;

        // Age the first planned occurrence 10 minutes into the past.
        let overdue = chrono::Utc::now() - chrono::Duration::minutes(10);
        age_first_occurrence(&store_path, overdue);

        // The store's own answer, read directly rather than inferred.
        assert_eq!(
            observe_schedule_firing_authority(&service).await,
            ScheduleFiringAuthority::Vacant,
            "a store with no firing host must report vacant firing authority"
        );

        let probe =
            probe_schedule_firing_pipeline(&service, &store_path, Duration::from_mins(1)).await;
        let ScheduleFiringProbe::Stalled { report } = probe else {
            panic!("an overdue unclaimed occurrence must probe as Stalled");
        };
        assert!(
            report.contains("never claimed"),
            "report must state the claim stall: {report}"
        );
        assert!(
            report.contains("firing authority is vacant"),
            "report must name the store's own firing-authority verdict, not infer one: {report}"
        );
    }

    /// Downstream app Observation B: one poisoned schedule row (e.g. a Deleted
    /// tombstone the recovery invariant rejects) fails the whole
    /// all-or-nothing list. The 0.8.11 driver skips it per-row, but that
    /// schedule never fires and list-backed surfaces are blind. The probe
    /// must surface the list failure and name the poisoned row.
    #[tokio::test]
    async fn schedule_claim_watchdog_probe_names_poisoned_schedule_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let start = chrono::Utc::now() + chrono::Duration::hours(1);
        let (service, store_path) = seed_sqlite_schedule(dir.path(), start).await;
        corrupt_first_row(&store_path, "schedule_schedules", "schedule_json");

        let probe =
            probe_schedule_firing_pipeline(&service, &store_path, Duration::from_mins(1)).await;
        let ScheduleFiringProbe::Incomplete { report } = probe else {
            panic!("a poisoned schedule row must probe as Incomplete");
        };
        assert!(
            report.contains("schedule list is failing"),
            "report must name the failing surface: {report}"
        );
        assert!(
            report.contains("schedule_schedules.schedule_id="),
            "report must name the poisoned row: {report}"
        );
    }

    /// A poisoned DUE occurrence row no longer starves its neighbors — the
    /// meerkat 0.8.11 claim scan skips it as a typed per-row fault
    /// (`schedule_claim_tolerates_poisoned_neighbor_row` proves that) — but
    /// the row ITSELF can never be claimed and will never fire. The probe's
    /// own read of the overdue window still fails wholesale on it, and the
    /// report must surface that failure and name the row.
    ///
    /// The row must be inside the probe's overdue window: the store's SQL
    /// prefilter (indexed `phase` + `due_at_ms`) never deserializes future
    /// rows, so a poisoned future occurrence is invisible until it comes due.
    #[tokio::test]
    async fn schedule_claim_watchdog_probe_names_poisoned_occurrence_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let start = chrono::Utc::now() + chrono::Duration::hours(1);
        let (service, store_path) = seed_sqlite_schedule(dir.path(), start).await;
        let overdue = chrono::Utc::now() - chrono::Duration::minutes(10);
        age_first_occurrence(&store_path, overdue);
        corrupt_first_row(&store_path, "schedule_occurrences", "occurrence_json");

        let probe =
            probe_schedule_firing_pipeline(&service, &store_path, Duration::from_mins(1)).await;
        let ScheduleFiringProbe::Incomplete { report } = probe else {
            panic!("a poisoned occurrence row must probe as Incomplete");
        };
        assert!(
            report.contains("occurrence scan is failing"),
            "report must name the failing surface: {report}"
        );
        assert!(
            report.contains("schedule_occurrences.occurrence_id="),
            "report must name the poisoned row: {report}"
        );
    }

    #[tokio::test]
    async fn ensure_steward_dream_schedule_is_idempotent_and_updates_cadence() {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let now: chrono::DateTime<chrono::Utc> = "2026-07-01T00:00:00Z".parse().expect("fixed now");

        // First call creates exactly one host-runnable schedule.
        ensure_steward_dream_schedule(&service, Duration::from_hours(6), now)
            .await
            .expect("create dream schedule");
        let schedules = service.list().await.expect("list");
        assert_eq!(
            dream_target_count(&schedules),
            1,
            "one dream schedule created"
        );
        assert_eq!(dream_interval_seconds(&schedules[0]), 6 * 3600);

        // Re-running with the same cadence is a no-op — no duplicate stacking
        // across boots.
        ensure_steward_dream_schedule(&service, Duration::from_hours(6), now)
            .await
            .expect("idempotent re-ensure");
        let schedules = service.list().await.expect("list");
        assert_eq!(
            dream_target_count(&schedules),
            1,
            "still one dream schedule"
        );

        // A changed cadence updates the existing schedule in place, not a new
        // one.
        ensure_steward_dream_schedule(&service, Duration::from_mins(30), now)
            .await
            .expect("update cadence");
        let schedules = service.list().await.expect("list");
        assert_eq!(
            dream_target_count(&schedules),
            1,
            "cadence change reuses schedule"
        );
        assert_eq!(dream_interval_seconds(&schedules[0]), 30 * 60);
    }

    #[tokio::test]
    async fn repair_rewrites_persisted_resumable_session_prompt_targets() {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let request = serde_json::from_value(json!({
            "name": "legacy-digest",
            "trigger": {
                "type": "interval",
                "start_at_utc": "2026-07-01T05:00:00Z",
                "every_seconds": 86400
            },
            "target": {
                "target_kind": "session",
                "type": "resumable_session",
                "session_id": "019ee0a7-a594-7670-b530-97e7c9e263b7",
                "action": {
                    "type": "prompt",
                    "prompt": "Send the morning digest.",
                    "render_metadata": {
                        "class": "external_event",
                        "salience": "important"
                    }
                }
            },
            "planning_horizon_occurrences": 1
        }))
        .expect("create request");
        service
            .create(request)
            .await
            .expect("create legacy schedule");

        let repaired =
            repair_resumable_session_targets_with_resolver(&service, |session_id| async move {
                assert_eq!(session_id, "019ee0a7-a594-7670-b530-97e7c9e263b7");
                Some(("example".to_string(), "domain:security".to_string()))
            })
            .await
            .expect("repair");

        assert_eq!(repaired, 1);
        let schedules = service.list().await.expect("list schedules");
        assert_eq!(schedules.len(), 1);
        assert_mob_member_target(
            &schedules[0].target,
            "example",
            "domain:security",
            "Send the morning digest.",
            "external_event",
        );
    }

    /// A mob state with no mobs, over an ephemeral session service, and the
    /// directory its agent factory uses.
    fn mob_state_without_mobs() -> (tempfile::TempDir, Arc<MobMcpState>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let factory = AgentFactory::new(dir.path());
        let builder = FactoryAgentBuilder::new(factory, Config::default());
        let service = Arc::new(meerkat_session::EphemeralSessionService::new(builder, 4));
        let state =
            MobMcpState::new(service, meerkat_mob::MobControlPrincipal::Owner).expect("mob state");
        (dir, Arc::new(state))
    }

    /// Schedule tools attached over an in-memory store, and the dispatcher
    /// they install in the builder's schedule-tools slot.
    fn schedule_tools_over_a_memory_store() -> (
        tempfile::TempDir,
        AttachedScheduleTools,
        Arc<dyn meerkat_core::AgentToolDispatcher>,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let builder = FactoryAgentBuilder::new(AgentFactory::new(dir.path()), Config::default());
        let attached =
            attach_schedule_tools_with_store(&builder, Arc::new(MemoryScheduleStore::default()));
        let installed = builder
            .default_schedule_tools
            .read()
            .expect("schedule tools slot")
            .clone()
            .expect("installed schedule tools");
        (dir, attached, installed)
    }

    /// Waits up to five seconds for `state` to be dropped.
    async fn dropped_within_bound(state: &std::sync::Weak<MobMcpState>) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.upgrade().is_some() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        state.upgrade().is_none()
    }

    /// Runs one schedule tool call through `tools`.
    async fn schedule_call(
        tools: &Arc<dyn meerkat_core::AgentToolDispatcher>,
        name: &str,
        args: serde_json::Value,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let args = RawValue::from_string(args.to_string()).expect("tool arguments");
        tools
            .dispatch(ToolCallView {
                id: "registry-call",
                name,
                args: &args,
            })
            .await
    }

    /// Create arguments with a resumable-session prompt target: a candidate
    /// for the rewrite to a mob member.
    fn resumable_session_create_args(name: &str) -> serde_json::Value {
        json!({
            "name": name,
            "trigger": {
                "type": "interval",
                "start_at_utc": "2026-07-01T05:00:00Z",
                "every_seconds": 3600
            },
            "target": {
                "target_kind": "session",
                "type": "resumable_session",
                "session_id": "019ee0a7-a594-7670-b530-97e7c9e263b7",
                "action": { "type": "prompt", "prompt": "Check in." }
            }
        })
    }

    /// The installed schedule tools keep no mob state alive: once its owner
    /// drops the adopted state, it is gone. The registry held it, so a
    /// stopped host's mob state lived on in every session service's schedule
    /// tools.
    #[tokio::test]
    async fn installed_schedule_tools_keep_no_mob_state_alive() {
        let (_state_dir, state) = mob_state_without_mobs();
        let (_dir, attached, _installed) = schedule_tools_over_a_memory_store();
        attached
            .mob_target_registry
            .set_mob_state(Some(Arc::clone(&state)));
        let adopted = Arc::downgrade(&state);
        drop(state);
        assert!(
            dropped_within_bound(&adopted).await,
            "the schedule tools kept the dropped mob state alive"
        );
    }

    /// A create whose resumable-session target the registry would rewrite to
    /// a mob member is refused as temporarily unavailable once the adopted
    /// mob state is gone, as Core refuses a stopped driver; schedule calls
    /// that are no rewrite candidates still run.
    #[tokio::test]
    async fn a_rewrite_candidate_after_the_mob_state_is_gone_is_unavailable() {
        let (_state_dir, state) = mob_state_without_mobs();
        let (_dir, attached, installed) = schedule_tools_over_a_memory_store();
        attached
            .mob_target_registry
            .set_mob_state(Some(Arc::clone(&state)));
        let adopted = Arc::downgrade(&state);
        drop(state);
        let gone = dropped_within_bound(&adopted).await;

        let refused = schedule_call(
            &installed,
            "meerkat_schedule_create",
            resumable_session_create_args("after-the-host-stopped"),
        )
        .await
        .err();
        assert!(
            matches!(
                &refused,
                Some(meerkat_core::ToolError::Unavailable {
                    reason: meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
                    ..
                })
            ),
            "{refused:?}"
        );
        assert!(gone, "the mob state is gone");
        assert!(attached.service.list().await.expect("list").is_empty());

        let current_session = json!({
            "name": "a-session-target",
            "trigger": {
                "type": "interval",
                "start_at_utc": "2026-07-01T05:00:00Z",
                "every_seconds": 3600
            },
            "target": {
                "target_kind": "session",
                "type": "resumable_session",
                "session_id": "019ee0a7-a594-7670-b530-97e7c9e263b7",
                "action": {
                    "type": "prompt",
                    "prompt": "Pinned to the session.",
                    "system_prompt": "Not a member prompt."
                }
            }
        });
        schedule_call(&installed, "meerkat_schedule_create", current_session)
            .await
            .expect("a target that is no rewrite candidate is created");
        let schedules = attached.service.list().await.expect("list");
        assert_eq!(schedules.len(), 1);
        let schedule_id = schedules[0].schedule_id.to_string();
        for (name, args) in [
            ("meerkat_schedule_list", json!({})),
            (
                "meerkat_schedule_get",
                json!({ "schedule_id": schedule_id }),
            ),
            (
                "meerkat_schedule_pause",
                json!({ "schedule_id": schedule_id }),
            ),
        ] {
            schedule_call(&installed, name, args)
                .await
                .unwrap_or_else(|error| panic!("{name} still answers: {error:?}"));
        }
    }

    /// A registry whose lock a writer poisoned by panicking no longer knows
    /// which mob state it adopted, so a rewrite candidate is refused as
    /// temporarily unavailable, as for a stopped host, not let through
    /// unrewritten.
    #[tokio::test]
    async fn a_poisoned_registry_refuses_a_rewrite_candidate() {
        let (_dir, attached, installed) = schedule_tools_over_a_memory_store();
        let lock = Arc::clone(&attached.mob_target_registry.state);
        let poisoner = std::thread::spawn(move || {
            let _held = lock.write().expect("the registry lock");
            panic!("a writer panics while it holds the registry lock");
        });
        assert!(poisoner.join().is_err(), "the writer panicked");
        assert!(attached.mob_target_registry.state.is_poisoned());

        let refused = schedule_call(
            &installed,
            "meerkat_schedule_create",
            resumable_session_create_args("after-a-writer-panicked"),
        )
        .await
        .err();
        assert!(
            matches!(
                &refused,
                Some(meerkat_core::ToolError::Unavailable {
                    reason: meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
                    ..
                })
            ),
            "{refused:?}"
        );
        assert!(attached.service.list().await.expect("list").is_empty());
    }

    /// A registry that never adopted a mob state passes a rewrite candidate
    /// through unchanged.
    #[tokio::test]
    async fn a_registry_without_a_mob_state_passes_the_target_through() {
        let (_dir, attached, installed) = schedule_tools_over_a_memory_store();
        schedule_call(
            &installed,
            "meerkat_schedule_create",
            resumable_session_create_args("no-mob-runtime"),
        )
        .await
        .expect("created");
        let schedules = attached.service.list().await.expect("list");
        assert!(
            matches!(
                schedules.first().map(|schedule| &schedule.target),
                Some(TargetBinding::Session(_))
            ),
            "{schedules:?}"
        );
    }

    /// A torn-down runtime leaves its mob state to no schedule tools: once
    /// the runtime that owns it shuts down and is dropped, the mob state its
    /// schedule tools' registry adopted is gone. The registry kept it alive
    /// for as long as anything held the registry.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_torn_down_runtime_leaves_its_mob_state_to_no_schedule_tools() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let state = temp_dir.path().join("state");
        std::fs::create_dir_all(&state).expect("state dir");
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
            meerkat_store::SqliteSessionStore::open(state.join("sessions.db"))
                .expect("session store"),
        );
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))
                .expect("runtime store"),
        );
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let factory = AgentFactory::new(&state).comms(true);
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
            session_store.clone(),
        )));
        builder.default_blob_store = Some(blob_store.clone());
        let jobs: Arc<dyn meerkat::DetachedJobStore> =
            Arc::new(meerkat::MemoryDetachedJobStore::new());
        builder.default_detached_job_store = Some(Arc::clone(&jobs));
        let delivery =
            crate::mob_handle_runtime::MobRuntimeDelivery::new(Arc::clone(&runtime_store), jobs);
        let attached =
            attach_schedule_tools_with_store(&builder, Arc::new(MemoryScheduleStore::default()));
        let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        let adapter = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(
                Arc::clone(&runtime_store),
                Arc::clone(&blob_store),
            )
            .expect("runtime machine"),
        );
        let session_service = Arc::new(PersistentSessionService::new(
            builder,
            16,
            session_store,
            runtime_store,
            blob_store,
        ));
        let definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"registry-teardown\"\n\n[profiles.general]\nmodel = \"gpt-5.5\"\n",
        )
        .expect("definition");
        let mob_spec = crate::mob_handle_runtime::MobBootstrapSpec::new(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            session_service,
        )
        .with_session_runtime_adapter(adapter)
        .expect("the session runtime owner")
        .with_runtime_delivery(delivery)
        .with_agent_mob_tools(mob_tools_slot)
        .expect("install the agent mob tools")
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::for_provider(
                meerkat_core::Provider::OpenAI,
            ))),
        });
        let runtime = crate::UnifiedRuntime::bootstrap(
            mob_spec,
            crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: "registry-teardown".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            Duration::from_secs(2),
        )
        .await
        .expect("bootstrap");
        let adopted = crate::gateway_composition::adopt_schedule_mob_targets(
            &runtime,
            &attached.service,
            &attached.mob_target_registry,
        )
        .await
        .expect("the runtime has a mob state");
        let adopted_state = Arc::downgrade(&adopted);
        drop(adopted);
        runtime.shutdown().await;
        drop(runtime);
        assert!(
            dropped_within_bound(&adopted_state).await,
            "the schedule tools kept the torn-down runtime's mob state alive"
        );
    }

    /// What one scheduled fire to a mob member did, observed from the
    /// schedule host, the member's event stream and the provider.
    #[derive(Debug)]
    struct ScheduledFireOutcome {
        /// The receipt detail of a dispatch, or the schedule domain error.
        dispatch: Result<Option<String>, String>,
        terminal_phase: Option<OccurrencePhase>,
        terminal_reason: Option<meerkat::DeliveryFailureReason>,
        terminal_detail: Option<String>,
        /// The member's agent events after the fire, by type.
        member_events: Vec<String>,
        /// The typed failure of a run the fire started, if one failed.
        run_failure: Option<meerkat_core::event::AgentErrorReport>,
        /// Provider calls the fire caused.
        llm_calls: usize,
        /// Native ingress checks the governed composition was asked.
        ingress_checks: usize,
        member_session_before: Option<SessionId>,
        member_session_after: Option<SessionId>,
    }

    /// A mob with one `worker` profile, the member every scheduled fire
    /// below targets.
    fn scheduled_worker_definition(mob_id: &str) -> meerkat_mob::MobDefinition {
        meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
             runtime_mode = \"turn_driven\"\n\n[profiles.worker.tools]\ncomms = true\n"
        ))
        .expect("mob definition")
    }

    /// Agent config whose global realm binds OpenAI to the fixture key, so
    /// sessions resolve their provider identity without the environment.
    fn scheduled_fire_config() -> Config {
        let mut config = Config::default();
        config.realm.insert(
            meerkat_core::connection::GLOBAL_REALM_SLUG.to_string(),
            meerkat_core::RealmConfigSection::from_inline_api_keys(&[(
                "openai",
                "sk-regression-test",
            )]),
        );
        config
    }

    /// The held-identity fixture's native grant owner with a native ingress
    /// check that admits and counts, so a refusal is not that check's.
    fn counting_governed_configuration(
        checks: Arc<std::sync::atomic::AtomicUsize>,
    ) -> meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration {
        let mut configuration =
            crate::unified_runtime::held_identity_test_support::host_configuration();
        configuration.ingress = Arc::new(
            move |_, _, _, _| -> Result<(), meerkat_runtime::input_authority::NativeAdmissionError> {
                checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        );
        configuration
    }

    /// The held-identity fixture's host tool policy.
    fn host_policy_registry() -> Arc<meerkat_core::ToolConsequencePolicyRegistry> {
        Arc::new(
            meerkat_core::ToolConsequencePolicyRegistry::new(
                vec![Arc::new(
                    crate::unified_runtime::held_identity_test_support::HostPolicy(
                        meerkat_core::PolicyProviderId::new("host").expect("provider id"),
                    ),
                )],
                meerkat_core::PolicyEvaluationSupervisorConfig::default(),
                None,
            )
            .expect("host policy registry"),
        )
    }

    /// Fire one due schedule whose target is a mob member through
    /// [`InternalDeliveryScheduleMobHost`], on a runtime built with or
    /// without `native_grant_work_configuration`. The governed composition
    /// is the held-identity fixture's (its grant owner, a destination hook,
    /// a host tool policy), with a native ingress check that admits and
    /// counts, so a refusal is not that check's.
    async fn scheduled_member_fire(governed: bool) -> ScheduledFireOutcome {
        use futures::StreamExt;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let llm_calls = Arc::new(AtomicUsize::new(0));
        let ingress_checks = Arc::new(AtomicUsize::new(0));
        let mob_id = format!(
            "schedule-governed-{}-{}",
            if governed { "on" } else { "off" },
            uuid::Uuid::new_v4().simple()
        );
        let mut builder = crate::UnifiedRuntime::builder()
            .definition(scheduled_worker_definition(&mob_id))
            .meerkat_config(scheduled_fire_config())
            .persistent_state(temp_dir.path().join("state"))
            .default_llm_client(Arc::new(CountingLlmClient {
                calls: llm_calls.clone(),
            }));
        if governed {
            builder = builder
                .native_grant_work_configuration(counting_governed_configuration(
                    ingress_checks.clone(),
                ))
                .member_runtime_registration_hook(Arc::new(
                    crate::unified_runtime::held_identity_test_support::SelectingHook,
                ))
                .tool_consequence_policy_registry(host_policy_registry());
        }
        let runtime = builder.build().await.expect("build runtime");
        let handle = runtime.mob_handle();
        let member = meerkat_mob::ids::AgentIdentity::from("scheduled-worker");
        runtime
            .spawn_many(vec![meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("worker"),
                member.clone(),
            )])
            .await
            .expect("spawn the member");
        let member_session_before = handle.resolve_bridge_session_id(&member).await;
        let mut probe = handle
            .subscribe_agent_events(&member)
            .await
            .expect("probe the member's events");
        let llm_baseline = llm_calls.load(Ordering::SeqCst);
        let ingress_baseline = ingress_checks.load(Ordering::SeqCst);

        let member_id = crate::member_comms_id::mob_member_id("scheduled-worker").to_string();
        let content = meerkat_core::ContentInput::Text("run the scheduled check".to_string());
        let binding = MobTargetBinding::Member {
            mob_id: mob_id.clone(),
            member_id,
            action: ScheduledMobAction::Send {
                content,
                render_metadata: None,
            },
        };
        let schedule = Schedule::new(CreateScheduleRequest {
            name: Some("governed-member-fire".to_string()),
            description: None,
            trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc: chrono::Utc::now(),
                every_seconds: 60,
                end_at_utc: None,
            }),
            target: TargetBinding::Mob(Box::new(binding.clone())),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("schedule");
        let occurrence = Occurrence::planned_from_schedule(
            &schedule,
            meerkat::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("occurrence");
        let host = InternalDeliveryScheduleMobHost {
            inner: Arc::new(NoopScheduleMobHost::new("unused fallback")),
            handle: handle.clone(),
            identity_runtime: None,
        };

        let mut outcome = ScheduledFireOutcome {
            dispatch: Ok(None),
            terminal_phase: None,
            terminal_reason: None,
            terminal_detail: None,
            member_events: Vec::new(),
            run_failure: None,
            llm_calls: 0,
            ingress_checks: 0,
            member_session_before,
            member_session_after: None,
        };
        match host
            .deliver_mob_target(
                &occurrence,
                &meerkat::ScheduleDeliveryIdentity::for_occurrence(&occurrence),
                &binding,
            )
            .await
        {
            Ok(dispatch) => {
                outcome.dispatch = Ok(dispatch.receipt.detail.clone());
                let terminal = dispatch.completion.await.expect("delivery terminal");
                outcome.terminal_phase = Some(terminal.phase);
                outcome.terminal_reason = terminal.delivery_failure_reason;
                outcome.terminal_detail = terminal.detail;
            }
            Err(error) => outcome.dispatch = Err(error.to_string()),
        }
        // Whatever the schedule recorded, follow the member until a run it
        // started ends, or a bounded window shows none started.
        let _ = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(event) = probe.next().await {
                outcome
                    .member_events
                    .push(meerkat_core::event::agent_event_type(&event.payload).to_string());
                match event.payload {
                    meerkat_core::event::AgentEvent::RunCompleted { .. } => break,
                    meerkat_core::event::AgentEvent::RunFailed { error_report, .. } => {
                        outcome.run_failure = Some(error_report);
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await;
        outcome.llm_calls = llm_calls.load(Ordering::SeqCst) - llm_baseline;
        outcome.ingress_checks = ingress_checks.load(Ordering::SeqCst) - ingress_baseline;
        outcome.member_session_after = handle.resolve_bridge_session_id(&member).await;
        drop(probe);
        runtime.shutdown().await;
        outcome
    }

    /// A runtime built with `native_grant_work_configuration` does not run a
    /// scheduled fire to a mob member. The schedule host submits the fire as
    /// plain internal work, which carries no input authority association,
    /// and the member's session refuses it at native admission before the
    /// host's ingress check is asked: the occurrence ends `DeliveryFailed`
    /// (`RuntimeRejected`) at the fire, no run starts, no provider call is
    /// made and the member's session is unchanged. The same fire on a
    /// runtime without that configuration runs the member's turn and
    /// completes.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_governed_members_scheduled_fire_does_not_run_and_an_ungoverned_one_does() {
        let ungoverned = scheduled_member_fire(false).await;
        assert!(
            matches!(&ungoverned.dispatch, Ok(Some(detail)) if detail.starts_with("work_ref=")),
            "{ungoverned:#?}"
        );
        assert_eq!(ungoverned.terminal_phase, Some(OccurrencePhase::Completed));
        assert_eq!(ungoverned.terminal_reason, None);
        assert_eq!(ungoverned.terminal_detail, None);
        assert_eq!(
            ungoverned.member_events.first().map(String::as_str),
            Some("run_started"),
            "{ungoverned:#?}"
        );
        assert_eq!(
            ungoverned.member_events.last().map(String::as_str),
            Some("run_completed"),
            "{ungoverned:#?}"
        );
        assert!(ungoverned.run_failure.is_none(), "{ungoverned:#?}");
        assert_eq!(ungoverned.llm_calls, 1, "the ungoverned fire runs one turn");
        assert_eq!(ungoverned.ingress_checks, 0);
        assert!(ungoverned.member_session_before.is_some());
        assert_eq!(
            ungoverned.member_session_after, ungoverned.member_session_before,
            "the fire runs in the member's own session"
        );

        let governed = scheduled_member_fire(true).await;
        let session = governed
            .member_session_before
            .clone()
            .expect("the governed member has a session");
        assert!(
            matches!(governed.dispatch, Ok(None)),
            "no work receipt: {governed:#?}"
        );
        assert_eq!(
            governed.terminal_phase,
            Some(OccurrencePhase::DeliveryFailed)
        );
        assert_eq!(
            governed.terminal_reason,
            Some(meerkat::DeliveryFailureReason::RuntimeRejected)
        );
        assert_eq!(
            governed.terminal_detail.as_deref(),
            Some(
                format!(
                    "internal schedule delivery failed (member scheduled-worker): member \
                     session {session} did not admit the input: Input validation failed: \
                     native work authority association is unavailable or mismatched"
                )
                .as_str()
            )
        );
        assert!(
            governed.member_events.is_empty(),
            "no run starts: {governed:#?}"
        );
        assert!(governed.run_failure.is_none());
        assert_eq!(governed.llm_calls, 0, "the governed fire runs no turn");
        assert_eq!(
            governed.ingress_checks, 0,
            "refused before the host's ingress check is asked"
        );
        assert_eq!(
            governed.member_session_after,
            Some(session),
            "the member's session is unchanged"
        );
    }

    /// The kinds of schedule target through which a fire reaches a mob
    /// member, or a session beside it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ScheduledTargetKind {
        /// A mob target naming the member.
        MobMember,
        /// The member's `mob_member:` identity, the target a member's own
        /// current-session schedule carries.
        MemberIdentity,
        /// A session target naming the member's session.
        MemberSession,
        /// A session target that materializes a session at the fire.
        MaterializedSession,
    }

    /// How long a fire's occurrence is followed to a terminal phase: well
    /// under the schedule host's 60 second claim lease, so an occurrence
    /// still in flight at the end was never reclaimed.
    const HOSTED_FIRE_TERMINAL_WAIT: Duration = Duration::from_secs(15);

    /// How long the member is watched for a run a fire to it started.
    const HOSTED_FIRE_RUN_WINDOW: Duration = Duration::from_secs(5);

    /// What one fire through the schedule host did, observed from the
    /// schedule store, the session service, the member's event stream and
    /// the provider.
    #[derive(Debug)]
    struct HostedFireOutcome {
        schedule_id: meerkat::ScheduleId,
        occurrence_id: Option<meerkat::OccurrenceId>,
        /// The session id a materialize-on-demand fire of the occurrence
        /// uses.
        occurrence_session: Option<SessionId>,
        /// The occurrence's phase once terminal, or when the wait ran out.
        phase: Option<OccurrencePhase>,
        failure_class: Option<meerkat::OccurrenceFailureClass>,
        failure_detail: Option<String>,
        /// The session the schedule's target is bound to after the fire.
        bound_session: Option<SessionId>,
        /// Sessions that exist after the fire and did not before, each with
        /// the kinds of its transcript's messages and its last assistant
        /// text.
        new_sessions: Vec<(SessionId, Vec<&'static str>, Option<String>)>,
        /// The member's agent events after a fire to the member, by type.
        member_events: Vec<String>,
        /// The typed failure of a member run the fire started, if one failed.
        run_failure: Option<meerkat_core::event::AgentErrorReport>,
        /// The member's session after the fire, and its message count before
        /// and after.
        member_session_after: Option<SessionId>,
        member_messages: (usize, usize),
        /// Provider calls the fire caused.
        llm_calls: usize,
        /// Native ingress checks the governed composition was asked.
        ingress_checks: usize,
    }

    fn message_kind(message: &meerkat_core::Message) -> &'static str {
        match message {
            meerkat_core::Message::System(_) => "system",
            meerkat_core::Message::SystemNotice(_) => "system_notice",
            meerkat_core::Message::User(_) => "user",
            meerkat_core::Message::BlockAssistant(_) => "assistant",
            meerkat_core::Message::ToolResults { .. } => "tool_results",
        }
    }

    /// A Meta runtime and its schedule host, composed the way the gateway
    /// composes a persistent runtime: the mob's members and the schedule
    /// host's session host share one runtime machine, built with or without
    /// the governed composition [`scheduled_member_fire`] uses.
    struct HostedScheduleFixture {
        runtime: crate::UnifiedRuntime,
        sessions: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        schedules: ScheduleService,
        host: ScheduleHostHandle,
        mob_id: String,
        member: meerkat_mob::ids::AgentIdentity,
        member_session: SessionId,
        llm_calls: Arc<std::sync::atomic::AtomicUsize>,
        ingress_checks: Arc<std::sync::atomic::AtomicUsize>,
        _temp_dir: tempfile::TempDir,
    }

    impl HostedScheduleFixture {
        async fn new(governed: bool) -> Self {
            Self::with_runnables(governed, None).await
        }

        /// [`HostedScheduleFixture::new`], its schedule host serving
        /// `runnables` too.
        async fn with_runnables(
            governed: bool,
            runnables: Option<Arc<dyn meerkat::ScheduleRunnableHost>>,
        ) -> Self {
            use std::sync::atomic::AtomicUsize;

            let temp_dir = tempfile::tempdir().expect("temp dir");
            let state = temp_dir.path().join("state");
            std::fs::create_dir_all(&state).expect("state dir");
            let llm_calls = Arc::new(AtomicUsize::new(0));
            let ingress_checks = Arc::new(AtomicUsize::new(0));
            let counting_client: Arc<dyn meerkat_client::LlmClient> = Arc::new(CountingLlmClient {
                calls: llm_calls.clone(),
            });
            let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
                meerkat_store::SqliteSessionStore::open(state.join("sessions.db"))
                    .expect("session store"),
            );
            let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
                meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))
                    .expect("runtime store"),
            );
            let blob_store: Arc<dyn meerkat_core::BlobStore> =
                Arc::new(meerkat_store::MemoryBlobStore::new());
            let mut builder = FactoryAgentBuilder::new(
                AgentFactory::new(&state).comms(true),
                scheduled_fire_config(),
            );
            // Sessions the schedule host materializes build on this builder.
            builder.default_llm_client = Some(Arc::clone(&counting_client));
            builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
                session_store.clone(),
            )));
            builder.default_blob_store = Some(blob_store.clone());
            let jobs: Arc<dyn meerkat::DetachedJobStore> =
                Arc::new(meerkat::MemoryDetachedJobStore::new());
            builder.default_detached_job_store = Some(Arc::clone(&jobs));
            let delivery = crate::mob_handle_runtime::MobRuntimeDelivery::new(
                Arc::clone(&runtime_store),
                jobs,
            );
            let attached = attach_schedule_tools_with_identity_targets(&builder, &state)
                .expect("schedule tools attach");
            let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
            let machine = Arc::new(
                crate::mob_handle_runtime::persistent_runtime_machine(
                    Arc::clone(&runtime_store),
                    Arc::clone(&blob_store),
                    governed.then(|| counting_governed_configuration(ingress_checks.clone())),
                )
                .expect("runtime machine"),
            );
            let sessions = Arc::new(PersistentSessionService::new(
                builder,
                16,
                session_store,
                runtime_store,
                blob_store,
            ));
            let mob_id = format!(
                "schedule-hosted-{}-{}",
                if governed { "on" } else { "off" },
                uuid::Uuid::new_v4().simple()
            );
            let mut spec = crate::mob_handle_runtime::MobBootstrapSpec::new(
                scheduled_worker_definition(&mob_id),
                meerkat_mob::MobStorage::in_memory(),
                sessions.clone(),
            )
            .with_session_runtime_adapter(Arc::clone(&machine))
            .expect("session runtime owner")
            .with_runtime_delivery(delivery)
            .with_agent_mob_tools(mob_tools_slot)
            .expect("agent mob tools")
            .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
                allow_ephemeral_sessions: true,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(counting_client),
            });
            if governed {
                spec = spec
                    .with_member_runtime_registration_hook(Arc::new(
                        crate::unified_runtime::held_identity_test_support::SelectingHook,
                    ))
                    .with_tool_consequence_policy_registry(host_policy_registry());
            }
            let runtime = crate::UnifiedRuntime::bootstrap(
                spec,
                crate::MetaConfig {
                    modules: vec![],
                    discovery: crate::DiscoverySpec {
                        namespace: "schedule-hosted".to_string(),
                        modules: vec![],
                    },
                    pre_spawn: vec![],
                },
                Duration::from_secs(2),
            )
            .await
            .expect("bootstrap");
            let member = meerkat_mob::ids::AgentIdentity::from("scheduled-worker");
            runtime
                .spawn_many(vec![meerkat_mob::SpawnMemberSpec::new(
                    meerkat_mob::ProfileName::from("worker"),
                    member.clone(),
                )])
                .await
                .expect("spawn the member");
            let handle = runtime.mob_handle();
            let member_session = handle
                .resolve_bridge_session_id(&member)
                .await
                .expect("the member has a session");
            let host = spawn_schedule_host_with_identity_runtime(
                Arc::clone(&sessions),
                machine,
                attached.service.clone(),
                runtime.mob_runtime().agent_mob_mcp_state(),
                handle,
                None,
                runnables,
                None,
                "schedule-hosted",
            )
            .expect("spawn the schedule host");
            Self {
                runtime,
                sessions,
                schedules: attached.service,
                host,
                mob_id,
                member,
                member_session,
                llm_calls,
                ingress_checks,
                _temp_dir: temp_dir,
            }
        }

        async fn target(&self, kind: ScheduledTargetKind) -> TargetBinding {
            let prompt = meerkat_core::ContentInput::Text("run the scheduled check".to_string());
            let action = ScheduledSessionAction::Prompt {
                prompt: prompt.clone(),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            };
            match kind {
                ScheduledTargetKind::MobMember => {
                    TargetBinding::Mob(Box::new(MobTargetBinding::Member {
                        mob_id: self.mob_id.clone(),
                        member_id: crate::member_comms_id::mob_member_id("scheduled-worker")
                            .to_string(),
                        action: ScheduledMobAction::Send {
                            content: prompt,
                            render_metadata: None,
                        },
                    }))
                }
                ScheduledTargetKind::MemberIdentity => {
                    let binding = self
                        .sessions
                        .load_authoritative_session(&self.member_session)
                        .await
                        .expect("load the member's session")
                        .expect("the member's session exists")
                        .session_metadata()
                        .and_then(|metadata| metadata.mob_member_binding)
                        .expect("the member's session names its mob member");
                    TargetBinding::identity(meerkat::IdentityTargetBinding::resumable(
                        meerkat::surface::mob_member_schedule_identity(&binding),
                        action,
                    ))
                }
                ScheduledTargetKind::MemberSession => {
                    TargetBinding::session(SessionTargetBinding::ExactSession {
                        session_id: self.member_session.clone(),
                        action,
                    })
                }
                ScheduledTargetKind::MaterializedSession => {
                    TargetBinding::session(SessionTargetBinding::materialize_on_demand(
                        meerkat::SessionMaterializationSpec {
                            model: "gpt-5.5".to_string(),
                            system_prompt: None,
                            max_tokens: None,
                            provider: None,
                            output_schema: None,
                            structured_output_retries: None,
                            provider_params: None,
                            comms_name: None,
                            peer_meta: None,
                            labels: BTreeMap::new(),
                            preload_skills: Vec::new(),
                            additional_instructions: Vec::new(),
                            realm_id: None,
                            instance_id: None,
                            backend: None,
                            config_generation: None,
                            keep_alive: false,
                            app_context: None,
                        },
                        action,
                    ))
                }
            }
        }

        async fn session_ids(&self) -> std::collections::BTreeSet<SessionId> {
            use meerkat_core::service::SessionService;

            self.sessions
                .list(meerkat_core::service::SessionQuery::default())
                .await
                .expect("list sessions")
                .into_iter()
                .map(|summary| summary.session_id)
                .collect()
        }

        async fn member_message_count(&self) -> usize {
            use meerkat_core::service::SessionService;

            self.sessions
                .read(&self.member_session)
                .await
                .expect("read the member's session")
                .state
                .message_count
        }

        /// The member's message count once it reaches `expected`, or what it
        /// is after [`HOSTED_FIRE_RUN_WINDOW`]: the read can trail the run's
        /// events.
        async fn member_message_count_reaching(&self, expected: usize) -> usize {
            let deadline = tokio::time::Instant::now() + HOSTED_FIRE_RUN_WINDOW;
            loop {
                let count = self.member_message_count().await;
                if count == expected || tokio::time::Instant::now() >= deadline {
                    return count;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        /// Schedule one fire of `kind`'s target due in a second, let the
        /// schedule host claim and deliver it, and follow its occurrence to a
        /// terminal phase or [`HOSTED_FIRE_TERMINAL_WAIT`]. A fire to the
        /// member is then followed until a run it started ends, or
        /// [`HOSTED_FIRE_RUN_WINDOW`] shows none started.
        async fn fire(&self, kind: ScheduledTargetKind) -> HostedFireOutcome {
            use futures::StreamExt;
            use meerkat_core::service::SessionService;
            use std::sync::atomic::Ordering;

            let target = self.target(kind).await;
            let handle = self.runtime.mob_handle();
            let sessions_before = self.session_ids().await;
            let member_messages_before = self.member_message_count().await;
            let mut probe = match kind {
                ScheduledTargetKind::MaterializedSession => None,
                _ => Some(
                    handle
                        .subscribe_agent_events(&self.member)
                        .await
                        .expect("probe the member's events"),
                ),
            };
            let llm_baseline = self.llm_calls.load(Ordering::SeqCst);
            let ingress_baseline = self.ingress_checks.load(Ordering::SeqCst);
            let schedule = self
                .schedules
                .create(CreateScheduleRequest {
                    name: Some(format!("{kind:?}")),
                    description: None,
                    trigger: TriggerSpec::Once {
                        due_at_utc: chrono::Utc::now() + chrono::Duration::seconds(1),
                    },
                    target,
                    misfire_policy: meerkat::MisfirePolicy::default(),
                    overlap_policy: meerkat::OverlapPolicy::default(),
                    missing_target_policy: meerkat::MissingTargetPolicy::default(),
                    labels: BTreeMap::new(),
                    planning_horizon_days: None,
                    planning_horizon_occurrences: None,
                })
                .await
                .expect("create the schedule");
            let deadline = tokio::time::Instant::now() + HOSTED_FIRE_TERMINAL_WAIT;
            let occurrence = loop {
                let occurrence = self
                    .schedules
                    .list_occurrences(&schedule.schedule_id)
                    .await
                    .expect("list occurrences")
                    .into_iter()
                    .next();
                let terminal = occurrence.as_ref().is_some_and(|occurrence| {
                    matches!(
                        occurrence.phase,
                        OccurrencePhase::Completed
                            | OccurrencePhase::Skipped
                            | OccurrencePhase::Misfired
                            | OccurrencePhase::Superseded
                            | OccurrencePhase::DeliveryFailed
                    )
                });
                if terminal || tokio::time::Instant::now() >= deadline {
                    break occurrence;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            };
            let mut member_events = Vec::new();
            let mut run_failure = None;
            if let Some(probe) = probe.as_mut() {
                let _ = tokio::time::timeout(HOSTED_FIRE_RUN_WINDOW, async {
                    while let Some(event) = probe.next().await {
                        member_events.push(
                            meerkat_core::event::agent_event_type(&event.payload).to_string(),
                        );
                        match event.payload {
                            meerkat_core::event::AgentEvent::RunCompleted { .. } => break,
                            meerkat_core::event::AgentEvent::RunFailed { error_report, .. } => {
                                run_failure = Some(error_report);
                                break;
                            }
                            _ => {}
                        }
                    }
                })
                .await;
            }
            drop(probe);
            let mut new_sessions = Vec::new();
            for session_id in self.session_ids().await {
                if sessions_before.contains(&session_id) {
                    continue;
                }
                let kinds = self
                    .sessions
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("load the new session")
                    .expect("the new session exists")
                    .messages()
                    .iter()
                    .map(message_kind)
                    .collect();
                let view = self
                    .sessions
                    .read(&session_id)
                    .await
                    .expect("read the new session");
                new_sessions.push((session_id, kinds, view.state.last_assistant_text));
            }
            let bound_session = match self
                .schedules
                .get(&schedule.schedule_id)
                .await
                .expect("read the schedule")
                .target
            {
                TargetBinding::Session(binding) => binding.resolved_session_id().cloned(),
                _ => None,
            };
            HostedFireOutcome {
                schedule_id: schedule.schedule_id,
                occurrence_id: occurrence
                    .as_ref()
                    .map(|occurrence| occurrence.occurrence_id.clone()),
                occurrence_session: occurrence.as_ref().map(Occurrence::materialized_session_id),
                phase: occurrence.as_ref().map(|occurrence| occurrence.phase),
                failure_class: occurrence
                    .as_ref()
                    .and_then(|occurrence| occurrence.failure_class),
                failure_detail: occurrence
                    .as_ref()
                    .and_then(|occurrence| occurrence.failure_detail.clone()),
                bound_session,
                new_sessions,
                member_events,
                run_failure,
                member_session_after: handle.resolve_bridge_session_id(&self.member).await,
                member_messages: (member_messages_before, self.member_message_count().await),
                llm_calls: self.llm_calls.load(Ordering::SeqCst) - llm_baseline,
                ingress_checks: self.ingress_checks.load(Ordering::SeqCst) - ingress_baseline,
            }
        }

        async fn shutdown(self) {
            self.host.shutdown().await;
            self.runtime.shutdown().await;
        }
    }

    /// The refusal Core gives a governed runtime's input that carries no
    /// input authority association.
    const NATIVE_ADMISSION_REFUSAL: &str =
        "Input validation failed: native work authority association is unavailable or mismatched";

    /// The schedule host's member fires: a mob target naming the member, and
    /// the member's `mob_member:` identity, which Meta delivers through the
    /// same internal work door. On a runtime built with
    /// `native_grant_work_configuration` each fails at the fire: the member's
    /// session refuses the internal work at native admission, so the
    /// occurrence ends `DeliveryFailed` (`RuntimeRejected`), no run starts,
    /// no provider call is made, and the member keeps its session and
    /// transcript. Without that configuration each runs the member's turn
    /// and completes.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_governed_members_identity_fire_fails_like_its_mob_target_fire() {
        let kinds = [
            ScheduledTargetKind::MobMember,
            ScheduledTargetKind::MemberIdentity,
        ];
        let ungoverned = HostedScheduleFixture::new(false).await;
        for kind in kinds {
            let outcome = ungoverned.fire(kind).await;
            assert_eq!(
                outcome.phase,
                Some(OccurrencePhase::Completed),
                "{kind:?}: {outcome:#?}"
            );
            assert_eq!(outcome.failure_class, None, "{kind:?}");
            assert_eq!(outcome.failure_detail, None, "{kind:?}");
            assert_eq!(
                outcome.member_events.first().map(String::as_str),
                Some("run_started"),
                "{kind:?}: {outcome:#?}"
            );
            assert_eq!(
                outcome.member_events.last().map(String::as_str),
                Some("run_completed"),
                "{kind:?}: {outcome:#?}"
            );
            assert!(outcome.run_failure.is_none(), "{kind:?}: {outcome:#?}");
            assert_eq!(outcome.llm_calls, 1, "{kind:?} runs one turn");
            assert_eq!(outcome.ingress_checks, 0, "{kind:?}");
            assert!(outcome.new_sessions.is_empty(), "{kind:?}: {outcome:#?}");
            assert_eq!(
                outcome.member_session_after.as_ref(),
                Some(&ungoverned.member_session),
                "{kind:?} runs in the member's own session"
            );
            let expected = outcome.member_messages.0 + 2;
            assert_eq!(
                ungoverned.member_message_count_reaching(expected).await,
                expected,
                "{kind:?} adds the prompt and the reply: {outcome:#?}"
            );
        }
        ungoverned.shutdown().await;

        let governed = HostedScheduleFixture::new(true).await;
        let refusal = format!(
            "internal schedule delivery failed (member scheduled-worker): member session {} did \
             not admit the input: {NATIVE_ADMISSION_REFUSAL}",
            governed.member_session
        );
        for kind in kinds {
            let outcome = governed.fire(kind).await;
            assert_eq!(
                outcome.phase,
                Some(OccurrencePhase::DeliveryFailed),
                "{kind:?}: {outcome:#?}"
            );
            assert_eq!(
                outcome.failure_class,
                Some(meerkat::OccurrenceFailureClass::RuntimeRejected),
                "{kind:?}"
            );
            assert_eq!(
                outcome.failure_detail.as_deref(),
                Some(refusal.as_str()),
                "{kind:?}"
            );
            assert!(
                outcome.member_events.is_empty(),
                "{kind:?} starts no run: {outcome:#?}"
            );
            assert!(outcome.run_failure.is_none(), "{kind:?}");
            assert_eq!(outcome.llm_calls, 0, "{kind:?} runs no turn");
            assert_eq!(
                outcome.ingress_checks, 0,
                "{kind:?} is refused before the host's ingress check is asked"
            );
            assert!(outcome.new_sessions.is_empty(), "{kind:?}: {outcome:#?}");
            assert_eq!(
                outcome.member_session_after.as_ref(),
                Some(&governed.member_session),
                "{kind:?}"
            );
            assert_eq!(
                outcome.member_messages.1, outcome.member_messages.0,
                "{kind:?} leaves the member's transcript unchanged"
            );
        }
        governed.shutdown().await;
    }

    /// A session target naming a mob member's session reaches it through
    /// the schedule host's session host, which submits the prompt as plain
    /// runtime input. On a runtime built with
    /// `native_grant_work_configuration` Core refuses that input at
    /// admission, and again on the host's exact redrive, so the occurrence
    /// ends `DeliveryFailed` classed `TransportError`; no run starts, no
    /// provider call is made, and the member keeps its session and
    /// transcript. Without that configuration the fire runs the member's
    /// turn and completes.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_governed_members_session_target_fire_does_not_run_and_an_ungoverned_one_does() {
        let ungoverned = HostedScheduleFixture::new(false).await;
        let outcome = ungoverned.fire(ScheduledTargetKind::MemberSession).await;
        assert_eq!(
            outcome.phase,
            Some(OccurrencePhase::Completed),
            "{outcome:#?}"
        );
        assert_eq!(outcome.failure_class, None);
        assert_eq!(outcome.failure_detail, None);
        assert_eq!(
            outcome.member_events.first().map(String::as_str),
            Some("run_started"),
            "{outcome:#?}"
        );
        assert_eq!(
            outcome.member_events.last().map(String::as_str),
            Some("run_completed"),
            "{outcome:#?}"
        );
        assert!(outcome.run_failure.is_none(), "{outcome:#?}");
        assert_eq!(outcome.llm_calls, 1, "the ungoverned fire runs one turn");
        assert_eq!(outcome.ingress_checks, 0);
        assert!(outcome.new_sessions.is_empty(), "{outcome:#?}");
        assert_eq!(
            outcome.bound_session.as_ref(),
            Some(&ungoverned.member_session)
        );
        assert_eq!(
            outcome.member_session_after.as_ref(),
            Some(&ungoverned.member_session)
        );
        let expected = outcome.member_messages.0 + 2;
        assert_eq!(
            ungoverned.member_message_count_reaching(expected).await,
            expected,
            "the fire adds the prompt and the reply: {outcome:#?}"
        );
        ungoverned.shutdown().await;

        let governed = HostedScheduleFixture::new(true).await;
        let outcome = governed.fire(ScheduledTargetKind::MemberSession).await;
        let occurrence = outcome
            .occurrence_id
            .clone()
            .expect("the fire's occurrence");
        assert_eq!(
            outcome.phase,
            Some(OccurrencePhase::DeliveryFailed),
            "{outcome:#?}"
        );
        assert_eq!(
            outcome.failure_class,
            Some(meerkat::OccurrenceFailureClass::TransportError)
        );
        assert_eq!(
            outcome.failure_detail,
            Some(format!(
                "internal error: runtime admission failed before any durable binding for \
                 stable key schedule:{}:occurrence:{occurrence} (internal error: \
                 {NATIVE_ADMISSION_REFUSAL}; redrive: internal error: \
                 {NATIVE_ADMISSION_REFUSAL})",
                outcome.schedule_id
            ))
        );
        assert!(
            outcome.member_events.is_empty(),
            "no run starts: {outcome:#?}"
        );
        assert!(outcome.run_failure.is_none());
        assert_eq!(outcome.llm_calls, 0, "the governed fire runs no turn");
        assert_eq!(
            outcome.ingress_checks, 0,
            "refused before the host's ingress check is asked"
        );
        assert!(outcome.new_sessions.is_empty(), "{outcome:#?}");
        assert_eq!(
            outcome.member_session_after.as_ref(),
            Some(&governed.member_session)
        );
        assert_eq!(
            outcome.member_messages.1, outcome.member_messages.0,
            "the member's transcript is unchanged"
        );
        governed.shutdown().await;
    }

    /// A session target that materializes a session at the fire. On a
    /// runtime built with `native_grant_work_configuration` the fire fails
    /// late, after materializing: the schedule host materializes the
    /// session (under the occurrence's session id) and binds it to the
    /// schedule, then Core refuses the prompt at admission, and again on
    /// the host's exact redrive, so the occurrence ends `DeliveryFailed`
    /// classed `TransportError`. The materialized session stays behind
    /// holding only its system prompt. No event probe watches that session,
    /// so the evidence that no run started is its system-only transcript
    /// and the absence of any provider call.
    ///
    /// Without that configuration the fire materializes the session, runs
    /// its turn and completes. Before Meerkat 0.9.3 the occurrence stayed
    /// `Dispatching` after the turn ran: the first fire of a
    /// materialize-on-demand target never completed.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_governed_materialized_session_fire_materializes_then_fails_without_running() {
        let ungoverned = HostedScheduleFixture::new(false).await;
        let outcome = ungoverned
            .fire(ScheduledTargetKind::MaterializedSession)
            .await;
        let session = outcome
            .occurrence_session
            .clone()
            .expect("the fire's occurrence");
        // Meerkat 0.9.3: the first materialize-on-demand fire completes.
        assert_eq!(
            outcome.phase,
            Some(OccurrencePhase::Completed),
            "{outcome:#?}"
        );
        assert_eq!(outcome.failure_class, None);
        assert_eq!(outcome.failure_detail, None);
        assert_eq!(outcome.bound_session.as_ref(), Some(&session));
        assert_eq!(
            outcome.new_sessions,
            vec![(
                session,
                vec!["system", "user", "assistant"],
                Some("done".to_string())
            )],
            "the materialized session runs its turn: {outcome:#?}"
        );
        assert_eq!(outcome.llm_calls, 1, "the ungoverned fire runs one turn");
        assert_eq!(outcome.ingress_checks, 0);
        assert_eq!(
            outcome.member_messages.1, outcome.member_messages.0,
            "the member is not addressed"
        );
        ungoverned.shutdown().await;

        let governed = HostedScheduleFixture::new(true).await;
        let outcome = governed
            .fire(ScheduledTargetKind::MaterializedSession)
            .await;
        let occurrence = outcome
            .occurrence_id
            .clone()
            .expect("the fire's occurrence");
        let session = outcome
            .occurrence_session
            .clone()
            .expect("the fire's occurrence");
        assert_eq!(
            outcome.phase,
            Some(OccurrencePhase::DeliveryFailed),
            "{outcome:#?}"
        );
        assert_eq!(
            outcome.failure_class,
            Some(meerkat::OccurrenceFailureClass::TransportError)
        );
        assert_eq!(
            outcome.failure_detail,
            Some(format!(
                "internal error: runtime admission failed before any durable binding for \
                 stable key schedule:{}:occurrence:{occurrence} (internal error: \
                 {NATIVE_ADMISSION_REFUSAL}; redrive: internal error: \
                 {NATIVE_ADMISSION_REFUSAL})",
                outcome.schedule_id
            ))
        );
        assert_eq!(
            outcome.bound_session.as_ref(),
            Some(&session),
            "the materialized session is bound to the schedule"
        );
        assert_eq!(
            outcome.new_sessions,
            vec![(session, vec!["system"], None)],
            "the session is materialized and holds only its system prompt: {outcome:#?}"
        );
        assert_eq!(outcome.llm_calls, 0, "the governed fire runs no turn");
        assert_eq!(
            outcome.ingress_checks, 0,
            "refused before the host's ingress check is asked"
        );
        assert_eq!(
            outcome.member_messages.1, outcome.member_messages.0,
            "the member is not addressed"
        );
        governed.shutdown().await;
    }

    /// What one scheduled fire to an identity-first member did on the
    /// identity lane: the schedule host's identity runtime dispatches it.
    #[derive(Debug)]
    struct IdentityFireOutcome {
        /// The receipt detail of a dispatch, or the schedule domain error.
        dispatch: Result<Option<String>, String>,
        terminal_phase: Option<OccurrencePhase>,
        terminal_reason: Option<meerkat::DeliveryFailureReason>,
        terminal_detail: Option<String>,
        /// The identity's lifecycle state, mob member count and session
        /// before and after the fire.
        state: (
            crate::identity_first::IdentityLifecycleState,
            crate::identity_first::IdentityLifecycleState,
        ),
        members: (usize, usize),
        session: (Option<SessionId>, Option<SessionId>),
        /// The identity's session after the fire: message count and last
        /// assistant text.
        transcript_after: Option<(usize, Option<String>)>,
        /// The member's agent events after the fire, when it was live before.
        member_events: Vec<String>,
        run_failure: Option<meerkat_core::event::AgentErrorReport>,
        /// Provider calls the fire caused.
        llm_calls: usize,
        /// Native ingress checks the governed composition was asked.
        ingress_checks: usize,
    }

    /// Fire one due schedule whose target is an identity-first member's mob
    /// target through [`InternalDeliveryScheduleMobHost`] with the runtime's
    /// identity runtime, as the gateway composes it, on a runtime built with
    /// or without `native_grant_work_configuration` (the composition
    /// [`scheduled_member_fire`] uses). The roster registers lazily, so the
    /// identity is dormant until something materializes it; `dormant` keeps
    /// it so up to the fire.
    async fn scheduled_identity_fire(governed: bool, dormant: bool) -> IdentityFireOutcome {
        use crate::identity_first::{
            AgentIdentity, ContinuityStore, IdentityFirstRuntimeContext, LocalContinuityStore,
        };
        use crate::unified_runtime::held_identity_test_support as support;
        use futures::StreamExt;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let llm_calls = Arc::new(AtomicUsize::new(0));
        let ingress_checks = Arc::new(AtomicUsize::new(0));
        let mob_id = format!(
            "schedule-identity-{}-{}",
            if governed { "on" } else { "off" },
            uuid::Uuid::new_v4().simple()
        );
        let mut builder = crate::UnifiedRuntime::builder()
            .definition(scheduled_worker_definition(&mob_id))
            .meerkat_config(scheduled_fire_config())
            .persistent_state(temp_dir.path().join("state"))
            .default_llm_client(Arc::new(CountingLlmClient {
                calls: llm_calls.clone(),
            }));
        if governed {
            builder = builder
                .native_grant_work_configuration(counting_governed_configuration(
                    ingress_checks.clone(),
                ))
                .member_runtime_registration_hook(Arc::new(support::SelectingHook))
                .tool_consequence_policy_registry(host_policy_registry());
        }
        let mut runtime = builder.build().await.expect("build runtime");
        let identity = AgentIdentity::parse("scheduled-identity").expect("identity");
        let roster = vec![support::member(&identity)];
        let continuity: Arc<dyn ContinuityStore> =
            Arc::new(LocalContinuityStore::in_memory().expect("continuity store"));
        let identity_runtime = support::identity_runtime(&runtime, continuity, &mob_id);
        runtime.install_identity_first_context(Arc::new(
            IdentityFirstRuntimeContext::new_with_lazy_materialization(
                Arc::clone(&identity_runtime),
                Arc::new(support::Roster(roster.clone())),
                None,
                None,
                Some(runtime.mob_handle().definition().clone()),
                true,
            ),
        ));
        runtime
            .bootstrap_installed_identity_first_context(&roster)
            .await
            .expect("register the roster");
        if !dormant {
            identity_runtime
                .materialize(&identity)
                .await
                .expect("materialize the identity");
        }
        let handle = runtime.mob_handle();
        let member = crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let status_before = identity_runtime
            .status(&identity)
            .await
            .expect("identity status");
        let members_before = handle.list_members().await.len();
        let mut probe = if dormant {
            None
        } else {
            Some(
                handle
                    .subscribe_agent_events(&member)
                    .await
                    .expect("probe the member's events"),
            )
        };
        let llm_baseline = llm_calls.load(Ordering::SeqCst);
        let ingress_baseline = ingress_checks.load(Ordering::SeqCst);

        let binding = MobTargetBinding::Member {
            mob_id: mob_id.clone(),
            member_id: member.to_string(),
            action: ScheduledMobAction::Send {
                content: meerkat_core::ContentInput::Text("run the scheduled check".to_string()),
                render_metadata: None,
            },
        };
        let schedule = Schedule::new(CreateScheduleRequest {
            name: Some("governed-identity-fire".to_string()),
            description: None,
            trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc: chrono::Utc::now(),
                every_seconds: 60,
                end_at_utc: None,
            }),
            target: TargetBinding::Mob(Box::new(binding.clone())),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("schedule");
        let occurrence = Occurrence::planned_from_schedule(
            &schedule,
            meerkat::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("occurrence");
        let host = InternalDeliveryScheduleMobHost {
            inner: Arc::new(NoopScheduleMobHost::new("unused fallback")),
            handle: handle.clone(),
            identity_runtime: runtime.identity_runtime_ref().cloned(),
        };
        let (dispatch_result, terminal) = match host
            .deliver_mob_target(
                &occurrence,
                &meerkat::ScheduleDeliveryIdentity::for_occurrence(&occurrence),
                &binding,
            )
            .await
        {
            Ok(dispatch) => {
                let detail = dispatch.receipt.detail.clone();
                let terminal = dispatch.completion.await.expect("delivery terminal");
                (Ok(detail), Some(terminal))
            }
            Err(error) => (Err(error.to_string()), None),
        };
        let mut member_events = Vec::new();
        let mut run_failure = None;
        match probe.as_mut() {
            Some(probe) => {
                let _ = tokio::time::timeout(Duration::from_secs(5), async {
                    while let Some(event) = probe.next().await {
                        member_events.push(
                            meerkat_core::event::agent_event_type(&event.payload).to_string(),
                        );
                        match event.payload {
                            meerkat_core::event::AgentEvent::RunCompleted { .. } => break,
                            meerkat_core::event::AgentEvent::RunFailed { error_report, .. } => {
                                run_failure = Some(error_report);
                                break;
                            }
                            _ => {}
                        }
                    }
                })
                .await;
            }
            // No member to probe before the fire: give a run it started the
            // same window to reach the provider.
            None => tokio::time::sleep(Duration::from_secs(5)).await,
        }
        drop(probe);
        let status_after = identity_runtime
            .status(&identity)
            .await
            .expect("identity status");
        // The session read can trail a run's events: after a provider call,
        // wait (bounded) for the reply to reach it.
        let ran = llm_calls.load(Ordering::SeqCst) > llm_baseline;
        let mut transcript_after = None;
        if let (Some(session), Some(sessions)) = (
            status_after.session_id.as_ref(),
            runtime.mob_runtime().session_service(),
        ) {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                transcript_after = sessions
                    .read(session)
                    .await
                    .ok()
                    .map(|view| (view.state.message_count, view.state.last_assistant_text));
                let settled = transcript_after
                    .as_ref()
                    .is_some_and(|(_, reply)| reply.is_some());
                if !ran || settled || tokio::time::Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        let outcome = IdentityFireOutcome {
            dispatch: dispatch_result,
            terminal_phase: terminal.as_ref().map(|terminal| terminal.phase),
            terminal_reason: terminal
                .as_ref()
                .and_then(|terminal| terminal.delivery_failure_reason),
            terminal_detail: terminal.and_then(|terminal| terminal.detail),
            state: (status_before.state, status_after.state),
            members: (members_before, handle.list_members().await.len()),
            session: (status_before.session_id, status_after.session_id),
            transcript_after,
            member_events,
            run_failure,
            llm_calls: llm_calls.load(Ordering::SeqCst) - llm_baseline,
            ingress_checks: ingress_checks.load(Ordering::SeqCst) - ingress_baseline,
        };
        drop(host);
        runtime.shutdown().await;
        outcome
    }

    /// The gateway hands the schedule host the runtime's identity runtime,
    /// so a fire to a durable identity's member takes the identity lane
    /// (the identity runtime's dispatch, through its session bridge), not
    /// the plain internal work door. On a runtime built with
    /// `native_grant_work_configuration` that lane meets the same refusal:
    /// the occurrence ends `DeliveryFailed` (`RuntimeRejected`) at the fire,
    /// no run starts and no provider call is made. A dormant identity is
    /// materialized first: its member is created and its session is created
    /// or resumed, and both stay live. The identity here was never embodied,
    /// so its session is fresh and holds only its system prompt; no probe
    /// watches a member that does not exist before the fire, so its no-run
    /// evidence is that system-only transcript and the absence of provider
    /// calls. Without that configuration an active and a dormant identity
    /// both run the turn.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_governed_identity_lane_fire_does_not_run_and_materializes_a_dormant_identity_first()
    {
        use crate::identity_first::IdentityLifecycleState::{Active, Dormant};

        for dormant in [false, true] {
            let ungoverned = scheduled_identity_fire(false, dormant).await;
            assert_eq!(ungoverned.dispatch, Ok(None), "{ungoverned:#?}");
            assert_eq!(
                ungoverned.terminal_phase,
                Some(OccurrencePhase::Completed),
                "{ungoverned:#?}"
            );
            assert_eq!(ungoverned.terminal_reason, None);
            assert_eq!(ungoverned.terminal_detail, None);
            assert_eq!(
                ungoverned.state,
                (if dormant { Dormant } else { Active }, Active)
            );
            assert_eq!(ungoverned.members, (usize::from(!dormant), 1));
            assert!(ungoverned.session.1.is_some(), "{ungoverned:#?}");
            if !dormant {
                assert_eq!(ungoverned.session.0, ungoverned.session.1);
                assert_eq!(
                    ungoverned.member_events.first().map(String::as_str),
                    Some("run_started"),
                    "{ungoverned:#?}"
                );
                assert_eq!(
                    ungoverned.member_events.last().map(String::as_str),
                    Some("run_completed"),
                    "{ungoverned:#?}"
                );
            }
            assert_eq!(
                ungoverned.transcript_after,
                Some((3, Some("done".to_string()))),
                "the ungoverned fire runs one turn: {ungoverned:#?}"
            );
            assert!(ungoverned.run_failure.is_none());
            assert_eq!(ungoverned.llm_calls, 1);
            assert_eq!(ungoverned.ingress_checks, 0);

            let governed = scheduled_identity_fire(true, dormant).await;
            let session = governed
                .session
                .1
                .clone()
                .expect("the identity has a session after the fire");
            assert_eq!(governed.dispatch, Ok(None), "{governed:#?}");
            assert_eq!(
                governed.terminal_phase,
                Some(OccurrencePhase::DeliveryFailed),
                "{governed:#?}"
            );
            assert_eq!(
                governed.terminal_reason,
                Some(meerkat::DeliveryFailureReason::RuntimeRejected)
            );
            assert_eq!(
                governed.terminal_detail,
                Some(format!(
                    "identity schedule delivery failed (member scheduled-identity): internal: \
                     bridge deliver: session bridge mob error: session bridge mob error: member \
                     session {session} did not admit the input: {NATIVE_ADMISSION_REFUSAL}"
                ))
            );
            if dormant {
                assert_eq!(
                    (governed.state, governed.members, governed.session.0.clone()),
                    ((Dormant, Active), (0, 1), None),
                    "a dormant identity is materialized before the refusal"
                );
            } else {
                assert_eq!(
                    (governed.state, governed.members, governed.session.0.clone()),
                    ((Active, Active), (1, 1), Some(session)),
                    "an active identity keeps its member and session"
                );
            }
            assert_eq!(
                governed.transcript_after,
                Some((1, None)),
                "the session holds only its system prompt: {governed:#?}"
            );
            if !dormant {
                assert!(
                    governed.member_events.is_empty(),
                    "no run starts: {governed:#?}"
                );
            }
            assert!(governed.run_failure.is_none());
            assert_eq!(governed.llm_calls, 0, "the governed fire runs no turn");
            assert_eq!(
                governed.ingress_checks, 0,
                "refused before the host's ingress check is asked"
            );
        }
    }

    // -----------------------------------------------------------------------
    // A Rust host's runnables, and mob announcement sources' runners
    // -----------------------------------------------------------------------

    /// A runnable that keeps every invocation it is handed.
    #[derive(Default)]
    struct RecordingRunnable {
        invocations: std::sync::Mutex<Vec<HostRunnableInvocation>>,
    }

    impl RecordingRunnable {
        fn handed(&self) -> Vec<HostRunnableInvocation> {
            self.invocations.lock().expect("invocations lock").clone()
        }
    }

    #[async_trait]
    impl HostRunnable for RecordingRunnable {
        async fn run(
            &self,
            invocation: HostRunnableInvocation,
        ) -> Result<HostRunnableOutcome, HostRunnableError> {
            self.invocations
                .lock()
                .expect("invocations lock")
                .push(invocation);
            Ok(HostRunnableOutcome::completed())
        }
    }

    /// A Rust host's runnables (what `GatewayHostComposition::host_runnables`
    /// hands init) register beside the SDK's callback runnables, and a fire
    /// hands one Core's invocation unchanged: its occurrence, schedule,
    /// trigger time, params and delivery idempotency key are the ones Core
    /// built. A callback runnable's name taken again, or the steward's
    /// reserved one, is refused.
    #[tokio::test]
    async fn a_rust_host_runnable_is_registered_beside_the_callback_runnables() {
        let bridge = ScriptedCallbackBridge::new(Ok(Value::Null));
        let collector = Arc::new(RecordingRunnable::default());
        let host = gateway_runnable_host_with(
            None,
            Some((
                Arc::clone(&bridge) as Arc<dyn CallbackBridge>,
                vec![runnable_name("digest")],
            )),
            vec![(
                runnable_name("inbox-collector"),
                Arc::clone(&collector) as Arc<dyn HostRunnable>,
            )],
        )
        .expect("compose runnable host")
        .expect("runnables registered");
        assert_eq!(
            host.probe_runnable(&runnable_name("digest")),
            meerkat::RunnableProbe::Registered
        );
        assert_eq!(
            host.probe_runnable(&runnable_name("inbox-collector")),
            meerkat::RunnableProbe::Registered
        );
        let mut invocation = callback_invocation(
            "inbox-collector",
            Some(r#"{"mob_id":"payments","source_id":"inbox"}"#),
        );
        invocation.delivery_idempotency_key = format!(
            "schedule:{}:occurrence:{}",
            invocation.schedule_id, invocation.occurrence_id
        );
        host.run_occurrence(invocation.clone())
            .await
            .expect("the host's runnable completes the occurrence");
        let handed = collector.handed();
        assert_eq!(handed.len(), 1);
        let handed = &handed[0];
        assert_eq!(handed.occurrence_id, invocation.occurrence_id);
        assert_eq!(handed.schedule_id, invocation.schedule_id);
        assert_eq!(
            handed.delivery_idempotency_key, invocation.delivery_idempotency_key,
            "Core's delivery idempotency key reaches the runnable unchanged"
        );
        assert_eq!(handed.runnable, invocation.runnable);
        assert_eq!(handed.trigger_time, invocation.trigger_time);
        assert_eq!(
            handed.params.as_deref().map(RawValue::get),
            invocation.params.as_deref().map(RawValue::get)
        );
        assert!(
            bridge.calls.lock().expect("calls lock").is_empty(),
            "a host runnable's fire never reaches the callback bridge"
        );
        for taken in ["digest", STEWARD_DREAM_RUNNABLE] {
            let refused = gateway_runnable_host_with(
                None,
                Some((
                    Arc::clone(&bridge) as Arc<dyn CallbackBridge>,
                    vec![runnable_name("digest")],
                )),
                vec![(
                    runnable_name(taken),
                    Arc::clone(&collector) as Arc<dyn HostRunnable>,
                )],
            );
            assert!(refused.is_err(), "{taken} refuses the composition");
        }
    }

    /// A source's runner fires on a governed gateway: the schedule host runs
    /// the collector's host runnable with the source it collects and Core's
    /// delivery identity for the occurrence, where the same runtime refuses
    /// a scheduled fire to a member. Collecting is host work, not member
    /// admission.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_declared_source_runner_fires_on_a_governed_gateway() {
        use crate::mob_announcements::AnnouncementStore as _;

        let collector = Arc::new(RecordingRunnable::default());
        let runnables = gateway_runnable_host_with(
            None,
            None,
            vec![(
                runnable_name("inbox-collector"),
                Arc::clone(&collector) as Arc<dyn HostRunnable>,
            )],
        )
        .expect("compose runnable host");
        let governed = HostedScheduleFixture::with_runnables(true, runnables).await;

        let sources = tempfile::tempdir().expect("sources dir");
        let store = crate::mob_announcements::SqliteAnnouncementStore::open(
            sources.path().join("mob_announcements.sqlite3"),
        )
        .expect("announcement store");
        let put: crate::mob_announcements::PutSource = serde_json::from_value(json!({
            "mob_id": "payments",
            "source_id": "inbox",
            "expected_revision": null,
            "new_generation": false,
            "config": {
                "identity": {
                    "provider": "google.gmail",
                    "principal": {
                        "kind": "connector_verified_account",
                        "issuer": "https://accounts.example",
                        "strategy_id": "oauth-code",
                        "subject": "subject-1",
                    },
                    "scope": { "kind": "mailbox_label", "id": "INBOX" },
                },
                "observation": { "baseline": "delta", "deletions": "explicit" },
                "topics": ["mail.received"],
                "audience": [
                    { "kind": "member", "member": { "mob_id": "payments", "identity": "agent:a" } }
                ],
                "routes": [{ "kind": "direct" }],
                "runner": { "host_runnable": "inbox-collector", "every_seconds": 60 },
            },
        }))
        .expect("a source declaration");
        let source = store
            .put_source(
                &crate::mob_announcements::OperationId::new("put-inbox").expect("operation id"),
                put,
                1,
            )
            .await
            .expect("source declared");
        crate::schedule_wiring::ScheduleSourceRunners::new(governed.schedules.clone())
            .reconcile(&source)
            .await
            .expect("the runner's schedule");
        let schedules = governed.schedules.list().await.expect("list schedules");
        assert_eq!(schedules.len(), 1, "{schedules:#?}");
        let schedule_id = schedules[0].schedule_id.clone();

        let deadline = tokio::time::Instant::now() + HOSTED_FIRE_TERMINAL_WAIT;
        let occurrence = loop {
            let completed = governed
                .schedules
                .list_occurrences(&schedule_id)
                .await
                .expect("list occurrences")
                .into_iter()
                .find(|occurrence| occurrence.phase == OccurrencePhase::Completed);
            if let Some(occurrence) = completed {
                break occurrence;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the runner's first occurrence never completed: {:#?}",
                governed.schedules.list_occurrences(&schedule_id).await
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let handed = collector.handed();
        let invocation = handed
            .iter()
            .find(|invocation| invocation.occurrence_id == occurrence.occurrence_id)
            .unwrap_or_else(|| panic!("the collector ran the occurrence: {handed:#?}"));
        assert_eq!(invocation.runnable.as_str(), "inbox-collector");
        assert_eq!(invocation.schedule_id, schedule_id);
        assert_eq!(
            invocation
                .params
                .as_deref()
                .map(|params| serde_json::from_str::<Value>(params.get()).expect("params")),
            Some(json!({ "mob_id": "payments", "source_id": "inbox" }))
        );
        assert_eq!(
            invocation.delivery_idempotency_key,
            meerkat::ScheduleDeliveryIdentity::for_occurrence(&occurrence).idempotency_key,
            "the collector dedupes on Core's delivery identity for the occurrence"
        );
        assert_eq!(
            governed.llm_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "collecting runs no turn"
        );
        governed.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // A candidate launch drives no schedule store
    // -----------------------------------------------------------------------

    /// A gateway-shaped launch over the schedule store in `schedule_state`,
    /// composed the way `rpc_gateway` composes one: its own session and
    /// runtime stores, the gateway's schedule tools attached for `authority`,
    /// a runtime bootstrapped for that authority, and its schedule host spawned
    /// through the gateway composition with `runnables`.
    struct ScheduleLaunch {
        runtime: crate::UnifiedRuntime,
        attached: AttachedScheduleTools,
        host: Option<ScheduleHostHandle>,
        watchdog: tokio::task::JoinHandle<()>,
        _stores: tempfile::TempDir,
    }

    impl ScheduleLaunch {
        async fn boot(
            schedule_state: &Path,
            authority: crate::mob_composition_manifest::CompositionAuthority,
            runnables: Option<Arc<dyn ScheduleRunnableHost>>,
        ) -> Self {
            let stores = tempfile::tempdir().expect("stores dir");
            let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
                meerkat_store::SqliteSessionStore::open(stores.path().join("sessions.db"))
                    .expect("session store"),
            );
            let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
                meerkat_runtime::store::SqliteRuntimeStore::new(
                    stores.path().join("runtime.sqlite"),
                )
                .expect("runtime store"),
            );
            let blob_store: Arc<dyn meerkat_core::BlobStore> =
                Arc::new(meerkat_store::MemoryBlobStore::new());
            let mut builder = FactoryAgentBuilder::new(
                AgentFactory::new(stores.path()).comms(true),
                Config::default(),
            );
            builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
                session_store.clone(),
            )));
            builder.default_blob_store = Some(blob_store.clone());
            let jobs: Arc<dyn meerkat::DetachedJobStore> =
                Arc::new(meerkat::MemoryDetachedJobStore::new());
            builder.default_detached_job_store = Some(Arc::clone(&jobs));
            let delivery = crate::mob_handle_runtime::MobRuntimeDelivery::new(
                Arc::clone(&runtime_store),
                jobs,
            );
            let attached = crate::gateway_composition::attach_gateway_schedule_tools(
                &builder,
                schedule_state,
                crate::gateway_composition::ScheduleFiringRole::for_composition_authority(
                    authority,
                ),
            )
            .expect("schedule tools attach");
            // The attach itself quiesces a candidate's tools, before any member
            // is built against the slot; an executor's stay pending until its
            // host spawns.
            assert_eq!(
                attached.firing_host_binding.is_quiesced_candidate(),
                !authority.speaks_for_composition()
            );
            assert!(!attached.firing_host_binding.is_bound());
            let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
            let machine = Arc::new(
                meerkat_runtime::MeerkatMachine::persistent(
                    Arc::clone(&runtime_store),
                    Arc::clone(&blob_store),
                )
                .expect("runtime machine"),
            );
            let sessions = Arc::new(PersistentSessionService::new(
                builder,
                16,
                session_store,
                runtime_store,
                blob_store,
            ));
            let definition = meerkat_mob::MobDefinition::from_toml(&format!(
                "[mob]\nid = \"schedule-launch-{}\"\n\n[profiles.general]\nmodel = \"gpt-5.5\"\n",
                uuid::Uuid::new_v4().simple()
            ))
            .expect("definition");
            let spec = crate::mob_handle_runtime::MobBootstrapSpec::new(
                definition,
                meerkat_mob::MobStorage::in_memory(),
                sessions.clone(),
            )
            .with_session_runtime_adapter(Arc::clone(&machine))
            .expect("the session runtime owner")
            .with_runtime_delivery(delivery)
            .with_agent_mob_tools(mob_tools_slot)
            .expect("install the agent mob tools")
            .with_composition_authority(authority)
            .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
                allow_ephemeral_sessions: true,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(meerkat_client::TestClient::for_provider(
                    meerkat_core::Provider::OpenAI,
                ))),
            });
            let runtime = crate::UnifiedRuntime::bootstrap(
                spec,
                crate::MetaConfig {
                    modules: vec![],
                    discovery: crate::DiscoverySpec {
                        namespace: "schedule-launch".to_string(),
                        modules: vec![],
                    },
                    pre_spawn: vec![],
                },
                Duration::from_secs(2),
            )
            .await
            .expect("bootstrap");
            let mob_state = crate::gateway_composition::adopt_schedule_mob_targets(
                &runtime,
                &attached.service,
                &attached.mob_target_registry,
            )
            .await;
            let (host, watchdog) = crate::gateway_composition::spawn_gateway_schedule_host(
                &runtime,
                mob_state,
                crate::gateway_composition::GatewayScheduleHostInputs {
                    schedule_service: attached.service.clone(),
                    session_service: sessions,
                    runtime_adapter: machine,
                    schedule_store_path: schedule_state.join(SCHEDULE_STORE_FILE),
                    firing_host_binding: attached.firing_host_binding.clone(),
                    runnable_host: runnables,
                    workgraph_service: None,
                    owner_id: format!("schedule-launch-{}", uuid::Uuid::new_v4().simple()),
                },
            )
            .await;
            Self {
                runtime,
                attached,
                host,
                watchdog,
                _stores: stores,
            }
        }

        async fn shutdown(self) {
            if let Some(host) = self.host {
                host.shutdown().await;
            }
            self.watchdog.abort();
            let _ = self.watchdog.await;
            self.runtime.shutdown().await;
        }
    }

    /// How long a candidate is watched over a due occurrence: many driver
    /// ticks, had it spawned one.
    const CANDIDATE_SCHEDULE_WATCH: Duration = Duration::from_secs(3);

    /// A candidate launch fires no due schedule. Over a durable schedule store
    /// holding an occurrence that is due, the candidate's gateway schedule
    /// host spawns no driver: no executor lease is taken, its runnable is
    /// never handed the occurrence, the occurrence stays pending, and its
    /// schedule tools stay quiesced. The authoritative launch that follows
    /// over the same store fires it exactly once.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_candidate_launch_fires_no_due_schedule_and_the_authoritative_launch_fires_it_once() {
        use crate::mob_composition_manifest::CompositionAuthority;

        let schedule_state = tempfile::tempdir().expect("schedule state dir");
        let probe = Arc::new(RecordingRunnable::default());
        let runnables = || {
            gateway_runnable_host_with(
                None,
                None,
                vec![(
                    runnable_name("candidate-probe"),
                    Arc::clone(&probe) as Arc<dyn HostRunnable>,
                )],
            )
            .expect("compose runnable host")
        };
        // What the candidate boots against: one occurrence due now, late
        // enough to be decided only by a launch that drives the store.
        let schedule_id = {
            let written = ScheduleService::new(Arc::new(
                SqliteScheduleStore::open(schedule_state.path().join(SCHEDULE_STORE_FILE))
                    .expect("open the schedule store"),
            ));
            written
                .create(CreateScheduleRequest {
                    name: Some("candidate probe".to_string()),
                    description: None,
                    trigger: TriggerSpec::Once {
                        due_at_utc: chrono::Utc::now(),
                    },
                    target: TargetBinding::host_runnable(HostRunnableTargetBinding {
                        runnable: runnable_name("candidate-probe"),
                        params: None,
                    }),
                    misfire_policy: meerkat::MisfirePolicy::CatchUpWithin {
                        window_seconds: 600,
                    },
                    overlap_policy: meerkat::OverlapPolicy::default(),
                    missing_target_policy: meerkat::MissingTargetPolicy::default(),
                    labels: BTreeMap::new(),
                    planning_horizon_days: None,
                    planning_horizon_occurrences: None,
                })
                .await
                .expect("create the due schedule")
                .schedule_id
        };

        let candidate = ScheduleLaunch::boot(
            schedule_state.path(),
            CompositionAuthority::NonAuthoritative,
            runnables(),
        )
        .await;
        assert!(
            candidate.host.is_none(),
            "a candidate spawns no schedule firing host"
        );
        assert!(
            candidate
                .attached
                .firing_host_binding
                .is_quiesced_candidate()
        );
        tokio::time::sleep(CANDIDATE_SCHEDULE_WATCH).await;
        assert!(
            probe.handed().is_empty(),
            "a candidate fired the due occurrence: {:#?}",
            probe.handed()
        );
        let occurrences = candidate
            .attached
            .service
            .list_occurrences(&schedule_id)
            .await
            .expect("list occurrences");
        assert!(
            !occurrences.is_empty()
                && occurrences
                    .iter()
                    .all(|occurrence| occurrence.phase == OccurrencePhase::Pending),
            "the due occurrence stays pending for the authoritative launch: {occurrences:#?}"
        );
        assert_eq!(
            observe_schedule_firing_authority(&candidate.attached.service).await,
            ScheduleFiringAuthority::Vacant,
            "a candidate takes no executor lease"
        );
        candidate.shutdown().await;

        let authoritative = ScheduleLaunch::boot(
            schedule_state.path(),
            CompositionAuthority::Authoritative,
            runnables(),
        )
        .await;
        assert!(authoritative.host.is_some());
        assert!(authoritative.attached.firing_host_binding.is_bound());
        let deadline = tokio::time::Instant::now() + HOSTED_FIRE_TERMINAL_WAIT;
        let fired = loop {
            let completed = authoritative
                .attached
                .service
                .list_occurrences(&schedule_id)
                .await
                .expect("list occurrences")
                .into_iter()
                .find(|occurrence| occurrence.phase == OccurrencePhase::Completed);
            if let Some(occurrence) = completed {
                break occurrence;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the authoritative launch never fired the due occurrence: {:#?}",
                authoritative
                    .attached
                    .service
                    .list_occurrences(&schedule_id)
                    .await
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        authoritative.shutdown().await;
        let handed = probe.handed();
        assert_eq!(handed.len(), 1, "fired exactly once: {handed:#?}");
        assert_eq!(handed[0].occurrence_id, fired.occurrence_id);
    }

    /// A candidate launch never fires a source runner, even with mob
    /// announcements off: these launches compose no announcements at all. The
    /// schedule a source's declaration keeps for its runner is due when a
    /// candidate boots over the same schedule store: the candidate's
    /// collector is never handed it, and the occurrence stays pending. The
    /// authoritative launch that follows runs the collector once for that
    /// occurrence, with the source it collects.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_candidate_launch_never_fires_a_source_runner() {
        use crate::mob_announcements::AnnouncementStore as _;
        use crate::mob_composition_manifest::CompositionAuthority;

        let schedule_state = tempfile::tempdir().expect("schedule state dir");
        let collector = Arc::new(RecordingRunnable::default());
        let runnables = || {
            gateway_runnable_host_with(
                None,
                None,
                vec![(
                    runnable_name("inbox-collector"),
                    Arc::clone(&collector) as Arc<dyn HostRunnable>,
                )],
            )
            .expect("compose runnable host")
        };
        // What the candidate boots against: the source declared with a
        // runner, and the runner schedule its declaration made, first due at
        // once. Its lateness window is widened so that only the launch that
        // drives the store decides the occurrence, however long the boots
        // take.
        let sources = tempfile::tempdir().expect("sources dir");
        let store = crate::mob_announcements::SqliteAnnouncementStore::open(
            sources.path().join("mob_announcements.sqlite3"),
        )
        .expect("announcement store");
        let put: crate::mob_announcements::PutSource = serde_json::from_value(json!({
            "mob_id": "payments",
            "source_id": "inbox",
            "expected_revision": null,
            "new_generation": false,
            "config": {
                "identity": {
                    "provider": "google.gmail",
                    "principal": {
                        "kind": "connector_verified_account",
                        "issuer": "https://accounts.example",
                        "strategy_id": "oauth-code",
                        "subject": "subject-1",
                    },
                    "scope": { "kind": "mailbox_label", "id": "INBOX" },
                },
                "observation": { "baseline": "delta", "deletions": "explicit" },
                "topics": ["mail.received"],
                "audience": [
                    { "kind": "member", "member": { "mob_id": "payments", "identity": "agent:a" } }
                ],
                "routes": [{ "kind": "direct" }],
                "runner": { "host_runnable": "inbox-collector", "every_seconds": 60 },
            },
        }))
        .expect("a source declaration");
        let source = store
            .put_source(
                &crate::mob_announcements::OperationId::new("put-inbox").expect("operation id"),
                put,
                1,
            )
            .await
            .expect("source declared");
        let schedule_id = {
            let written = ScheduleService::new(Arc::new(
                SqliteScheduleStore::open(schedule_state.path().join(SCHEDULE_STORE_FILE))
                    .expect("open the schedule store"),
            ));
            ScheduleSourceRunners::new(written.clone())
                .reconcile(&source)
                .await
                .expect("the runner's schedule");
            let schedules = written.list().await.expect("list schedules");
            assert_eq!(schedules.len(), 1, "{schedules:#?}");
            written
                .update(
                    &schedules[0].schedule_id,
                    UpdateScheduleRequest {
                        misfire_policy: Some(meerkat::MisfirePolicy::CatchUpWithin {
                            window_seconds: 600,
                        }),
                        ..Default::default()
                    },
                )
                .await
                .expect("widen the lateness window");
            schedules[0].schedule_id.clone()
        };

        let candidate = ScheduleLaunch::boot(
            schedule_state.path(),
            CompositionAuthority::NonAuthoritative,
            runnables(),
        )
        .await;
        assert!(candidate.host.is_none());
        tokio::time::sleep(CANDIDATE_SCHEDULE_WATCH).await;
        assert!(
            collector.handed().is_empty(),
            "a candidate ran the source's collector: {:#?}",
            collector.handed()
        );
        let occurrences = candidate
            .attached
            .service
            .list_occurrences(&schedule_id)
            .await
            .expect("list occurrences");
        // Nothing was claimed, run or misfired: the due occurrence waits (an
        // earlier one the widening replanned reads superseded).
        assert!(
            occurrences
                .iter()
                .any(|occurrence| occurrence.phase == OccurrencePhase::Pending)
                && occurrences.iter().all(|occurrence| matches!(
                    occurrence.phase,
                    OccurrencePhase::Pending | OccurrencePhase::Superseded
                )),
            "the runner's occurrence stays pending for the authoritative launch: \
             {occurrences:#?}"
        );
        candidate.shutdown().await;

        let authoritative = ScheduleLaunch::boot(
            schedule_state.path(),
            CompositionAuthority::Authoritative,
            runnables(),
        )
        .await;
        let deadline = tokio::time::Instant::now() + HOSTED_FIRE_TERMINAL_WAIT;
        let fired = loop {
            let completed = authoritative
                .attached
                .service
                .list_occurrences(&schedule_id)
                .await
                .expect("list occurrences")
                .into_iter()
                .find(|occurrence| occurrence.phase == OccurrencePhase::Completed);
            if let Some(occurrence) = completed {
                break occurrence;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the authoritative launch never ran the collector: {:#?}",
                authoritative
                    .attached
                    .service
                    .list_occurrences(&schedule_id)
                    .await
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        authoritative.shutdown().await;
        // The runner fires every minute: only the occurrence the candidate
        // left pending is judged.
        let handed = collector
            .handed()
            .into_iter()
            .filter(|invocation| invocation.occurrence_id == fired.occurrence_id)
            .collect::<Vec<_>>();
        assert_eq!(handed.len(), 1, "the collector ran once: {handed:#?}");
        assert_eq!(
            handed[0]
                .params
                .as_deref()
                .map(|params| serde_json::from_str::<Value>(params.get()).expect("params")),
            Some(json!({ "mob_id": "payments", "source_id": "inbox" }))
        );
    }
}
