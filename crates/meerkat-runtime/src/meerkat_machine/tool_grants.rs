//! The runtime owner's scoped tool grants: the grant service over its
//! store, the deadline driver of its served sessions, and each served
//! session's grant seam on its runtime bindings.
//!
//! A grant, revoke, list or audit read touches only grant rows: it neither
//! loads, registers nor wakes the subject's session. A grant or list that
//! finds the session serving its subject served here arms that session's
//! deadline driver for the subject.

use std::sync::Arc;

use meerkat_core::approval::decision::{DecisionContractError, StoredDecision};
use meerkat_core::tool_grant::{
    InstalledToolGrantGate, RunningToolGrantGate, SessionToolGrants, ToolGrantId, ToolGrantSubject,
};
use meerkat_core::types::SessionId;

use super::MeerkatMachine;
use crate::decision::DurableDecisionService;
use crate::tool_grant::{
    RevokedToolGrant, RuntimeSessionToolGrants, StoredToolGrant, ToolGrantAuditPage,
    ToolGrantClock, ToolGrantDeadlines, ToolGrantExpiryDriver, ToolGrantIssueBody,
    ToolGrantIssueExecutor, ToolGrantIssueRequest, ToolGrantListing, ToolGrantPolicy,
    ToolGrantRequest, ToolGrantRequestError, ToolGrantRequestHost, ToolGrantRequestNotices,
    ToolGrantRequestPolicy, ToolGrantRevocation, ToolGrantService, ToolGrantServiceError,
    ToolGrantSubjectObserver, hold_tool_grant_request, tool_grant_requests_of,
};

impl MeerkatMachine {
    /// The scoped tool grant gate the session `session_id` runs now, as its
    /// machine records it: the gate its build installed, the names a
    /// boundary since gated for a gated server, the names a live grant
    /// opened at its last boundary, and the MCP servers its external tool
    /// surface holds (an add staged or pending, or the server active), read
    /// from the same state. `None` when the session is not registered here:
    /// this loads, registers and wakes nothing.
    pub async fn tool_grant_gate(&self, session_id: &SessionId) -> Option<RunningToolGrantGate> {
        let authority = {
            let sessions = self.sessions.read().await;
            Arc::clone(&sessions.get(session_id)?.dsl_authority)
        };
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        let attached_servers = state
            .known_surfaces
            .iter()
            .filter(|surface| {
                state.surface_base_state.get(*surface)
                    == Some(&super::dsl::ExternalToolSurfaceBaseState::Active)
                    || state.surface_staged_op.get(*surface)
                        == Some(&super::dsl::SurfaceStagedOp::Add)
                    || state.surface_pending_op.get(*surface)
                        == Some(&super::dsl::SurfacePendingOp::Add)
            })
            .cloned()
            .collect();
        Some(RunningToolGrantGate {
            gate: InstalledToolGrantGate {
                gated: state.tool_grant_gated_names.clone(),
                gated_servers: state.tool_grant_gated_servers.clone(),
                gates_unconfigured_servers: state.tool_grant_gates_unconfigured_servers,
                ungated_servers: state.tool_grant_ungated_servers.clone(),
                not_mounted_names: state.tool_grant_not_mounted_names.clone(),
            },
            open: state.tool_grant_open_names.clone(),
            attached_servers,
        })
    }
    /// This runtime's scoped tool grant service, over its store.
    #[must_use]
    pub fn tool_grant_service(&self) -> Arc<ToolGrantService> {
        Arc::clone(&self.tool_grant_service)
    }

    /// The deadline driver of this runtime's served sessions.
    #[must_use]
    pub fn tool_grant_deadlines(&self) -> &ToolGrantDeadlines {
        &self.tool_grant_deadlines
    }

    /// Read `clock` instead of the host clock in every grant adjudication.
    pub fn set_tool_grant_clock(&self, clock: Arc<dyn ToolGrantClock>) {
        self.tool_grant_service.set_clock(clock);
    }

    /// Issue grants under `policy`.
    pub fn set_tool_grant_policy(&self, policy: ToolGrantPolicy) {
        self.tool_grant_service.set_policy(policy);
    }

