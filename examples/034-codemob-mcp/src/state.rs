use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use meerkat::{build_ephemeral_service, AgentFactory, FactoryAgentBuilder};
use meerkat_core::config::Config;
use meerkat_mob::MobControlPrincipal;
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
/// tool. Production deployments use the runtime-backed path instead; this
/// example intentionally stays in explicit standalone mode.
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
    mobs_dir: PathBuf,
    /// Keep temp dir alive for the session store.
    #[allow(dead_code)]
    store_dir: tempfile::TempDir,
}

impl ForceState {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::in_workspace(std::env::current_dir()?)
    }

    pub(crate) fn in_workspace(workspace: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let scratch = workspace.join(".codemob-mcp");
        std::fs::create_dir_all(&scratch)?;
        let store_dir = tempfile::Builder::new()
            .prefix("sessions-")
            .tempdir_in(&scratch)?;
        let store_path = store_dir.path().join("sessions");
        std::fs::create_dir_all(&store_path)?;

        let factory = workspace_factory(&store_path, &workspace);
        let config = Config::default();
        let session_service: Arc<SessionSvc> =
            Arc::new(build_ephemeral_service(factory, config, 64));

        let mob_state = Arc::new(MobMcpState::new(
            session_service.clone(),
            MobControlPrincipal::Owner,
        ));
        let mut pack_registry = PackRegistry::new();
        // Load user packs from disk on startup
        let mobs_dir = workspace.join(".codemob-mcp/mobs");
        for pack in mobs::load_user_packs(&mobs_dir) {
            pack_registry.register(pack);
        }

        Ok(Self {
            session_service,
            mob_state,
            pack_registry: RwLock::new(pack_registry),
            mobs_dir,
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
        let user_packs = mobs::load_user_packs(&self.mobs_dir);

        let mut registry = self
            .pack_registry
            .write()
            .expect("pack_registry lock poisoned");

        *registry = PackRegistry::new();

        // Re-add user packs from disk
        for pack in user_packs {
            registry.register(pack);
        }
    }
}

pub(crate) fn workspace_factory(store_path: &Path, workspace: &Path) -> AgentFactory {
    AgentFactory::new(store_path)
        .project_root(workspace.to_path_buf())
        .comms(true)
        .memory(true)
}

#[cfg(test)]
impl ForceState {
    pub(crate) fn with_test_client(
        workspace: &Path,
        client: Arc<dyn meerkat_client::LlmClient>,
    ) -> Self {
        let mut state = Self::in_workspace(workspace.to_path_buf()).unwrap();
        let factory =
            workspace_factory(&state.store_dir.path().join("sessions"), workspace).memory(false);
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(client.clone());
        state.session_service = Arc::new(EphemeralSessionService::new(builder, 64));
        state.mob_state = Arc::new(
            MobMcpState::new(state.session_service.clone(), MobControlPrincipal::Owner)
                .with_default_llm_client(Some(client)),
        );
        state
    }
}

// ── Skill resolution ───────────────────────────────────────────────────────

/// Search directories for `.claude/skills/` collections.
fn skill_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // Project-level (CWD)
    dirs.push(PathBuf::from(".claude/skills"));
    // User-level
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".claude/skills"));
    }
    dirs
}

/// Read a SKILL.md and all `references/*.md` files, concatenated.
fn read_skill_dir(skill_dir: &Path) -> Option<String> {
    let skill_md = skill_dir.join("SKILL.md");
    let mut content = std::fs::read_to_string(&skill_md).ok()?;

    let refs_dir = skill_dir.join("references");
    if refs_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&refs_dir) {
            let mut ref_files: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
                .collect();
            ref_files.sort_by_key(|e| e.file_name());
            for entry in ref_files {
                if let Ok(ref_content) = std::fs::read_to_string(entry.path()) {
                    content.push_str("\n\n---\n\n");
                    content.push_str(&ref_content);
                }
            }
        }
    }
    Some(content)
}

/// Resolve skill names to their rendered content.
///
/// Searches project-level `.claude/skills/` then user-level `~/.claude/skills/`.
/// Returns the content for each found skill; silently skips unknown names.
///
/// NOTE: No path sanitization — skill names are joined directly onto the search
/// directories. This is safe in a trusted-caller MCP environment (the calling
/// agent controls the input). Do not expose to untrusted clients without adding
/// validation (e.g. reject names containing `/` or `..`).
pub fn resolve_skills(names: &[String]) -> Vec<String> {
    let dirs = skill_search_dirs();
    names
        .iter()
        .filter_map(|name| {
            for dir in &dirs {
                let skill_dir = dir.join(name);
                if let Some(content) = read_skill_dir(&skill_dir) {
                    return Some(content);
                }
            }
            tracing::warn!(skill = %name, "skill not found in any search path");
            None
        })
        .collect()
}
