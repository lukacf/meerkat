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
mod delegate;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use delegate::__test_mint_tool_delegate;
pub use delegate::{ToolDelegateAttribution, ToolDelegateRole};
mod audit;
pub use audit::{
    MANDATORY_HOOK_ID_BYTES, MANDATORY_HOOK_PROVIDER_NATIVE_NAME_BYTES,
    MANDATORY_HOOK_PROVIDER_NATIVE_TOOLS, MandatoryHookCause, MandatoryHookProviderTool,
    MandatoryHookWithheldEffect, MandatoryHookWithholding, MandatoryHookWithholdingRecord,
    OperationEntryBoundary, OperationObservation, OperationObservationBarrier,
    OperationObservationError, OperationObservationPhase, OperationObservedOutcome,
};
mod owner_change_wait;
pub use owner_change_wait::{
    OWNER_CHANGE_FREE_REBUILDS, OWNER_CHANGE_WAIT_BOUND, OwnerChangeChurn, OwnerChangeWait,
};
mod refusal_observer;
pub use refusal_observer::{
    OperationRefusalDisposition, OperationRefusalObservation, OperationRefusalObserver,
    OperationRefusalOutcome, OperationRefusalSurface,
};
pub(crate) use refusal_observer::{RefusalObserverBinding, WaitReport};

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
    /// An original the input or operation relies on is still its source
    /// owner's submission candidate: the owner has not settled its admission
    /// yet and will promote or withdraw it within its own bound.
    #[error("an original's admission is not yet settled by its source owner")]
    OriginalAdmissionPending,
    /// The exact live actor's turn boundary is held now (a turn or another
    /// boundary holder has it), and the caller does not wait for it: a
    /// trusted host answers this instead of queueing behind the turn. Not a
    /// verdict; asking again after the boundary is released may succeed.
    #[error("the session's turn boundary is held")]
    TurnBoundaryHeld,
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

impl ModelAuthorizationFacts {
    /// The canonical key of this request's actual lowered route
    /// ([`crate::ControllerRouteKey`]), computed exactly as at admission.
    /// `None` when the request names no credential owner or backend profile,
    /// which no admitted controller route lacks.
    pub fn controller_route_key(&self) -> Option<crate::ControllerRouteKey> {
        Some(crate::ControllerRouteKey::of(
            crate::llm_client::ControllerRouteParts {
                provider: self.identity.provider,
                model: &self.identity.model,
                self_hosted_server_id: self.identity.self_hosted_server_id.as_deref(),
                auth_binding: self.identity.auth_binding.as_ref(),
                credential: self.credential.as_ref()?,
                backend_profile_id: self.backend_profile_id.as_deref()?,
                backend_kind: &self.backend_kind,
                endpoint: &self.endpoint,
                wire_model: &self.wire_model,
            },
        ))
    }
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
    delegate_attribution: Option<ToolDelegateAttribution>,
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
            delegate_attribution: None,
        }))
    }

    pub(crate) fn new_review_child(
        facts: OperationAuthorizationFacts,
        attribution: crate::approval::review::ReviewChildAttribution,
    ) -> Self {
        Self(Arc::new(BoundOperation {
            facts,
            review_attribution: Some(Arc::new(attribution)),
            delegate_attribution: None,
        }))
    }

    /// The exact binding of a builtin tool's delegated operation, attributed
    /// to the call that entered the tool (see [`ToolDelegateAttribution`]).
    pub(crate) fn new_delegate_child(
        facts: OperationAuthorizationFacts,
        attribution: ToolDelegateAttribution,
    ) -> Self {
        Self(Arc::new(BoundOperation {
            facts,
            review_attribution: None,
            delegate_attribution: Some(attribution),
        }))
    }

    pub fn review_attribution(&self) -> Option<&crate::approval::review::ReviewChildAttribution> {
        self.0.review_attribution.as_deref()
    }

    /// The call whose builtin tool performs this operation as its delegate,
    /// and the delegate's role. Owners key the delegate's own lineage and
    /// facts on it; it is data, never a permission.
    pub fn delegate_attribution(&self) -> Option<&ToolDelegateAttribution> {
        self.0.delegate_attribution.as_ref()
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

/// Upper bound, in bytes, for each field of [`ReviewOperationContext`]. It
/// leaves room for a composed mandate at realistic sizes: a work's mandate
/// followed by the bounds its contributing originals narrow it to, or the
/// listing of every input's own mandate for a multi-input work. A field over
/// it is refused, never truncated, and its owner answers the material as
/// unavailable.
pub const MAX_REVIEW_OPERATION_CONTEXT_BYTES: usize = 64 * 1024;

/// Passive owner material for one reviewed operation: the action account the
/// operation owner actually selected and the current mandate the admitted-work
/// owner holds, read under their existing publication. It is presentation and
/// reviewer input only, never permission, a credential reference or an
/// explanation rebuilt from identifiers or history.
#[derive(Clone, PartialEq, Eq)]
pub struct ReviewOperationContext {
    action_account: Option<Arc<str>>,
    mandate: Option<Arc<str>>,
}

/// The owner supplied no field, a blank field or a field over its bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("review operation context is empty, blank or exceeds its bound")]
pub struct ReviewOperationContextInvalid;

impl ReviewOperationContext {
    /// At least one field is required; each present field must be nonblank
    /// and at most [`MAX_REVIEW_OPERATION_CONTEXT_BYTES`]. Nothing is truncated.
    pub fn new(
        action_account: Option<String>,
        mandate: Option<String>,
    ) -> Result<Self, ReviewOperationContextInvalid> {
        fn bounded(
            field: Option<String>,
        ) -> Result<Option<Arc<str>>, ReviewOperationContextInvalid> {
            match field {
                None => Ok(None),
                Some(text)
                    if text.trim().is_empty()
                        || text.len() > MAX_REVIEW_OPERATION_CONTEXT_BYTES =>
                {
                    Err(ReviewOperationContextInvalid)
                }
                Some(text) => Ok(Some(Arc::from(text))),
            }
        }
        let action_account = bounded(action_account)?;
        let mandate = bounded(mandate)?;
        if action_account.is_none() && mandate.is_none() {
            return Err(ReviewOperationContextInvalid);
        }
        Ok(Self {
            action_account,
            mandate,
        })
    }

    pub fn action_account(&self) -> Option<&str> {
        self.action_account.as_deref()
    }

    pub fn mandate(&self) -> Option<&str> {
        self.mandate.as_deref()
    }
}

impl fmt::Debug for ReviewOperationContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReviewOperationContext([REDACTED])")
    }
}

