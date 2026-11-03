//! Explicit configured A evidence and historical source custody for one cohort.
//! This does not create the A-to-G delegation required by waiter effect custody.
use super::*;
use meerkat_core::MobMemberBinding;
use meerkat_toolkit::setup::SetupWorkflowRef;

impl PrivateOriginals {
    pub(in crate::private_originals) async fn check_workflow_waiter_binding(
        &self,
        record: &SourceRecord,
        reference: &PrivateOriginalRef,
    ) -> Result<(), PrivateOriginalError> {
        let (planning, shared) = match &record.planning_source_evidence {
            Some(PlanningSourceEvidence::Waiter { planning, .. }) => (planning, false),
            Some(PlanningSourceEvidence::EffectWaiter { planning, .. }) => (planning, true),
            _ => return Ok(()),
        };
        let member = record
            .configured_setup
            .as_ref()
            .and_then(|saved| saved.capability.helper.as_ref())
            .ok_or(PrivateOriginalError::Conflict)?;
        self.check_workflow_waiter_profile(record, reference, planning, member, shared)
            .await?;
        self.check_historical_setup_selection(record, reference)
            .await
    }

    pub(in crate::private_originals) async fn select_workflow_waiter_evidence(
        &self,
        workflow: &SetupWorkflowRef,
        planning: &PrivateOriginalRef,
        member: &MobMemberBinding,
        reference: &PrivateOriginalRef,
        shared: bool,
    ) -> Result<(), PrivateOriginalError> {
        let row = self
            .store
            .get(&self.key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&row, reference)?;
        self.check_workflow_waiter_profile(&record, reference, planning, member, shared)
            .await?;
        let selected = if shared {
            PlanningSourceEvidence::EffectWaiter {
                workflow: workflow.clone(),
                planning: planning.clone(),
            }
        } else {
            PlanningSourceEvidence::Waiter {
                workflow: workflow.clone(),
                planning: planning.clone(),
            }
        };
        self.select_planning_source_evidence(reference, selected, None)
            .await
    }

