//! Trusted required-resource checks over a freshly admitted native input.
//! Profiles narrow dispatch and interpret an actual committed tool result. They
//! do not grant native access, authenticate a caller, or prove another resource.
use crate::ConnectError;
use meerkat::governed_realm::stock::caller::{
    StockAuthenticatedCaller, StockCallerAccess, StockCallerRequest, StockLoginLifetime,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_core::{
    OperationAuthorizationError, OperationRefusalKind, OperationRefused, PrincipalRef, SessionId,
    ToolResult,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) mod native_host;
pub use native_host::{NativeHostClosed, NativeHostFence};

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(u64::MAX)
}
pub(crate) fn unavailable() -> ConnectError {
    ConnectError::InvalidInput("The selected governed service probe is unavailable; no replacement authority or successful capability report was created".into())
}
fn name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

/// The host's authenticated transport binding, selected before pipe startup.
/// None of these values are inferred from a model or returned browser answer.
pub struct ProbeHostBinding {
    pub application_id: String,
    pub instance_id: String,
    pub origin: String,
    pub viewer_id: String,
    pub login_scope_id: String,
    pub expires_at_ms: u64,
    pub realm: meerkat_core::RealmLocator,
    pub context_root: std::path::PathBuf,
    pub user_config_root: std::path::PathBuf,
    pub caller: StockAuthenticatedCaller,
    pub ingress: PrincipalRef,
}

/// Resource-semantic interpretation supplied by the trusted application host.
/// Implementations must bind the exact resource selector and required operation
/// to the actual result schema. Generic success is not evidence of that resource.
/// This Rust-only contract cannot be supplied through initialization or a prompt.
pub trait ProbeResultContract: Send + Sync {
    /// Check the selected installed native connection and operation against the
    /// exact host-selected resource before any new input is admitted. This must
    /// reject a foreign endpoint/account mapping even when a payload could look
    /// like the expected result. No browser-provided predicate is accepted.
    fn accepts_target(&self, selection: &ProbeResourceTarget<'_>) -> bool;
    fn assess(&self, observation: &CommittedProbeResult<'_>) -> ResourceAssessment;
}

/// The exact trusted resource requirement and installed native connection.
/// The contract checks this target before admitting an input and again before
/// interpreting the committed tool result.
pub struct ProbeResourceTarget<'a> {
    pub profile_id: &'a str,
    pub resource: &'a Value,
    pub raw_operation: &'a str,
    pub arguments: &'a Value,
    pub installed_connection: &'a meerkat_core::McpServerConfig,
}

/// Disclosed only after exact native call, run, input, durable terminal and
/// joined close checks. This is not a model's textual answer.
pub struct CommittedProbeResult<'a> {
    pub target: ProbeResourceTarget<'a>,
    pub result: &'a ToolResult,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAssessment {
    Verified,
    NotVerified,
    Unknown,
}

/// One dynamic workspace/resource requirement, chosen by trusted Rust code.
/// Arguments are nonsecret operation parameters; credentials remain native.
/// `family` and `profile` must match the actual checked installation receipt.
/// No default result contract is supplied and no success boolean is accepted.
pub struct RequiredServiceProbe {
    pub(crate) id: String,
    pub(crate) family: String,
    pub(crate) profile: String,
    pub(crate) agent: String,
    pub(crate) operation: String,
    pub(crate) arguments: Value,
    pub(crate) resource: Value,
    pub(crate) contract: Option<Arc<dyn ProbeResultContract>>,
}
impl RequiredServiceProbe {
    pub fn outbound_mcp(
        id: String,
        family: String,
        profile: String,
        agent: String,
        operation: String,
        arguments: Value,
        resource: Value,
        contract: Option<Arc<dyn ProbeResultContract>>,
    ) -> Result<Self, ConnectError> {
        if [&id, &family, &profile, &agent, &operation]
            .iter()
            .any(|value| !name(value))
            || !arguments.is_object()
            || arguments.to_string().len() > 32 * 1024
            || resource.is_null()
            || resource.to_string().len() > 4096
        {
            return Err(unavailable());
        }
        Ok(Self {
            id,
            family,
            profile,
            agent,
            operation,
            arguments,
            resource,
            contract,
        })
    }
}

