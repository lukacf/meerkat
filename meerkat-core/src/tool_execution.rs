//! Internal tool-execution declarations and pre-dispatch resolution.
//!
//! Execution metadata lives beside the internal tool catalog. It is not part
//! of provider-facing [`crate::ToolDef`] serialization.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Execution class declared by a tool identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolExecutionMode {
    Fast,
    Streaming,
    Detached,
}

/// Owner of one deadline that contributes to a resolved tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolDeadlineOwner {
    CoreToolDispatch,
    Dispatcher,
    ToolInternal,
    StreamingAbsolute,
    MobkitPublicCallback,
    SdkCallbackCancellation,
    GatewayWire,
    DetachedSubmission,
    DirectCaller,
}

impl ToolDeadlineOwner {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CoreToolDispatch => "core tool dispatch",
            Self::Dispatcher => "dispatcher",
            Self::ToolInternal => "tool internal",
            Self::StreamingAbsolute => "streaming absolute",
            Self::MobkitPublicCallback => "mobkit public callback",
            Self::SdkCallbackCancellation => "sdk callback cancellation",
            Self::GatewayWire => "gateway wire",
            Self::DetachedSubmission => "detached submission",
            Self::DirectCaller => "direct caller",
        }
    }
}

/// One finite or unbounded deadline in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolDeadlineContributor {
    owner: ToolDeadlineOwner,
    timeout: Option<Duration>,
}

impl ToolDeadlineContributor {
    pub const fn finite(owner: ToolDeadlineOwner, timeout: Duration) -> Self {
        Self {
            owner,
            timeout: Some(timeout),
        }
    }

    pub const fn unbounded(owner: ToolDeadlineOwner) -> Self {
        Self {
            owner,
            timeout: None,
        }
    }

    pub const fn owner(&self) -> ToolDeadlineOwner {
        self.owner
    }

    pub const fn timeout(&self) -> Option<Duration> {
        self.timeout
    }
}

/// Invalid deadline-chain declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeadlineChainError {
    Empty,
    Zero { owner: ToolDeadlineOwner },
}

impl std::fmt::Display for DeadlineChainError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("tool deadline chain must not be empty"),
            Self::Zero { owner } => write!(
                formatter,
                "tool deadline from '{}' must be greater than zero",
                owner.as_str()
            ),
        }
    }
}

impl std::error::Error for DeadlineChainError {}

/// A resolved deadline chain did not retain its upstream chain as an ordered
/// prefix.
///
/// Resolvers may only append contributors. Replacing, removing, or reordering
/// an upstream contributor can silently widen a caller-owned deadline and is
/// therefore rejected before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeadlineChainExtensionError {
    ShorterThanUpstream {
        upstream_len: usize,
        resolved_len: usize,
    },
    ContributorMismatch {
        index: usize,
        expected: ToolDeadlineContributor,
        actual: ToolDeadlineContributor,
    },
}

impl std::fmt::Display for DeadlineChainExtensionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShorterThanUpstream {
                upstream_len,
                resolved_len,
            } => write!(
                formatter,
                "resolved tool deadline chain has {resolved_len} contributors but its upstream chain has {upstream_len}"
            ),
            Self::ContributorMismatch {
                index,
                expected,
                actual,
            } => write!(
                formatter,
                "resolved tool deadline contributor {index} replaced upstream owner '{}' ({:?}) with '{}' ({:?})",
                expected.owner().as_str(),
                expected.timeout(),
                actual.owner().as_str(),
                actual.timeout()
            ),
        }
    }
}

impl std::error::Error for DeadlineChainExtensionError {}

/// Ordered deadline chain resolved before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDeadlineChain {
    contributors: Vec<ToolDeadlineContributor>,
}

impl ToolDeadlineChain {
    pub fn new(contributors: Vec<ToolDeadlineContributor>) -> Result<Self, DeadlineChainError> {
        if contributors.is_empty() {
            return Err(DeadlineChainError::Empty);
        }
        if let Some(contributor) = contributors
            .iter()
            .find(|contributor| contributor.timeout == Some(Duration::ZERO))
        {
            return Err(DeadlineChainError::Zero {
                owner: contributor.owner,
            });
        }
        Ok(Self { contributors })
    }

    pub fn contributors(&self) -> &[ToolDeadlineContributor] {
        &self.contributors
    }

    pub fn winner(&self) -> Option<&ToolDeadlineContributor> {
        self.contributors
            .iter()
            .filter(|contributor| contributor.timeout.is_some())
            .min_by_key(|contributor| contributor.timeout)
    }

    pub fn effective_timeout(&self) -> Option<Duration> {
        self.winner().and_then(ToolDeadlineContributor::timeout)
    }

    pub fn diagnostic(&self) -> String {
        let mut diagnostic = String::new();
        match self.effective_timeout() {
            Some(timeout) => {
                let _ = writeln!(
                    diagnostic,
                    "effective deadline: {}",
                    format_duration(timeout)
                );
            }
            None => diagnostic.push_str("effective deadline: unbounded\n"),
        }
        diagnostic.push_str("contributors:\n");
        for contributor in &self.contributors {
            let limit = contributor
                .timeout
                .map_or_else(|| "unbounded".to_string(), format_duration);
            let _ = writeln!(diagnostic, "  {}: {limit}", contributor.owner.as_str());
        }
        diagnostic.push_str("winner: ");
        diagnostic.push_str(
            self.winner()
                .map(|winner| winner.owner.as_str())
                .unwrap_or("unbounded"),
        );
        diagnostic
    }

    pub fn with_contributor(
        &self,
        contributor: ToolDeadlineContributor,
    ) -> Result<Self, DeadlineChainError> {
        let mut contributors = self.contributors.clone();
        contributors.push(contributor);
        Self::new(contributors)
    }

    /// Verify that `upstream` is an exact ordered prefix of this chain.
    pub fn validate_extends(&self, upstream: &Self) -> Result<(), DeadlineChainExtensionError> {
        if self.contributors.len() < upstream.contributors.len() {
            return Err(DeadlineChainExtensionError::ShorterThanUpstream {
                upstream_len: upstream.contributors.len(),
                resolved_len: self.contributors.len(),
            });
        }
        for (index, expected) in upstream.contributors.iter().copied().enumerate() {
            let actual = self.contributors[index];
            if actual != expected {
                return Err(DeadlineChainExtensionError::ContributorMismatch {
                    index,
                    expected,
                    actual,
                });
            }
        }
        Ok(())
    }
}

fn format_duration(duration: Duration) -> String {
    if duration.subsec_nanos() == 0 {
        format!("{}s", duration.as_secs())
    } else if duration.as_millis() > 0 {
        format!("{}ms", duration.as_millis())
    } else if duration.as_micros() > 0 {
        format!("{}us", duration.as_micros())
    } else {
        format!("{}ns", duration.as_nanos())
    }
}

/// Restart behavior declared by a detached runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RestartClass {
    Adoptable,
    CheckpointResumable,
    Replayable,
    NonResumable,
}

/// Stable submission-deduplication scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdempotencyScope {
    ToolCall,
    InteractionAndArguments,
    HostSemanticKey,
}

/// Stable runner identity and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerIdentity {
    name: String,
    version: String,
}

impl RunnerIdentity {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, ToolExecutionDeclarationError> {
        let name = name.into();
        let version = version.into();
        if name.trim().is_empty() {
            return Err(ToolExecutionDeclarationError::EmptyRunnerName);
        }
        if version.trim().is_empty() {
            return Err(ToolExecutionDeclarationError::EmptyRunnerVersion);
        }
        Ok(Self { name, version })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Whether one resolved-plan facet applies to the selected execution mode.
///
/// This is intentionally not represented as `Option<T>`: callers must handle
/// the semantic distinction between a facet that does not apply and one whose
/// applicable value is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionApplicability<T> {
    NotApplicable,
    Applicable(T),
}

/// Non-secret credential context to resolve afresh for an execution attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCredentialContextRef {
    /// Resolve the current owning realm/profile using the required scopes.
    OwningProfile { required_scopes: BTreeSet<String> },
    /// Resolve a specific typed auth binding. The binding is an identity
    /// reference only; it does not contain secret material.
    AuthBinding {
        auth_binding: crate::AuthBindingRef,
        required_scopes: BTreeSet<String>,
    },
}

