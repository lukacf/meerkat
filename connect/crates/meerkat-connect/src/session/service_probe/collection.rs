//! Sequential result-driven collection using the existing retained Ready owner.
//! No alternate router, native authority, credential store or flow is created.
use super::*;
use crate::service_collection::{
    self as api, CollectionCallObservation, CollectionStatus, CollectionStep, CollectionTarget,
    CommittedCollectionResult, ObservedSelectionFreshness, RequiredOperationObservation,
    RequiredServiceCollection, ReusableSourceCoverage, ServiceCollectionReport,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    panic::{AssertUnwindSafe, catch_unwind},
};

/// The retained driver must await this result and preserve its actual error.
/// A serialized partial report never replaces a native cleanup owner in error.
pub(crate) struct CollectionExecution {
    pub(crate) report: ServiceCollectionReport,
    pub(crate) error: Option<ConnectError>,
}
#[derive(Debug)]
struct Stop {
    status: CollectionStatus,
    error: Option<ConnectError>,
}
impl Stop {
    fn plain(status: CollectionStatus) -> Self {
        Self {
            status,
            error: None,
        }
    }
    fn owner(status: CollectionStatus, error: ConnectError) -> Self {
        Self {
            status,
            error: Some(error),
        }
    }
}
fn admit_source(admitted: &mut BTreeSet<String>, source: &str) -> Result<(), Stop> {
    if !admitted.insert(source.to_owned()) {
        return Err(Stop::plain(CollectionStatus::AttemptAlreadyObserved));
    }
    Ok(())
}
fn require_generation(selected: &mut Option<u64>, actual: u64) -> Result<(), ConnectError> {
    match selected {
        Some(expected) if *expected != actual => Err(api::refused()),
        Some(_) => Ok(()),
        None => {
            *selected = Some(actual);
            Ok(())
        }
    }
}
fn retire_failed(host: &ServiceHost, error: Option<&ConnectError>) {
    if error.is_some() {
        host.control().retire();
    }
}
fn callback<T>(f: impl FnOnce() -> T) -> Result<T, Stop> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|_| Stop::plain(CollectionStatus::ProgramPanicked))
}
fn stopped_status(status: &ProbeStatus) -> CollectionStatus {
    match status {
        ProbeStatus::GovernedPrerequisiteUnavailable => {
            CollectionStatus::GovernedPrerequisiteUnavailable
        }
        ProbeStatus::SelectionChanged => CollectionStatus::SelectionChanged,
        ProbeStatus::NotObserved => CollectionStatus::NotObserved,
        ProbeStatus::NativeRefused => CollectionStatus::NativeRefused,
        ProbeStatus::OutcomeUnknown => CollectionStatus::OutcomeUnknown,
        ProbeStatus::ToolFailed => CollectionStatus::ToolFailed,
        ProbeStatus::Cancelled => CollectionStatus::Cancelled,
        ProbeStatus::CleanupUnconfirmed => CollectionStatus::CleanupUnconfirmed,
        ProbeStatus::Unsupported => CollectionStatus::Unsupported,
        ProbeStatus::ToolCompleted
        | ProbeStatus::CapabilityVerified
        | ProbeStatus::CapabilityNotVerified => CollectionStatus::NativeStopped,
    }
}
fn finish_gate(
    assessment: ResourceAssessment,
    calls: &[CollectionCallObservation],
) -> Result<(), Stop> {
    if assessment == ResourceAssessment::Verified
        && (calls.is_empty()
            || calls.iter().any(|step| {
                !matches!(&step.native.status, ProbeStatus::ToolCompleted)
                    || !step.native.native_terminal_committed
                    || !step.native.native_close_joined
                    || step.native.dispatch_outcome != Some(LiveBridgeEffectOutcome::Committed)
            }))
    {
        return Err(Stop::plain(CollectionStatus::ProgramRefused));
    }
    Ok(())
}
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), canonical(v)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
        value => value.clone(),
    }
}
struct Budget {
    limits: api::CollectionLimits,
    seen: BTreeSet<(String, String, String)>,
    calls: usize,
    arguments: usize,
    results: usize,
    attempt: Arc<()>,
    reads: std::collections::BTreeMap<usize, CompletedRead>,
}
struct CompletedRead {
    source: String,
    requirement: String,
    resource: Value,
    config: meerkat_core::McpServerConfig,
    generation: u64,
    call: api::CollectionCall,
    spent: bool,
}
impl Budget {
    fn new(limits: api::CollectionLimits) -> Self {
        Self {
            limits,
            seen: BTreeSet::new(),
            calls: 0,
            arguments: 0,
            results: 0,
            attempt: Arc::new(()),
            reads: Default::default(),
        }
    }
    fn reserve(
        &mut self,
        requirement: &api::RequiredCollectionOperation,
        call: &api::CollectionCall,
    ) -> Result<String, Stop> {
        self.reserve_call(requirement, call, false)
    }
    fn reserve_call(
        &mut self,
        requirement: &api::RequiredCollectionOperation,
        call: &api::CollectionCall,
        reobserving: bool,
    ) -> Result<String, Stop> {
        if !requirement.operations.contains(&call.operation) {
            return Err(Stop::plain(CollectionStatus::ProgramRefused));
        }
        let bytes = serde_json::to_vec(&canonical(&call.arguments))
            .map_err(|_| Stop::plain(CollectionStatus::ProgramRefused))?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let key = (
            requirement.id.clone(),
            call.operation.clone(),
            digest.clone(),
        );
        if self.seen.contains(&key) != reobserving {
            return Err(Stop::plain(CollectionStatus::ReplayRefused));
        }
        if self.calls >= self.limits.calls
            || self.arguments.saturating_add(bytes.len()) > self.limits.argument_bytes
        {
            return Err(Stop::plain(CollectionStatus::BudgetExceeded));
        }
        self.seen.insert(key);
        self.calls += 1;
        self.arguments += bytes.len();
        Ok(digest)
    }
    // Called only after the actual native read-only call's disclosure boundary.
    fn completed_read(
        &mut self,
        source: &str,
        requirement: &api::RequiredCollectionOperation,
        config: &meerkat_core::McpServerConfig,
        generation: u64,
        call: &api::CollectionCall,
    ) -> Option<api::ReadObservation> {
        if !call.read_only {
            return None;
        }
        self.reads.insert(
            self.calls,
            CompletedRead {
                source: source.into(),
                requirement: requirement.id.clone(),
                resource: requirement.resource.clone(),
                config: config.clone(),
                generation,
                call: call.clone(),
                spent: false,
            },
        );
        Some(api::ReadObservation {
            attempt: self.attempt.clone(),
            ordinal: self.calls,
        })
    }
    fn take_read(
        &mut self,
        handle: api::ReadObservation,
        source: &str,
        requirement: &api::RequiredCollectionOperation,
        config: &meerkat_core::McpServerConfig,
        generation: u64,
    ) -> Result<api::CollectionCall, Stop> {
        let refused = || Stop::plain(CollectionStatus::ReplayRefused);
        if !Arc::ptr_eq(&handle.attempt, &self.attempt) {
            return Err(refused());
        }
        let read = self.reads.get_mut(&handle.ordinal).ok_or_else(refused)?;
        if read.spent
            || read.source != source
            || read.requirement != requirement.id
            || read.resource != requirement.resource
            || &read.config != config
            || read.generation != generation
        {
            return Err(refused());
        }
        // Consume before budgets, callbacks, awaits or another native entry.
        read.spent = true;
        Ok(read.call.clone())
    }
    fn result(&mut self, bytes: usize) -> Result<(), Stop> {
        // Retain the actual observed byte count even when it exceeds disclosure
        // budget. Such a result is not supplied to the program and never retries.
        self.results = self.results.saturating_add(bytes);
        if self.results > self.limits.result_bytes {
            return Err(Stop::plain(CollectionStatus::BudgetExceeded));
        }
        Ok(())
    }
    fn project(&self, report: &mut ServiceCollectionReport) {
        report.total_calls = self.calls;
        report.total_argument_bytes = self.arguments;
        report.total_result_bytes = self.results;
    }
}
impl Ready {
    pub(crate) async fn run_collection(
        &self,
        selection: Arc<RequiredServiceCollection>,
    ) -> CollectionExecution {
        let host = self.host.clone();
        let mut report = ServiceCollectionReport {
            source_id: selection.source_id.clone(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            application_id: host.binding.application_id.clone(),
            instance_id: host.binding.instance_id.clone(),
            viewer_id: host.binding.viewer_id.clone(),
            login_scope_id: host.binding.login_scope_id.clone(),
            origin: host.binding.origin.clone(),
            realm: self.capture.storage.locator.realm.as_str().to_owned(),
            session_id: self.capture.session.clone(),
            job_id: self.job_id().to_owned(),
            family: selection.family.clone(),
            profile: selection.profile.clone(),
            agent: selection.agent.clone(),
            status: CollectionStatus::NotObserved,
            requirements: selection
                .requirements
                .iter()
                .map(|r| RequiredOperationObservation {
                    required_operation_id: r.id.clone(),
                    resource: r.resource.clone(),
                    raw_operation_ceiling: r.operations.clone(),
                    assessment: None,
                    calls: Vec::new(),
                })
                .collect(),
            observed_selection: ObservedSelectionFreshness::NotObserved,
            native_own_config_generation: None,
            reusable_source_coverage: ReusableSourceCoverage::UnqualifiedCredentialContinuity,
            limits: selection.limits,
            total_calls: 0,
            total_argument_bytes: 0,
            total_result_bytes: 0,
        };
        // One lock for the entire attempt. The step helper is explicitly
        // unlocked, so ordinary run() cannot recursively acquire this lock.
        let mut owner = self.execution.lock().await;
        if let Err(stop) = admit_source(&mut owner, &selection.source_id) {
            report.status = stop.status;
            return CollectionExecution {
                report,
                error: None,
            };
        }
        // The exact source ID stays admitted even on unknown, cancellation or
        // failure. A second call cannot reset its cursors/replay budget.
        let mut budget = Budget::new(selection.limits);
        let outcome = self
            .collect_locked(&host, &selection, &mut report, &mut budget)
            .await;
        budget.project(&mut report);
        let error = match outcome {
            Ok(()) => {
                report.status = CollectionStatus::HistoricalChecksCompleted;
                None
            }
            Err(stop) => {
                report.status = stop.status;
                stop.error
            }
        };
        // A real error may retain unjoined native cleanup custody. Retire the
        // original host before this lock can admit other source work. Keep the
        // actual error in the result; retirement is not a joined-close proof.
        retire_failed(&host, error.as_ref());
        CollectionExecution { report, error }
    }
    fn collection_target<'a>(
        &'a self,
        selection: &'a RequiredServiceCollection,
        requirement: &'a api::RequiredCollectionOperation,
    ) -> CollectionTarget<'a> {
        CollectionTarget {
            source_id: &selection.source_id,
            requirement_id: &requirement.id,
            resource: &requirement.resource,
            operations: &requirement.operations,
            installed: &self.selected.config,
        }
    }
    async fn collect_locked(
        &self,
        host: &Arc<ServiceHost>,
        selection: &RequiredServiceCollection,
        report: &mut ServiceCollectionReport,
        budget: &mut Budget,
    ) -> Result<(), Stop> {
        self.recheck_collection(host, selection, report).await?;
        for (index, requirement) in selection.requirements.iter().enumerate() {
            let target = self.collection_target(selection, requirement);
            if !callback(|| requirement.program.accepts_target(&target))? {
                return Err(Stop::plain(CollectionStatus::ProgramRefused));
            }
            let mut cursor = callback(|| requirement.program.start(&target))?;
            let mut step = callback(|| cursor.begin())?;
            loop {
                let mut reobserving = false;
                let selected_step = match step {
                    CollectionStep::ReobserveRead(handle) => {
                        self.recheck_collection(host, selection, report).await?;
                        let generation = report
                            .native_own_config_generation
                            .ok_or_else(|| Stop::plain(CollectionStatus::SelectionChanged))?;
                        let call = budget.take_read(
                            handle,
                            &selection.source_id,
                            requirement,
                            &self.selected.config,
                            generation,
                        )?;
                        reobserving = true;
                        CollectionStep::Call(call)
                    }
                    other => other,
                };
                match selected_step {
                    CollectionStep::ReobserveRead(_) => {
                        return Err(Stop::plain(CollectionStatus::ReplayRefused));
                    }
                    CollectionStep::Refuse => {
                        return Err(Stop::plain(CollectionStatus::ProgramRefused));
                    }
                    CollectionStep::Finish(assessment) => {
                        finish_gate(assessment, &report.requirements[index].calls)?;
                        // Recheck after the final program callback as well as
                        // after each close. This remains an observation, not a
                        // credential lease or permission to reuse old checks.
                        self.recheck_collection(host, selection, report).await?;
                        if !callback(|| {
                            requirement
                                .program
                                .accepts_target(&self.collection_target(selection, requirement))
                        })? {
                            return Err(Stop::plain(CollectionStatus::ProgramRefused));
                        }
                        report.requirements[index].assessment = Some(assessment);
                        match assessment {
                            ResourceAssessment::Verified => break,
                            ResourceAssessment::NotVerified => {
                                return Err(Stop::plain(CollectionStatus::NotVerified));
                            }
                            ResourceAssessment::Unknown => {
                                return Err(Stop::plain(CollectionStatus::NotObserved));
                            }
                        }
                    }
                    CollectionStep::Call(call) => {
                        self.recheck_collection(host, selection, report).await?;
                        if !callback(|| {
                            requirement
                                .program
                                .accepts_target(&self.collection_target(selection, requirement))
                        })? {
                            return Err(Stop::plain(CollectionStatus::ProgramRefused));
                        }
                        let digest = if reobserving {
                            budget.reserve_call(requirement, &call, true)?
                        } else {
                            budget.reserve(requirement, &call)?
                        };
                        budget.project(report);
                        // No result predicate is installed on this one call:
                        // the collection cursor sees only the actual joined
                        // committed result and interprets its own fixed row.
                        let profile = Arc::new(
                            RequiredServiceProbe::outbound_mcp(
                                requirement.id.clone(),
                                selection.family.clone(),
                                selection.profile.clone(),
                                selection.agent.clone(),
                                call.operation.clone(),
                                call.arguments.clone(),
                                requirement.resource.clone(),
                                None,
                            )
                            .map_err(|error| {
                                Stop::owner(CollectionStatus::ProgramRefused, error)
                            })?,
                        );
                        let read_intent = if call.read_only {
                            Some((
                                report.native_own_config_generation.ok_or_else(|| {
                                    Stop::plain(CollectionStatus::SelectionChanged)
                                })?,
                                requirement.program.as_ref(),
                            ))
                        } else {
                            None
                        };
                        let DetailedExecution {
                            execution:
                                Execution {
                                    report: mut native,
                                    error,
                                },
                            committed,
                            partial,
                        } = self
                            .run_unlocked_with_read_only(host, &profile, read_intent)
                            .await;
                        // Preserve actual admission even when cancellation settles
                        // before a dispatch. These facts add no terminal/result credit.
                        if let Some(partial) = partial {
                            partial.retain(&mut native);
                        }
                        let stop_status = stopped_status(&native.status);
                        let disclosed = matches!(&native.status, ProbeStatus::ToolCompleted)
                            && native.native_terminal_committed
                            && native.native_close_joined
                            && native.dispatch_outcome == Some(LiveBridgeEffectOutcome::Committed);
                        report.requirements[index]
                            .calls
                            .push(CollectionCallObservation {
                                source_id: selection.source_id.clone(),
                                required_operation_id: requirement.id.clone(),
                                resource: requirement.resource.clone(),
                                ordinal: budget.calls,
                                raw_operation: call.operation.clone(),
                                arguments_sha256: digest,
                                native,
                            });
                        if let Some(error) = error {
                            return Err(Stop::owner(stop_status, error));
                        }
                        let Some(result) = committed.filter(|_| disclosed) else {
                            return Err(Stop::plain(stop_status));
                        };
                        self.recheck_collection(host, selection, report).await?;
                        report.observed_selection =
                            ObservedSelectionFreshness::RecheckedAfterLastClose;
                        let bytes = serde_json::to_vec(&result)
                            .map_err(|_| Stop::plain(CollectionStatus::NotObserved))?;
                        budget.result(bytes.len())?;
                        budget.project(report);
                        let read_observation = read_intent.and_then(|(generation, _)| {
                            budget.completed_read(
                                &selection.source_id,
                                requirement,
                                &self.selected.config,
                                generation,
                                &call,
                            )
                        });
                        let native = &report.requirements[index]
                            .calls
                            .last()
                            .expect("actual step retained")
                            .native;
                        let result = CommittedCollectionResult {
                            target: self.collection_target(selection, requirement),
                            operation: &call.operation,
                            arguments: &call.arguments,
                            native,
                            result: &result,
                            read_observation,
                            invocation: api::CollectionInvocationObservation(
                                host.collection_identity.clone(),
                            ),
                        };
                        step = callback(|| cursor.observe(&result))?;
                    }
                }
            }
        }
        self.recheck_collection(host, selection, report).await?;
        Ok(())
    }
    async fn recheck_collection(
        &self,
        host: &Arc<ServiceHost>,
        selection: &RequiredServiceCollection,
        report: &mut ServiceCollectionReport,
    ) -> Result<(), Stop> {
        let current = self
            .recheck_collection_selection(host, selection)
            .await
            .and_then(|generation| {
                require_generation(&mut report.native_own_config_generation, generation)
            });
        if let Err(error) = current {
            report.observed_selection = ObservedSelectionFreshness::ChangedOrUnconfirmed;
            return Err(Stop::owner(
                if host.lifetime.current(host.binding.expires_at_ms) {
                    CollectionStatus::SelectionChanged
                } else {
                    CollectionStatus::Cancelled
                },
                error,
            ));
        }
        Ok(())
    }
    async fn recheck_collection_selection(
        &self,
        host: &Arc<ServiceHost>,
        selection: &RequiredServiceCollection,
    ) -> Result<u64, ConnectError> {
        if host.binding.realm != self.capture.storage.locator
            || selection.family != self.selected.installed.family
            || selection.profile != self.selected.installed.profile
            || selection.agent != self.selected.request.agent
            || !self.finalized.probe_selected(&self.selected)
            || !host.lifetime.current(host.binding.expires_at_ms)
        {
            return Err(api::refused());
        }
        let StockGrantTarget::Mcp {
            server,
            operations: McpOperations::Exact(operations),
        } = &self.selected.request.target
        else {
            return Err(api::refused());
        };
        if server != &self.selected.config.name
            || selection.requirements.iter().any(|requirement| {
                requirement.operations.iter().any(|operation| {
                    !operations.contains(operation)
                        || self
                            .selected
                            .config
                            .access
                            .as_ref()
                            .is_none_or(|ceiling| !ceiling.allowed_operations.contains(operation))
                })
            })
        {
            return Err(api::refused());
        }
        self.selected.recheck().await?;
        let runtime = crate::realm::own_config_runtime(&self.capture.storage)?;
        let own = meerkat_core::McpRealmPersistTarget::new(&runtime)
            .servers()
            .await
            .map_err(|_| api::refused())?;
        require_selected(&own.0, &self.selected.config)?;
        let effective = crate::realm::read_config(&self.capture.storage).await?;
        require_selected(&effective.tools.mcp_servers, &self.selected.config)?;
        self.selected.recheck().await?;
        let after = meerkat_core::McpRealmPersistTarget::new(&runtime)
            .servers()
            .await
            .map_err(|_| api::refused())?;
        require_selected(&after.0, &self.selected.config)?;
        if own.1 != after.1 || !host.lifetime.current(host.binding.expires_at_ms) {
            return Err(api::refused());
        }
        // Actual native own-document generation, not a host epoch and not
        // credential continuity. Equal restored bytes alone do not pass it.
        Ok(after.1)
    }
}

#[cfg(test)]
mod tests;
