//! Typed identity and receipts for exact asynchronous operations.
//!
//! This module is deliberately not a lifecycle machine. It gives domain
//! adapters a typed carrier over their existing generated authority and gives
//! runtime observer plumbing one exact identity to preserve on every exit.

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use sha2::{Digest, Sha256};

use crate::ops::OperationId;
use crate::{InputId, RuntimeEpochId, SessionId};

/// Execution coordinates required to attribute one admitted operation.
///
/// Runtime inputs carry generated session, epoch, and input identities.
/// Domain operations such as Flow runs already carry exact identity in their
/// typed domain correlation and must not fabricate runtime coordinates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OperationExecutionScope {
    RuntimeInput {
        owner_session_id: SessionId,
        runtime_epoch_id: RuntimeEpochId,
        submitted_input_id: InputId,
        canonical_input_id: InputId,
    },
    Domain,
}

/// Terminal-facing execution coordinates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OperationTerminalScope {
    RuntimeInput {
        owner_session_id: SessionId,
        runtime_epoch_id: RuntimeEpochId,
        canonical_input_id: InputId,
    },
    Domain,
}

/// Runtime classification of the input named by an admission receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationAcceptClass {
    Fresh,
    InFlightDuplicate,
    TerminalDuplicate,
}

/// Exact identity allocated before a domain operation is admitted.
///
/// `submitted_input_id` is the caller's identity. `canonical_input_id` is the
/// runtime input that will terminalize and differs only after deduplication.
/// `domain_correlation` stays strongly typed and domain-owned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExactOperationIdentity<D> {
    operation_id: OperationId,
    execution_scope: OperationExecutionScope,
    domain_correlation: D,
}

impl<D> ExactOperationIdentity<D> {
    pub fn for_runtime_input(
        operation_id: OperationId,
        owner_session_id: SessionId,
        runtime_epoch_id: RuntimeEpochId,
        submitted_input_id: InputId,
        canonical_input_id: InputId,
        domain_correlation: D,
    ) -> Self {
        Self {
            operation_id,
            execution_scope: OperationExecutionScope::RuntimeInput {
                owner_session_id,
                runtime_epoch_id,
                submitted_input_id,
                canonical_input_id,
            },
            domain_correlation,
        }
    }

    pub fn for_domain(operation_id: OperationId, domain_correlation: D) -> Self {
        Self {
            operation_id,
            execution_scope: OperationExecutionScope::Domain,
            domain_correlation,
        }
    }

    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    pub fn execution_scope(&self) -> &OperationExecutionScope {
        &self.execution_scope
    }

    pub fn domain_correlation(&self) -> &D {
        &self.domain_correlation
    }
}

/// Durable proof that one exact operation passed admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationAdmissionReceipt<D> {
    identity: ExactOperationIdentity<D>,
    accept_class: OperationAcceptClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result_projection: Option<ValidatedResultProjectionSpec>,
}

impl<D> OperationAdmissionReceipt<D> {
    pub fn new(
        identity: ExactOperationIdentity<D>,
        accept_class: OperationAcceptClass,
        result_projection: Option<ValidatedResultProjectionSpec>,
    ) -> Self {
        Self {
            identity,
            accept_class,
            result_projection,
        }
    }

    pub fn identity(&self) -> &ExactOperationIdentity<D> {
        &self.identity
    }

    pub fn accept_class(&self) -> OperationAcceptClass {
        self.accept_class
    }

    pub fn result_projection(&self) -> Option<&ValidatedResultProjectionSpec> {
        self.result_projection.as_ref()
    }
}

/// Identity available to terminal authority after admission and deduplication.
///
/// The caller's submitted input id is intentionally absent. It remains in the
/// admission receipt as provenance, while terminal authority names only the
/// canonical input it actually executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationTerminalIdentity<D> {
    operation_id: OperationId,
    execution_scope: OperationTerminalScope,
    domain_correlation: D,
}

impl<D> OperationTerminalIdentity<D> {
    pub fn for_runtime_input(
        operation_id: OperationId,
        owner_session_id: SessionId,
        runtime_epoch_id: RuntimeEpochId,
        canonical_input_id: InputId,
        domain_correlation: D,
    ) -> Self {
        Self {
            operation_id,
            execution_scope: OperationTerminalScope::RuntimeInput {
                owner_session_id,
                runtime_epoch_id,
                canonical_input_id,
            },
            domain_correlation,
        }
    }

    pub fn for_domain(operation_id: OperationId, domain_correlation: D) -> Self {
        Self {
            operation_id,
            execution_scope: OperationTerminalScope::Domain,
            domain_correlation,
        }
    }

    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    pub fn execution_scope(&self) -> &OperationTerminalScope {
        &self.execution_scope
    }

