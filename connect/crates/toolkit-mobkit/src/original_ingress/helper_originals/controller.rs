//! The pending controller intent is the first phase of the existing helper
//! source row. It authenticates no original and supplies no admission custody.
use super::*;
mod report;
mod catalog;
use meerkat_authorization::grants::ChildGrantIntent;
use meerkat_authorization_contracts::constraints::{DelegationDepth, LifetimeBound};

/// The remaining native intent fields live in the same immutable admission
/// facts: issuer, independent parent, grantee and represented subject. Neither
/// a retry's model selection nor a new attempt can replace these fields.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HelperControllerIntent {
    id: EvidenceId,
    target: NativeWorkTarget,
    controller_model: meerkat_core::ControllerModelSelection,
    requested: ExecutionRestrictions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    report: Option<report::HelperReportIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<report::HelperReportIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public: Option<report::HelperReportIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    catalog: Option<catalog::HelperCatalogIntent>,
}

impl HelperControllerIntent {
    pub(crate) fn new(
        admitted: &AdmittedHelperWork,
        target: NativeWorkTarget,
        controller_model: meerkat_core::ControllerModelSelection,
        requested: ExecutionRestrictions,
    ) -> Result<Self, OriginalWriterError> {
        let report = report::HelperReportIntent::for_target(admitted, &target)?;
        let account = report::HelperReportIntent::for_account(admitted, &target)?;
        let public = report::HelperReportIntent::for_public(admitted, &target)?;
        let catalog = catalog::HelperCatalogIntent::for_target(admitted, &target)?;
        let value = Self {
            id: child_id(admitted.realm(), &admitted.original_work()?)?,
            target, controller_model, requested, report, account, public, catalog,
        };
        value.validate(admitted)?;
        Ok(value)
    }

    fn validate(&self, admitted: &AdmittedHelperWork) -> Result<(), OriginalWriterError> {
        let member = &admitted.selection().member;
        if self.id != child_id(admitted.realm(), &admitted.original_work()?)?
            || self.target.logical_owner != member.logical_owner
            || self.target.context_generation != member.context_generation
            || self.target.audience != member.audience
            || self.requested.delegation_depth != DelegationDepth::remaining(0)
            || !matches!(self.requested.lifetime.bound(), LifetimeBound::Unrestricted)
        { return Err(OriginalWriterError::Conflict); }
        if self.report != report::HelperReportIntent::for_target(admitted, &self.target)?
            || self.account != report::HelperReportIntent::for_account(admitted, &self.target)?
            || self.public != report::HelperReportIntent::for_public(admitted, &self.target)? {
            return Err(OriginalWriterError::Conflict);
        }
        if self.catalog != catalog::HelperCatalogIntent::for_target(admitted, &self.target)? {
            return Err(OriginalWriterError::Conflict);
        }
        Ok(())
    }

    fn native(&self, facts: &HelperAdmissionFacts) -> ChildGrantIntent {
        ChildGrantIntent {
            id: self.id.clone(), issuer: facts.selection.grant_caller.clone(),
            parent_lineage: facts.selection.helper.controller_parent.clone(),
            grantee: facts.selection.helper.logical_executor.clone(),
            represented_subject: facts.selection.helper.represented_subject.clone(),
            requested: self.requested.clone(),
        }
    }

    pub(super) fn check_record_binding(
        &self, facts: &HelperAdmissionFacts, binding: &HelperNativeBinding,
    ) -> Result<(), OriginalWriterError> {
        let intent = self.native(facts);
        if binding.target != self.target || binding.controller_model != self.controller_model
            || binding.controller_grant_lineage.len() != intent.parent_lineage.len() + 1
            || !binding.controller_grant_lineage.starts_with(&intent.parent_lineage)
            || binding.controller_grant_lineage.last().map(|leaf| &leaf.grant_id) != Some(&intent.id)
        { return Err(OriginalWriterError::Conflict); }
        let operation_basis = catalog::operation_basis(self.catalog.as_ref(), facts, &binding.target, &binding.authority_basis)?;
        report::check_record_basis(self.report.as_ref(), self.account.as_ref(), self.public.as_ref(), facts,
            &operation_basis, &binding.authority_basis)?;
        Ok(())
    }

