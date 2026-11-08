//! Passive, non-circular linkage embedded in a workflow resource association.
//! Its native domain comes from the actual captured machine's authority. This
//! record holds no claim, grants no consent and proves no recovery disposition.
use super::*;
use meerkat_core::{
    SessionId,
    session_hosting::{HostingClaim, HostingDomainRef},
};
use meerkat_runtime::MeerkatMachine;

/// Exact selected issuer/application and native session. A missing domain is
/// explicit process-local provenance, usable for ordinary operation but never
/// sufficient for cross-process recovery. Absence of this whole record in an
/// older association remains Unknown, not an empty/default issuer selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentResourceProvenance {
    identity: ConsentApplicationIdentity,
    issuer: ConsentIssuerSelection,
    session: SessionId,
    domain: Option<HostingDomainRef>,
}
impl ApplicationConsentOwner {
    pub(crate) fn resource_provenance(
        &self,
        machine: &MeerkatMachine,
        session: &SessionId,
    ) -> Result<ConsentResourceProvenance, ConsentOwnerError> {
        self.check()?;
        Ok(ConsentResourceProvenance {
            identity: self.identity().clone(),
            issuer: self.selection().clone(),
            session: session.clone(),
            domain: machine.hosting_authority().capability().domain_ref(),
        })
    }
}
impl ConsentResourceProvenance {
    pub fn identity(&self) -> &ConsentApplicationIdentity {
        &self.identity
    }
    pub fn issuer(&self) -> &ConsentIssuerSelection {
        &self.issuer
    }
    pub fn session(&self) -> &SessionId {
        &self.session
    }
    pub fn domain(&self) -> Option<&HostingDomainRef> {
        self.domain.as_ref()
    }

    pub(crate) fn check_current(
        &self,
        owner: &ApplicationConsentOwner,
        machine: &MeerkatMachine,
        session: &SessionId,
    ) -> Result<(), ConsentOwnerError> {
        owner.check()?;
        if &self.identity != owner.identity() {
            return Err(ConsentOwnerError::WrongApplication);
        }
        if &self.issuer != owner.selection() {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        if &self.session != session
            || self.domain != machine.hosting_authority().capability().domain_ref()
        {
            return Err(ConsentOwnerError::WrongClaim);
        }
        Ok(())
    }

    // The canonical ResourceAssociation reader supplies its own checked outer
    // artifact. Keeping that hash outside this record avoids a self-hash cycle.
    // No grant/reservation/registration is obtained here; the caller must hold
    // the real native claim and retain it through its separate recovery work.
    pub(crate) fn bind_claim(
        &self,
        owner: &ApplicationConsentOwner,
        machine: &MeerkatMachine,
        session: &SessionId,
        artifact: SelectedHostArtifact,
        claim: &HostingClaim,
    ) -> Result<ConsentAssociation, ConsentOwnerError> {
        self.check_current(owner, machine, session)?;
        let domain = self.domain.as_ref().ok_or(ConsentOwnerError::WrongClaim)?;
        let authority = machine.hosting_authority();
        if !claim.is_cross_process()
            || claim.owner() != authority.owner()
            || claim.session_id() != session
            || claim.domain_ref().as_ref() != Some(domain)
        {
            return Err(ConsentOwnerError::WrongClaim);
        }
        owner.bind_association(artifact, claim)
    }
}
