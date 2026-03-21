// @generated — protocol helpers for `comms_drain_spawn`
// Producer: CommsDrainLifecycleMachine, Effect: SpawnDrainTask
// Closure policy: AckRequired
// Liveness: eventual feedback under task-scheduling fairness
//
// This module enforces the handoff protocol between the CommsDrainLifecycle
// authority (producer) and the session-service host (owner). Every
// `SpawnDrainTask` effect creates an obligation that must be closed by
// submitting `TaskSpawned` or `TaskExited` feedback.

use crate::comms_drain_lifecycle_authority::{
    CommsDrainLifecycleAuthority, CommsDrainLifecycleEffect, CommsDrainLifecycleError,
    CommsDrainLifecycleInput, CommsDrainLifecycleMutator, CommsDrainMode, DrainExitReason,
};

/// Obligation token for the `comms_drain_spawn` protocol.
///
/// Created when `SpawnDrainTask` is emitted; consumed when the owner submits
/// `TaskSpawned` or `TaskExited` feedback. Move semantics enforce that every
/// spawn obligation is closed exactly once.
#[derive(Debug)]
pub struct CommsDrainSpawnObligation {
    pub mode: CommsDrainMode,
}

/// Outcome of executing the spawn protocol through the authority.
#[derive(Debug)]
pub struct SpawnExecutionResult {
    /// All effects emitted by the authority transition (including the spawn itself).
    pub effects: Vec<CommsDrainLifecycleEffect>,
    /// The obligation token that must be closed via feedback.
    pub obligation: CommsDrainSpawnObligation,
}

/// Execute `EnsureRunning` through the authority, returning an obligation token.
///
/// The caller must eventually close the obligation by calling either
/// [`submit_task_spawned`] or [`submit_task_exited`].
pub fn execute_ensure_running(
    authority: &mut CommsDrainLifecycleAuthority,
    mode: CommsDrainMode,
) -> Result<SpawnExecutionResult, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::EnsureRunning { mode })?;

    // The transition must emit SpawnDrainTask — extract the mode for the obligation.
    let spawn_mode = transition
        .effects
        .iter()
        .find_map(|e| match e {
            CommsDrainLifecycleEffect::SpawnDrainTask { mode } => Some(*mode),
            _ => None,
        })
        .unwrap_or(mode);

    Ok(SpawnExecutionResult {
        effects: transition.effects,
        obligation: CommsDrainSpawnObligation { mode: spawn_mode },
    })
}

/// Submit `TaskSpawned` feedback, consuming the obligation token.
///
/// Called when the drain task has successfully started.
pub fn submit_task_spawned(
    authority: &mut CommsDrainLifecycleAuthority,
    _obligation: CommsDrainSpawnObligation,
) -> Result<Vec<CommsDrainLifecycleEffect>, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::TaskSpawned)?;
    Ok(transition.effects)
}

/// Submit `TaskExited` feedback, consuming the obligation token.
///
/// Called when the drain task exits before `TaskSpawned` was submitted
/// (i.e., from Starting phase). Closes the spawn obligation.
pub fn submit_task_exited(
    authority: &mut CommsDrainLifecycleAuthority,
    _obligation: CommsDrainSpawnObligation,
    reason: DrainExitReason,
) -> Result<Vec<CommsDrainLifecycleEffect>, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::TaskExited { reason })?;
    Ok(transition.effects)
}

