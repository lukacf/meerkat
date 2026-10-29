//! Governed resume of retained scoped original work.
//!
//! A retained-work resume (an internal completion, such as a fork_off or
//! council outcome, admitted to the runtime that staged the original run)
//! carries no fresh transport authentication. The runtime has already
//! checked that every original contributor row is present and exact in its
//! ledger. [`ScopedRetainedResume`] is the installed
//! [`NativeRetainedResumeCheck`] of the scoped original composition: it
//! reacquires each retained original through the same owners that admitted
//! it, and supplies the actual controller client:
//!
//! - the original's ingress namespace is the installed ingress domain;
//! - the host adapter still holds that original
//!   ([`AuthenticatedOriginalSource::retained_custody`], the same custody
//!   that constrains every operation of work naming it) and its current
//!   mandate is the retained one. While the adapter has not settled its
//!   admission yet, the resume is not ready (the carrying delivery stays
//!   pending), never refused;
//! - the same host invocation policy still lets the original requester invoke
//!   the executor and represented-subject mandate, and a grant-lineage
//!   mandate still resolves in the actual grant owner;
//! - every contributor the original names is still held, and its own
//!   grant-lineage mandate is still live for its own grantee and subject
//!   (liveness only, as at admission).
//!
//! The usable controller client comes from the installed
//! [`RetainedControllerOwner`], which reacquires it through the existing
//! controller owners for the retained selection; this module composes it but
//! is no controller authority, and a selection in the evidence is data, never
//! a client. The local-grant host then revalidates each original's
//! current controller grant, account, model and ceilings. This module keeps
//! no registry and never promotes a retained association into authority.

use std::sync::Arc;

use meerkat_authorization::grant_policy::{AdmittedWorkPolicyOwner, OperationPolicyOwner};
use meerkat_authorization::grants::{GrantRefusal, LocalGrantAuthority};
use meerkat_authorization_contracts::constraints::ResourceDomain;
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
};
use meerkat_core::authorization::ControllerReadinessFailure;
use meerkat_core::{OperationRefusalKind, OperationRefused};

use super::local_authorization::NativeRetainedResumeCheck;
use super::scoped_original::{
    AuthenticatedOriginalSource, CustodyClass, ScopedGrantComposition, ScopedInvocation,
    ScopedMandateOwner, classify_custody, require_contributor_live,
};
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_authority::NativeAdmissionError;
use crate::retained_work::{RetainedResumeError, RetainedResumeEvidence};

/// The existing controller owners' reacquisition of the actual usable
/// controller client for a retained selection, composed by the embedding.
/// Any asynchronous client setup has completed before the synchronous resume
/// authentication asks; `Ok(None)` means no client is available for the
/// selection, an error that its owner cannot say now. The check answers
/// `Ok(None)` as `Readiness(UnsupportedScope)`: a fresh resume stays owed,
/// while an admitted resume whose client did not survive the store ends as a
/// refused resume at once. An error makes the runtime hold such a resume,
/// with its attempts, and ask again once per pause; once 30 seconds have
/// passed since its first such answer, the next one ends it as a refused
/// resume (see `NativeWorkAuthorizationHost::authenticate_retained_resume`).
pub trait RetainedControllerOwner: Send + Sync {
    fn reacquire_controller(
        &self,
        selection: &meerkat_core::ControllerModelSelection,
    ) -> Result<Option<meerkat_core::ControllerModelClient>, ControllerReadinessFailure>;
}

/// The retained-resume face of the host invocation policy: whether the
/// original requester may still invoke the association's executor for its
/// represented subject under its mandate. Implemented by the same concrete
/// policy as [`ScopedMandateOwner`] and [`AdmittedWorkPolicyOwner`], under
/// the same grant publication, synchronously and without I/O.
pub trait RetainedInvocationOwner: Send + Sync {
    fn reauthorize_retained(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError>;
}

/// The installed retained-resume check of the scoped original composition.
pub struct ScopedRetainedResume {
    grants: Arc<LocalGrantAuthority>,
    ingress_domain: ResourceDomain,
    originals: Arc<dyn AuthenticatedOriginalSource>,
    invocation: Arc<dyn RetainedInvocationOwner>,
    controllers: Arc<dyn RetainedControllerOwner>,
}

impl ScopedRetainedResume {
    pub fn new(
        grants: Arc<LocalGrantAuthority>,
        ingress_domain: ResourceDomain,
        originals: Arc<dyn AuthenticatedOriginalSource>,
        invocation: Arc<dyn RetainedInvocationOwner>,
        controllers: Arc<dyn RetainedControllerOwner>,
    ) -> Self {
        Self {
            grants,
            ingress_domain,
            originals,
            invocation,
            controllers,
        }
    }

    /// The check as `NativeGrantWorkConfiguration::retained_resume`.
    pub fn into_check(self) -> Arc<NativeRetainedResumeCheck> {
        let this = Arc::new(self);
        Arc::new(move |runtime, input, evidence| this.check(runtime, input, evidence))
    }

