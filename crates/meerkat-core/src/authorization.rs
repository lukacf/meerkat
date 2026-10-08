//! Feature-owned authorization at existing operation boundaries.
//!
//! The selected feature implements the per-work policy context. Core
//! retains an immutable binding with the actual prepared request; it does not
//! resolve policy from session metadata or maintain an authority registry.
//!
//! This module defines no turn, run, or session disposition for a refusal.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::auth::PrincipalRef;
use crate::blob::BlobRef;
use crate::comms::StreamScope;
use crate::connection::AuthCredentialIdentity;
use crate::exact_operation::OperationExecutionScope;
use crate::lifecycle::RunId;
use crate::live_execution::{CanonicalContextRevision, LiveChannelId};
use crate::memory::{MemorySearchScope, MessageRange};
use crate::ops::OperationId;
use crate::session::SessionLlmIdentity;
use crate::tool_execution::ResolvedToolExecutionPlan;
use crate::types::{SessionId, ToolCallView, ToolName};

mod entry;
pub use entry::PreparedOperationCheck;
mod audit;
pub use audit::{
    OperationObservation, OperationObservationError, OperationObservationPhase,
    OperationObservedOutcome,
};

/// Why an input or session operation cannot obtain current runtime authority.
/// No variant is a permission verdict or a terminal result for existing work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ControllerReadinessFailure {
    #[error("current controller policy is unavailable")]
    PolicyUnavailable,
    /// No current executor support is available for governed input.
    #[error("governed executor support is unavailable")]
    ExecutorUnavailable,
    /// The selected client cannot report its actual plain model route.
    #[error("selected controller cannot provide plain model facts")]
    FactsUnavailable,
    #[error("controller credential custody is busy")]
    Busy,
    #[error("controller credential authority is unavailable")]
    AuthorityUnavailable,
    #[error("controller credential authority changed")]
    AuthorityChanged,
    #[error("controller credential custody is unsupported for this scope")]
    UnsupportedScope,
    #[error("controller credential is not currently usable")]
    CredentialUnusable {
        disposition: crate::handles::CredentialUseDisposition,
    },
    /// Provider-neutral failure from existing-credential preparation. Provider
    /// diagnostics and secrets never cross this admission error boundary.
    #[error("controller credential preparation failed: {kind:?}")]
    CredentialPreparationFailed { kind: crate::auth::AuthErrorKind },
    #[error("controller policy observation must be refreshed")]
    PolicyChanged,
    #[error("replacement controller authority must be an empty owner")]
    ReplacementNotEmpty,
}

/// A domain owner's external resource or physical destination coordinates.
///
/// These are facts supplied by the existing owner, not a second resource
/// registry. The feature validates qualification and maps them into its own
/// action/resource/audience contract. Namespace and id are opaque, exact values.
#[derive(Clone)]
pub struct OwnerQualifiedTarget {
    pub authority: PrincipalRef,
    pub namespace: Arc<str>,
    pub id: Arc<str>,
}

/// Actual selected model route, including its nonsecret credential identity.
/// The adapter supplies these after fallback/override resolution. An endpoint
/// must not contain a credential; authentication material stays in its owner.
#[derive(Clone)]
pub struct ModelAuthorizationFacts {
    pub identity: Arc<SessionLlmIdentity>,
    /// Exact model value lowered by the provider, which can differ from a
    /// factory model alias. It is not inferred from a response or display name.
    pub wire_model: Arc<str>,
    /// Provider-hosted capabilities actually enabled on this request. Their
    /// presence is an operation fact, not permission to enable them.
    pub hosted_capabilities: Arc<[crate::ServerToolKind]>,
    pub backend_profile_id: Option<Arc<str>>,
    pub backend_kind: Arc<str>,
    pub endpoint: Arc<str>,
    pub credential: Option<AuthCredentialIdentity>,
    pub usage: ModelAuthorizationUse,
    pub live_channel: Option<LiveChannelId>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ModelAuthorizationUse {
    /// An ordinary requested model operation, including an alternate target.
    Inference,
    /// A request for the admitted controller. This label alone grants nothing:
    /// the owner also requires the exact retained selection and controller
    /// lineage, then evaluates the actual final provider operation facts.
    ControllerInference,
    Compaction,
    Live,
}

/// Actual tool arguments are input data, not a policy JSON namespace.
#[derive(Clone)]
pub struct ToolAuthorizationFacts {
    pub call_id: Arc<str>,
    pub name: ToolName,
    pub arguments: Arc<serde_json::value::RawValue>,
    pub target: ToolAuthorizationTarget,
}

impl ToolAuthorizationFacts {
    pub fn call(&self) -> ToolCallView<'_> {
        ToolCallView {
            id: &self.call_id,
            name: self.name.as_str(),
            args: &self.arguments,
        }
    }
}