    pub(super) fn check_retained_binding(
        &self, admitted: &AdmittedHelperWork, binding: &HelperNativeBinding,
    ) -> Result<(), OriginalWriterError> {
        self.validate(admitted)?;
        self.check_record_binding(&admitted.facts, binding)
    }

    pub(super) fn check_binding(
        &self, owner: &ToolkitOriginalOwner, admitted: &AdmittedHelperWork,
        binding: &HelperNativeBinding,
    ) -> Result<(), OriginalWriterError> {
        self.check_retained_binding(admitted, binding)?;
        let intent = self.native(&admitted.facts);
        let parent = owner.grants.resolve_controller_lineage(
            &intent.parent_lineage, &intent.issuer, intent.represented_subject.as_ref(),
        ).map_err(|_| OriginalWriterError::OperationDenied)?;
        let expected = parent.restrictions().for_child(&intent.requested)
            .map_err(|_| OriginalWriterError::OperationDenied)?;
        let child = owner.grants.resolve_controller_lineage(
            &binding.controller_grant_lineage, &intent.grantee, intent.represented_subject.as_ref(),
        ).map_err(|_| OriginalWriterError::OperationDenied)?;
        if child.restrictions() != &expected || binding.controller_ceiling != expected {
            return Err(OriginalWriterError::Conflict);
        }
        if let Some(report) = &self.report {
            report.check_current(owner, &admitted.facts, &binding.authority_basis)?;
        }
        if let Some(account) = &self.account {
            account.check_account_current(owner, &admitted.facts, &binding.authority_basis)?;
        }
        if let Some(public) = &self.public {
            public.check_public_current(owner, &admitted.facts, &binding.authority_basis)?;
        }
        if let Some(catalog) = &self.catalog {
            catalog.check_current(owner, &admitted.facts, &binding.authority_basis)?;
        }
        parent.check_current().map_err(|_| OriginalWriterError::Unavailable)?;
        child.check_current().map_err(|_| OriginalWriterError::Unavailable)?;
        Ok(())
    }
}

fn child_id(realm: &str, original: &OriginalWorkRef) -> Result<EvidenceId, OriginalWriterError> {
    let identity = canonical_json(&("toolkit.helper-controller.v1", realm, original))
        .map_err(|_| OriginalWriterError::Conflict)?;
    EvidenceId::new(format!("helper-controller/{}", ContentDigest::of_bytes(&identity)))
        .map_err(|_| OriginalWriterError::Conflict)
}

impl ToolkitOriginalOwner {
    /// Retain/recover the full exact intent before any native issuance. Only a
    /// known settled read/admit returns it. The writer guard ends here, before
    /// the native operation takes its own publication reservation.
    #[cfg(test)]
    pub(crate) async fn retain_helper_controller_intent(
        self: &Arc<Self>, store: &dyn ToolkitStore, admitted: &AdmittedHelperWork,
        controller: HelperControllerIntent,
    ) -> Result<ChildGrantIntent, OriginalWriterError> {
        self.retain_helper_grant_intents(store, admitted, controller).await.map(|value| value.0)
    }

    #[cfg(test)]
    pub(crate) async fn retain_helper_grant_intents(
        self: &Arc<Self>, store: &dyn ToolkitStore, admitted: &AdmittedHelperWork,
        controller: HelperControllerIntent,
    ) -> Result<(ChildGrantIntent, Option<ChildGrantIntent>), OriginalWriterError> {
        self.retain_helper_operation_intents(store, admitted, controller).await
            .map(|(controller, report, _, _, _)| (controller, report))
    }