    /// Issue one grant ([`ToolGrantService::grant`]). `serving` is the
    /// session that serves an identity subject now, when the surface knows
    /// it (a session subject is served by its own session). When that
    /// session is served here, its deadline driver is armed for the subject
    /// with the grant's deadline.
    ///
    /// Grant a mob member identity through
    /// `meerkat_mob::MobHandle::grant_member_tool`, which the mob's actor
    /// decides against its live roster as it commits, in order with the
    /// retirements that end an identity's grants. This writer, like
    /// [`ToolGrantService::grant`] with a member subject, does not consult the
    /// roster: a grant it writes for an identity that has retired is issued,
    /// and a name seated again in the same incarnation holds it.
    ///
    /// # Errors
    /// The service's refusal.
    pub async fn grant_tool(
        &self,
        request: ToolGrantRequest,
        serving: Option<&SessionId>,
    ) -> Result<StoredToolGrant, ToolGrantServiceError> {
        let grant = self.tool_grant_service.grant(request).await?;
        self.arm_if_served(&grant.record.subject, serving, grant.expires_at_ms)
            .await;
        Ok(grant)
    }

    /// Revoke one grant ([`ToolGrantService::revoke`]). A session that
    /// serves it refuses its names at the next gated dispatch, in this
    /// process or any other over the same store, and hides them at its next
    /// model-request boundary. The answer says whether this revoke changed
    /// the grant.
    ///
    /// # Errors
    /// The service's refusal.
    pub async fn revoke_tool_grant(
        &self,
        grant_id: &ToolGrantId,
        revocation: ToolGrantRevocation,
    ) -> Result<RevokedToolGrant, ToolGrantServiceError> {
        self.tool_grant_service.revoke(grant_id, revocation).await
    }

    /// End every live grant of `subject` ([`ToolGrantService::revoke_subject`]):
    /// its owner ended the subject (a mob member identity whose identity
    /// ended). Answers how many grants this call ended.
    ///
    /// # Errors
    /// The service's refusal; a grant it could not end stays live.
    pub async fn end_tool_grants_of(
        &self,
        subject: &ToolGrantSubject,
        revocation: ToolGrantRevocation,
    ) -> Result<usize, ToolGrantServiceError> {
        self.tool_grant_service
            .revoke_subject(&subject.key(), revocation)
            .await
    }

    /// One grant as its row stands, read without settling it, so a surface
    /// can check a grant's subject before it acts on the grant.
    ///
    /// # Errors
    /// [`ToolGrantServiceError::NotFound`], a row this binary cannot read,
    /// and store failures.
    pub async fn tool_grant(
        &self,
        grant_id: &ToolGrantId,
    ) -> Result<StoredToolGrant, ToolGrantServiceError> {
        self.tool_grant_service.load(grant_id).await
    }

    /// Every grant of `subject`, each due one settled as expired first, and
    /// every row of the subject this binary cannot read
    /// ([`ToolGrantService::list`]). When the session serving the subject
    /// (`serving` for an identity, the subject's own session for a session)
    /// is served here, its deadline driver is armed for the subject with its
    /// next deadline.
    ///
    /// # Errors
    /// The service's refusal.
    pub async fn list_tool_grants(
        &self,
        subject: &ToolGrantSubject,
        serving: Option<&SessionId>,
    ) -> Result<ToolGrantListing, ToolGrantServiceError> {
        let settled = self
            .tool_grant_service
            .settle(&subject.key(), ToolGrantExpiryDriver::Read)
            .await?;
        if let Some(deadline_ms) = settled.next_deadline_ms {
            self.arm_if_served(subject, serving, deadline_ms).await;
        }
        Ok(ToolGrantListing {
            grants: settled.grants,
            unreadable: settled.unreadable,
        })
    }

    /// A page of the audit rows of `subject`'s grants
    /// ([`ToolGrantService::audit`]): the rows this binary reads, and the
    /// rows of the page it cannot read.
    ///
    /// # Errors
    /// The service's refusal.
    pub async fn read_tool_grant_audit(
        &self,
        subject: &ToolGrantSubject,
        after: Option<u64>,
        limit: usize,
    ) -> Result<ToolGrantAuditPage, ToolGrantServiceError> {
        self.tool_grant_service.audit(subject, after, limit).await
    }

    /// Hold the decision of one grant request over `decisions`
    /// ([`hold_tool_grant_request`]), at this runtime's grant clock and
    /// maximum lifetime: the decision, and the body it holds. Nothing is
    /// issued.
    ///
    /// # Errors
    /// The request's refusal, the clock's failure, or the decision owner's.
    pub fn request_tool_grant(
        &self,
        decisions: &DurableDecisionService,
        policy: &ToolGrantRequestPolicy,
        request: ToolGrantIssueRequest,
    ) -> Result<(StoredDecision, ToolGrantIssueBody), ToolGrantRequestError> {
        let now_ms = self
            .tool_grant_service
            .now_ms()
            .map_err(|error| ToolGrantRequestError::Clock(error.to_string()))?;
        let decision = hold_tool_grant_request(
            decisions,
            policy,
            self.tool_grant_service.policy().max_lifetime_ms(),
            now_ms,
            request,
        )?;
        let body = ToolGrantIssueBody::of(&decision)?;
        Ok((decision, body))
    }

