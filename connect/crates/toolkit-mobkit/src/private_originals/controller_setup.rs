//! Obtain the existing actor's actual controller for a local setup review.
//! Native input is not submitted by this preparation. Human interaction starts
//! only after the native tracked call returns and releases its identity lease.

use super::*;
use async_trait::async_trait;
use meerkat_core::{ControllerModelClient, SessionId};
use meerkat_mobkit::identity_first::{
    BoundGovernedInput, GovernedBindContext, GovernedBindError, GovernedBindRefusal,
    GovernedBindRefusalKind, GovernedInputBinder, GovernedInputError, GovernedSettlement,
    GovernedWaitBudget, IdentityRuntimeError,
};
use meerkat_runtime::{Input, PromptInput};

/// A retained native client from the actual installed member, not a serialized
/// route label, a second client resolution or a permission. There is no wire
/// constructor or host-owned lookup table for this per-call value.
pub struct LocalControllerSetup {
    source_owner: Weak<PrivateOriginals>,
    original: PrivateOriginalRef,
    session_id: SessionId,
    controller: ControllerModelClient,
}

impl LocalControllerSetup {
    pub(in crate::private_originals) fn from_console_bind(
        source: &Arc<PrivateOriginals>,
        original: PrivateOriginalRef,
        context: &GovernedBindContext,
    ) -> Self {
        Self {
            source_owner: Arc::downgrade(source),
            original,
            session_id: context.session_id.clone(),
            controller: context.controller.clone(),
        }
    }
    pub(crate) fn source_owner(&self) -> Option<Arc<PrivateOriginals>> {
        self.source_owner.upgrade()
    }

    pub(crate) fn matches_grant_owner(&self, owner: &Arc<ToolkitOriginalOwner>) -> bool {
        self.source_owner()
            .is_some_and(|source| Arc::ptr_eq(&source.owner, owner))
    }

    pub fn original(&self) -> &PrivateOriginalRef {
        &self.original
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn controller(&self) -> &ControllerModelClient {
        &self.controller
    }

    pub fn into_parts(self) -> (PrivateOriginalRef, SessionId, ControllerModelClient) {
        (self.original, self.session_id, self.controller)
    }

    // Source-owner component fixtures do not simulate native preparation.
    // There is no corresponding constructor in production or on the wire.
    #[cfg(test)]
    pub(crate) fn owner_test_fixture(
        owner: &Arc<PrivateOriginals>,
        original: PrivateOriginalRef,
        session_id: SessionId,
        controller: ControllerModelClient,
    ) -> Self {
        Self {
            source_owner: Arc::downgrade(owner),
            original,
            session_id,
            controller,
        }
    }
}

impl PrivateOriginals {
    pub(crate) async fn prepare_local_controller_setup(
        self: &Arc<Self>,
        submission: PrivateOriginalSubmit,
    ) -> Result<LocalControllerSetup, PrivateOriginalError> {
        let route = &self.route(&submission.original.route)?;
        let event = self.event(&submission.original).await?;
        if digest(submission.content.as_bytes()) != event.content_sha256 {
            return Err(PrivateOriginalError::Conflict);
        }
        // Check the observation's shape before selecting the actual installed
        // target. This preparation never claims ingress admission from it.
        super::governed_submit::evidence(
            &route.authentication_domain,
            &submission.transport_authentication,
        )?;
        let runtime = self
            .identity_runtime
            .get()
            .and_then(|runtime| runtime.upgrade())
            .ok_or(PrivateOriginalError::Unavailable)?;
        let alias = route.grant_target.as_str();
        let identity = runtime
            .owned_identity_for_member_alias(alias)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let binder = Arc::new(ControllerSetupBinder {
            source_owner: Arc::downgrade(self),
            original: submission.original,
            captured: Mutex::new(None),
        });
        let result = runtime
            .send_governed_with_turn_ticket(
                &identity,
                Some(alias),
                Input::Prompt(PromptInput::new(submission.content, None)),
                binder.clone(),
            )
            .await;
        // The binder's withheld refusal precedes any input submission. The
        // retained typed capture establishes our preparation result; no error
        // text or ticket is interpreted as an admission observation.
        if !matches!(
            &result,
            Err(IdentityRuntimeError::GovernedInput(GovernedInputError::Refused(refusal)))
                if refusal.kind == GovernedBindRefusalKind::Withheld
        ) {
            return Err(PrivateOriginalError::Unavailable);
        }
        let captured = binder
            .captured
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .take();
        captured.ok_or(PrivateOriginalError::Unavailable)
    }
}

struct ControllerSetupBinder {
    source_owner: Weak<PrivateOriginals>,
    original: PrivateOriginalRef,
    captured: Mutex<Option<LocalControllerSetup>>,
}

#[async_trait]
impl GovernedInputBinder for ControllerSetupBinder {
    async fn try_bind(
        &self,
        context: &GovernedBindContext,
        _input: Input,
    ) -> Result<BoundGovernedInput, GovernedBindError> {
        let mut captured = self.captured.lock().map_err(|_| {
            GovernedBindRefusal::unavailable("Toolkit local controller preparation is unavailable")
        })?;
        if captured.is_some() {
            return Err(GovernedBindRefusal::denied(
                "Toolkit local controller preparation was already captured",
            )
            .into());
        }
        *captured = Some(LocalControllerSetup {
            source_owner: self.source_owner.clone(),
            original: self.original.clone(),
            session_id: context.session_id.clone(),
            controller: context.controller.clone(),
        });
        // Captured: this preparation binds nothing by design.
        Err(GovernedBindRefusal::withheld(
            "Toolkit local controller review precedes input submission",
        )
        .into())
    }

    /// A capture never waits: it answers at once.
    fn wait_budget(&self) -> GovernedWaitBudget {
        GovernedWaitBudget::None
    }

    fn retain_unresolved(&self, _settlement: Box<dyn GovernedSettlement>, _detail: &str) {
        // Native invokes this only for custody returned by bind(). This binder
        // never returns BoundGovernedInput and creates no settlement to submit,
        // retain or withdraw, so there is no native admission path to here.
    }
}

#[cfg(test)]
#[path = "controller_setup_tests.rs"]
mod tests;