/// Where the selected execution mode commits its canonical output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutputPolicy {
    InlineTerminal,
    StreamingEvents,
    DurableJobResult,
}

/// Where the selected execution mode publishes non-terminal progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProgressPolicy {
    StreamingEvents,
    DurableJobEvents,
}

/// Turn-local identity of one live logical catalog binding.
///
/// Projection allocation identity is deliberately absent: dispatchers may
/// rebuild equivalent `Arc<ToolDef>` values on every catalog read. The
/// semantic declaration is paired with a chain of process-local authority
/// instances and their live epochs. Mutable authorities must advance their
/// epoch when a logical binding is replaced, even by identical metadata.
///
/// This type intentionally has no serialization implementation. It is
/// pre-dispatch TOCTOU fencing only, never durable job, restart, or recovery
/// authority. A reconstructed dispatcher must resolve a fresh plan.
///
/// ```compile_fail
/// fn requires_durable_serialization<T: serde::Serialize>() {}
/// requires_durable_serialization::<meerkat_core::EphemeralToolBindingFingerprint>();
/// ```
#[derive(Clone)]
pub struct EphemeralToolBindingFingerprint {
    tool_name: crate::ToolName,
    description: String,
    input_schema: serde_json::Value,
    provenance: Option<crate::ToolProvenance>,
    plane: crate::ToolPlaneClass,
    callability: crate::ToolCallability,
    deferred_eligibility: crate::ToolCatalogDeferredEligibility,
    execution: ToolExecutionContract,
    authority_chain: Vec<(usize, u64)>,
}

impl std::fmt::Debug for EphemeralToolBindingFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EphemeralToolBindingFingerprint")
            .field("tool_name", &self.tool_name)
            .field("authority_depth", &self.authority_chain.len())
            .finish_non_exhaustive()
    }
}

impl PartialEq for EphemeralToolBindingFingerprint {
    fn eq(&self, other: &Self) -> bool {
        self.tool_name == other.tool_name
            && self.description == other.description
            && self.input_schema == other.input_schema
            && self.provenance == other.provenance
            && self.plane == other.plane
            && self.callability == other.callability
            && self.deferred_eligibility == other.deferred_eligibility
            && self.execution == other.execution
            && self.authority_chain == other.authority_chain
    }
}

impl Eq for EphemeralToolBindingFingerprint {}

pub fn ephemeral_tool_catalog_binding_fingerprint(
    entry: &crate::ToolCatalogEntry,
) -> EphemeralToolBindingFingerprint {
    EphemeralToolBindingFingerprint {
        tool_name: entry.tool.name.clone(),
        description: entry.tool.description.clone(),
        input_schema: entry.tool.input_schema.clone(),
        provenance: entry.tool.provenance.clone(),
        plane: entry.plane,
        callability: entry.callability,
        deferred_eligibility: entry.deferred_eligibility.clone(),
        execution: entry.execution.clone(),
        authority_chain: Vec::new(),
    }
}

impl EphemeralToolBindingFingerprint {
    #[must_use]
    pub fn with_live_authority(mut self, authority_instance: usize, epoch: u64) -> Self {
        self.authority_chain.push((authority_instance, epoch));
        self
    }

    #[must_use]
    pub fn with_dependency(mut self, dependency: &Self) -> Self {
        self.authority_chain
            .extend(dependency.authority_chain.iter().copied());
        self
    }
}

/// Opaque, non-provider-facing witness for the dispatcher owner selected
/// during plan resolution.
///
/// A composite dispatcher can attach this witness and require the same live
/// owner/binding object at dispatch. The witness is valid only for the current
/// pre-dispatch resolution flow. It must be discarded on restart, recovery,
/// dispatcher reconstruction, or turn replay; those paths must resolve again.
/// It is not a job fence, attempt token, or durable authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionOwnerWitness {
    authority_key: String,
    owner_key: String,
    binding_fingerprint: EphemeralToolBindingFingerprint,
}

impl ToolExecutionOwnerWitness {
    pub fn new(
        authority_key: impl Into<String>,
        owner_key: impl Into<String>,
        binding_fingerprint: EphemeralToolBindingFingerprint,
    ) -> Result<Self, ToolExecutionDeclarationError> {
        let authority_key = authority_key.into();
        let owner_key = owner_key.into();
        if authority_key.trim().is_empty() {
            return Err(ToolExecutionDeclarationError::EmptyOwnerWitnessAuthorityKey);
        }
        if owner_key.trim().is_empty() {
            return Err(ToolExecutionDeclarationError::EmptyOwnerWitnessKey);
        }
        Ok(Self {
            authority_key,
            owner_key,
            binding_fingerprint,
        })
    }

    pub fn authority_key(&self) -> &str {
        &self.authority_key
    }

    pub fn owner_key(&self) -> &str {
        &self.owner_key
    }

    pub const fn binding_fingerprint(&self) -> &EphemeralToolBindingFingerprint {
        &self.binding_fingerprint
    }
}

/// Liveness policy for a streaming tool implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamingToolExecutionPolicy {
    inactivity_timeout: Duration,
    absolute_timeout: Duration,
}

impl StreamingToolExecutionPolicy {
    pub fn new(
        inactivity_timeout: Duration,
        absolute_timeout: Duration,
    ) -> Result<Self, ToolExecutionDeclarationError> {
        if inactivity_timeout.is_zero() {
            return Err(ToolExecutionDeclarationError::ZeroStreamingInactivity);
        }
        if absolute_timeout.is_zero() {
            return Err(ToolExecutionDeclarationError::ZeroStreamingAbsolute);
        }
        if inactivity_timeout > absolute_timeout {
            return Err(ToolExecutionDeclarationError::StreamingInactivityExceedsAbsolute);
        }
        Ok(Self {
            inactivity_timeout,
            absolute_timeout,
        })
    }

    pub const fn inactivity_timeout(&self) -> Duration {
        self.inactivity_timeout
    }

    pub const fn absolute_timeout(&self) -> Duration {
        self.absolute_timeout
    }
}

/// Submission and restart declaration for a detached tool implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachedToolExecutionPolicy {
    runner: RunnerIdentity,
    restart_class: RestartClass,
    idempotency_scope: IdempotencyScope,
    submission_timeout: Duration,
    credential_scopes: BTreeSet<String>,
}

impl DetachedToolExecutionPolicy {
    pub fn new(
        runner: RunnerIdentity,
        restart_class: RestartClass,
        idempotency_scope: IdempotencyScope,
        submission_timeout: Duration,
    ) -> Result<Self, ToolExecutionDeclarationError> {
        if submission_timeout.is_zero() {
            return Err(ToolExecutionDeclarationError::ZeroDetachedSubmissionDeadline);
        }
        Ok(Self {
            runner,
            restart_class,
            idempotency_scope,
            submission_timeout,
            credential_scopes: BTreeSet::new(),
        })
    }

    #[must_use]
    pub fn with_credential_scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.credential_scopes.extend(
            scopes
                .into_iter()
                .map(Into::into)
                .filter(|scope: &String| !scope.trim().is_empty()),
        );
        self
    }

    pub fn runner(&self) -> &RunnerIdentity {
        &self.runner
    }

    pub const fn restart_class(&self) -> RestartClass {
        self.restart_class
    }

    pub const fn idempotency_scope(&self) -> IdempotencyScope {
        self.idempotency_scope
    }

    pub const fn submission_timeout(&self) -> Duration {
        self.submission_timeout
    }

    pub fn credential_scopes(&self) -> &BTreeSet<String> {
        &self.credential_scopes
    }
}

