//! Trusted bounded service collections over one retained native Ready owner.
//! A completed historical check does not establish reusable credential coverage.
//! This module accepts no browser/model program and no caller-authored receipt.
use crate::{
    ConnectError,
    service_probe::{ResourceAssessment, ServiceProbeReport},
};
use meerkat_core::{McpServerConfig, ToolResult};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc};

pub(crate) fn refused() -> ConnectError {
    ConnectError::InvalidInput("The selected native service collection cannot continue; partial observations remain historical and reusable source coverage is unqualified".into())
}
fn name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

/// Finite bounds are shared by every required operation in this attempt.
/// Arguments must be nonsecret parameters. Credentials stay with native owners.
#[derive(Clone, Copy, Serialize)]
pub struct CollectionLimits {
    pub(crate) calls: usize,
    pub(crate) argument_bytes: usize,
    pub(crate) result_bytes: usize,
}
impl CollectionLimits {
    pub fn new(
        calls: usize,
        argument_bytes: usize,
        result_bytes: usize,
    ) -> Result<Self, ConnectError> {
        if !(1..=256).contains(&calls)
            || !(1..=8 * 1024 * 1024).contains(&argument_bytes)
            || !(1..=32 * 1024 * 1024).contains(&result_bytes)
        {
            return Err(refused());
        }
        Ok(Self {
            calls,
            argument_bytes,
            result_bytes,
        })
    }
}

/// One exact next call chosen by trusted code, never a model-authored plan.
/// The engine still checks the requirement ceiling, budgets and replay key;
/// native grants and dispatch admission remain independently authoritative.
#[derive(Clone)]
pub struct CollectionCall {
    pub(crate) operation: String,
    pub(crate) arguments: Value,
    pub(crate) read_only: bool,
}
impl CollectionCall {
    pub fn new(operation: String, arguments: Value) -> Result<Self, ConnectError> {
        if !name(&operation) || !arguments.is_object() || arguments.to_string().len() > 32 * 1024 {
            return Err(refused());
        }
        Ok(Self {
            operation,
            arguments,
            read_only: false,
        })
    }
    /// Request native read-only enforcement. This is intent, not a declaration
    /// of effect: the exact installed operation owner must independently agree.
    pub fn read_only(operation: String, arguments: Value) -> Result<Self, ConnectError> {
        let mut call = Self::new(operation, arguments)?;
        call.read_only = true;
        Ok(call)
    }
}

/// A bounded request to observe the same read again. Only the engine can issue
/// this handle after an enforced read commits and closes. Copies share one use;
/// there are no replacement arguments, public constructor or wire decoder.
#[derive(Clone)]
pub struct ReadObservation {
    pub(crate) attempt: Arc<()>,
    pub(crate) ordinal: usize,
}

pub enum CollectionStep {
    Call(CollectionCall),
    ReobserveRead(ReadObservation),
    Finish(ResourceAssessment),
    Refuse,
}

/// Exact immutable intent plus the actual installed native descriptor. This is
/// an observed selection, not a credential-generation or currentness lease.
pub struct CollectionTarget<'a> {
    pub(crate) source_id: &'a str,
    pub(crate) requirement_id: &'a str,
    pub(crate) resource: &'a Value,
    pub(crate) operations: &'a BTreeSet<String>,
    pub(crate) installed: &'a McpServerConfig,
}
impl<'a> CollectionTarget<'a> {
    pub fn source_id(&self) -> &str {
        self.source_id
    }
    pub fn required_operation_id(&self) -> &str {
        self.requirement_id
    }
    pub fn resource(&self) -> &Value {
        self.resource
    }
    pub fn raw_operation_ceiling(&self) -> &BTreeSet<String> {
        self.operations
    }
    pub fn installed_connection(&self) -> &McpServerConfig {
        self.installed
    }
}

