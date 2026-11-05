//! The approval gate as Meerkat's `ToolDispatchAdmission`.
//!
//! The factory wraps every agent's tool dispatcher in an
//! `ExecutionPolicyGatedDispatcher` when a build carries
//! `SessionBuildOptions::tool_dispatch_admission`; that dispatcher awaits
//! [`ApprovalGate::await_dispatch_admission`] immediately before each call
//! reaches a tool. The gate asks the [`ApprovalEngine`]:
//!
//! - allow ⇒ `Ok(())`: the call runs (the ordinary access policy still
//!   applies after it);
//! - deny ⇒ `ToolError::PolicyDenied` with code [`DENIAL_CODE`] and
//!   [`deny_message`] as its model-facing text. It is an ordinary tool
//!   result (`policy_denied`), so the model reads the reason and the run
//!   continues;
//! - escalate ⇒ the engine waits for the human (or the escalation timeout,
//!   which denies) inside this call, while the UI shows the dialog.
//!
//! Before deciding, the gate asks the engine where the session's tool of
//! the call's name comes from (its live tool definitions, see
//! [`super::catalog`]): a name is a builtin, file or note tool only when
//! its provenance says so.
//!
//! [`install`] composes the gate into a build, after any admission the build
//! already carries, so every agent the TUI builds (fresh or resumed) runs
//! behind it. A helper the session delegates to (a mob member) gets a gate
//! from [`ApprovalGate::for_helper`], installed on its spawn spec by the
//! TUI's spawn customizer ([`crate::mobs`]): its calls are decided as the
//! delegating session's, naming the helper.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::service::SessionBuildOptions;
use meerkat_core::types::ToolCallView;
use meerkat_core::{
    LiveBridgeEffectKind, LiveBridgeEffectOutcome, SessionId, ToolConsequenceDenial,
    ToolDispatchAdmission, ToolDispatchContext, ToolError,
};

use super::engine::{ApprovalEngine, Caller, ToolCall};
use super::{DENIAL_CODE, Verdict, deny_message};

/// One agent's view of the approval engine.
pub struct ApprovalGate {
    engine: Arc<ApprovalEngine>,
    /// The session this gate was built for. For the session's own agent a
    /// dispatch context that names its origin session wins; a helper's calls
    /// always count for it.
    session_id: SessionId,
    /// The helper this gate was built for (`None`: the session's own agent).
    helper: Option<String>,
    /// An admission the build already carried; it decides first.
    preceding: Option<Arc<dyn ToolDispatchAdmission>>,
}

impl std::fmt::Debug for ApprovalGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalGate")
            .field("session_id", &self.session_id)
            .field("helper", &self.helper)
            .field("preceding", &self.preceding.is_some())
            .finish_non_exhaustive()
    }
}

impl ApprovalGate {
    /// A gate for `session_id`'s calls.
    #[must_use]
    pub fn new(engine: Arc<ApprovalEngine>, session_id: SessionId) -> Self {
        Self {
            engine,
            session_id,
            helper: None,
            preceding: None,
        }
    }

    /// A gate for the calls of `helper`, which works for `session_id`: they
    /// are judged with that session's transcript, allow cache and Stop, and
    /// count against the shared denial counters (see
    /// [`ApprovalEngine::decide_as`]).
    #[must_use]
    pub fn for_helper(
        engine: Arc<ApprovalEngine>,
        session_id: SessionId,
        helper: impl Into<String>,
    ) -> Self {
        Self {
            engine,
            session_id,
            helper: Some(helper.into()),
            preceding: None,
        }
    }

    /// The session this gate decides for.
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The helper this gate decides for, if any.
    #[must_use]
    pub fn helper(&self) -> Option<&str> {
        self.helper.as_deref()
    }

    /// Run `preceding` before this gate.
    #[must_use]
    pub fn after(mut self, preceding: Option<Arc<dyn ToolDispatchAdmission>>) -> Self {
        self.preceding = preceding;
        self
    }
}

/// Install the approval gate for `session_id` into `build`, composed after
/// any admission `build` already carries.
pub fn install(
    build: &mut SessionBuildOptions,
    engine: &Arc<ApprovalEngine>,
    session_id: SessionId,
) {
    let gate = ApprovalGate::new(Arc::clone(engine), session_id)
        .after(build.tool_dispatch_admission.take());
    build.tool_dispatch_admission = Some(Arc::new(gate));
}

/// The model-facing error for a denial.
#[must_use]
pub fn denial_error(reason: &super::DenyReason) -> ToolError {
    ToolError::PolicyDenied {
        denial: ToolConsequenceDenial::new(DENIAL_CODE, deny_message(reason)),
    }
}

#[async_trait]
impl ToolDispatchAdmission for ApprovalGate {
    async fn await_dispatch_admission(
        &self,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
        effect_kind: LiveBridgeEffectKind,
    ) -> Result<(), ToolError> {
        if let Some(preceding) = &self.preceding {
            preceding
                .await_dispatch_admission(call, context, effect_kind)
                .await?;
        }
        let origin_session = context.and_then(ToolDispatchContext::origin_session_id);
        let caller = match &self.helper {
            Some(helper) => Caller::helper(&self.session_id, helper),
            None => Caller::session(origin_session.unwrap_or(&self.session_id)),
        };
        // Where the calling agent's tool of this name comes from: the
        // session that is dispatching (a helper's own member session; for
        // a helper without a dispatch context nothing is known, so its
        // calls are judged as foreign tools).
        let tools_of = match &self.helper {
            Some(_) => origin_session,
            None => Some(origin_session.unwrap_or(&self.session_id)),
        };
        let origin = match tools_of {
            Some(session) => self.engine.tool_origin(session, call.name).await,
            None => None,
        };
        let tool_call = ToolCall {
            id: call.id,
            name: call.name,
            args: call.args,
            origin: origin.as_ref(),
        };
        match self.engine.decide_as(caller, tool_call).await {
            Verdict::Allow { .. } => Ok(()),
            Verdict::Deny { reason, .. } => Err(denial_error(&reason)),
        }
    }

    async fn record_dispatch_outcome(
        &self,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
        effect_kind: LiveBridgeEffectKind,
        outcome: LiveBridgeEffectOutcome,
    ) -> Result<(), ToolError> {
        match &self.preceding {
            Some(preceding) => {
                preceding
                    .record_dispatch_outcome(call, context, effect_kind, outcome)
                    .await
            }
            None => Ok(()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::DenyReason;
    use super::*;

    #[test]
    fn denials_are_policy_denied_with_the_reason() {
        let error = denial_error(&DenyReason::Human);
        assert_eq!(error.error_code(), "policy_denied");
        let text = error.to_string();
        assert!(
            text.starts_with("Tool call denied (auto_mode): Blocked by auto-mode safety policy"),
            "{text}"
        );
        assert!(text.contains("the user denied this action"));
    }
}
