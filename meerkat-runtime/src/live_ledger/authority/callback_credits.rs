//! Growth of callback fields whose empty entries are charged at run staging.

use crate::store::RuntimeStoreError;
use meerkat_core::session::CallbackBatchIdentity;
use serde::Serialize;

pub(super) fn snapshot_ceiling() -> Result<u64, RuntimeStoreError> {
    let identity = CallbackBatchIdentity::encoded_storage_byte_ceiling()
        .map_err(invalid)?
        .checked_sub(2)
        .ok_or_else(|| invalid("callback identity encoding is shorter than an empty string"))?;
    let digest = "f".repeat(64);
    let claim = uuid::Uuid::nil().to_string();
    let other_fields = used_snapshot_bytes("", &digest, &digest, u64::MAX, Some(&claim))?;
    (identity as u64)
        .checked_add(other_fields)
        .ok_or_else(|| invalid("callback snapshot ceiling overflow"))
}

pub(super) fn used_snapshot_bytes(
    identity: &str,
    receipt: &str,
    digest: &str,
    sequence: u64,
    claim: Option<&str>,
) -> Result<u64, RuntimeStoreError> {
    let members: &[&str] = match &claim {
        Some(claim) => std::slice::from_ref(claim),
        None => &[],
    };
    [
        growth(identity, "")?,
        growth(receipt, "")?,
        growth(digest, "")?,
        growth(&sequence, &0_u64)?,
        growth(members, &[] as &[&str])?,
    ]
    .into_iter()
    .try_fold(0_u64, |sum, bytes| {
        sum.checked_add(bytes)
            .ok_or_else(|| invalid("callback snapshot charge overflow"))
    })
}

fn growth<T: Serialize + ?Sized>(value: &T, empty: &T) -> Result<u64, RuntimeStoreError> {
    let bytes = serde_json::to_vec(value).map_err(invalid)?.len() as u64;
    let empty = serde_json::to_vec(empty).map_err(invalid)?.len() as u64;
    bytes
        .checked_sub(empty)
        .ok_or_else(|| invalid("callback encoding shrank below its staged empty entry"))
}

fn invalid(error: impl std::fmt::Display) -> RuntimeStoreError {
    RuntimeStoreError::WriteFailed(format!("Live callback completion accounting: {error}"))
}

pub(in crate::live_ledger) fn suspension_digest(
    identity: &str,
    receipt: &str,
    claims: &std::collections::BTreeSet<String>,
) -> Result<String, RuntimeStoreError> {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"meerkat.live-callback-suspension.v1\0");
    digest.update(serde_json::to_vec(&(identity, receipt, claims)).map_err(invalid)?);
    Ok(format!("{:x}", digest.finalize()))
}

pub(in crate::live_ledger) fn validate_completion_delta(
    before: &super::dsl::LiveRequestMachineState,
    after: &super::dsl::LiveRequestMachineState,
    record: &crate::live_ledger::completion::LiveCompletionRecord,
    charge: crate::live_resources::LiveResourceCharge,
) -> Result<(), RuntimeStoreError> {
    use crate::live_ledger::completion::LiveCompletionEvent;
    let LiveCompletionEvent::CallbackSuspended {
        claim_id,
        request_id,
        input_id,
        run_id,
        batch_digest,
    } = &record.event
    else {
        return Err(invalid("callback spend requires its suspension record"));
    };
    let claim = claim_id.to_string();
    let request = request_id.to_string();
    let run = run_id.to_string();
    let identity_record = after
        .run_callback_records
        .get(&run)
        .ok_or_else(|| invalid("missing callback identity"))?;
    let identity: CallbackBatchIdentity = serde_json::from_str(identity_record).map_err(invalid)?;
    let receipt = after
        .run_callback_receipts
        .get(&run)
        .ok_or_else(|| invalid("missing callback receipt"))?;
    let claims = after
        .run_callback_claims
        .get(&run)
        .ok_or_else(|| invalid("missing callback members"))?;
    let ordinal = claims
        .iter()
        .position(|member| member == &claim)
        .ok_or_else(|| invalid("callback record has no retained member"))?;
    let final_sequence = record
        .sequence
        .get()
        .checked_add((claims.len() - ordinal - 1) as u64)
        .ok_or_else(|| invalid("callback sequence overflow"))?;
    let prior_records = before
        .claim_credit_spent_records
        .get(&claim)
        .ok_or_else(|| invalid("missing prior claim spend"))?;
    let prior_bytes = before
        .claim_credit_spent_bytes
        .get(&claim)
        .ok_or_else(|| invalid("missing prior claim bytes"))?;
    if identity.session_id() != &record.session_id
        || identity.run_id() != run_id
        || identity.batch_digest() != batch_digest.as_bytes()
        || before.run_callback_records.get(&run).map(String::as_str) != Some("")
        || after.request_phases.get(&request) != Some(&super::dsl::LiveRequestPhase::Suspended)
        || after.request_runs.get(&request) != Some(&run)
        || after.run_inputs.get(&run) != Some(&input_id.to_string())
        || !claims.contains(&claim)
        || after.run_callback_sequences.get(&run) != Some(&final_sequence)
        || after.run_callback_digests.get(&run)
            != Some(&suspension_digest(identity_record, receipt, claims)?)
        || after.claim_phases.get(&claim) != before.claim_phases.get(&claim)
        || prior_records.checked_add(charge.records)
            != after.claim_credit_spent_records.get(&claim).copied()
        || prior_bytes.checked_add(charge.encoded_bytes)
            != after.claim_credit_spent_bytes.get(&claim).copied()
    {
        return Err(invalid(
            "callback record differs from generated suspension and exact credit spend",
        ));
    }
    Ok(())
}
