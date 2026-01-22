//! System prompt configuration and AGENTS.md support
//!
//! Provides configurable system prompts with support for:
//! - Default system prompt
//! - Custom system prompt override
//! - AGENTS.md file injection (global + project)
//!
//! AGENTS.md discovery:
//! - Global: ~/.rkat/AGENTS.md
//! - Project: ./AGENTS.md or ./.rkat/AGENTS.md
//!
//! The final prompt is composed as:
//! 1. System prompt (custom or default)
//! 2. AGENTS.md content (if found), wrapped in a marker

use std::fs;
use std::path::{Path, PathBuf};

/// Default system prompt for Meerkat agents
pub const DEFAULT_SYSTEM_PROMPT: &str = r#"You are an autonomous agent. Your task is to accomplish the user's goal by systematically using the tools available to you.

# Core Behavior
- Break complex tasks into steps and execute them one by one.
- Use tools to gather information, take actions, and verify results.
- When multiple tool calls are independent, execute them in parallel.
- If a tool call fails, analyze the error and try alternative approaches.
- Continue working until the task is complete or you determine it cannot be completed.

# Decision Making
- Act on the information you have. Make reasonable assumptions when necessary.
- If critical information is missing and no tool can provide it, state what you need and why.
- Prioritize correctness over speed. Verify your work when possible.

# Output
- When the task is complete, provide a clear summary of what was accomplished.
- If the task cannot be completed, explain what blocked progress and what was attempted."#;

/// Maximum size for AGENTS.md files (32 KiB, matching Codex default)
pub const AGENTS_MD_MAX_BYTES: usize = 32 * 1024;

/// Configuration for system prompt composition
#[derive(Debug, Clone, Default)]
pub struct SystemPromptConfig {
    /// Custom system prompt (overrides default if set)
    pub system_prompt: Option<String>,
    /// Whether to load AGENTS.md files
    pub load_agents_md: bool,
    /// Custom path to global AGENTS.md (defaults to ~/.rkat/AGENTS.md)
    pub global_agents_md_path: Option<PathBuf>,
    /// Custom path to project AGENTS.md (defaults to ./AGENTS.md or ./.rkat/AGENTS.md)
    pub project_agents_md_path: Option<PathBuf>,
}

impl SystemPromptConfig {
    /// Create a new config with defaults
    pub fn new() -> Self {
        Self {
            system_prompt: None,
            load_agents_md: true,
            global_agents_md_path: None,
            project_agents_md_path: None,
        }
    }

    /// Set a custom system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Disable AGENTS.md loading
    pub fn without_agents_md(mut self) -> Self {
        self.load_agents_md = false;
        self
    }

    /// Set custom global AGENTS.md path
    pub fn with_global_agents_md(mut self, path: impl Into<PathBuf>) -> Self {
        self.global_agents_md_path = Some(path.into());
        self
    }

    /// Set custom project AGENTS.md path
    pub fn with_project_agents_md(mut self, path: impl Into<PathBuf>) -> Self {
        self.project_agents_md_path = Some(path.into());
        self
    }

    /// Compose the final system prompt
    pub fn compose(&self) -> String {
        let base = self
            .system_prompt
            .as_deref()
            .unwrap_or(DEFAULT_SYSTEM_PROMPT);

        if !self.load_agents_md {
            return base.to_string();
        }

        let mut parts = vec![base.to_string()];

        // Load global AGENTS.md
        if let Some(content) = self.load_global_agents_md() {
            parts.push(format!(
                "\n# Project Instructions (from global AGENTS.md)\n\n{}",
                content
            ));
        }

        // Load project AGENTS.md
        if let Some(content) = self.load_project_agents_md() {
            parts.push(format!(
                "\n# Project Instructions (from AGENTS.md)\n\n{}",
                content
            ));
        }

        parts.join("\n")
    }

    fn load_global_agents_md(&self) -> Option<String> {
        let path = self
            .global_agents_md_path
            .clone()
            .or_else(default_global_agents_md_path)?;
        load_agents_md_file(&path)
    }

    fn load_project_agents_md(&self) -> Option<String> {
        let path = self
            .project_agents_md_path
            .clone()
            .or_else(find_project_agents_md)?;
        load_agents_md_file(&path)
    }
}

/// Get the default global AGENTS.md path: ~/.rkat/AGENTS.md
pub fn default_global_agents_md_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join(".rkat/AGENTS.md"))
}

/// Find project AGENTS.md in current directory
/// Checks: ./AGENTS.md, ./.rkat/AGENTS.md
pub fn find_project_agents_md() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_project_agents_md_in(&cwd)
}

/// Find project AGENTS.md in a specific directory
/// Checks: <dir>/AGENTS.md, <dir>/.rkat/AGENTS.md
pub fn find_project_agents_md_in(dir: &Path) -> Option<PathBuf> {
    // Check ./AGENTS.md first
    let root_path = dir.join("AGENTS.md");
    if root_path.exists() {
        return Some(root_path);
    }

    // Check ./.rkat/AGENTS.md
    let meerkat_path = dir.join(".rkat/AGENTS.md");
    if meerkat_path.exists() {
        return Some(meerkat_path);
    }

    None
}