/// Invalid streaming or detached policy declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionDeclarationError {
    EmptyRunnerName,
    EmptyRunnerVersion,
    EmptyOwnerWitnessAuthorityKey,
    EmptyOwnerWitnessKey,
    ZeroStreamingInactivity,
    ZeroStreamingAbsolute,
    StreamingInactivityExceedsAbsolute,
    ZeroDetachedSubmissionDeadline,
}

impl std::fmt::Display for ToolExecutionDeclarationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyRunnerName => formatter.write_str("detached runner name must not be empty"),
            Self::EmptyRunnerVersion => {
                formatter.write_str("detached runner version must not be empty")
            }
            Self::EmptyOwnerWitnessAuthorityKey => {
                formatter.write_str("tool execution owner witness authority key must not be empty")
            }
            Self::EmptyOwnerWitnessKey => {
                formatter.write_str("tool execution owner witness key must not be empty")
            }
            Self::ZeroStreamingInactivity => {
                formatter.write_str("streaming inactivity timeout must be greater than zero")
            }
            Self::ZeroStreamingAbsolute => {
                formatter.write_str("streaming absolute timeout must be greater than zero")
            }
            Self::StreamingInactivityExceedsAbsolute => formatter
                .write_str("streaming inactivity timeout must not exceed the absolute timeout"),
            Self::ZeroDetachedSubmissionDeadline => {
                formatter.write_str("detached submission timeout must be greater than zero")
            }
        }
    }
}

impl std::error::Error for ToolExecutionDeclarationError {}

/// Invalid combination of supported modes and mode-specific policies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionContractError {
    NoSupportedModes,
    DefaultModeUnsupported {
        default_mode: ToolExecutionMode,
    },
    MissingStreamingPolicy,
    UnexpectedStreamingPolicy,
    MissingDetachedPolicy,
    UnexpectedDetachedPolicy,
    RequestedModeUnsupported {
        requested_mode: ToolExecutionMode,
    },
    RequestedRestartClassUnsupported {
        restart_class: RestartClass,
    },
    ResolvedPlanFacetMismatch {
        mode: ToolExecutionMode,
        facet: &'static str,
    },
}

impl std::fmt::Display for ToolExecutionContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSupportedModes => {
                formatter.write_str("tool execution contract must support at least one mode")
            }
            Self::DefaultModeUnsupported { default_mode } => write!(
                formatter,
                "default tool execution mode {default_mode:?} is not supported"
            ),
            Self::MissingStreamingPolicy => {
                formatter.write_str("streaming mode requires a streaming policy")
            }
            Self::UnexpectedStreamingPolicy => {
                formatter.write_str("streaming policy requires streaming mode support")
            }
            Self::MissingDetachedPolicy => {
                formatter.write_str("detached mode requires a detached policy")
            }
            Self::UnexpectedDetachedPolicy => {
                formatter.write_str("detached policy requires detached mode support")
            }
            Self::RequestedModeUnsupported { requested_mode } => write!(
                formatter,
                "requested tool execution mode {requested_mode:?} is not supported"
            ),
            Self::RequestedRestartClassUnsupported { restart_class } => write!(
                formatter,
                "requested detached restart class {restart_class:?} is not supported"
            ),
            Self::ResolvedPlanFacetMismatch { mode, facet } => write!(
                formatter,
                "resolved {mode:?} tool execution plan does not match advertised facet '{facet}'"
            ),
        }
    }
}

impl std::error::Error for ToolExecutionContractError {}

/// Typed, caller-owned facts used during pre-dispatch plan resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionResolutionContext {
    deadlines: ToolDeadlineChain,
}

impl ToolExecutionResolutionContext {
    pub const fn new(deadlines: ToolDeadlineChain) -> Self {
        Self { deadlines }
    }

    pub fn deadlines(&self) -> &ToolDeadlineChain {
        &self.deadlines
    }

    pub fn with_deadline(
        &self,
        contributor: ToolDeadlineContributor,
    ) -> Result<Self, DeadlineChainError> {
        self.deadlines.with_contributor(contributor).map(Self::new)
    }

    /// Verify that a resolver only appended deadlines to this caller-owned
    /// chain.
    pub fn validate_resolved_plan(
        &self,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<(), ToolExecutionResolutionError> {
        plan.deadlines
            .validate_extends(&self.deadlines)
            .map_err(ToolExecutionResolutionError::DeadlineExtension)
    }
}

/// Failure to resolve an exact pre-dispatch tool execution plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionResolutionError {
    NotFound {
        tool_name: String,
    },
    Unavailable {
        tool_name: String,
        reason: crate::tool_catalog::ToolUnavailableReason,
    },
    AccessDenied {
        tool_name: String,
    },
    InvalidArguments {
        tool_name: String,
        reason: String,
    },
    Deadline(DeadlineChainError),
    DeadlineExtension(DeadlineChainExtensionError),
    OwnerWitnessAlreadyAssigned {
        existing: Box<ToolExecutionOwnerWitness>,
        attempted: Box<ToolExecutionOwnerWitness>,
    },
    ResolvedCallMismatch {
        tool_name: String,
    },
    RootDispatcherChanged {
        tool_name: String,
    },
    Declaration(ToolExecutionDeclarationError),
    Contract(ToolExecutionContractError),
}

impl std::fmt::Display for ToolExecutionResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { tool_name } => {
                write!(
                    formatter,
                    "tool '{tool_name}' is not present in the catalog"
                )
            }
            Self::Unavailable { tool_name, reason } => {
                write!(formatter, "tool '{tool_name}' is unavailable: {reason}")
            }
            Self::AccessDenied { tool_name } => {
                write!(
                    formatter,
                    "tool '{tool_name}' is denied by execution policy"
                )
            }
            Self::InvalidArguments { tool_name, reason } => {
                write!(
                    formatter,
                    "tool '{tool_name}' arguments cannot resolve an execution plan: {reason}"
                )
            }
            Self::Deadline(error) => std::fmt::Display::fmt(error, formatter),
            Self::DeadlineExtension(error) => {
                write!(
                    formatter,
                    "tool deadline resolution replaced an upstream deadline: {error}"
                )
            }
            Self::OwnerWitnessAlreadyAssigned {
                existing,
                attempted,
            } => write!(
                formatter,
                "tool execution authority '{}' already selected owner '{}'; refusing replacement by '{}'",
                existing.authority_key(),
                existing.owner_key(),
                attempted.owner_key(),
            ),
            Self::ResolvedCallMismatch { tool_name } => write!(
                formatter,
                "resolved execution plan for tool '{tool_name}' was dispatched with a different call identity"
            ),
            Self::RootDispatcherChanged { tool_name } => write!(
                formatter,
                "resolved execution plan for tool '{tool_name}' was dispatched through a different dispatcher instance"
            ),
            Self::Declaration(error) => std::fmt::Display::fmt(error, formatter),
            Self::Contract(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for ToolExecutionResolutionError {}

impl From<DeadlineChainError> for ToolExecutionResolutionError {
    fn from(error: DeadlineChainError) -> Self {
        Self::Deadline(error)
    }
}

impl From<ToolExecutionContractError> for ToolExecutionResolutionError {
    fn from(error: ToolExecutionContractError) -> Self {
        Self::Contract(error)
    }
}

impl From<ToolExecutionDeclarationError> for ToolExecutionResolutionError {
    fn from(error: ToolExecutionDeclarationError) -> Self {
        Self::Declaration(error)
    }
}

impl From<ToolExecutionResolutionError> for crate::error::ToolError {
    fn from(error: ToolExecutionResolutionError) -> Self {
        match error {
            ToolExecutionResolutionError::NotFound { tool_name } => Self::not_found(tool_name),
            ToolExecutionResolutionError::Unavailable { tool_name, reason } => {
                Self::unavailable(tool_name, reason)
            }
            ToolExecutionResolutionError::AccessDenied { tool_name } => {
                Self::access_denied(tool_name)
            }
            ToolExecutionResolutionError::InvalidArguments { tool_name, reason } => {
                Self::invalid_arguments(tool_name, reason)
            }
            ToolExecutionResolutionError::Deadline(error) => {
                Self::execution_failed(format!("tool deadline resolution failed: {error}"))
            }
            ToolExecutionResolutionError::DeadlineExtension(error) => Self::execution_failed(
                format!("tool deadline resolution replaced an upstream deadline: {error}"),
            ),
            ToolExecutionResolutionError::OwnerWitnessAlreadyAssigned {
                existing,
                attempted,
            } => Self::execution_failed(format!(
                "tool execution authority '{}' already selected owner '{}'; refusing replacement by '{}'",
                existing.authority_key(),
                existing.owner_key(),
                attempted.owner_key(),
            )),
            ToolExecutionResolutionError::ResolvedCallMismatch { tool_name }
            | ToolExecutionResolutionError::RootDispatcherChanged { tool_name } => {
                Self::unavailable(
                    tool_name,
                    crate::ToolUnavailableReason::ExecutionOwnerChanged,
                )
            }
            ToolExecutionResolutionError::Declaration(error) => {
                Self::execution_failed(format!("tool execution declaration failed: {error}"))
            }
            ToolExecutionResolutionError::Contract(error) => Self::execution_failed(format!(
                "tool execution contract resolution failed: {error}"
            )),
        }
    }
}

/// Internal execution declaration attached to a catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionContract {
    supported_modes: BTreeSet<ToolExecutionMode>,
    default_mode: ToolExecutionMode,
    streaming_policy: Option<StreamingToolExecutionPolicy>,
    detached_policy: Option<DetachedToolExecutionPolicy>,
    detached_restart_classes: BTreeSet<RestartClass>,
}

