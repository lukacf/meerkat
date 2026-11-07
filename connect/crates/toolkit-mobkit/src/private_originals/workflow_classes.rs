//! Inert workflow source preparation and exact canonical class association.
//! Native member construction and authority remain with their existing owners.
use super::*;
use crate::installed_host::dynamic_members::{AdmittedMemberClass, MemberClassWorkflowBinding};
use meerkat_toolkit::{
    host_lifetime::policy::{
        CompiledMemberClass, CompiledMemberClassInstance, CompiledMemberClassPurpose,
        CompiledNativeSourceRoute,
    },
    setup::SetupWorkflowTerms,
};

/// Historical facts retained by this source owner before a G route is active.
/// There is no public constructor and this value grants no input or tool use.
pub struct PreparedWorkflowClassSource {
    source: Weak<PrivateOriginals>,
    class: CompiledMemberClass,
    instance: CompiledMemberClassInstance,
    parent: PrivateOriginalRef,
    original: PrivateOriginalRef,
    route: CompiledNativeSourceRoute,
    key: StoreKey,
}
impl PreparedWorkflowClassSource {
    pub fn source_key(&self) -> &StoreKey {
        &self.key
    }
    pub fn original(&self) -> &PrivateOriginalRef {
        &self.original
    }
    pub fn route(&self) -> &CompiledNativeSourceRoute {
        &self.route
    }
}
fn host_error(
    error: PrivateOriginalError,
) -> meerkat_toolkit::host_lifetime::control::HostLifetimeError {
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    match error {
        PrivateOriginalError::Unavailable => HostLifetimeError::Unavailable,
        _ => HostLifetimeError::Denied,
    }
}
impl PrivateOriginals {
    fn current_workflow_parent(
        &self,
        parent: &PrivateOriginalRef,
    ) -> Result<Arc<HeldPrivateOriginal>, PrivateOriginalError> {
        let held = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| &held.reference == parent)
            .cloned()
            .ok_or(PrivateOriginalError::Unavailable)?;
        self.owner
            .workflow_controller(&held.original)
            .map_err(setup_workflow::custody_error)?;
        Ok(held)
    }

    pub(crate) async fn retain_workflow_class_source(
        self: &Arc<Self>,
        instance: CompiledMemberClassInstance,
        parent: &PrivateOriginalRef,
        route: CompiledNativeSourceRoute,
        event: PrivateAuthenticatedEvent,
    ) -> Result<PreparedWorkflowClassSource, PrivateOriginalError> {
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let native = lifetime
            .selected_policy()
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let class = native
            .member_classes
            .iter()
            .find(|class| class.id == instance.class)
            .ok_or(PrivateOriginalError::Denied)?
            .clone();
        if class.purpose != CompiledMemberClassPurpose::SharedWorkflow
            || instance.member.setup_capability.is_some()
            || route.application_parent.is_some()
            || route.controller_parent.is_some()
        {
            return Err(PrivateOriginalError::Denied);
        }
        class
            .materialize(&native.realm, &instance)
            .map_err(|_| PrivateOriginalError::Denied)?;
        let projected = self.project_class_source_route(&instance.member, &route)?;
        let held = self.current_workflow_parent(parent)?;
        let p = held.original.association().candidate();
        if projected.requester != p.requester
            || projected.realm != p.ingress_namespace.realm
            || projected.source_authority != p.original_work.authority
            || projected.logical_executor != instance.member.logical_owner
            || event.contributors.as_slice() != [p.original_work.work.clone()]
            || event.original == parent.original
            || event.route != route.id
        {
            return Err(PrivateOriginalError::Denied);
        }
        let original = self.retain_event_for_route(event, &projected).await?;
        let key = self.key_for_route(&original, &projected)?;
        let prepared = PreparedWorkflowClassSource {
            source: Arc::downgrade(self),
            class,
            instance,
            parent: parent.clone(),
            original,
            route,
            key,
        };
        self.check_prepared_workflow_class(&prepared).await?;
        Ok(prepared)
    }

    async fn check_prepared_workflow_class(
        self: &Arc<Self>,
        selected: &PreparedWorkflowClassSource,
    ) -> Result<(), PrivateOriginalError> {
        if !selected
            .source
            .upgrade()
            .is_some_and(|owner| Arc::ptr_eq(&owner, self))
        {
            return Err(PrivateOriginalError::Denied);
        }
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if lifetime
            .selected_policy()
            .native_host
            .as_ref()
            .is_none_or(|native| !native.member_classes.contains(&selected.class))
        {
            return Err(PrivateOriginalError::Denied);
        }
        let route = self.project_class_source_route(&selected.instance.member, &selected.route)?;
        let stored = self
            .store
            .get(&selected.key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record_for_route(&stored, &selected.original, &route)?;
        if record.operation_cancelled || record.final_association.is_some() {
            return Err(PrivateOriginalError::Denied);
        }
        self.current_workflow_parent(&selected.parent)?;
        Ok(())
    }

    pub(crate) async fn reserve_workflow_class(
        self: &Arc<Self>,
        id: Id,
        selected: &PreparedWorkflowClassSource,
        terms: &SetupWorkflowTerms,
    ) -> Result<AdmittedMemberClass, meerkat_toolkit::host_lifetime::control::HostLifetimeError>
    {
        self.check_prepared_workflow_class(selected)
            .await
            .map_err(host_error)?;
        let route = self
            .project_class_source_route(&selected.instance.member, &selected.route)
            .map_err(host_error)?;
        let now = self
            .owner
            .clock()
            .now()
            .map_err(|_| host_error(PrivateOriginalError::Unavailable))?
            .unix_ms;
        if terms.planning_source != selected.key
            || terms.lead != selected.instance.member.member
            || terms.realm != route.realm
            || terms.requester != route.requester
            || terms.deadline_ms <= now
        {
            return Err(host_error(PrivateOriginalError::Denied));
        }
        let workflow = self
            .workflow_store()
            .map_err(host_error)?
            .workflow_ref(terms)
            .map_err(|_| host_error(PrivateOriginalError::Conflict))?;
        self.service_tool_policy
            .get()
            .ok_or_else(|| host_error(PrivateOriginalError::Unavailable))?
            .admit_workflow_member_declaration(
                id,
                selected.instance.clone(),
                MemberClassWorkflowBinding {
                    workflow,
                    parent: selected.parent.clone(),
                    original: selected.original.clone(),
                },
            )
            .await
    }

    pub(crate) async fn check_workflow_class_source(
        &self,
        admitted: &AdmittedMemberClass,
        route: &CompiledNativeSourceRoute,
    ) -> Result<(), meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
        self.check_member_class_owner(admitted)?;
        let binding = admitted
            .workflow()
            .ok_or_else(|| host_error(PrivateOriginalError::Denied))?;
        if admitted.class().purpose != CompiledMemberClassPurpose::SharedWorkflow
            || route.id != binding.original.route
        {
            return Err(host_error(PrivateOriginalError::Denied));
        }
        let projected = self
            .project_class_source_route(&admitted.instance().member, route)
            .map_err(host_error)?;
        let key = self
            .key_for_route(&binding.original, &projected)
            .map_err(host_error)?;
        let stored = self
            .store
            .get(&key)
            .await
            .map_err(store_error)
            .map_err(host_error)?
            .ok_or_else(|| host_error(PrivateOriginalError::Unavailable))?;
        let record = self
            .decode_record_for_route(&stored, &binding.original, &projected)
            .map_err(host_error)?;
        if record.operation_cancelled || record.final_association.is_some() {
            return Err(host_error(PrivateOriginalError::Denied));
        }
        self.current_workflow_parent(&binding.parent)
            .map_err(host_error)?;
        // A source read may suspend after the initial admission observation.
        // Retirement must still refuse this final publication check.
        self.check_member_class_owner(admitted)?;
        Ok(())
    }
}