/// Outcome of reading review material. `NotApplicable` is an explicit owner
/// answer that the operation has no such material. An owner that requires
/// material it cannot read, or that does not support the read, is
/// `Unavailable` instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewOperationContextResolution {
    NotApplicable,
    Material(ReviewOperationContext),
}

/// An owner change in progress on a work's own authorization publication: a
/// token for waiting until that change completes, never a permission. A
/// published change is named by its own odd sequence: equal tokens name the
/// same change in progress, and a different token is a later change. An
/// owner may also name a hold that has not published (the native work owner
/// names every reservation of its publication's writer: an owner change
/// still being prepared, a held exclusion, another operation's final entry)
/// by the odd sequence it would publish at, so consecutive unpublished holds
/// share one token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerChangeInProgress {
    instance: uuid::Uuid,
    sequence: u64,
}

impl OwnerChangeInProgress {
    /// The token of the change in progress on publication `instance` at
    /// its odd `sequence`. Only that publication's owner mints it.
    #[must_use]
    pub const fn new(instance: uuid::Uuid, sequence: u64) -> Self {
        Self { instance, sequence }
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

    /// Passive review material for this exact prepared operation, read once
    /// after R1 is ruled out and before reviewer transport or human
    /// presentation. Synchronous and without I/O. A wrapper must forward it.
    /// Fail closed: an owner that does not implement this is `Unavailable`,
    /// so R2/R3 review settles as context unavailable. Only an owner that
    /// knows the material is genuinely inapplicable answers `NotApplicable`.
    fn review_operation_context(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<ReviewOperationContextResolution, OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
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
    /// Stage a typed historical mandatory-hook withholding for the actual
    /// model candidate in this work's protected native custody. Audit-only:
    /// it prepares nothing, re-runs no permission and looks up no provider.
    /// Work without protected custody cannot record it, so there is no
    /// successful default; a failure is infrastructure, never a denial.
    fn observe_mandatory_hook_withholding(
        &self,
        _record: &MandatoryHookWithholdingRecord,
    ) -> Result<(), OperationObservationError> {
        Err(OperationObservationError)
    }

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

    /// Run `operation` only while the exact prepared operation of
    /// `binding` is currently permitted by this work's own retained owners
    /// and inside `prepared`, the validity of the decision the caller
    /// actually holds (its window and monotonic cutoff; `None` refuses),
    /// conjoined with `further` (typed facts of a trusted native composition,
    /// for example further retained originals), holding this owner's own
    /// non-waiting exclusion of authority changes through `operation`.
    /// Wrappers check their own custody and forward; the default is
    /// unavailable. `operation` runs at most once and must not await.
    ///
    /// This is the generic composition boundary: the owner that answers is
    /// the context's own authorization chain by construction, and an
    /// implementor that does not override it refuses without running
    /// `operation`.
    fn run_if_current_within(
        &self,
        _binding: &PreparedAuthorizationBinding,
        _prepared: Option<&OperationValidity>,
        _further: &(dyn std::any::Any + Send + Sync),
        _operation: &mut dyn FnMut(),
    ) -> Result<(), OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }

    /// As `run_if_current_within`, for a further step of an effect a
    /// dispatch already entered (`crate::EnteredDispatchOperation`): the
    /// complete current decision, computed inside this owner's own
    /// non-waiting exclusion, may require no more review than
    /// `entered_tier`, the review the dispatch actually entered (the rule
    /// `issue_child_if_current_within` applies), and no review runs here.
    /// `prepared` is the validity of the decision the dispatch entered (its
    /// window and monotonic cutoff; `None` refuses). An implementor must
    /// refuse a decision stricter than `entered_tier` under its own
    /// exclusion, whatever its caller checked before. Wrappers check their
    /// own custody and forward. The default is unavailable and never runs
    /// `operation`: a wrapper that does not forward it refuses every entered
    /// step as unavailable. `operation` runs at most once and must not
    /// await.
    fn run_entered_if_current_within(
        &self,
        _binding: &PreparedAuthorizationBinding,
        _prepared: Option<&OperationValidity>,
        _further: &(dyn std::any::Any + Send + Sync),
        _operation: &mut dyn FnMut(),
        _entered_tier: OperationReviewTier,
    ) -> Result<(), OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }

