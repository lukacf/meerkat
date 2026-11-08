//! Call-scoped classification around the original native context owner.
//! The returned context, including its credential final check, is never rebuilt.
use super::*;
use crate::service_install::read_only::InstalledReadOnly;
use meerkat_core::{
    McpServerConfig, ToolAccessPolicy, ToolMutationClass, ops::ToolAccessConstraint,
};
use meerkat_mcp::{
    McpCallContext, McpCallContextError, McpCallContextProvider, McpCallTarget, McpResourceTarget,
};

pub(super) fn policy(exposed: &str, read_only: bool) -> ToolAccessPolicy {
    let names = [exposed.to_owned()].into_iter().collect();
    if read_only {
        ToolAccessPolicy::Constraints(vec![
            ToolAccessConstraint::AllowNames(names),
            ToolAccessConstraint::ReadOnly,
        ])
    } else {
        ToolAccessPolicy::AllowList(names)
    }
}
fn classification(inner: ToolMutationClass, declared: bool) -> ToolMutationClass {
    match inner {
        ToolMutationClass::Mutating => ToolMutationClass::Mutating,
        ToolMutationClass::ReadOnly => ToolMutationClass::ReadOnly,
        ToolMutationClass::Unknown if declared => ToolMutationClass::ReadOnly,
        ToolMutationClass::Unknown => ToolMutationClass::Unknown,
    }
}
pub(super) struct Provider {
    inner: Arc<dyn McpCallContextProvider>,
    declaration: Option<InstalledReadOnly>,
    config: McpServerConfig,
    storage: ResolvedStorage,
    host: Arc<ServiceHost>,
    gate: Arc<ProbeGate>,
    generation: u64,
}
impl Provider {
    pub(super) fn new(
        inner: Arc<dyn McpCallContextProvider>,
        declaration: Option<InstalledReadOnly>,
        config: McpServerConfig,
        storage: ResolvedStorage,
        host: Arc<ServiceHost>,
        gate: Arc<ProbeGate>,
        generation: u64,
    ) -> Self {
        Self {
            inner,
            declaration,
            config,
            storage,
            host,
            gate,
            generation,
        }
    }
    pub(super) async fn recheck(&self) -> Result<(), McpCallContextError> {
        if !self.host.lifetime.current(self.host.binding.expires_at_ms) {
            return Err(McpCallContextError::Denied);
        }
        let (servers, generation) = crate::realm::read_own_mcp_config(&self.storage)
            .await
            .map_err(|_| McpCallContextError::Unavailable)?;
        if generation != self.generation || require_selected(&servers, &self.config).is_err() {
            return Err(McpCallContextError::Denied);
        }
        Ok(())
    }
    fn selected_call(
        &self,
        target: &McpCallTarget<'_>,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> bool {
        if target.config != &self.config
            || target.raw_operation != self.gate.profile.operation
            || call.name != self.gate.exposed
            || !context.read_only_execution_required()
            || context.origin_session_id() != Some(&self.gate.access.session)
            || context.work_authorization().is_none()
            || context.round().is_none()
            || call.args.get().len() > 32 * 1024
            || serde_json::from_str::<crate::forge::UniqueJson>(call.args.get()).is_err()
            || serde_json::from_str::<Value>(call.args.get()).ok().as_ref()
                != Some(&self.gate.profile.arguments)
            || !self.host.lifetime.current(self.host.binding.expires_at_ms)
        {
            return false;
        }
        self.gate.attempt.lock().is_ok_and(|attempt| {
            attempt.as_ref().is_some_and(|actual| {
                actual.call == call.id
                    && context.run_id() == Some(&actual.run)
                    && actual.outcome.is_none()
            })
        })
    }
}
#[async_trait]
impl McpCallContextProvider for Provider {
    fn tool_mutation_class(&self, config: &McpServerConfig, operation: &str) -> ToolMutationClass {
        if config != &self.config
            || operation != self.gate.profile.operation
            || !self.host.lifetime.current(self.host.binding.expires_at_ms)
        {
            return ToolMutationClass::Unknown;
        }
        classification(
            self.inner.tool_mutation_class(config, operation),
            self.declaration
                .as_ref()
                .is_some_and(|declaration| declaration.matches(config, operation)),
        )
    }
    async fn prepare(
        &self,
        target: McpCallTarget<'_>,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        if !self.selected_call(&target, call, context)
            || self.tool_mutation_class(target.config, target.raw_operation)
                != ToolMutationClass::ReadOnly
        {
            return Err(McpCallContextError::Denied);
        }
        self.recheck().await?;
        let actual_config = target.config;
        let operation = target.raw_operation;
        let prepared = self.inner.prepare(target, call, context).await?;
        self.recheck().await?;
        if self.tool_mutation_class(actual_config, operation) != ToolMutationClass::ReadOnly {
            return Err(McpCallContextError::Denied);
        }
        // Return the same opaque owner object, not with_final_check (which
        // would replace its existing credential final check). Native routing
        // retains that object's guard and final custody through transport.
        Ok(prepared)
    }
    async fn prepare_resource(
        &self,
        target: McpResourceTarget<'_>,
        context: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        self.inner.prepare_resource(target, context).await
    }
    fn resource_authorization_target(
        &self,
        target: McpResourceTarget<'_>,
    ) -> Option<meerkat_core::authorization::OwnerQualifiedTarget> {
        self.inner.resource_authorization_target(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_policy_requires_both_exact_name_and_positive_owner_class() {
        let native = meerkat_core::ToolExecutionPolicy::resolve(policy("selected", true)).unwrap();
        assert!(native.permits_call("selected", ToolMutationClass::ReadOnly));
        assert!(!native.permits_call("other", ToolMutationClass::ReadOnly));
        assert!(!native.permits_call("selected", ToolMutationClass::Unknown));
        assert!(!native.permits_call("selected", ToolMutationClass::Mutating));
        let ordinary =
            meerkat_core::ToolExecutionPolicy::resolve(policy("selected", false)).unwrap();
        assert!(ordinary.permits_call("selected", ToolMutationClass::Unknown));
        assert!(!ordinary.permits_call("other", ToolMutationClass::ReadOnly));
    }
    #[test]
    fn declaration_never_overrides_known_mutation_or_invents_absent_read_class() {
        for declared in [false, true] {
            assert_eq!(
                classification(ToolMutationClass::Mutating, declared),
                ToolMutationClass::Mutating
            );
            assert_eq!(
                classification(ToolMutationClass::ReadOnly, declared),
                ToolMutationClass::ReadOnly
            );
        }
        assert_eq!(
            classification(ToolMutationClass::Unknown, false),
            ToolMutationClass::Unknown
        );
        assert_eq!(
            classification(ToolMutationClass::Unknown, true),
            ToolMutationClass::ReadOnly
        );
    }
}
