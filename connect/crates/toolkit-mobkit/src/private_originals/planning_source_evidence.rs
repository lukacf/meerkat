//! Explicit planning input evidence, separate from native source custody.
use super::cold_service_admission::writer_error;
use super::*;
use crate::original_ingress::{OriginalCancellationSource, controller_only_basis};
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, OriginalWorkRef, WorkAuthorityBasis,
};
use meerkat_runtime::meerkat_machine::{AdmittedOriginalRefusal, AuthenticatedOriginal};

/// Immutable source-owner selection. Lineage and envelope remain in the
/// existing bound input and native grant owner, so they are not copied here.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LateWorkflowHelperProvenance {
    pub admission: crate::installed_host::dynamic_members::MemberClassAdmission,
    pub server: meerkat_core::McpServerConfig,
}

/// Only the actual host preparation calls select this evidence purpose. It is
/// not a grant, a recipient selection or an authority reconstruction format.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case", deny_unknown_fields)]
#[allow(
    clippy::large_enum_variant,
    reason = "one evidence value per source record; boxing would touch every construction and match"
)]
pub(super) enum PlanningSourceEvidence {
    Parent,
    OperationalParent {
        selection: crate::installed_host::operator_setup::WorkflowParentAuthoritySelection,
    },
    Lead {
        parent: PrivateOriginalRef,
    },
    EffectWaiter {
        workflow: meerkat_toolkit::setup::SetupWorkflowRef,
        planning: PrivateOriginalRef,
    },
    Waiter {
        workflow: meerkat_toolkit::setup::SetupWorkflowRef,
        planning: PrivateOriginalRef,
    },
}

pub(super) fn validate(record: &SourceRecord) -> Result<(), PrivateOriginalError> {
    if let Some(helper) = &record.late_workflow_helper {
        let Some(PlanningSourceEvidence::Lead { parent }) = &record.planning_source_evidence else {
            return Err(PrivateOriginalError::Conflict);
        };
        let binding = helper
            .admission
            .workflow
            .as_ref()
            .ok_or(PrivateOriginalError::Conflict)?;
        if helper.admission.class.purpose
            != meerkat_toolkit::host_lifetime::policy::CompiledMemberClassPurpose::SharedWorkflow
            || !matches!(
                helper.admission.phase,
                crate::installed_host::dynamic_members::MemberClassAdmissionPhase::Active { .. }
            )
            || &binding.parent != parent
            || binding.original.original != record.event.original
            || binding.original.route != record.event.route
            || binding.original.event_sha256
                != digest(
                    &canonical_json(&record.event).map_err(|_| PrivateOriginalError::Conflict)?,
                )
            || helper
                .admission
                .route
                .as_ref()
                .is_none_or(|route| route.id != record.event.route)
            || helper.server.name.is_empty()
            || helper.server.connector.is_some()
        {
            return Err(PrivateOriginalError::Conflict);
        }
    }
    let Some(selection) = &record.planning_source_evidence else {
        return Ok(());
    };
    if record.native_operations.is_some() {
        return Err(PrivateOriginalError::Conflict);
    }
    match selection {
        PlanningSourceEvidence::Parent | PlanningSourceEvidence::OperationalParent { .. }
            if !record.event.contributors.is_empty() =>
        {
            return Err(PrivateOriginalError::Conflict);
        }
        PlanningSourceEvidence::Lead { parent }
            if record.event.contributors.as_slice() != [parent.original.clone()]
                || record.event.original == parent.original
                || !valid_digest(&parent.event_sha256) =>
        {
            return Err(PrivateOriginalError::Conflict);
        }
        PlanningSourceEvidence::Waiter { planning, .. }
        | PlanningSourceEvidence::EffectWaiter { planning, .. }
            if !record.event.contributors.is_empty()
                || record.event.original == planning.original
                || !valid_digest(&planning.event_sha256) =>
        {
            return Err(PrivateOriginalError::Conflict);
        }
        _ => (),
    }
    if let PlanningSourceEvidence::OperationalParent { selection } = selection
        && (!valid_digest(&selection.commission_sha256)
            || selection.authority.parent_issuance_id == selection.authority.leaf_issuance_id)
    {
        return Err(PrivateOriginalError::Conflict);
    }
    if let Some(association) = &record.final_association {
        check_binding(record, association)?;
    }
    Ok(())
}

