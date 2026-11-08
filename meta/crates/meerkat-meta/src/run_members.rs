//! The members a flow run reached: the members its role targets bind, and
//! every target of the mob's per-target dispatch records of the run, the
//! `StepDispatched`, `StepTargetCompleted` and `StepTargetFailed` events of
//! its event log (the flow engine appends one before every dispatch
//! attempt). The run's step ledger names no target: every entry of it
//! carries the flow's system provenance.

use std::collections::{BTreeMap, BTreeSet};

use meerkat_mob::{MobError, MobEventKind, MobHandle, MobRun};

/// Events read from the log per poll.
const PAGE: usize = 512;

/// The targets each run of one mob's flows dispatched to, read from the
/// mob's event log as it grows: a read first takes the events past the last
/// one read before. Shared by every clone of the runtime that holds it.
#[derive(Default)]
pub(crate) struct RunMembersIndex {
    state: tokio::sync::Mutex<RunTargets>,
}

impl std::fmt::Debug for RunMembersIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunMembersIndex").finish_non_exhaustive()
    }
}

/// The dispatch targets of each run read so far, and the cursor of the last
/// event read.
#[derive(Default)]
struct RunTargets {
    cursor: u64,
    by_run: BTreeMap<String, BTreeSet<String>>,
}

impl RunTargets {
    fn record(&mut self, kind: MobEventKind) {
        let (run_id, target) = match kind {
            MobEventKind::StepDispatched { run_id, target, .. }
            | MobEventKind::StepTargetCompleted { run_id, target, .. }
            | MobEventKind::StepTargetFailed { run_id, target, .. } => (run_id, target),
            _ => return,
        };
        self.by_run
            .entry(run_id.to_string())
            .or_default()
            .insert(target.identity.to_string());
    }
}

/// The index read up to the newest event of the log when it was taken.
pub(crate) struct RunMembers<'a> {
    targets: tokio::sync::MutexGuard<'a, RunTargets>,
}

impl RunMembers<'_> {
    /// Every member `run` reached (see the module docs).
    pub(crate) fn of(&self, run: &MobRun) -> Vec<String> {
        let mut members: BTreeSet<String> = run
            .role_targets
            .iter()
            .flat_map(|targets| {
                targets
                    .bound_roles()
                    .flat_map(|(_, members)| members.iter())
            })
            .map(ToString::to_string)
            .collect();
        if let Some(targets) = self.targets.by_run.get(&run.run_id.to_string()) {
            members.extend(targets.iter().cloned());
        }
        members.into_iter().collect()
    }
}

impl RunMembersIndex {
    /// The index, read first up to the newest event of `handle`'s mob's
    /// event log. A log that cannot be read is an error: who the runs
    /// reached is then unknown.
    pub(crate) async fn read(&self, handle: &MobHandle) -> Result<RunMembers<'_>, MobError> {
        let mut targets = self.state.lock().await;
        loop {
            let events = handle.poll_events(targets.cursor, PAGE).await?;
            let Some(last) = events.last().map(|event| event.cursor) else {
                break;
            };
            let full = events.len() >= PAGE;
            for event in events {
                targets.record(event.kind);
            }
            targets.cursor = last;
            if !full {
                break;
            }
        }
        Ok(RunMembers { targets })
    }
}

/// Every member `run` reached, read through `runtime`'s index when there is
/// a runtime, else through an index of its own over `handle`'s mob.
pub(crate) async fn run_members(
    runtime: Option<&crate::MobRuntime>,
    handle: &MobHandle,
    run: &MobRun,
) -> Result<Vec<String>, MobError> {
    match runtime {
        Some(runtime) => Ok(runtime.run_members().read(handle).await?.of(run)),
        None => Ok(RunMembersIndex::default().read(handle).await?.of(run)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_targets_of_every_per_target_record_name_the_run_s_members() {
        let mut targets = RunTargets::default();
        let run_id = meerkat_mob::RunId::new();
        let other = meerkat_mob::RunId::new();
        let target = |member: &str| {
            meerkat_mob::AgentRuntimeId::initial(meerkat_mob::AgentIdentity::from(member))
        };
        let step = meerkat_mob::StepId::from("work");
        targets.record(MobEventKind::StepDispatched {
            run_id: run_id.clone(),
            step_id: step.clone(),
            target: target("alpha"),
        });
        targets.record(MobEventKind::StepTargetCompleted {
            run_id: run_id.clone(),
            step_id: step.clone(),
            target: target("beta"),
            output: None,
            remote_turn_obligation: None,
        });
        targets.record(MobEventKind::StepTargetFailed {
            run_id: run_id.clone(),
            step_id: step.clone(),
            target: target("gamma"),
            reason: "failed".to_string(),
            remote_turn_obligation: None,
            error_report: None,
            error: None,
        });
        targets.record(MobEventKind::StepDispatched {
            run_id: other.clone(),
            step_id: step,
            target: target("delta"),
        });
        assert_eq!(
            targets.by_run.get(&run_id.to_string()),
            Some(&BTreeSet::from([
                "alpha".to_string(),
                "beta".to_string(),
                "gamma".to_string(),
            ]))
        );
        assert_eq!(
            targets.by_run.get(&other.to_string()),
            Some(&BTreeSet::from(["delta".to_string()]))
        );
    }
}
