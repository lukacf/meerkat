//! Mechanical encoding bounds for the generated request-completion obligation.

use super::dsl::{LiveRequestMachineState, LiveRequestPhase};
use crate::live_ledger::completion::{LiveCompletionEvent, LiveCompletionRecord};
use crate::live_ledger::completion_budget::{
    CompletionCreditReservation, CompletionEnvelopeBudgetV1,
};
use crate::live_resources::{LiveCompletionObligation, LiveResourceCharge};
use crate::store::RuntimeStoreError;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
pub(in crate::live_ledger) struct RequestCompletionBudget {
    pub envelope: CompletionEnvelopeBudgetV1,
    pub snapshot_ceiling: u64,
}

#[derive(Serialize)]
struct MutableCreditImage<'a> {
    phase: LiveRequestPhase,
    spent_records: u64,
    spent_bytes: u64,
    terminal_sequence: u64,
    terminal_digest: &'a str,
    ordinary_completion_digest: &'a str,
}

impl MutableCreditImage<'_> {
    fn encoded_bytes(&self) -> Result<u64, RuntimeStoreError> {
        serde_json::to_vec(self)
            .map(|bytes| bytes.len() as u64)
            .map_err(invalid)
    }
}

impl RequestCompletionBudget {
    pub fn measured() -> Result<Self, RuntimeStoreError> {
        static BUDGET: OnceLock<RequestCompletionBudget> = OnceLock::new();
        if let Some(budget) = BUDGET.get() {
            return Ok(*budget);
        }
        let envelope =
            CompletionEnvelopeBudgetV1::for_obligation(LiveCompletionObligation::RequestChain)
                .map_err(invalid)?;
        let digest = "f".repeat(64);
        let mut snapshot_ceiling = 0;
        for phase in [
            LiveRequestPhase::Reserved,
            LiveRequestPhase::Admitted,
            LiveRequestPhase::Running,
            LiveRequestPhase::Suspended,
            LiveRequestPhase::Terminal,
        ] {
            snapshot_ceiling = snapshot_ceiling.max(
                MutableCreditImage {
                    phase,
                    spent_records: u64::MAX,
                    spent_bytes: u64::MAX,
                    terminal_sequence: u64::MAX,
                    terminal_digest: &digest,
                    ordinary_completion_digest: &digest,
                }
                .encoded_bytes()?,
            );
        }
        Ok(*BUDGET.get_or_init(|| Self {
            envelope,
            snapshot_ceiling,
        }))
    }
}

fn field<'a, T>(map: &'a BTreeMap<String, T>, request: &str) -> Result<&'a T, RuntimeStoreError> {
    map.get(request)
        .ok_or_else(|| invalid("generated request credit field is missing"))
}

pub(in crate::live_ledger) fn reserved_completion_charge(
    state: &LiveRequestMachineState,
) -> Result<LiveResourceCharge, RuntimeStoreError> {
    let budget = RequestCompletionBudget::measured()?;
    let requests =
        state
            .request_ids
            .iter()
            .try_fold(LiveResourceCharge::default(), |total, request| {
                if *field(&state.request_credit_records, request)?
                    != budget.envelope.total().records
                    || *field(&state.request_credit_bytes, request)?
                        != budget.envelope.total().encoded_bytes
                    || *field(&state.request_credit_snapshot_ceiling, request)?
                        != budget.snapshot_ceiling
                {
                    return Err(invalid(
                        "request does not retain the measured completion budget",
                    ));
                }
                let spent = LiveResourceCharge {
                    records: *field(&state.request_credit_spent_records, request)?,
                    encoded_bytes: *field(&state.request_credit_spent_bytes, request)?,
                };
                let reservation = CompletionCreditReservation::from_record(budget.envelope, spent)
                    .map_err(invalid)?;
                let phase = *field(&state.request_phases, request)?;
                let image = MutableCreditImage {
                    phase,
                    spent_records: spent.records,
                    spent_bytes: spent.encoded_bytes,
                    terminal_sequence: *field(&state.request_terminal_sequences, request)?,
                    terminal_digest: field(&state.request_terminal_digests, request)?,
                    ordinary_completion_digest: field(
                        &state.request_ordinary_completion_digests,
                        request,
                    )?,
                };
                let remaining = budget
                    .snapshot_ceiling
                    .checked_sub(image.encoded_bytes()?)
                    .ok_or_else(|| invalid("request exceeds its completion snapshot ceiling"))?;
                if state.request_completion_obligations.contains(request) {
                    total
                        .checked_add(
                            reservation
                                .remaining()
                                .checked_add(LiveResourceCharge {
                                    records: 0,
                                    encoded_bytes: remaining,
                                })
                                .map_err(invalid)?,
                        )
                        .map_err(invalid)
                } else {
                    Ok(total)
                }
            })?;
    state
        .run_continuation_stage_credits
        .iter()
        .try_fold(requests, |total, (run, bytes)| {
            let request = field(&state.run_requests, run)?;
            if *field(&state.request_phases, request)? == LiveRequestPhase::Terminal {
                Ok(total)
            } else {
                total
                    .checked_add(LiveResourceCharge {
                        records: 0,
                        encoded_bytes: *bytes,
                    })
                    .map_err(invalid)
            }
        })
}

