//! The application's explicit policy, prepared before native assembly.
//! This retains one immutable registry through each native prepare/finish.
use std::sync::{Arc, OnceLock};

pub(crate) mod service_policy;
pub use service_policy::{ServiceToolPolicySelection, ServiceToolInvocation, MemberPolicyLifetime,
    ServiceToolPolicyVersion, ServiceToolPolicyProposal, ServiceToolPolicyApproval,
    ActiveMemberToolPolicyDecision, ApplicationToolPolicyRecoveryRequest, ApplicationToolPolicyRecoveryDecision,
    ServiceToolPolicyNativeOutcome, ServiceToolPolicyObservation};

use meerkat_core::{
    ApplicationToolPolicyBinding, CompiledApplicationToolPolicy, MobMemberBinding,
    PreparedToolCatalog, PreparedToolSurface, ToolConsequenceNarrowingPolicy,
    ToolConsequencePolicyRegistry, ToolIdentity, ToolSurfaceSelection,
};
use meerkat_mobkit::member_tool_policy::{CompiledPolicyProvider, providers_from_canonical_payloads};
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;

/// Pure canonical validation. Preparation stays separate because candidate
/// validation must not start native policy evaluation workers.
pub(super) struct ValidatedInstalledToolPolicy {
    compiled: CompiledApplicationToolPolicy,
    providers: Vec<Arc<CompiledPolicyProvider>>,
}
impl ValidatedInstalledToolPolicy {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, HostLifetimeError> {
        let compiled = CompiledApplicationToolPolicy::parse_canonical_json(bytes)
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        let providers = providers_from_canonical_payloads([bytes.to_vec()])
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        Ok(Self { compiled, providers })
    }

    pub(super) fn contains_member(&self, member: &MobMemberBinding) -> bool {
        self.compiled.members.iter().any(|row| row.member_identity == member.member)
    }

    /// Both values came through canonical native parsing. Compare the whole
    /// typed artifact, including grants and source provenance, not only policy
    /// identifiers. Returning the supplied value preserves its exact registry.
    pub(super) fn adopt(
        self,
        prepared: PreparedInstalledToolPolicy,
    ) -> Result<PreparedInstalledToolPolicy, HostLifetimeError> {
        if &self.compiled != prepared.compiled.as_ref() {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(prepared)
    }

    pub(super) fn prepare(self) -> Result<PreparedInstalledToolPolicy, HostLifetimeError> {
        let registry = Arc::new(ToolConsequencePolicyRegistry::new(
            self.providers.iter().cloned().map(|provider| provider as Arc<dyn ToolConsequenceNarrowingPolicy>).collect(),
            Default::default(), None,
        ).map_err(|_| HostLifetimeError::Unavailable)?);
        Ok(PreparedInstalledToolPolicy { compiled: Arc::new(self.compiled), registry, providers: self.providers,
            service_owner: Arc::new(OnceLock::new()), service_owner_init: Arc::new(tokio::sync::Mutex::new(())) })
    }
}

/// Clones share the same registry and actual providers. A later surface never
/// selects a policy; only an explicit trusted same-slot successor can advance it.
#[derive(Clone)]
pub struct PreparedInstalledToolPolicy {
    compiled: Arc<CompiledApplicationToolPolicy>,
    registry: Arc<ToolConsequencePolicyRegistry>,
    providers: Vec<Arc<CompiledPolicyProvider>>,
    service_owner: Arc<OnceLock<Arc<service_policy::ServicePolicyOwner>>>,
    service_owner_init: Arc<tokio::sync::Mutex<()>>,
}
impl PreparedInstalledToolPolicy {
    /// Construct the actual native registry before assembly. This starts its
    /// existing evaluation workers, not an agent runtime.
    pub fn from_canonical(bytes: &[u8]) -> Result<Self, HostLifetimeError> {
        ValidatedInstalledToolPolicy::parse(bytes)?.prepare()
    }
    pub fn registry(&self) -> Arc<ToolConsequencePolicyRegistry> { self.registry.clone() }
    /// Selected compilation inputs retained by this handle. Native evaluation
    /// reads the shared provider snapshot, not this historical input accessor.
    pub fn compiled(&self) -> &CompiledApplicationToolPolicy { &self.compiled }