    pub fn domain_correlation(&self) -> &D {
        &self.domain_correlation
    }
}

impl<D: Clone> From<&ExactOperationIdentity<D>> for OperationTerminalIdentity<D> {
    fn from(identity: &ExactOperationIdentity<D>) -> Self {
        let execution_scope = match &identity.execution_scope {
            OperationExecutionScope::RuntimeInput {
                owner_session_id,
                runtime_epoch_id,
                canonical_input_id,
                ..
            } => OperationTerminalScope::RuntimeInput {
                owner_session_id: owner_session_id.clone(),
                runtime_epoch_id: runtime_epoch_id.clone(),
                canonical_input_id: canonical_input_id.clone(),
            },
            OperationExecutionScope::Domain => OperationTerminalScope::Domain,
        };
        Self {
            operation_id: identity.operation_id.clone(),
            execution_scope,
            domain_correlation: identity.domain_correlation.clone(),
        }
    }
}

/// Domain terminal projected by the owner of the admitted operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationTerminal<D, T> {
    identity: OperationTerminalIdentity<D>,
    terminal: T,
}

impl<D, T> OperationTerminal<D, T> {
    pub fn new(identity: OperationTerminalIdentity<D>, terminal: T) -> Self {
        Self { identity, terminal }
    }
}

/// Why a purported terminal cannot resolve an exact admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OperationAttributionError {
    #[error("terminal operation id does not match the admitted operation")]
    OperationMismatch,
    #[error("terminal execution scope does not match the admitted operation")]
    ExecutionScopeMismatch,
    #[error("terminal domain correlation does not match the admitted operation")]
    DomainCorrelationMismatch,
    #[error("terminal payload could not be encoded for exact receipt custody")]
    TerminalEncodingFailed,
}

/// Durable exact terminal truth after every attribution check succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminalReceipt<D, T> {
    admission: OperationAdmissionReceipt<D>,
    terminal_identity: OperationTerminalIdentity<D>,
    terminal: T,
    terminal_digest: [u8; 32],
}

impl<D: Clone + PartialEq, T: Serialize> TerminalReceipt<D, T> {
    pub fn try_from_terminal(
        admission: OperationAdmissionReceipt<D>,
        terminal: OperationTerminal<D, T>,
    ) -> Result<Self, OperationAttributionError> {
        let expected = admission.identity();
        let actual = &terminal.identity;
        if actual.operation_id != expected.operation_id {
            return Err(OperationAttributionError::OperationMismatch);
        }
        if actual.execution_scope != OperationTerminalIdentity::from(expected).execution_scope {
            return Err(OperationAttributionError::ExecutionScopeMismatch);
        }
        if actual.domain_correlation != expected.domain_correlation {
            return Err(OperationAttributionError::DomainCorrelationMismatch);
        }
        let terminal_digest = terminal_payload_digest(&terminal.terminal)
            .map_err(|_| OperationAttributionError::TerminalEncodingFailed)?;
        Ok(Self {
            admission,
            terminal_identity: terminal.identity,
            terminal: terminal.terminal,
            terminal_digest,
        })
    }
}

impl<D, T> TerminalReceipt<D, T> {
    pub fn admission(&self) -> &OperationAdmissionReceipt<D> {
        &self.admission
    }

    pub fn terminal(&self) -> &T {
        &self.terminal
    }

    pub fn terminal_digest(&self) -> &[u8; 32] {
        &self.terminal_digest
    }

    pub fn into_parts(self) -> (OperationAdmissionReceipt<D>, T) {
        (self.admission, self.terminal)
    }
}

fn terminal_payload_digest<T: Serialize>(terminal: &T) -> Result<[u8; 32], serde_json::Error> {
    let encoded = serde_json::to_vec(terminal)?;
    Ok(Sha256::digest(encoded).into())
}

#[derive(Deserialize)]
struct TerminalReceiptWire<D, T> {
    admission: OperationAdmissionReceipt<D>,
    terminal_identity: OperationTerminalIdentity<D>,
    terminal: T,
    terminal_digest: [u8; 32],
}

impl<'de, D, T> Deserialize<'de> for TerminalReceipt<D, T>
where
    D: Clone + PartialEq + Deserialize<'de>,
    T: Serialize + Deserialize<'de>,
{
    fn deserialize<De>(deserializer: De) -> Result<Self, De::Error>
    where
        De: Deserializer<'de>,
    {
        let wire = TerminalReceiptWire::deserialize(deserializer)?;
        let receipt = Self::try_from_terminal(
            wire.admission,
            OperationTerminal::new(wire.terminal_identity, wire.terminal),
        )
        .map_err(De::Error::custom)?;
        if receipt.terminal_digest != wire.terminal_digest {
            return Err(De::Error::custom(
                "terminal receipt digest does not match its payload",
            ));
        }
        Ok(receipt)
    }
}

