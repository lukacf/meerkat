//! Mechanical byte accounting over generated completion-credit state.

use super::dsl::{LiveEffectPhase, LiveRequestMachineState};
use crate::live_ledger::completion::LivePhysicalEffectOutcome;
use crate::live_ledger::completion_budget::{
    CompletionCreditReservation, CompletionCreditSchema, CompletionEnvelopeBudgetV1,
};
use crate::live_resources::{
    LIVE_EVENT_STORAGE_ALLOWANCE_BYTES, LiveCompletionObligation, LiveResourceCharge,
};
use crate::store::RuntimeStoreError;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
pub(in crate::live_ledger) struct EffectCompletionBudget {
    pub envelope: CompletionEnvelopeBudgetV1,
    pub minimum_record_charge: u64,
    pub maximum_record_charge: u64,
    pub snapshot_ceiling: u64,
}

#[derive(Serialize)]
struct MutableCreditImage<'a> {
    phase: LiveEffectPhase,
    retry_eligible: bool,
    spent_records: u64,
    spent_bytes: u64,
    terminal_sequence: u64,
    terminal_digest: &'a str,
    known_tokens: u64,
    request_known_tokens: u64,
    accounting_status: meerkat_core::execution_scope::ScopedTokenAccountingStatus,
    accounting_record: &'a str,
}

impl MutableCreditImage<'_> {
    fn encoded_bytes(&self) -> Result<u64, RuntimeStoreError> {
        serde_json::to_vec(self)
            .map(|bytes| bytes.len() as u64)
            .map_err(invalid)
    }
}

impl EffectCompletionBudget {
    pub fn for_kind(
        kind: meerkat_core::execution_scope::ScopedEffectKind,
    ) -> Result<Self, RuntimeStoreError> {
        let mut budget = Self::measured()?;
        if kind == meerkat_core::execution_scope::ScopedEffectKind::ToolDispatch {
            budget.snapshot_ceiling = budget
                .snapshot_ceiling
                .checked_add(super::callback_credits::snapshot_ceiling()?)
                .ok_or_else(|| invalid("callback snapshot reservation overflow"))?;
        }
        Ok(budget)
    }

    pub fn measured() -> Result<Self, RuntimeStoreError> {
        static BUDGET: OnceLock<EffectCompletionBudget> = OnceLock::new();
        if let Some(budget) = BUDGET.get() {
            return Ok(*budget);
        }
        let envelope =
            CompletionEnvelopeBudgetV1::for_obligation(LiveCompletionObligation::EffectStart)
                .map_err(invalid)?;
        let minimum_record_charge = envelope
            .minimum_encoded_record_bytes()
            .checked_add(LIVE_EVENT_STORAGE_ALLOWANCE_BYTES)
            .ok_or_else(|| invalid("minimum charge overflow"))?;
        let maximum_record_charge = envelope
            .maximum_encoded_record_bytes()
            .checked_add(LIVE_EVENT_STORAGE_ALLOWANCE_BYTES)
            .ok_or_else(|| invalid("maximum charge overflow"))?;
        // These are encoding extrema, not a fabricated machine snapshot.
        let digest = "f".repeat(64);
        let phases = LivePhysicalEffectOutcome::ALL
            .iter()
            .copied()
            .map(effect_phase)
            .chain(std::iter::once(LiveEffectPhase::Claimed))
            .collect::<Vec<_>>();
        let mut snapshot_ceiling = 0;
        for accounting in crate::live_ledger::completion_budget::token_accounting_encoding_extrema()
        {
            let accounting_record = serde_json::to_string(&accounting).map_err(invalid)?;
            for phase in &phases {
                snapshot_ceiling = snapshot_ceiling.max(
                    MutableCreditImage {
                        phase: *phase,
                        retry_eligible: false,
                        spent_records: u64::MAX,
                        spent_bytes: u64::MAX,
                        terminal_sequence: u64::MAX,
                        terminal_digest: &digest,
                        known_tokens: u64::MAX,
                        request_known_tokens: u64::MAX,
                        accounting_status: accounting.status(),
                        accounting_record: &accounting_record,
                    }
                    .encoded_bytes()?,
                );
            }
        }
        let budget = Self {
            envelope,
            minimum_record_charge,
            maximum_record_charge,
            snapshot_ceiling,
        };
        Ok(*BUDGET.get_or_init(|| budget))
    }
}