/// Provider-hosted tools do not traverse the native dispatcher plan seam.
#[derive(Clone)]
pub enum ToolAuthorizationTarget {
    Dispatcher(Arc<ResolvedToolExecutionPlan>),
    ProviderHosted(ModelAuthorizationFacts),
}

#[derive(Clone)]
pub enum SourceAuthorizationTarget {
    Blob(BlobRef),
    Memory(MemorySearchScope),
    /// The actual native owner's retained original input, not a transcript
    /// range or a source-owned work reference. The policy owner must map this
    /// explicit resource family; unknown targets never inherit permission.
    RuntimeInput {
        owner_session_id: SessionId,
        runtime_epoch_id: crate::RuntimeEpochId,
        input_id: crate::InputId,
    },
    Transcript {
        session_id: SessionId,
        range: MessageRange,
    },
    External(OwnerQualifiedTarget),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SourceAuthorizationUse {
    Read,
    Hydrate,
    Retain,
}

#[derive(Clone)]
pub struct SourceAuthorizationFacts {
    pub target: SourceAuthorizationTarget,
    pub usage: SourceAuthorizationUse,
}

/// One actual subscriber or physical destination, never a global broadcast.
#[derive(Clone)]
pub enum PublicationRecipient {
    Principal(PrincipalRef),
    Destination(OwnerQualifiedTarget),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PublicationMode {
    Buffered,
    Stream,
    Replay,
    Live,
}

#[derive(Clone)]
pub struct PublicationAuthorizationFacts {
    pub recipient: PublicationRecipient,
    pub scope: Option<StreamScope>,
    pub live_channel: Option<LiveChannelId>,
    pub mode: PublicationMode,
}

/// Facts remain correlated within one operation. A feature must not flatten
/// separate resource/action/processor/audience predicates into independent sets.
#[derive(Clone)]
pub enum AuthorizationOperation {
    Model(ModelAuthorizationFacts),
    Tool(ToolAuthorizationFacts),
    Source(SourceAuthorizationFacts),
    Publication(PublicationAuthorizationFacts),
}

/// Native operation coordinates and the exact selected sink facts.
///
/// Domain operations use the existing `OperationExecutionScope::Domain` instead
/// of fabricating runtime inputs. Missing run/transcript coordinates are explicit
/// for operations without them; a feature refuses missing facts it requires.
#[derive(Clone)]
pub struct OperationAuthorizationFacts {
    pub operation_id: OperationId,
    pub execution_scope: OperationExecutionScope,
    pub run_id: Option<RunId>,
    pub context_revision: Option<CanonicalContextRevision>,
    pub operation: AuthorizationOperation,
}

struct BoundOperation {
    facts: OperationAuthorizationFacts,
    review_attribution: Option<Arc<crate::approval::review::ReviewChildAttribution>>,
}

/// Immutable identity of one actual prepared operation, not permission.
///
/// The operation owner creates this once and retains it with the exact request
/// payload/plan through dispatch or publication. Any change to payload,
/// arguments, context, model route, account, or recipient
/// requires a fresh binding and preparation, even if other facts compare equal.
/// Recreating a binding from equal-looking facts cannot recover its identity.
/// A clone retains the same association and must not be attached to another
/// request. This is a trusted host integration contract, not a Rust sandbox.
///
/// No serialized representation or mutable access exists. Construction proves
/// neither that the supplied facts are authentic nor that an operation is
/// allowed; the owning feature must resolve those facts during preparation.
#[derive(Clone)]
pub struct PreparedAuthorizationBinding(Arc<BoundOperation>);

impl PreparedAuthorizationBinding {
    pub fn new(facts: OperationAuthorizationFacts) -> Self {
        Self(Arc::new(BoundOperation {
            facts,
            review_attribution: None,
        }))
    }