impl Default for ToolExecutionContract {
    fn default() -> Self {
        Self {
            supported_modes: BTreeSet::from([ToolExecutionMode::Fast]),
            default_mode: ToolExecutionMode::Fast,
            streaming_policy: None,
            detached_policy: None,
            detached_restart_classes: BTreeSet::new(),
        }
    }
}

impl ToolExecutionContract {
    pub fn new(
        supported_modes: BTreeSet<ToolExecutionMode>,
        default_mode: ToolExecutionMode,
        streaming_policy: Option<StreamingToolExecutionPolicy>,
        detached_policy: Option<DetachedToolExecutionPolicy>,
    ) -> Result<Self, ToolExecutionContractError> {
        if supported_modes.is_empty() {
            return Err(ToolExecutionContractError::NoSupportedModes);
        }
        if !supported_modes.contains(&default_mode) {
            return Err(ToolExecutionContractError::DefaultModeUnsupported { default_mode });
        }
        match (
            supported_modes.contains(&ToolExecutionMode::Streaming),
            streaming_policy.is_some(),
        ) {
            (true, false) => return Err(ToolExecutionContractError::MissingStreamingPolicy),
            (false, true) => return Err(ToolExecutionContractError::UnexpectedStreamingPolicy),
            _ => {}
        }
        match (
            supported_modes.contains(&ToolExecutionMode::Detached),
            detached_policy.is_some(),
        ) {
            (true, false) => return Err(ToolExecutionContractError::MissingDetachedPolicy),
            (false, true) => return Err(ToolExecutionContractError::UnexpectedDetachedPolicy),
            _ => {}
        }
        let detached_restart_classes = detached_policy
            .as_ref()
            .map(|policy| BTreeSet::from([policy.restart_class()]))
            .unwrap_or_default();
        Ok(Self {
            supported_modes,
            default_mode,
            streaming_policy,
            detached_policy,
            detached_restart_classes,
        })
    }

    pub fn supported_modes(&self) -> &BTreeSet<ToolExecutionMode> {
        &self.supported_modes
    }

    pub const fn default_mode(&self) -> ToolExecutionMode {
        self.default_mode
    }

    pub fn streaming_policy(&self) -> Option<&StreamingToolExecutionPolicy> {
        self.streaming_policy.as_ref()
    }

    pub fn detached_policy(&self) -> Option<&DetachedToolExecutionPolicy> {
        self.detached_policy.as_ref()
    }

    pub fn detached_restart_classes(&self) -> &BTreeSet<RestartClass> {
        &self.detached_restart_classes
    }

    pub fn with_detached_restart_classes(
        mut self,
        restart_classes: BTreeSet<RestartClass>,
    ) -> Result<Self, ToolExecutionContractError> {
        let policy = self
            .detached_policy
            .as_ref()
            .ok_or(ToolExecutionContractError::MissingDetachedPolicy)?;
        if !restart_classes.contains(&policy.restart_class()) {
            return Err(
                ToolExecutionContractError::RequestedRestartClassUnsupported {
                    restart_class: policy.restart_class(),
                },
            );
        }
        self.detached_restart_classes = restart_classes;
        Ok(self)
    }

    pub fn resolve_default(
        &self,
        deadlines: ToolDeadlineChain,
    ) -> Result<ResolvedToolExecutionPlan, ToolExecutionContractError> {
        self.resolve(self.default_mode, deadlines)
    }

    pub fn resolve(
        &self,
        mode: ToolExecutionMode,
        mut deadlines: ToolDeadlineChain,
    ) -> Result<ResolvedToolExecutionPlan, ToolExecutionContractError> {
        if !self.supported_modes.contains(&mode) {
            return Err(ToolExecutionContractError::RequestedModeUnsupported {
                requested_mode: mode,
            });
        }
        let (
            kind,
            runner,
            restart_class,
            idempotency_scope,
            credential_context_refs,
            output_policy,
            progress_policy,
        ) = match mode {
            ToolExecutionMode::Fast => (
                ResolvedExecutionKind::Fast,
                ToolExecutionApplicability::NotApplicable,
                ToolExecutionApplicability::NotApplicable,
                ToolExecutionApplicability::NotApplicable,
                ToolExecutionApplicability::NotApplicable,
                ToolExecutionApplicability::Applicable(ToolOutputPolicy::InlineTerminal),
                ToolExecutionApplicability::NotApplicable,
            ),
            ToolExecutionMode::Streaming => {
                let policy = self
                    .streaming_policy
                    .clone()
                    .ok_or(ToolExecutionContractError::MissingStreamingPolicy)?;
                deadlines.contributors.push(ToolDeadlineContributor::finite(
                    ToolDeadlineOwner::StreamingAbsolute,
                    policy.absolute_timeout(),
                ));
                (
                    ResolvedExecutionKind::Streaming(policy),
                    ToolExecutionApplicability::NotApplicable,
                    ToolExecutionApplicability::NotApplicable,
                    ToolExecutionApplicability::NotApplicable,
                    ToolExecutionApplicability::NotApplicable,
                    ToolExecutionApplicability::Applicable(ToolOutputPolicy::StreamingEvents),
                    ToolExecutionApplicability::Applicable(ToolProgressPolicy::StreamingEvents),
                )
            }
            ToolExecutionMode::Detached => {
                let policy = self
                    .detached_policy
                    .clone()
                    .ok_or(ToolExecutionContractError::MissingDetachedPolicy)?;
                deadlines.contributors.push(ToolDeadlineContributor::finite(
                    ToolDeadlineOwner::DetachedSubmission,
                    policy.submission_timeout(),
                ));
                (
                    ResolvedExecutionKind::Detached(policy.clone()),
                    ToolExecutionApplicability::Applicable(policy.runner().clone()),
                    ToolExecutionApplicability::Applicable(policy.restart_class()),
                    ToolExecutionApplicability::Applicable(policy.idempotency_scope()),
                    ToolExecutionApplicability::Applicable(vec![
                        ToolCredentialContextRef::OwningProfile {
                            required_scopes: policy.credential_scopes().clone(),
                        },
                    ]),
                    ToolExecutionApplicability::Applicable(ToolOutputPolicy::DurableJobResult),
                    ToolExecutionApplicability::Applicable(ToolProgressPolicy::DurableJobEvents),
                )
            }
        };
        Ok(ResolvedToolExecutionPlan {
            deadlines,
            kind,
            runner,
            restart_class,
            idempotency_scope,
            credential_context_refs,
            output_policy,
            progress_policy,
            owner_witnesses: Vec::new(),
            resolved_call: None,
            root_dispatcher: None,
        })
    }

