//! Idle G custody restoration after actual ordinary registration/resume.
use super::*;
use meerkat_authorization_contracts::constraints::DelegationDepth;
use meerkat_core::MobMemberBinding;
use meerkat_toolkit::setup::SetupWorkflowRef;
use toolkit_package::ContentDigest;

// Both values are live native observations, never retained permission.
pub(in crate::private_originals) struct WorkflowLeadGrants {
    base: meerkat_authorization::grants::ResolvedGrant,
    mcp: Option<meerkat_authorization::grants::ResolvedGrant>,
}
impl WorkflowLeadGrants {
    pub(in crate::private_originals) fn restrictions(
        &self,
    ) -> &meerkat_authorization_contracts::constraints::ExecutionRestrictions {
        self.base.restrictions()
    }
    pub(in crate::private_originals) fn check_current(&self) -> Result<(), PrivateOriginalError> {
        self.base
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if let Some(mcp) = &self.mcp {
            mcp.check_current()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
        }
        Ok(())
    }
}

impl PrivateOriginals {
    pub(in crate::private_originals) async fn restore_workflow_lead_source(
        &self,
        workflow: &SetupWorkflowRef,
        reference: &PrivateOriginalRef,
        member: &MobMemberBinding,
    ) -> Result<(), PrivateOriginalError> {
        // Validate the complete read-only selection before the first native
        // await can promote P. Repeat it under the existing source publication.
        let (_, selected, _, _, _, parent, _) = self
            .workflow_lead_evidence(workflow, reference, member)
            .await?;
        self.owner
            .check_saved_workflow_destination(
                workflow,
                member,
                selected
                    .final_association
                    .as_ref()
                    .ok_or(PrivateOriginalError::Unavailable)?,
            )
            .map_err(writer_error)?;
        self.restore_workflow_parent_source(&parent).await?;
        let writer = self.owner.try_source_writer().map_err(writer_error)?;
        let (_, stamp) = self
            .owner
            .publication()
            .observe(|| ())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let (row, record, parent_row, parent_record, snapshot, checked_parent, operation) = self
            .workflow_lead_evidence(workflow, reference, member)
            .await?;
        if checked_parent != parent {
            return Err(PrivateOriginalError::Conflict);
        }
        let association = record
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let parent_association = parent_record
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !self
            .owner
            .has_admitted_source(parent_association)
            .map_err(writer_error)?
        {
            return Err(PrivateOriginalError::Unavailable);
        }
        let input = record
            .bound_service_input
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .decode()?;
        let terms = snapshot.terms();
        let claims = association.candidate();
        let parent_claims = parent_association.candidate();
        let grants = self.owner.grants();
        let controller = grants
            .resolve_controller_lineage(
                &claims.controller_grant_lineage,
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )
            .map_err(|error| writer_error(OriginalWriterError::Grant(error)))?;
        let parent_controller = grants
            .resolve_controller_lineage(
                &parent_claims.controller_grant_lineage,
                &parent_claims.logical_executor,
                parent_claims.represented_subject.as_ref(),
            )
            .map_err(|error| writer_error(OriginalWriterError::Grant(error)))?;
        let session = meerkat_core::SessionId::parse(claims.target.context.as_str())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        let sources = [
            OriginalCancellationSource {
                realm: parent_record.source.realm.clone(),
                original: parent_claims.original_work.clone(),
                record: Arc::new(parent_row),
                association: Some(parent_association.clone()),
            },
            OriginalCancellationSource {
                realm: record.source.realm.clone(),
                original: claims.original_work.clone(),
                record: Arc::new(row),
                association: Some(association.clone()),
            },
        ];
        stamp
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        drop(writer);
        let pins = self
            .controller_pin_source
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let machine = self.machine()?;
        Self::wake_original_session_owner(&machine, &*pins, &session, &stamp).await?;
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
                    operation
                        .check_current()
                        .map_err(|_| PrivateOriginalError::Unavailable)?;
                    controller
                        .check_current()
                        .map_err(|_| PrivateOriginalError::Unavailable)?;
                    parent_controller
                        .check_current()
                        .map_err(|_| PrivateOriginalError::Unavailable)?;
                    if self
                        .owner
                        .clock()
                        .now()
                        .map_err(|_| PrivateOriginalError::Unavailable)?
                        .unix_ms
                        >= terms.deadline_ms
                    {
                        return Err(PrivateOriginalError::Denied);
                    }
                    if admitted.input_id() != input.id() {
                        return Err(PrivateOriginalError::Conflict);
                    }
                    self.owner
                        .check_saved_workflow_destination(workflow, member, association)
                        .map_err(writer_error)?;
                    self.owner
                        .check_admitted_workflow_member(
                            workflow,
                            member,
                            &claims.original_work,
                            association,
                            admitted,
                        )
                        .map_err(writer_error)?;
                    // Promotion installs only the actual source/controller custody. The
                    // member's controller=None keeps input and operation policy closed.
                    // No fallible member mutation follows this single source mutation.
                    self.owner
                        .promote_historical_task(association, custody, &sources, admitted)
                        .map_err(writer_error)
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
        self.hold_binding(reference.clone(), retained).await
    }

