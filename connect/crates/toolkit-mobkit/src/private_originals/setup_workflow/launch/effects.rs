//! Fresh shared-effect attenuation on the existing configured/source owner.
//! Canonical membership is correlation; native A authority and exact source
//! custody remain necessary before issuance, staging and every operation.
use super::*;
use crate::private_originals::planning_source_evidence::PlanningSourceEvidence;
use meerkat_authorization_contracts::constraints::LifetimeRestriction;

mod restore;

#[derive(Clone, PartialEq, Eq)]
struct PreparedEffect {
    target: GrantTarget,
    lineage: Vec<GrantLineageRef>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct PreparedWaiterEffects {
    original: PrivateOriginalRef,
    request: String,
    request_digest: ContentDigest,
    effects: Vec<PreparedEffect>,
}

pub(super) fn child_id(
    planning: &PrivateOriginalRef,
    membership: &SetupWorkflowWaiter,
    original: &PrivateOriginalRef,
    target: &GrantTarget,
) -> Result<EvidenceId, PrivateOriginalError> {
    let bytes = canonical_json(&(
        "toolkit.shared-effect-child.v1",
        planning,
        membership,
        original,
        target,
    ))
    .map_err(|_| PrivateOriginalError::Conflict)?;
    EvidenceId::new(format!("shared-effect/{}", digest(&bytes)))
        .map_err(|_| PrivateOriginalError::Conflict)
}

impl InstalledWorkflowPlanning {
    /// Called only after conditional canonical join, before launch or context
    /// disclosure. Existing Find-only interests remain untouched. All native
    /// issuance/reads finish here, never in the accepted callback or binder.
    pub(crate) async fn prepare_waiter_effects(
        self: &Arc<Self>,
        membership: &SetupWorkflowWaiter,
        original: &crate::private_originals::acquisition::AcquisitionOriginal,
        access: SetupAccess<'_>,
    ) -> Result<(), PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        self.check_for_binding(&source, &self.submission.original)?;
        let lifetime = source
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        source
            .owner
            .retain_followup_source(original.original.association(), || async {
                let row = source
                    .store
                    .get(
                        &source
                            .key(&original.reference)
                            .map_err(|_| OriginalWriterError::WrongOwner)?,
                    )
                    .await
                    .map_err(|_| OriginalWriterError::Unavailable)?
                    .ok_or(OriginalWriterError::Unavailable)?;
                let record = source
                    .decode_record(&row, &original.reference)
                    .map_err(|_| OriginalWriterError::Conflict)?;
                let Some(PlanningSourceEvidence::EffectWaiter { workflow, planning }) =
                    &record.planning_source_evidence
                else {
                    return Ok(());
                };
                if workflow != membership.workflow()
                    || planning != &self.submission.original
                    || record.operation_cancelled
                    || record.final_association.as_ref() != Some(original.original.association())
                    || row.key != *membership.source()
                {
                    return Err(OriginalWriterError::Conflict);
                }
                let configured = record
                    .configured_setup
                    .as_ref()
                    .ok_or(OriginalWriterError::Conflict)?;
                if !configured.capability.helper_targets.is_empty()
                    || configured.capability.helper.as_ref() != Some(&self.terms.lead)
                {
                    return Err(OriginalWriterError::WrongOwner);
                }
                let targets = &configured.capability.workflow_targets;
                crate::installed_host::operator_setup::setup_targets::validate_workflow_compiled(
                    targets,
                )
                .map_err(|_| OriginalWriterError::Conflict)?;
                let setup = source
                    .workflow_store()
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                let current = setup
                    .load_setup_workflow(workflow)
                    .await
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                if current.terms() != &self.terms {
                    return Err(OriginalWriterError::Conflict);
                }
                require_active(&current, membership)?;
                let acquisition = setup
                    .load(
                        &Id::new(membership.acquisition().id.clone())
                            .map_err(|_| OriginalWriterError::Conflict)?,
                        access.evidence,
                        access.context,
                        access.references,
                    )
                    .await
                    .map_err(|_| OriginalWriterError::OperationDenied)?;
                if acquisition.key() != membership.acquisition()
                    || acquisition.request().evidence().association()
                        != original.original.association()
                    || setup
                        .workflow_waiter(&self.terms, &acquisition, row.key.clone())
                        .map_err(|_| OriginalWriterError::Conflict)?
                        != *membership
                    || !matches!(
                        acquisition.state().lifecycle_phase,
                        toolkit_catalog::AcquisitionPhase::Investigating
                            | toolkit_catalog::AcquisitionPhase::Preparing
                    )
                {
                    return Err(OriginalWriterError::Conflict);
                }
                let request = String::from_utf8(
                    canonical_json(acquisition.request().request())
                        .map_err(|_| OriginalWriterError::Conflict)?,
                )
                .map_err(|_| OriginalWriterError::Conflict)?;
                let request_digest = ContentDigest::of_bytes(request.as_bytes());
                if request_digest != membership.input().content_digest {
                    return Err(OriginalWriterError::Conflict);
                }
                let g = source
                    .route(&self.submission.original.route)
                    .map_err(|_| OriginalWriterError::WrongOwner)?;
                let a = original.original.association().candidate();
                let origin = source
                    .setup_origin_for(&original.reference)
                    .map_err(|_| OriginalWriterError::WrongOwner)?;
                let grants = source.owner.grants();
                // The async reads above do not retain host entry. Recheck and
                // hold the existing close fence only for synchronous native
                // issuance. Exact partial-issue cleanup does not require Open.
                let issue = || -> Result<Vec<PreparedEffect>, OriginalWriterError> {
                    let mut effects = Vec::new();
                    for selected in targets {
                        let target = selected
                            .target
                            .as_ref()
                            .ok_or(OriginalWriterError::Conflict)?;
                        let parent = a
                            .authority_basis
                            .lineage_for(Some(target))
                            .ok_or(OriginalWriterError::OperationDenied)?;
                        let expected_leaf = crate::private_originals::setup_targets::child_id(
                            &source
                                .configured_child_id(&original.reference)
                                .map_err(|_| OriginalWriterError::Conflict)?,
                            &selected.target,
                        )
                        .map_err(|_| OriginalWriterError::Conflict)?;
                        if parent.len() != 2
                            || parent[0] != selected.parent
                            || parent[1].grant_id != expected_leaf
                        {
                            return Err(OriginalWriterError::WrongOwner);
                        }
                        let resolved = grants
                            .resolve_lineage(
                                parent,
                                &a.logical_executor,
                                a.represented_subject.as_ref(),
                            )
                            .map_err(OriginalWriterError::Grant)?;
                        let mut expected =
                            crate::installed_host::operator_setup::setup_targets::child_bounds(
                                &g.grant_caller,
                                &origin,
                                &selected.target,
                                &selected.restrictions,
                            )
                            .map_err(|_| OriginalWriterError::Conflict)?;
                        expected.delegation_depth = DelegationDepth::remaining(1);
                        if resolved.restrictions() != &expected {
                            return Err(OriginalWriterError::OperationDenied);
                        }
                        resolved
                            .check_current()
                            .map_err(|_| OriginalWriterError::Unavailable)?;
                        let mut requested = expected;
                        requested.delegation_depth = DelegationDepth::remaining(0);
                        requested.lifetime =
                            requested.lifetime.conjoin(&LifetimeRestriction::window(
                                0,
                                membership.deadline_ms().min(self.terms.deadline_ms),
                            ));
                        let expected = resolved
                            .restrictions()
                            .for_child(&requested)
                            .map_err(|_| OriginalWriterError::OperationDenied)?;
                        let id = child_id(
                            &self.submission.original,
                            membership,
                            &original.reference,
                            target,
                        )
                        .map_err(|_| OriginalWriterError::Conflict)?;
                        let leaf = match grants
                            .issued_reference(&id)
                            .map_err(OriginalWriterError::Grant)?
                        {
                            Some(leaf) => leaf,
                            None => grants
                                .issue_child(
                                    &a.logical_executor,
                                    parent.last().ok_or(OriginalWriterError::Conflict)?,
                                    id,
                                    g.logical_executor.clone(),
                                    requested,
                                )
                                .map_err(OriginalWriterError::Grant)?,
                        };
                        let mut lineage = parent.to_vec();
                        lineage.push(leaf);
                        let grant = grants
                            .resolve_lineage(
                                &lineage,
                                &g.logical_executor,
                                a.represented_subject.as_ref(),
                            )
                            .map_err(OriginalWriterError::Grant)?;
                        if grant.restrictions() != &expected {
                            return Err(OriginalWriterError::OperationDenied);
                        }
                        grant
                            .check_current()
                            .map_err(|_| OriginalWriterError::Unavailable)?;
                        effects.push(PreparedEffect {
                            target: target.clone(),
                            lineage,
                        });
                    }
                    Ok(effects)
                };
                let effects = lifetime
                    .with_current_setup(|| Ok(issue()))
                    .map_err(|_| OriginalWriterError::Unavailable)??;
                let mut interests = self
                    .interests
                    .lock()
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                let interest = interests
                    .iter_mut()
                    .find(|interest| {
                        interest.membership == *membership
                            && interest
                                .original
                                .as_ref()
                                .is_some_and(|held| Arc::ptr_eq(held, &original.original))
                    })
                    .ok_or(OriginalWriterError::WrongOwner)?;
                if interest.closed {
                    return Err(OriginalWriterError::OperationCancelled);
                }
                if let Some(prior) = &interest.effects
                    && (prior.original != original.reference
                        || prior.request != request
                        || prior.request_digest != request_digest
                        || prior.effects.len() != effects.len()
                        || prior
                            .effects
                            .iter()
                            .zip(&effects)
                            .any(|(a, b)| a.target != b.target || a.lineage != b.lineage))
                {
                    return Err(OriginalWriterError::Conflict);
                }
                interest.effects = Some(PreparedWaiterEffects {
                    original: original.reference.clone(),
                    request,
                    request_digest,
                    effects,
                });
                drop(interests);
                // G may already be admitted when B joins. Attach before B's context
                // is disclosed, using the same source writer as cancellation.
                let attached = source
                    .retained
                    .lock()
                    .map_err(|_| OriginalWriterError::Unavailable)?
                    .iter()
                    .find(|held| held.reference == self.submission.original)
                    .and_then(|held| held.setup_workflow.get().cloned());
                if let Some(attached) = attached {
                    self.stage_effects(&source, &attached)
                        .map_err(|_| OriginalWriterError::Unavailable)?;
                }
                Ok(())
            })
            .await
            .map_err(custody_error)
    }