    pub fn resolve_detached_with_restart_class(
        &self,
        restart_class: RestartClass,
        deadlines: ToolDeadlineChain,
    ) -> Result<ResolvedToolExecutionPlan, ToolExecutionContractError> {
        if !self.detached_restart_classes.contains(&restart_class) {
            return Err(
                ToolExecutionContractError::RequestedRestartClassUnsupported { restart_class },
            );
        }
        let mut policy = self
            .detached_policy
            .clone()
            .ok_or(ToolExecutionContractError::MissingDetachedPolicy)?;
        policy.restart_class = restart_class;
        let mut plan = self.resolve(ToolExecutionMode::Detached, deadlines)?;
        plan.kind = ResolvedExecutionKind::Detached(policy);
        plan.restart_class = ToolExecutionApplicability::Applicable(restart_class);
        Ok(plan)
    }

    /// Validate that a resolver's selected mode and every mode-derived facet
    /// stay within this advertised catalog contract.
    ///
    /// Hybrid resolvers remain free to select any advertised mode from typed
    /// arguments. They may append owner witnesses and additional deadline
    /// contributors, but cannot invent a mode, runner, restart/idempotency
    /// declaration, credential context, or output/progress policy that the
    /// catalog did not advertise.
    pub fn validate_resolved_plan(
        &self,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<(), ToolExecutionContractError> {
        let mode = plan.mode();
        if !self.supported_modes.contains(&mode) {
            return Err(ToolExecutionContractError::RequestedModeUnsupported {
                requested_mode: mode,
            });
        }
        let comparison_deadline = ToolDeadlineChain {
            contributors: vec![ToolDeadlineContributor::unbounded(
                ToolDeadlineOwner::Dispatcher,
            )],
        };
        let expected = if mode == ToolExecutionMode::Detached {
            let ToolExecutionApplicability::Applicable(restart_class) = plan.restart_class else {
                return Err(ToolExecutionContractError::ResolvedPlanFacetMismatch {
                    mode,
                    facet: "restart_class",
                });
            };
            self.resolve_detached_with_restart_class(restart_class, comparison_deadline)?
        } else {
            self.resolve(mode, comparison_deadline)?
        };

        macro_rules! require_facet {
            ($field:ident) => {
                if plan.$field != expected.$field {
                    return Err(ToolExecutionContractError::ResolvedPlanFacetMismatch {
                        mode,
                        facet: stringify!($field),
                    });
                }
            };
        }

        require_facet!(kind);
        require_facet!(runner);
        require_facet!(restart_class);
        require_facet!(idempotency_scope);
        require_facet!(credential_context_refs);
        require_facet!(output_policy);
        require_facet!(progress_policy);
        Ok(())
    }
}

/// Mode-specific part of a resolved execution plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedExecutionKind {
    Fast,
    Streaming(StreamingToolExecutionPolicy),
    Detached(DetachedToolExecutionPolicy),
}

/// Exact execution and deadline plan selected before tool dispatch.
#[derive(Clone)]
pub struct ResolvedToolExecutionPlan {
    deadlines: ToolDeadlineChain,
    kind: ResolvedExecutionKind,
    runner: ToolExecutionApplicability<RunnerIdentity>,
    restart_class: ToolExecutionApplicability<RestartClass>,
    idempotency_scope: ToolExecutionApplicability<IdempotencyScope>,
    credential_context_refs: ToolExecutionApplicability<Vec<ToolCredentialContextRef>>,
    output_policy: ToolExecutionApplicability<ToolOutputPolicy>,
    progress_policy: ToolExecutionApplicability<ToolProgressPolicy>,
    owner_witnesses: Vec<ToolExecutionOwnerWitness>,
    resolved_call: Option<ResolvedToolCallIdentity>,
    root_dispatcher: Option<RootDispatcherLease>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedToolCallIdentity {
    tool_use_id: String,
    tool_name: crate::ToolName,
    canonical_arguments_sha256: [u8; 32],
}

trait ErasedRootDispatcherLease: Send + Sync {
    fn data_ptr(&self) -> *const ();
}

struct TypedRootDispatcherLease<T: ?Sized + Send + Sync + 'static> {
    dispatcher: Arc<T>,
}

impl<T: ?Sized + Send + Sync + 'static> ErasedRootDispatcherLease for TypedRootDispatcherLease<T> {
    fn data_ptr(&self) -> *const () {
        Arc::as_ptr(&self.dispatcher).cast::<()>()
    }
}

#[derive(Clone)]
struct RootDispatcherLease(Arc<dyn ErasedRootDispatcherLease>);

impl RootDispatcherLease {
    fn new<T: ?Sized + Send + Sync + 'static>(dispatcher: Arc<T>) -> Self {
        Self(Arc::new(TypedRootDispatcherLease { dispatcher }))
    }

    fn matches<T: ?Sized + Send + Sync + 'static>(&self, dispatcher: &Arc<T>) -> bool {
        self.0.data_ptr() == Arc::as_ptr(dispatcher).cast::<()>()
    }
}

impl std::fmt::Debug for ResolvedToolExecutionPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedToolExecutionPlan")
            .field("deadlines", &self.deadlines)
            .field("kind", &self.kind)
            .field("runner", &self.runner)
            .field("restart_class", &self.restart_class)
            .field("idempotency_scope", &self.idempotency_scope)
            .field("credential_context_refs", &self.credential_context_refs)
            .field("output_policy", &self.output_policy)
            .field("progress_policy", &self.progress_policy)
            .field("owner_witnesses", &self.owner_witnesses)
            .field("resolved_call", &self.resolved_call)
            .field("has_root_dispatcher_lease", &self.root_dispatcher.is_some())
            .finish()
    }
}

impl PartialEq for ResolvedToolExecutionPlan {
    fn eq(&self, other: &Self) -> bool {
        self.deadlines == other.deadlines
            && self.kind == other.kind
            && self.runner == other.runner
            && self.restart_class == other.restart_class
            && self.idempotency_scope == other.idempotency_scope
            && self.credential_context_refs == other.credential_context_refs
            && self.output_policy == other.output_policy
            && self.progress_policy == other.progress_policy
            && self.owner_witnesses == other.owner_witnesses
            && self.resolved_call == other.resolved_call
            && match (&self.root_dispatcher, &other.root_dispatcher) {
                (Some(left), Some(right)) => left.0.data_ptr() == right.0.data_ptr(),
                (None, None) => true,
                (Some(_), None) | (None, Some(_)) => false,
            }
    }
}

impl Eq for ResolvedToolExecutionPlan {}

impl ResolvedToolExecutionPlan {
    pub const fn mode(&self) -> ToolExecutionMode {
        match self.kind {
            ResolvedExecutionKind::Fast => ToolExecutionMode::Fast,
            ResolvedExecutionKind::Streaming(_) => ToolExecutionMode::Streaming,
            ResolvedExecutionKind::Detached(_) => ToolExecutionMode::Detached,
        }
    }

    pub fn deadlines(&self) -> &ToolDeadlineChain {
        &self.deadlines
    }

    pub fn kind(&self) -> &ResolvedExecutionKind {
        &self.kind
    }

    pub fn runner(&self) -> &ToolExecutionApplicability<RunnerIdentity> {
        &self.runner
    }