/// Mechanical waiter failure that retains exact admitted identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationWaitError<D, E> {
    admission: OperationAdmissionReceipt<D>,
    error: E,
}

impl<D, E> OperationWaitError<D, E> {
    pub fn new(admission: OperationAdmissionReceipt<D>, error: E) -> Self {
        Self { admission, error }
    }

    pub fn admission(&self) -> &OperationAdmissionReceipt<D> {
        &self.admission
    }

    pub fn error(&self) -> &E {
        &self.error
    }

    pub fn into_parts(self) -> (OperationAdmissionReceipt<D>, E) {
        (self.admission, self.error)
    }
}

/// Cleanup truth bound to the exact operation it follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupReceipt<D, C> {
    identity: ExactOperationIdentity<D>,
    cleanup: C,
}

impl<D, C> CleanupReceipt<D, C> {
    pub fn new(identity: ExactOperationIdentity<D>, cleanup: C) -> Self {
        Self { identity, cleanup }
    }

    pub fn identity(&self) -> &ExactOperationIdentity<D> {
        &self.identity
    }

    pub fn cleanup(&self) -> &C {
        &self.cleanup
    }

    pub fn into_inner(self) -> C {
        self.cleanup
    }
}

/// Exact terminal truth composed with an independent cleanup receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperationCompletion<D, T, C> {
    terminal: TerminalReceipt<D, T>,
    cleanup: CleanupReceipt<D, C>,
}

#[derive(Deserialize)]
#[serde(bound(
    deserialize = "D: Clone + PartialEq + Deserialize<'de>, T: Serialize + Deserialize<'de>, C: Deserialize<'de>"
))]
struct OperationCompletionWire<D, T, C> {
    terminal: TerminalReceipt<D, T>,
    cleanup: CleanupReceipt<D, C>,
}

impl<'de, D, T, C> Deserialize<'de> for OperationCompletion<D, T, C>
where
    D: Clone + PartialEq + Deserialize<'de>,
    T: Serialize + Deserialize<'de>,
    C: Deserialize<'de>,
{
    fn deserialize<De>(deserializer: De) -> Result<Self, De::Error>
    where
        De: Deserializer<'de>,
    {
        let wire = OperationCompletionWire::deserialize(deserializer)?;
        Self::try_new(wire.terminal, wire.cleanup).map_err(De::Error::custom)
    }
}

impl<D: PartialEq, T, C> OperationCompletion<D, T, C> {
    pub fn try_new(
        terminal: TerminalReceipt<D, T>,
        cleanup: CleanupReceipt<D, C>,
    ) -> Result<Self, OperationCompletionAttributionError> {
        if terminal.admission.identity != cleanup.identity {
            return Err(OperationCompletionAttributionError::CleanupIdentityMismatch);
        }
        Ok(Self { terminal, cleanup })
    }

    pub fn terminal(&self) -> &TerminalReceipt<D, T> {
        &self.terminal
    }