pub(super) fn check_binding(
    record: &SourceRecord,
    association: &InputAuthorityAssociation,
) -> Result<(), PrivateOriginalError> {
    let Some(selection) = &record.planning_source_evidence else {
        return Ok(());
    };
    if record.approved_task.is_some()
        || record.service_admission.is_some()
        || record.setup_request.is_some()
        || (record.configured_setup.is_some()
            && !matches!(
                selection,
                PlanningSourceEvidence::Waiter { .. } | PlanningSourceEvidence::EffectWaiter { .. }
            ))
    {
        return Err(PrivateOriginalError::Conflict);
    }
    let claims = association.candidate();
    let matches = match selection {
        PlanningSourceEvidence::Parent => controller_only_basis(&claims.authority_basis)
            && claims.contributing_work.is_empty(),
        PlanningSourceEvidence::OperationalParent { selection } =>
            claims.contributing_work.is_empty() && matches!(&claims.authority_basis,
                WorkAuthorityBasis::GrantLineage { lineage } if lineage.len() == 2
                    && lineage[0].grant_id == selection.authority.parent_issuance_id
                    && lineage[1].grant_id == selection.authority.leaf_issuance_id),
        PlanningSourceEvidence::Lead { parent } =>
            crate::installed_host::operator_setup::workflow_mcp::split(&claims.authority_basis)
                .is_ok_and(|(_, target)| target.is_none() || record.late_workflow_helper.as_ref()
                    .is_some_and(|helper| helper.server.name == crate::installed_host::operator_setup::workflow_mcp::SERVER))
                && claims.contributing_work.as_slice() == [OriginalWorkRef {
                    authority: record.source.source_authority.clone(), work: parent.original.clone(),
                }],
        PlanningSourceEvidence::EffectWaiter { .. } => record.configured_setup.as_ref().is_some_and(|saved|
            saved.capability.helper.is_some() && saved.capability.helper_targets.is_empty()
                && !saved.capability.workflow_targets.is_empty()
                && crate::installed_host::operator_setup::setup_targets::validate_workflow_compiled(&saved.capability.workflow_targets).is_ok())
            && claims.contributing_work.is_empty()
            && matches!(&claims.authority_basis, WorkAuthorityBasis::TargetedGrantLineages { base: None, .. }),
        PlanningSourceEvidence::Waiter { .. } => record.configured_setup.as_ref().is_some_and(|saved|
            saved.capability.helper.is_some() && saved.capability.helper_targets.is_empty() && saved.capability.workflow_targets.is_empty())
            && claims.contributing_work.is_empty()
            && matches!(&claims.authority_basis, WorkAuthorityBasis::GrantLineage { lineage } if lineage.len() == 2),
    };
    if matches {
        Ok(())
    } else {
        Err(PrivateOriginalError::Conflict)
    }
}

impl PrivateOriginals {
    /// Opt this exact host-authenticated P into retaining its bound Input before
    /// native submission. Call before P's first binding, then submit through the
    /// ordinary authenticated-original owner. No content is copied by this call.
    ///
    /// The same selection replays without a write. An already-bound unselected
    /// original cannot be retrofitted: its body still belongs to its application.
    /// This profile supports host-submitted sources without a Console login.
    /// Submission with a live Console login is refused; restoring that login's
    /// current custody is a separate, unsupported composition.
    /// This does not select a workflow, grant permission or prove admission.
    pub async fn prepare_workflow_parent_source(
        &self,
        reference: &PrivateOriginalRef,
    ) -> Result<(), PrivateOriginalError> {
        self.select_planning_source_evidence(reference, PlanningSourceEvidence::Parent, None)
            .await
    }

