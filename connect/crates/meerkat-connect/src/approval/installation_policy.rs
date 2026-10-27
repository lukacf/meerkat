//! Trusted application provisioning policy for an exact checked installation.
//!
//! This is an in-process application composition, not a model/SDK wire policy,
//! a permission store, a grant or an authenticated actor constructor. The host
//! must supply its actual provisioning-policy owner. Merely implementing this
//! interface does not authenticate arbitrary application code.

use std::sync::Arc;
use meerkat_core::{ApprovalPrincipalId, PrincipalRef, RealmId, ToolCallView, agent::ToolDispatchContext};
use meerkat_toolkit::host_lifetime::{NativeHostLifetime, control::HostLifetimeError};
use serde_json::{Map, Value, json};
use toolkit_package::Id;

/// The exact retained software action, borrowed only for this policy decision.
/// The policy owner must check the artifact/effect/destination scope it actually
/// authorizes. Invocation permission and service/account grants remain separate.
/// The proposal is validated package evidence, never an authority claim.
pub struct InstallationPolicyRequest<'a> {
    pub application: &'a PrincipalRef,
    pub deployment: &'a Id,
    pub source_policy_sha256: &'a str,
    pub realm: &'a RealmId,
    pub call: ToolCallView<'a>,
    pub context: &'a ToolDispatchContext,
    pub proposal: &'a Value,
}

