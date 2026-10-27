//! Configured acquisition is selected only by the existing first source bind.
use super::*;
use crate::installed_host::application_operations;
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::{
    constraints::{DelegationDepth, ExecutionRestrictions},
    grant::GrantLineageRef,
    work_association::{InputAuthorityAssociation, WorkAuthorityBasis},
};
use meerkat_toolkit::host_lifetime::{
    NativeHostLifetime,
    control::HostLifetimeError,
    policy::{CompiledAgentSetupCapability, CompiledNativeMember, CompiledNativeSourceRoute},
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfiguredSetupSelection {
    pub capability: CompiledAgentSetupCapability,
    pub session_id: meerkat_core::SessionId,
}

struct ConfiguredSetup {
    lifetime: Arc<NativeHostLifetime>,
    application: PrincipalRef,
    route: CompiledNativeSourceRoute,
    member: CompiledNativeMember,
    selection: Option<CompiledAgentSetupCapability>,
}
impl ConfiguredSetup {
    fn parent_bounds(&self) -> ExecutionRestrictions {
        application_operations::configured_setup_parent(
            &self.application,
            &self.route.logical_executor,
            &self.member.audience,
        )
    }
    fn child_bounds(&self, session: &meerkat_core::SessionId) -> ExecutionRestrictions {
        let mut bounds =
            application_operations::parent_restrictions(application_operations::find_and_install(
                &self.application,
                &self.route.logical_executor,
                &self.member.audience,
                session,
            ));
        bounds.delegation_depth = DelegationDepth::remaining(0);
        bounds
    }
}
impl PrivateOriginals {
    fn configured_setup(
        &self,
        reference: &PrivateOriginalRef,
    ) -> Result<ConfiguredSetup, PrivateOriginalError> {
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let selected = lifetime.selected_policy();
        let native = selected
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let route = native
            .routes
            .iter()
            .find(|route| route.id == reference.route)
            .ok_or(PrivateOriginalError::Denied)?;
        let (binding, requester, actor) = route
            .source_binding(selected)
            .map_err(|_| PrivateOriginalError::Denied)?;
        let member = native
            .members
            .iter()
            .find(|member| &member.member == binding)
            .ok_or(PrivateOriginalError::Denied)?;
        let source = self.route(&reference.route)?;
        if source.grant_target.as_str() != binding.member
            || source.grant_caller != native.grant_administrator
            || &source.requester != requester
            || &source.ingress_actor != actor
            || source.logical_executor != route.logical_executor
            || source.represented_subject != route.represented_subject
            || source.grant_audience != member.audience
        {
            return Err(PrivateOriginalError::Conflict);
        }
        // A capability for one explicit source route does not flow to another
        // provider event or principal merely because it names the same member.
        let selection = member
            .setup_capability
            .as_ref()
            .filter(|cap| cap.route == reference.route)
            .cloned();
        Ok(ConfiguredSetup {
            application: native.grant_administrator.clone(),
            route: route.clone(),
            member: member.clone(),
            selection,
            lifetime,
        })
    }

    fn configured_child_id(
        &self,
        reference: &PrivateOriginalRef,
    ) -> Result<EvidenceId, PrivateOriginalError> {
        EvidenceId::new(format!(
            "original-configured-setup-{}",
            self.key(reference)?.id
        ))
        .map_err(|_| PrivateOriginalError::Conflict)
    }

    fn configured_parent_current(
        &self,
        facts: &ConfiguredSetup,
        parent: &GrantLineageRef,
    ) -> Result<bool, PrivateOriginalError> {
        let resolved = match self.owner.grants().resolve_lineage(
            std::slice::from_ref(parent),
            &facts.application,
            facts.route.represented_subject.as_ref(),
        ) {
            Ok(resolved) => resolved,
            // Missing or revoked selection grants no setup capability. Store
            // unavailability is never interpreted as permission or absence.
            Err(GrantRefusal::Denied) => return Ok(false),
            Err(_) => return Err(PrivateOriginalError::Unavailable),
        };
        if resolved.restrictions() != &facts.parent_bounds() {
            return Err(PrivateOriginalError::Denied);
        }
        resolved
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(true)
    }

    fn check_configured_lineage(
        &self,
        facts: &ConfiguredSetup,
        reference: &PrivateOriginalRef,
        session: &meerkat_core::SessionId,
        lineage: &[GrantLineageRef],
    ) -> Result<(), PrivateOriginalError> {
        if lineage.len() != 2 || lineage[1].grant_id != self.configured_child_id(reference)? {
            return Err(PrivateOriginalError::Conflict);
        }
        if !self.configured_parent_current(facts, &lineage[0])? {
            return Err(PrivateOriginalError::Denied);
        }
        let resolved = self
            .owner
            .grants()
            .resolve_lineage(
                lineage,
                &facts.route.logical_executor,
                facts.route.represented_subject.as_ref(),
            )
            .map_err(|error| match error {
                GrantRefusal::Denied => PrivateOriginalError::Denied,
                _ => PrivateOriginalError::Unavailable,
            })?;
        if resolved.restrictions() != &facts.child_bounds(session) {
            return Err(PrivateOriginalError::Denied);
        }
        resolved
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)
    }

    pub(super) async fn configured_setup_basis(
        &self,
        record: &SourceRecord,
        reference: &PrivateOriginalRef,
        session: &meerkat_core::SessionId,
        console: Option<super::console_ingress::ConsoleEntry>,
    ) -> Result<Option<WorkAuthorityBasis>, PrivateOriginalError> {
        // Preserve the existing controller-only replay shortcut. Its already
        // retained owner and native admission still decide currentness. A new
        // configured capability cannot add a host or lineage requirement here.
        if record.final_association.as_ref().is_some_and(|saved| matches!(
            &saved.candidate().authority_basis,
            WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } if targets.is_empty()
        )) {
            return Ok(None);
        }
        if record.operation_cancelled
            || record.approved_task.is_some()
            || record.service_admission.is_some()
        {
            return Err(PrivateOriginalError::Denied);
        }
        let facts = self.configured_setup(reference)?;
        if let Some(saved) = &record.final_association {
            if saved.candidate().target.context.as_str() != session.to_string() {
                return Err(PrivateOriginalError::Conflict);
            }
            self.check_configured_setup_binding(reference, record, saved)
                .map_err(|_| PrivateOriginalError::Conflict)?;
            return Ok(Some(saved.candidate().authority_basis.clone()));
        }
        let selection = if let Some(retained) = &record.configured_setup {
            if &retained.session_id != session || retained.capability.route != reference.route {
                return Err(PrivateOriginalError::Conflict);
            }
            retained.capability.clone()
        } else {
            let Some(selection) = &facts.selection else { return Ok(None); };
            if !self.configured_parent_current(&facts, &selection.parent)? { return Ok(None); }
            self.retain_configured_setup(reference, session, selection.clone()).await?.capability
        };
        let origin = self.setup_origin_for(reference)?;
        let selected_targets = selection.helper_targets.clone();
        let owner = self.owner.clone();
        let parent = selection.parent.clone();
        let child_id = self.configured_child_id(reference)?;
        let bounds = facts.child_bounds(session);
        let grants = self.owner.grants();
        // The blocking writer keeps the exact verified login alive if its
        // waiting binder disappears. No Python callback or mutex spans await.
        let basis = tokio::task::spawn_blocking(move || {
            super::first_input::with_controller_login(console.as_ref(), None, || {
                facts.lifetime.with_current_setup(|| {
                    let issue = || {
                        let current = grants.resolve_lineage(
                            std::slice::from_ref(&parent),
                            &facts.application,
                            facts.route.represented_subject.as_ref(),
                        )?;
                        if current.restrictions() != &facts.parent_bounds() {
                            return Err(GrantRefusal::Denied);
                        }
                        let expected = current
                            .restrictions()
                            .for_child(&bounds)
                            .map_err(|_| GrantRefusal::Denied)?;
                        current
                            .check_current()
                            .map_err(|_| GrantRefusal::Unavailable)?;
                        let child = match grants.issued_reference(&child_id)? {
                            Some(child) => child,
                            None => grants.issue_child(
                                &facts.application,
                                &parent,
                                child_id.clone(),
                                facts.route.logical_executor.clone(),
                                bounds,
                            )?,
                        };
                        let lineage = vec![parent, child];
                        let current = grants.resolve_lineage(
                            &lineage,
                            &facts.route.logical_executor,
                            facts.route.represented_subject.as_ref(),
                        )?;
                        if current.restrictions() != &expected {
                            return Err(GrantRefusal::Denied);
                        }
                        current
                            .check_current()
                            .map_err(|_| GrantRefusal::Unavailable)?;
                        Ok(lineage)
                    };
                    let lineage = issue().map_err(|_| HostLifetimeError::Unavailable)?;
                    let targets = super::setup_targets::issue_targets(&owner, &facts.application,
                        &facts.route.logical_executor, facts.route.represented_subject.as_ref(),
                        &origin, &child_id, &selected_targets)?;
                    super::setup_targets::project(lineage, targets)
                })
            })
        })
        .await
        .map_err(|_| PrivateOriginalError::Unavailable)?
        .map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(Some(basis))
    }

    #[cfg(test)]
    pub(super) async fn configured_setup_lineage(&self, record: &SourceRecord,
        reference: &PrivateOriginalRef, session: &meerkat_core::SessionId,
        console: Option<super::console_ingress::ConsoleEntry>)
        -> Result<Option<Vec<GrantLineageRef>>, PrivateOriginalError>
    {
        self.configured_setup_basis(record, reference, session, console).await?.map(|basis|
            super::setup_targets::find_lineage(&basis).map(<[GrantLineageRef]>::to_vec)
                .ok_or(PrivateOriginalError::Conflict)).transpose()
    }

    pub(super) async fn retain_configured_setup(&self, reference: &PrivateOriginalRef,
        session: &meerkat_core::SessionId, capability: CompiledAgentSetupCapability)
        -> Result<ConfiguredSetupSelection, PrivateOriginalError>
    {
        crate::installed_host::operator_setup::setup_targets::validate_compiled(&capability.helper_targets)
            .map_err(|_| PrivateOriginalError::Denied)?;
        let selection = ConfiguredSetupSelection { capability, session_id: session.clone() };
        let route = self.route(&reference.route)?;
        let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: route.source_authority.clone(), work: reference.original.clone(),
        };
        self.owner.with_unbound_source_selection(&route.realm, &original, || async {
            let key = self.key(reference)?;
            let saved = self.store.get(&key).await.map_err(store_error)?.ok_or(PrivateOriginalError::Unavailable)?;
            let mut record = self.decode_record(&saved, reference)?;
            if record.operation_cancelled || record.final_association.is_some() || record.setup_request.is_some()
                || record.approved_task.is_some() || record.service_admission.is_some()
            { return Err(PrivateOriginalError::Denied); }
            if let Some(prior) = record.configured_setup {
                return if prior == selection { Ok(prior) } else { Err(PrivateOriginalError::Conflict) };
            }
            // A legacy or damaged row cannot use a previously entered child
            // as permission to adopt a wider present-day target selection.
            if !selection.capability.helper_targets.is_empty()
                && self.owner.grants().issued_reference(&self.configured_child_id(reference)?)
                    .map_err(|_| PrivateOriginalError::Unavailable)?.is_some()
            { return Err(PrivateOriginalError::Conflict); }
            record.configured_setup = Some(selection.clone());
            self.store.compare_and_swap(Transition { key, expected_revision: saved.revision,
                expected_digest: Some(saved.content_digest), claim: None, data: encode(record)?,
                events: vec![], effects: vec![], }).await.map_err(store_error)?;
            Ok(selection)
        }).await.map_err(|error| match error {
            OriginalWriterError::Conflict | OriginalWriterError::OperationCancelled => PrivateOriginalError::Denied,
            _ => PrivateOriginalError::Unavailable,
        })?
    }

    /// Runs after the canonical read under the existing original mutation
    /// owner, immediately before its original-row CAS. Explicit reviewed setup
    /// and approved service B keep their own independent validation paths.
    pub(super) fn check_configured_setup_binding(
        &self,
        reference: &PrivateOriginalRef,
        record: &SourceRecord,
        association: &InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        if record.setup_request.is_some()
            || record.approved_task.is_some()
            || record.service_admission.is_some()
        {
            return Ok(());
        }
        // This join applies only to installed first-bind composition. Existing
        // explicit trusted source writers without a host have no compiled
        // capability to select; their native authority checks are unchanged.
        if self.host_lifetime.get().is_none() {
            return Ok(());
        }
        let checked = || {
            let facts = self.configured_setup(reference)?;
            let claims = association.candidate();
            let session = meerkat_core::SessionId::parse(claims.target.context.as_str())
                .map_err(|_| PrivateOriginalError::Conflict)?;
            if let Some(saved) = &record.final_association {
                if saved != association { return Err(PrivateOriginalError::Conflict); }
            }
            if let Some(retained) = &record.configured_setup {
                if retained.session_id != session || retained.capability.route != reference.route {
                    return Err(PrivateOriginalError::Conflict);
                }
                super::setup_targets::check_structure(std::slice::from_ref(&retained.capability.parent),
                    &self.configured_child_id(reference)?, &retained.capability.helper_targets,
                    &claims.authority_basis).map_err(|_| PrivateOriginalError::Conflict)?;
                if record.final_association.is_none() {
                    let lineage = super::setup_targets::find_lineage(&claims.authority_basis)
                        .ok_or(PrivateOriginalError::Conflict)?;
                    self.check_configured_lineage(&facts, reference, &session, lineage)?;
                }
                return Ok(());
            }
            match &claims.authority_basis {
                WorkAuthorityBasis::GrantLineage { lineage } => {
                    if record.final_association.is_none()
                        && facts.selection.as_ref().map(|cap| &cap.parent) != lineage.first()
                    {
                        return Err(PrivateOriginalError::Conflict);
                    }
                    self.check_configured_lineage(&facts, reference, &session, lineage)
                }
                WorkAuthorityBasis::TargetedGrantLineages {
                    base: None,
                    targets,
                } if targets.is_empty() => {
                    if record.final_association.is_none() {
                        if let Some(cap) = &facts.selection {
                            if self.configured_parent_current(&facts, &cap.parent)? {
                                return Err(PrivateOriginalError::Conflict);
                            }
                        }
                    }
                    Ok(())
                }
                _ => Err(PrivateOriginalError::Conflict),
            }
        };
        checked().map_err(|error| match error {
            PrivateOriginalError::Unavailable => OriginalWriterError::Unavailable,
            _ => OriginalWriterError::Conflict,
        })
    }
}

#[cfg(test)]
mod selection_tests;

pub(crate) mod service;
