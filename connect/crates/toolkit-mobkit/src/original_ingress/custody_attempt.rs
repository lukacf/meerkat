//! Data and transition rules for one first attempt in an existing source row.
//! These facts neither admit native input nor publish live source custody.
use super::OriginalWriterError;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::{SessionId, lifecycle::InputId};
use meerkat_runtime::{Input, LogicalRuntimeId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const STEPS: u8 = 8;
const INPUT_FORMAT: &str = "toolkit.prepared-original-input.v1";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RecoveryPolicy { EightStepsV1 }

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CustodyDecision { Pending, Promoted, Withdrawn }

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CustodyAttempt {
    session_id: SessionId,
    input_id: InputId,
    prepared_input_sha256: String,
    recovery_policy: RecoveryPolicy,
    spent_steps: u8,
    resolution_entered: bool,
    decision: CustodyDecision,
}

/// The caller owns the actual native observation and terminal arbitration.
/// These changes validate canonical data only, never that owner's evidence.
#[derive(Clone, Copy)]
pub(crate) enum CustodyChange { ChargeResolution, ResolutionSettled, Promote, Withdraw }

impl CustodyAttempt {
    pub(crate) fn for_prepared(
        session: &SessionId, input: &Input, association: &InputAuthorityAssociation,
    ) -> Result<Self, OriginalWriterError> {
        if input.header().ingress_context.is_none() || input.header().retained_resume.is_some()
            || input.header().authority_association.as_ref() != Some(association)
            || input.header().idempotency_key.is_none()
        { return Err(OriginalWriterError::Conflict); }
        let value = Self {
            session_id: session.clone(), input_id: input.id().clone(),
            prepared_input_sha256: Self::fingerprint(input)?,
            recovery_policy: RecoveryPolicy::EightStepsV1, spent_steps: 0,
            resolution_entered: false, decision: CustodyDecision::Pending,
        };
        value.validate_binding(association)?;
        Ok(value)
    }

    fn fingerprint(input: &Input) -> Result<String, OriginalWriterError> {
        // Native serde omits process handles. This exact fingerprint does not
        // normalize ID/time and is not Core's private prompt replay identity.
        let bytes = toolkit_package::canonical_json(&(INPUT_FORMAT, input))
            .map_err(|_| OriginalWriterError::Conflict)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    pub(crate) fn validate(&self) -> Result<(), OriginalWriterError> {
        if self.prepared_input_sha256.len() != 64
            || !self.prepared_input_sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.spent_steps > STEPS
            || (self.resolution_entered && (self.spent_steps == 0 || self.decision != CustodyDecision::Pending))
        { return Err(OriginalWriterError::Conflict); }
        Ok(())
    }

    pub(crate) fn validate_binding(&self, association: &InputAuthorityAssociation)
        -> Result<(), OriginalWriterError>
    {
        self.validate()?;
        if association.candidate().target.logical_runtime.as_str()
            != LogicalRuntimeId::for_session(&self.session_id).to_string()
        { return Err(OriginalWriterError::Conflict); }
        Ok(())
    }

    pub(crate) fn matches_input(&self, input: &Input) -> Result<bool, OriginalWriterError> {
        self.validate()?;
        Ok(input.id() == &self.input_id && Self::fingerprint(input)? == self.prepared_input_sha256)
    }

    pub(crate) fn same_attempt(&self, other: &Self) -> bool {
        self.session_id == other.session_id && self.input_id == other.input_id
            && self.prepared_input_sha256 == other.prepared_input_sha256
            && self.recovery_policy == other.recovery_policy
    }

    pub(crate) fn session(&self) -> &SessionId { &self.session_id }
    pub(crate) fn input_id(&self) -> &InputId { &self.input_id }
    pub(crate) fn decision(&self) -> CustodyDecision { self.decision }
    pub(crate) fn observation_entered(&self) -> bool { self.resolution_entered }
    pub(crate) fn can_observe(&self) -> bool { self.decision == CustodyDecision::Pending && self.spent_steps < STEPS }

    pub(crate) fn changed(&self, change: CustodyChange) -> Result<Self, OriginalWriterError> {
        self.validate()?;
        if self.decision != CustodyDecision::Pending { return Err(OriginalWriterError::Conflict); }
        let mut next = self.clone();
        match change {
            CustodyChange::ChargeResolution if !self.resolution_entered && self.spent_steps < STEPS => {
                next.spent_steps += 1; next.resolution_entered = true;
            }
            CustodyChange::ResolutionSettled if self.resolution_entered => next.resolution_entered = false,
            CustodyChange::Promote => { next.decision = CustodyDecision::Promoted; next.resolution_entered = false; }
            CustodyChange::Withdraw => { next.decision = CustodyDecision::Withdrawn; next.resolution_entered = false; }
            _ => return Err(OriginalWriterError::Conflict),
        }
        Self::validate_transition(Some(self), &next)?;
        Ok(next)
    }

    pub(crate) fn validate_transition(prior: Option<&Self>, next: &Self) -> Result<(), OriginalWriterError> {
        next.validate()?;
        let Some(prior) = prior else {
            return if next.decision == CustodyDecision::Pending && next.spent_steps == 0 && !next.resolution_entered {
                Ok(())
            } else { Err(OriginalWriterError::Conflict) };
        };
        prior.validate()?;
        if prior == next { return Ok(()); }
        if prior.session_id != next.session_id || prior.input_id != next.input_id
            || prior.prepared_input_sha256 != next.prepared_input_sha256
            || prior.recovery_policy != next.recovery_policy
            || prior.decision != CustodyDecision::Pending
        { return Err(OriginalWriterError::Conflict); }
        let allowed = match next.decision {
            CustodyDecision::Promoted | CustodyDecision::Withdrawn =>
                prior.spent_steps == next.spent_steps && !next.resolution_entered,
            CustodyDecision::Pending =>
                (!prior.resolution_entered && next.resolution_entered && prior.spent_steps < STEPS
                    && next.spent_steps == prior.spent_steps + 1)
                || (prior.resolution_entered && !next.resolution_entered && next.spent_steps == prior.spent_steps),
        };
        if allowed { Ok(()) } else { Err(OriginalWriterError::Conflict) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure schema fixtures have no native ingress/custody and cannot be used
    // with for_prepared. Real binding/CAS controls live beside source owners.
    fn attempt() -> CustodyAttempt {
        CustodyAttempt { session_id: SessionId::new(), input_id: InputId::new(),
            prepared_input_sha256: "a".repeat(64), recovery_policy: RecoveryPolicy::EightStepsV1,
            spent_steps: 0, resolution_entered: false, decision: CustodyDecision::Pending }
    }

    #[test]
    fn charged_steps_survive_serialization_and_last_step_can_promote() {
        let mut current = attempt();
        for step in 1..=STEPS {
            current = current.changed(CustodyChange::ChargeResolution).unwrap();
            let encoded = serde_json::to_value(&current).unwrap();
            current = serde_json::from_value(encoded).unwrap();
            assert_eq!(current.spent_steps, step);
            assert!(current.resolution_entered);
            assert!(current.changed(CustodyChange::ChargeResolution).is_err());
            if step != STEPS { current = current.changed(CustodyChange::ResolutionSettled).unwrap(); }
        }
        let promoted = current.changed(CustodyChange::Promote).unwrap();
        assert!(promoted.decision == CustodyDecision::Promoted);
        assert_eq!(promoted.spent_steps, STEPS);
        let exhausted = current.changed(CustodyChange::ResolutionSettled).unwrap();
        assert!(exhausted.changed(CustodyChange::ChargeResolution).is_err());
        assert!(exhausted.changed(CustodyChange::Withdraw).is_ok());
    }

    #[test]
    fn terminal_and_attempt_identity_cannot_be_replaced_or_refunded() {
        let current = attempt().changed(CustodyChange::ChargeResolution).unwrap();
        let mut changed = current.clone(); changed.spent_steps = 0;
        assert!(CustodyAttempt::validate_transition(Some(&current), &changed).is_err());
        for field in ["session_id", "input_id", "prepared_input_sha256"] {
            let other = attempt(); let mut value = serde_json::to_value(&current).unwrap();
            value[field] = if field == "prepared_input_sha256" { serde_json::json!("b".repeat(64)) }
                else { serde_json::to_value(&other).unwrap()[field].clone() };
            let changed: CustodyAttempt = serde_json::from_value(value).unwrap();
            assert!(CustodyAttempt::validate_transition(Some(&current), &changed).is_err());
        }
        for terminal in [CustodyChange::Promote, CustodyChange::Withdraw] {
            let terminal = current.changed(terminal).unwrap();
            let reopened: CustodyAttempt = serde_json::from_value(serde_json::to_value(&terminal).unwrap()).unwrap();
            assert!(CustodyAttempt::validate_transition(Some(&terminal), &reopened).is_ok());
            assert!(CustodyAttempt::validate_transition(Some(&terminal), &current).is_err());
            assert!(terminal.changed(CustodyChange::Promote).is_err());
            assert!(terminal.changed(CustodyChange::Withdraw).is_err());
        }
    }

    #[test]
    fn malformed_policy_budget_and_entered_state_refuse() {
        for (field, value) in [("recovery_policy", serde_json::json!("future_policy")),
            ("spent_steps", serde_json::json!(9)), ("resolution_entered", serde_json::json!(true)),
            ("prepared_input_sha256", serde_json::json!("A".repeat(64))), ("extra", serde_json::json!(true))]
        {
            let mut json = serde_json::to_value(attempt()).unwrap(); json[field] = value;
            assert!(match serde_json::from_value::<CustodyAttempt>(json) { Err(_) => true, Ok(v) => v.validate().is_err() });
        }
    }
}