    /// Whether this owner can take `further` and `intent` for
    /// `issue_child_if_current_within`: their types and this owner's own
    /// issuance composition, checked before any review is spent. It decides
    /// nothing about permission. The default is unavailable.
    fn validate_child_issuance(
        &self,
        _further: &(dyn std::any::Any + Send + Sync),
        _intent: &(dyn std::any::Any + Send + Sync),
    ) -> Result<(), OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }

    /// Issue (or recover) a child grant for `intent` only while the exact
    /// prepared operation of `binding` is currently permitted by this
    /// work's own retained owners with `further` conjoined, under the same
    /// single non-waiting exclusion `run_if_current_within` uses: the
    /// complete entry decision, its review tier no stricter than
    /// `entered_tier` (the review this dispatch actually entered), and the
    /// grant owner's own reserved issuance with the decision's validity.
    /// The typed outcome is returned boxed. Wrappers check their own custody
    /// and forward; the default is unavailable and issues nothing.
    fn issue_child_if_current_within(
        &self,
        _binding: &PreparedAuthorizationBinding,
        _prepared: Option<&OperationValidity>,
        _further: &(dyn std::any::Any + Send + Sync),
        _intent: &(dyn std::any::Any + Send + Sync),
        _entered_tier: OperationReviewTier,
    ) -> Result<Box<dyn std::any::Any + Send>, OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }

    /// The owner change in progress on this work's own authorization
    /// publication, if any: a token for waiting until it completes
    /// ([`OwnerChangeChurn`]). A hint only, never a permission; it covers
    /// this work's own publication instance and nothing else. A wrapper of a
    /// run's work authorization forwards it. An owner without a publication
    /// answers `None`, and so does a poisoned publication, whose observations
    /// stay unavailable; a refusal then takes its ordinary path. An owner may
    /// also name a hold of its publication that has not published until it
    /// ends (the native work owner names every reservation of its
    /// publication's writer), so a wait lasts until the holder releases it.
    fn owner_change_in_progress(&self) -> Option<OwnerChangeInProgress> {
        None
    }
}