/// Load an AGENTS.md file, respecting size limits
fn load_agents_md_file(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }

    let content = fs::read_to_string(path).ok()?;

    // Skip empty files
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Enforce size limit
    if content.len() > AGENTS_MD_MAX_BYTES {
        // Truncate to limit
        let truncated: String = content.chars().take(AGENTS_MD_MAX_BYTES).collect();
        return Some(truncated);
    }

    Some(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_prompt_used_when_no_override() {
        let config = SystemPromptConfig::new().without_agents_md();
        let prompt = config.compose();
        assert_eq!(prompt, DEFAULT_SYSTEM_PROMPT);
    }

    #[test]
    fn test_custom_prompt_overrides_default() {
        let config = SystemPromptConfig::new()
            .with_system_prompt("Custom prompt")
            .without_agents_md();
        let prompt = config.compose();
        assert_eq!(prompt, "Custom prompt");
    }

    #[test]
    fn test_agents_md_disabled() {
        let temp = TempDir::new().unwrap();
        let agents_path = temp.path().join("AGENTS.md");
        fs::write(&agents_path, "# Should not appear").unwrap();

        let config = SystemPromptConfig::new()
            .with_project_agents_md(&agents_path)
            .without_agents_md();

        let prompt = config.compose();
        assert!(!prompt.contains("Should not appear"));
    }

    #[test]
    fn test_project_agents_md_injected() {
        let temp = TempDir::new().unwrap();
        let agents_path = temp.path().join("AGENTS.md");
        fs::write(&agents_path, "# Build Instructions\nRun `make build`").unwrap();

        let config = SystemPromptConfig::new().with_project_agents_md(&agents_path);

        let prompt = config.compose();
        assert!(prompt.contains(DEFAULT_SYSTEM_PROMPT));
        assert!(prompt.contains("# Project Instructions (from AGENTS.md)"));
        assert!(prompt.contains("# Build Instructions"));
        assert!(prompt.contains("Run `make build`"));
    }

    #[test]
    fn test_global_agents_md_injected() {
        let temp = TempDir::new().unwrap();
        let global_path = temp.path().join("global-agents.md");
        fs::write(&global_path, "# Global rules\nAlways be nice").unwrap();

        let config = SystemPromptConfig::new().with_global_agents_md(&global_path);

        let prompt = config.compose();
        assert!(prompt.contains("# Project Instructions (from global AGENTS.md)"));
        assert!(prompt.contains("# Global rules"));
    }

    #[test]
    fn test_both_global_and_project_agents_md() {
        let temp = TempDir::new().unwrap();

        let global_path = temp.path().join("global.md");
        fs::write(&global_path, "Global instructions").unwrap();

        let project_path = temp.path().join("project.md");
        fs::write(&project_path, "Project instructions").unwrap();

        let config = SystemPromptConfig::new()
            .with_global_agents_md(&global_path)
            .with_project_agents_md(&project_path);

        let prompt = config.compose();

        // Global should come before project
        let global_pos = prompt.find("Global instructions").unwrap();
        let project_pos = prompt.find("Project instructions").unwrap();
        assert!(global_pos < project_pos);
    }

    #[test]
    fn test_empty_agents_md_ignored() {
        let temp = TempDir::new().unwrap();
        let agents_path = temp.path().join("AGENTS.md");
        fs::write(&agents_path, "   \n\n  ").unwrap(); // whitespace only

        let config = SystemPromptConfig::new().with_project_agents_md(&agents_path);

        let prompt = config.compose();
        assert!(!prompt.contains("Project Instructions"));
    }

    #[test]
    fn test_agents_md_size_limit() {
        let temp = TempDir::new().unwrap();
        let agents_path = temp.path().join("AGENTS.md");

        // Create content larger than limit
        let large_content = "x".repeat(AGENTS_MD_MAX_BYTES + 1000);
        fs::write(&agents_path, &large_content).unwrap();

        let config = SystemPromptConfig::new().with_project_agents_md(&agents_path);

        let prompt = config.compose();

        // Should be truncated
        let agents_section_start = prompt.find("# Project Instructions").unwrap();
        let agents_content = &prompt[agents_section_start..];
        assert!(agents_content.len() <= AGENTS_MD_MAX_BYTES + 100); // some buffer for header
    }

    #[test]
    fn test_find_project_agents_md_root() {
        let temp = TempDir::new().unwrap();
        let agents_path = temp.path().join("AGENTS.md");
        fs::write(&agents_path, "content").unwrap();

        let found = find_project_agents_md_in(temp.path());
        assert_eq!(found, Some(agents_path));
    }

    #[test]
    fn test_find_project_agents_md_in_meerkat_dir() {
        let temp = TempDir::new().unwrap();
        let meerkat_dir = temp.path().join(".rkat");
        fs::create_dir_all(&meerkat_dir).unwrap();
        let agents_path = meerkat_dir.join("AGENTS.md");
        fs::write(&agents_path, "content").unwrap();

        let found = find_project_agents_md_in(temp.path());
        assert_eq!(found, Some(agents_path));
    }

    #[test]
    fn test_find_project_agents_md_root_takes_precedence() {
        let temp = TempDir::new().unwrap();

        // Create both
        let root_path = temp.path().join("AGENTS.md");
        fs::write(&root_path, "root content").unwrap();

        let meerkat_dir = temp.path().join(".rkat");
        fs::create_dir_all(&meerkat_dir).unwrap();
        fs::write(meerkat_dir.join("AGENTS.md"), "test content").unwrap();

        // Root should win
        let found = find_project_agents_md_in(temp.path());
        assert_eq!(found, Some(root_path));
    }

    #[test]
    fn test_missing_agents_md_returns_none() {
        let temp = TempDir::new().unwrap();
        let found = find_project_agents_md_in(temp.path());
        assert_eq!(found, None);
    }
}
