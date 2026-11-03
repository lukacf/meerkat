//! [`RuntimeSessionToolGrants`]: one served session's scoped tool grants,
//! as the agent's model-request boundary and its dispatch gate read them.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::tool_grant::{
    GatedToolTable, SessionToolGrants, ToolGrantCall, ToolGrantRefusal, ToolGrantSubject,
    ToolGrantSubjectKey,
};
use meerkat_core::types::{SessionId, ToolNameSet};

use super::deadlines::ToolGrantDeadlines;
use super::service::ToolGrantService;

/// One served session's scoped tool grants over the runtime's grant
/// service. The runtime hands it to the factory on the session's runtime
/// bindings.
///
/// It adjudicates the session's own grants (the session subject). Each
/// boundary arms the runtime's deadline driver with the earliest deadline of
/// the grants it opened, so the session's grants expire on time while it is
/// served; nothing here wakes or registers a session.
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
    async fn open_names(&self, gated: &GatedToolTable) -> Result<ToolNameSet, ToolGrantRefusal> {
        let adjudication = self
            .service
            .adjudicate_boundary(&self.subjects, gated)
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
}
