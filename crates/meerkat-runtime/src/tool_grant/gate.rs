//! [`RuntimeSessionToolGrants`]: one served session's scoped tool grants,
//! as the agent's model-request boundary and its dispatch gate read them.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::MobMemberBinding;
use meerkat_core::service::LiveMobIncarnation;
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
/// It adjudicates the grants of its subjects: the session's own, and for a
/// mob member's build ([`SessionToolGrants::for_member`]) the member's
/// identity first, then the session. The subjects are fixed per build. Each
/// boundary arms the runtime's deadline driver, per subject, with the
/// earliest deadline of the grants it opened, so they expire on time while
/// the session is served; nothing here wakes or registers a session.
pub struct RuntimeSessionToolGrants {
    service: Arc<ToolGrantService>,
    deadlines: ToolGrantDeadlines,
    session_id: SessionId,
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
        let subjects = vec![ToolGrantSubject::session(session_id.clone()).key()];
        Self {
            service,
            deadlines,
            session_id,
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
    async fn open_names(&self, gated: &GatedToolTable) -> Result<ToolNameSet, ToolGrantRefusal> {
        let adjudication = self
            .service
            .adjudicate_boundary(&self.subjects, gated)
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
