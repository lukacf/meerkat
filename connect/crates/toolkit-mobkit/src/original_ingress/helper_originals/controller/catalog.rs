//! Catalog intent retained with the existing controller/source transaction.
use super::*;
use crate::installed_host::application_operations;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HelperCatalogIntent {
    id: EvidenceId,
    requested: ExecutionRestrictions,
}

impl HelperCatalogIntent {
    pub(super) fn for_target(admitted: &AdmittedHelperWork, target: &NativeWorkTarget)
        -> Result<Option<Self>, OriginalWriterError>
    {
        let selected = admitted.selection();
        let Some(capability) = &selected.helper.catalog_capability else { return Ok(None); };
        capability.validate(&selected.helper.operation_basis).map_err(|_| OriginalWriterError::Conflict)?;
        let identity = canonical_json(&("toolkit.helper-catalog.v1", admitted.realm(), admitted.original_work()?))
            .map_err(|_| OriginalWriterError::Conflict)?;
        Ok(Some(Self {
            id: EvidenceId::new(format!("helper-catalog/{}", ContentDigest::of_bytes(&identity)))
                .map_err(|_| OriginalWriterError::Conflict)?, requested: requested(&admitted.facts, target)?,
        }))
    }

    pub(super) fn native(&self, facts: &HelperAdmissionFacts) -> Result<ChildGrantIntent, OriginalWriterError> {
        let helper = &facts.selection.helper;
        let capability = helper.catalog_capability.as_ref().ok_or(OriginalWriterError::Conflict)?;
        Ok(ChildGrantIntent {
            id: self.id.clone(), issuer: facts.selection.grant_caller.clone(),
            parent_lineage: capability.parent.clone(), grantee: helper.logical_executor.clone(),
            represented_subject: helper.represented_subject.clone(), requested: self.requested.clone(),
        })
    }

    pub(super) fn check_current(&self, owner: &ToolkitOriginalOwner, facts: &HelperAdmissionFacts,
        basis: &WorkAuthorityBasis) -> Result<(), OriginalWriterError>
    {
        let intent = self.native(facts)?;
        let lineage = basis.lineage_for(None).ok_or(OriginalWriterError::Conflict)?;
        let parent = owner.grants.resolve_controller_lineage(&intent.parent_lineage, &intent.issuer,
            intent.represented_subject.as_ref()).map_err(|_| OriginalWriterError::OperationDenied)?;
        let expected = parent.restrictions().for_child(&intent.requested)
            .map_err(|_| OriginalWriterError::OperationDenied)?;
        let child = owner.grants.resolve_controller_lineage(lineage, &intent.grantee,
            intent.represented_subject.as_ref()).map_err(|_| OriginalWriterError::OperationDenied)?;
        if child.restrictions() != &expected { return Err(OriginalWriterError::Conflict); }
        parent.check_current().map_err(|_| OriginalWriterError::Unavailable)?;
        child.check_current().map_err(|_| OriginalWriterError::Unavailable)?;
        Ok(())
    }
}

pub(super) fn operation_basis(intent: Option<&HelperCatalogIntent>, facts: &HelperAdmissionFacts,
    target: &NativeWorkTarget, bound: &WorkAuthorityBasis) -> Result<WorkAuthorityBasis, OriginalWriterError>
{
    let helper = &facts.selection.helper;
    match (intent, &helper.catalog_capability) {
        (None, None) => Ok(helper.operation_basis.clone()),
        (Some(intent), Some(capability)) => {
            let lineage = bound.lineage_for(None).ok_or(OriginalWriterError::Conflict)?;
            if intent.requested != requested(facts, target)?
                || lineage.last().map(|leaf| &leaf.grant_id) != Some(&intent.id) {
                return Err(OriginalWriterError::Conflict);
            }
            capability.project(&helper.operation_basis, lineage.to_vec()).map_err(|_| OriginalWriterError::Conflict)
        }
        _ => Err(OriginalWriterError::Conflict),
    }
}

fn requested(facts: &HelperAdmissionFacts, target: &NativeWorkTarget)
    -> Result<ExecutionRestrictions, OriginalWriterError>
{
    let session = meerkat_core::SessionId::parse(target.context.as_str())
        .map_err(|_| OriginalWriterError::Conflict)?;
    let selected = &facts.selection;
    let mut requested = application_operations::parent_restrictions(application_operations::catalog_read(
        &selected.grant_caller, &selected.helper.logical_executor, &selected.member.audience, &session,
    ));
    requested.delegation_depth = DelegationDepth::remaining(0);
    Ok(requested)
}
