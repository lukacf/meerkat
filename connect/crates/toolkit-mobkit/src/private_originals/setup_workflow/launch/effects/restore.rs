//! Recover only exact previously issued children. Native permission and expiry
//! remain per-operation checks; this path never invokes an issuer.
use super::*;

impl PreparedWaiterEffects {
    pub(in crate::private_originals::setup_workflow::launch) fn restore(
        source: &PrivateOriginals,
        planning: &InstalledWorkflowPlanning,
        record: &SourceRecord,
        original: &PrivateOriginalRef,
        membership: &SetupWorkflowWaiter,
        setup: &meerkat_toolkit::setup::SetupSnapshot,
    ) -> Result<Self, PrivateOriginalError> {
        if record.planning_source_evidence
            != Some(PlanningSourceEvidence::EffectWaiter {
                workflow: membership.workflow().clone(),
                planning: planning.submission.original.clone(),
            })
            || membership.source() != &source.key(original)?
            || setup.key() != membership.acquisition()
            || record.operation_cancelled
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let configured = record
            .configured_setup
            .as_ref()
            .ok_or(PrivateOriginalError::Conflict)?;
        if !configured.capability.helper_targets.is_empty()
            || configured.capability.helper.as_ref() != Some(&planning.terms.lead)
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let targets = &configured.capability.workflow_targets;
        crate::installed_host::operator_setup::setup_targets::validate_workflow_compiled(targets)
            .map_err(|_| PrivateOriginalError::Conflict)?;
        let association = record
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if setup.request().evidence().association() != association {
            return Err(PrivateOriginalError::Conflict);
        }
        let request = String::from_utf8(
            canonical_json(setup.request().request())
                .map_err(|_| PrivateOriginalError::Conflict)?,
        )
        .map_err(|_| PrivateOriginalError::Conflict)?;
        let request_digest = ContentDigest::of_bytes(request.as_bytes());
        if request_digest != membership.input().content_digest {
            return Err(PrivateOriginalError::Conflict);
        }
        let grants = source.owner.grants();
        let mut effects = Vec::new();
        for selected in targets {
            let target = selected
                .target
                .as_ref()
                .ok_or(PrivateOriginalError::Conflict)?;
            let parent = association
                .candidate()
                .authority_basis
                .lineage_for(Some(target))
                .ok_or(PrivateOriginalError::Conflict)?;
            let expected_a = crate::private_originals::setup_targets::child_id(
                &source
                    .configured_child_id(original)
                    .map_err(|_| PrivateOriginalError::Conflict)?,
                &selected.target,
            )
            .map_err(|_| PrivateOriginalError::Conflict)?;
            if parent.len() != 2 || parent[0] != selected.parent || parent[1].grant_id != expected_a
            {
                return Err(PrivateOriginalError::Conflict);
            }
            for retained in parent {
                if grants
                    .issued_reference(&retained.grant_id)
                    .map_err(|_| PrivateOriginalError::Unavailable)?
                    .as_ref()
                    != Some(retained)
                {
                    return Err(PrivateOriginalError::Unavailable);
                }
            }
            let id = child_id(&planning.submission.original, membership, original, target)?;
            let leaf = grants
                .issued_reference(&id)
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            if leaf.grant_id != id || leaf.root_authority != selected.parent.root_authority {
                return Err(PrivateOriginalError::Conflict);
            }
            let mut lineage = parent.to_vec();
            lineage.push(leaf);
            effects.push(PreparedEffect {
                target: target.clone(),
                lineage,
            });
        }
        Ok(Self {
            original: original.clone(),
            request,
            request_digest,
            effects,
        })
    }

    /// Prepare inert relationships before the native B/M/D callback. The
    /// callback publishes them with the exact interest and controller gate.
    pub(in crate::private_originals::setup_workflow::launch) fn stage_restored(
        &self,
        source: &Arc<PrivateOriginals>,
        workflow: &Arc<HeldSetupWorkflow>,
        membership: &SetupWorkflowWaiter,
        original: &Arc<RetainedToolkitOriginal>,
    ) -> Result<Vec<Arc<HeldWorkflowWaiter>>, PrivateOriginalError> {
        self.effects
            .iter()
            .map(|effect| {
                let authority = source
                    .owner
                    .stage_workflow_waiter_custody(
                        workflow.planning.clone(),
                        original.clone(),
                        Id::new(membership.acquisition().id.clone())
                            .map_err(|_| PrivateOriginalError::Conflict)?,
                        effect.target.clone(),
                        effect.lineage.clone(),
                        membership.deadline_ms(),
                    )
                    .map_err(custody_error)?;
                Ok(Arc::new(HeldWorkflowWaiter {
                    workflow: Arc::downgrade(workflow),
                    membership: membership.clone(),
                    original: self.original.clone(),
                    request: self.request.clone(),
                    request_digest: self.request_digest.clone(),
                    authority,
                }))
            })
            .collect()
    }
}