/// The actual host provisioning-policy adapter, installed by trusted host code.
/// Reserve is bounded and synchronous. It must authenticate its selected owner
/// and examine this exact request, not an agent claim or a previous approval.
/// It must not acquire the native publication/phase reservation, perform native
/// dispatch or call back into the selected NativeHostLifetime.
pub trait ApplicationInstallationPolicy: Send + Sync {
    fn reserve(&self, request: &InstallationPolicyRequest<'_>)
        -> Result<InstallationPolicyDecision, HostLifetimeError>;
}

pub enum InstallationPolicyDecision {
    /// No permit is retained while a human is awaited. Without a presenter,
    /// Toolkit returns a requires_action tool result to the setup invocation.
    /// The caller owns the transition to its actual human review surface.
    RequiresHuman,
    Permit(Box<dyn InstallationPolicyReservation>),
}

/// An owned reservation from the actual policy owner, not a cached allow bit.
/// While retained, that owner must exclude changes that would invalidate this
/// exact permission. It must not hold native phase/publication locks, and native
/// authorization checks must not deadlock trying to reenter its exclusion.
///
/// Toolkit checks currentness again before entry and retains the reservation
/// through the exact dispatch future. Settlement or dropping that future releases
/// it, while existing one-use/uncertain custody remains. Drop must be synchronous
/// and nonblocking. Cancellation does not prove an entered effect did not occur.
pub trait InstallationPolicyReservation: Send + Sync {
    fn owner(&self) -> &PrincipalRef;
    /// Nonsecret structured evidence from this policy decision, not a permit.
    fn provenance(&self) -> &Map<String, Value>;
    /// Bounded observation of this reservation only. Toolkit can invoke this
    /// while its approval state is locked; do not enter the native host lifetime,
    /// phase/publication, another Toolkit owner or a blocking policy writer.
    fn check_current(&self) -> Result<(), HostLifetimeError>;
}

/// The actual installed lifetime and the application's trusted policy adapter.
/// No wire codec or actor-string/boolean constructor exists. The authenticated
/// compiled application principal is fixed by the selected native host; the
/// provider must return that same owner for every permitted action.
#[derive(Clone)]
pub struct InstalledInstallationPolicy {
    lifetime: Arc<NativeHostLifetime>,
    provider: Arc<dyn ApplicationInstallationPolicy>,
    owner: PrincipalRef,
    realm: RealmId,
}
impl InstalledInstallationPolicy {
    pub fn new(lifetime: Arc<NativeHostLifetime>, provider: Arc<dyn ApplicationInstallationPolicy>)
        -> Result<Self, HostLifetimeError>
    {
        lifetime.validate_current_selection()?;
        let native = lifetime.selected_policy().native_host.as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        Ok(Self { owner: native.grant_administrator.clone(), realm: native.realm.clone(), lifetime, provider })
    }
    pub(crate) fn lifetime(&self) -> &Arc<NativeHostLifetime> { &self.lifetime }
    /// Observe the selected owner before acquiring the approval-state lock.
    /// The native dispatcher still owns its actual admission check; stopped-host
    /// replacement must close/join this owner before changing selected files.
    pub(crate) fn check_entry(&self) -> Result<(), HostLifetimeError> {
        self.lifetime.validate_current_selection()?;
        self.lifetime.validate_native_entry(None)
    }
    pub(crate) fn actor(&self) -> Result<ApprovalPrincipalId, HostLifetimeError> {
        // Ordinary native actor attribution is derived from the actual qualified
        // principal. It is not a native POLICY origin or human authentication.
        ApprovalPrincipalId::new(serde_json::to_string(&self.owner)
            .map_err(|_| HostLifetimeError::InvalidSelection)?)
            .map_err(|_| HostLifetimeError::InvalidSelection)
    }
    pub(crate) fn reserve(&self, realm: &RealmId, call: ToolCallView<'_>, context: &ToolDispatchContext,
        proposal: &Value) -> Result<Option<PolicyEntry>, HostLifetimeError>
    {
        self.lifetime.validate_current_selection()?;
        if realm != &self.realm { return Err(HostLifetimeError::WrongOwner); }
        let selected = self.lifetime.selected_policy();
        let request = InstallationPolicyRequest { application: &self.owner,
            deployment: &selected.deployment, source_policy_sha256: &selected.source_policy_sha256,
            realm, call, context, proposal };
        match self.provider.reserve(&request)? {
            InstallationPolicyDecision::RequiresHuman => Ok(None),
            InstallationPolicyDecision::Permit(reservation) => {
                if reservation.owner() != &self.owner { return Err(HostLifetimeError::WrongOwner); }
                reservation.check_current()?;
                // Audit data stays bounded; values must never contain credentials.
                if reservation.provenance().is_empty()
                    || serde_json::to_vec(reservation.provenance()).map_err(|_| HostLifetimeError::InvalidSelection)?.len() > 16 * 1024
                { return Err(HostLifetimeError::InvalidSelection); }
                let provenance = json!({"application": self.owner, "deployment": selected.deployment,
                    "source_policy_sha256": selected.source_policy_sha256,
                    "decision": reservation.provenance()});
                Ok(Some(PolicyEntry { policy: self.clone(), reservation, provenance }))
            }
        }
    }
}

/// Bound immediately for in-process hosts, or the same retained factory's
/// one-shot final adoption. Resolving a pending binding never grants authority.
#[derive(Clone)]
pub(crate) enum InstallationPolicySource {
    Installed(InstalledInstallationPolicy),
    #[cfg(all(unix, feature = "console"))]
    Selected(Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>),
}
impl InstallationPolicySource {
    pub(crate) fn resolve(&self) -> Result<Option<InstalledInstallationPolicy>, HostLifetimeError> {
        match self {
            Self::Installed(policy) => Ok(Some(policy.clone())),
            #[cfg(all(unix, feature = "console"))]
            Self::Selected(selected) => selected.installation(),
        }
    }
}

pub(crate) struct PolicyEntry {
    policy: InstalledInstallationPolicy,
    reservation: Box<dyn InstallationPolicyReservation>,
    provenance: Value,
}
impl PolicyEntry {
    pub(crate) fn provenance(&self) -> &Value { &self.provenance }
    pub(crate) fn actor(&self) -> Result<ApprovalPrincipalId, HostLifetimeError> { self.policy.actor() }
    pub(crate) fn check_current(&self) -> Result<(), HostLifetimeError> {
        self.reservation.check_current()
    }
    pub(crate) fn record<T>(self, record: impl FnOnce(Self) -> T) -> Result<T, HostLifetimeError> {
        let lifetime = self.policy.lifetime.clone();
        // Complete the phase observation before the callback may lock approval
        // state. Service grant entry takes phase before checking that same state.
        lifetime.with_current_consent(|| {
            self.check_current()?;
            Ok(record(self))
        })?
    }
}

/// Preserve unavailable policy as unavailable; it is never a human request or
/// evidence of a denial decision. Other owner refusals retain access denial.
pub(crate) fn tool_refusal(name: &str, error: HostLifetimeError) -> meerkat_core::ToolError {
    match error {
        HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => meerkat_core::ToolError::unavailable(
            name, meerkat_core::ToolUnavailableReason::TemporarilyUnavailable),
        _ => meerkat_core::ToolError::access_denied(name),
    }
}
