//! The runtime owner's scoped tool grants: the grant service over its
//! store, the deadline driver of its served sessions, and each served
//! session's grant seam on its runtime bindings.
//!
//! A grant, revoke, list or audit read touches only grant rows: it neither
//! loads, registers nor wakes the subject's session. A grant or list that
//! finds its session served here arms that session's deadline driver.

use std::sync::Arc;

use meerkat_core::tool_grant::{SessionToolGrants, ToolGrantId, ToolGrantSubject};
use meerkat_core::types::SessionId;

use super::MeerkatMachine;
use crate::tool_grant::{
    RuntimeSessionToolGrants, StoredToolGrant, ToolGrantAuditRecord, ToolGrantClock,
    ToolGrantDeadlines, ToolGrantExpiryDriver, ToolGrantListing, ToolGrantPolicy, ToolGrantRequest,
    ToolGrantRevocation, ToolGrantService, ToolGrantServiceError,
};

impl MeerkatMachine {
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

    /// Issue one grant ([`ToolGrantService::grant`]). When its subject's
    /// session is served here, the session's deadline driver is armed with
    /// the grant's deadline.
    ///
    /// # Errors
    /// The service's refusal.
    pub async fn grant_tool(
        &self,
        request: ToolGrantRequest,
    ) -> Result<StoredToolGrant, ToolGrantServiceError> {
        let grant = self.tool_grant_service.grant(request).await?;
        self.arm_if_served(&grant.record.subject, grant.expires_at_ms)
            .await;
        Ok(grant)
    }

    /// Revoke one grant ([`ToolGrantService::revoke`]). A session that
    /// serves it refuses its names at the next gated dispatch, in this
    /// process or any other over the same store, and hides them at its next
    /// model-request boundary.
    ///
    /// # Errors
    /// The service's refusal.
    pub async fn revoke_tool_grant(
        &self,
        grant_id: &ToolGrantId,
        revocation: ToolGrantRevocation,
    ) -> Result<StoredToolGrant, ToolGrantServiceError> {
        self.tool_grant_service.revoke(grant_id, revocation).await
    }

    /// Every grant of `subject`, each due one settled as expired first, and
    /// every row of the subject this binary cannot read
    /// ([`ToolGrantService::list`]). When the subject's session is served
    /// here, its deadline driver is armed with its next deadline.
    ///
    /// # Errors
    /// The service's refusal.
    pub async fn list_tool_grants(
        &self,
        subject: &ToolGrantSubject,
    ) -> Result<ToolGrantListing, ToolGrantServiceError> {
        let settled = self
            .tool_grant_service
            .settle(&subject.key(), ToolGrantExpiryDriver::Read)
            .await?;
        if let Some(deadline_ms) = settled.next_deadline_ms {
            self.arm_if_served(subject, deadline_ms).await;
        }
        Ok(ToolGrantListing {
            grants: settled.grants,
            unreadable: settled.unreadable,
        })
    }

    /// The audit rows of `subject`'s grants ([`ToolGrantService::audit`]).
    ///
    /// # Errors
    /// The service's refusal.
    pub async fn read_tool_grant_audit(
        &self,
        subject: &ToolGrantSubject,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<ToolGrantAuditRecord>, ToolGrantServiceError> {
        self.tool_grant_service.audit(subject, after, limit).await
    }

    /// `session_id`'s scoped tool grant seam, carried on its runtime
    /// bindings.
    pub(super) fn session_tool_grants(&self, session_id: &SessionId) -> Arc<dyn SessionToolGrants> {
        Arc::new(RuntimeSessionToolGrants::new(
            Arc::clone(&self.tool_grant_service),
            self.tool_grant_deadlines.clone(),
            session_id.clone(),
        ))
    }

    /// Arm the deadline driver of `subject`'s session with `deadline_ms`
    /// when the session is served here. An unserved session is left alone.
    async fn arm_if_served(&self, subject: &ToolGrantSubject, deadline_ms: u64) {
        let ToolGrantSubject::Session { session_id } = subject else {
            return;
        };
        if self.sessions.read().await.contains_key(session_id) {
            self.tool_grant_deadlines.arm(session_id, deadline_ms);
        }
    }
}
