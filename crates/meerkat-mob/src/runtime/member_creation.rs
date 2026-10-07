use crate::member_creation::{
    MemberCreationError, MemberCreationSnapshot, MemberCreationSource, MemberCreationSourceWitness,
};
use meerkat_core::SessionId;

impl super::MobHandle {
    /// Read the committed canonical event cursor directly from the journal.
    /// Available before activation; this does not wait for the runtime actor.
    pub async fn member_creation_journal_cursor(&self) -> Result<u64, MemberCreationError> {
        Ok(self.events.latest_cursor().await?)
    }

    /// Read the journal's original binding and creation facts for an exact
    /// session, including retired members. A read fault is never absence.
    /// This method does not authorize a current caller; verify that caller's
    /// current `get_member` binding independently before serving an operation.
    pub async fn member_creation_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<MemberCreationSnapshot>, MemberCreationError> {
        self.roster
            .read()
            .await
            .member_creation_for_session(session_id)
    }

    /// Capture a sealed source witness from a currently bound member and its
    /// persisted member binding. The witness can accompany a spawn in
    /// another mob; no comms label or model-supplied parent is consulted.
    pub async fn capture_member_creation_source(
        &self,
        session_id: &SessionId,
    ) -> Result<MemberCreationSourceWitness, MemberCreationError> {
        if !self.session_service.supports_persistent_sessions() {
            return Err(MemberCreationError::Absent(
                crate::MemberCreationAbsence::NonDurableService,
            ));
        }
        let snapshot = self.member_creation_for_session(session_id).await?.ok_or(
            MemberCreationError::Unavailable("session has no member creation event"),
        )?;
        let current = self
            .get_member(&crate::AgentIdentity::from(
                snapshot.member_binding.member.as_str(),
            ))
            .await?
            .ok_or(MemberCreationError::Unavailable(
                "source is no longer a member",
            ))?;
        if current.bridge_session_id() != Some(session_id) {
            return Err(MemberCreationError::Unavailable(
                "source session binding changed",
            ));
        }
        let creation_id = snapshot
            .creation
            .creation_id
            .ok_or(MemberCreationError::Absent(
                crate::MemberCreationAbsence::LegacyCreation,
            ))?;
        let metadata = crate::member_creation::load_creation_source_metadata(
            self.session_service.as_ref(),
            session_id,
        )
        .await?
        .ok_or(MemberCreationError::Unavailable(
            "source metadata is unavailable",
        ))?;
        let metadata = metadata
            .session_metadata
            .ok_or(MemberCreationError::Unavailable(
                "source metadata has no typed binding",
            ))?;
        let binding = metadata
            .mob_member_binding
            .ok_or(MemberCreationError::Unavailable(
                "source metadata has no member binding",
            ))?;
        if binding.mob_id != snapshot.member_binding.mob_id
            || binding.member != snapshot.member_binding.member
            || binding.role != current.role.as_str()
        {
            return Err(MemberCreationError::Unavailable(
                "source metadata binding disagrees",
            ));
        }
        Ok(MemberCreationSourceWitness {
            successor: false,
            source: Some(MemberCreationSource {
                session_id: session_id.clone(),
                member_binding: binding,
                creation_id,
            }),
        })
    }
}