    pub(in crate::private_originals) async fn workflow_lead_evidence(
        &self,
        workflow: &SetupWorkflowRef,
        reference: &PrivateOriginalRef,
        member: &MobMemberBinding,
    ) -> Result<
        (
            toolkit_store::StoredRecord,
            SourceRecord,
            toolkit_store::StoredRecord,
            SourceRecord,
            meerkat_toolkit::setup::SetupWorkflowSnapshot,
            PrivateOriginalRef,
            WorkflowLeadGrants,
        ),
        PrivateOriginalError,
    > {
        let row = self
            .store
            .get(&self.key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&row, reference)?;
        let Some(PlanningSourceEvidence::Lead { parent }) = &record.planning_source_evidence else {
            return Err(PrivateOriginalError::Unavailable);
        };
        self.check_planning_evidence_source(&record, reference)?;
        if record.operation_cancelled {
            return Err(PrivateOriginalError::Denied);
        }
        let input = record
            .bound_service_input
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .decode()?;
        let association = record
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if input.header().authority_association.as_ref() != Some(association) {
            return Err(PrivateOriginalError::Conflict);
        }
        if self
            .owner
            .captured_workflow_member(workflow, association)
            .map_err(writer_error)?
            != *member
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let parent_row = self
            .store
            .get(&self.key(parent)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let parent_record = self.decode_record(&parent_row, parent)?;
        self.check_planning_evidence_source(&parent_record, parent)?;
        if parent_record.operation_cancelled {
            return Err(PrivateOriginalError::Denied);
        }
        if !crate::private_originals::planning_source_evidence::is_parent(
            parent_record.planning_source_evidence.as_ref(),
        ) || parent_record.bound_service_input.is_none()
        {
            return Err(PrivateOriginalError::Unavailable);
        }
        let parent_association = parent_record
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let snapshot = self
            .workflow_store()?
            .load_setup_workflow(workflow)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let terms = snapshot.terms();
        let claims = association.candidate();
        let parent_claims = parent_association.candidate();
        if terms.planning_source != self.key(reference)?
            || &terms.lead != member
            || terms.requester != claims.requester
            || terms.realm != claims.ingress_namespace.realm
            || parent_claims.requester != claims.requester
            || parent_claims.ingress_namespace.realm != claims.ingress_namespace.realm
            || claims.contributing_work.as_slice() != [parent_claims.original_work.clone()]
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let now = self
            .owner
            .clock()
            .now()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .unix_ms;
        if now >= terms.deadline_ms {
            return Err(PrivateOriginalError::Denied);
        }
        let (lineage, target) =
            crate::installed_host::operator_setup::workflow_mcp::split(&claims.authority_basis)
                .map_err(|_| PrivateOriginalError::Conflict)?;
        self.owner
            .check_historical_task_child(association)
            .map_err(writer_error)?;
        let grants = self.owner.grants();
        let operation = grants
            .resolve_lineage(
                lineage,
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )
            .map_err(|error| writer_error(OriginalWriterError::Grant(error)))?;
        if operation.restrictions().delegation_depth != DelegationDepth::remaining(0)
            || operation.expires_at_ms() < terms.deadline_ms
            || ContentDigest::of_bytes(
                &canonical_json(operation.restrictions())
                    .map_err(|_| PrivateOriginalError::Conflict)?,
            ) != terms.envelope_digest
        {
            return Err(PrivateOriginalError::Denied);
        }
        let mcp = match target {
            Some(target) => {
                let provenance = record
                    .late_workflow_helper
                    .as_ref()
                    .ok_or(PrivateOriginalError::Conflict)?;
                let lifetime = self.status_lifetime()?;
                let native = lifetime
                    .selected_policy()
                    .native_host
                    .as_ref()
                    .ok_or(PrivateOriginalError::Unavailable)?;
                Some(
                    crate::installed_host::operator_setup::workflow_mcp::resolve_target(
                        &grants,
                        &native.grant_administrator,
                        &claims.logical_executor,
                        claims.represented_subject.as_ref(),
                        lineage,
                        target,
                        operation.restrictions(),
                        &provenance.server,
                    )
                    .map_err(|error| match error {
                        meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable => {
                            PrivateOriginalError::Unavailable
                        }
                        _ => PrivateOriginalError::Denied,
                    })?,
                )
            }
            None => None,
        };
        let operation = WorkflowLeadGrants {
            base: operation,
            mcp,
        };
        // Membership can depart through its own durable owner during native
        // observation. It is deliberately not an admission fence here: restored
        // historical custody remains restricted regardless of waiter count.
        let parent = parent.clone();
        Ok((
            row,
            record,
            parent_row,
            parent_record,
            snapshot,
            parent,
            operation,
        ))
    }
}
