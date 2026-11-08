//! [`RuntimeSessionToolGrants`]: one served session's scoped tool grants,
//! as the agent's model-request boundary and its dispatch gate read them,
//! and the seam its agent asks for a grant through.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::service::LiveMobIncarnation;
use meerkat_core::tool_grant::{
    GatedToolTable, SessionToolGrants, ToolGrantAgentRequest, ToolGrantAgentRequestRefusal,
    ToolGrantCall, ToolGrantId, ToolGrantRefusal, ToolGrantRequestReceipt, ToolGrantSubject,
    ToolGrantSubjectKey,
};
use meerkat_core::types::{SessionId, ToolNameSet};
use meerkat_core::{MobMemberBinding, RunId};

use super::audit::ToolGrantExpiryDriver;
use super::deadlines::ToolGrantDeadlines;
use super::request::{
    ToolGrantIssueRequest, ToolGrantRequestError, ToolGrantRequestOrigin,
    hold_agent_tool_grant_request,
};
use super::service::ToolGrantService;
use crate::decision::DecisionError;

/// One served session's scoped tool grants over the runtime's grant
/// service. The runtime hands it to the factory on the session's runtime
/// bindings.
///
/// It adjudicates the grants of its subjects: the session's own, and for a
/// mob member's build ([`SessionToolGrants::for_member`]) the member's
/// identity first, then the session. The subjects are fixed per build. Each
/// boundary arms the runtime's deadline driver, per subject, with the
/// earliest deadline of the grants it opened, so they expire on time while
/// the session is served, and the end of each of the session's runs ends the
/// grants of every subject scoped to that run; nothing here wakes or
/// registers a session. While the runtime takes its agents' grant requests
/// ([`ToolGrantService::request_host`]), the agent asks for a grant of its
/// first subject (a member's identity, or the session) through it.
pub struct RuntimeSessionToolGrants {
    service: Arc<ToolGrantService>,
    deadlines: ToolGrantDeadlines,
    session_id: SessionId,
    /// The subject an agent's request names: a member's identity, or the
    /// session.
    subject: ToolGrantSubject,
    /// The subjects read, in order: a member's identity first, then the
    /// session. A refusal no grant explains is audited under the first.
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
        let subject = ToolGrantSubject::session(session_id.clone());
        let subjects = vec![subject.key()];
        Self {
            service,
            deadlines,
            session_id,
            subject,
            subjects,
        }
    }

    /// The grants of a mob member's build served by `session_id`: the
    /// identity `member` first, then the session.
    #[must_use]
    pub fn for_member_subject(
        service: Arc<ToolGrantService>,
        deadlines: ToolGrantDeadlines,
        session_id: SessionId,
        member: &ToolGrantSubject,
    ) -> Self {
        let subjects = vec![
            member.key(),
            ToolGrantSubject::session(session_id.clone()).key(),
        ];
        Self {
            service,
            deadlines,
            session_id,
            subject: member.clone(),
            subjects,
        }
    }

    /// The subjects this seam reads, in order.
    #[must_use]
    pub fn subjects(&self) -> &[ToolGrantSubjectKey] {
        &self.subjects
    }
}

impl std::fmt::Debug for RuntimeSessionToolGrants {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeSessionToolGrants")
            .field("session_id", &self.session_id)
            .field("subjects", &self.subjects)
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
        for (subject, deadline_ms) in &adjudication.next_deadlines {
            self.deadlines.arm(&self.session_id, subject, *deadline_ms);
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

    /// Hold the agent's request for a grant to its first subject, with what
    /// the runtime takes requests with: the requester its host names, the
    /// host policy's approver and deadline, the grant policy's maximum
    /// lifetime, keyed by the call's run and id. A call for a target the
    /// subject has an open or approved window request for joins that
    /// request, and the receipt says so with that request's duration.
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
        // The life of the subject the agent asks in: a request of another
        // life is never joined, and its approval issues nothing in this one.
        let subject_life = match &host.subjects {
            Some(subjects) => subjects.life(&self.subject).await,
            None => None,
        };
        let answer = hold_agent_tool_grant_request(
            &host.decisions,
            &host.policy,
            self.service.policy().max_lifetime_ms(),
            now_ms,
            ToolGrantIssueRequest {
                subject: self.subject.clone(),
                target: request.target.clone(),
                duration: request.duration,
                run_scope: None,
                reason: request.reason.map(str::to_owned),
                origin: ToolGrantRequestOrigin::Agent {
                    run_id: request.run_id.clone(),
                    call_id: request.call_id.to_owned(),
                },
                requester: host.requester,
                subject_life,
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
            grant_id: ToolGrantId::approved(&answer.decision.decision_id),
            phase: answer.decision.state.phase,
            deadline_at_ms: answer.decision.descriptor.deadline_at_ms,
            duration_ms: answer.body.duration_ms,
            joined: answer.joined,
            decision_id: answer.decision.decision_id,
        })
    }

    fn for_member(
        &self,
        member: &MobMemberBinding,
        incarnation: &LiveMobIncarnation,
    ) -> Option<Arc<dyn SessionToolGrants>> {
        let subject =
            ToolGrantSubject::member(member.mob_id.as_str(), incarnation, member.member.as_str());
        Some(Arc::new(Self::for_member_subject(
            Arc::clone(&self.service),
            self.deadlines.clone(),
            self.session_id.clone(),
            &subject,
        )))
    }
}