impl PrivateOriginals {
    /// Select only an existing Active workflow into this process's restrictive
    /// source owner before native construction. Retained rows supply identity
    /// and provenance; actual registration and admitted-source recovery must
    /// still supply capture and controller custody. This issues no grant or Add.
    pub async fn restore_workflow_class_source(
        self: &Arc<Self>,
        admitted: Arc<AdmittedMemberClass>,
        identity: &meerkat_mobkit::identity_first::AgentIdentity,
    ) -> Result<(), meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
        use crate::installed_host::dynamic_members::MemberClassAdmissionPhase;
        use crate::private_originals::planning_source_evidence::PlanningSourceEvidence;
        use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
        self.check_member_class_owner(&admitted)?;
        let owner = self
            .service_tool_policy
            .get()
            .ok_or(HostLifetimeError::Unavailable)?;
        let current = owner
            .observed_admission(&admitted.instance().member.member)?
            .ok_or(HostLifetimeError::Denied)?;
        let MemberClassAdmissionPhase::Active {
            context,
            context_generation,
        } = &current.phase
        else {
            return Err(HostLifetimeError::Denied);
        };
        if current != admitted.admission
            || current.class.purpose != CompiledMemberClassPurpose::SharedWorkflow
            || current.installed_surface.is_none()
            || current.native_surface.is_none()
            || meerkat_mobkit::member_comms_id::roster_member_id_for_identity(identity.as_str())
                .as_str()
                != current.instance.member.member.member
            || current.destination.as_ref().is_none_or(|destination| {
                &destination.context != context
                    || destination.context_generation != *context_generation
            })
        {
            return Err(HostLifetimeError::Denied);
        }
        let binding = current.workflow.as_ref().ok_or(HostLifetimeError::Denied)?;
        let route = current.route.as_ref().ok_or(HostLifetimeError::Denied)?;
        let (_, stamp) = self
            .owner
            .publication()
            .observe(|| ())
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let projected = self
            .project_class_source_route(&current.selected_member()?, route)
            .map_err(host_error)?;
        let key = self
            .key_for_route(&binding.original, &projected)
            .map_err(host_error)?;
        let row = self
            .store
            .get(&key)
            .await
            .map_err(store_error)
            .map_err(host_error)?
            .ok_or(HostLifetimeError::Unavailable)?;
        let record = self
            .decode_record_for_route(&row, &binding.original, &projected)
            .map_err(host_error)?;
        self.check_planning_evidence_source(&record, &binding.original)
            .map_err(host_error)?;
        let association = record
            .final_association
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        let input = record
            .bound_service_input
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?
            .decode()
            .map_err(host_error)?;
        if record.operation_cancelled
            || record.planning_source_evidence
                != Some(PlanningSourceEvidence::Lead {
                    parent: binding.parent.clone(),
                })
            || input.header().authority_association.as_ref() != Some(association)
            || &association.candidate().target.context != context
            || association.candidate().target.context_generation != *context_generation
        {
            return Err(HostLifetimeError::Denied);
        }
        let parent_row = self
            .store
            .get(&self.key(&binding.parent).map_err(host_error)?)
            .await
            .map_err(store_error)
            .map_err(host_error)?
            .ok_or(HostLifetimeError::Unavailable)?;
        let parent = self
            .decode_record(&parent_row, &binding.parent)
            .map_err(host_error)?;
        self.check_planning_evidence_source(&parent, &binding.parent)
            .map_err(host_error)?;
        let parent_association = parent
            .final_association
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        if parent.operation_cancelled
            || !crate::private_originals::planning_source_evidence::is_parent(
                parent.planning_source_evidence.as_ref(),
            )
            || parent.bound_service_input.is_none()
            || association.candidate().contributing_work.as_slice()
                != [parent_association.candidate().original_work.clone()]
        {
            return Err(HostLifetimeError::Denied);
        }
        let workflow = self
            .workflow_store()
            .map_err(host_error)?
            .load_setup_workflow(&binding.workflow)
            .await
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let terms = workflow.terms();
        if terms.planning_source != key
            || terms.lead != current.instance.member.member
            || terms.realm != projected.realm
            || terms.requester != projected.requester
            || self
                .owner
                .clock()
                .now()
                .map_err(|_| HostLifetimeError::Unavailable)?
                .unix_ms
                >= terms.deadline_ms
        {
            return Err(HostLifetimeError::Denied);
        }
        let selected = crate::original_ingress::workflow_member::SelectedWorkflowMember {
            class_admission: Some(admitted.clone()),
            reference: Some(binding.workflow.clone()),
            member: current.instance.member.member.clone(),
            original: association.candidate().original_work.clone(),
            realm: projected.realm,
            identity: identity.clone(),
            captured: None,
        };
        self.owner
            .select_workflow_member(
                selected,
                || {
                    stamp
                        .check_current()
                        .map_err(|_| OriginalWriterError::Unavailable)?;
                    self.check_member_class_owner(&admitted)
                        .map_err(|_| OriginalWriterError::WrongOwner)?;
                    if owner
                        .observed_admission(&current.instance.member.member)
                        .map_err(|_| OriginalWriterError::Unavailable)?
                        .as_ref()
                        != Some(&current)
                    {
                        return Err(OriginalWriterError::WrongOwner);
                    }
                    Ok(())
                },
                || Ok(()),
            )
            .await
            .map_err(setup_workflow::custody_error)
            .map_err(host_error)
    }

    pub(crate) async fn select_workflow_class(
        &self,
        admitted: Arc<AdmittedMemberClass>,
        route: &CompiledNativeSourceRoute,
        spec: &meerkat_mobkit::identity_first::DurableAgentSpec,
    ) -> Result<(), meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
        self.check_workflow_class_source(&admitted, route).await?;
        let binding = admitted
            .workflow()
            .ok_or_else(|| host_error(PrivateOriginalError::Denied))?;
        let projected = self
            .project_class_source_route(&admitted.instance().member, route)
            .map_err(host_error)?;
        self.owner
            .select_workflow_member(
                crate::original_ingress::workflow_member::SelectedWorkflowMember {
                    reference: Some(binding.workflow.clone()),
                    member: admitted.instance().member.member.clone(),
                    original: meerkat_authorization_contracts::work_association::OriginalWorkRef {
                        authority: projected.source_authority,
                        work: binding.original.original.clone(),
                    },
                    realm: projected.realm,
                    identity: spec.identity.clone(),
                    captured: None,
                    class_admission: Some(admitted),
                },
                || Ok(()),
                || Ok(()),
            )
            .await
            .map_err(setup_workflow::custody_error)
            .map_err(host_error)
    }
    pub(crate) fn capture_workflow_class(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
        destination: &meerkat_mob::MemberDestinationSelection,
    ) -> Result<(), PrivateOriginalError> {
        let owner = self
            .service_tool_policy
            .get()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let Some(admission) = owner
            .observed_admission(identity.member())
            .map_err(|_| PrivateOriginalError::Unavailable)?
        else {
            return Ok(());
        };
        let Some(binding) = &admission.workflow else {
            return Ok(());
        };
        self.owner
            .capture_workflow_member(&binding.workflow, identity, destination)
            .map_err(setup_workflow::custody_error)
    }
    pub(crate) async fn finish_workflow_class(
        &self,
        admitted: &AdmittedMemberClass,
        held: &meerkat_mobkit::identity_first::HeldIdentityMember,
    ) -> Result<(), meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
        let Some(binding) = admitted.workflow() else {
            return Ok(());
        };
        self.owner
            .finish_workflow_member(&binding.workflow, held)
            .await
            .map_err(setup_workflow::custody_error)
            .map_err(host_error)
    }
}

impl PrivateOriginals {
    pub(crate) fn completed_workflow_class(
        self: &Arc<Self>,
        admitted: &AdmittedMemberClass,
    ) -> Result<
        (
            crate::installed_host::dynamic_members::MemberClassAdmission,
            Arc<setup_workflow::InstalledWorkflowMember>,
        ),
        meerkat_toolkit::host_lifetime::control::HostLifetimeError,
    > {
        self.check_member_class_owner(admitted)?;
        let binding = admitted
            .workflow()
            .ok_or_else(|| host_error(PrivateOriginalError::Denied))?;
        let (current, _) = admitted
            .owner
            .member_route(&binding.original.route)?
            .ok_or_else(|| host_error(PrivateOriginalError::Unavailable))?;
        if current.id != *admitted.admission_id()
            || current.workflow.as_ref() != Some(binding)
            || current.native_surface.is_none()
            || current.class.purpose != CompiledMemberClassPurpose::SharedWorkflow
        {
            return Err(host_error(PrivateOriginalError::Denied));
        }
        self.current_workflow_parent(&binding.parent)
            .map_err(host_error)?;
        let member = setup_workflow::InstalledWorkflowMember::from_completed_class(self, &current)
            .map_err(host_error)?;
        Ok((current, member))
    }
}