pub(in crate::live_ledger) fn effect_phase(outcome: LivePhysicalEffectOutcome) -> LiveEffectPhase {
    match outcome {
        LivePhysicalEffectOutcome::Succeeded => LiveEffectPhase::Succeeded,
        LivePhysicalEffectOutcome::Failed => LiveEffectPhase::Failed,
        LivePhysicalEffectOutcome::Cancelled => LiveEffectPhase::Cancelled,
        LivePhysicalEffectOutcome::Unknown => LiveEffectPhase::Unknown,
        LivePhysicalEffectOutcome::NotStarted => LiveEffectPhase::NotStarted,
    }
}

fn field<'a, T>(map: &'a BTreeMap<String, T>, claim: &str) -> Result<&'a T, RuntimeStoreError> {
    map.get(claim)
        .ok_or_else(|| invalid("generated claim credit field is missing"))
}

pub(in crate::live_ledger) fn reserved_completion_charge(
    state: &LiveRequestMachineState,
) -> Result<LiveResourceCharge, RuntimeStoreError> {
    if state.completion_credit_schema != CompletionCreditSchema::V1 {
        return Err(invalid("unsupported generated completion-credit schema"));
    }
    let request_charge = super::request_credits::reserved_completion_charge(state)?;
    if state.claim_ids.is_empty() {
        return Ok(request_charge);
    }
    state
        .claim_ids
        .iter()
        .try_fold(request_charge, |total, claim| {
            let kind = *field(&state.claim_kinds, claim)?;
            let budget = EffectCompletionBudget::for_kind(kind)?;
            if *field(&state.claim_credit_records, claim)? != budget.envelope.total().records
                || *field(&state.claim_credit_bytes, claim)?
                    != budget.envelope.total().encoded_bytes
                || *field(&state.claim_credit_minimum_record_charge, claim)?
                    != budget.minimum_record_charge
                || *field(&state.claim_credit_maximum_record_charge, claim)?
                    != budget.maximum_record_charge
                || *field(&state.claim_credit_snapshot_ceiling, claim)? != budget.snapshot_ceiling
            {
                return Err(invalid(
                    "generated claim does not retain the current measured completion budget",
                ));
            }
            let phase = *field(&state.claim_phases, claim)?;
            let spent = LiveResourceCharge {
                records: *field(&state.claim_credit_spent_records, claim)?,
                encoded_bytes: *field(&state.claim_credit_spent_bytes, claim)?,
            };
            let reservation = CompletionCreditReservation::from_record(budget.envelope, spent)
                .map_err(invalid)?;
            let image = MutableCreditImage {
                phase,
                retry_eligible: *field(&state.claim_retry_eligible, claim)?,
                spent_records: spent.records,
                spent_bytes: spent.encoded_bytes,
                terminal_sequence: *field(&state.claim_terminal_sequences, claim)?,
                terminal_digest: field(&state.claim_terminal_digests, claim)?.as_str(),
                known_tokens: *field(&state.claim_known_tokens, claim)?,
                request_known_tokens: *field(
                    &state.request_known_tokens,
                    field(&state.claim_requests, claim)?,
                )?,
                accounting_status: *field(&state.claim_accounting_status, claim)?,
                accounting_record: field(&state.claim_accounting_records, claim)?,
            };
            let run = field(&state.claim_runs, claim)?;
            let callback_member = field(&state.run_callback_claims, run)?.contains(claim);
            let callback_record = field(&state.run_callback_records, run)?;
            let callback_growth =
                if kind == meerkat_core::execution_scope::ScopedEffectKind::ToolDispatch {
                    super::callback_credits::used_snapshot_bytes(
                        callback_record,
                        field(&state.run_callback_receipts, run)?,
                        field(&state.run_callback_digests, run)?,
                        *field(&state.run_callback_sequences, run)?,
                        callback_member.then_some(claim.as_str()),
                    )?
                } else {
                    0
                };
            let snapshot_used = image
                .encoded_bytes()?
                .checked_add(callback_growth)
                .ok_or_else(|| invalid("callback snapshot charge overflow"))?;
            let snapshot_remaining = budget
                .snapshot_ceiling
                .checked_sub(snapshot_used)
                .ok_or_else(|| {
                    invalid("generated claim exceeds its completion snapshot ceiling")
                })?;
            let callback_pending = kind
                == meerkat_core::execution_scope::ScopedEffectKind::ToolDispatch
                && phase != LiveEffectPhase::NotStarted
                && *field(&state.request_phases, field(&state.claim_requests, claim)?)?
                    != super::dsl::LiveRequestPhase::Terminal
                && (callback_record.is_empty() || callback_member);
            if matches!(phase, LiveEffectPhase::Claimed | LiveEffectPhase::Unknown)
                || callback_pending
            {
                let remaining = reservation
                    .remaining()
                    .checked_add(LiveResourceCharge {
                        records: 0,
                        encoded_bytes: snapshot_remaining,
                    })
                    .map_err(invalid)?;
                total.checked_add(remaining).map_err(invalid)
            } else {
                Ok(total)
            }
        })
}