    pub fn check(
        &self,
        _runtime: &LogicalRuntimeId,
        _input: &Input,
        evidence: &RetainedResumeEvidence,
    ) -> Result<meerkat_core::ControllerModelClient, RetainedResumeError> {
        let selection = evidence
            .identity()
            .controller()
            .ok_or_else(|| refused(OperationRefusalKind::MalformedFacts))?;
        if evidence.contributors().is_empty() {
            return Err(refused(OperationRefusalKind::MalformedFacts));
        }
        for original in evidence.contributors() {
            self.reacquire(original.association())?;
        }
        let client = self
            .controllers
            .reacquire_controller(selection)
            .map_err(RetainedResumeError::Readiness)?
            .ok_or(RetainedResumeError::Readiness(
                ControllerReadinessFailure::UnsupportedScope,
            ))?;
        if client.selection() != selection {
            return Err(refused(OperationRefusalKind::MalformedFacts));
        }
        Ok(client)
    }

    /// Reacquire one retained original through the owners that admitted it.
    fn reacquire(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<(), RetainedResumeError> {
        let claims = association.candidate();
        if claims.ingress_namespace.ingress != self.ingress_domain {
            return Err(refused(OperationRefusalKind::MalformedFacts));
        }
        let basis = self.held(&claims.ingress_namespace, &claims.original_work)?;
        if basis != claims.authority_basis {
            return Err(refused(OperationRefusalKind::MalformedFacts));
        }
        self.invocation
            .reauthorize_retained(association)
            .map_err(admission_error)?;
        self.resolve_basis(&basis, association)?;
        // A contributor may be another executor's original: as at
        // admission, its conjunct is the liveness of its own mandate, for its
        // own leaf, never a resolution for this original's executor.
        for contributor in &claims.contributing_work {
            let basis = self.held(&claims.ingress_namespace, contributor)?;
            require_contributor_live(&self.grants, &basis).map_err(admission_error)?;
        }
        Ok(())
    }

    /// An original the adapter no longer holds has no current authority. One
    /// whose admission it has not settled yet is not ready: the carrying
    /// delivery stays pending until the owner promotes or withdraws it.
    fn held(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<WorkAuthorityBasis, RetainedResumeError> {
        match classify_custody(self.originals.as_ref(), namespace, original)
            .map_err(RetainedResumeError::Readiness)?
        {
            CustodyClass::Retained(basis) => Ok(basis),
            CustodyClass::Pending => Err(RetainedResumeError::Readiness(
                ControllerReadinessFailure::OriginalAdmissionPending,
            )),
            CustodyClass::Cancelled | CustodyClass::NotRetained => {
                Err(refused(OperationRefusalKind::Denied))
            }
        }
    }

    fn resolve_basis(
        &self,
        basis: &WorkAuthorityBasis,
        association: &InputAuthorityAssociation,
    ) -> Result<(), RetainedResumeError> {
        let claims = association.candidate();
        for lineage in meerkat_authorization::grant_policy::admission_grant_lineages(basis) {
            self.grants
                .resolve_lineage(
                    lineage,
                    &claims.logical_executor,
                    claims.represented_subject.as_ref(),
                )
                .map_err(|refusal| match refusal {
                    GrantRefusal::Denied => refused(OperationRefusalKind::Denied),
                    GrantRefusal::ControllerInUse
                    | GrantRefusal::Duplicate
                    | GrantRefusal::Unavailable => RetainedResumeError::Readiness(
                        ControllerReadinessFailure::PolicyUnavailable,
                    ),
                })?;
        }
        Ok(())
    }
}

impl ScopedGrantComposition {
    /// The scoped original composition with its retained-resume check, where
    /// fresh admission and resume use one adapter custody and one host
    /// invocation policy, each taken from the same `Arc`.
    pub fn with_retained_resume<A, P>(
        grants: Arc<LocalGrantAuthority>,
        ingress_domain: ResourceDomain,
        originals: Arc<A>,
        policy: Arc<P>,
        controllers: Arc<dyn RetainedControllerOwner>,
        operation_owner: Arc<dyn OperationPolicyOwner>,
    ) -> Self
    where
        A: AuthenticatedOriginalSource + 'static,
        P: ScopedMandateOwner + AdmittedWorkPolicyOwner + RetainedInvocationOwner + 'static,
    {
        let retained_resume = ScopedRetainedResume::new(
            Arc::clone(&grants),
            ingress_domain.clone(),
            Arc::clone(&originals) as Arc<dyn AuthenticatedOriginalSource>,
            Arc::clone(&policy) as Arc<dyn RetainedInvocationOwner>,
            controllers,
        )
        .into_check();
        Self {
            grants,
            ingress_domain,
            originals,
            invocation: ScopedInvocation::from_policy(policy),
            operation_owner,
            retained_resume: Some(retained_resume),
        }
    }
}

fn refused(kind: OperationRefusalKind) -> RetainedResumeError {
    RetainedResumeError::Refused(OperationRefused::new(kind))
}

fn admission_error(error: NativeAdmissionError) -> RetainedResumeError {
    match error {
        NativeAdmissionError::Refused(refusal) => RetainedResumeError::Refused(refusal),
        NativeAdmissionError::Readiness(reason) => RetainedResumeError::Readiness(reason),
    }
}