    pub(crate) async fn retain_helper_operation_intents(
        self: &Arc<Self>, store: &dyn ToolkitStore, admitted: &AdmittedHelperWork,
        controller: HelperControllerIntent,
    ) -> Result<(ChildGrantIntent, Option<ChildGrantIntent>, Option<ChildGrantIntent>, Option<ChildGrantIntent>, Option<ChildGrantIntent>), OriginalWriterError> {
        self.check_canonical_source(store, &admitted.source_scope)?;
        store.capabilities().require_durable_contract()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let _mutation = self.mutation.try_lock().map_err(|_| OriginalWriterError::Unavailable)?;
        self.check_admitted_helper(admitted)?;
        controller.validate(admitted)?;
        let key = admitted.source_key()?;
        let candidate = HelperSourceRecord { custody_attempt: None, bound_input: None, facts: admitted.facts.clone(), controller,
            state: HelperSourceState::ControllerPending };
        let saved = match store.get(&key).await.map_err(store_error)? {
            Some(saved) => saved,
            None => store.admit(AdmissionRequest {
                key: key.clone(), admission_key: source_admission_key(
                    &admitted.facts.realm, &admitted.facts.namespace, &admitted.facts.admission.key,
                )?, data: candidate.encode()?,
                events: vec![], effects: vec![],
            }).await.map_err(store_error)?.record().clone(),
        };
        let retained = HelperSourceRecord::decode(&saved)?;
        if saved.key != key || retained.facts != candidate.facts
            || retained.controller != candidate.controller
        { return Err(OriginalWriterError::Conflict); }
        // Cancellation may occur during storage. A retained pending intent is
        // kept for reconciliation, but it is never permission to issue a child.
        self.check_admitted_helper(admitted)?;
        let report = retained.controller.report.as_ref()
            .map(|value| value.native(&retained.facts)).transpose()?;
        let account = retained.controller.account.as_ref()
            .map(|value| value.native_account(&retained.facts)).transpose()?;
        let public = retained.controller.public.as_ref()
            .map(|value| value.native_public(&retained.facts)).transpose()?;
        let catalog = retained.controller.catalog.as_ref()
            .map(|value| value.native(&retained.facts)).transpose()?;
        Ok((retained.controller.native(&retained.facts), report, account, public, catalog))
    }
}

pub(super) fn store_error(error: toolkit_store::StoreError) -> OriginalWriterError {
    match error {
        toolkit_store::StoreError::AdmissionConflict | toolkit_store::StoreError::Conflict
        | toolkit_store::StoreError::AlreadyExists => OriginalWriterError::Conflict,
        _ => OriginalWriterError::Unavailable,
    }
}

/// Existing policy/storage fixtures issue through the real grant owner, but
/// deliberately omit a live dispatch. They do not prove native effect entry.
#[cfg(test)]
pub(super) async fn stage_fixture_controller(
    owner: &Arc<ToolkitOriginalOwner>, store: &dyn ToolkitStore,
    admitted: &AdmittedHelperWork, mut binding: HelperNativeBinding,
) -> HelperNativeBinding {
    let mut requested = binding.controller_ceiling.clone();
    requested.delegation_depth = DelegationDepth::remaining(0);
    let candidate = HelperControllerIntent::new(admitted, binding.target.clone(),
        binding.controller_model.clone(), requested).unwrap();
    let intent = owner.retain_helper_controller_intent(store, admitted, candidate).await.unwrap();
    let leaf = match owner.grants.issued_reference(&intent.id).unwrap() {
        Some(leaf) => leaf,
        None => owner.grants.issue_child(&intent.issuer, intent.parent_lineage.last().unwrap(),
            intent.id, intent.grantee.clone(), intent.requested).unwrap(),
    };
    binding.controller_grant_lineage = intent.parent_lineage;
    binding.controller_grant_lineage.push(leaf);
    binding.controller_ceiling = owner.grants.resolve_controller_lineage(
        &binding.controller_grant_lineage, &intent.grantee, intent.represented_subject.as_ref(),
    ).unwrap().restrictions().clone();
    binding
}

#[cfg(test)]
mod tests;