    pub const fn restart_class(&self) -> ToolExecutionApplicability<RestartClass> {
        match self.restart_class {
            ToolExecutionApplicability::NotApplicable => ToolExecutionApplicability::NotApplicable,
            ToolExecutionApplicability::Applicable(value) => {
                ToolExecutionApplicability::Applicable(value)
            }
        }
    }

    pub const fn idempotency_scope(&self) -> ToolExecutionApplicability<IdempotencyScope> {
        match self.idempotency_scope {
            ToolExecutionApplicability::NotApplicable => ToolExecutionApplicability::NotApplicable,
            ToolExecutionApplicability::Applicable(value) => {
                ToolExecutionApplicability::Applicable(value)
            }
        }
    }

    pub fn credential_context_refs(
        &self,
    ) -> &ToolExecutionApplicability<Vec<ToolCredentialContextRef>> {
        &self.credential_context_refs
    }

    pub const fn output_policy(&self) -> &ToolExecutionApplicability<ToolOutputPolicy> {
        &self.output_policy
    }

    pub const fn progress_policy(&self) -> &ToolExecutionApplicability<ToolProgressPolicy> {
        &self.progress_policy
    }

    pub fn owner_witness(&self, authority_key: &str) -> Option<&ToolExecutionOwnerWitness> {
        self.owner_witnesses
            .iter()
            .find(|witness| witness.authority_key() == authority_key)
    }

    pub fn owner_witnesses(&self) -> &[ToolExecutionOwnerWitness] {
        &self.owner_witnesses
    }

    pub fn with_owner_witness(
        mut self,
        witness: ToolExecutionOwnerWitness,
    ) -> Result<Self, ToolExecutionResolutionError> {
        if let Some(existing) = self.owner_witness(witness.authority_key()).cloned() {
            return Err(ToolExecutionResolutionError::OwnerWitnessAlreadyAssigned {
                existing: Box::new(existing),
                attempted: Box::new(witness),
            });
        }
        self.owner_witnesses.push(witness);
        Ok(self)
    }

    pub(crate) fn bind_root_dispatch<T: ?Sized + Send + Sync + 'static>(
        mut self,
        dispatcher: Arc<T>,
        call: crate::ToolCallView<'_>,
    ) -> Result<Self, ToolExecutionResolutionError> {
        self.resolved_call = Some(ResolvedToolCallIdentity::from_call(call)?);
        self.root_dispatcher = Some(RootDispatcherLease::new(dispatcher));
        Ok(self)
    }

    pub(crate) fn validate_root_dispatch<T: ?Sized + Send + Sync + 'static>(
        &self,
        dispatcher: &Arc<T>,
        call: crate::ToolCallView<'_>,
    ) -> Result<(), ToolExecutionResolutionError> {
        let Some(root_dispatcher) = self.root_dispatcher.as_ref() else {
            return Err(ToolExecutionResolutionError::RootDispatcherChanged {
                tool_name: call.name.to_string(),
            });
        };
        if !root_dispatcher.matches(dispatcher) {
            return Err(ToolExecutionResolutionError::RootDispatcherChanged {
                tool_name: call.name.to_string(),
            });
        }
        let actual = ResolvedToolCallIdentity::from_call(call)?;
        if self.resolved_call.as_ref() != Some(&actual) {
            return Err(ToolExecutionResolutionError::ResolvedCallMismatch {
                tool_name: call.name.to_string(),
            });
        }
        Ok(())
    }
}

impl ResolvedToolCallIdentity {
    fn from_call(call: crate::ToolCallView<'_>) -> Result<Self, ToolExecutionResolutionError> {
        let arguments: serde_json::Value =
            serde_json::from_str(call.args.get()).map_err(|error| {
                ToolExecutionResolutionError::InvalidArguments {
                    tool_name: call.name.to_string(),
                    reason: error.to_string(),
                }
            })?;
        let mut canonical = Vec::new();
        write_canonical_json(&arguments, &mut canonical).map_err(|error| {
            ToolExecutionResolutionError::InvalidArguments {
                tool_name: call.name.to_string(),
                reason: error.to_string(),
            }
        })?;
        Ok(Self {
            tool_use_id: call.id.to_string(),
            tool_name: call.name.into(),
            canonical_arguments_sha256: Sha256::digest(canonical).into(),
        })
    }
}