    pub(crate) fn new_review_child(
        facts: OperationAuthorizationFacts,
        attribution: crate::approval::review::ReviewChildAttribution,
    ) -> Self {
        Self(Arc::new(BoundOperation {
            facts,
            review_attribution: Some(Arc::new(attribution)),
        }))
    }

    pub fn review_attribution(&self) -> Option<&crate::approval::review::ReviewChildAttribution> {
        self.0.review_attribution.as_deref()
    }

    pub fn facts(&self) -> &OperationAuthorizationFacts {
        &self.0.facts
    }

    /// Constant-time association check; does not inspect payloads.
    pub fn same_operation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for PreparedAuthorizationBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedAuthorizationBinding([REDACTED])")
    }
}

/// Historical observation of the actual process-local publication enclosing
/// policy reads. The instance is minted once by that synchronization owner;
/// neither it nor its sequence is a principal, durable policy revision or
/// recovered permission/currentness proof. A refused preparation can stop at
/// its first failed conjunct; this observation does not claim every owner read.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PolicyPublicationObservation {
    LocalPublication { instance: uuid::Uuid, sequence: u64 },
}

impl fmt::Debug for PolicyPublicationObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PolicyPublicationObservation([REDACTED])")
    }
}

/// Result and historical observation from the same actual owner evaluation.
/// Refusal preserves its native kind. Absence never means unrestricted policy.
pub struct ObservedAuthorizationResult<T> {
    pub result: Result<T, OperationAuthorizationError>,
    pub policy: Option<PolicyPublicationObservation>,
}

impl<T> ObservedAuthorizationResult<T> {
    pub fn unobserved(result: Result<T, OperationAuthorizationError>) -> Self {
        Self {
            result,
            policy: None,
        }
    }

    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> ObservedAuthorizationResult<U> {
        ObservedAuthorizationResult {
            result: self.result.map(map),
            policy: self.policy,
        }
    }
}

impl<T> fmt::Debug for ObservedAuthorizationResult<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ObservedAuthorizationResult([REDACTED])")
    }
}

/// Immutable feature-owned context attached to one admitted work item.
///
/// Implementations resolve current owner policy against all correlated operation
/// facts once, without a network round trip. Construction of an
/// implementation is the actual feature's responsibility, not a core registry.
/// This trait and its result are never persisted or reconstructed by serde.
pub trait WorkAuthorization: Send + Sync {
    /// Read complete review material through this retained work owner.
    /// Caller-provided coordinates cannot choose another input. Implementations
    /// separately authorize every source and recheck native custody after waits.
    /// This optional R2 path is never called by ordinary R1 preparation/checks.
    fn read_review_context<'a>(
        &'a self,
        _binding: &'a PreparedAuthorizationBinding,
        _attribution: Option<&'a crate::approval::review::ReviewOperationAttribution>,
    ) -> crate::approval::review::ReviewContextFuture<'a> {
        Box::pin(async { Err(OperationAuthorizationError::Unavailable) })
    }

    /// The native work owner's admitted controller selection, if supported.
    /// This is data for matching the actual selected client, never permission
    /// to construct a client or infer an external account from credentials.
    fn controller_model_selection(&self) -> Option<crate::ControllerModelSelection> {
        None
    }

    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError>;

    /// Preserve the publication from this exact preparation, including a
    /// coherent refusal. Legacy owners report only what their decision retains.
    fn prepare_observed(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> ObservedAuthorizationResult<Arc<dyn PreparedOperationAuthorization>> {
        let result = self.prepare(binding);
        let policy = result
            .as_ref()
            .ok()
            .and_then(|decision| decision.policy_observation());
        ObservedAuthorizationResult { result, policy }
    }
}

