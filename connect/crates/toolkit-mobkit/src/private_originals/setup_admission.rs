//! Read actual installed source and selected-host facts for setup admission.
use super::*;
use crate::ToolAdmissionError;
use meerkat_toolkit::host_lifetime::{control::HostLifetimeError, policy::CompiledLocalPolicy};

pub(crate) struct InstalledSetupFacts {
    pub tenant: Id,
    pub deployment: Id,
    pub realm: RealmId,
    pub target: Id,
    pub target_member: PrincipalRef,
    pub helper: Option<meerkat_core::MobMemberBinding>,
    pub policy: CompiledLocalPolicy,
    pub now_ms: u64,
}
impl PrivateOriginals {
    pub(crate) fn installed_setup_facts(&self, reference: &PrivateOriginalRef)
        -> Result<InstalledSetupFacts, ToolAdmissionError>
    {
        let lifetime = self.host_lifetime.get().and_then(Weak::upgrade).ok_or(ToolAdmissionError::Unavailable)?;
        let result = || {
            let policy = lifetime.current_setup_policy()?;
            let facts: InstalledSetupFacts = {
                let native = policy.native_host.as_ref().ok_or(HostLifetimeError::Unavailable)?;
                if self.scope.realm != native.realm.as_str() || self.scope.owner != policy.deployment.as_str()
                    || self.owner.ingress_domain() != &native.ingress_domain
                { return Err(HostLifetimeError::WrongOwner); }
                let retained = self.retained.lock().map_err(|_| HostLifetimeError::Unavailable)?;
                let held = retained.iter().find(|held| held.reference == *reference).ok_or(HostLifetimeError::Denied)?;
                let route = self.routes().map_err(|_| HostLifetimeError::Unavailable)?.iter().find(|route| route.id == reference.route).ok_or(HostLifetimeError::Denied)?;
                let selected = native.routes.iter().find(|route| route.id == reference.route).ok_or(HostLifetimeError::Denied)?;
                let (binding, requester, actor) = selected.source_binding(&policy)?;
                let member = native.members.iter().find(|member| &member.member == binding).ok_or(HostLifetimeError::Denied)?;
                let helper = match &member.setup_capability {
                    Some(capability) if capability.route == reference.route => capability.helper.clone(),
                    Some(capability) if capability.helper.is_some() => return Err(HostLifetimeError::Denied),
                    _ => None,
                };
                let claims = held.original.association().candidate();
                if &claims.requester != requester || &claims.ingress_actor != actor
                    || claims.logical_executor != selected.logical_executor
                    || claims.target.logical_owner != member.logical_owner
                    || claims.target.context_generation != member.context_generation
                    || claims.target.audience != member.audience || route.grant_target.as_str() != binding.member
                { return Err(HostLifetimeError::Denied); }
                let now_ms = self.owner.clock().now().map_err(|_| HostLifetimeError::Unavailable)?.unix_ms;
                InstalledSetupFacts {
                    tenant: Id::new(self.scope.realm.clone()).map_err(|_| HostLifetimeError::InvalidSelection)?,
                    deployment: policy.deployment.clone(), realm: native.realm.clone(), target: route.grant_target.clone(),
                    target_member: member.logical_owner.clone(), helper, policy: policy.clone(), now_ms,
                }
            };
            // Source/retained guards end before taking another selected-file
            // and lifetime observation. A failed recheck releases no facts.
            if lifetime.current_setup_policy()? != policy {
                return Err(HostLifetimeError::Denied);
            }
            Ok(facts)
        };
        result().map_err(|error| match error {
            HostLifetimeError::Unavailable => ToolAdmissionError::Unavailable,
            _ => ToolAdmissionError::Denied,
        })
    }
}