struct WorkAuthorizationContextInner {
    authorization: Arc<dyn WorkAuthorization>,
    execution_scope: OperationExecutionScope,
    controller_client: Option<crate::ControllerModelClient>,
    retained_work: Option<crate::retained_work::RetainedWorkIdentity>,
    consent_scope: Option<crate::approval::ApprovalConsentScope>,
    consent_hosting: Option<crate::session_hosting::HostingClaim>,
    resumed_inputs: Vec<crate::retained_work::ResumedInput>,
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
            consent_scope: None,
            consent_hosting: None,
            resumed_inputs: Vec::new(),
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
            consent_scope: self.0.consent_scope.clone(),
            consent_hosting: self.0.consent_hosting.clone(),
            resumed_inputs: self.0.resumed_inputs.clone(),
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
            consent_scope: self.0.consent_scope.clone(),
            consent_hosting: self.0.consent_hosting.clone(),
            resumed_inputs: self.0.resumed_inputs.clone(),
        }))
    }

    /// Record the hosting domain and session of the staged run this context
    /// authorizes. On the native path the runtime attaches it from the run it
    /// staged; a trusted host building its own context may attach it too. A
    /// consent approval admitted under this context commits it as passive
    /// scope. Identity data only, never a permission or custody.
    ///
    /// A hosting claim lineage recorded before stays only when it derives
    /// exactly this scope (see [`Self::with_consent_hosting`]).
    #[must_use]
    pub fn with_consent_scope(self, scope: crate::approval::ApprovalConsentScope) -> Self {
        let consent_hosting = self
            .0
            .consent_hosting
            .clone()
            .filter(|claim| consent_scope_of(claim).as_ref() == Some(&scope));
        Self(Arc::new(WorkAuthorizationContextInner {
            authorization: Arc::clone(&self.0.authorization),
            execution_scope: self.0.execution_scope.clone(),
            controller_client: self.0.controller_client.clone(),
            retained_work: self.0.retained_work.clone(),
            consent_scope: Some(scope),
            consent_hosting,
            resumed_inputs: self.0.resumed_inputs.clone(),
        }))
    }

    /// Record the staged inputs of this run that resume retained work, each
    /// with the identity of the run it resumes. On the native path the
    /// runtime attaches them from the batch it staged. Identity data only:
    /// the consent owner moves parked custody into the run only for an input
    /// its own committed notice receipt names.
    #[must_use]
    pub fn with_resumed_inputs(self, inputs: Vec<crate::retained_work::ResumedInput>) -> Self {
        Self(Arc::new(WorkAuthorizationContextInner {
            authorization: Arc::clone(&self.0.authorization),
            execution_scope: self.0.execution_scope.clone(),
            controller_client: self.0.controller_client.clone(),
            retained_work: self.0.retained_work.clone(),
            consent_scope: self.0.consent_scope.clone(),
            consent_hosting: self.0.consent_hosting.clone(),
            resumed_inputs: inputs,
        }))
    }

    /// The staged inputs of this run that resume retained work.
    pub fn resumed_inputs(&self) -> &[crate::retained_work::ResumedInput] {
        &self.0.resumed_inputs
    }

    /// The hosting domain and session of the staged run, when the native
    /// owner recorded them.
    pub fn consent_scope(&self) -> Option<&crate::approval::ApprovalConsentScope> {
        self.0.consent_scope.as_ref()
    }

    /// Record a lineage clone of the runtime owner's hosting claim for the
    /// session of the staged run, with the consent scope that lineage
    /// derives: its hosting domain and its session. A lineage and a scope
    /// can therefore never name different sessions or domains. The native
    /// runtime attaches it where it fills the consent scope. A consent
    /// candidate admitted under this context keeps the clone for as long as
    /// it holds custody, so the session's claim is never released while
    /// custody exists. A claim without a cross-process domain records
    /// nothing. Lineage only: it grants no permission and is never persisted.
    #[must_use]
    pub fn with_consent_hosting(self, claim: crate::session_hosting::HostingClaim) -> Self {
        let Some(scope) = consent_scope_of(&claim) else {
            return self;
        };
        Self(Arc::new(WorkAuthorizationContextInner {
            authorization: Arc::clone(&self.0.authorization),
            execution_scope: self.0.execution_scope.clone(),
            controller_client: self.0.controller_client.clone(),
            retained_work: self.0.retained_work.clone(),
            consent_scope: Some(scope),
            consent_hosting: Some(claim),
            resumed_inputs: self.0.resumed_inputs.clone(),
        }))
    }

    /// The hosting claim lineage the native runtime attached for the session
    /// of the staged run, when it attached one.
    pub fn consent_hosting(&self) -> Option<&crate::session_hosting::HostingClaim> {
        self.0.consent_hosting.as_ref()
    }

    /// Identity of the staged run this context authorizes, when the native
    /// owner recorded one.
    pub fn retained_work(&self) -> Option<&crate::retained_work::RetainedWorkIdentity> {
        self.0.retained_work.as_ref()
    }

    pub fn controller_client(&self) -> Option<&crate::ControllerModelClient> {
        self.0.controller_client.as_ref()
    }

    /// Record that a mandatory hook withheld the actual model candidate
    /// `operation_id`, under this work's own execution scope.
    pub fn observe_mandatory_hook_withholding(
        &self,
        operation_id: crate::OperationId,
        run_id: Option<RunId>,
        withholding: MandatoryHookWithholding,
    ) -> Result<(), OperationObservationError> {
        self.0.authorization.observe_mandatory_hook_withholding(
            &MandatoryHookWithholdingRecord::new(
                operation_id,
                self.0.execution_scope.clone(),
                run_id,
                withholding,
            ),
        )
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

/// The consent scope a hosting claim lineage derives: its cross-process
/// hosting domain and its session.
fn consent_scope_of(
    claim: &crate::session_hosting::HostingClaim,
) -> Option<crate::approval::ApprovalConsentScope> {
    Some(crate::approval::ApprovalConsentScope {
        domain: claim.domain_ref()?,
        session_id: claim.session_id().clone(),
    })
}

impl fmt::Debug for WorkAuthorizationContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WorkAuthorizationContext([REDACTED])")
    }
}

