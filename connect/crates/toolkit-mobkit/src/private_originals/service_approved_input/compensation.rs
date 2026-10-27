//! Joined approval failure reconciliation. The existing intent records and
//! native grant authority remain the only durable owners. Abrupt future drop
//! and process loss still require retained-intent recovery; Drop is not revoke.
use super::*;
use crate::installed_host::operator_setup::ServiceMcpParentCompensation;

impl InstalledServiceTask {
    pub(super) async fn compensate_unpublished_approval(&self,
        cleanup: ServiceMcpParentCompensation, grant: PrivateServiceGrantRef,
    ) -> Result<(), PrivateOriginalError> {
        if !cleanup.belongs_to(&self.source.owner) { return Err(PrivateOriginalError::Denied); }
        let source = self.source.clone();
        source.owner.reconcile_followup_source(&self.parent_association, || async {
            let stored = source.store.get(&source.key(&self.parent)?).await.map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let parent_source = source.decode_record(&stored, &self.parent)?;
            if parent_source.final_association.as_ref() != Some(&self.parent_association) {
                return Err(PrivateOriginalError::Conflict);
            }
            let key = StoreKey { realm: source.scope.realm.clone(), owner: source.scope.owner.clone(),
                id: source_key_id(&parent_source.source.realm, &parent_source.source.ingress,
                    &parent_source.source.source_authority, &self.fresh)? };
            let published = source.store.get(&key).await.map_err(store_error)?;
            let parent = cleanup.issued().await.map_err(application::policy_error)?;
            let Some(parent) = parent else { return Ok(()); };
            if let Some(published) = published {
                let record = source.decode_stored(&published)?;
                if record.source != parent_source.source || record.event.original != self.fresh {
                    return Err(PrivateOriginalError::Conflict);
                }
                let link = record.approved_task.as_ref().ok_or(PrivateOriginalError::Conflict)?;
                if link.parent != self.parent || link.acquisition != self.acquisition
                    || link.original_task != self.task {
                    return Err(PrivateOriginalError::Conflict);
                }
                // Preserve a successful exact replay or a lost source-commit
                // acknowledgement. A different approval can name the same task
                // but its parent does not acquire this published child's owner.
                if link.contains(&grant) && source.service_compensation_child(&grant, &parent).await?.is_some() {
                    return Ok(());
                }
            }
            // Revoke the parent first. Native serializes this with issue_child:
            // once acknowledged, no late worker can issue another child under
            // it. Then read the retained child intent and actual native owner.
            let machine = source.machine()?;
            let parent_result = match machine.try_controller_grant_mutation() {
                Ok(mut custody) => source.owner.grants().revoke(cleanup.issuer(), &parent, &mut custody)
                    .map_err(|_| PrivateOriginalError::Unavailable),
                Err(_) => Err(PrivateOriginalError::Unavailable),
            };
            let child_result = match source.service_compensation_child(&grant, &parent).await {
                Ok(Some((issuer, child))) => match machine.try_controller_grant_mutation() {
                    Ok(mut custody) => source.owner.grants().revoke(&issuer, &child, &mut custody)
                        .map_err(|_| PrivateOriginalError::Unavailable),
                    Err(_) => Err(PrivateOriginalError::Unavailable),
                },
                Ok(None) => Ok(()),
                Err(error) => Err(error),
            };
            parent_result.and(child_result)
        }).await.map_err(|_| PrivateOriginalError::Unavailable)?
    }
}

impl PrivateOriginals {
    /// Reconcile only the exact rejected batch's effects. The canonical source
    /// writer excludes first binding, and each child must belong to its actual
    /// retained parent. Native journal records are the durable cleanup result.
    pub(super) async fn compensate_rejected_batch(&self, reference: &PrivateOriginalRef)
        -> Result<(), PrivateOriginalError> {
        let _writer = self.owner.try_source_writer().map_err(|_| PrivateOriginalError::Unavailable)?;
        let saved = self.store.get(&self.key(reference)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&saved, reference)?;
        let Some(task @ ApprovedTaskLink::Batch(_)) = &record.approved_task else { return Ok(()); };
        if !record.operation_cancelled { return Err(PrivateOriginalError::Denied); }
        // An admitted source belongs to the existing continuation/cancellation
        // owner. Its selected grants are not pre-entry compensation work.
        if record.service_admission.is_some() { return Ok(()); }
        // Binding without settled admission is uncertain, never proven nonentry.
        if record.final_association.is_some() || record.bound_service_input.is_some() { return Err(PrivateOriginalError::Unavailable); }
        let lifetime = self.host_lifetime.get().and_then(Weak::upgrade).ok_or(PrivateOriginalError::Unavailable)?;
        let operator = self.service_operator_setup(lifetime).map_err(application::policy_error)?;
        let machine = self.machine()?;
        // Canonical pre-bind setup intent is already stored. Only observed
        // exact task children are revoked, never configured shared parents.
        let mut failed = match self.prepare_service_setup_cleanup(reference, &saved).await {
            Ok(cleanup) => cleanup.revoke(self).err(),
            Err(error) => Some(error),
        };
        for grant in task.grants() {
            let result = async {
                let access = self.store.get(&self.access_key(grant)?).await.map_err(store_error)?
                    .ok_or(PrivateOriginalError::Unavailable)?;
                let access = self.decode_access(&access, grant)?;
                if access.parent.len() != 1 { return Err(PrivateOriginalError::Conflict); }
                let parent = &access.parent[0];
                let key = StoreKey { realm: self.scope.realm.clone(), owner: self.scope.owner.clone(),
                    id: parent.grant_id.as_str().to_owned() };
                let saved = self.store.get(&key).await.map_err(store_error)?.ok_or(PrivateOriginalError::Unavailable)?;
                let approved = operator.retained_service_approval(&saved).map_err(application::policy_error)?
                    .ok_or(PrivateOriginalError::Conflict)?;
                if approved.context.fresh_original != reference.original || approved.context.business_task != task.acquisition
                    || approved.context.prior_original.work != task.parent.original
                    || !approved.cleanup.belongs_to(&self.owner)
                    || approved.cleanup.issued().await.map_err(application::policy_error)?.as_ref() != Some(parent) {
                    return Err(PrivateOriginalError::Conflict);
                }
                let selected = match (&approved.decision, &approved.selection) {
                    (super::super::service_grants::AccessDecision::Mcp(decision), Some(selection)) =>
                        self.retained_service_selection(decision, selection, &approved.requested)?,
                    (super::super::service_grants::AccessDecision::Catalog(decision), None) =>
                        self.retained_catalog_selection(decision, &approved.requested)?,
                    _ => return Err(PrivateOriginalError::Conflict),
                };
                if &selected.reference != grant { return Err(PrivateOriginalError::Conflict); }
                let child = self.service_compensation_child(grant, parent).await?
                    .ok_or(PrivateOriginalError::Unavailable)?;
                let parent_result = {
                    let mut custody = machine.try_controller_grant_mutation().map_err(|_| PrivateOriginalError::Unavailable)?;
                    self.owner.grants().revoke(approved.cleanup.issuer(), parent, &mut custody)
                        .map_err(|_| PrivateOriginalError::Unavailable)
                };
                let child_result = {
                    let mut custody = machine.try_controller_grant_mutation().map_err(|_| PrivateOriginalError::Unavailable)?;
                    self.owner.grants().revoke(&child.0, &child.1, &mut custody)
                        .map_err(|_| PrivateOriginalError::Unavailable)
                };
                parent_result.and(child_result)
            }.await;
            if let Err(error) = result { failed = Some(error); }
        }
        failed.map_or(Ok(()), Err)
    }
}