    /// The actual G association is now bound. This synchronously stages only
    /// precomputed relationships. G may use its own bound Open input before
    /// promotion; named children and retained resumes still require admission.
    pub(super) fn stage_effects(
        &self,
        source: &Arc<PrivateOriginals>,
        workflow: &Arc<HeldSetupWorkflow>,
    ) -> Result<(), PrivateOriginalError> {
        let interests = self
            .interests
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let mut waiters = workflow
            .waiters
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        for interest in interests.iter().filter(|interest| !interest.closed) {
            let Some(prepared) = &interest.effects else {
                continue;
            };
            let original = interest
                .original
                .as_ref()
                .ok_or(PrivateOriginalError::Conflict)?;
            for effect in &prepared.effects {
                let authority = source
                    .owner
                    .stage_workflow_waiter_custody(
                        workflow.planning.clone(),
                        original.clone(),
                        Id::new(interest.membership.acquisition().id.clone())
                            .map_err(|_| PrivateOriginalError::Conflict)?,
                        effect.target.clone(),
                        effect.lineage.clone(),
                        interest.membership.deadline_ms(),
                    )
                    .map_err(custody_error)?;
                if let Some(prior) = waiters.iter().find(|waiter| {
                    waiter.membership == interest.membership
                        && waiter.authority.target() == &effect.target
                }) {
                    if prior.original != prepared.original
                        || prior.request != prepared.request
                        || !prior.authority.same_selection(&authority)
                    {
                        return Err(PrivateOriginalError::Conflict);
                    }
                    continue;
                }
                let publication = source.owner.publication();
                // Departure takes publication before interests. Do not wait
                // for its writer while retaining these local relationship locks.
                let _change = publication
                    .try_begin_owner_change()
                    .map_err(|_| PrivateOriginalError::Unavailable)?;
                waiters.push(Arc::new(HeldWorkflowWaiter {
                    workflow: Arc::downgrade(workflow),
                    membership: interest.membership.clone(),
                    original: prepared.original.clone(),
                    request: prepared.request.clone(),
                    request_digest: prepared.request_digest.clone(),
                    authority,
                }));
            }
        }
        Ok(())
    }
}