/// The temporal window of one exact prepared decision: usable from
/// `not_before_ms` and before `expires_at_ms` on the wall clock, and before
/// `deadline` on the monotonic clock of the read that compiled it. Data of
/// the decision, never a permission by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationValidity {
    pub not_before_ms: u64,
    pub expires_at_ms: u64,
    pub deadline: crate::time_compat::Instant,
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

    /// The exact temporal window this decision was compiled with, when its
    /// owner bounds it so. Stored Copy data only. `None` (the default, and
    /// any wrapper that does not delegate) makes a caller that needs the
    /// window refuse.
    fn validity(&self) -> Option<OperationValidity> {
        None
    }

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

/// Owner- and model-facing disposition of the affected operation, not a
/// turn/run disposition. One of three closed values that carries no private
/// fact: it names no class, source, principal, sink or policy text, and no
/// audit record. It is no permission either: a typed reason never grants
/// replay after entry, and a new call is a new operation with fresh
/// admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum OperationRefusalKind {
    ReprepareRequired,
    Denied,
    MalformedFacts,
}

/// Whether the tool code a refusal's certifier guards ran for the refused
/// call. Only a core component that sees its own refusal happen before it
/// invokes that code certifies [`Self::NotDispatched`]; every other refusal
/// is [`Self::Unknown`]. Like the kind, neither value is a permission.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalDispatch {
    /// No code of the tool that the certifying core component guards ran
    /// for this call: the fenced dispatch refused before it invoked the
    /// dispatcher, or the execution policy gate before it invoked its inner
    /// dispatcher. The certification covers only the code below its
    /// certifier: a dispatcher layer that runs tool code of its own before
    /// delegating forwards such a refusal as
    /// `OperationRefused::new(refusal.kind())`, which is [`Self::Unknown`].
    NotDispatched,
    /// No certification: the tool code may or may not have run.
    Unknown,
}

/// Audience-safe refusal with a typed owner- and model-facing disposition
/// and whether the guarded tool code ran ([`RefusalDispatch`]).
/// No private class, source, principal, sink or policy explanation is
/// carried, and neither fact grants replay after entry. Equality compares
/// both facts.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OperationRefused {
    kind: OperationRefusalKind,
    dispatch: RefusalDispatch,
}

impl OperationRefused {
    /// A refusal that makes no claim about whether tool code ran
    /// ([`RefusalDispatch::Unknown`]).
    pub const fn new(kind: OperationRefusalKind) -> Self {
        Self {
            kind,
            dispatch: RefusalDispatch::Unknown,
        }
    }

    /// This refusal certified by the core component that met it before it
    /// invoked the tool code it guards.
    #[must_use]
    pub(crate) const fn not_dispatched(self) -> Self {
        Self {
            dispatch: RefusalDispatch::NotDispatched,
            ..self
        }
    }

