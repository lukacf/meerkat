//! The purge of a tombstoned memory record's content.

use super::IdentityRuntime;
use crate::identity_first::agent_memory::{
    AgentMemoryError, AgentMemoryPurgeError, AgentMemoryPurgeResult,
};
use crate::memory::records::{MemoryAuthor, MemoryScope};

impl IdentityRuntime {
    /// Whether the provider can purge a record's content.
    pub async fn agent_memory_supports_purge(&self) -> bool {
        self.agent_memory
            .read()
            .await
            .as_ref()
            .is_some_and(|injector| injector.provider().supports_purge())
    }

    /// Blank the content of the tombstoned record `memory_id` of `scope`
    /// and of the earlier versions it replaced, as the application. The
    /// rows stay. `purged` is false for an unknown id, a record of another
    /// scope, or a record purged before; a record that is not tombstoned is
    /// refused ([`AgentMemoryPurgeError::NotTombstoned`]). An identity's
    /// scope may be named whether or not the identity is still registered:
    /// this is the host's door to the memory of an agent that was removed.
    pub async fn purge_agent_memory(
        &self,
        scope: &MemoryScope,
        memory_id: &str,
    ) -> Result<AgentMemoryPurgeResult, AgentMemoryPurgeError> {
        self.purge_agent_memory_by(scope, memory_id, MemoryAuthor::Application)
            .await
    }

    /// [`Self::purge_agent_memory`], recorded as done by `author`.
    pub(crate) async fn purge_agent_memory_by(
        &self,
        scope: &MemoryScope,
        memory_id: &str,
        author: MemoryAuthor,
    ) -> Result<AgentMemoryPurgeResult, AgentMemoryPurgeError> {
        self.scoped_agent_memory()
            .await?
            .purge_authored(scope, memory_id, author)
            .await
    }

    /// The scope the record `memory_id` of `realm` lies in, whatever scope
    /// a caller names it under; `None` for an id no record holds, and from
    /// a provider that has no read of a record by its id. A purge names its
    /// record by a member's name and asks here for a record kept under
    /// that member's owner.
    pub(crate) async fn agent_memory_record_scope(
        &self,
        realm: &str,
        memory_id: &str,
    ) -> Result<Option<MemoryScope>, AgentMemoryError> {
        let Some(store) = self.scoped_agent_memory().await?.as_steward_store() else {
            return Ok(None);
        };
        let records = store
            .records_by_ids(realm, &[memory_id.trim().to_string()])
            .await?;
        Ok(records.into_iter().next().map(|record| record.scope))
    }
}