struct WorkAuthorizationContextInner {
    authorization: Arc<dyn WorkAuthorization>,
    execution_scope: OperationExecutionScope,
    controller_client: Option<crate::ControllerModelClient>,
    retained_work: Option<crate::retained_work::RetainedWorkIdentity>,
}

/// Process-local authorization context for one admitted work association.
///
/// The native owner supplies the exact execution scope. Construction does not
/// authenticate the association or grant permission. There is no default scope
/// and no persisted representation. Each operation owner supplies the actual
/// operation facts when preparing the exact check.
#[derive(Clone)]
pub struct WorkAuthorizationContext(Arc<WorkAuthorizationContextInner>);

impl PartialEq for WorkAuthorizationContext {
    fn eq(&self, other: &Self) -> bool {
        self.same_context(other)
    }
}

impl Eq for WorkAuthorizationContext {}

impl WorkAuthorizationContext {
    pub fn new(
        authorization: Arc<dyn WorkAuthorization>,
        execution_scope: OperationExecutionScope,
    ) -> Self {
        Self(Arc::new(WorkAuthorizationContextInner {
            authorization,
            execution_scope,
            controller_client: None,
            retained_work: None,
        }))
    }

    /// Attach the actual selected client retained by native admission. A wire
    /// selection cannot construct this runnable handle. The native host must
    /// obtain it before accepting work; this check only verifies that its exact
    /// selection agrees with the admitted owner and the retained client.
    pub fn with_controller_client(
        self,
        controller_client: crate::ControllerModelClient,
    ) -> Result<Self, OperationRefused> {
        let selected = controller_client.selection();
        if self.0.authorization.controller_model_selection().as_ref() != Some(selected)
            || controller_client
                .client()
                .controller_model_selection()
                .as_ref()
                != Some(selected)
        {
            return Err(OperationRefused::new(OperationRefusalKind::MalformedFacts));
        }
        Ok(Self(Arc::new(WorkAuthorizationContextInner {
            authorization: Arc::clone(&self.0.authorization),
            execution_scope: self.0.execution_scope.clone(),
            controller_client: Some(controller_client),
            retained_work: self.0.retained_work.clone(),
        })))
    }

    /// Record the identity of the staged run this context authorizes. The
    /// native owner attaches it from the batch it staged; it is identity data
    /// for a later resume to be matched against, never a permission.
    #[must_use]
    pub fn with_retained_work(self, identity: crate::retained_work::RetainedWorkIdentity) -> Self {
        Self(Arc::new(WorkAuthorizationContextInner {
            authorization: Arc::clone(&self.0.authorization),
            execution_scope: self.0.execution_scope.clone(),
            controller_client: self.0.controller_client.clone(),
            retained_work: Some(identity),
        }))
    }

    /// Identity of the staged run this context authorizes, when the native
    /// owner recorded one.
    pub fn retained_work(&self) -> Option<&crate::retained_work::RetainedWorkIdentity> {
        self.0.retained_work.as_ref()
    }

    pub fn controller_client(&self) -> Option<&crate::ControllerModelClient> {
        self.0.controller_client.as_ref()
    }

    pub fn authorization(&self) -> &Arc<dyn WorkAuthorization> {
        &self.0.authorization
    }

    pub fn execution_scope(&self) -> &OperationExecutionScope {
        &self.0.execution_scope
    }

