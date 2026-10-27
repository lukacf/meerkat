//! Exact source-owned batch selection. Native receives independent lineages;
//! currentness of every member is an initial-admission requirement only.
use super::*;
use crate::private_originals::service_approved_input::ApprovedTaskLink;
use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage, WorkAuthorityBasis};

struct BatchEntryFence {
    publication: meerkat_authorization::publication::LocalPublicationStamp,
    clock: Arc<dyn meerkat_authorization::clock::LocalAuthorizationClock>,
    expires_at_ms: u64,
    login: crate::private_host::ConsoleLoginHandle,
}

impl meerkat_mobkit::identity_first::GovernedEntryFence for BatchEntryFence {
    fn enter(&self) -> Result<Box<dyn meerkat_mobkit::identity_first::GovernedEntryPermit>,
        meerkat_mobkit::identity_first::GovernedEntryRefusal> {
        use meerkat_mobkit::identity_first::{GovernedEntryFence, GovernedEntryOutcome, GovernedEntryRefusal};
        let permit = GovernedEntryFence::enter(&self.login)?;
        // This exact stamp predates the final canonical/native reads, after
        // all binder writes. Entry performs no store/runtime lookup or await.
        // The native FirstAuthentication hook checks again at actual admission;
        // a later target revocation is checked independently at operation use.
        if !self.clock.now().is_ok_and(|now| now.unix_ms < self.expires_at_ms)
            || self.publication.check_current().is_err() {
            permit.finish(GovernedEntryOutcome::Unentered);
            return Err(GovernedEntryRefusal::Unavailable(
                "The selected public service set changed before admission".into()));
        }
        Ok(permit)
    }
}

impl PrivateOriginals {
    pub(in crate::private_originals) async fn batch_entry_fence(self: &Arc<Self>, source: &PrivateOriginalRef,
        task: &ApprovedTaskLink, original: &Arc<RetainedToolkitOriginal>,
        login: &crate::private_host::ConsoleLoginHandle,
    ) -> Result<Arc<dyn meerkat_mobkit::identity_first::GovernedEntryFence>, PrivateOriginalError> {
        // The caller has finished both binding and bound-input persistence.
        // Source commits and native grant changes share this publication. Never
        // replace this stamp after an awaited read with a newer observation.
        let (_, publication) = self.owner.publication().observe(|| ())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        self.service_input_host_current()?;
        self.service_batch_basis(source, task, true).await?;
        let route = self.route(&source.route)?;
        let mut expires_at_ms = task.before_ms;
        for reference in task.grants() {
            let stored = self.store.get(&self.access_key(reference)?).await.map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let record = self.decode_access(&stored, reference)?;
            if self.access_observation(reference.clone(), &record)?.state != PrivateServiceGrantState::GrantIssued {
                return Err(PrivateOriginalError::Unavailable);
            }
            let IssuanceObservation::Issued { lineage } = &record.issuance else {
                return Err(PrivateOriginalError::Unavailable);
            };
            let resolved = self.owner.grants().resolve_lineage(lineage,
                &route.logical_executor, route.represented_subject.as_ref())
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            resolved.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
            expires_at_ms = expires_at_ms.min(resolved.expires_at_ms()).min(
                record.decision.lifetime().expires_at_ms.ok_or(PrivateOriginalError::Denied)?);
        }
        publication.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
        self.owner.prepare_service_set_admission(original, publication.clone(), expires_at_ms)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(Arc::new(BatchEntryFence { publication, clock: self.owner.clock(),
            expires_at_ms, login: login.clone() }))
    }

