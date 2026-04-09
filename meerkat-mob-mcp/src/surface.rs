use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use meerkat_core::service::MobToolsFactory;
use meerkat_mob::MobSessionService;

use crate::{AgentMobToolSurfaceFactory, MobMcpState};

pub fn wire_mob_tools(
    builder_mob_tools_slot: &Arc<RwLock<Option<Arc<dyn MobToolsFactory>>>>,
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<meerkat_runtime::RuntimeSessionAdapter>>,
    persistent_storage_root: Option<PathBuf>,
) -> Arc<MobMcpState> {
    let state = Arc::new(
        MobMcpState::new_with_runtime_adapter(session_service, runtime_adapter)
            .with_persistent_storage_root(persistent_storage_root),
    );
    *builder_mob_tools_slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
        AgentMobToolSurfaceFactory::new(Arc::clone(&state)),
    ));
    state
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use meerkat_mob::MobSessionService;

    #[test]
    fn wire_mob_tools_installs_factory_and_returns_state() {
        let slot: Arc<RwLock<Option<Arc<dyn MobToolsFactory>>>> = Arc::new(RwLock::new(None));
        let session_service: Arc<dyn MobSessionService> =
            Arc::new(crate::LocalSessionService::new());
        let runtime_adapter = Some(Arc::new(meerkat_runtime::RuntimeSessionAdapter::ephemeral()));

        let state = wire_mob_tools(&slot, session_service, runtime_adapter, None);

        assert!(
            slot.read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some(),
            "mob tools slot should be populated"
        );
        assert!(
            state.runtime_adapter.is_some(),
            "returned mob state should preserve the runtime adapter"
        );
    }

    #[test]
    fn wire_mob_tools_preserves_persistent_storage_root() {
        let slot: Arc<RwLock<Option<Arc<dyn MobToolsFactory>>>> = Arc::new(RwLock::new(None));
        let session_service: Arc<dyn MobSessionService> =
            Arc::new(crate::LocalSessionService::new());
        let root = PathBuf::from("/tmp/meerkat-mob-root");

        let state = wire_mob_tools(
            &slot,
            session_service,
            Some(Arc::new(meerkat_runtime::RuntimeSessionAdapter::ephemeral())),
            Some(root.clone()),
        );

        assert_eq!(
            state.persistent_storage_root.as_deref(),
            Some(crate::MobMcpState::persistent_mob_root(&root).as_path())
        );
    }
}
