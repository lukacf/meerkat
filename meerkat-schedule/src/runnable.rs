//! Host-runnable schedule targets.
//!
//! Vocabulary for schedules whose target is a host-registered callback
//! instead of a session or a mob. The host registers named runnables at
//! startup (via [`HostRunnableRegistry`] or its own
//! [`ScheduleRunnableHost`] implementation); the schedule driver reaches
//! them exclusively through the target probe/delivery seams, so occurrences
//! flow through the full normal lifecycle (probe → dispatch → completion →
//! receipts) with zero machine churn.

use crate::types::{HostRunnableName, OccurrenceId, ScheduleId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::value::RawValue;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;
use std::sync::Arc;

/// Outcome of a host-runnable probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnableProbe {
    /// A runnable with the requested name is registered on this host.
    Registered,
    /// No runnable with the requested name is registered on this host.
    Unknown,
}

/// Invocation payload handed to a host runnable for one occurrence attempt.
#[derive(Debug, Clone)]
pub struct HostRunnableInvocation {
    /// Occurrence being delivered.
    pub occurrence_id: OccurrenceId,
    /// Schedule the occurrence belongs to.
    pub schedule_id: ScheduleId,
    /// Name of the runnable being invoked.
    pub runnable: HostRunnableName,
    /// The occurrence's scheduled trigger time (UTC).
    pub trigger_time: DateTime<Utc>,
    /// Wire-opaque host payload from the target binding, in the binding's
    /// canonical JSON text form.
    pub params: Option<Box<RawValue>>,
}

/// Successful completion of a host-runnable invocation.
///
/// Deliberately carries no payload today: the occurrence authority records
/// completion as a typed `Complete` transition and consumes no success
/// detail (parity with mob flow completions). `#[non_exhaustive]` so future
/// completion facts can be added without breaking hosts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct HostRunnableOutcome {}

impl HostRunnableOutcome {
    /// Typed success witness for a completed invocation.
    pub fn completed() -> Self {
        Self {}
    }
}

/// Typed failure reported by a host-runnable invocation.
#[derive(Debug, thiserror::Error)]
pub enum HostRunnableError {
    /// The runnable executed and reported a failure.
    #[error("host runnable invocation failed: {detail}")]
    Failed { detail: String },
}

/// Typed error for [`HostRunnableRegistry::register`].
#[derive(Debug, thiserror::Error)]
pub enum HostRunnableRegistryError {
    /// A runnable is already registered under the requested name.
    #[error("host runnable '{name}' is already registered")]
    DuplicateRunnable { name: HostRunnableName },
}

/// A single host-implemented runnable.
///
/// Hosts implement this per named runnable and register the implementations
/// with a [`HostRunnableRegistry`] at startup.
#[async_trait]
pub trait HostRunnable: Send + Sync {
    /// Execute one occurrence attempt.
    async fn run(
        &self,
        invocation: HostRunnableInvocation,
    ) -> Result<HostRunnableOutcome, HostRunnableError>;
}

/// Host seam for schedules targeting host-registered runnables.
///
/// `probe_runnable` is a synchronous registry lookup consumed by the target
/// probe; `run_occurrence` performs the in-process invocation and is awaited
/// by the delivery completion future, so the occurrence lifecycle
/// (dispatch → awaiting-completion → terminal + receipts) is identical to
/// session and mob targets.
#[async_trait]
pub trait ScheduleRunnableHost: Send + Sync {
    /// Report whether `runnable` is registered with this host.
    fn probe_runnable(&self, runnable: &HostRunnableName) -> RunnableProbe;

    /// Invoke the named runnable for one occurrence attempt.
    async fn run_occurrence(
        &self,
        invocation: HostRunnableInvocation,
    ) -> Result<HostRunnableOutcome, HostRunnableError>;
}

/// Startup-populated name → runnable map; itself a [`ScheduleRunnableHost`].
///
/// Registration is register-at-startup only: [`register`] takes `&mut self`,
/// so once the registry is shared as `Arc<dyn ScheduleRunnableHost>` the set
/// of runnables is immutable.
///
/// [`register`]: HostRunnableRegistry::register
#[derive(Default)]
pub struct HostRunnableRegistry {
    runnables: BTreeMap<HostRunnableName, Arc<dyn HostRunnable>>,
}

impl HostRunnableRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `runnable` under `name`.
    ///
    /// # Errors
    /// Returns [`HostRunnableRegistryError::DuplicateRunnable`] when a
    /// runnable is already registered under `name`.
    pub fn register(
        &mut self,
        name: HostRunnableName,
        runnable: Arc<dyn HostRunnable>,
    ) -> Result<(), HostRunnableRegistryError> {
        match self.runnables.entry(name) {
            Entry::Occupied(entry) => Err(HostRunnableRegistryError::DuplicateRunnable {
                name: entry.key().clone(),
            }),
            Entry::Vacant(entry) => {
                entry.insert(runnable);
                Ok(())
            }
        }
    }
}

impl fmt::Debug for HostRunnableRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostRunnableRegistry")
            .field(
                "runnables",
                &self.runnables.keys().collect::<Vec<&HostRunnableName>>(),
            )
            .finish()
    }
}

#[async_trait]
impl ScheduleRunnableHost for HostRunnableRegistry {
    fn probe_runnable(&self, runnable: &HostRunnableName) -> RunnableProbe {
        if self.runnables.contains_key(runnable) {
            RunnableProbe::Registered
        } else {
            RunnableProbe::Unknown
        }
    }

