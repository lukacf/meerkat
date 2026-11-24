//! Memory calls that name a scope instead of an identity: the removal of a
//! record of a shared scope.

use std::sync::Arc;

use super::IdentityRuntime;
use crate::identity_first::agent_memory::{
    AgentMemoryError, AgentMemoryForgetResult, AgentMemoryProvider, AgentMemoryRuntimeInjector,
};
use crate::memory::records::{MemoryAuthor, MemoryScope};

impl IdentityRuntime {
    /// The configured memory provider, for a call that names a scope: no
    /// identity is named, so none needs to be registered.
    pub(super) async fn scoped_agent_memory(
        &self,
    ) -> Result<Arc<dyn AgentMemoryProvider>, AgentMemoryError> {
        self.agent_memory
            .read()
            .await
            .as_ref()
            .map(AgentMemoryRuntimeInjector::provider)
            .ok_or_else(|| {
                AgentMemoryError::InvalidConfig("agent memory is not configured".to_string())
            })
    }

    /// Whether the provider can retire a record of a scope a call names.
    pub async fn agent_memory_supports_forget_in_scope(&self) -> bool {
        self.agent_memory
            .read()
            .await
            .as_ref()
            .is_some_and(|injector| {
                let provider = injector.provider();
                provider.supports_forget() && provider.supports_authored_writes()
            })
    }

    /// Retire the record `memory_id` of the shared scope `scope`, as the
    /// application. `deleted` is false for an unknown id, a record of
    /// another scope, or a record already tombstoned: the call never
    /// confirms an id in a scope it was not given.
    pub async fn forget_agent_memory_in_scope(
        &self,
        scope: &MemoryScope,
        memory_id: &str,
    ) -> Result<AgentMemoryForgetResult, AgentMemoryError> {
        self.forget_agent_memory_in_scope_by(scope, memory_id, MemoryAuthor::Application)
            .await
    }

    /// [`Self::forget_agent_memory_in_scope`], recorded as done by `author`.
    pub(crate) async fn forget_agent_memory_in_scope_by(
        &self,
        scope: &MemoryScope,
        memory_id: &str,
        author: MemoryAuthor,
    ) -> Result<AgentMemoryForgetResult, AgentMemoryError> {
        self.scoped_agent_memory()
            .await?
            .forget_authored(scope, memory_id, author)
            .await
    }
}