    pub(super) async fn select_workflow_lead_evidence(
        &self,
        reference: &PrivateOriginalRef,
        parent: &PrivateOriginalRef,
        helper: Option<&InstalledLateWorkflowHelper>,
    ) -> Result<(), PrivateOriginalError> {
        // Called only after prepare_workflow_planning has validated its actual
        // retained P, independently issued G lineage and exact destination.
        let row = self
            .store
            .get(&self.key(parent)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let parent_record = self.decode_record(&row, parent)?;
        let row = self
            .store
            .get(&self.key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&row, reference)?;
        if parent_record.final_association.is_none()
            || parent_record.source.source_authority != record.source.source_authority
            || parent_record.source.realm != record.source.realm
            || parent_record.source.requester != record.source.requester
        {
            return Err(PrivateOriginalError::Conflict);
        }
        // Existing live workflows without this evidence keep their warm path.
        // This read is not permission to reconstruct or resubmit the old input.
        if record.final_association.is_some() && record.planning_source_evidence.is_none() {
            return Ok(());
        }
        self.select_planning_source_evidence(
            reference,
            PlanningSourceEvidence::Lead {
                parent: parent.clone(),
            },
            helper,
        )
        .await
    }

    async fn select_planning_source_evidence(
        &self,
        reference: &PrivateOriginalRef,
        selection: PlanningSourceEvidence,
        helper: Option<&InstalledLateWorkflowHelper>,
    ) -> Result<(), PrivateOriginalError> {
        let selected_helper = helper.map(InstalledLateWorkflowHelper::provenance);
        if let Some(helper) = helper {
            helper
                .check_source(self)
                .map_err(|_| PrivateOriginalError::Denied)?;
        }
        let key = self.key(reference)?;
        let row = self
            .store
            .get(&key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&row, reference)?;
        self.check_planning_evidence_source(&record, reference)?;
        if let Some(saved) = &record.planning_source_evidence {
            if saved != &selection
                || (record.late_workflow_helper.is_some()
                    && record.late_workflow_helper != selected_helper)
            {
                return Err(PrivateOriginalError::Conflict);
            }
            if record.late_workflow_helper == selected_helper {
                return Ok(());
            }
            // Legacy warm custody remains usable, but bound source evidence is
            // never retrofitted with newly inferred cold-recovery provenance.
            if record.final_association.is_some() && record.late_workflow_helper.is_none() {
                return Ok(());
            }
        }
        if record.final_association.is_some() {
            return Err(PrivateOriginalError::Unavailable);
        }
        let route = self.route(&reference.route)?;
        let original = OriginalWorkRef {
            authority: route.source_authority.clone(),
            work: reference.original.clone(),
        };
        self.owner
            .with_unbound_source_selection(&route.realm, &original, || async {
                let row = self
                    .store
                    .get(&key)
                    .await
                    .map_err(store_error)?
                    .ok_or(PrivateOriginalError::Unavailable)?;
                let mut record = self.decode_record(&row, reference)?;
                self.check_planning_evidence_source(&record, reference)?;
                if let Some(saved) = &record.planning_source_evidence {
                    if saved != &selection
                        || (record.late_workflow_helper.is_some()
                            && record.late_workflow_helper != selected_helper)
                    {
                        return Err(PrivateOriginalError::Conflict);
                    }
                    if record.late_workflow_helper == selected_helper {
                        return Ok(());
                    }
                }
                if record.final_association.is_some() {
                    return Err(PrivateOriginalError::Unavailable);
                }
                if record.operation_cancelled
                    || record.approved_task.is_some()
                    || record.service_admission.is_some()
                    || record.native_operations.is_some()
                    || record.setup_request.is_some()
                    || record.configured_setup.is_some()
                {
                    return Err(PrivateOriginalError::Denied);
                }
                record.planning_source_evidence = Some(selection);
                record.late_workflow_helper = selected_helper;
                validate(&record)?;
                if let Some(helper) = helper {
                    helper
                        .check_source(self)
                        .map_err(|_| PrivateOriginalError::Denied)?;
                }
                // A lost store reply remains uncertain. Repeating this exact
                // selection reads the same row; it never creates another input.
                self.store
                    .compare_and_swap(Transition {
                        key,
                        expected_revision: row.revision,
                        expected_digest: Some(row.content_digest),
                        claim: None,
                        data: encode(record)?,
                        events: vec![],
                        effects: vec![],
                    })
                    .await
                    .map_err(store_error)?;
                Ok(())
            })
            .await
            .map_err(writer_error)?
    }

    pub(super) fn check_planning_evidence_source(
        &self,
        record: &SourceRecord,
        reference: &PrivateOriginalRef,
    ) -> Result<(), PrivateOriginalError> {
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
        let route = self.route(&reference.route)?;
        let (compiled, member, _) = self.selected_source_route(&reference.route)?;
        let (binding, requester, actor) = compiled
            .source_binding(selected)
            .map_err(|_| PrivateOriginalError::Denied)?;
        if &member.member != binding {
            return Err(PrivateOriginalError::Conflict);
        }
        if self.scope.realm != native.realm.to_string()
            || self.scope.owner != selected.deployment.to_string()
            || route.grant_caller != native.grant_administrator
            || route.grant_target.as_str() != binding.member
            || &record.source.requester != requester
            || &record.source.ingress_actor != actor
            || record.source.logical_executor != compiled.logical_executor
            || record.source.represented_subject != compiled.represented_subject
            || record.source.source_authority != compiled.source_authority
            || record.source.authentication_domain != compiled.authentication_domain
            || record.source.event_domain != compiled.event_domain
        {
            return Err(PrivateOriginalError::Conflict);
        }
        if let Some(association) = &record.final_association {
            let claims = association.candidate();
            let session = meerkat_core::SessionId::parse(claims.target.context.as_str())
                .map_err(|_| PrivateOriginalError::Conflict)?;
            let parent = compiled
                .controller_parent
                .as_ref()
                .ok_or(PrivateOriginalError::Unavailable)?;
            if claims.target.logical_owner != member.logical_owner
                || claims.target.audience != member.audience
                || claims.target.context_generation != member.context_generation
                || claims.target.logical_runtime.as_str()
                    != meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string()
                || claims.controller_grant_lineage.len() != parent.len() + 1
                || !claims.controller_grant_lineage.starts_with(parent)
            {
                return Err(PrivateOriginalError::Conflict);
            }
        }
        Ok(())
    }

    /// Reacquire only an explicitly selected P's historical
    /// source custody. Missing evidence or native absence stays unavailable;
    /// this never dispatches an input, restores a login, or creates a task grant.
    pub(crate) async fn restore_workflow_parent_source(
        &self,
        reference: &PrivateOriginalRef,
    ) -> Result<(), PrivateOriginalError> {
        let writer = self.owner.try_source_writer().map_err(writer_error)?;
        let (_, stamp) = self
            .owner
            .publication()
            .observe(|| ())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let row = self
            .store
            .get(&self.key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&row, reference)?;
        if !is_parent(record.planning_source_evidence.as_ref()) {
            return Err(PrivateOriginalError::Unavailable);
        }
        self.check_planning_evidence_source(&record, reference)?;
        if record.operation_cancelled {
            return Err(PrivateOriginalError::Denied);
        }
        let association = record
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let input = record
            .bound_service_input
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .decode()?;
        let claims = association.candidate();
        let session = meerkat_core::SessionId::parse(claims.target.context.as_str())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        // Resolution can wait on the grant owner, so complete it before native
        // acquires B/M/D. Only its atomic publication check runs in the callback.
        let controller = self
            .owner
            .grants()
            .resolve_controller_lineage(
                &claims.controller_grant_lineage,
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )
            .map_err(|error| writer_error(OriginalWriterError::Grant(error)))?;
        let operational = self.workflow_parent_basis(&record, reference)?;
        let operational = match operational.as_ref() {
            Some(WorkAuthorityBasis::GrantLineage { lineage }) => Some(
                self.owner
                    .grants()
                    .resolve_lineage(
                        lineage,
                        &claims.logical_executor,
                        claims.represented_subject.as_ref(),
                    )
                    .map_err(|error| writer_error(OriginalWriterError::Grant(error)))?,
            ),
            None => None,
            _ => return Err(PrivateOriginalError::Conflict),
        };
        let sources = [OriginalCancellationSource {
            realm: record.source.realm.clone(),
            original: claims.original_work.clone(),
            record: Arc::new(row),
            association: Some(association.clone()),
        }];
        stamp
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        // Source cancellation can enter while native obtains the actual B/M/D
        // owners. Their synchronous callback reacquires only nonwaiting locks.
        drop(writer);
        let pins = self
            .controller_pin_source
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let machine = self.machine()?;
        let registration = machine
            .current_session_registration_witness(&session)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let attachment = machine
            .current_executor_attachment_witness(&session)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let actor = pins
            .live_actor(&session)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let custody = AuthenticatedOriginal {
            original_work: claims.original_work.clone(),
            occurrence_scope: claims.ingress_namespace.occurrence_scope.clone(),
            original_authentication: claims.original_authentication.clone(),
            root_event: claims.root_event.clone(),
            contributing_work: claims.contributing_work.clone(),
            target: claims.target.clone(),
            authority_basis: claims.authority_basis.clone(),
        };
        let observed = machine
            .with_admitted_original(
                &registration,
                &attachment,
                &actor.executor_actor_identity(),
                &input,
                |admitted| {
                    let _writer = self.owner.try_source_writer().map_err(writer_error)?;
                    stamp
                        .check_current()
                        .map_err(|_| PrivateOriginalError::Unavailable)?;
                    controller
                        .check_current()
                        .map_err(|_| PrivateOriginalError::Unavailable)?;
                    if admitted.input_id() != input.id() {
                        return Err(PrivateOriginalError::Conflict);
                    }
                    if let Some(operational) = &operational {
                        operational
                            .check_current()
                            .map_err(|_| PrivateOriginalError::Unavailable)?;
                        self.owner
                            .promote_historical_task(association, custody, &sources, admitted)
                            .map_err(writer_error)
                    } else {
                        self.owner
                            .promote_historical_controller_only(
                                association,
                                custody,
                                &sources,
                                admitted,
                            )
                            .map_err(writer_error)
                    }
                },
            )
            .await;
        let retained = match observed {
            Ok(result) => result?,
            Err(AdmittedOriginalRefusal::Conflict) => return Err(PrivateOriginalError::Conflict),
            Err(AdmittedOriginalRefusal::NotAdmitted | AdmittedOriginalRefusal::Unavailable) => {
                return Err(PrivateOriginalError::Unavailable);
            }
        };
        // Native releases B/M/D before this existing holder's canonical read.
        self.hold_binding(reference.clone(), retained).await
    }
}

pub(super) fn is_parent(selection: Option<&PlanningSourceEvidence>) -> bool {
    matches!(
        selection,
        Some(PlanningSourceEvidence::Parent | PlanningSourceEvidence::OperationalParent { .. })
    )
}

mod lead;
mod operational_parent;
mod waiter;
