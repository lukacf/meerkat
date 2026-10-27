//! Concrete request validation before the setup composition owner admits work.
//!
//! Wording is preserved, not resolved into integrations. This boundary does not
//! create an operation, spawn agents, admit effects or establish role readiness.

use crate::authority::{
    Action, SetupContextError, SetupProposalConstraints, SetupRequestEvidence, SetupUseContext,
};
use meerkat_authorization_contracts::work_association::OriginalWorkRef;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt, io};
use toolkit_package::{ContentDigest, Id, Revision, canonical_json};

const MAX_INLINE_BYTES: usize = 1024 * 1024;
const MAX_CONTEXT_ITEMS: usize = 256;

/// The entire model-facing input. Authority and routing fields cannot be added
/// through JSON. Context and observations remain untrusted content.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindAndInstallRequest {
    pub request: String,
    #[serde(default)]
    pub user_context: Vec<RequestContext>,
    #[serde(default)]
    pub caller_observations: Vec<String>,
}

impl fmt::Debug for FindAndInstallRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FindAndInstallRequest")
            .field("content", &"[redacted]")
            .field("context_items", &self.user_context.len())
            .field("caller_observations", &self.caller_observations.len())
            .finish()
    }
}

/// Inline strings preserve the simple tool-call shape. Larger material can use
/// exact host-owned references. Possessing a reference does not grant access.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestContext {
    Text(String),
    Reference(ContextReference),
}

impl fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(_) => f.write_str("Text([redacted])"),
            Self::Reference(_) => f.write_str("Reference([redacted])"),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextReference {
    pub owner: Id,
    pub id: Id,
    pub revision: Revision,
    pub content_digest: ContentDigest,
}

/// Trusted composition port. Implementations must check the exact reference
/// against current host authority before any content is exposed to a helper.
/// No default implementation treats a model-supplied ID as authorization.
pub trait ContextReferenceOwner: Send + Sync {
    fn verify_access(
        &self,
        reference: &ContextReference,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
    ) -> Result<(), RequestError>;
}

/// Historical native identity cloned from the actual original tool invocation.
/// The installed adapter selects `member` from its seated native roster, or
/// retains a standalone session destination. These are identity facts only:
/// native continuation admission rechecks the original session and full work.
/// Deserialization restores no permission and never creates a native receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeOriginalTask {
    pub origin_session_id: meerkat_core::SessionId,
    pub retained_work: meerkat_core::retained_work::RetainedWorkIdentity,
    pub member: Option<meerkat_core::MobMemberBinding>,
}

/// Exact continuation identity from the original task's owner. This is not a
/// model-authored destination or permission to send to that task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskContinuation {
    pub owner: Id,
    /// Compatibility/display projection. Native delivery uses `native_original`
    /// in full; this label cannot reconstruct a missing native work binding.
    pub task: Id,
    /// Compatibility/display projection, not the native work identity.
    pub revision: Revision,
    pub delivery_key: Id,
    /// Exact intended recipients. The authority ceiling is only an upper bound.
    pub audience: BTreeSet<Id>,
    /// Required by the governed installed adapter. Older/generic records may
    /// decode without it but cannot be submitted as native retained work.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_original: Option<NativeOriginalTask>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetupLimits {
    pub deadline_ms: u64,
    pub max_workers: u16,
    pub max_effect_attempts: u16,
}

/// Supplied by the authenticated host adapter, never deserialized from tool
/// input. The host owns correlation, original-task routing and resource limits.
/// Correlation is unique for one logical invocation in the qualified native original's
/// namespace, with tenant/deployment checked against current trusted placement.
/// A new authority requires a new admission; it must not silently mint a new
/// correlation for a lost-response retry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostSetupRequest {
    pub target: Id,
    pub correlation: Id,
    pub parent_operation: Option<Id>,
    /// Exact helper selected by the current route owner before admission.
    /// This is host configuration, never a model-provided destination.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_helper: Option<meerkat_core::MobMemberBinding>,
    pub continuation: TaskContinuation,
    pub environment_revision: Revision,
    pub limits: SetupLimits,
}

/// Value checked at a particular host boundary. It is deliberately not
/// deserializable. Admission and effect dispatch must still recheck freshness.
#[derive(Clone, Serialize)]
pub struct ValidatedSetupRequest {
    request: FindAndInstallRequest,
    host: HostSetupRequest,
    evidence: SetupRequestEvidence,
    input_digest: ContentDigest,
    correlation_digest: ContentDigest,
}

impl fmt::Debug for ValidatedSetupRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidatedSetupRequest")
            .field("input_digest", &self.input_digest)
            .field("correlation_digest", &self.correlation_digest)
            .finish_non_exhaustive()
    }
}

