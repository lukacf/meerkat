//! New held Add selects parents on the existing operator/native grant owner.
//! A parent is not an input permission. Delivery children and their exact native
//! run/source lifetime fence belong to the receiver binder; no child is issued here.
use super::*;
use crate::installed_host::{
    application_operations,
    dynamic_members::{MemberStandingOperations, MemberStandingParents},
};
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::{
    constraints::{ActionRef, DomainRestriction, ExactRestriction},
    evidence::EvidenceId,
    work_association::GrantLineageRef,
};

fn refusal(error: GrantRefusal) -> HostLifetimeError {
    match error {
        GrantRefusal::Unavailable => unavailable(),
        _ => HostLifetimeError::Denied,
    }
}

impl ServicePolicyOwner {
    /// Caller holds the member exclusion. The held Add supplies this identity;
    /// neither a saved row nor a peer message can select a new mandate.
    pub(super) fn standing_intent(
        &self,
        admission: &MemberClassAdmission,
        identity: &meerkat_mobkit::identity_first::AgentIdentity,
    ) -> Result<MemberStandingOperations, HostLifetimeError> {
        if !admission.class.purpose.is_ordinary() || admission.workflow.is_some() {
            return Err(HostLifetimeError::Denied);
        }
        let source = self
            .callback_source
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(unavailable)?;
        let context = source.callback_grant_context()?;
        context.lifetime.with_current_setup(|| {
            let route = admission.route.as_ref().ok_or(HostLifetimeError::Denied)?;
            source.check_operator_member_route(&admission.instance.member, route, identity)?;
            let native = context
                .lifetime
                .selected_policy()
                .native_host
                .as_ref()
                .ok_or_else(unavailable)?;
            let bundle = self.callback_bundle(admission)?;
            Ok(MemberStandingOperations {
                peer_message_parent: EvidenceId::new(format!("{bundle}/peer-message"))
                    .map_err(|_| invalid())?,
                recorder_write_parent: EvidenceId::new(format!("{bundle}/recorder-write"))
                    .map_err(|_| invalid())?,
                recorder_scope: meerkat_mobkit::memory::MemoryScope::Identity {
                    realm: native.realm.to_string(),
                    identity: identity.to_string(),
                }
                .operation_scope_ref(),
                issued: None,
            })
        })
    }

    /// Caller retains the same member exclusion/current reservation throughout
    /// issuance and canonical commitment. Existing IDs are always reconciled,
    /// never replaced after revocation, expiry or a lost observer.
    pub(crate) fn issue_standing_parents(
        &self,
        admission: &MemberClassAdmission,
    ) -> Result<MemberStandingParents, HostLifetimeError> {
        self.validate_admission(admission)?;
        let selected = admission
            .standing_operations
            .as_ref()
            .ok_or(HostLifetimeError::Denied)?;
        let route = admission.route.as_ref().ok_or(HostLifetimeError::Denied)?;
        let source = self
            .callback_source
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(unavailable)?;
        let context = source.callback_grant_context()?;
        context.lifetime.with_current_setup(|| {
            // Same established one-level owned-domain parent shape as catalog
            // selection. The eventual child narrows to the current sender's
            // exact peer domain and same declared operator audience.
            let mut peer = application_operations::configured_catalog_parent(
                &context.application,
                &route.logical_executor,
                &admission.instance.member.audience,
            );
            peer.actions = ExactRestriction::exact([ActionRef {
                feature: "toolkit".into(),
                action: "peer-message".into(),
            }]);
            peer.resource_domains = DomainRestriction::owned_by([context.application.clone()]);
            let recorder = application_operations::parent_restrictions(
                application_operations::recorder_operation(
                    &context.application,
                    &route.logical_executor,
                    &admission.instance.member.audience,
                    &selected.recorder_scope,
                    meerkat_core::MemoryOperation::Write,
                )
                .ok_or_else(invalid)?,
            );
            let issue =
                |id: &EvidenceId,
                 bounds: &meerkat_authorization_contracts::constraints::ExecutionRestrictions|
                 -> Result<GrantLineageRef, HostLifetimeError> {
                    let root = match context.grants.issued_reference(id).map_err(refusal)? {
                        Some(root) => root,
                        None => context
                            .grants
                            .issue_root(
                                &meerkat_core::AdmissionGuard::none(),
                                &context.operator,
                                id.clone(),
                                context.application.clone(),
                                route.represented_subject.clone(),
                                bounds.clone(),
                            )
                            .map_err(refusal)?,
                    };
                    if root.root_authority != context.operator {
                        return Err(HostLifetimeError::Denied);
                    }
                    let resolved = context
                        .grants
                        .resolve_lineage(
                            std::slice::from_ref(&root),
                            &context.application,
                            route.represented_subject.as_ref(),
                        )
                        .map_err(refusal)?;
                    if resolved.restrictions() != bounds {
                        return Err(HostLifetimeError::Denied);
                    }
                    resolved.check_current().map_err(|_| unavailable())?;
                    Ok(root)
                };
            Ok(MemberStandingParents {
                peer_message: issue(&selected.peer_message_parent, &peer)?,
                recorder_write: issue(&selected.recorder_write_parent, &recorder)?,
            })
        })
    }
}