    /// Bind the one actual application service-decision owner before any
    /// native configuration is exposed. Clones retain the same provider/slot.
    pub(crate) async fn bind_service_owner(&self, store: Arc<dyn toolkit_store::ToolkitStore>,
        scope: toolkit_store::StoreScope, application: meerkat_core::PrincipalRef,
        participant: Option<Arc<dyn crate::private_originals::ApplicationServicePolicy>>,
    ) -> Result<Arc<service_policy::ServicePolicyOwner>, HostLifetimeError> {
        let _initialization = self.service_owner_init.lock().await;
        if let Some(owner) = self.service_owner.get() {
            if !owner.same_binding(&store, &scope, &application, &participant) {
                return Err(HostLifetimeError::InvalidSelection);
            }
            owner.recover().await?;
            return Ok(owner.clone());
        }
        if self.providers.len() != 1 { return Err(HostLifetimeError::InvalidSelection); }
        let bootstrap = self.compiled.canonical_json().map_err(|_| HostLifetimeError::InvalidSelection)?;
        let owner = service_policy::ServicePolicyOwner::open(store, scope, application,
            self.providers[0].clone(), String::from_utf8(bootstrap).map_err(|_| HostLifetimeError::InvalidSelection)?,
            participant).await?;
        self.service_owner.set(owner.clone()).map_err(|_| HostLifetimeError::InvalidSelection)?;
        Ok(owner)
    }

    /// A bound replacement may select only the exact current policy already
    /// approved and observed by this application's canonical decision owner.
    /// Unbound construction retains the native prior/candidate validation.
    /// Neither replacement artifacts nor the bootstrap accessor approve grants.
    pub(super) fn successor(mut self, prior: &[u8], candidate: &[u8]) -> Result<Self, HostLifetimeError> {
        let next = CompiledApplicationToolPolicy::parse_canonical_json(candidate)
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        if self.providers.len() != 1 || next.provider_id != self.compiled.provider_id
            || next.policy_id != self.compiled.policy_id
        { return Err(HostLifetimeError::InvalidSelection); }
        if let Some(owner) = self.service_owner.get() {
            // A replacement artifact is not an approval. The same canonical
            // owner must already have restored/accepted this exact candidate.
            // Keep compiled as the authenticated bootstrap input for adoption.
            if !owner.is_observed_current(candidate)? { return Err(HostLifetimeError::Denied); }
            return Ok(self);
        }
        // Unbound pure construction controls retain the original behavior.
        // Installed hosts bind the canonical owner before returning.
        // Validate the whole sequence before mutating the retained provider.
        let original = self.compiled.canonical_json().map_err(|_| HostLifetimeError::InvalidSelection)?;
        providers_from_canonical_payloads([original.as_slice(), prior, candidate])
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        self.providers[0].accept_canonical_json(prior).map_err(|_| HostLifetimeError::InvalidSelection)?;
        self.providers[0].accept_canonical_json(candidate).map_err(|_| HostLifetimeError::InvalidSelection)?;
        self.compiled = Arc::new(next);
        Ok(self)
    }

