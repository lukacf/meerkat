//! Reattach a canonical launch/delivery interest to actual restored A/P/G.
//! No request body leaves its owner and no Find, input or effect grant is made.
use super::*;
use crate::private_originals::{
    cold_service_admission::writer_error, planning_source_evidence::PlanningSourceEvidence,
};
use meerkat_runtime::meerkat_machine::AdmittedOriginalRefusal;

#[cfg(test)]
tokio::task_local! {
    // Same-crate control only: pause after the real canonical reads and before
    // the native observation, with the source writer already released.
    pub(super) static AFTER_MEMBERSHIP_READ: Arc<(tokio::sync::Notify, tokio::sync::Notify)>;
}

impl PrivateOriginals {
    /// Restore the already-admitted G attachment without manufacturing a
    /// waiter. An empty attachment keeps unowned MCP/public calls refused.
    /// The existing worker supplies the retained future; all native and grant
    /// owners are reobserved before publishing the attachment and controller.
    pub(in crate::private_originals) async fn restore_workflow_planning_source(
        self: &Arc<Self>,
        reference: &SetupWorkflowRef,
        lead: &PrivateOriginalRef,
        member: &MobMemberBinding,
    ) -> Result<Arc<InstalledWorkflowPlanning>, PrivateOriginalError> {
        self.restore_workflow_planning_observed(reference, lead, member)
            .await
            .map(|(planning, _)| planning)
    }