/// The one authenticated host allocation retained across setup, commissioning,
/// finalization and native calls. Its fields are not wire inputs or mutable
/// credential-currentness evidence. Only a trusted selection creates it.
pub struct ServiceHost {
    pub(crate) binding: ProbeHostBinding,
    pub(crate) lifetime: Arc<ProbeLifetime>,
    pub(crate) collection_identity: Arc<()>,
}
impl ServiceHost {
    pub(crate) fn new(binding: ProbeHostBinding) -> Result<Arc<Self>, ConnectError> {
        if [
            &binding.application_id,
            &binding.instance_id,
            &binding.viewer_id,
            &binding.login_scope_id,
        ]
        .iter()
        .any(|value| !name(value))
            || !binding.context_root.is_absolute()
            || !binding.user_config_root.is_absolute()
            || !binding.realm.state_root.is_absolute()
            || binding.expires_at_ms <= now()
            || binding.ingress.validate_qualified().is_err()
            || binding.caller.requester() == &binding.ingress
        {
            return Err(unavailable());
        }
        Ok(Arc::new(Self {
            binding,
            lifetime: Arc::new(ProbeLifetime::default()),
            collection_identity: Arc::new(()),
        }))
    }
    pub(crate) fn control(&self) -> ServiceProbeControl {
        ServiceProbeControl(self.lifetime.clone())
    }
}

/// Trusted process composition for explicit protocol 6. This is not serialized.
/// Its nonempty fixed-profile contract remains unchanged by protocol 7.
pub struct ServiceProbeSelection {
    pub(crate) host: Arc<ServiceHost>,
    pub(crate) profiles: Vec<Arc<RequiredServiceProbe>>,
}
impl std::ops::Deref for ServiceProbeSelection {
    type Target = ServiceHost;
    fn deref(&self) -> &ServiceHost {
        &self.host
    }
}
impl ServiceProbeSelection {
    pub fn new(
        binding: ProbeHostBinding,
        profiles: Vec<RequiredServiceProbe>,
    ) -> Result<Self, ConnectError> {
        let mut ids = BTreeSet::new();
        if profiles.is_empty()
            || profiles.len() > 16
            || profiles
                .iter()
                .any(|profile| !ids.insert(profile.id.clone()))
        {
            return Err(unavailable());
        }
        Ok(Self {
            host: ServiceHost::new(binding)?,
            profiles: profiles.into_iter().map(Arc::new).collect(),
        })
    }
    /// Retires admission; the pipe owner still joins any entered native work.
    pub fn control(&self) -> ServiceProbeControl {
        self.host.control()
    }
}
#[derive(Clone)]
pub struct ServiceProbeControl(Arc<ProbeLifetime>);
impl ServiceProbeControl {
    pub fn retire(&self) {
        self.0.retire();
    }

    /// Read the bounded private setup observation retained after its actual
    /// owner close. None means unobserved, never successful setup or cleanup.
    /// This diagnostic is not authority, admission, or a settlement receipt.
    /// Truncated or omitted guard batches are identified in the observation.
    pub fn private_setup_diagnostic(&self) -> Option<Value> {
        self.0.private_setup_diagnostic.get().cloned()
    }

    /// Retire this exact host's local native admission and check its joined
    /// publication fences. This does not wait for entered work or authenticate
    /// a parent. Use the returned observation to await successful invocation
    /// joins while continuing to drive the actual stdio invocation.
    ///
    /// Failure still attempts retirement, but returns no acknowledgement.
    /// Neither this receipt nor `retire` is a cross-process acknowledgement,
    /// an original-parent reservation or a native effect outcome.
    pub fn fence(&self) -> Result<NativeHostFence, ConnectError> {
        NativeHostFence::establish(self.0.clone())
    }
}