    async fn run_occurrence(
        &self,
        invocation: HostRunnableInvocation,
    ) -> Result<HostRunnableOutcome, HostRunnableError> {
        // Defensive: the registry is immutable once shared, so a delivery
        // that probed `Registered` cannot miss here. A miss means the caller
        // skipped the probe; report it as an ordinary invocation failure.
        let Some(runnable) = self.runnables.get(&invocation.runnable) else {
            return Err(HostRunnableError::Failed {
                detail: format!("host runnable '{}' is not registered", invocation.runnable),
            });
        };
        runnable.run(invocation).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct RecordingRunnable {
        invocations: Arc<Mutex<Vec<HostRunnableInvocation>>>,
        result: Result<HostRunnableOutcome, String>,
    }

    #[async_trait]
    impl HostRunnable for RecordingRunnable {
        async fn run(
            &self,
            invocation: HostRunnableInvocation,
        ) -> Result<HostRunnableOutcome, HostRunnableError> {
            self.invocations
                .lock()
                .expect("invocation lock")
                .push(invocation);
            self.result
                .clone()
                .map_err(|detail| HostRunnableError::Failed { detail })
        }
    }

    fn name(value: &str) -> HostRunnableName {
        HostRunnableName::parse(value).expect("valid runnable name")
    }

    fn sample_invocation(runnable: &str) -> HostRunnableInvocation {
        HostRunnableInvocation {
            occurrence_id: OccurrenceId::new(),
            schedule_id: ScheduleId::new(),
            runnable: name(runnable),
            trigger_time: Utc::now(),
            params: Some(
                RawValue::from_string(r#"{"depth":3}"#.to_string()).expect("valid raw params"),
            ),
        }
    }

    #[test]
    fn registry_probe_reports_registered_and_unknown() {
        let mut registry = HostRunnableRegistry::new();
        registry
            .register(
                name("nightly-report"),
                Arc::new(RecordingRunnable {
                    invocations: Arc::new(Mutex::new(Vec::new())),
                    result: Ok(HostRunnableOutcome::completed()),
                }),
            )
            .expect("first registration");

        assert_eq!(
            registry.probe_runnable(&name("nightly-report")),
            RunnableProbe::Registered
        );
        assert_eq!(
            registry.probe_runnable(&name("unknown-runnable")),
            RunnableProbe::Unknown
        );
    }

    #[test]
    fn registry_rejects_duplicate_registration_with_typed_error() {
        let mut registry = HostRunnableRegistry::new();
        let runnable = || -> Arc<dyn HostRunnable> {
            Arc::new(RecordingRunnable {
                invocations: Arc::new(Mutex::new(Vec::new())),
                result: Ok(HostRunnableOutcome::completed()),
            })
        };
        registry
            .register(name("nightly-report"), runnable())
            .expect("first registration");

        let error = registry
            .register(name("nightly-report"), runnable())
            .expect_err("duplicate registration must fail");
        let HostRunnableRegistryError::DuplicateRunnable { name: duplicate } = error;
        assert_eq!(duplicate.as_str(), "nightly-report");
    }

    #[tokio::test]
    async fn registry_dispatches_invocation_to_named_runnable() {
        let invocations = Arc::new(Mutex::new(Vec::new()));
        let other_invocations = Arc::new(Mutex::new(Vec::new()));
        let mut registry = HostRunnableRegistry::new();
        registry
            .register(
                name("nightly-report"),
                Arc::new(RecordingRunnable {
                    invocations: Arc::clone(&invocations),
                    result: Ok(HostRunnableOutcome::completed()),
                }),
            )
            .expect("register nightly-report");
        registry
            .register(
                name("other-runnable"),
                Arc::new(RecordingRunnable {
                    invocations: Arc::clone(&other_invocations),
                    result: Ok(HostRunnableOutcome::completed()),
                }),
            )
            .expect("register other-runnable");
        // Consume through the trait object exactly like the delivery adapter.
        let host: Arc<dyn ScheduleRunnableHost> = Arc::new(registry);

        let invocation = sample_invocation("nightly-report");
        let outcome = host
            .run_occurrence(invocation.clone())
            .await
            .expect("invocation succeeds");
        assert_eq!(outcome, HostRunnableOutcome::completed());

        let recorded = invocations.lock().expect("invocation lock");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].occurrence_id, invocation.occurrence_id);
        assert_eq!(recorded[0].schedule_id, invocation.schedule_id);
        assert_eq!(recorded[0].runnable, invocation.runnable);
        assert_eq!(recorded[0].trigger_time, invocation.trigger_time);
        assert_eq!(
            recorded[0].params.as_deref().map(RawValue::get),
            Some(r#"{"depth":3}"#)
        );
        assert!(
            other_invocations.lock().expect("other lock").is_empty(),
            "only the named runnable may be invoked"
        );
    }

    #[tokio::test]
    async fn registry_reports_typed_failure_for_unknown_runnable_invocation() {
        let registry = HostRunnableRegistry::new();
        let error = registry
            .run_occurrence(sample_invocation("missing-runnable"))
            .await
            .expect_err("unknown runnable must fail");
        let HostRunnableError::Failed { detail } = error;
        assert!(detail.contains("missing-runnable"));
        assert!(detail.contains("not registered"));
    }

    #[tokio::test]
    async fn registry_propagates_runnable_failure() {
        let mut registry = HostRunnableRegistry::new();
        registry
            .register(
                name("flaky-runnable"),
                Arc::new(RecordingRunnable {
                    invocations: Arc::new(Mutex::new(Vec::new())),
                    result: Err("downstream export failed".to_string()),
                }),
            )
            .expect("register flaky-runnable");

        let error = registry
            .run_occurrence(sample_invocation("flaky-runnable"))
            .await
            .expect_err("runnable failure must propagate");
        assert_eq!(
            error.to_string(),
            "host runnable invocation failed: downstream export failed"
        );
    }
}