/// A factory must create a fresh per-attempt cursor. Its target predicate and
/// result-schema program bind the exact resource and native endpoint/account
/// selection. Neither generic tool success nor prose is resource evidence.
/// Callbacks are synchronous trusted application code, not a sandboxed language.
pub trait CollectionProgram: Send + Sync {
    fn accepts_target(&self, target: &CollectionTarget<'_>) -> bool;
    fn start(&self, target: &CollectionTarget<'_>) -> Box<dyn CollectionCursor>;
    /// Trusted host classification for these exact installed software bytes.
    /// Pin reviewed release identities and the raw operation; do not copy the
    /// package's claimed effect or accept a server name as software identity.
    /// This grants no access and never overrides a native Mutating class.
    /// The default leaves selected software Unknown under read-only policy.
    fn declares_read_only(
        &self,
        _installed: &SelectedPackageIdentity<'_>,
        _raw_operation: &str,
    ) -> bool {
        false
    }
}

/// A checked identity view supplied only to a trusted collection program.
/// Package declarations remain untrusted. Receipt and frozen input checks
/// establish which software was selected, not that its behavior is read-only.
/// There is no public constructor, wire representation or credential value.
pub struct SelectedPackageIdentity<'a> {
    pub(crate) package: &'a toolkit_package::PackageIdentity,
    pub(crate) package_sha256: &'a str,
    pub(crate) selection_document: &'a str,
    pub(crate) selection_sha256: &'a str,
    pub(crate) effect_sha256: &'a str,
    pub(crate) receipt_sha256: &'a str,
    pub(crate) component: &'a str,
    pub(crate) service: &'a str,
    pub(crate) artifact_sha256: &'a str,
    pub(crate) installed: &'a McpServerConfig,
}
impl SelectedPackageIdentity<'_> {
    pub fn package(&self) -> &toolkit_package::PackageIdentity {
        self.package
    }
    pub fn package_sha256(&self) -> &str {
        self.package_sha256
    }
    pub fn selection_document(&self) -> &str {
        self.selection_document
    }
    pub fn selection_sha256(&self) -> &str {
        self.selection_sha256
    }
    /// Exact retained setup proposal identity, not a read classification.
    pub fn effect_sha256(&self) -> &str {
        self.effect_sha256
    }
    pub fn receipt_sha256(&self) -> &str {
        self.receipt_sha256
    }
    pub fn component(&self) -> &str {
        self.component
    }
    pub fn service(&self) -> &str {
        self.service
    }
    pub fn artifact_sha256(&self) -> &str {
        self.artifact_sha256
    }
    pub fn installed_connection(&self) -> &McpServerConfig {
        self.installed
    }
}
pub trait CollectionCursor: Send {
    fn begin(&mut self) -> CollectionStep;
    fn observe(&mut self, result: &CommittedCollectionResult<'_>) -> CollectionStep;
}

/// Historical correlation of committed results from the same retained host
/// invocation. No public constructor/decoder, access grant, currentness check or
/// credential identity is provided. Equal labels in another host do not match.
#[derive(Clone)]
pub struct CollectionInvocationObservation(pub(crate) Arc<()>);
impl CollectionInvocationObservation {
    pub fn same_invocation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// The engine alone constructs this view after native call/run/input/durable
/// terminal correlation and joined close. It is unavailable for an uncertain,
/// failed, cancelled or unconfirmed-cleanup result. The view belongs only to
/// this source/requirement and cannot be fed back as a receipt by the caller.
pub struct CommittedCollectionResult<'a> {
    pub(crate) target: CollectionTarget<'a>,
    pub(crate) operation: &'a str,
    pub(crate) arguments: &'a Value,
    pub(crate) native: &'a ServiceProbeReport,
    pub(crate) result: &'a ToolResult,
    pub(crate) read_observation: Option<ReadObservation>,
    pub(crate) invocation: CollectionInvocationObservation,
}
impl<'a> CommittedCollectionResult<'a> {
    pub fn target(&self) -> &CollectionTarget<'a> {
        &self.target
    }
    pub fn operation(&self) -> &str {
        self.operation
    }
    pub fn arguments(&self) -> &Value {
        self.arguments
    }
    pub fn native_observation(&self) -> &ServiceProbeReport {
        self.native
    }
    pub fn result(&self) -> &ToolResult {
        self.result
    }
    pub fn read_observation(&self) -> Option<ReadObservation> {
        self.read_observation.clone()
    }
    pub fn invocation_observation(&self) -> CollectionInvocationObservation {
        self.invocation.clone()
    }
}

pub struct RequiredCollectionOperation {
    pub(crate) id: String,
    pub(crate) resource: Value,
    pub(crate) operations: BTreeSet<String>,
    pub(crate) program: Arc<dyn CollectionProgram>,
}
impl RequiredCollectionOperation {
    pub fn new(
        id: String,
        resource: Value,
        raw_operation_ceiling: Vec<String>,
        program: Arc<dyn CollectionProgram>,
    ) -> Result<Self, ConnectError> {
        let operations: BTreeSet<_> = raw_operation_ceiling.iter().cloned().collect();
        if !name(&id)
            || resource.is_null()
            || resource.to_string().len() > 4096
            || operations.is_empty()
            || operations.len() > 32
            || operations.len() != raw_operation_ceiling.len()
            || operations.iter().any(|value| !name(value))
        {
            return Err(refused());
        }
        Ok(Self {
            id,
            resource,
            operations,
            program,
        })
    }
}