    /// Keep the exact native admission observation with the restored planning
    /// owner. This does not recover a pending binder or submit a new G input.
    pub(super) async fn restore_workflow_planning_observed(
        self: &Arc<Self>,
        reference: &SetupWorkflowRef,
        lead: &PrivateOriginalRef,
        member: &MobMemberBinding,
    ) -> Result<(Arc<InstalledWorkflowPlanning>, WorkflowLaunchObservation), PrivateOriginalError>
    {
        self.service_input_host_current()?;
        let writer = self.owner.source_writer().await;
        let publication = self.owner.publication();
        let (_, stamp) = publication
            .observe(|| ())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let (_, record, _, parent_record, snapshot, parent, operation) =
            self.workflow_lead_evidence(reference, lead, member).await?;
        let (p, g) = {
            let held = self
                .retained
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            (
                held.iter()
                    .find(|held| held.reference == parent)
                    .cloned()
                    .ok_or(PrivateOriginalError::Unavailable)?,
                held.iter()
                    .find(|held| &held.reference == lead)
                    .cloned()
                    .ok_or(PrivateOriginalError::Unavailable)?,
            )
        };
        if record.final_association.as_ref() != Some(g.original.association())
            || parent_record.final_association.as_ref() != Some(p.original.association())
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let input = record
            .bound_service_input
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .decode()?;
        let Input::Prompt(prompt) = &input else {
            return Err(PrivateOriginalError::Conflict);
        };
        let meerkat_core::ContentInput::Text(content) = &prompt.content else {
            return Err(PrivateOriginalError::Conflict);
        };
        let (lineage, mcp_lineage) = crate::installed_host::operator_setup::workflow_mcp::split(
            &g.original.association().candidate().authority_basis,
        )
        .map_err(|_| PrivateOriginalError::Conflict)?;
        let helper = if let Some(provenance) = &record.late_workflow_helper {
            let admitted = self
                .owner
                .captured_workflow_class(reference, g.original.association())
                .map_err(writer_error)?;
            self.check_member_class_owner(&admitted)
                .map_err(|_| PrivateOriginalError::Denied)?;
            let (current, _) = admitted
                .owner
                .member_route(&lead.route)
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let helper = InstalledLateWorkflowHelper {
                source: Arc::downgrade(self),
                admitted,
                current_resource: current.workflow_resource.clone(),
                admission: provenance.admission.clone(),
                server: provenance.server.clone(),
                envelope: operation.restrictions().clone(),
                lineage: lineage.to_vec(),
                mcp_lineage: mcp_lineage.map(<[GrantLineageRef]>::to_vec),
                public_lineage:
                    crate::installed_host::operator_setup::workflow_mcp::public_lineage(
                        &g.original.association().candidate().authority_basis,
                    )
                    .map_err(|_| PrivateOriginalError::Conflict)?
                    .map(<[GrantLineageRef]>::to_vec),
            };
            helper
                .check_selection(self)
                .map_err(|_| PrivateOriginalError::Denied)?;
            Some(helper)
        } else {
            None
        };
        let mut authorities = Vec::new();
        for held in [&p, &g] {
            if !self
                .owner
                .has_admitted_source(held.original.association())
                .map_err(writer_error)?
            {
                return Err(PrivateOriginalError::Unavailable);
            }
            let claims = held.original.association().candidate();
            let lineages = if Arc::ptr_eq(held, &g) {
                crate::installed_host::operator_setup::workflow_mcp::planning_lineages(
                    &claims.authority_basis,
                )
                .map_err(|_| PrivateOriginalError::Conflict)?
            } else {
                claims.authority_basis.grant_lineages().collect()
            };
            for lineage in lineages {
                authorities.push(
                    self.owner
                        .grants()
                        .resolve_lineage(
                            lineage,
                            &claims.logical_executor,
                            claims.represented_subject.as_ref(),
                        )
                        .map_err(grant_error)?,
                );
            }
            authorities.push(
                self.owner
                    .grants()
                    .resolve_controller_lineage(
                        &claims.controller_grant_lineage,
                        &claims.logical_executor,
                        claims.represented_subject.as_ref(),
                    )
                    .map_err(grant_error)?,
            );
        }
        let route = self
            .owner
            .workflow_controller(&g.original)
            .map_err(writer_error)?
            .plain_facts()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .route_key();
        let session = meerkat_core::SessionId::parse(
            g.original.association().candidate().target.context.as_str(),
        )
        .map_err(|_| PrivateOriginalError::Conflict)?;
        let candidate = g.setup_workflow.get().cloned().unwrap_or_else(|| {
            Arc::new(HeldSetupWorkflow {
                source: Arc::downgrade(self),
                planning: g.original.clone(),
                reference: reference.clone(),
                member: member.clone(),
                session: session.clone(),
                controller_route: route.clone(),
                accepted: AtomicBool::new(true),
                deadline_ms: snapshot.terms().deadline_ms,
                waiters: Mutex::new(Vec::new()),
            })
        });
        stamp
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        drop(writer);
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
        let attachment = machine
            .current_executor_attachment_witness(&session)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let actor = pins
            .live_actor(&session)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
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
                    if let Some(helper) = &helper {
                        helper
                            .check_selection(self)
                            .map_err(|_| PrivateOriginalError::Denied)?;
                    }
                    for authority in &authorities {
                        authority
                            .check_current()
                            .map_err(|_| PrivateOriginalError::Unavailable)?;
                    }
                    for held in [&p, &g] {
                        if !self
                            .owner
                            .has_admitted_source(held.original.association())
                            .map_err(writer_error)?
                        {
                            return Err(PrivateOriginalError::Unavailable);
                        }
                    }
                    if self
                        .owner
                        .clock()
                        .now()
                        .map_err(|_| PrivateOriginalError::Unavailable)?
                        .unix_ms
                        >= snapshot.terms().deadline_ms
                    {
                        return Err(PrivateOriginalError::Denied);
                    }
                    if admitted.input_id() != input.id() {
                        return Err(PrivateOriginalError::Conflict);
                    }
                    self.owner
                        .complete_restored_workflow_member(
                            reference,
                            member,
                            g.original.association(),
                            admitted,
                            &route,
                            |apply| {
                                if !candidate
                                    .source
                                    .upgrade()
                                    .is_some_and(|source| Arc::ptr_eq(&source, self))
                                    || candidate.reference != *reference
                                    || candidate.member != *member
                                    || candidate.session != session
                                    || !Arc::ptr_eq(&candidate.planning, &g.original)
                                    || candidate.controller_route != route
                                    || candidate.deadline_ms != snapshot.terms().deadline_ms
                                    || !candidate.accepted.load(Ordering::Acquire)
                                    || g.setup_workflow
                                        .get()
                                        .is_some_and(|prior| !Arc::ptr_eq(prior, &candidate))
                                {
                                    return Err(OriginalWriterError::Conflict);
                                }
                                let changed = g.setup_workflow.get().is_none();
                                if apply {
                                    g.setup_workflow.get_or_init(|| candidate.clone());
                                }
                                Ok(changed)
                            },
                        )
                        .map_err(writer_error)?;
                    Ok::<_, PrivateOriginalError>(WorkflowLaunchObservation::HistoricalAdmitted {
                        session_id: admitted.session_id().to_string(),
                        input_id: admitted.input_id().to_string(),
                    })
                },
            )
            .await;
        let observed = match observed {
            Ok(result) => result?,
            Err(AdmittedOriginalRefusal::Conflict) => return Err(PrivateOriginalError::Conflict),
            Err(AdmittedOriginalRefusal::NotAdmitted | AdmittedOriginalRefusal::Unavailable) => {
                return Err(PrivateOriginalError::Unavailable);
            }
        };
        let planning = self
            .prepare_workflow_planning_selected(
                &parent,
                PrivateOriginalSubmit {
                    original: lead.clone(),
                    content: content.clone(),
                    transport_authentication: record.event.original_authentication,
                },
                snapshot.terms().clone(),
                lineage.to_vec(),
                helper,
            )
            .await?;
        Ok((planning, observed))
    }

    pub(in crate::private_originals) async fn restore_workflow_interest(
        self: &Arc<Self>,
        reference: &SetupWorkflowRef,
        lead: &PrivateOriginalRef,
        member: &MobMemberBinding,
        waiter: &PrivateOriginalRef,
        membership: &SetupWorkflowWaiter,
    ) -> Result<Arc<InstalledWorkflowPlanning>, PrivateOriginalError> {
        self.service_input_host_current()?;
        if membership.workflow() != reference || membership.source() != &self.key(waiter)? {
            return Err(PrivateOriginalError::Denied);
        }
        let (a, g) = {
            let held = self
                .retained
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            (
                held.iter()
                    .find(|held| &held.reference == waiter)
                    .cloned()
                    .ok_or(PrivateOriginalError::Unavailable)?,
                held.iter()
                    .find(|held| &held.reference == lead)
                    .cloned()
                    .ok_or(PrivateOriginalError::Unavailable)?,
            )
        };
        // These are exact selected source records, not a reconstructed user
        // request. G's retained native Input supplies its already-admitted body.
        let g_row = self
            .store
            .get(&self.key(lead)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let g_record = self.decode_record(&g_row, lead)?;
        let Some(PlanningSourceEvidence::Lead { parent }) = &g_record.planning_source_evidence
        else {
            return Err(PrivateOriginalError::Unavailable);
        };
        let p = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| &held.reference == parent)
            .cloned()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let input = g_record
            .bound_service_input
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .decode()?;
        let Input::Prompt(prompt) = &input else {
            return Err(PrivateOriginalError::Conflict);
        };
        let meerkat_core::ContentInput::Text(content) = &prompt.content else {
            return Err(PrivateOriginalError::Conflict);
        };
        if g_record.final_association.as_ref() != Some(g.original.association())
            || input.header().authority_association.as_ref() != Some(g.original.association())
            || g_record.operation_cancelled
        {
            return Err(PrivateOriginalError::Denied);
        }
        let snapshot = self
            .workflow_store()?
            .load_setup_workflow(reference)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if &snapshot.terms().lead != member {
            return Err(PrivateOriginalError::Conflict);
        }
        let (lineage, _) = crate::installed_host::operator_setup::workflow_mcp::split(
            &g.original.association().candidate().authority_basis,
        )
        .map_err(|_| PrivateOriginalError::Conflict)?;
        let planning = if g_record.late_workflow_helper.is_some() {
            self.restore_workflow_planning_source(reference, lead, member)
                .await?
        } else {
            self.prepare_workflow_planning(
                parent,
                PrivateOriginalSubmit {
                    original: lead.clone(),
                    content: content.clone(),
                    // Retained historical source evidence, not a new transport login.
                    // This method never binds or submits this reconstructed holder.
                    transport_authentication: g_record.event.original_authentication.clone(),
                },
                snapshot.terms().clone(),
                lineage.to_vec(),
            )
            .await?
        };
        if !Arc::ptr_eq(&planning.parent, &p.original) {
            return Err(PrivateOriginalError::Conflict);
        }

        // Normal departure and cancellation use this same writer. Read canonical
        // membership/phase under it, then use its publication stamp across the
        // native await. A winning departure leaves a tombstone in this interned
        // planning owner even when the local interest was not published yet.
        let writer = self.owner.source_writer().await;
        let publication = self.owner.publication();
        let (_, stamp) = publication
            .observe(|| ())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let current_g = self
            .store
            .get(&g_row.key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        if current_g != g_row {
            return Err(PrivateOriginalError::Conflict);
        }
        let p_row = self
            .store
            .get(&self.key(parent)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let p_record = self.decode_record(&p_row, parent)?;
        if !crate::private_originals::planning_source_evidence::is_parent(
            p_record.planning_source_evidence.as_ref(),
        ) || p_record.final_association.as_ref() != Some(p.original.association())
            || p_record.bound_service_input.is_none()
            || p_record.operation_cancelled
        {
            return Err(PrivateOriginalError::Denied);
        }
        self.check_planning_evidence_source(&p_record, parent)?;
        let a_row = self
            .store
            .get(&self.key(waiter)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let a_record = self.decode_record(&a_row, waiter)?;
        let shared = match &a_record.planning_source_evidence {
            Some(PlanningSourceEvidence::Waiter { workflow, planning })
                if workflow == reference && planning == lead =>
            {
                false
            }
            Some(PlanningSourceEvidence::EffectWaiter { workflow, planning })
                if workflow == reference && planning == lead =>
            {
                true
            }
            _ => return Err(PrivateOriginalError::Denied),
        };
        if a_record.final_association.as_ref() != Some(a.original.association())
            || a_record.bound_service_input.is_none()
            || a_record.operation_cancelled
        {
            return Err(PrivateOriginalError::Denied);
        }
        self.check_workflow_waiter_binding(&a_record, waiter)
            .await?;
        let (current, setup) = self
            .workflow_store()?
            .workflow_waiter_recovery_setup(membership, a.original.association())
            .await
            .map_err(|error| match error {
                meerkat_toolkit::setup::SetupError::Denied => PrivateOriginalError::Denied,
                _ => PrivateOriginalError::Unavailable,
            })?;
        if current.terms() != &planning.terms {
            return Err(PrivateOriginalError::Conflict);
        }
        let correlation = setup.request().host().correlation.clone();
        let effects = if shared {
            Some(effects::PreparedWaiterEffects::restore(
                self, &planning, &a_record, waiter, membership, &setup,
            )?)
        } else {
            None
        };
        let now = self
            .owner
            .clock()
            .now()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .unix_ms;
        if now >= membership.deadline_ms() || now >= planning.terms.deadline_ms {
            return Err(PrivateOriginalError::Denied);
        }
        // Resolve outside the native callback. Under B/M/D only atomic checks
        // and nonwaiting source/attachment publication are permitted.
        let mut authorities = Vec::new();
        for held in [&a, &p, &g] {
            if !self
                .owner
                .has_admitted_source(held.original.association())
                .map_err(writer_error)?
            {
                return Err(PrivateOriginalError::Unavailable);
            }
            let claims = held.original.association().candidate();
            // Shared A's exact historical targeted basis was validated above.
            // A revoked leaf must not become an all-target recovery gate.
            if !(shared && Arc::ptr_eq(held, &a)) {
                let lineages = if Arc::ptr_eq(held, &g) {
                    crate::installed_host::operator_setup::workflow_mcp::planning_lineages(
                        &claims.authority_basis,
                    )
                    .map_err(|_| PrivateOriginalError::Conflict)?
                } else {
                    claims.authority_basis.grant_lineages().collect()
                };
                for lineage in lineages {
                    authorities.push(
                        self.owner
                            .grants()
                            .resolve_lineage(
                                lineage,
                                &claims.logical_executor,
                                claims.represented_subject.as_ref(),
                            )
                            .map_err(grant_error)?,
                    );
                }
            }
            authorities.push(
                self.owner
                    .grants()
                    .resolve_controller_lineage(
                        &claims.controller_grant_lineage,
                        &claims.logical_executor,
                        claims.represented_subject.as_ref(),
                    )
                    .map_err(grant_error)?,
            );
        }
        let controller = self
            .owner
            .workflow_controller(&g.original)
            .map_err(writer_error)?;
        let route = controller
            .plain_facts()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .route_key();
        let session = meerkat_core::SessionId::parse(
            g.original.association().candidate().target.context.as_str(),
        )
        .map_err(|_| PrivateOriginalError::Conflict)?;
        let candidate = g.setup_workflow.get().cloned().unwrap_or_else(|| {
            Arc::new(HeldSetupWorkflow {
                source: Arc::downgrade(self),
                planning: g.original.clone(),
                reference: reference.clone(),
                member: member.clone(),
                session: session.clone(),
                controller_route: route.clone(),
                accepted: AtomicBool::new(true),
                deadline_ms: planning.terms.deadline_ms,
                waiters: Mutex::new(Vec::new()),
            })
        });
        let staged = match &effects {
            Some(effects) => effects.stage_restored(self, &candidate, membership, &a.original)?,
            None => Vec::new(),
        };
        stamp
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        drop(writer);
        #[cfg(test)]
        if let Ok(pause) = AFTER_MEMBERSHIP_READ.try_with(Arc::clone) {
            pause.0.notify_one();
            pause.1.notified().await;
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
        let attachment = machine
            .current_executor_attachment_witness(&session)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let actor = pins
            .live_actor(&session)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
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
                    for authority in &authorities {
                        authority
                            .check_current()
                            .map_err(|_| PrivateOriginalError::Unavailable)?;
                    }
                    for held in [&a, &p, &g] {
                        if !self
                            .owner
                            .has_admitted_source(held.original.association())
                            .map_err(writer_error)?
                        {
                            return Err(PrivateOriginalError::Unavailable);
                        }
                    }
                    let now = self
                        .owner
                        .clock()
                        .now()
                        .map_err(|_| PrivateOriginalError::Unavailable)?
                        .unix_ms;
                    if now >= membership.deadline_ms() || now >= planning.terms.deadline_ms {
                        return Err(PrivateOriginalError::Denied);
                    }
                    if admitted.input_id() != input.id() {
                        return Err(PrivateOriginalError::Conflict);
                    }
                    self.owner
                        .complete_restored_workflow_member(
                            reference,
                            member,
                            g.original.association(),
                            admitted,
                            &route,
                            |apply| {
                                let mut interests = planning
                                    .interests
                                    .try_lock()
                                    .map_err(|_| OriginalWriterError::Unavailable)?;
                                let mut selections = a
                                    .workflow_selections
                                    .try_lock()
                                    .map_err(|_| OriginalWriterError::Unavailable)?;
                                let prior_interest = interests.iter().find(|interest| {
                                    interest.membership.acquisition() == membership.acquisition()
                                });
                                if let Some(prior) = prior_interest {
                                    if prior.membership != *membership
                                        || prior.closed
                                        || !prior
                                            .original
                                            .as_ref()
                                            .is_some_and(|held| Arc::ptr_eq(held, &a.original))
                                        || prior.effects != effects
                                    {
                                        return Err(OriginalWriterError::Conflict);
                                    }
                                } else if interests.len() >= usize::from(planning.terms.max_waiters)
                                {
                                    return Err(OriginalWriterError::Unavailable);
                                }
                                let prior_selection = selections
                                    .iter()
                                    .find(|selected| selected.correlation == correlation);
                                if let Some(prior) = prior_selection {
                                    if !same_optional(prior.selection.as_ref(), Some(&planning)) {
                                        return Err(OriginalWriterError::Conflict);
                                    }
                                    // An existing host-owned transfer selection stays owned.
                                } else if selections.len() >= 128 {
                                    return Err(OriginalWriterError::Unavailable);
                                }
                                if let Some(prior) = g.setup_workflow.get()
                                    && (!Arc::ptr_eq(prior, &candidate)
                                        || prior.reference != *reference
                                        || prior.member != *member
                                        || prior.session != session
                                        || !Arc::ptr_eq(&prior.planning, &g.original)
                                        || prior.controller_route != route
                                        || !prior.accepted.load(Ordering::Acquire))
                                {
                                    return Err(OriginalWriterError::Conflict);
                                }
                                let mut attached = candidate
                                    .waiters
                                    .try_lock()
                                    .map_err(|_| OriginalWriterError::Unavailable)?;
                                let mut missing = Vec::new();
                                for waiter in &staged {
                                    if let Some(prior) = attached.iter().find(|prior| {
                                        prior.membership == waiter.membership
                                            && prior.authority.target() == waiter.authority.target()
                                    }) {
                                        if prior.original != waiter.original
                                            || prior.request != waiter.request
                                            || prior.request_digest != waiter.request_digest
                                            || !prior.authority.same_selection(&waiter.authority)
                                        {
                                            return Err(OriginalWriterError::Conflict);
                                        }
                                    } else {
                                        missing.push(waiter.clone());
                                    }
                                }
                                // No fallible step follows these owner mutations. The
                                // controller gate is still closed until the outer commit.
                                let add_interest = prior_interest.is_none();
                                let add_selection = prior_selection.is_none();
                                let changed = g.setup_workflow.get().is_none()
                                    || add_interest
                                    || add_selection
                                    || !missing.is_empty();
                                if !apply {
                                    return Ok(changed);
                                }
                                g.setup_workflow.get_or_init(|| candidate.clone());
                                attached.extend(missing);
                                if add_interest {
                                    interests.push(LaunchInterest {
                                        membership: membership.clone(),
                                        original: Some(a.original.clone()),
                                        closed: false,
                                        entries: Vec::new(),
                                        effects: effects.clone(),
                                    });
                                }
                                if add_selection {
                                    selections.push(InvocationPlanning {
                                        correlation: correlation.clone(),
                                        selection: Some(planning.clone()),
                                        context_transfer: None,
                                    });
                                }
                                Ok(changed)
                            },
                        )
                        .map_err(writer_error)
                },
            )
            .await;
        match observed {
            Ok(result) => result?,
            Err(AdmittedOriginalRefusal::Conflict) => return Err(PrivateOriginalError::Conflict),
            Err(AdmittedOriginalRefusal::NotAdmitted | AdmittedOriginalRefusal::Unavailable) => {
                return Err(PrivateOriginalError::Unavailable);
            }
        }
        Ok(planning)
    }
}

// A native read that cannot establish current authority stays unavailable;
// actual grant refusals remain denied. Restoration does not retry either one.
pub(super) fn grant_error(
    error: meerkat_authorization::grants::GrantRefusal,
) -> PrivateOriginalError {
    match error {
        meerkat_authorization::grants::GrantRefusal::Unavailable => {
            PrivateOriginalError::Unavailable
        }
        _ => PrivateOriginalError::Denied,
    }
}
