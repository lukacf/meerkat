//! Reconstruct executable class callbacks through Meta's existing customizer.
//! Durable admission stays with the application policy owner, native runtime
//! and rebuilds stay with Meta, and the factory remains trusted host code.
use super::*;
use crate::installed_host::tool_policy::PreparedInstalledToolPolicy;
use meerkat_mobkit::SessionCreatedContext;
use meerkat_mobkit::identity_first::{
    AgentBuildContext, AgentBuildDraft, AgentCustomizer, AgentIdentity, CustomizerError,
    DurableAgentSpec, LocalExternalToolOverlay,
};
use std::sync::Arc;

pub(super) struct MemberClassCustomizer {
    pub policy: PreparedInstalledToolPolicy,
    pub inner: Arc<dyn AgentCustomizer>,
    pub factory: Option<Arc<dyn MemberClassFactory>>,
}

fn refused(_: HostLifetimeError) -> CustomizerError {
    CustomizerError::BuildFailed("current member class factory binding refused".into())
}

#[async_trait::async_trait]
impl AgentCustomizer for MemberClassCustomizer {
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        let admission = self
            .policy
            .member_class_build_admission(&context.identity)
            .map_err(refused)?;
        let Some(admission) = admission else {
            self.inner.customize_build(context, spec, draft).await?;
            if self
                .policy
                .member_class_build_admission(&context.identity)
                .map_err(refused)?
                .is_some()
            {
                return Err(refused(HostLifetimeError::Denied));
            }
            return Ok(());
        };
        if context.identity != spec.identity || spec.profile.as_str() != admission.class.profile {
            return Err(refused(HostLifetimeError::Denied));
        }
        self.inner.customize_build(context, spec, draft).await?;
        if self
            .policy
            .member_class_build_admission(&context.identity)
            .map_err(refused)?
            .as_ref()
            != Some(&admission)
        {
            return Err(refused(HostLifetimeError::Denied));
        }
        let supplied = match &self.factory {
            Some(factory) => {
                let tools = factory
                    .prepare(&admission.class, &admission.instance)
                    .map_err(refused)?;
                if tools.class() != &admission.class
                    || !draft.external_tools.is_empty()
                    || draft
                        .local_external_tools
                        .dispatcher()
                        .is_some_and(|other| !Arc::ptr_eq(&other, &tools.dispatcher()))
                {
                    return Err(refused(HostLifetimeError::InvalidSelection));
                }
                Some(tools)
            }
            None => match draft.local_external_tools.dispatcher() {
                Some(dispatcher) => Some(
                    PreparedMemberClassTools::from_factory(
                        &admission.class,
                        &admission.class.factory,
                        dispatcher,
                    )
                    .map_err(refused)?,
                ),
                None if admission.class.callbacks.is_empty() && draft.external_tools.is_empty() => {
                    None
                }
                None => return Err(refused(HostLifetimeError::InvalidSelection)),
            },
        };
        let identities = supplied
            .as_ref()
            .map_or(&[][..], PreparedMemberClassTools::identities);
        if admission
            .installed_surface
            .as_deref()
            .is_some_and(|prior| prior != identities)
        {
            return Err(refused(HostLifetimeError::InvalidSelection));
        }
        if self
            .policy
            .member_class_build_admission(&context.identity)
            .map_err(refused)?
            .as_ref()
            != Some(&admission)
        {
            return Err(refused(HostLifetimeError::Denied));
        }
        self.policy
            .bind_member_class_surface(&admission, identities)
            .await
            .map_err(refused)?;
        if let Some(tools) = supplied {
            let bundle = self
                .policy
                .member_class_callback_bundle(&admission)
                .map_err(refused)?;
            let handle = context
                .runtime_services
                .mob_handle()
                .ok_or_else(|| refused(HostLifetimeError::Unavailable))?;
            // A host namespace is not reserved by Core. Check the actual
            // selected definition, never a model-authored draft label.
            if handle
                .definition()
                .resolve_inline_profile(&meerkat_mob::ProfileName::from(
                    admission.class.profile.clone(),
                ))
                .is_none()
            {
                return Err(refused(HostLifetimeError::InvalidSelection));
            }
            if handle
                .definition()
                .profiles
                .values()
                .filter_map(|p| p.as_inline())
                .any(|p| p.tools.rust_bundles.contains(&bundle))
            {
                return Err(refused(HostLifetimeError::InvalidSelection));
            }
            let dispatcher = super::callbacks::ClassCallbackDispatcher::new(
                tools.dispatcher(),
                bundle,
                &admission.class,
            )
            .map_err(refused)?;
            draft.local_external_tools = LocalExternalToolOverlay::new(Arc::new(dispatcher));
        }
        Ok(())
    }

    async fn after_create(
        &self,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        self.inner.after_create(identity, session_id, context).await
    }
}