fn invalid(error: impl std::fmt::Display) -> RuntimeStoreError {
    RuntimeStoreError::WriteFailed(format!("Live effect completion accounting: {error}"))
}

pub(in crate::live_ledger) fn completion_digest(
    record: &crate::live_ledger::completion::LiveCompletionRecord,
) -> Result<String, RuntimeStoreError> {
    completion_content_digest(record, b"meerkat.live-effect-feedback.v1\0")
}

pub(in crate::live_ledger) fn completion_content_digest(
    record: &crate::live_ledger::completion::LiveCompletionRecord,
    domain: &[u8],
) -> Result<String, RuntimeStoreError> {
    use sha2::{Digest, Sha256};
    let content = serde_json::to_vec(&(&record.session_id, &record.channel_id, &record.event))
        .map_err(invalid)?;
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(content);
    Ok(format!("{:x}", digest.finalize()))
}

pub(in crate::live_ledger) fn validate_completion_delta(
    before: &LiveRequestMachineState,
    after: &LiveRequestMachineState,
    record: &crate::live_ledger::completion::LiveCompletionRecord,
    charge: LiveResourceCharge,
) -> Result<(), RuntimeStoreError> {
    let crate::live_ledger::completion::LiveCompletionEvent::EffectTerminal {
        claim_id,
        request_id,
        outcome,
        token_accounting,
        ..
    } = &record.event
    else {
        return Err(invalid(
            "effect settlement requires an effect terminal record",
        ));
    };
    let claim = claim_id.to_string();
    let spent_before = LiveResourceCharge {
        records: *field(&before.claim_credit_spent_records, &claim)?,
        encoded_bytes: *field(&before.claim_credit_spent_bytes, &claim)?,
    };
    let spent_after = LiveResourceCharge {
        records: *field(&after.claim_credit_spent_records, &claim)?,
        encoded_bytes: *field(&after.claim_credit_spent_bytes, &claim)?,
    };
    let prior_tokens = *field(&before.claim_known_tokens, &claim)?;
    let next_tokens = match token_accounting.known_tokens() {
        Some(observed) => prior_tokens.max(observed),
        None => prior_tokens,
    };
    let request_tokens = field(&before.request_known_tokens, &request_id.to_string())?
        .saturating_add(next_tokens - prior_tokens);
    if spent_before.checked_add(charge).map_err(invalid)? != spent_after
        || *field(&after.claim_phases, &claim)? != effect_phase(*outcome)
        || field(&after.claim_requests, &claim)? != &request_id.to_string()
        || *field(&after.claim_terminal_sequences, &claim)? != record.sequence.get()
        || field(&after.claim_terminal_digests, &claim)? != &completion_digest(record)?
        || field(&after.claim_accounting_records, &claim)?
            != &serde_json::to_string(token_accounting).map_err(invalid)?
        || *field(&after.claim_accounting_status, &claim)? != token_accounting.status()
        || *field(&after.claim_known_tokens, &claim)? != next_tokens
        || *field(&after.request_known_tokens, &request_id.to_string())? != request_tokens
    {
        return Err(invalid(
            "completion bytes do not match generated credit spend and settlement",
        ));
    }
    Ok(())
}