    /// The trusted host separately authenticates the member and its installed
    /// associations. Native finish consumes this exact surface and uses this
    /// same registry. This selection is neither publication nor invocation.
    pub fn select(
        &self, member: &MobMemberBinding, surface: &PreparedToolSurface,
    ) -> Result<ToolSurfaceSelection, HostLifetimeError> {
        if !self.compiled.members.iter().any(|row| row.member_identity == member.member) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        self.registry.bind(
            member.clone(), self.compiled.provider_id.clone(), self.compiled.policy_id.clone(),
        ).map_err(|_| HostLifetimeError::Unavailable)?;
        Ok(ToolSurfaceSelection {
            binding: ApplicationToolPolicyBinding::Provider {
                provider_id: self.compiled.provider_id.clone(),
                policy_id: self.compiled.policy_id.clone(),
            },
            compiled_against: surface.clone(),
        })
    }
}

/// Full actual identities require installed associations, even if hidden by
/// static filters. An honestly Complete catalog contributes its exact entries;
/// VisibleOnly never becomes a guessed deferred inventory.
pub(super) fn validate_installed_surface(
    surface: &PreparedToolSurface, installed: &[ToolIdentity],
) -> Result<(), HostLifetimeError> {
    if surface.tools.iter().any(|identity| !installed.contains(identity))
        || surface.projected.iter().any(|identity| !surface.tools.contains(identity))
        || matches!(&surface.catalog, PreparedToolCatalog::Complete(entries)
            if entries.iter().any(|entry| !installed.contains(&entry.identity)))
    { return Err(HostLifetimeError::InvalidSelection); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{CompiledMemberToolGrants, CompiledPolicySourceProvenance,
        PolicyDigest, PolicyId, PolicyProviderId, PolicyRevision, ToolDef};
    fn member(name: &str) -> MobMemberBinding {
        MobMemberBinding { mob_id: "application".into(), member: name.into(), role: "worker".into() }
    }
    fn policy() -> Vec<u8> {
        CompiledApplicationToolPolicy::new(
            PolicyProviderId::new("application").unwrap(),
            PolicyId::new("explicit").unwrap(), PolicyRevision(1),
            CompiledPolicySourceProvenance { source_id: "selected".into(),
                source_digest: PolicyDigest::from_canonical_bytes(b"selected application") },
            vec![CompiledMemberToolGrants { member_identity: "worker".into(), grants: vec![] }],
        ).unwrap().canonical_json().unwrap()
    }
    fn identity(name: &str) -> ToolIdentity {
        ToolDef::new(name, "fixture", serde_json::json!({"type":"object"})).identity()
    }
    #[test]
    fn one_registry_and_exact_selection_are_retained() {
        let prepared = PreparedInstalledToolPolicy::from_canonical(&policy()).unwrap();
        let clone = prepared.clone();
        assert!(Arc::ptr_eq(&prepared.registry(), &clone.registry()));
        let surface = PreparedToolSurface { tools: vec![identity("research")],
            projected: vec![], catalog: PreparedToolCatalog::VisibleOnly };
        let selection = clone.select(&member("worker"), &surface).unwrap();
        assert_eq!(selection.compiled_against, surface);
        assert!(matches!(selection.binding, ApplicationToolPolicyBinding::Provider { .. }));
        assert!(prepared.select(&member("foreign"), &surface).is_err());
        assert!(prepared.compiled().members[0].grants.is_empty());
    }
    #[test]
    fn adoption_retains_the_original_policy_registry() {
        let bytes = policy();
        let prepared = PreparedInstalledToolPolicy::from_canonical(&bytes).unwrap();
        let registry = prepared.registry();
        let adopted = ValidatedInstalledToolPolicy::parse(&bytes)
            .unwrap()
            .adopt(prepared)
            .unwrap();
        assert!(Arc::ptr_eq(&registry, &adopted.registry()));
        assert_eq!(adopted.compiled().canonical_json().unwrap(), bytes);
        let surface = PreparedToolSurface {
            tools: vec![identity("research")],
            projected: vec![],
            catalog: PreparedToolCatalog::VisibleOnly,
        };
        assert_eq!(
            adopted.select(&member("worker"), &surface).unwrap().compiled_against,
            surface,
        );
    }
    #[test]
    fn adoption_refuses_changed_provenance_under_the_same_policy_ids() {
        let bytes = policy();
        let prepared = PreparedInstalledToolPolicy::from_canonical(&bytes).unwrap();
        let compiled = prepared.compiled();
        let changed = CompiledApplicationToolPolicy::new(
            compiled.provider_id.clone(),
            compiled.policy_id.clone(),
            compiled.revision,
            CompiledPolicySourceProvenance {
                source_id: compiled.source.source_id.clone(),
                source_digest: PolicyDigest::from_canonical_bytes(b"different application source"),
            },
            compiled.members.clone(),
        ).unwrap().canonical_json().unwrap();
        let selected = ValidatedInstalledToolPolicy::parse(&changed).unwrap();
        assert_eq!(selected.compiled.provider_id, prepared.compiled().provider_id);
        assert_eq!(selected.compiled.policy_id, prepared.compiled().policy_id);
        assert_eq!(selected.compiled.revision, prepared.compiled().revision);
        assert!(matches!(selected.adopt(prepared), Err(HostLifetimeError::InvalidSelection)));
    }
    #[test]
    fn actual_upper_bound_is_checked_without_projected_equality() {
        let visible = identity("visible");
        let hidden = identity("hidden");
        let surface = PreparedToolSurface { tools: vec![visible.clone(), hidden.clone()],
            projected: vec![visible.clone()], catalog: PreparedToolCatalog::VisibleOnly };
        assert!(validate_installed_surface(&surface, &[visible.clone(), hidden]).is_ok());
        assert!(validate_installed_surface(&surface, &[visible]).is_err());
        let mut foreign = surface.clone();
        foreign.projected.push(identity("not-in-this-construction"));
        assert!(validate_installed_surface(&foreign, &surface.tools).is_err());
    }
    #[test]
    fn canonical_validation_does_not_repair_bytes_or_missing_members() {
        let bytes = policy();
        let validated = ValidatedInstalledToolPolicy::parse(&bytes).unwrap();
        assert!(validated.contains_member(&member("worker")));
        assert!(!validated.contains_member(&member("foreign")));
        let mut noncanonical = bytes;
        noncanonical.push(b' ');
        assert!(ValidatedInstalledToolPolicy::parse(&noncanonical).is_err());
    }
    #[test]
    fn complete_catalog_entries_keep_their_own_full_identity() {
        let lazy = identity("lazy");
        let surface = PreparedToolSurface {
            tools: vec![], projected: vec![],
            catalog: PreparedToolCatalog::Complete(vec![meerkat_core::PreparedCatalogEntry {
                identity: lazy.clone(), plane: meerkat_core::ToolPlaneClass::Session,
                callability: meerkat_core::ToolCallability::Callable, deferred_eligible: true,
            }]),
        };
        assert!(validate_installed_surface(&surface, std::slice::from_ref(&lazy)).is_ok());
        assert!(validate_installed_surface(&surface, &[]).is_err());
        let unknown = PreparedToolSurface { tools: vec![], projected: vec![],
            catalog: PreparedToolCatalog::VisibleOnly };
        assert!(validate_installed_surface(&unknown, &[lazy]).is_ok());
        assert!(!unknown.contains(&identity("lazy")));
    }
    #[test]
    fn successor_keeps_actual_provider_and_registry_and_fences_prior_desired_revision() {
        fn at(revision: u64, source: &str) -> Vec<u8> {
            let old = CompiledApplicationToolPolicy::parse_canonical_json(&policy()).unwrap();
            CompiledApplicationToolPolicy::new(old.provider_id, old.policy_id, PolicyRevision(revision),
                CompiledPolicySourceProvenance { source_id: source.into(), source_digest: PolicyDigest::from_canonical_bytes(source.as_bytes()) },
                old.members).unwrap().canonical_json().unwrap()
        }
        let prepared = PreparedInstalledToolPolicy::from_canonical(&policy()).unwrap();
        let original = prepared.clone();
        let accepted = prepared.successor(&at(5, "prior"), &at(6, "candidate")).unwrap();
        assert!(Arc::ptr_eq(&original.registry, &accepted.registry));
        assert!(Arc::ptr_eq(&original.providers[0], &accepted.providers[0]));
        assert_eq!(original.providers[0].accepted_revision(&accepted.compiled.policy_id), Some(6));
        assert!(accepted.clone().successor(&at(7, "actual-prior"), &at(6, "candidate")).is_err());
        assert_eq!(original.providers[0].accepted_revision(&accepted.compiled.policy_id), Some(6));
        assert!(accepted.clone().successor(&at(6, "other-source"), &at(7, "next")).is_err());
        assert_eq!(original.providers[0].accepted_revision(&accepted.compiled.policy_id), Some(6));
        let actual = PreparedToolSurface { tools: vec![identity("changed")], projected: vec![], catalog: PreparedToolCatalog::VisibleOnly };
        assert_eq!(accepted.select(&member("worker"), &actual).unwrap().compiled_against, actual);
    }

}
