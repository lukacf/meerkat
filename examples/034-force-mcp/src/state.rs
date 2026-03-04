use std::sync::Arc;

use meerkat::{AgentFactory, FactoryAgentBuilder, build_ephemeral_service};
use meerkat_core::config::Config;
use meerkat_mob_mcp::MobMcpState;
use meerkat_session::EphemeralSessionService;

use crate::packs::PackRegistry;

type SessionSvc = EphemeralSessionService<FactoryAgentBuilder>;

/// Global server state — created once at startup.
pub struct ForceState {
    /// Session service for direct single-agent use (consult tool).
    pub session_service: Arc<SessionSvc>,
    /// Mob state manager for multi-agent orchestration (deliberate tool).
    pub mob_state: Arc<MobMcpState>,
    /// Registry of available pack definitions.
    pub pack_registry: PackRegistry,
    /// Keep temp dir alive for the session store.
    #[allow(dead_code)]
    store_dir: tempfile::TempDir,
}

impl ForceState {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let store_dir = tempfile::tempdir()?;
        let store_path = store_dir.path().join("sessions");
        std::fs::create_dir_all(&store_path)?;

        let factory = AgentFactory::new(&store_path).comms(true);
        let config = Config::default();
        let session_service: Arc<SessionSvc> =
            Arc::new(build_ephemeral_service(factory, config, 64));

        let mob_state = Arc::new(MobMcpState::new(session_service.clone()));
        let pack_registry = PackRegistry::new();

        Ok(Self {
            session_service,
            mob_state,
            pack_registry,
            store_dir,
        })
    }
}