pub(in crate::live_ledger) fn completion_digest(
    record: &LiveCompletionRecord,
) -> Result<String, RuntimeStoreError> {
    super::effect_credits::completion_content_digest(
        record,
        b"meerkat.live-request-completion.v1\0",
    )
}

pub(in crate::live_ledger) fn validate_completion_delta(
    before: &LiveRequestMachineState,
    after: &LiveRequestMachineState,
    record: &LiveCompletionRecord,
    charge: LiveResourceCharge,
) -> Result<(), RuntimeStoreError> {
    let LiveCompletionEvent::RequestOutcome {
        request_id,
        outcome,
    } = &record.event
    else {
        return Err(invalid(
            "request settlement requires its request outcome record",
        ));
    };
    use crate::live_ledger::completion::LiveRequestCompletionFact;
    let (input_id, run_id) = match outcome {
        LiveRequestCompletionFact::OrdinaryTerminal {
            input_id, run_id, ..
        }
        | LiveRequestCompletionFact::Completed {
            input_id, run_id, ..
        }
        | LiveRequestCompletionFact::Failed {
            input_id, run_id, ..
        }
        | LiveRequestCompletionFact::Cancelled {
            input_id, run_id, ..
        } => (input_id, Some(run_id)),
        LiveRequestCompletionFact::OrdinaryRunlessTerminal { input_id, .. } => (input_id, None),
        _ => return Err(invalid("outcome is not an ordinary terminal reference")),
    };
    let request = request_id.to_string();
    if let LiveRequestCompletionFact::OrdinaryTerminal { receipt_digest, .. }
    | LiveRequestCompletionFact::OrdinaryRunlessTerminal { receipt_digest, .. } = outcome
        && field(&after.request_ordinary_completion_digests, &request)? != receipt_digest.as_str()
    {
        return Err(invalid(
            "request record differs from the exact ordinary receipt",
        ));
    }
    if *field(&before.request_credit_spent_records, &request)? != 0
        || *field(&before.request_credit_spent_bytes, &request)? != 0
        || *field(&after.request_credit_spent_records, &request)? != charge.records
        || *field(&after.request_credit_spent_bytes, &request)? != charge.encoded_bytes
        || *field(&after.request_phases, &request)? != LiveRequestPhase::Terminal
        || *field(&after.request_terminal_sequences, &request)? != record.sequence.get()
        || field(&after.request_terminal_digests, &request)? != &completion_digest(record)?
        || match run_id {
            Some(run_id) => {
                field(&after.run_inputs, &run_id.to_string())? != &input_id.to_string()
                    || field(&after.request_runs, &request)? != &run_id.to_string()
            }
            None => match after.request_runs.get(&request) {
                Some(previous_run) => {
                    !after.bound_requests.contains(&request)
                        || field(&before.request_phases, &request)? != &LiveRequestPhase::Suspended
                        || field(&before.request_runs, &request)? != previous_run
                        || field(&after.run_requests, previous_run)? != &request
                        || field(&before.run_continuation_inputs, previous_run)?
                            != &input_id.to_string()
                        || field(&after.run_continuation_inputs, previous_run)?
                            != &input_id.to_string()
                        || !field(&before.run_successors, previous_run)?.is_empty()
                        || !field(&after.run_successors, previous_run)?.is_empty()
                }
                None => {
                    after.bound_requests.contains(&request)
                        || field(&after.request_inputs, &request)? != &input_id.to_string()
                }
            },
        }
    {
        return Err(invalid(
            "request record differs from generated completion and spend",
        ));
    }
    Ok(())
}

fn invalid(error: impl std::fmt::Display) -> RuntimeStoreError {
    RuntimeStoreError::WriteFailed(format!("Live request completion accounting: {error}"))
}