    pub(in crate::private_originals) async fn service_batch_basis(
        &self, source: &PrivateOriginalRef, task: &ApprovedTaskLink, current: bool,
    ) -> Result<(WorkAuthorityBasis, Vec<RetainedServiceMcpAccess>), PrivateOriginalError> {
        let ApprovedTaskLink::Batch(batch) = task else { return Err(PrivateOriginalError::Denied); };
        task.validate()?;
        let stored = self.store.get(&self.key(source)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&stored, source)?;
        if record.approved_task.as_ref() != Some(task) || record.operation_cancelled {
            return Err(PrivateOriginalError::Denied);
        }
        let mut targets = Vec::with_capacity(batch.members.len());
        let mut selections = Vec::with_capacity(batch.members.len());
        let mut catalog = None;
        for reference in task.grants() {
            let stored = self.store.get(&self.access_key(reference)?).await.map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let access = self.decode_access(&stored, reference)?;
            if reference.route != source.route
                || !matches!(&access.decision.lifetime().scope,
                    LifetimeScope::Task { task } if task.as_str() == source.original.as_str())
                || access.caller != self.route(&source.route)?.grant_caller
                || access.application_policy.is_some()
            { return Err(PrivateOriginalError::Conflict); }
            if current && self.access_observation(reference.clone(), &access)?.state != PrivateServiceGrantState::GrantIssued {
                return Err(PrivateOriginalError::Unavailable);
            }
            let IssuanceObservation::Issued { lineage } = access.issuance else { return Err(PrivateOriginalError::Unavailable); };
            match (access.decision, access.mcp) {
                (AccessDecision::Catalog(decision), None) if reference == &batch.catalog => {
                    if decision.original != source.original || decision.session != task.original_task.origin_session_id {
                        return Err(PrivateOriginalError::Conflict);
                    }
                    match (&decision.kind, &batch.setup) {
                        (catalog::CatalogDecisionKind::PublicServiceCatalogV1, None) => {},
                        (catalog::CatalogDecisionKind::PublicServiceCatalogWithSetupV2 { helper }, Some(setup))
                            if helper == &setup.helper && decision.lifetime.expires_at_ms == Some(setup.expires_at_ms) => {},
                        _ => return Err(PrivateOriginalError::Conflict),
                    }
                    catalog = Some(lineage);
                }
                (AccessDecision::Mcp(decision), Some(selection)) => {
                    let member = batch.members.iter().find(|member| &member.grant == reference)
                        .ok_or(PrivateOriginalError::Conflict)?;
                    if member.connection != decision.permission.connection
                        || member.connection.as_str() != selection.server
                        || decision.catalog.is_some() || decision.live_attachment.is_none()
                    { return Err(PrivateOriginalError::Conflict); }
                    targets.push(TargetGrantLineage { target: GrantTarget::McpServer { server: selection.server.clone() }, lineage });
                    selections.push(RetainedServiceMcpAccess { decision, selection });
                }
                _ => return Err(PrivateOriginalError::Conflict),
            }
        }
        if targets.len() != batch.members.len() || catalog.is_none() { return Err(PrivateOriginalError::Conflict); }
        if let Some(setup) = &batch.setup {
            targets.extend(self.service_setup_targets(source, setup, current)?);
            targets.sort_by(|left, right| left.target.cmp(&right.target));
            if targets.windows(2).any(|pair| pair[0].target == pair[1].target)
                || targets.len() > meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES {
                return Err(PrivateOriginalError::Conflict);
            }
        }
        Ok((WorkAuthorityBasis::TargetedGrantLineages { base: catalog, targets }, selections))
    }

    pub(in crate::private_originals) async fn bind_batch_source(
        &self, source: &PrivateOriginalRef, task: &ApprovedTaskLink,
        association: meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
        custody: meerkat_runtime::meerkat_machine::AuthenticatedOriginal,
    ) -> Result<Arc<RetainedToolkitOriginal>, PrivateOriginalError> {
        let (basis, mcp) = self.service_batch_basis(source, task, true).await?;
        if association.candidate().authority_basis != basis
            || association.candidate().original_work.work != source.original {
            return Err(PrivateOriginalError::Conflict);
        }
        let route = self.route(&source.route)?;
        let setup_targets = task.setup().map(|setup| setup.binding_targets()).transpose()?;
        let selected_targets = match &setup_targets {
            None => crate::original_ingress::ServiceSetBindingTargets::PublicServicesV1,
            Some(targets) => crate::original_ingress::ServiceSetBindingTargets::WithConfiguredSetupV2(targets),
        };
        let retained = self.owner.bind_issued_service_set_async(association, custody,
            &route.requester, &route.ingress_actor, &route.grant_caller, selected_targets, |association| async move {
                // Native holds the existing source writer through this callback.
                // Targeted grants deliberately do not impose blanket liveness at
                // later use, so this source owner checks every initial member.
                let (current, _) = self.service_batch_basis(source, task, true).await
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                if association.candidate().authority_basis != current { return Err(OriginalWriterError::Conflict); }
                self.persist_or_observe_binding(source, &association, true).await
            }).await.map_err(|error| match error {
                OriginalWriterError::OperationCancelled => PrivateOriginalError::Denied,
                OriginalWriterError::Conflict => PrivateOriginalError::Conflict,
                _ => PrivateOriginalError::Unavailable,
            })?;
        self.hold_binding_with_mcp(source.clone(), retained.clone(), mcp).await?;
        Ok(retained)
    }
}
