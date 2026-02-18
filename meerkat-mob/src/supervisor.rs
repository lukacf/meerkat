//! Mob supervisor: monitors active runs for stuck steps.
//!
//! [`MobSupervisor`] detects dispatch timeouts (steps dispatched but no
//! response within the configured timeout) and emits
//! [`MobEventKind::SupervisorEscalation`] events. Optionally notifies a
//! designated role via PeerRequest.
//!
//! In v1, the supervisor runs as a synchronous check during flow execution
//! rather than as a separate long-lived background task.

use chrono::{DateTime, Utc};

use crate::event::{MobEvent, MobEventKind, RetentionCategory};
use crate::run::{MobRun, StepEntryStatus};
use crate::spec::SupervisorSpec;

/// A detected escalation from the supervisor.
#[derive(Debug, Clone)]
pub struct Escalation {
    /// The step that is stuck.
    pub step_id: String,
    /// The target meerkat that has not responded.
    pub target_meerkat_id: String,
    /// How long the dispatch has been outstanding, in milliseconds.
    pub elapsed_ms: u64,
    /// Human-readable description.
    pub description: String,
}

/// Monitors active runs for stuck steps.
///
/// Call [`check_run`] during flow execution to detect dispatch timeouts.
/// The supervisor does not perform remediation; it only reports escalations.
pub struct MobSupervisor<'a> {
    spec: &'a SupervisorSpec,
}

impl<'a> MobSupervisor<'a> {
    /// Create a new supervisor with the given spec.
    pub fn new(spec: &'a SupervisorSpec) -> Self {
        Self { spec }
    }

    /// Check a run for stuck steps (dispatched but not completed within timeout).
    ///
    /// Returns a list of escalations for steps that have exceeded
    /// `dispatch_timeout_ms`. Each escalation corresponds to a single
    /// target meerkat that has not responded.
    pub fn check_run(&self, run: &MobRun, now: DateTime<Utc>) -> Vec<Escalation> {
        if !self.spec.enabled {
            return Vec::new();
        }

        let timeout_ms = self.spec.dispatch_timeout_ms;
        let mut escalations = Vec::new();

        for entry in &run.step_ledger {
            if entry.status != StepEntryStatus::Dispatched {
                continue;
            }

            if let Some(dispatched_at) = entry.dispatched_at {
                let elapsed = now
                    .signed_duration_since(dispatched_at)
                    .num_milliseconds();
                if elapsed < 0 {
                    continue;
                }
                let elapsed_ms = elapsed as u64;
                if elapsed_ms >= timeout_ms {
                    escalations.push(Escalation {
                        step_id: entry.step_id.clone(),
                        target_meerkat_id: entry.target_meerkat_id.clone(),
                        elapsed_ms,
                        description: format!(
                            "step '{}' target '{}' has been dispatched for {}ms \
                             (timeout: {}ms)",
                            entry.step_id,
                            entry.target_meerkat_id,
                            elapsed_ms,
                            timeout_ms,
                        ),
                    });
                }
            }
        }

        escalations
    }

    /// Convert escalations into MobEvents for emission.
    pub fn escalation_events(
        &self,
        escalations: &[Escalation],
        mob_id: &str,
        run_id: &str,
        flow_id: Option<&str>,
    ) -> Vec<MobEvent> {
        escalations
            .iter()
            .map(|esc| MobEvent {
                cursor: 0,
                timestamp: Utc::now(),
                mob_id: mob_id.to_string(),
                run_id: Some(run_id.to_string()),
                flow_id: flow_id.map(|s| s.to_string()),
                step_id: Some(esc.step_id.clone()),
                retention: RetentionCategory::Ops,
                kind: MobEventKind::SupervisorEscalation {
                    description: esc.description.clone(),
                },
            })
            .collect()
    }