    /// Identity of the retained context, never an authorization decision.
    pub fn same_context(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for WorkAuthorizationContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WorkAuthorizationContext([REDACTED])")
    }
}

/// Prepared decision bound to the immutable operation and coherent owner state.
///
/// An implementation retains the exact binding it evaluated. `check_current`
/// first checks its identity with `same_operation`, then its captured owner
/// publication and deadline. The synchronous warm check must be allocation-free
/// and O(1): no resource traversal, argument/payload hashing, serialization, policy
/// rebuild, blocking I/O, network, or durable write. A changed owner publication
/// or operation identity returns `ReprepareRequired`, never implicit permission.
///
/// The actual sink owner performs this check at entry/release under its declared
/// local ordering. This interface alone supplies neither that ordering nor a
/// one-use execution claim, and does not settle or relabel completed effects.
pub trait PreparedOperationAuthorization: Send + Sync {
    /// The immutable observation captured by this decision. This must never
    /// resample a current counter or relabel an older policy evaluation. Return
    /// stored Copy data only, without allocation, owner reads or traversal.
    fn policy_observation(&self) -> Option<PolicyPublicationObservation> {
        None
    }

    /// Review tier the policy owner resolved for this exact decision, under
    /// the same publication as its permission. Required, with no default: an
    /// owner or wrapper that forgets it fails to compile instead of silently
    /// implying R1. Wrappers delegate; return stored Copy data only.
    fn review_tier(&self) -> OperationReviewTier;

    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), crate::OperationAuthorizationError>;

    /// Stage a protected native observation at an actual boundary. This is
    /// separate from the allocation-free currentness check. Governed native
    /// contexts install a real input-row sink; this compatibility default is
    /// for extensions and tests and makes no authoritative-audit claim.
    fn observe(
        &self,
        _binding: &PreparedAuthorizationBinding,
        _observation: OperationObservation,
    ) -> Result<(), OperationObservationError> {
        Ok(())
    }
}

/// Review tier the policy owner requires for one exact prepared operation,
/// resolved under the same publication as its permission (ADR-001).
///
/// Ordered by strictness, so combining owners keeps the strictest tier.
/// Every tier retains all native permission, delegation, account, resource
/// and confinement checks; R1 adds no review, it never widens permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationReviewTier {
    /// No additional review; native permission and entry checks suffice.
    R1,
    /// One bound reviewer decision for this exact candidate.
    R2,
    /// Fresh qualified human decision; a model allow never satisfies it.
    R3,
}

/// Internal disposition of the affected operation, not a turn/run disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum OperationRefusalKind {
    ReprepareRequired,
    Denied,
    MalformedFacts,
}

/// Audience-safe refusal with a typed owner-facing disposition.
/// No private class, source, principal, sink or policy explanation is carried.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OperationRefused {
    kind: OperationRefusalKind,
}

impl OperationRefused {
    pub const fn new(kind: OperationRefusalKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> OperationRefusalKind {
        self.kind
    }
}

impl fmt::Display for OperationRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("operation unavailable under current authorization")
    }
}

impl fmt::Debug for OperationRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OperationRefused([REDACTED])")
    }
}

impl std::error::Error for OperationRefused {}

