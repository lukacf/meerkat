//! Trusted gateway selection. The review is built from the retained issuer;
//! a caller cannot pair an unrelated prebuilt review with a claimed owner.
use super::{ApplicationConsentOwner, ConsentApplicationIdentity, ConsentOwnerError};
use meerkat_core::approval::{
    consent::HumanConsentRoute,
    review::{BoundOperationReview, OperationReviewer},
};
use std::{sync::Arc, time::Duration};
use toolkit_mobkit::installed_host::PreparedInstallationOwner;

/// Explicit process composition for a prepared or installed application. Persist the
/// owner's exact selection before use, and reopen that selection for a later
/// process. This does not change the application's grants or review tiers.
///
/// No human route is installed by default. Connect's Console route still
/// requires Meta's authenticated currentness evidence; this type does not
/// manufacture it from a token, login flag or the application identity.
pub struct ApplicationOperationReview {
    owner: Arc<ApplicationConsentOwner>,
    reviewer: Arc<dyn OperationReviewer>,
    deadline: Duration,
    human: Option<(Arc<dyn HumanConsentRoute>, Duration)>,
    #[cfg(test)]
    observer: Option<Arc<dyn meerkat_core::approval::review::OperationReviewObserver>>,
}
impl ApplicationOperationReview {
    pub fn new(
        owner: Arc<ApplicationConsentOwner>,
        reviewer: Arc<dyn OperationReviewer>,
        deadline: Duration,
    ) -> Self {
        Self {
            owner,
            reviewer,
            deadline,
            human: None,
            #[cfg(test)]
            observer: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_observer(
        mut self,
        observer: Arc<dyn meerkat_core::approval::review::OperationReviewObserver>,
    ) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Supply an actual trusted route with its own authentication and current
    /// eligibility checks. The finite validity is selected by the application.
    /// This is not a built-in Console adapter and grants no review tier.
    #[must_use]
    pub fn with_human_consent(
        mut self,
        route: Arc<dyn HumanConsentRoute>,
        validity: Duration,
    ) -> Self {
        self.human = Some((route, validity));
        self
    }

    pub(crate) fn bind(
        self,
        prepared: &PreparedInstallationOwner,
    ) -> Result<RetainedApplicationOperationReview, ConsentOwnerError> {
        self.bind_identity(ConsentApplicationIdentity::from_preparation(prepared)?)
    }

    /// Trusted installed/restored composition before Meta constructs factories.
    /// Reopen the owner's exact persisted selection first; missing history is
    /// an error, never a request to create a replacement issuer. The host itself
    /// supplies the lifetime used for identity binding, not an independent pair.
    pub fn compose_installed(
        self,
        host: toolkit_mobkit::host_gateway::ToolkitGatewayHost,
    ) -> Result<
        toolkit_mobkit::host_gateway::ToolkitGatewayHost,
        meerkat_toolkit::host_lifetime::control::HostLifetimeError,
    > {
        self.compose_installed_with_owner(host)
            .map(|(host, _)| host)
    }

    // Return only the issuer whose identity the actual host accepted. This
    // internal carrier feeds workflow persistence before native construction.
    pub(crate) fn compose_installed_with_owner(
        self,
        host: toolkit_mobkit::host_gateway::ToolkitGatewayHost,
    ) -> Result<
        (
            toolkit_mobkit::host_gateway::ToolkitGatewayHost,
            Arc<ApplicationConsentOwner>,
        ),
        meerkat_toolkit::host_lifetime::control::HostLifetimeError,
    > {
        let owner = self.owner.clone();
        let host = host.with_operation_review(move |actual| {
            let identity = ConsentApplicationIdentity::from_installed_selection(actual)
                .map_err(installed_error)?;
            let retained = self.bind_identity(identity).map_err(installed_error)?;
            Ok(Arc::new(move || {
                retained.current().map_err(installed_error)
            }))
        })?;
        Ok((host, owner))
    }

    fn bind_identity(
        self,
        identity: ConsentApplicationIdentity,
    ) -> Result<RetainedApplicationOperationReview, ConsentOwnerError> {
        if self.owner.identity() != &identity {
            return Err(ConsentOwnerError::WrongApplication);
        }
        self.owner.check()?;
        if self.deadline.is_zero()
            || self
                .human
                .as_ref()
                .is_some_and(|(_, validity)| validity.is_zero())
        {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        let mut review =
            BoundOperationReview::new(self.reviewer, self.owner.approvals(), self.deadline);
        #[cfg(test)]
        if let Some(observer) = self.observer {
            review = review.with_observer(observer);
        }
        if let Some((route, validity)) = self.human {
            review = review.with_human_consent(route, validity);
        }
        Ok(RetainedApplicationOperationReview {
            owner: self.owner,
            review: Arc::new(review),
        })
    }
}

/// Process-owned selection made before resource preparation or native factories.
/// Review and settlement clones retain the same ApprovalService/store lease.
pub(crate) struct RetainedApplicationOperationReview {
    owner: Arc<ApplicationConsentOwner>,
    review: Arc<BoundOperationReview>,
}
impl RetainedApplicationOperationReview {
    pub(crate) fn owner(&self) -> Result<Arc<ApplicationConsentOwner>, ConsentOwnerError> {
        self.owner.check()?;
        Ok(self.owner.clone())
    }
    pub(crate) fn current(&self) -> Result<Arc<BoundOperationReview>, ConsentOwnerError> {
        self.owner.check()?;
        Ok(self.review.clone())
    }
}

fn installed_error(
    error: ConsentOwnerError,
) -> meerkat_toolkit::host_lifetime::control::HostLifetimeError {
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    match error {
        ConsentOwnerError::WrongApplication | ConsentOwnerError::WrongClaim => {
            HostLifetimeError::WrongOwner
        }
        // Missing/changed issuer history and store failure cannot become None.
        _ => HostLifetimeError::Unavailable,
    }
}

// Test-only prepared composition retains the exact early review object. It
// verifies the final installed identity without opening another issuer or
// constructing another BoundOperationReview.
#[cfg(test)]
impl RetainedApplicationOperationReview {
    pub(crate) fn compose_same_installed_for_test(
        self: Arc<Self>,
        host: toolkit_mobkit::host_gateway::ToolkitGatewayHost,
    ) -> Result<
        toolkit_mobkit::host_gateway::ToolkitGatewayHost,
        meerkat_toolkit::host_lifetime::control::HostLifetimeError,
    > {
        host.with_operation_review(move |actual| {
            let identity = ConsentApplicationIdentity::from_installed_selection(actual)
                .map_err(installed_error)?;
            if self.owner.identity() != &identity {
                return Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::WrongOwner);
            }
            self.current().map_err(installed_error)?;
            Ok(Arc::new(move || self.current().map_err(installed_error)))
        })
    }
}