    /// The notify role from the supervisor spec (if any).
    pub fn notify_role(&self) -> Option<&str> {
        self.spec.notify_role.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{MobRun, RunStatus, StepEntryStatus, StepLedgerEntry};
    use crate::spec::SupervisorSpec;
    use chrono::{Duration, Utc};

    fn make_supervisor_spec(timeout_ms: u64) -> SupervisorSpec {
        SupervisorSpec {
            enabled: true,
            dispatch_timeout_ms: timeout_ms,
            notify_role: Some("escalation_handler".to_string()),
        }
    }

    fn make_run_with_dispatched_step(
        dispatched_at: DateTime<Utc>,
    ) -> MobRun {
        let now = Utc::now();
        MobRun {
            run_id: "run-sup-1".to_string(),
            mob_id: "test_mob".to_string(),
            flow_id: "test_flow".to_string(),
            spec_revision: 1,
            status: RunStatus::Running,
            step_ledger: vec![StepLedgerEntry {
                step_id: "slow_step".to_string(),
                target_meerkat_id: "meerkat-slow".to_string(),
                status: StepEntryStatus::Dispatched,
                attempt: 1,
                dispatched_at: Some(dispatched_at),
                completed_at: None,
                result: None,
                error: None,
            }],
            failure_ledger: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn test_supervisor_detects_timeout() {
        let spec = make_supervisor_spec(1000); // 1 second timeout
        let supervisor = MobSupervisor::new(&spec);

        // Dispatch was 2 seconds ago
        let dispatched_at = Utc::now() - Duration::milliseconds(2000);
        let run = make_run_with_dispatched_step(dispatched_at);

        let now = Utc::now();
        let escalations = supervisor.check_run(&run, now);

        assert_eq!(
            escalations.len(),
            1,
            "Should detect one timeout escalation"
        );
        assert_eq!(escalations[0].step_id, "slow_step");
        assert_eq!(escalations[0].target_meerkat_id, "meerkat-slow");
        assert!(
            escalations[0].elapsed_ms >= 1000,
            "Elapsed should be >= timeout"
        );
        assert!(
            escalations[0].description.contains("slow_step"),
            "Description should mention the step"
        );
    }

    #[test]
    fn test_supervisor_no_timeout_within_threshold() {
        let spec = make_supervisor_spec(5000); // 5 second timeout
        let supervisor = MobSupervisor::new(&spec);

        // Dispatch was 100ms ago (well within timeout)
        let dispatched_at = Utc::now() - Duration::milliseconds(100);
        let run = make_run_with_dispatched_step(dispatched_at);

        let now = Utc::now();
        let escalations = supervisor.check_run(&run, now);

        assert!(
            escalations.is_empty(),
            "Should not detect timeout within threshold"
        );
    }

    #[test]
    fn test_supervisor_disabled() {
        let spec = SupervisorSpec {
            enabled: false,
            dispatch_timeout_ms: 1,
            notify_role: None,
        };
        let supervisor = MobSupervisor::new(&spec);

        let dispatched_at = Utc::now() - Duration::milliseconds(10_000);
        let run = make_run_with_dispatched_step(dispatched_at);

        let now = Utc::now();
        let escalations = supervisor.check_run(&run, now);

        assert!(
            escalations.is_empty(),
            "Disabled supervisor should not detect anything"
        );
    }

    #[test]
    fn test_supervisor_ignores_completed_steps() {
        let spec = make_supervisor_spec(1000);
        let supervisor = MobSupervisor::new(&spec);

        let now = Utc::now();
        let run = MobRun {
            run_id: "run-sup-2".to_string(),
            mob_id: "test_mob".to_string(),
            flow_id: "test_flow".to_string(),
            spec_revision: 1,
            status: RunStatus::Running,
            step_ledger: vec![StepLedgerEntry {
                step_id: "done_step".to_string(),
                target_meerkat_id: "meerkat-fast".to_string(),
                status: StepEntryStatus::Completed,
                attempt: 1,
                dispatched_at: Some(now - Duration::milliseconds(5000)),
                completed_at: Some(now - Duration::milliseconds(4000)),
                result: Some(serde_json::json!({"ok": true})),
                error: None,
            }],
            failure_ledger: Vec::new(),
            created_at: now,
            updated_at: now,
        };

        let escalations = supervisor.check_run(&run, now);
        assert!(
            escalations.is_empty(),
            "Completed steps should not trigger escalation"
        );
    }

    #[test]
    fn test_supervisor_escalation_events() {
        let spec = make_supervisor_spec(1000);
        let supervisor = MobSupervisor::new(&spec);

        let dispatched_at = Utc::now() - Duration::milliseconds(2000);
        let run = make_run_with_dispatched_step(dispatched_at);

        let now = Utc::now();
        let escalations = supervisor.check_run(&run, now);
        let events = supervisor.escalation_events(
            &escalations,
            "test_mob",
            "run-sup-1",
            Some("test_flow"),
        );

        assert_eq!(events.len(), 1);
        match &events[0].kind {
            MobEventKind::SupervisorEscalation { description } => {
                assert!(description.contains("slow_step"));
                assert!(description.contains("meerkat-slow"));
            }
            other => panic!("Expected SupervisorEscalation, got: {other:?}"),
        }
    }

    #[test]
    fn test_supervisor_notify_role() {
        let spec = make_supervisor_spec(1000);
        let supervisor = MobSupervisor::new(&spec);
        assert_eq!(
            supervisor.notify_role(),
            Some("escalation_handler")
        );

        let no_notify_spec = SupervisorSpec {
            enabled: true,
            dispatch_timeout_ms: 1000,
            notify_role: None,
        };
        let no_notify_supervisor = MobSupervisor::new(&no_notify_spec);
        assert_eq!(no_notify_supervisor.notify_role(), None);
    }
}