/// One immutable source ID with its complete explicit operation/resource set.
/// Different source IDs are never merged by feature, service name or prose.
/// This object is Rust-only and grants no runtime or credential authority.
pub struct RequiredServiceCollection {
    pub(crate) source_id: String,
    pub(crate) family: String,
    pub(crate) profile: String,
    pub(crate) agent: String,
    pub(crate) requirements: Vec<RequiredCollectionOperation>,
    pub(crate) limits: CollectionLimits,
}
impl RequiredServiceCollection {
    pub fn new(
        source_id: String,
        family: String,
        profile: String,
        agent: String,
        requirements: Vec<RequiredCollectionOperation>,
        limits: CollectionLimits,
    ) -> Result<Self, ConnectError> {
        let mut ids = BTreeSet::new();
        if [&source_id, &family, &profile, &agent]
            .iter()
            .any(|value| !name(value))
            || requirements.is_empty()
            || requirements.len() > 64
            || requirements
                .iter()
                .any(|value| !ids.insert(value.id.clone()))
        {
            return Err(refused());
        }
        Ok(Self {
            source_id,
            family,
            profile,
            agent,
            requirements,
            limits,
        })
    }
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
    pub fn required_operation_ids(&self) -> impl Iterator<Item = &str> {
        self.requirements.iter().map(|r| r.id.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionStatus {
    HistoricalChecksCompleted,
    NotVerified,
    NotObserved,
    NativeStopped,
    GovernedPrerequisiteUnavailable,
    NativeRefused,
    OutcomeUnknown,
    ToolFailed,
    CleanupUnconfirmed,
    Unsupported,
    SelectionChanged,
    Cancelled,
    ProgramRefused,
    ProgramPanicked,
    AttemptAlreadyObserved,
    ReplayRefused,
    BudgetExceeded,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedSelectionFreshness {
    NotObserved,
    RecheckedAfterLastClose,
    ChangedOrUnconfirmed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReusableSourceCoverage {
    /// No supported native opaque credential-continuity witness is bound here.
    /// Config digests, attachment generations and secret presence cannot fill it.
    UnqualifiedCredentialContinuity,
}

#[derive(Serialize)]
pub struct CollectionCallObservation {
    pub(crate) source_id: String,
    pub(crate) required_operation_id: String,
    pub(crate) resource: Value,
    pub(crate) ordinal: usize,
    pub(crate) raw_operation: String,
    pub(crate) arguments_sha256: String,
    pub(crate) native: ServiceProbeReport,
}
#[derive(Serialize)]
pub struct RequiredOperationObservation {
    pub(crate) required_operation_id: String,
    pub(crate) resource: Value,
    pub(crate) raw_operation_ceiling: BTreeSet<String>,
    pub(crate) assessment: Option<ResourceAssessment>,
    pub(crate) calls: Vec<CollectionCallObservation>,
}
#[derive(Serialize)]
pub struct ServiceCollectionReport {
    pub(crate) source_id: String,
    pub(crate) attempt_id: String,
    pub(crate) application_id: String,
    pub(crate) instance_id: String,
    pub(crate) viewer_id: String,
    pub(crate) login_scope_id: String,
    pub(crate) origin: String,
    pub(crate) realm: String,
    pub(crate) session_id: meerkat_core::SessionId,
    pub(crate) job_id: String,
    pub(crate) family: String,
    pub(crate) profile: String,
    pub(crate) agent: String,
    pub(crate) status: CollectionStatus,
    pub(crate) requirements: Vec<RequiredOperationObservation>,
    pub(crate) observed_selection: ObservedSelectionFreshness,
    pub(crate) native_own_config_generation: Option<u64>,
    pub(crate) reusable_source_coverage: ReusableSourceCoverage,
    pub(crate) limits: CollectionLimits,
    pub(crate) total_calls: usize,
    pub(crate) total_argument_bytes: usize,
    pub(crate) total_result_bytes: usize,
}
impl ServiceCollectionReport {
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
    pub fn status(&self) -> CollectionStatus {
        self.status
    }
    pub fn reusable_source_coverage(&self) -> ReusableSourceCoverage {
        self.reusable_source_coverage
    }
}

#[cfg(test)]
mod tests;

/// Trusted protocol7 intent for one installed service and caller. Each source
/// remains distinct. No profile, resource, program or operation is accepted on
/// the private wire. Explicit service groups retain separate actual setup and
/// installation owners under the same authenticated host invocation.
pub struct ServiceCollectionSelection {
    pub(crate) host: Arc<crate::service_probe::ServiceHost>,
    pub(crate) collections: Vec<Arc<RequiredServiceCollection>>,
    operations: BTreeSet<String>,
    pub(crate) services: Vec<ServiceGroup>,
    #[cfg(target_os = "linux")]
    pub(crate) selected_input: Option<Arc<crate::protected_parent::selected_input::SelectedInput>>,
}
#[derive(Clone)]
pub(crate) struct ServiceGroup {
    pub(crate) id: String,
    pub(crate) collections: Vec<Arc<RequiredServiceCollection>>,
    operations: BTreeSet<String>,
}
impl ServiceCollectionSelection {
    pub fn new(
        binding: crate::service_probe::ProbeHostBinding,
        collections: Vec<RequiredServiceCollection>,
    ) -> Result<Self, ConnectError> {
        Self::for_services(binding, vec![("default".into(), collections)])
    }
    /// Explicit ordered service selections under one original host. Each group
    /// retains its own exact family/profile/agent and operation union. Source
    /// IDs are unique across the invocation. This grants no native access.
    pub fn for_services(
        binding: crate::service_probe::ProbeHostBinding,
        selections: Vec<(String, Vec<RequiredServiceCollection>)>,
    ) -> Result<Self, ConnectError> {
        if selections.is_empty() || selections.len() > 16 {
            return Err(refused());
        }
        let mut service_ids = BTreeSet::new();
        let mut sources = BTreeSet::new();
        let mut all_operations = BTreeSet::new();
        let mut rows = 0;
        let mut collections = Vec::new();
        let mut services = Vec::new();
        for (id, source_group) in selections {
            if !name(&id) || !service_ids.insert(id.clone()) {
                return Err(refused());
            }
            let first = source_group.first().ok_or_else(refused)?;
            let mut operations = BTreeSet::new();
            for source in &source_group {
                if !sources.insert(source.source_id.clone())
                    || source.family != first.family
                    || source.profile != first.profile
                    || source.agent != first.agent
                {
                    return Err(refused());
                }
                rows += source.requirements.len();
                for requirement in &source.requirements {
                    operations.extend(requirement.operations.iter().cloned());
                }
            }
            all_operations.extend(operations.iter().cloned());
            if sources.len() > 16 || rows > 256 || all_operations.len() > 256 {
                return Err(refused());
            }
            let group: Vec<_> = source_group.into_iter().map(Arc::new).collect();
            collections.extend(group.iter().cloned());
            services.push(ServiceGroup {
                id,
                collections: group,
                operations,
            });
        }
        // The entire selection is never commissioned as one cross-service
        // union. The driver supplies only service_selection(active_index).
        Ok(Self {
            host: crate::service_probe::ServiceHost::new(binding)?,
            collections,
            operations: all_operations,
            services,
            #[cfg(target_os = "linux")]
            selected_input: None,
        })
    }
    pub fn service_ids(&self) -> impl Iterator<Item = &str> {
        self.services.iter().map(|s| s.id.as_str())
    }
    pub(crate) fn service_selection(&self, index: usize) -> Option<Arc<Self>> {
        let group = self.services.get(index)?.clone();
        Some(Arc::new(Self {
            host: self.host.clone(),
            collections: group.collections.clone(),
            operations: group.operations.clone(),
            #[cfg(target_os = "linux")]
            selected_input: self
                .selected_input
                .as_ref()
                .filter(|input| input.service_id() == group.id)
                .cloned(),
            services: vec![group],
        }))
    }
    /// Retires the original native admission. The retained pipe worker must
    /// still join entered operations and preserve any cleanup error owner.
    pub fn control(&self) -> crate::service_probe::ServiceProbeControl {
        self.host.control()
    }
    pub fn source_ids(&self) -> impl Iterator<Item = &str> {
        self.collections
            .iter()
            .map(|source| source.source_id.as_str())
    }
    pub(crate) fn permits_review(
        &self,
        family: &str,
        profile: &str,
        agent: &str,
        operations: &BTreeSet<String>,
        connection: &meerkat_core::McpServerConfig,
    ) -> bool {
        let first = &self.collections[0];
        self.services.len() == 1
            && first.family == family
            && first.profile == profile
            && first.agent == agent
            && operations == &self.operations
            && connection.access.as_ref().is_some_and(|access| {
                self.operations
                    .iter()
                    .all(|op| access.allowed_operations.contains(op))
            })
    }
}

#[cfg(test)]
mod selection_tests;

/// Failed protocol7 invocation with its actual body-free historical reports and
/// original owned error. It does not claim that a broken pipe delivered them.
/// Trusted Rust hosts may inspect this source via ConnectError::Native.
/// Display and Debug deliberately exclude the reports and inner diagnostics.
pub struct ServiceCollectionFailure {
    reports: Vec<ServiceCollectionReport>,
    error: ConnectError,
}
impl ServiceCollectionFailure {
    pub(crate) fn new(reports: Vec<ServiceCollectionReport>, error: ConnectError) -> Self {
        Self { reports, error }
    }
    pub fn reports(&self) -> &[ServiceCollectionReport] {
        &self.reports
    }
    pub fn original_error(&self) -> &ConnectError {
        &self.error
    }
}
impl std::fmt::Debug for ServiceCollectionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceCollectionFailure")
            .field("retained_reports", &self.reports.len())
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for ServiceCollectionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The collection invocation failed; historical reports and original error custody remain retained"
        )
    }
}
impl std::error::Error for ServiceCollectionFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}
