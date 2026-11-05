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
//! [`install`] composes the gate into a build, after any admission the build
//! already carries, so every agent the TUI builds (fresh or resumed) runs
//! behind it.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::service::SessionBuildOptions;
use meerkat_core::types::ToolCallView;
use meerkat_core::{
    LiveBridgeEffectKind, LiveBridgeEffectOutcome, SessionId, ToolConsequenceDenial,
    ToolDispatchAdmission, ToolDispatchContext, ToolError,
};
use serde_json::Value;

use super::engine::ApprovalEngine;
use super::{DENIAL_CODE, Verdict, deny_message};

/// One session's view of the approval engine.
pub struct ApprovalGate {
    engine: Arc<ApprovalEngine>,
    /// The session this gate was built for; a dispatch context that names its
    /// origin session wins.
    session_id: SessionId,
    /// An admission the build already carried; it decides first.
    preceding: Option<Arc<dyn ToolDispatchAdmission>>,
}

impl std::fmt::Debug for ApprovalGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalGate")
            .field("session_id", &self.session_id)
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
            preceding: None,
        }
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

/// The call's arguments as JSON; unparsable arguments are passed on as their
/// raw text (they parse as no known action, so they reach the classifier).
fn call_args(call: ToolCallView<'_>) -> Value {
    call.parse_args::<Value>()
        .unwrap_or_else(|_| Value::String(call.args.get().to_string()))
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
        let session_id = context
            .and_then(ToolDispatchContext::origin_session_id)
            .unwrap_or(&self.session_id);
        let args = call_args(call);
        match self
            .engine
            .decide(session_id, call.id, call.name, &args)
            .await
        {
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