/// Retained trusted intent, never decoded from browser or model arguments.
#[derive(Clone)]
pub(crate) enum ServiceSelection {
    Probes(Arc<ServiceProbeSelection>),
    Collections(Arc<crate::service_collection::ServiceCollectionSelection>),
}
impl ServiceSelection {
    #[cfg(target_os = "linux")]
    pub(crate) fn selected_input(
        &self,
    ) -> Option<Arc<crate::protected_parent::selected_input::SelectedInput>> {
        match self {
            Self::Collections(s) if s.services.len() == 1 => s.selected_input.clone(),
            _ => None,
        }
    }
    pub(crate) fn host(&self) -> &Arc<ServiceHost> {
        match self {
            Self::Probes(s) => &s.host,
            Self::Collections(s) => &s.host,
        }
    }
    pub(crate) fn version(&self) -> u8 {
        match self {
            Self::Probes(_) => 6,
            Self::Collections(_) => 7,
        }
    }
    pub(crate) fn permits_review(
        &self,
        family: &str,
        profile: &str,
        agent: &str,
        operations: &BTreeSet<String>,
        connection: &meerkat_core::McpServerConfig,
    ) -> bool {
        match self {
            // Preserve v6's existing matched-profile permission check.
            Self::Probes(s) => s.profiles.iter().any(|p| {
                p.family == family
                    && p.profile == profile
                    && p.agent == agent
                    && operations.contains(&p.operation)
                    && connection
                        .access
                        .as_ref()
                        .is_some_and(|access| access.allowed_operations.contains(&p.operation))
            }),
            Self::Collections(s) => {
                s.permits_review(family, profile, agent, operations, connection)
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct ProbeLifetime {
    retired: AtomicBool,
    publications: Mutex<Vec<LocalAuthorizationPublication>>,
    retired_notice: tokio::sync::Notify,
    invocation: native_host::InvocationCustody,
    private_setup_diagnostic: OnceLock<Value>,
}
impl ProbeLifetime {
    pub(crate) fn retain_private_setup_diagnostic(&self, diagnostic: Value) {
        // Observation only: this cell is never read by admission or cleanup.
        let _ = self.private_setup_diagnostic.set(diagnostic);
    }
    pub(crate) fn current(&self, deadline: u64) -> bool {
        !self.retired.load(Ordering::Acquire) && now() < deadline
    }
    pub(crate) fn join(
        &self,
        publication: LocalAuthorizationPublication,
    ) -> Result<(), ConnectError> {
        let mut publications = self.publications.lock().map_err(|_| unavailable())?;
        if self.retired.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        publications.push(publication);
        Ok(())
    }
    pub(crate) async fn closed(&self, deadline: u64) {
        loop {
            let retired = self.retired_notice.notified();
            tokio::pin!(retired);
            retired.as_mut().enable();
            if !self.current(deadline) {
                return;
            }
            tokio::select! { _ = retired => (),
            _ = tokio::time::sleep(std::time::Duration::from_millis(deadline.saturating_sub(now()).min(60_000))) => () }
        }
    }
    pub(crate) fn retire(&self) {
        let _ = self.retire_checked();
    }
    fn retire_checked(&self) -> Result<(), ConnectError> {
        let result = match self.publications.lock() {
            Ok(publications) => {
                // Policy reads use only immutable selection plus the atomic
                // retirement fact, so this publication change cannot reenter
                // the custody mutex. Keep all changes open across the mutation.
                let changes: Vec<_> = publications
                    .iter()
                    .map(|p| p.begin_owner_change())
                    .collect();
                let confirmed = changes.iter().all(Result::is_ok);
                self.retired.store(true, Ordering::Release);
                drop(changes);
                if confirmed {
                    Ok(())
                } else {
                    Err(unavailable())
                }
            }
            Err(_) => {
                self.retired.store(true, Ordering::Release);
                Err(unavailable())
            }
        };
        self.retired_notice.notify_waiters();
        result
    }
}

pub(crate) struct ProbeAccess {
    pub(crate) lifetime: Arc<ProbeLifetime>,
    pub(crate) expires_at_ms: u64,
    pub(crate) caller: StockAuthenticatedCaller,
    pub(crate) executor: PrincipalRef,
    pub(crate) owner: PrincipalRef,
    pub(crate) session: SessionId,
}
impl StockCallerAccess for ProbeAccess {
    fn authorize_caller(
        &self,
        request: &StockCallerRequest<'_>,
    ) -> Result<StockLoginLifetime, OperationAuthorizationError> {
        if !self.lifetime.current(self.expires_at_ms)
            || request.caller() != self.caller.requester()
            || request.authentication() != Some(self.caller.authentication())
            || request.executor() != &self.executor
            || request.owner() != &self.owner
            || request.session() != &self.session
        {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        Ok(StockLoginLifetime::ExpiresAt {
            unix_ms: self.expires_at_ms,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    GovernedPrerequisiteUnavailable,
    SelectionChanged,
    NotObserved,
    NativeRefused,
    OutcomeUnknown,
    ToolFailed,
    ToolCompleted,
    CapabilityNotVerified,
    CapabilityVerified,
    Cancelled,
    CleanupUnconfirmed,
    Unsupported,
}
#[derive(Serialize)]
pub struct ServiceProbeReport {
    pub profile_id: String,
    pub application_id: String,
    pub instance_id: String,
    pub login_scope_id: String,
    pub viewer_id: String,
    pub origin: String,
    pub realm: String,
    pub session_id: SessionId,
    pub job_id: String,
    pub server: String,
    pub agent: String,
    pub operation: String,
    pub resource: Value,
    pub status: ProbeStatus,
    pub input_id: Option<meerkat_core::InputId>,
    pub run_id: Option<meerkat_core::RunId>,
    pub tool_call_id: Option<String>,
    pub result_sha256: Option<String>,
    pub installed_connection_sha256: Option<String>,
    pub dispatch_outcome: Option<meerkat_core::LiveBridgeEffectOutcome>,
    pub native_terminal_committed: bool,
    pub native_close_joined: bool,
}

#[cfg(test)]
pub(crate) mod tests;