impl ValidatedSetupRequest {
    pub fn request(&self) -> &FindAndInstallRequest {
        &self.request
    }

    pub fn host(&self) -> &HostSetupRequest {
        &self.host
    }

    /// Historical data only, never a reconstructed current permission.
    pub fn evidence(&self) -> &SetupRequestEvidence {
        &self.evidence
    }
    pub fn constraints(&self) -> &SetupProposalConstraints {
        self.evidence.constraints()
    }
    pub fn original_work(&self) -> &OriginalWorkRef {
        self.evidence.original_work()
    }

    /// Mechanical binding for the acquisition machine, not a claims digest.
    pub fn authority_digest(&self) -> Result<ContentDigest, RequestError> {
        Ok(self.evidence.digest()?)
    }

    /// Full exact content and host contract, used to reject changed retries.
    pub fn input_digest(&self) -> &ContentDigest {
        &self.input_digest
    }

    /// Exact retry identity, not similarity, semantic selection or a fan-out
    /// sharing key. The operation owner must atomically bind this to the input.
    pub fn correlation_digest(&self) -> &ContentDigest {
        &self.correlation_digest
    }

    pub fn revalidate(
        &self,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
        references: &dyn ContextReferenceOwner,
    ) -> Result<(), RequestError> {
        if authority != &self.evidence {
            return Err(RequestError::ChangedAuthority);
        }
        validate_host(&self.host, authority, context)?;
        verify_references(&self.request, authority, context, references)
    }

    /// Check proposal identity for reading or cancelling an admitted request after its setup
    /// deadline. This does not extend that deadline or authorize new setup work.
    pub fn revalidate_access(
        &self,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
        references: &dyn ContextReferenceOwner,
    ) -> Result<(), RequestError> {
        self.revalidate_control(authority, context)?;
        verify_references(&self.request, authority, context, references)
    }

