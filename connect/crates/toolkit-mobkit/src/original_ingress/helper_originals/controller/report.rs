//! Setup operation intents are staged with the controller in the same row.
//! Each operation retains its own parent, action and qualified original scope.
use super::*;
use crate::installed_host::application_operations;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HelperReportIntent {
    id: EvidenceId,
    requested: ExecutionRestrictions,
}

#[derive(Clone, Copy)]
enum Operation { Report, Account, Public }

impl Operation {
    fn parent(self, capability: &crate::installed_host::acquisition::ConfiguredHelperReportCapability)
        -> Option<&[meerkat_authorization_contracts::grant::GrantLineageRef]>
    {
        match self {
            Self::Report => Some(&capability.parent),
            Self::Account => capability.account_parent.as_deref(),
            Self::Public => capability.public_parent.as_deref(),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Report => "toolkit.helper-report.v1",
            Self::Account => "toolkit.helper-account.v1",
            Self::Public => "toolkit.helper-public-service.v1",
        }
    }
    fn id_prefix(self) -> &'static str {
        match self {
            Self::Report => "helper-report", Self::Account => "helper-account",
            Self::Public => "helper-public-service",
        }
    }
}

impl HelperReportIntent {
    pub(super) fn for_target(admitted: &AdmittedHelperWork, target: &NativeWorkTarget)
        -> Result<Option<Self>, OriginalWriterError>
    { Self::for_operation(admitted, target, Operation::Report) }

    pub(super) fn for_account(admitted: &AdmittedHelperWork, target: &NativeWorkTarget)
        -> Result<Option<Self>, OriginalWriterError>
    { Self::for_operation(admitted, target, Operation::Account) }

    pub(super) fn for_public(admitted: &AdmittedHelperWork, target: &NativeWorkTarget)
        -> Result<Option<Self>, OriginalWriterError>
    { Self::for_operation(admitted, target, Operation::Public) }

    fn for_operation(admitted: &AdmittedHelperWork, target: &NativeWorkTarget, operation: Operation)
        -> Result<Option<Self>, OriginalWriterError>
    {
        let selected = admitted.selection();
        let Some(capability) = &selected.helper.report_capability else { return Ok(None); };
        if operation.parent(capability).is_none() { return Ok(None); }
        capability.validate(&selected.helper.operation_basis).map_err(|_| OriginalWriterError::Conflict)?;
        let _session = meerkat_core::SessionId::parse(target.context.as_str())
            .map_err(|_| OriginalWriterError::Conflict)?;
        let identity = canonical_json(&(operation.label(), admitted.realm(), admitted.original_work()?))
            .map_err(|_| OriginalWriterError::Conflict)?;
        // The native target remains in the complete staged controller intent.
        // Operation scope uses the already frozen qualified original coordinate.
        let requested = requested(&admitted.facts, operation)?;
        Ok(Some(Self {
            id: EvidenceId::new(format!("{}/{}", operation.id_prefix(), ContentDigest::of_bytes(&identity)))
                .map_err(|_| OriginalWriterError::Conflict)?, requested,
        }))
    }

    pub(super) fn native(&self, facts: &HelperAdmissionFacts)
        -> Result<ChildGrantIntent, OriginalWriterError>
    { self.native_operation(facts, Operation::Report) }

    pub(super) fn native_account(&self, facts: &HelperAdmissionFacts)
        -> Result<ChildGrantIntent, OriginalWriterError>
    { self.native_operation(facts, Operation::Account) }

    pub(super) fn native_public(&self, facts: &HelperAdmissionFacts)
        -> Result<ChildGrantIntent, OriginalWriterError>
    { self.native_operation(facts, Operation::Public) }

    fn native_operation(&self, facts: &HelperAdmissionFacts, operation: Operation)
        -> Result<ChildGrantIntent, OriginalWriterError>
    {
        let helper = &facts.selection.helper;
        let capability = helper.report_capability.as_ref().ok_or(OriginalWriterError::Conflict)?;
        if self.requested != requested(facts, operation)? { return Err(OriginalWriterError::Conflict); }
        Ok(ChildGrantIntent {
            id: self.id.clone(), issuer: facts.selection.grant_caller.clone(),
            parent_lineage: operation.parent(capability).ok_or(OriginalWriterError::Conflict)?.to_vec(),
            grantee: helper.logical_executor.clone(),
            represented_subject: helper.represented_subject.clone(), requested: self.requested.clone(),
        })
    }

