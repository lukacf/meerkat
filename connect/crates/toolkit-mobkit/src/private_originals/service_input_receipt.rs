//! Read one approved B input through its existing source and native owners.
//! Source evidence selects a read; it never grants execution or restores ingress.
use super::*;
use meerkat_core::{SessionId, lifecycle::InputId};
use meerkat_runtime::{
    completion::CompletionOutcome,
    input_state::{InputLifecycleState, InputTerminalOutcome},
    terminal_status::{InputTerminalReceiptRead, InputTerminalReceiptScope, InteractionSelector,
        Sourced, TerminalWitnessSource},
};

/// A read observation, not permission to execute, retry or publish an input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PrivateServiceInputObservation {
    /// Canonical admission is unavailable; native effects may already exist.
    AdmissionUnresolved,
    Admitted {
        session_id: String,
        input_id: String,
        /// None means this exact native read returned no receipt, not no effect.
        receipt: Option<PrivateServiceInputReceipt>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateServiceInputReceipt {
    pub source: PrivateReceiptWitness,
    pub report: PrivateServiceInputReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateReceiptWitness { LiveRuntime, DurableStore }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PrivateReceiptScope {
    Run { run_id: String },
    RuntimeTermination,
}

/// Native state and outcome fields retain their native serialization. In
/// particular, a Run scope or terminal Pending field is not completion proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum PrivateServiceInputReport {
    Pending {
        input_id: String,
        phase: InputLifecycleState,
        terminal: Option<InputTerminalOutcome>,
        last_run_id: Option<String>,
        attempt_count: u32,
    },
    Finalized {
        input_id: String,
        terminal: InputTerminalOutcome,
        attempt_count: u32,
        scope: PrivateReceiptScope,
        run_id: Option<String>,
        owner_input_id: String,
        recipient_input_ids: Vec<String>,
        outcome: CompletionOutcome,
    },
    TerminalWithoutReceipt {
        input_id: String,
        terminal: InputTerminalOutcome,
        last_run_id: Option<String>,
        attempt_count: u32,
    },
}

fn project_receipt(
    selected_input: &InputId,
    receipt: Sourced<InputTerminalReceiptRead>,
) -> Result<PrivateServiceInputReceipt, PrivateOriginalError> {
    if receipt.report.input_id() != selected_input {
        return Err(PrivateOriginalError::Conflict);
    }
    #[allow(unreachable_patterns)]
    let source = match receipt.source {
        TerminalWitnessSource::LiveRuntime => PrivateReceiptWitness::LiveRuntime,
        TerminalWitnessSource::DurableStore => PrivateReceiptWitness::DurableStore,
        _ => return Err(PrivateOriginalError::Unavailable),
    };
    let report = match receipt.report {
        InputTerminalReceiptRead::Pending { input_id, phase, terminal, last_run_id, attempt_count } =>
            PrivateServiceInputReport::Pending { input_id: input_id.to_string(), phase, terminal,
                last_run_id: last_run_id.map(|run| run.to_string()), attempt_count },
        InputTerminalReceiptRead::TerminalWithoutReceipt { input_id, terminal, last_run_id, attempt_count } =>
            PrivateServiceInputReport::TerminalWithoutReceipt { input_id: input_id.to_string(), terminal,
                last_run_id: last_run_id.map(|run| run.to_string()), attempt_count },
        InputTerminalReceiptRead::Finalized(receipt) => {
            let scope = match receipt.scope() {
                InputTerminalReceiptScope::Run { run_id } => PrivateReceiptScope::Run { run_id: run_id.to_string() },
                InputTerminalReceiptScope::RuntimeTermination => PrivateReceiptScope::RuntimeTermination,
                _ => return Err(PrivateOriginalError::Unavailable),
            };
            PrivateServiceInputReport::Finalized {
                input_id: receipt.input_id().to_string(), terminal: receipt.terminal().clone(),
                attempt_count: receipt.attempt_count(), scope,
                run_id: receipt.run_id().map(ToString::to_string),
                owner_input_id: receipt.owner_input_id().to_string(),
                recipient_input_ids: receipt.recipient_input_ids().iter().map(ToString::to_string).collect(),
                outcome: receipt.outcome().clone(),
            }
        }
        _ => return Err(PrivateOriginalError::Unavailable),
    };
    Ok(PrivateServiceInputReceipt { source, report })
}

struct ServiceInputReceiptSelection {
    record: SourceRecord,
    session: SessionId,
    input: InputId,
}

impl ServiceInputReceiptSelection {
    fn from_record(record: SourceRecord) -> Result<Option<Self>, PrivateOriginalError> {
        let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?;
        if record.setup_request.is_some() || record.configured_setup.is_some() {
            return Err(PrivateOriginalError::Conflict);
        }
        let Some(admission) = record.service_admission.as_ref() else {
            // Native may already have entered before its canonical acknowledgement.
            // Absence here is not a native NotAdmitted or no-effect observation.
            return Ok(None);
        };
        let association = record.final_association.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        let bound = record.bound_service_input.as_ref().ok_or(PrivateOriginalError::Unavailable)?.decode()?;
        cold_service_admission::validate_retained_input(&record)?;
        let session = task.original_task.origin_session_id.clone();
        let input = InputId::from_uuid(admission.input_id.parse().map_err(|_| PrivateOriginalError::Conflict)?);
        if admission.session_id != session.to_string()
            || association.candidate().target.context.as_str() != session.to_string()
            || bound.id() != &input
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(Some(Self { record, session, input }))
    }

    fn recheck(&self, current: &SourceRecord) -> Result<(), PrivateOriginalError> {
        // This is observation currentness, not execution currentness. A later
        // operation_cancelled value must not erase a native finalized receipt.
        if current.source != self.record.source || current.event != self.record.event
            || current.approved_task != self.record.approved_task
            || current.final_association != self.record.final_association
            || current.bound_service_input != self.record.bound_service_input
            || current.service_admission != self.record.service_admission
            || current.setup_request != self.record.setup_request
            || current.configured_setup != self.record.configured_setup
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(())
    }
}

impl PrivateOriginals {
    /// Observe the exact canonical approved B through the machine already bound
    /// to this source owner. Namespace checks authenticate the read selection;
    /// later cancellation, grant revocation or expiry do not rewrite history.
    pub async fn observe_service_input_receipt(
        &self, reference: &PrivateOriginalRef,
    ) -> Result<PrivateServiceInputObservation, PrivateOriginalError> {
        let Some(selected) = self.service_input_receipt_selection(reference).await? else {
            return Ok(PrivateServiceInputObservation::AdmissionUnresolved);
        };
        let machine = self.machine()?;
        let read = machine.input_terminal_receipt(
            &selected.session, InteractionSelector::InputId(selected.input.clone()),
        ).await;
        // Recheck the immutable selector even when native observation failed.
        self.recheck_service_input_receipt(reference, &selected).await?;
        let receipt = read.map_err(|_| PrivateOriginalError::Unavailable)?
            .map(|receipt| project_receipt(&selected.input, receipt)).transpose()?;
        Ok(PrivateServiceInputObservation::Admitted {
            session_id: selected.session.to_string(), input_id: selected.input.to_string(), receipt,
        })
    }

    async fn service_input_receipt_selection(
        &self, reference: &PrivateOriginalRef,
    ) -> Result<Option<ServiceInputReceiptSelection>, PrivateOriginalError> {
        let stored = self.store.get(&self.key(reference)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        ServiceInputReceiptSelection::from_record(self.decode_record(&stored, reference)?)
    }

    async fn recheck_service_input_receipt(
        &self, reference: &PrivateOriginalRef, selected: &ServiceInputReceiptSelection,
    ) -> Result<(), PrivateOriginalError> {
        let stored = self.store.get(&self.key(reference)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        selected.recheck(&self.decode_record(&stored, reference)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These public native variant constructions exercise projection only.
    // Real finalized receipt production is covered by attachment_native_tests.
    #[test]
    fn pending_terminal_and_native_witness_remain_pending() {
        let input = InputId::from_uuid(uuid::Uuid::new_v4());
        let projected = project_receipt(&input, Sourced {
            source: TerminalWitnessSource::DurableStore,
            report: InputTerminalReceiptRead::Pending {
                input_id: input.clone(), phase: InputLifecycleState::Consumed,
                terminal: Some(InputTerminalOutcome::Consumed), last_run_id: None, attempt_count: 2,
            },
        }).unwrap();
        assert!(matches!(projected.source, PrivateReceiptWitness::DurableStore));
        assert!(matches!(projected.report, PrivateServiceInputReport::Pending {
            terminal: Some(InputTerminalOutcome::Consumed), attempt_count: 2, ..
        }));
    }

    #[test]
    fn terminal_without_receipt_is_not_a_run_result() {
        let input = InputId::from_uuid(uuid::Uuid::new_v4());
        let projected = project_receipt(&input, Sourced {
            source: TerminalWitnessSource::LiveRuntime,
            report: InputTerminalReceiptRead::TerminalWithoutReceipt {
                input_id: input.clone(), terminal: InputTerminalOutcome::Consumed,
                last_run_id: None, attempt_count: 0,
            },
        }).unwrap();
        assert!(matches!(projected.source, PrivateReceiptWitness::LiveRuntime));
        assert!(matches!(projected.report, PrivateServiceInputReport::TerminalWithoutReceipt { .. }));
    }

    #[test]
    fn native_receipt_for_another_input_is_refused() {
        let input = InputId::from_uuid(uuid::Uuid::new_v4());
        let result = project_receipt(&input, Sourced {
            source: TerminalWitnessSource::LiveRuntime,
            report: InputTerminalReceiptRead::Pending {
                input_id: InputId::from_uuid(uuid::Uuid::new_v4()), phase: InputLifecycleState::Queued,
                terminal: None, last_run_id: None, attempt_count: 0,
            },
        });
        assert!(matches!(result, Err(PrivateOriginalError::Conflict)));
    }
}
