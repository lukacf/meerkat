//! Canonical request projections from the existing acquisition owner.
//! No input cache, extra journal, native permission or continuation is created.

use super::*;
use crate::authority::Action;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use toolkit_package::Revision;

/// Existing immutable request terms, without its research content. The installed
/// host uses these on exact replay so wall-clock time cannot silently renew them.
pub struct SetupInvocationTerms {
    pub host: HostSetupRequest,
    pub evidence: SetupRequestEvidence,
}

/// Address of the immutable full request inside its canonical acquisition row.
/// Producing this descriptor grants no access and proves no row was committed.
/// The preparation owner checks it against its authenticated recovered snapshot.
pub fn canonical_setup_input_reference(
    request: &ValidatedSetupRequest,
) -> Result<ContextReference, SetupError> {
    Ok(ContextReference {
        owner: Id::new(format!("toolkit/acquisition/{}", request.constraints().deployment))
            .map_err(|_| SetupError::InvalidRecord)?,
        id: operation_id(request)?,
        revision: Revision::new(format!("input/{}", request.input_digest()))
            .map_err(|_| SetupError::InvalidRecord)?,
        content_digest: ContentDigest::of_bytes(&canonical_json(request.request())?),
    })
}

impl SetupStore<'_> {
    /// Recover only this exact retained native invocation's canonical terms.
    /// The caller supplies its current native context, not stored permission.
    /// A changed policy, association or target cannot disclose or renew old terms.
    pub async fn invocation_terms(
        &self,
        association: &InputAuthorityAssociation,
        correlation: &Id,
        context: &SetupUseContext<'_>,
    ) -> Result<Option<SetupInvocationTerms>, SetupError> {
        if context.tenant != &self.scope.tenant
            || context.deployment != &self.scope.deployment
            || context.realm != &association.candidate().ingress_namespace.realm
            || context.target != &association.candidate().target
        {
            return Err(SetupError::WrongScope);
        }
        // Screen even an absence lookup against this invocation's exact
        // historical native source. This is identity matching, not present
        // permission. The installed owner guards every eventual read outcome.
        // Native retained-work identity uses this JSON codec, not its binary codec.
        let digest = ContentDigest::of_bytes(&serde_json::to_vec(association)?);
        if !context.authorization.retained_work().is_some_and(|work| {
            work.contributors().iter().any(|source| source.association_digest == digest.as_str())
        }) {
            return Err(SetupError::Denied);
        }
        let digest = crate::request::invocation_correlation_digest(
            association, &self.scope.tenant, &self.scope.deployment, correlation,
        )?;
        let operation = Id::new(format!("setup-{digest}"))
            .map_err(|_| SetupError::InvalidRecord)?;
        let Some(stored) = self.store.get(&self.scope.key(&operation)).await? else {
            return Ok(None);
        };
        let snapshot = self.recover(stored)?;
        let request = snapshot.request();
        if request.evidence().association() != association
            || &request.host().correlation != correlation
        {
            return Err(SetupError::Denied);
        }
        request.evidence().check_proposal(Action::FindAndInstall, &request.host().target, context)
            .map_err(RequestError::from)?;
        Ok(Some(SetupInvocationTerms {
            host: request.host().clone(),
            evidence: request.evidence().clone(),
        }))
    }
}

/// This is used only after the preparation owner has recovered and authorized
/// the exact stored acquisition snapshot. External references retain their own
/// owner; the canonical full input is already inside the authenticated snapshot.
pub(super) fn verify_work_input_reference(
    request: &ValidatedSetupRequest,
    reference: &ContextReference,
    evidence: &SetupRequestEvidence,
    context: &SetupUseContext<'_>,
    references: &dyn ContextReferenceOwner,
) -> Result<(), SetupError> {
    if reference == &canonical_setup_input_reference(request)? {
        request.revalidate_access(evidence, context, references)?;
        return Ok(());
    }
    references.verify_access(reference, evidence, context)?;
    Ok(())
}