    pub fn cleanup(&self) -> &CleanupReceipt<D, C> {
        &self.cleanup
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum OperationCompletionAttributionError {
    #[error("cleanup receipt identity does not match the exact operation terminal")]
    CleanupIdentityMismatch,
}

/// Validated receiver-owned projection for one terminal text value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidatedResultProjectionSpec {
    label: String,
    max_text_bytes: usize,
    protocol_safe_text_ceiling_bytes: usize,
}

#[derive(Deserialize)]
struct ResultProjectionWire {
    label: String,
    max_text_bytes: usize,
    protocol_safe_text_ceiling_bytes: usize,
}

impl<'de> Deserialize<'de> for ValidatedResultProjectionSpec {
    fn deserialize<De>(deserializer: De) -> Result<Self, De::Error>
    where
        De: Deserializer<'de>,
    {
        let wire = ResultProjectionWire::deserialize(deserializer)?;
        Self::new(
            wire.label,
            wire.max_text_bytes,
            wire.protocol_safe_text_ceiling_bytes,
        )
        .map_err(De::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ResultProjectionValidationError {
    #[error("result label must not be empty")]
    EmptyLabel,
    #[error("maximum text bytes must be greater than zero")]
    EmptyTextBudget,
    #[error("result projection exceeds the adapter-proved protocol-safe text ceiling")]
    ProtocolSafeTextCeilingExceeded,
}

impl ValidatedResultProjectionSpec {
    /// Validate a receiver-owned logical text budget before admission.
    ///
    /// The adapter must derive `protocol_safe_text_ceiling_bytes` from its
    /// canonical encoder, including label encoding, escaping, fixed fields,
    /// and the actual durable wire or journal ceiling. Core deliberately does
    /// not infer encoded size from raw UTF-8 lengths.
    pub fn new(
        label: impl Into<String>,
        max_text_bytes: usize,
        protocol_safe_text_ceiling_bytes: usize,
    ) -> Result<Self, ResultProjectionValidationError> {
        let label = label.into();
        if label.is_empty() {
            return Err(ResultProjectionValidationError::EmptyLabel);
        }
        if max_text_bytes == 0 {
            return Err(ResultProjectionValidationError::EmptyTextBudget);
        }
        if max_text_bytes > protocol_safe_text_ceiling_bytes {
            return Err(ResultProjectionValidationError::ProtocolSafeTextCeilingExceeded);
        }
        Ok(Self {
            label,
            max_text_bytes,
            protocol_safe_text_ceiling_bytes,
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn max_text_bytes(&self) -> usize {
        self.max_text_bytes
    }

    pub fn protocol_safe_text_ceiling_bytes(&self) -> usize {
        self.protocol_safe_text_ceiling_bytes
    }

    pub fn project_text(&self, text: &str) -> ProjectedTerminalText {
        if text.len() <= self.max_text_bytes {
            return ProjectedTerminalText {
                text: text.to_string(),
                truncated: false,
            };
        }
        let mut end = self.max_text_bytes;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        ProjectedTerminalText {
            text: text[..end].to_string(),
            truncated: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectedTerminalText {
    pub text: String,
    pub truncated: bool,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn identity(domain_correlation: u64) -> ExactOperationIdentity<u64> {
        let canonical = InputId::new();
        ExactOperationIdentity::for_runtime_input(
            OperationId::new(),
            SessionId::new(),
            RuntimeEpochId::new(),
            canonical.clone(),
            canonical,
            domain_correlation,
        )
    }

    #[test]
    fn hostile_projection_json_cannot_bypass_validation() {
        let invalid = r#"{"label":"x","max_text_bytes":9,"protocol_safe_text_ceiling_bytes":8}"#;
        assert!(serde_json::from_str::<ValidatedResultProjectionSpec>(invalid).is_err());
    }

    #[test]
    fn hostile_terminal_receipt_json_cannot_change_payload_without_digest() {
        let exact_identity = identity(7);
        let admission = OperationAdmissionReceipt::new(
            exact_identity.clone(),
            OperationAcceptClass::Fresh,
            None,
        );
        let receipt = TerminalReceipt::try_from_terminal(
            admission,
            OperationTerminal::new(
                OperationTerminalIdentity::from(&exact_identity),
                "first".to_string(),
            ),
        )
        .expect("valid receipt");
        let mut wire = serde_json::to_value(receipt).expect("serialize receipt");
        wire["terminal"] = serde_json::Value::String("forged".to_string());
        assert!(serde_json::from_value::<TerminalReceipt<u64, String>>(wire).is_err());
    }

    #[test]
    fn hostile_completion_json_cannot_pair_cleanup_from_another_operation() {
        let exact_identity = identity(8);
        let admission = OperationAdmissionReceipt::new(
            exact_identity.clone(),
            OperationAcceptClass::Fresh,
            None,
        );
        let terminal = TerminalReceipt::try_from_terminal(
            admission,
            OperationTerminal::new(
                OperationTerminalIdentity::from(&exact_identity),
                "done".to_string(),
            ),
        )
        .expect("valid receipt");
        let completion = OperationCompletion::try_new(
            terminal,
            CleanupReceipt::new(exact_identity, "clean".to_string()),
        )
        .expect("valid completion");
        let mut wire = serde_json::to_value(completion).expect("serialize completion");
        wire["cleanup"]["identity"]["operation_id"] =
            serde_json::Value::String(OperationId::new().to_string());
        assert!(serde_json::from_value::<OperationCompletion<u64, String, String>>(wire).is_err());
    }

    fn admission(domain_correlation: u64) -> OperationAdmissionReceipt<u64> {
        OperationAdmissionReceipt::new(
            identity(domain_correlation),
            OperationAcceptClass::Fresh,
            None,
        )
    }

    #[test]
    fn terminal_requires_every_exact_identity_atom() {
        let expected = admission(7);
        let mut cases = Vec::new();

        let mut wrong = OperationTerminalIdentity::from(expected.identity());
        wrong.operation_id = OperationId::new();
        cases.push((wrong, OperationAttributionError::OperationMismatch));

        let wrong = OperationTerminalIdentity::for_runtime_input(
            expected.identity().operation_id().clone(),
            SessionId::new(),
            RuntimeEpochId::new(),
            InputId::new(),
            *expected.identity().domain_correlation(),
        );
        cases.push((wrong, OperationAttributionError::ExecutionScopeMismatch));

        let mut wrong = OperationTerminalIdentity::from(expected.identity());
        wrong.domain_correlation = 8;
        cases.push((wrong, OperationAttributionError::DomainCorrelationMismatch));

        for (actual, expected_error) in cases {
            assert_eq!(
                TerminalReceipt::try_from_terminal(
                    expected.clone(),
                    OperationTerminal::new(actual, "done"),
                ),
                Err(expected_error),
            );
        }

        let domain_admission = OperationAdmissionReceipt::new(
            ExactOperationIdentity::for_domain(OperationId::new(), 33),
            OperationAcceptClass::Fresh,
            None,
        );
        let domain_terminal = OperationTerminal::new(
            OperationTerminalIdentity::from(domain_admission.identity()),
            "domain-done",
        );
        assert_eq!(
            TerminalReceipt::try_from_terminal(domain_admission, domain_terminal)
                .expect("domain terminal needs no fabricated runtime identity")
                .terminal(),
            &"domain-done"
        );
    }

    #[test]
    fn durable_receipts_round_trip_with_typed_domain_terminals() {
        let admission = admission(9);
        let receipt = TerminalReceipt::try_from_terminal(
            admission.clone(),
            OperationTerminal::new(
                OperationTerminalIdentity::from(admission.identity()),
                Result::<String, u16>::Err(42),
            ),
        )
        .expect("exact terminal");
        let completion = OperationCompletion::try_new(
            receipt,
            CleanupReceipt::new(admission.identity().clone(), Err::<(), _>(17u8)),
        )
        .expect("matching cleanup identity");
        let encoded = serde_json::to_vec(&completion).expect("serialize completion");
        let decoded: OperationCompletion<u64, Result<String, u16>, Result<(), u8>> =
            serde_json::from_slice(&encoded).expect("rehydrate completion");
        assert_eq!(decoded, completion);
        assert_eq!(decoded.terminal().terminal(), &Err(42));
        assert_eq!(decoded.cleanup().cleanup(), &Err(17));
    }

    #[test]
    fn wait_failure_preserves_complete_admission_receipt() {
        let expected = admission(11);
        let failure = OperationWaitError::new(expected.clone(), "channel closed");
        assert_eq!(failure.admission(), &expected);
    }

    #[test]
    fn projection_requires_adapter_proved_ceiling_before_utf8_safe_projection() {
        assert_eq!(
            ValidatedResultProjectionSpec::new("label", 6, 5),
            Err(ResultProjectionValidationError::ProtocolSafeTextCeilingExceeded)
        );
        let spec = ValidatedResultProjectionSpec::new("x", 5, 5)
            .expect("projection fits the adapter-proved ceiling");
        assert_eq!(
            spec.project_text("aååå"),
            ProjectedTerminalText {
                text: "aåå".to_string(),
                truncated: true,
            }
        );
    }

    #[test]
    fn projection_does_not_claim_raw_json_lengths_are_protocol_safe() {
        let label = "quoted\"\nlabel";
        let text = "control\u{0001}\"text";
        let raw_bytes = label.len() + text.len();
        let encoded_bytes = serde_json::to_vec(&(label, text))
            .expect("representative canonical JSON encoding")
            .len();
        assert!(encoded_bytes > raw_bytes);
        let adapter_proved_text_ceiling = 4;
        assert_eq!(
            ValidatedResultProjectionSpec::new(label, 5, adapter_proved_text_ceiling),
            Err(ResultProjectionValidationError::ProtocolSafeTextCeilingExceeded)
        );
    }

    #[test]
    fn completion_rejects_cleanup_from_another_exact_operation() {
        let admitted = admission(21);
        let terminal = TerminalReceipt::try_from_terminal(
            admitted.clone(),
            OperationTerminal::new(OperationTerminalIdentity::from(admitted.identity()), "done"),
        )
        .expect("exact terminal");
        let other = admission(22);
        assert_eq!(
            OperationCompletion::try_new(
                terminal,
                CleanupReceipt::new(other.identity().clone(), "cleaned"),
            ),
            Err(OperationCompletionAttributionError::CleanupIdentityMismatch)
        );
    }
}
