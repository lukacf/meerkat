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
    pub(crate) fn check_setup_owner(
        &self,
        lifetime: &Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>,
        owner: &Arc<ToolkitOriginalOwner>,
    ) -> Result<(), HostLifetimeError> {
        let current = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        if !Arc::ptr_eq(&current, lifetime) || !Arc::ptr_eq(&self.owner, owner) {
            return Err(HostLifetimeError::WrongOwner);
        }
        lifetime.validate_current_selection()
    }

    pub(crate) async fn installed_setup_facts(
        &self,
        reference: &PrivateOriginalRef,
    ) -> Result<InstalledSetupFacts, ToolAdmissionError> {
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(ToolAdmissionError::Unavailable)?;
        let saved = self
            .store
            .get(
                &self
                    .key(reference)
                    .map_err(|_| ToolAdmissionError::Denied)?,
            )
            .await
            .map_err(|_| ToolAdmissionError::Unavailable)?
            .ok_or(ToolAdmissionError::Unavailable)?;
        let record = self
            .decode_record(&saved, reference)
            .map_err(|_| ToolAdmissionError::Denied)?;
        let selected_caller = self
            .selected_source_route(&reference.route)
            .map_err(|error| match error {
                PrivateOriginalError::Unavailable => ToolAdmissionError::Unavailable,
                _ => ToolAdmissionError::Denied,
            })?;
        let result = || {
            let policy = lifetime.current_setup_policy()?;
            let facts: InstalledSetupFacts = {
                let native = policy
                    .native_host
                    .as_ref()
                    .ok_or(HostLifetimeError::Unavailable)?;
                if self.scope.realm != native.realm.as_str()
                    || self.scope.owner != policy.deployment.as_str()
                    || self.owner.ingress_domain() != &native.ingress_domain
                {
                    return Err(HostLifetimeError::WrongOwner);
                }
                let retained = self
                    .retained
                    .lock()
                    .map_err(|_| HostLifetimeError::Unavailable)?;
                let held = retained
                    .iter()
                    .find(|held| held.reference == *reference)
                    .ok_or(HostLifetimeError::Denied)?;
                let (selected, member, context) = &selected_caller;
                let (binding, requester, actor) = selected.source_binding(&policy)?;
                if record.operation_cancelled
                    || record.final_association.as_ref() != Some(held.original.association())
                {
                    return Err(HostLifetimeError::Denied);
                }
                let capability = record
                    .configured_setup
                    .as_ref()
                    .map(|saved| &saved.capability)
                    .or(member.setup_capability.as_ref());
                let helper = match capability {
                    Some(capability) if capability.route == reference.route => {
                        capability.helper.clone()
                    }
                    Some(capability) if capability.helper.is_some() => {
                        return Err(HostLifetimeError::Denied);
                    }
                    _ => None,
                };
                let claims = held.original.association().candidate();
                if &claims.requester != requester
                    || &claims.ingress_actor != actor
                    || &member.member != binding
                    || context
                        .as_ref()
                        .is_some_and(|expected| &claims.target.context != expected)
                    || claims.represented_subject != selected.represented_subject
                    || claims.original_work.authority != selected.source_authority
                    || claims.logical_executor != selected.logical_executor
                    || claims.target.logical_owner != member.logical_owner
                    || claims.target.context_generation != member.context_generation
                    || claims.target.audience != member.audience
                {
                    return Err(HostLifetimeError::Denied);
                }
                let now_ms = self
                    .owner
                    .clock()
                    .now()
                    .map_err(|_| HostLifetimeError::Unavailable)?
                    .unix_ms;
                InstalledSetupFacts {
                    tenant: Id::new(self.scope.realm.clone())
                        .map_err(|_| HostLifetimeError::InvalidSelection)?,
                    deployment: policy.deployment.clone(),
                    realm: native.realm.clone(),
                    target: Id::new(binding.member.clone())
                        .map_err(|_| HostLifetimeError::InvalidSelection)?,
                    target_member: member.logical_owner.clone(),
                    helper,
                    policy: policy.clone(),
                    now_ms,
                }
            };
            // Source/retained guards end before taking another selected-file
            // and lifetime observation. A failed recheck releases no facts.
            if lifetime.current_setup_policy()? != policy {
                return Err(HostLifetimeError::Denied);
            }
            if self
                .selected_source_route(&reference.route)
                .map_err(|error| match error {
                    PrivateOriginalError::Unavailable => HostLifetimeError::Unavailable,
                    _ => HostLifetimeError::Denied,
                })?
                != selected_caller
            {
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
