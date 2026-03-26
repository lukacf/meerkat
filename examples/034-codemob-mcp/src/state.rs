use std::sync::{Arc, RwLock};

use meerkat::{AgentFactory, FactoryAgentBuilder, build_ephemeral_service};
use meerkat_core::config::Config;
use meerkat_mob_mcp::MobMcpState;
use meerkat_session::EphemeralSessionService;

use crate::packs::PackRegistry;
use crate::tools::mobs;

type SessionSvc = EphemeralSessionService<FactoryAgentBuilder>;

/// Global server state — lazy-initialized on first tool call.
///
/// Both `session_service` and `mob_state` share the same underlying
/// `EphemeralSessionService` (in-memory substrate). The `session_service`
/// field provides direct access for the `consult` tool (single-agent, no mob).
/// The `mob_state` wraps it for multi-agent orchestration in the `deliberate`
/// tool. Production deployments use the runtime-backed path instead.
///
/// API keys are read from environment variables by `Config::default()`:
/// `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`.
pub struct ForceState {
    /// Session service for direct single-agent use (consult tool).
    pub session_service: Arc<SessionSvc>,
    /// Mob state manager for multi-agent orchestration (deliberate tool).
    pub mob_state: Arc<MobMcpState>,
    /// Registry of available pack definitions (built-in + user-created).
    pack_registry: RwLock<PackRegistry>,
    /// Names of built-in packs (never removed by reload).
    builtin_names: Vec<String>,
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
        let mut pack_registry = PackRegistry::new();
        let builtin_names: Vec<String> =
            pack_registry.list_names().iter().map(|s| s.to_string()).collect();

        // Load user packs from disk on startup
        let mobs_dir = std::path::PathBuf::from(".codemob-mcp/mobs");
        for pack in mobs::load_user_packs(&mobs_dir) {
            pack_registry.register(pack);
        }

        Ok(Self {
            session_service,
            mob_state,
            pack_registry: RwLock::new(pack_registry),
            builtin_names,
            store_dir,
        })
    }

    /// Access the pack registry (read lock).
    pub fn pack_registry(&self) -> std::sync::RwLockReadGuard<'_, PackRegistry> {
        self.pack_registry
            .read()
            .expect("pack_registry lock poisoned")
    }

    /// Reload user packs from disk, preserving built-in packs.
    pub fn reload_user_packs(&self) {
        let mobs_dir = std::path::PathBuf::from(".codemob-mcp/mobs");
        let user_packs = mobs::load_user_packs(&mobs_dir);

        let mut registry = self
            .pack_registry
            .write()
            .expect("pack_registry lock poisoned");

        // Remove old user packs (anything not in builtin_names)
        let to_remove: Vec<String> = registry
            .list_names()
            .iter()
            .filter(|name| !self.builtin_names.contains(&name.to_string()))
            .map(|s| s.to_string())
            .collect();
        for name in to_remove {
            registry.remove(&name);
        }

        // Re-add user packs from disk
        for pack in user_packs {
            registry.register(pack);
        }
    }
}