/// Notify that a running drain task has exited.
///
/// Unlike [`submit_task_exited`], this does not require an obligation token
/// because the spawn obligation was already closed by [`submit_task_spawned`].
/// This is the normal exit path: the task was spawned, acknowledged, ran,
/// and then exited.
pub fn notify_running_task_exited(
    authority: &mut CommsDrainLifecycleAuthority,
    reason: DrainExitReason,
) -> Result<Vec<CommsDrainLifecycleEffect>, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::TaskExited { reason })?;
    Ok(transition.effects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comms_drain_lifecycle_authority::CommsDrainPhase;

    #[test]
    fn spawn_and_task_spawned_reaches_running() {
        let mut auth = CommsDrainLifecycleAuthority::new();
        assert_eq!(auth.phase(), CommsDrainPhase::Inactive);

        let result = execute_ensure_running(&mut auth, CommsDrainMode::PersistentHost)
            .expect("ensure running");
        assert_eq!(auth.phase(), CommsDrainPhase::Starting);
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, CommsDrainLifecycleEffect::SpawnDrainTask { .. }))
        );
        assert!(result.effects.iter().any(|e| matches!(
            e,
            CommsDrainLifecycleEffect::SetTurnBoundaryDrainSuppressed { active: true }
        )));
        assert_eq!(result.obligation.mode, CommsDrainMode::PersistentHost);

        let effects = submit_task_spawned(&mut auth, result.obligation).expect("task spawned");
        assert_eq!(auth.phase(), CommsDrainPhase::Running);
        assert!(effects.is_empty());
        assert!(auth.suppresses_turn_boundary_drain());
    }

    #[test]
    fn spawn_task_exited_failed_reaches_exited_respawnable() {
        let mut auth = CommsDrainLifecycleAuthority::new();

        let result = execute_ensure_running(&mut auth, CommsDrainMode::PersistentHost)
            .expect("ensure running");
        assert_eq!(auth.phase(), CommsDrainPhase::Starting);

        // Task exits with failure before TaskSpawned — obligation closed by submit_task_exited
        let effects = submit_task_exited(&mut auth, result.obligation, DrainExitReason::Failed)
            .expect("task exited");
        assert_eq!(auth.phase(), CommsDrainPhase::ExitedRespawnable);
        assert!(!auth.suppresses_turn_boundary_drain());
        assert!(effects.iter().any(|e| matches!(
            e,
            CommsDrainLifecycleEffect::SetTurnBoundaryDrainSuppressed { active: false }
        )));
    }

    #[test]
    fn respawn_after_failed_exit() {
        let mut auth = CommsDrainLifecycleAuthority::new();

        // Initial spawn + failure
        let result = execute_ensure_running(&mut auth, CommsDrainMode::PersistentHost)
            .expect("first ensure running");
        submit_task_exited(&mut auth, result.obligation, DrainExitReason::Failed)
            .expect("first task exited");
        assert_eq!(auth.phase(), CommsDrainPhase::ExitedRespawnable);

        // Respawn from ExitedRespawnable
        let result = execute_ensure_running(&mut auth, CommsDrainMode::PersistentHost)
            .expect("respawn ensure running");
        assert_eq!(auth.phase(), CommsDrainPhase::Starting);
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, CommsDrainLifecycleEffect::SpawnDrainTask { .. }))
        );

        let _effects =
            submit_task_spawned(&mut auth, result.obligation).expect("respawn task spawned");
        assert_eq!(auth.phase(), CommsDrainPhase::Running);
        assert!(auth.suppresses_turn_boundary_drain());
    }

    #[test]
    fn running_task_exit_dismissed_reaches_stopped() {
        let mut auth = CommsDrainLifecycleAuthority::new();

        // Spawn + ack
        let result = execute_ensure_running(&mut auth, CommsDrainMode::PersistentHost)
            .expect("ensure running");
        submit_task_spawned(&mut auth, result.obligation).expect("task spawned");
        assert_eq!(auth.phase(), CommsDrainPhase::Running);

        // Running task exits (dismissed) — no obligation needed
        let effects = notify_running_task_exited(&mut auth, DrainExitReason::Dismissed)
            .expect("task exited dismissed");
        assert_eq!(auth.phase(), CommsDrainPhase::Stopped);
        assert!(!auth.suppresses_turn_boundary_drain());
        assert!(effects.iter().any(|e| matches!(
            e,
            CommsDrainLifecycleEffect::SetTurnBoundaryDrainSuppressed { active: false }
        )));
    }

    #[test]
    fn running_task_exit_failed_persistent_reaches_exited_respawnable() {
        let mut auth = CommsDrainLifecycleAuthority::new();

        // Spawn + ack
        let result = execute_ensure_running(&mut auth, CommsDrainMode::PersistentHost)
            .expect("ensure running");
        submit_task_spawned(&mut auth, result.obligation).expect("task spawned");
        assert_eq!(auth.phase(), CommsDrainPhase::Running);

        // Running task fails — persistent host mode gets ExitedRespawnable
        let effects = notify_running_task_exited(&mut auth, DrainExitReason::Failed)
            .expect("task exited failed");
        assert_eq!(auth.phase(), CommsDrainPhase::ExitedRespawnable);
        assert!(!auth.suppresses_turn_boundary_drain());
        assert!(effects.iter().any(|e| matches!(
            e,
            CommsDrainLifecycleEffect::SetTurnBoundaryDrainSuppressed { active: false }
        )));
    }
}