/// An operation refusal, unavailable authority, or failed observation.
/// None of these variants authorizes retry or bypassing the actual owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OperationAuthorizationError {
    /// Current authoritative facts could not be obtained; no policy verdict.
    #[error("operation authorization unavailable")]
    Unavailable,
    #[error(transparent)]
    Refused(#[from] OperationRefused),
    #[error(transparent)]
    ObservationUnavailable(#[from] OperationObservationError),
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn binding_fixture() -> PreparedAuthorizationBinding {
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: OperationId(uuid::Uuid::nil()),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(
                    SessionId::from_uuid(uuid::Uuid::nil()),
                )),
                usage: SourceAuthorizationUse::Read,
            }),
        })
    }

    #[test]
    fn binding_clone_preserves_identity_but_equal_facts_cannot_recreate_it() {
        let first = binding_fixture();
        let retained = first.clone();
        let reconstructed = PreparedAuthorizationBinding::new(first.facts().clone());
        assert!(first.same_operation(&retained));
        assert!(!first.same_operation(&reconstructed));
    }

    #[test]
    fn public_diagnostics_do_not_disclose_facts_counts_or_refusal_class() {
        let binding = binding_fixture();
        assert_eq!(
            format!("{binding:?}"),
            "PreparedAuthorizationBinding([REDACTED])"
        );
        for kind in [
            OperationRefusalKind::ReprepareRequired,
            OperationRefusalKind::Denied,
            OperationRefusalKind::MalformedFacts,
        ] {
            let refusal = OperationRefused::new(kind);
            assert!(refusal.kind() == kind);
            assert_eq!(
                refusal.to_string(),
                "operation unavailable under current authorization"
            );
            assert_eq!(format!("{refusal:?}"), "OperationRefused([REDACTED])");
        }
    }

    #[test]
    fn unavailable_final_check_observes_unavailable_and_preserves_append_failure() {
        use std::sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        };
        struct Check {
            binding: PreparedAuthorizationBinding,
            unavailable: Arc<AtomicBool>,
            append_fails: Arc<AtomicBool>,
            observed: Arc<Mutex<Vec<OperationObservation>>>,
        }
        impl PreparedOperationAuthorization for Check {
            fn review_tier(&self) -> crate::authorization::OperationReviewTier {
                crate::authorization::OperationReviewTier::R1
            }

            fn check_current(
                &self,
                binding: &PreparedAuthorizationBinding,
            ) -> Result<(), OperationAuthorizationError> {
                assert!(self.binding.same_operation(binding));
                if self.unavailable.load(Ordering::SeqCst) {
                    Err(OperationAuthorizationError::Unavailable)
                } else {
                    Ok(())
                }
            }
            fn observe(
                &self,
                binding: &PreparedAuthorizationBinding,
                event: OperationObservation,
            ) -> Result<(), OperationObservationError> {
                assert!(self.binding.same_operation(binding));
                self.observed.lock().unwrap().push(event);
                if self.append_fails.load(Ordering::SeqCst) {
                    Err(OperationObservationError)
                } else {
                    Ok(())
                }
            }
        }
        struct Work(Arc<Check>);
        impl WorkAuthorization for Work {
            fn prepare(
                &self,
                binding: &PreparedAuthorizationBinding,
            ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError>
            {
                assert!(self.0.binding.same_operation(binding));
                Ok(self.0.clone())
            }
        }
        let binding = binding_fixture();
        let state = Arc::new(Check {
            binding: binding.clone(),
            unavailable: Arc::new(AtomicBool::new(false)),
            append_fails: Arc::new(AtomicBool::new(false)),
            observed: Arc::new(Mutex::new(vec![])),
        });
        let context = WorkAuthorizationContext::new(
            Arc::new(Work(state.clone())),
            OperationExecutionScope::Domain,
        );
        let check = PreparedOperationCheck::prepare(context, binding).unwrap();
        check.current().unwrap();
        assert!(
            state.observed.lock().unwrap().is_empty(),
            "healthy warm check is append-free"
        );
        state.unavailable.store(true, Ordering::SeqCst);
        assert!(matches!(
            check.current(),
            Err(OperationAuthorizationError::Unavailable)
        ));
        state.append_fails.store(true, Ordering::SeqCst);
        assert!(matches!(
            check.current(),
            Err(OperationAuthorizationError::ObservationUnavailable(_))
        ));
        let observed = state.observed.lock().unwrap();
        assert_eq!(observed.len(), 2);
        assert!(
            observed
                .iter()
                .all(|event| matches!(event, OperationObservation::AuthorizationUnavailable))
        );
    }
}