    async fn check_workflow_waiter_profile(
        &self,
        record: &SourceRecord,
        reference: &PrivateOriginalRef,
        planning: &PrivateOriginalRef,
        member: &MobMemberBinding,
        shared: bool,
    ) -> Result<(), PrivateOriginalError> {
        self.check_planning_evidence_source(record, reference)?;
        if reference == planning
            || !record.event.contributors.is_empty()
            || record.approved_task.is_some()
            || record.service_admission.is_some()
            || record.setup_request.is_some()
        {
            return Err(PrivateOriginalError::Denied);
        }
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let selected = lifetime.selected_policy();
        let native = selected
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let route = native
            .routes
            .iter()
            .find(|row| row.id == reference.route)
            .ok_or(PrivateOriginalError::Denied)?;
        let binding = route
            .source_binding(selected)
            .map_err(|_| PrivateOriginalError::Denied)?
            .0;
        let capability = if shared {
            record
                .configured_setup
                .as_ref()
                .map(|saved| &saved.capability)
                .or_else(|| {
                    native
                        .members
                        .iter()
                        .find(|row| &row.member == binding)
                        .and_then(|row| row.setup_capability.as_ref())
                })
        } else {
            native
                .members
                .iter()
                .find(|row| &row.member == binding)
                .and_then(|row| row.setup_capability.as_ref())
        }
        .ok_or(PrivateOriginalError::Unavailable)?;
        // This first profile matches the actual SDK cohort. A configured Find
        // child has no helper effect targets and cannot delegate another child.
        if capability.route != reference.route || capability.helper.as_ref() != Some(member)
            || !capability.helper_targets.is_empty()
            || (shared == capability.workflow_targets.is_empty())
            || (shared && crate::installed_host::operator_setup::setup_targets::validate_workflow_compiled(&capability.workflow_targets).is_err())
            || record.configured_setup.as_ref().is_some_and(|saved| &saved.capability != capability)
        { return Err(PrivateOriginalError::Conflict); }
        let row = self
            .store
            .get(&self.key(planning)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let lead = self.decode_record(&row, planning)?;
        self.check_planning_evidence_source(&lead, planning)?;
        let (lead_route, lead_member, _) = self.selected_source_route(&planning.route)?;
        if &lead_member.member != member {
            return Err(PrivateOriginalError::Conflict);
        }
        if lead_route
            .source_binding(selected)
            .map_err(|_| PrivateOriginalError::Denied)?
            .0
            != member
            || lead.event.contributors.len() != 1
            || lead.source.requester != record.source.requester
            || lead.source.realm != record.source.realm
            || lead.source.source_authority != record.source.source_authority
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(())
    }

    pub(in crate::private_originals) async fn restore_workflow_waiter_source(
        &self,
        workflow: &SetupWorkflowRef,
        planning: &PrivateOriginalRef,
        member: &MobMemberBinding,
        reference: &PrivateOriginalRef,
    ) -> Result<(), PrivateOriginalError> {
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("workflow.waiter_restore.entered", ());
        // This ordinary async entry holds no native B/M/D custody. Join the
        // existing writer; only the callback below must acquire it without waiting.
        let writer = self.owner.source_writer().await;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("workflow.waiter_restore.source_writer", ());
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
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("workflow.waiter_restore.record", ());
        let shared = match &record.planning_source_evidence {
            Some(PlanningSourceEvidence::Waiter {
                workflow: saved,
                planning: lead,
            }) if saved == workflow && lead == planning => false,
            Some(PlanningSourceEvidence::EffectWaiter {
                workflow: saved,
                planning: lead,
            }) if saved == workflow && lead == planning => true,
            _ => return Err(PrivateOriginalError::Unavailable),
        };
        self.check_workflow_waiter_profile(&record, reference, planning, member, shared)
            .await?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("workflow.waiter_restore.profile", ());
        if record.operation_cancelled {
            return Err(PrivateOriginalError::Denied);
        }
        let snapshot = self
            .workflow_store()?
            .load_setup_workflow(workflow)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let terms = snapshot.terms();
        if terms.planning_source != self.key(planning)?
            || &terms.lead != member
            || terms.requester != record.source.requester
            || terms.realm != record.source.realm
        {
            return Err(PrivateOriginalError::Conflict);
        }
        // No canonical waiter count is treated as a currentness fence. This
        // restores only A custody; G's incomplete attachment remains restricted.
        self.check_historical_setup_selection(&record, reference)
            .await?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("workflow.waiter_restore.historical_selection", ());
        let association = record
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let input = record
            .bound_service_input
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .decode()?;
        if input.header().authority_association.as_ref() != Some(association) {
            return Err(PrivateOriginalError::Conflict);
        }
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("workflow.waiter_restore.retained_input", ());
        let claims = association.candidate();
        let grants = self.owner.grants();
        let operation = match (&claims.authority_basis, shared) {
            (WorkAuthorityBasis::GrantLineage { lineage }, false) => Some(
                grants
                    .resolve_lineage(
                        lineage,
                        &claims.logical_executor,
                        claims.represented_subject.as_ref(),
                    )
                    .map_err(|error| writer_error(OriginalWriterError::Grant(error)))?,
            ),
            // Historical targeted selection is identity, not a joint permission
            // check. The selected operation resolves its own lineage on entry.
            (WorkAuthorityBasis::TargetedGrantLineages { .. }, true) => None,
            _ => return Err(PrivateOriginalError::Conflict),
        };
        let controller = grants
            .resolve_controller_lineage(
                &claims.controller_grant_lineage,
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )
            .map_err(|error| writer_error(OriginalWriterError::Grant(error)))?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("workflow.waiter_restore.controller_lineage", ());
        let session = meerkat_core::SessionId::parse(claims.target.context.as_str())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        let sources = [OriginalCancellationSource {
            realm: record.source.realm.clone(),
            original: claims.original_work.clone(),
            record: Arc::new(row),
            association: Some(association.clone()),
        }];
        stamp
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        drop(writer);
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("workflow.waiter_restore.source_checked", ());
        #[cfg(test)]
        if let Some(probe) = self.metadata_probe.get() {
            probe.before_interest_native(&sources[0].record.key).await;
        }
        let machine = self.machine()?;
        let pins = self
            .controller_pin_source
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let registration = machine
            .current_session_registration_witness(&session)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("workflow.waiter_restore.registration", ());
        let attachment = machine
            .current_executor_attachment_witness(&session)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("workflow.waiter_restore.attachment", ());
        let actor = pins
            .live_actor(&session)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("workflow.waiter_restore.actor", ());
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
                    #[cfg(test)]
                    Self::synthetic_admission_diagnostic(
                        "workflow.waiter_restore.native_callback",
                        (),
                    );
                    let writer = self.owner.try_source_writer();
                    #[cfg(test)]
                    if writer.is_err()
                        && let Some(probe) = self.metadata_probe.get()
                    {
                        probe.writer_refused(&sources[0].record.key);
                    }
                    let _writer = writer.map_err(writer_error)?;
                    #[cfg(test)]
                    Self::synthetic_admission_diagnostic(
                        "workflow.waiter_restore.callback_writer",
                        (),
                    );
                    stamp
                        .check_current()
                        .map_err(|_| PrivateOriginalError::Unavailable)?;
                    #[cfg(test)]
                    Self::synthetic_admission_diagnostic(
                        "workflow.waiter_restore.callback_stamp",
                        (),
                    );
                    if let Some(operation) = &operation {
                        operation
                            .check_current()
                            .map_err(|_| PrivateOriginalError::Unavailable)?;
                    }
                    #[cfg(test)]
                    Self::synthetic_admission_diagnostic(
                        "workflow.waiter_restore.callback_operation",
                        (),
                    );
                    controller
                        .check_current()
                        .map_err(|_| PrivateOriginalError::Unavailable)?;
                    #[cfg(test)]
                    Self::synthetic_admission_diagnostic(
                        "workflow.waiter_restore.callback_controller",
                        (),
                    );
                    if admitted.input_id() != input.id() {
                        return Err(PrivateOriginalError::Conflict);
                    }
                    #[cfg(test)]
                    Self::synthetic_admission_diagnostic(
                        "workflow.waiter_restore.callback_checked",
                        (),
                    );
                    self.owner
                        .promote_historical_task(association, custody, &sources, admitted)
                        .map_err(writer_error)
                },
            )
            .await;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic(
            "workflow.waiter_restore.native_result",
            observed.as_ref().map(|result| result.as_ref().map(|_| ())),
        );
        let retained = match observed {
            Ok(result) => result?,
            Err(AdmittedOriginalRefusal::Conflict) => return Err(PrivateOriginalError::Conflict),
            Err(AdmittedOriginalRefusal::NotAdmitted | AdmittedOriginalRefusal::Unavailable) => {
                return Err(PrivateOriginalError::Unavailable);
            }
        };
        self.hold_binding(reference.clone(), retained).await
    }
}