    /// The grant requests held for `subject` over `decisions`
    /// ([`tool_grant_requests_of`]): the open and approved ones and, with
    /// `include_ended`, the ended ones whose deadline is within the grant
    /// policy's maximum lifetime of this runtime's grant clock, so a list
    /// reads recent requests only.
    ///
    /// # Errors
    /// The clock's failure, or the decision owner's read failure.
    pub fn list_tool_grant_requests(
        &self,
        decisions: &DurableDecisionService,
        subject: &ToolGrantSubject,
        include_ended: bool,
    ) -> Result<Vec<(StoredDecision, ToolGrantIssueBody)>, ToolGrantRequestError> {
        let ended_since_ms = if include_ended {
            let now_ms = self
                .tool_grant_service
                .now_ms()
                .map_err(|error| ToolGrantRequestError::Clock(error.to_string()))?;
            Some(now_ms.saturating_sub(self.tool_grant_service.policy().max_lifetime_ms()))
        } else {
            None
        };
        Ok(tool_grant_requests_of(decisions, subject, ended_since_ms)?)
    }

    /// Take this runtime's agents' grant requests through `host` (`None`:
    /// take none): a gated session's `request_tool_grant` holds its request
    /// there. A host sets it while it runs the executor of the decisions
    /// the requests hold.
    pub fn set_tool_grant_request_host(&self, host: Option<ToolGrantRequestHost>) {
        self.tool_grant_service.set_request_host(host);
    }

    /// The executor of `tool_grant.issue` decisions over this runtime's grant
    /// service, asking `subjects` what a request's subject is now (whether it
    /// still holds grants, and its current run) and handing each ended
    /// request's notice to `notices`.
    ///
    /// # Errors
    /// Never in practice: the decision kind is a valid token.
    pub fn tool_grant_issue_executor(
        &self,
        subjects: Arc<dyn ToolGrantSubjectObserver>,
        notices: Arc<dyn ToolGrantRequestNotices>,
    ) -> Result<ToolGrantIssueExecutor, DecisionContractError> {
        ToolGrantIssueExecutor::new(Arc::clone(&self.tool_grant_service), subjects, notices)
    }

    /// `session_id`'s scoped tool grant seam, carried on its runtime
    /// bindings. A mob member's build turns it into the member's seam
    /// ([`SessionToolGrants::for_member`]).
    pub(super) fn session_tool_grants(&self, session_id: &SessionId) -> Arc<dyn SessionToolGrants> {
        Arc::new(RuntimeSessionToolGrants::new(
            Arc::clone(&self.tool_grant_service),
            self.tool_grant_deadlines.clone(),
            session_id.clone(),
        ))
    }

    /// Arm the deadline driver of the session serving `subject` (its own
    /// session for a session subject, `serving` for an identity) for the
    /// subject with `deadline_ms`, when that session is served here. An
    /// unserved session is left alone. A grant issued outside
    /// [`Self::grant_tool`] (a mob member identity's, which the mob's actor
    /// commits against its live roster) is armed this way once it is issued.
    pub async fn arm_tool_grant_deadline(
        &self,
        subject: &ToolGrantSubject,
        serving: Option<&SessionId>,
        deadline_ms: u64,
    ) {
        self.arm_if_served(subject, serving, deadline_ms).await;
    }

    /// Arm the deadline driver of the session serving `subject` (its own
    /// session for a session subject, `serving` for an identity) for the
    /// subject with `deadline_ms`, when that session is served here. An
    /// unserved session is left alone.
    async fn arm_if_served(
        &self,
        subject: &ToolGrantSubject,
        serving: Option<&SessionId>,
        deadline_ms: u64,
    ) {
        let session_id = match subject {
            ToolGrantSubject::Session { session_id } => session_id,
            ToolGrantSubject::Identity { .. } => match serving {
                Some(session_id) => session_id,
                None => return,
            },
        };
        if self.sessions.read().await.contains_key(session_id) {
            self.tool_grant_deadlines
                .arm(session_id, &subject.key(), deadline_ms);
        }
    }
}
