// @generated — protocol helpers for `comms_drain_spawn`
// Composition: comms_drain_lifecycle, Producer: comms_drain, Effect: SpawnDrainTask
// Closure policy: AckRequired
// Liveness: eventual feedback under task-scheduling fairness

use crate::comms_drain_lifecycle_authority::{
    CommsDrainLifecycleAuthority, CommsDrainLifecycleEffect, CommsDrainLifecycleError,
    CommsDrainLifecycleInput, CommsDrainLifecycleMutator, CommsDrainMode, DrainExitReason,
};

#[derive(Debug, Clone)]
pub struct CommsDrainSpawnObligation {
    pub mode: CommsDrainMode,
}

#[derive(Debug)]
pub struct CommsDrainSpawnExecutionResult {
    pub effects: Vec<CommsDrainLifecycleEffect>,
    pub obligation: CommsDrainSpawnObligation,
}

pub fn execute_ensure_running(
    authority: &mut CommsDrainLifecycleAuthority,
    mode: CommsDrainMode,
) -> Result<CommsDrainSpawnExecutionResult, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::EnsureRunning { mode: mode })?;
    let obligation = transition.effects.iter().find_map(|effect| match effect {
        CommsDrainLifecycleEffect::SpawnDrainTask { mode } => {
            Some(CommsDrainSpawnObligation { mode: mode.clone() })
        }
        _ => None,
    });
    Ok(CommsDrainSpawnExecutionResult {
        effects: transition.effects,
        obligation: obligation.expect("protocol effect `SpawnDrainTask` must be emitted"),
    })
}

pub fn submit_task_spawned(
    authority: &mut CommsDrainLifecycleAuthority,
    obligation: CommsDrainSpawnObligation,
) -> Result<Vec<CommsDrainLifecycleEffect>, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::TaskSpawned)?;
    Ok(transition.effects)
}

pub fn notify_task_spawned(
    authority: &mut CommsDrainLifecycleAuthority,
) -> Result<Vec<CommsDrainLifecycleEffect>, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::TaskSpawned)?;
    Ok(transition.effects)
}

pub fn submit_task_exited(
    authority: &mut CommsDrainLifecycleAuthority,
    obligation: CommsDrainSpawnObligation,
    reason: DrainExitReason,
) -> Result<Vec<CommsDrainLifecycleEffect>, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::TaskExited { reason: reason })?;
    Ok(transition.effects)
}

pub fn notify_task_exited(
    authority: &mut CommsDrainLifecycleAuthority,
    reason: DrainExitReason,
) -> Result<Vec<CommsDrainLifecycleEffect>, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::TaskExited { reason: reason })?;
    Ok(transition.effects)
}
