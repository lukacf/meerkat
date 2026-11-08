//! Fixture selection of the production installed review policy. Metadata
//! consequence remains R2; only the actual authorized native target gets R3.
use super::*;
use meerkat_core::{OperationAuthorizationError, authorization::OperationReviewTier};
use toolkit_mobkit::installed_host::InstalledOperationReviewContext;

pub(crate) fn same_application(
    owner: &ApplicationConsentOwner,
    context: &InstalledOperationReviewContext<'_>,
) -> Result<(), OperationAuthorizationError> {
    let identity = owner.identity();
    if identity.application() != &context.native().grant_administrator
        || identity.deployment() != &context.selected().deployment
        || identity.realm() != &context.native().realm
        || context.selected().native_host.as_ref() != Some(context.native())
    {
        return Err(OperationAuthorizationError::Unavailable);
    }
    Ok(())
}

pub(crate) fn find(
    owner: &ApplicationConsentOwner,
    context: InstalledOperationReviewContext<'_>,
    selected: &[&str],
) -> Result<OperationReviewTier, OperationAuthorizationError> {
    let meerkat_core::ToolOperationTarget::RegisteredToolBundleOperation {
        bundle, operation, ..
    } = context.target()
    else {
        return Ok(OperationReviewTier::R1);
    };
    if bundle != toolkit_mobkit::FIND_AND_INSTALL_BUNDLE
        || operation != toolkit_mobkit::FIND_AND_INSTALL
    {
        return Ok(OperationReviewTier::R1);
    }
    same_application(owner, &context)?;
    let claims = context.association().candidate();
    // The selected names choose immutable member records, not a wire identity.
    // Normal installed authorization already checked the full retained source.
    let mut members = context.native().members.iter().filter(|member| {
        selected.contains(&member.member.member.as_str())
            && claims.target.logical_owner == member.logical_owner
            && claims.target.context_generation == member.context_generation
            && claims.target.audience == member.audience
    });
    if members.next().is_none() || members.next().is_some() {
        return Err(OperationAuthorizationError::Unavailable);
    }
    Ok(OperationReviewTier::R3)
}