    pub(super) fn check_current(&self, owner: &ToolkitOriginalOwner, facts: &HelperAdmissionFacts,
        basis: &WorkAuthorityBasis) -> Result<(), OriginalWriterError>
    { self.check_operation_current(owner, facts, basis, Operation::Report) }

    pub(super) fn check_account_current(&self, owner: &ToolkitOriginalOwner, facts: &HelperAdmissionFacts,
        basis: &WorkAuthorityBasis) -> Result<(), OriginalWriterError>
    { self.check_operation_current(owner, facts, basis, Operation::Account) }

    pub(super) fn check_public_current(&self, owner: &ToolkitOriginalOwner, facts: &HelperAdmissionFacts,
        basis: &WorkAuthorityBasis) -> Result<(), OriginalWriterError>
    { self.check_operation_current(owner, facts, basis, Operation::Public) }

    fn check_operation_current(&self, owner: &ToolkitOriginalOwner, facts: &HelperAdmissionFacts,
        basis: &WorkAuthorityBasis, operation: Operation) -> Result<(), OriginalWriterError>
    {
        let intent = self.native_operation(facts, operation)?;
        let lineage = selected_lineage(basis, operation)?;
        if lineage.len() != intent.parent_lineage.len() + 1
            || !lineage.starts_with(&intent.parent_lineage)
            || lineage.last().map(|leaf| &leaf.grant_id) != Some(&self.id)
        { return Err(OriginalWriterError::Conflict); }
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

fn requested(facts: &HelperAdmissionFacts, operation: Operation) -> Result<ExecutionRestrictions, OriginalWriterError> {
    let operation = match operation {
        Operation::Report => application_operations::setup_report_for_origin,
        Operation::Account => application_operations::setup_account_request_for_origin,
        Operation::Public => application_operations::setup_public_service_request_for_origin,
    };
    let mut requested = application_operations::parent_restrictions(operation(
        &facts.selection.grant_caller, &facts.selection.helper.logical_executor,
        &facts.selection.member.audience, &facts.origin,
    ).map_err(|_| OriginalWriterError::Conflict)?);
    requested.delegation_depth = DelegationDepth::remaining(0);
    Ok(requested)
}

pub(super) fn check_record_basis(report: Option<&HelperReportIntent>, account: Option<&HelperReportIntent>,
    public: Option<&HelperReportIntent>, facts: &HelperAdmissionFacts,
    operation_basis: &WorkAuthorityBasis, basis: &WorkAuthorityBasis)
    -> Result<(), OriginalWriterError>
{
    let helper = &facts.selection.helper;
    match (report, &helper.report_capability) {
        (None, None) if account.is_none() && public.is_none() && basis == operation_basis => Ok(()),
        (Some(report), Some(capability)) => {
            // Structural reconstruction validates the frozen requested tuple
            // without resolving every target's current grant.
            report.native(facts)?;
            let lineage = selected_lineage(basis, Operation::Report)?;
            let optional_lineage = |intent: Option<&HelperReportIntent>, operation: Operation| {
                match (intent, operation.parent(capability)) {
                    (None, None) => Ok(None),
                    (Some(intent), Some(_)) => {
                        intent.native_operation(facts, operation)?;
                        let lineage = selected_lineage(basis, operation)?;
                        if lineage.last().map(|leaf| &leaf.grant_id) != Some(&intent.id) {
                            return Err(OriginalWriterError::Conflict);
                        }
                        Ok(Some(lineage.to_vec()))
                    },
                    _ => Err(OriginalWriterError::Conflict),
                }
            };
            let account_lineage = optional_lineage(account, Operation::Account)?;
            let public_lineage = optional_lineage(public, Operation::Public)?;
            if lineage.last().map(|leaf| &leaf.grant_id) != Some(&report.id)
                || capability.project_with_operations(operation_basis, lineage.to_vec(), account_lineage, public_lineage)
                    .map_err(|_| OriginalWriterError::Conflict)? != *basis
            { return Err(OriginalWriterError::Conflict); }
            Ok(())
        }
        _ => Err(OriginalWriterError::Conflict),
    }
}

fn selected_lineage(basis: &WorkAuthorityBasis, operation: Operation)
    -> Result<&[meerkat_authorization_contracts::grant::GrantLineageRef], OriginalWriterError>
{
    use crate::installed_host::acquisition::ConfiguredHelperReportCapability as Capability;
    match operation {
        Operation::Report => Capability::selected_lineage(basis),
        Operation::Account => Capability::selected_account_lineage(basis),
        Operation::Public => Capability::selected_public_lineage(basis),
    }.map_err(|_| OriginalWriterError::Conflict)
}