fn write_canonical_json(
    value: &serde_json::Value,
    output: &mut Vec<u8>,
) -> Result<(), serde_json::Error> {
    match value {
        serde_json::Value::Object(object) => {
            output.push(b'{');
            let mut keys: Vec<_> = object.keys().collect();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)?;
                output.push(b':');
                write_canonical_json(&object[key], output)?;
            }
            output.push(b'}');
        }
        serde_json::Value::Array(array) => {
            output.push(b'[');
            for (index, item) in array.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical_json(item, output)?;
            }
            output.push(b']');
        }
        scalar => serde_json::to_writer(output, scalar)?,
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::time::Duration;

    fn finite(owner: ToolDeadlineOwner, seconds: u64) -> ToolDeadlineContributor {
        ToolDeadlineContributor::finite(owner, Duration::from_secs(seconds))
    }

    fn ephemeral_fingerprint(label: &str) -> EphemeralToolBindingFingerprint {
        ephemeral_tool_catalog_binding_fingerprint(&crate::ToolCatalogEntry::session_inline(
            Arc::new(crate::ToolDef::new(
                format!("test_{label}"),
                label,
                serde_json::json!({"type": "object"}),
            )),
            true,
        ))
    }

    #[test]
    fn deadline_chain_rejects_empty_and_zero_deadlines() {
        assert_eq!(
            ToolDeadlineChain::new(Vec::new()),
            Err(DeadlineChainError::Empty)
        );
        assert_eq!(
            ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 0,)]),
            Err(DeadlineChainError::Zero {
                owner: ToolDeadlineOwner::CoreToolDispatch,
            })
        );
    }

    #[test]
    fn deadline_chain_retains_every_contributor_and_selects_narrowest() {
        let chain = ToolDeadlineChain::new(vec![
            finite(ToolDeadlineOwner::CoreToolDispatch, 600),
            finite(ToolDeadlineOwner::MobkitPublicCallback, 120),
            finite(ToolDeadlineOwner::SdkCallbackCancellation, 125),
            finite(ToolDeadlineOwner::GatewayWire, 130),
        ])
        .expect("valid deadline chain");

        assert_eq!(chain.contributors().len(), 4);
        assert_eq!(chain.effective_timeout(), Some(Duration::from_secs(120)));
        assert_eq!(
            chain.winner().map(ToolDeadlineContributor::owner),
            Some(ToolDeadlineOwner::MobkitPublicCallback)
        );
        assert_eq!(
            chain.diagnostic(),
            "effective deadline: 120s\ncontributors:\n  core tool dispatch: 600s\n  mobkit public callback: 120s\n  sdk callback cancellation: 125s\n  gateway wire: 130s\nwinner: mobkit public callback"
        );
    }

    #[test]
    fn equal_deadlines_use_declaration_order_as_deterministic_tie_breaker() {
        let chain = ToolDeadlineChain::new(vec![
            finite(ToolDeadlineOwner::Dispatcher, 30),
            finite(ToolDeadlineOwner::ToolInternal, 30),
        ])
        .expect("valid deadline chain");

        assert_eq!(
            chain.winner().map(ToolDeadlineContributor::owner),
            Some(ToolDeadlineOwner::Dispatcher)
        );
    }

    #[test]
    fn unbounded_contributors_are_diagnostic_but_never_win_a_finite_chain() {
        let chain = ToolDeadlineChain::new(vec![
            ToolDeadlineContributor::unbounded(ToolDeadlineOwner::DirectCaller),
            finite(ToolDeadlineOwner::ToolInternal, 30),
        ])
        .expect("valid deadline chain");

        assert_eq!(chain.effective_timeout(), Some(Duration::from_secs(30)));
        assert_eq!(
            chain.winner().map(ToolDeadlineContributor::owner),
            Some(ToolDeadlineOwner::ToolInternal)
        );
        assert!(chain.diagnostic().contains("direct caller: unbounded"));
    }

    #[test]
    fn deadline_diagnostic_never_rounds_a_nonzero_timeout_to_zero() {
        let chain = ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
            ToolDeadlineOwner::ToolInternal,
            Duration::from_nanos(1),
        )])
        .expect("valid deadline chain");

        assert!(chain.diagnostic().contains("effective deadline: 1ns"));
        assert!(chain.diagnostic().contains("tool internal: 1ns"));
        assert!(!chain.diagnostic().contains("0ms"));
    }

    #[test]
    fn execution_contract_default_is_fast_only() {
        let contract = ToolExecutionContract::default();

        assert_eq!(contract.default_mode(), ToolExecutionMode::Fast);
        assert_eq!(
            contract.supported_modes(),
            &BTreeSet::from([ToolExecutionMode::Fast])
        );
        assert!(contract.streaming_policy().is_none());
        assert!(contract.detached_policy().is_none());
    }

    #[test]
    fn execution_contract_rejects_incoherent_mode_policies() {
        let streaming =
            StreamingToolExecutionPolicy::new(Duration::from_secs(5), Duration::from_secs(60))
                .expect("valid streaming policy");
        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("homecore.security_scan", "v1").expect("valid runner"),
            RestartClass::NonResumable,
            IdempotencyScope::InteractionAndArguments,
            Duration::from_secs(10),
        )
        .expect("valid detached policy");

        assert_eq!(
            ToolExecutionContract::new(BTreeSet::new(), ToolExecutionMode::Fast, None, None,),
            Err(ToolExecutionContractError::NoSupportedModes)
        );
        assert_eq!(
            ToolExecutionContract::new(
                BTreeSet::from([ToolExecutionMode::Fast]),
                ToolExecutionMode::Detached,
                None,
                None,
            ),
            Err(ToolExecutionContractError::DefaultModeUnsupported {
                default_mode: ToolExecutionMode::Detached,
            })
        );
        assert_eq!(
            ToolExecutionContract::new(
                BTreeSet::from([ToolExecutionMode::Fast]),
                ToolExecutionMode::Fast,
                Some(streaming),
                None,
            ),
            Err(ToolExecutionContractError::UnexpectedStreamingPolicy)
        );
        assert_eq!(
            ToolExecutionContract::new(
                BTreeSet::from([ToolExecutionMode::Streaming]),
                ToolExecutionMode::Streaming,
                None,
                None,
            ),
            Err(ToolExecutionContractError::MissingStreamingPolicy)
        );
        assert_eq!(
            ToolExecutionContract::new(
                BTreeSet::from([ToolExecutionMode::Fast]),
                ToolExecutionMode::Fast,
                None,
                Some(detached),
            ),
            Err(ToolExecutionContractError::UnexpectedDetachedPolicy)
        );
        assert_eq!(
            ToolExecutionContract::new(
                BTreeSet::from([ToolExecutionMode::Detached]),
                ToolExecutionMode::Detached,
                None,
                None,
            ),
            Err(ToolExecutionContractError::MissingDetachedPolicy)
        );
    }

    #[test]
    fn detached_resolution_carries_typed_runner_restart_and_idempotency() {
        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("homecore.security_scan", "v1").expect("valid runner"),
            RestartClass::NonResumable,
            IdempotencyScope::InteractionAndArguments,
            Duration::from_secs(10),
        )
        .expect("valid detached policy")
        .with_credential_scopes(["network"]);
        let contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(detached.clone()),
        )
        .expect("valid detached contract");
        let deadlines =
            ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                .expect("valid upstream deadline");

        let plan = contract
            .resolve_default(deadlines)
            .expect("default mode resolves");

        assert_eq!(plan.mode(), ToolExecutionMode::Detached);
        assert_eq!(
            plan.deadlines().contributors(),
            &[
                finite(ToolDeadlineOwner::CoreToolDispatch, 600),
                finite(ToolDeadlineOwner::DetachedSubmission, 10),
            ]
        );
        assert_eq!(plan.kind(), &ResolvedExecutionKind::Detached(detached));
    }

    #[test]
    fn mode_resolution_appends_its_own_absolute_deadline() {
        let upstream =
            ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                .expect("valid upstream deadline");
        let streaming =
            StreamingToolExecutionPolicy::new(Duration::from_secs(5), Duration::from_secs(60))
                .expect("valid streaming policy");
        let streaming_contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Streaming]),
            ToolExecutionMode::Streaming,
            Some(streaming),
            None,
        )
        .expect("valid streaming contract");

        let streaming_plan = streaming_contract
            .resolve_default(upstream.clone())
            .expect("streaming mode resolves");
        assert_eq!(
            streaming_plan.deadlines().contributors(),
            &[
                finite(ToolDeadlineOwner::CoreToolDispatch, 600),
                finite(ToolDeadlineOwner::StreamingAbsolute, 60),
            ]
        );

        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("homecore.security_scan", "v1").expect("valid runner"),
            RestartClass::NonResumable,
            IdempotencyScope::InteractionAndArguments,
            Duration::from_secs(10),
        )
        .expect("valid detached policy");
        let detached_contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(detached),
        )
        .expect("valid detached contract");

        let detached_plan = detached_contract
            .resolve_default(upstream)
            .expect("detached mode resolves");
        assert_eq!(
            detached_plan.deadlines().contributors(),
            &[
                finite(ToolDeadlineOwner::CoreToolDispatch, 600),
                finite(ToolDeadlineOwner::DetachedSubmission, 10),
            ]
        );
    }

    #[test]
    fn advertised_contract_rejects_an_unadvertised_resolved_mode() {
        let upstream =
            ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                .expect("valid upstream deadline");
        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("dishonest.runner", "v1").expect("valid runner"),
            RestartClass::NonResumable,
            IdempotencyScope::ToolCall,
            Duration::from_secs(10),
        )
        .expect("valid detached policy");
        let detached_contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(detached),
        )
        .expect("valid detached contract");
        let detached_plan = detached_contract
            .resolve_default(upstream)
            .expect("detached plan resolves");

        assert_eq!(
            ToolExecutionContract::default().validate_resolved_plan(&detached_plan),
            Err(ToolExecutionContractError::RequestedModeUnsupported {
                requested_mode: ToolExecutionMode::Detached,
            })
        );
    }

    #[test]
    fn advertised_contract_rejects_forged_mode_derived_facets() {
        let contract = ToolExecutionContract::default();
        let mut plan = contract
            .resolve_default(
                ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                    .expect("valid deadline"),
            )
            .expect("fast plan resolves");
        plan.output_policy =
            ToolExecutionApplicability::Applicable(ToolOutputPolicy::DurableJobResult);

        assert_eq!(
            contract.validate_resolved_plan(&plan),
            Err(ToolExecutionContractError::ResolvedPlanFacetMismatch {
                mode: ToolExecutionMode::Fast,
                facet: "output_policy",
            })
        );
    }

    #[test]
    fn logical_binding_fingerprint_ignores_projection_arc_rebuild() {
        let tool = || {
            std::sync::Arc::new(crate::ToolDef::new(
                "stable_tool",
                "stable declaration",
                serde_json::json!({"type": "object", "properties": {}}),
            ))
        };
        let original = crate::ToolCatalogEntry::session_inline(tool(), true);
        let rebuilt = crate::ToolCatalogEntry::session_inline(tool(), true);
        let changed = crate::ToolCatalogEntry::session_inline(
            std::sync::Arc::new(crate::ToolDef::new(
                "stable_tool",
                "replacement declaration",
                serde_json::json!({"type": "object", "properties": {}}),
            )),
            true,
        );

        assert_eq!(
            ephemeral_tool_catalog_binding_fingerprint(&original),
            ephemeral_tool_catalog_binding_fingerprint(&rebuilt),
            "equivalent projection allocations are one live logical binding"
        );
        assert_ne!(
            ephemeral_tool_catalog_binding_fingerprint(&original),
            ephemeral_tool_catalog_binding_fingerprint(&changed),
            "name reuse with a different declaration must be fenced"
        );
    }

    #[test]
    fn resolution_context_rejects_replaced_or_reordered_upstream_deadlines() {
        let upstream = ToolDeadlineChain::new(vec![
            finite(ToolDeadlineOwner::CoreToolDispatch, 600),
            finite(ToolDeadlineOwner::Dispatcher, 30),
        ])
        .expect("valid upstream deadline");
        let context = ToolExecutionResolutionContext::new(upstream.clone());
        let valid_plan = ToolExecutionContract::default()
            .resolve_default(
                upstream
                    .with_contributor(finite(ToolDeadlineOwner::ToolInternal, 10))
                    .expect("valid extension"),
            )
            .expect("default contract resolves");
        context
            .validate_resolved_plan(&valid_plan)
            .expect("ordered extension is valid");

        let replaced_plan = ToolExecutionContract::default()
            .resolve_default(
                ToolDeadlineChain::new(vec![
                    finite(ToolDeadlineOwner::CoreToolDispatch, 600),
                    finite(ToolDeadlineOwner::ToolInternal, 10),
                ])
                .expect("valid but replaced chain"),
            )
            .expect("default contract resolves");
        assert_eq!(
            context.validate_resolved_plan(&replaced_plan),
            Err(ToolExecutionResolutionError::DeadlineExtension(
                DeadlineChainExtensionError::ContributorMismatch {
                    index: 1,
                    expected: finite(ToolDeadlineOwner::Dispatcher, 30),
                    actual: finite(ToolDeadlineOwner::ToolInternal, 10),
                }
            ))
        );

        let shorter_plan = ToolExecutionContract::default()
            .resolve_default(
                ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                    .expect("valid but shorter chain"),
            )
            .expect("default contract resolves");
        assert_eq!(
            context.validate_resolved_plan(&shorter_plan),
            Err(ToolExecutionResolutionError::DeadlineExtension(
                DeadlineChainExtensionError::ShorterThanUpstream {
                    upstream_len: 2,
                    resolved_len: 1,
                }
            ))
        );
    }

    #[test]
    fn resolved_plan_exposes_explicit_mode_applicability_and_owner_witness() {
        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("homecore.security_scan", "v1").expect("valid runner"),
            RestartClass::NonResumable,
            IdempotencyScope::InteractionAndArguments,
            Duration::from_secs(10),
        )
        .expect("valid detached policy")
        .with_credential_scopes(["network"]);
        let contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(detached),
        )
        .expect("valid detached contract");
        let witness = ToolExecutionOwnerWitness::new(
            "dynamic-composite:1",
            "security_scan",
            ephemeral_fingerprint("security_scan"),
        )
        .expect("valid owner witness");
        let plan = contract
            .resolve_default(
                ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                    .expect("valid deadline"),
            )
            .expect("detached contract resolves")
            .with_owner_witness(witness.clone())
            .expect("first owner witness is accepted");

        assert_eq!(
            plan.runner(),
            &ToolExecutionApplicability::Applicable(
                RunnerIdentity::new("homecore.security_scan", "v1").expect("valid runner")
            )
        );
        assert_eq!(
            plan.restart_class(),
            ToolExecutionApplicability::Applicable(RestartClass::NonResumable)
        );
        assert_eq!(
            plan.idempotency_scope(),
            ToolExecutionApplicability::Applicable(IdempotencyScope::InteractionAndArguments)
        );
        assert_eq!(
            plan.credential_context_refs(),
            &ToolExecutionApplicability::Applicable(vec![
                ToolCredentialContextRef::OwningProfile {
                    required_scopes: BTreeSet::from(["network".to_string()]),
                },
            ])
        );
        assert_eq!(
            plan.output_policy(),
            &ToolExecutionApplicability::Applicable(ToolOutputPolicy::DurableJobResult)
        );
        assert_eq!(
            plan.progress_policy(),
            &ToolExecutionApplicability::Applicable(ToolProgressPolicy::DurableJobEvents)
        );
        assert_eq!(plan.owner_witness("dynamic-composite:1"), Some(&witness));

        let fast = ToolExecutionContract::default()
            .resolve_default(
                ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                    .expect("valid deadline"),
            )
            .expect("fast contract resolves");
        assert_eq!(fast.runner(), &ToolExecutionApplicability::NotApplicable);
        assert_eq!(
            fast.credential_context_refs(),
            &ToolExecutionApplicability::NotApplicable
        );
        assert_eq!(
            fast.output_policy(),
            &ToolExecutionApplicability::Applicable(ToolOutputPolicy::InlineTerminal)
        );
        assert_eq!(
            fast.progress_policy(),
            &ToolExecutionApplicability::NotApplicable
        );
    }

    #[test]
    fn detached_contract_can_advertise_and_validate_call_resolved_restart_classes() {
        let policy = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("meerkat.monitor_script", "v1").expect("runner"),
            RestartClass::NonResumable,
            IdempotencyScope::ToolCall,
            Duration::from_secs(30),
        )
        .expect("policy");
        let contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(policy),
        )
        .expect("contract")
        .with_detached_restart_classes(BTreeSet::from([
            RestartClass::Replayable,
            RestartClass::CheckpointResumable,
            RestartClass::NonResumable,
        ]))
        .expect("restart set");
        let deadlines =
            ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                .expect("deadlines");
        let resolved = contract
            .resolve_detached_with_restart_class(
                RestartClass::CheckpointResumable,
                deadlines.clone(),
            )
            .expect("resolved");
        assert_eq!(
            resolved.restart_class(),
            ToolExecutionApplicability::Applicable(RestartClass::CheckpointResumable)
        );
        contract
            .validate_resolved_plan(&resolved)
            .expect("resolved class remains within the advertised contract");
        assert_eq!(
            contract.resolve_detached_with_restart_class(RestartClass::Adoptable, deadlines),
            Err(
                ToolExecutionContractError::RequestedRestartClassUnsupported {
                    restart_class: RestartClass::Adoptable
                }
            )
        );
    }

    #[test]
    fn owner_witness_rejects_blank_owner_keys() {
        assert_eq!(
            ToolExecutionOwnerWitness::new(
                "authority:a",
                "  ",
                ephemeral_fingerprint("blank-owner"),
            ),
            Err(ToolExecutionDeclarationError::EmptyOwnerWitnessKey)
        );

        assert_eq!(
            ToolExecutionOwnerWitness::new(
                "  ",
                "owner:a",
                ephemeral_fingerprint("blank-authority"),
            ),
            Err(ToolExecutionDeclarationError::EmptyOwnerWitnessAuthorityKey)
        );

        let original = ToolExecutionOwnerWitness::new(
            "authority:a",
            "owner:a",
            ephemeral_fingerprint("original"),
        )
        .expect("valid witness");
        let nested = ToolExecutionOwnerWitness::new(
            "authority:b",
            "owner:b",
            ephemeral_fingerprint("nested"),
        )
        .expect("valid witness");
        let attempted = ToolExecutionOwnerWitness::new(
            "authority:a",
            "owner:c",
            ephemeral_fingerprint("attempted"),
        )
        .expect("valid witness");
        let plan = ToolExecutionContract::default()
            .resolve_default(
                ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                    .expect("valid deadline"),
            )
            .expect("fast contract resolves")
            .with_owner_witness(original.clone())
            .expect("first witness is accepted")
            .with_owner_witness(nested.clone())
            .expect("a nested authority may append its own witness");
        assert_eq!(plan.owner_witness("authority:a"), Some(&original));
        assert_eq!(plan.owner_witness("authority:b"), Some(&nested));
        assert_eq!(
            plan.with_owner_witness(attempted.clone()),
            Err(ToolExecutionResolutionError::OwnerWitnessAlreadyAssigned {
                existing: Box::new(original),
                attempted: Box::new(attempted),
            })
        );
    }
}