    /// Control of an admitted operation does not require reopening its old
    /// research inputs. Used for requester cancellation, never to read context
    /// or authorize installation, role grants or frontend exposure.
    pub fn revalidate_control(
        &self,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        if authority != &self.evidence {
            return Err(RequestError::ChangedAuthority);
        }
        authority.check_proposal(Action::FindAndInstall, &self.host.target, context)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error(transparent)]
    Authority(#[from] SetupContextError),
    #[error("setup request must contain the user's request")]
    EmptyRequest,
    #[error("setup input exceeds its inline limit; use authorized host context references")]
    InputLimit,
    #[error("setup limits are empty, expired, or exceed the authority lifetime")]
    InvalidLimits,
    #[error("context reference is inaccessible, stale, or has different content")]
    ContextUnavailable,
    #[error("request authority differs from the validated input")]
    ChangedAuthority,
    #[error("continuation audience is empty or exceeds the host authority ceiling")]
    InvalidAudience,
    #[error("request encoding failed")]
    Encoding(#[from] serde_json::Error),
}

pub fn validate_request(
    request: FindAndInstallRequest,
    host: HostSetupRequest,
    authority: &SetupRequestEvidence,
    context: &SetupUseContext<'_>,
    references: &dyn ContextReferenceOwner,
) -> Result<ValidatedSetupRequest, RequestError> {
    validate_host(&host, authority, context)?;
    checked_request_content(request, host, authority, context, references)
}

/// Only the persisted bridge consumes this observation candidate. A past setup
/// deadline permits exact recovery of existing work, never a fresh admission.
pub(crate) fn validate_replay_request(
    request: FindAndInstallRequest,
    host: HostSetupRequest,
    authority: &SetupRequestEvidence,
    context: &SetupUseContext<'_>,
    references: &dyn ContextReferenceOwner,
) -> Result<ValidatedSetupRequest, RequestError> {
    validate_host_access(&host, authority, context)?;
    checked_request_content(request, host, authority, context, references)
}

fn checked_request_content(
    request: FindAndInstallRequest,
    host: HostSetupRequest,
    authority: &SetupRequestEvidence,
    context: &SetupUseContext<'_>,
    references: &dyn ContextReferenceOwner,
) -> Result<ValidatedSetupRequest, RequestError> {
    validate_input_size(&request)?;
    if request.request.trim().is_empty() {
        return Err(RequestError::EmptyRequest);
    }
    verify_references(&request, authority, context, references)?;
    let correlation_digest = correlation_digest(&host, authority)?;
    let input_digest = input_digest(&request, &host, &authority.digest()?)?;
    Ok(ValidatedSetupRequest {
        request,
        host,
        evidence: authority.clone(),
        input_digest,
        correlation_digest,
    })
}
fn correlation_digest(
    host: &HostSetupRequest,
    evidence: &SetupRequestEvidence,
) -> Result<ContentDigest, serde_json::Error> {
    invocation_correlation_digest(
        evidence.association(),
        &evidence.constraints().tenant,
        &evidence.constraints().deployment,
        &host.correlation,
    )
}

/// Exact existing acquisition key projection. This is a mechanical correlation,
/// never authority or a continuation identity. Admission and lookup share it.
pub(crate) fn invocation_correlation_digest(
    association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    tenant: &Id,
    deployment: &Id,
    correlation: &Id,
) -> Result<ContentDigest, serde_json::Error> {
    let association = association.candidate();
    Ok(ContentDigest::of_bytes(&canonical_json(&(
        "toolkit-request-correlation-v3",
        &association.ingress_namespace,
        &association.requester,
        &association.original_work,
        &association.target,
        tenant,
        deployment,
        correlation,
    ))?))
}

fn input_digest(
    request: &FindAndInstallRequest,
    host: &HostSetupRequest,
    evidence_digest: &ContentDigest,
) -> Result<ContentDigest, serde_json::Error> {
    Ok(ContentDigest::of_bytes(&canonical_json(&(
        "toolkit-request-input-v3",
        request,
        host,
        evidence_digest,
    ))?))
}

/// Private storage codec. Reading it does not authenticate its saved claims.
/// Public operation boundaries still require current native host context.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredSetupRequest {
    request: FindAndInstallRequest,
    host: StoredHostRequest,
    evidence: SetupRequestEvidence,
    input_digest: ContentDigest,
    correlation_digest: ContentDigest,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredHostRequest {
    target: Id,
    correlation: Id,
    parent_operation: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    setup_helper: Option<meerkat_core::MobMemberBinding>,
    continuation: StoredContinuation,
    environment_revision: Revision,
    limits: StoredLimits,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredContinuation {
    owner: Id,
    task: Id,
    revision: Revision,
    delivery_key: Id,
    audience: BTreeSet<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_original: Option<NativeOriginalTask>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLimits {
    deadline_ms: u64,
    max_workers: u16,
    max_effect_attempts: u16,
}

impl StoredSetupRequest {
    pub(crate) fn capture(request: &ValidatedSetupRequest) -> Result<Self, serde_json::Error> {
        serde_json::from_value(serde_json::to_value(request)?)
    }

    /// Validate exact stored content before treating it as a historical request.
    /// This reconstructs no signature or current permission.
    pub(crate) fn recover(&self) -> Result<ValidatedSetupRequest, RequestError> {
        let host = HostSetupRequest {
            target: self.host.target.clone(),
            correlation: self.host.correlation.clone(),
            parent_operation: self.host.parent_operation.clone(),
            setup_helper: self.host.setup_helper.clone(),
            continuation: TaskContinuation {
                owner: self.host.continuation.owner.clone(),
                task: self.host.continuation.task.clone(),
                revision: self.host.continuation.revision.clone(),
                delivery_key: self.host.continuation.delivery_key.clone(),
                audience: self.host.continuation.audience.clone(),
                native_original: self.host.continuation.native_original.clone(),
            },
            environment_revision: self.host.environment_revision.clone(),
            limits: SetupLimits {
                deadline_ms: self.host.limits.deadline_ms,
                max_workers: self.host.limits.max_workers,
                max_effect_attempts: self.host.limits.max_effect_attempts,
            },
        };
        let claims = self.evidence.constraints();
        let evidence_digest = self.evidence.digest()?;
        validate_audience(&host.continuation, claims)?;
        validate_input_size(&self.request)?;
        if self.request.request.trim().is_empty()
            || self.input_digest != input_digest(&self.request, &host, &evidence_digest)?
            || self.correlation_digest != correlation_digest(&host, &self.evidence)?
            || host.limits.max_workers == 0
            || host.limits.max_effect_attempts == 0
            || host.limits.deadline_ms <= claims.issued_at_ms
            || host.limits.deadline_ms > claims.expires_at_ms
        {
            return Err(RequestError::ChangedAuthority);
        }
        Ok(ValidatedSetupRequest {
            request: self.request.clone(),
            host,
            evidence: self.evidence.clone(),
            input_digest: self.input_digest.clone(),
            correlation_digest: self.correlation_digest.clone(),
        })
    }
}

fn validate_input_size(request: &FindAndInstallRequest) -> Result<(), RequestError> {
    if request.user_context.len() > MAX_CONTEXT_ITEMS
        || request.caller_observations.len() > MAX_CONTEXT_ITEMS
    {
        return Err(RequestError::InputLimit);
    }

    // String lengths are a lower bound on their JSON size. Reject excessive
    // content before scanning, escaping or copying any of it, including blanks.
    let mut raw = ByteBudget::new(MAX_INLINE_BYTES);
    let mut count = |text: &str| {
        raw.consume(text.len())
            .map_err(|_| RequestError::InputLimit)
    };
    count(&request.request)?;
    for context in &request.user_context {
        match context {
            RequestContext::Text(text) => count(text)?,
            RequestContext::Reference(reference) => {
                count(reference.owner.as_str())?;
                count(reference.id.as_str())?;
                count(reference.revision.as_str())?;
                count(reference.content_digest.as_str())?;
            }
        }
    }
    for observation in &request.caller_observations {
        count(observation)?;
    }

    // Count encoded bytes without retaining them. Key ordering does not change
    // this size; canonical JSON is produced only after this exact check passes.
    validate_encoded_size(request, MAX_INLINE_BYTES)
}

fn validate_encoded_size(value: &impl Serialize, limit: usize) -> Result<(), RequestError> {
    let mut budget = ByteBudget::new(limit);
    match serde_json::to_writer(&mut budget, value) {
        Err(_) if budget.exceeded => Err(RequestError::InputLimit),
        Err(error) => Err(RequestError::Encoding(error)),
        Ok(()) => Ok(()),
    }
}

/// A counting sink with constant storage, including when encoding is rejected.
struct ByteBudget {
    remaining: usize,
    exceeded: bool,
}

impl ByteBudget {
    fn new(limit: usize) -> Self {
        Self {
            remaining: limit,
            exceeded: false,
        }
    }

    fn consume(&mut self, length: usize) -> io::Result<()> {
        if let Some(remaining) = self.remaining.checked_sub(length)
            && !self.exceeded
        {
            self.remaining = remaining;
            Ok(())
        } else {
            self.exceeded = true;
            Err(io::ErrorKind::FileTooLarge.into())
        }
    }
}

impl io::Write for ByteBudget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.consume(bytes.len())?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn validate_host(
    host: &HostSetupRequest,
    authority: &SetupRequestEvidence,
    context: &SetupUseContext<'_>,
) -> Result<(), RequestError> {
    validate_host_access(host, authority, context)?;
    if host.limits.deadline_ms <= context.now_ms {
        return Err(RequestError::InvalidLimits);
    }
    Ok(())
}

fn validate_host_access(
    host: &HostSetupRequest,
    authority: &SetupRequestEvidence,
    context: &SetupUseContext<'_>,
) -> Result<(), RequestError> {
    validate_audience(&host.continuation, authority.constraints())?;
    authority.check_proposal(Action::FindAndInstall, &host.target, context)?;
    if host.limits.max_workers == 0
        || host.limits.max_effect_attempts == 0
        || host.limits.deadline_ms <= authority.constraints().issued_at_ms
        || host.limits.deadline_ms > authority.constraints().expires_at_ms
    {
        return Err(RequestError::InvalidLimits);
    }
    Ok(())
}

fn validate_audience(
    continuation: &TaskContinuation,
    authority: &SetupProposalConstraints,
) -> Result<(), RequestError> {
    if continuation.audience.is_empty()
        || !continuation
            .audience
            .is_subset(&authority.ceiling.output_audiences)
    {
        return Err(RequestError::InvalidAudience);
    }
    Ok(())
}

fn verify_references(
    request: &FindAndInstallRequest,
    authority: &SetupRequestEvidence,
    context: &SetupUseContext<'_>,
    references: &dyn ContextReferenceOwner,
) -> Result<(), RequestError> {
    for item in &request.user_context {
        if let RequestContext::Reference(reference) = item {
            references.verify_access(reference, authority, context)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_budget_rejects_overflow_and_remains_exhausted() {
        let mut budget = ByteBudget::new(3);
        assert!(budget.consume(usize::MAX).is_err());
        assert!(budget.consume(0).is_err());
        assert_eq!(budget.remaining, 3);
        assert!(budget.exceeded);

        let mut budget = ByteBudget::new(usize::MAX);
        assert!(budget.consume(usize::MAX).is_ok());
        assert_eq!(budget.remaining, 0);
        assert!(budget.consume(1).is_err());
    }

    #[test]
    fn encoded_count_stops_before_serializing_the_rest_of_rejected_input() {
        struct Unvisited;

        impl Serialize for Unvisited {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                panic!("the encoded byte limit must stop before visiting this value");
            }
        }

        // The first field's escape sequence crosses the limit. The serializer
        // must propagate the sink's error rather than encode the complete value.
        assert!(matches!(
            validate_encoded_size(&("\0", Unvisited), 4),
            Err(RequestError::InputLimit)
        ));
    }
}
