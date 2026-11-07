//! [`RuntimeSessionToolGrants`]: one served session's scoped tool grants,
//! as the agent's model-request boundary and its dispatch gate read them,
//! and the seam its agent asks for a grant through.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::RunId;
use meerkat_core::tool_grant::{
    GatedToolTable, SessionToolGrants, ToolGrantAgentRequest, ToolGrantAgentRequestRefusal,
    ToolGrantCall, ToolGrantId, ToolGrantRefusal, ToolGrantRequestReceipt, ToolGrantSubject,
    ToolGrantSubjectKey,
};
use meerkat_core::types::{SessionId, ToolNameSet};

use super::audit::ToolGrantExpiryDriver;
use super::deadlines::ToolGrantDeadlines;
use super::request::{
    ToolGrantIssueRequest, ToolGrantRequestError, ToolGrantRequestOrigin, hold_tool_grant_request,
};
use super::service::ToolGrantService;
use crate::decision::DecisionError;

/// One served session's scoped tool grants over the runtime's grant
/// service. The runtime hands it to the factory on the session's runtime
/// bindings.
///
/// It adjudicates the session's own grants (the session subject). Each
/// boundary arms the runtime's deadline driver with the earliest deadline of
/// the grants it opened, so the session's grants expire on time while it is
/// served, and the end of each of its runs ends the grants scoped to that
/// run; nothing here wakes or registers a session. While the runtime takes
/// its agents' grant requests ([`ToolGrantService::request_host`]), the
/// session's agent asks for a grant of its own session through it.
pub struct RuntimeSessionToolGrants {
    service: Arc<ToolGrantService>,
    deadlines: ToolGrantDeadlines,
    session_id: SessionId,
    subjects: Vec<ToolGrantSubjectKey>,
}

impl RuntimeSessionToolGrants {
    /// The grants of `session_id`, adjudicated by `service`, their deadlines
    /// driven by `deadlines`.
    #[must_use]
    pub fn new(
        service: Arc<ToolGrantService>,
        deadlines: ToolGrantDeadlines,
        session_id: SessionId,
    ) -> Self {
        let subjects = vec![ToolGrantSubject::session(session_id.clone()).key()];
        Self {
            service,
            deadlines,
            session_id,
            subjects,
        }
    }
}

impl std::fmt::Debug for RuntimeSessionToolGrants {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeSessionToolGrants")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionToolGrants for RuntimeSessionToolGrants {
    async fn open_names(
        &self,
        gated: &GatedToolTable,
        run_id: Option<&RunId>,
    ) -> Result<ToolNameSet, ToolGrantRefusal> {
        let adjudication = self
            .service
            .adjudicate_boundary(&self.subjects, gated, run_id)
            .await?;
        if let Some(deadline_ms) = adjudication.next_deadline_ms {
            self.deadlines.arm(&self.session_id, deadline_ms);
        }
        Ok(adjudication.open)
    }

    async fn admit_call(
        &self,
        gated: &GatedToolTable,
        call: ToolGrantCall<'_>,
    ) -> Result<(), ToolGrantRefusal> {
        // An ungated name never needs a grant.
        if !gated.is_gated(call.name) {
            return Ok(());
        }
        self.service.admit_call(&self.subjects, call).await
    }

    async fn end_run(&self, run_id: &RunId) -> Result<(), ToolGrantRefusal> {
        self.service
            .end_run(&self.subjects, run_id, ToolGrantExpiryDriver::Run)
            .await
            .map_err(|error| ToolGrantRefusal::Unavailable {
                detail: error.to_string(),
            })
    }

    /// Hold the agent's request for a grant to its own session, with what
    /// the runtime takes requests with: the requester its host names, the
    /// host policy's approver and deadline, the grant policy's maximum
    /// lifetime, keyed by the call's run and id.
    async fn request_grant(
        &self,
        request: ToolGrantAgentRequest<'_>,
    ) -> Result<ToolGrantRequestReceipt, ToolGrantAgentRequestRefusal> {
        let Some(host) = self.service.request_host() else {
            return Err(ToolGrantAgentRequestRefusal::NotTaken);
        };
        let now_ms = self
            .service
            .now_ms()
            .map_err(|error| ToolGrantAgentRequestRefusal::Unavailable(error.to_string()))?;
        let decision = hold_tool_grant_request(
            &host.decisions,
            &host.policy,
            self.service.policy().max_lifetime_ms(),
            now_ms,
            ToolGrantIssueRequest {
                subject: ToolGrantSubject::session(self.session_id.clone()),
                target: request.target.clone(),
                duration: request.duration,
                run_scope: None,
                reason: request.reason.map(str::to_owned),
                origin: ToolGrantRequestOrigin::Agent {
                    run_id: request.run_id.clone(),
                    call_id: request.call_id.to_owned(),
                },
                requester: host.requester,
            },
        )
        .map_err(|error| match error {
            ToolGrantRequestError::EmptyTarget
            | ToolGrantRequestError::InvalidDuration { .. }
            | ToolGrantRequestError::Invalid(_)
            | ToolGrantRequestError::Decision(DecisionError::Refused { .. }) => {
                ToolGrantAgentRequestRefusal::Invalid(error.to_string())
            }
            other => ToolGrantAgentRequestRefusal::Unavailable(other.to_string()),
        })?;
        Ok(ToolGrantRequestReceipt {
            grant_id: ToolGrantId::approved(&decision.decision_id),
            phase: decision.state.phase,
            deadline_at_ms: decision.descriptor.deadline_at_ms,
            decision_id: decision.decision_id,
        })
    }
}