    /// Test-only: this refusal as a fenced dispatch certifies it, so a test
    /// outside core can build what a certified refusal looks like
    /// downstream.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    #[must_use]
    pub const fn certified_not_dispatched_for_test(self) -> Self {
        self.not_dispatched()
    }

    pub const fn kind(self) -> OperationRefusalKind {
        self.kind
    }

    /// Whether the guarded tool code ran for the refused call; see
    /// [`RefusalDispatch`].
    pub const fn dispatch(self) -> RefusalDispatch {
        self.dispatch
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
    #[test]
    fn review_operation_context_is_bounded_at_construction_and_redacted() {
        use super::{
            MAX_REVIEW_OPERATION_CONTEXT_BYTES, ReviewOperationContext,
            ReviewOperationContextInvalid,
        };
        assert_eq!(
            ReviewOperationContext::new(None, None),
            Err(ReviewOperationContextInvalid)
        );
        assert_eq!(
            ReviewOperationContext::new(Some(" \n".into()), None),
            Err(ReviewOperationContextInvalid)
        );
        assert_eq!(
            ReviewOperationContext::new(
                None,
                Some("x".repeat(MAX_REVIEW_OPERATION_CONTEXT_BYTES + 1))
            ),
            Err(ReviewOperationContextInvalid)
        );
        let mandate = "x".repeat(MAX_REVIEW_OPERATION_CONTEXT_BYTES);
        let context =
            ReviewOperationContext::new(Some("billing-account-7".into()), Some(mandate.clone()))
                .unwrap();
        assert_eq!(context.action_account(), Some("billing-account-7"));
        assert_eq!(context.mandate(), Some(mandate.as_str()));
        assert_eq!(format!("{context:?}"), "ReviewOperationContext([REDACTED])");
    }

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

    /// Consent owner loss (3): a work context records the consent scope its
    /// hosting claim lineage derives (the lineage's domain and session), so
    /// a lineage and a scope never name different sessions or domains. A
    /// scope set afterwards keeps the lineage only when the lineage derives
    /// it, and a claim without a cross-process domain records nothing.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_consent_lineage_and_its_scope_name_one_session_in_one_domain() {
        use crate::approval::ApprovalConsentScope;
        use crate::session_hosting::{
            HostingCapability, HostingOwner, HostingPaths, grant_session_hosting,
        };
        struct NoWork;
        impl WorkAuthorization for NoWork {
            fn prepare(
                &self,
                _binding: &PreparedAuthorizationBinding,
            ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError>
            {
                Err(OperationAuthorizationError::Unavailable)
            }
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = |name: &str| HostingPaths {
            hosting_lock_dir: dir.path().join(name),
            cold_delivery_lock: dir.path().join(name).join("cold-delivery.lock"),
            database: None,
        };
        let capability = HostingCapability::OsLock(Arc::new(paths("hosting")));
        let session = SessionId::new();
        let claim =
            grant_session_hosting(&capability, &HostingOwner::mint(), &session).expect("claim");
        let work = WorkAuthorizationContext::new(Arc::new(NoWork), OperationExecutionScope::Domain);
        let derived = ApprovalConsentScope {
            domain: paths("hosting").domain_ref(),
            session_id: session.clone(),
        };

        let hosted = work.clone().with_consent_hosting(claim);
        assert_eq!(
            hosted.consent_scope(),
            Some(&derived),
            "the lineage derives its scope"
        );
        assert!(hosted.consent_hosting().is_some());
        let other_session = hosted.clone().with_consent_scope(ApprovalConsentScope {
            domain: derived.domain.clone(),
            session_id: SessionId::new(),
        });
        assert!(
            other_session.consent_hosting().is_none(),
            "a scope of another session drops the lineage"
        );
        let other_domain = hosted.clone().with_consent_scope(ApprovalConsentScope {
            domain: paths("other").domain_ref(),
            session_id: session.clone(),
        });
        assert!(
            other_domain.consent_hosting().is_none(),
            "a scope of another domain drops the lineage"
        );
        assert!(
            hosted
                .with_consent_scope(derived)
                .consent_hosting()
                .is_some(),
            "the scope the lineage derives keeps it"
        );

        let unclaimed = grant_session_hosting(
            &HostingCapability::ProcessLocal,
            &HostingOwner::mint(),
            &session,
        )
        .expect("an unclaimed grant");
        let local = work.with_consent_hosting(unclaimed);
        assert!(
            local.consent_hosting().is_none() && local.consent_scope().is_none(),
            "a claim without a cross-process domain records nothing"
        );
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