impl InstalledWorkflowPlanning {
    /// Called by the existing departure owner after its launch interest closes
    /// under the source writer. Even partial/unknown issuance is recovered from
    /// the same canonical target selection and exact child IDs, never replaced.
    pub(super) async fn revoke_waiter_effects(
        &self,
        source: &PrivateOriginals,
        membership: &SetupWorkflowWaiter,
    ) -> Result<(), PrivateOriginalError> {
        let attached = source
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| held.reference == self.submission.original)
            .and_then(|held| held.setup_workflow.get().cloned());
        if let Some(attached) = attached {
            let waiters = attached
                .waiters
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            source
                .owner
                .retire_workflow_effects_under_writer(
                    waiters
                        .iter()
                        .filter(|waiter| waiter.membership == *membership)
                        .map(|waiter| waiter.authority.clone())
                        .collect(),
                )
                .map_err(custody_error)?;
        }
        let row = source
            .store
            .get(membership.source())
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = source.decode_stored(&row)?;
        let Some(PlanningSourceEvidence::EffectWaiter { workflow, planning }) =
            &record.planning_source_evidence
        else {
            return Ok(());
        };
        if workflow != membership.workflow() || planning != &self.submission.original {
            return Err(PrivateOriginalError::Conflict);
        }
        let original = PrivateOriginalRef {
            route: record.event.route.clone(),
            original: record.event.original.clone(),
            event_sha256: digest(
                &canonical_json(&record.event).map_err(|_| PrivateOriginalError::Conflict)?,
            ),
        };
        source.decode_record(&row, &original)?;
        let selected = record
            .configured_setup
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        crate::installed_host::operator_setup::setup_targets::validate_workflow_compiled(
            &selected.capability.workflow_targets,
        )
        .map_err(|_| PrivateOriginalError::Conflict)?;
        let a = record
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .candidate();
        let grants = source.owner.grants();
        let machine = source.machine()?;
        let mut custody = machine
            .try_controller_grant_mutation()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        for target in &selected.capability.workflow_targets {
            let target_kind = target
                .target
                .as_ref()
                .ok_or(PrivateOriginalError::Conflict)?;
            let id = child_id(
                &self.submission.original,
                membership,
                &original,
                target_kind,
            )?;
            let Some(leaf) = grants
                .issued_reference(&id)
                .map_err(|_| PrivateOriginalError::Unavailable)?
            else {
                continue;
            };
            if leaf.root_authority != target.parent.root_authority || leaf.grant_id != id {
                return Err(PrivateOriginalError::Conflict);
            }
            grants
                .revoke(&a.logical_executor, &leaf, &mut custody)
                .map_err(|_| PrivateOriginalError::Unavailable)?;
        }
        Ok(())
    }
}
