//! Pure system prompt rendering and AGENTS.md section support.
//!
//! Filesystem discovery and loading are owned by outer surfaces. Core only
//! receives already-loaded instruction content and renders it deterministically.

/// Default system prompt for Meerkat agents
pub const DEFAULT_SYSTEM_PROMPT: &str = r"You are an autonomous agent. Your task is to accomplish the user's goal by systematically using the tools available to you.

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
- If the task cannot be completed, explain what blocked progress and what was attempted.";

/// Maximum size for AGENTS.md files (32 KiB, matching Codex default)
pub const AGENTS_MD_MAX_BYTES: usize = 32 * 1024;

/// Already-loaded AGENTS.md content plus its display source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentsMdSection {
    source: String,
    content: String,
}

/// Configuration for pure system prompt composition.
#[derive(Debug, Clone, Default)]
pub struct SystemPromptConfig {
    /// Custom system prompt (overrides default if set)
    pub system_prompt: Option<String>,
    agents_md_sections: Vec<AgentsMdSection>,
}

impl SystemPromptConfig {
    /// Create a new config with defaults.
    pub fn new() -> Self {
        Self {
            system_prompt: None,
            agents_md_sections: Vec::new(),
        }
    }

    /// Set a custom system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Clear caller-supplied AGENTS.md sections.
    pub fn without_agents_md(mut self) -> Self {
        self.agents_md_sections.clear();
        self
    }

    /// Add already-loaded global AGENTS.md content.
    pub fn with_global_agents_md_content(self, content: impl Into<String>) -> Self {
        self.with_agents_md_section("global AGENTS.md", content)
    }

    /// Add already-loaded project AGENTS.md content.
    pub fn with_project_agents_md_content(self, content: impl Into<String>) -> Self {
        self.with_agents_md_section("AGENTS.md", content)
    }

    /// Add an already-loaded AGENTS.md section with a display source.
    pub fn with_agents_md_section(
        mut self,
        source: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        if let Some(content) = normalize_agents_md_content(&content.into()) {
            self.agents_md_sections.push(AgentsMdSection {
                source: source.into(),
                content,
            });
        }
        self
    }

    /// Compose the final system prompt.
    pub fn compose_sync(&self) -> String {
        let base = self
            .system_prompt
            .as_deref()
            .unwrap_or(DEFAULT_SYSTEM_PROMPT);

        let mut parts = vec![base.to_string()];
        for section in &self.agents_md_sections {
            parts.push(format!(
                "\n# Project Instructions (from {})\n\n{}",
                section.source, section.content
            ));
        }

        parts.join("\n")
    }

    /// Compose the final system prompt.
    ///
    /// This remains async for API compatibility with earlier filesystem-backed
    /// composition, but core rendering is synchronous and side-effect free.
    pub async fn compose(&self) -> String {
        self.compose_sync()
    }
}

/// Normalize already-loaded AGENTS.md content for prompt inclusion.
pub fn normalize_agents_md_content(content: &str) -> Option<String> {
    if content.trim().is_empty() {
        return None;
    }

    if content.len() > AGENTS_MD_MAX_BYTES {
        return Some(content.chars().take(AGENTS_MD_MAX_BYTES).collect());
    }

    Some(content.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_default_prompt_used_when_no_override() {
        let config = SystemPromptConfig::new().without_agents_md();
        let prompt = config.compose().await;
        assert_eq!(prompt, DEFAULT_SYSTEM_PROMPT);
    }

    #[tokio::test]
    async fn test_core_prompt_does_not_read_process_filesystem() {
        let prompt = SystemPromptConfig::new().compose().await;

        assert!(
            !prompt.contains("BuildBuddy"),
            "core prompt composition must not read the repository AGENTS.md from cwd"
        );
    }

    #[tokio::test]
    async fn test_custom_prompt_overrides_default() {
        let config = SystemPromptConfig::new().with_system_prompt("Custom prompt");
        let prompt = config.compose().await;
        assert_eq!(prompt, "Custom prompt");
    }

    #[tokio::test]
    async fn test_project_agents_md_content_injected() {
        let config = SystemPromptConfig::new()
            .with_project_agents_md_content("# Build Instructions\nRun `make build`");

        let prompt = config.compose().await;
        assert!(prompt.contains(DEFAULT_SYSTEM_PROMPT));
        assert!(prompt.contains("# Project Instructions (from AGENTS.md)"));
        assert!(prompt.contains("# Build Instructions"));
        assert!(prompt.contains("Run `make build`"));
    }

    #[tokio::test]
    async fn test_global_agents_md_content_injected() {
        let config = SystemPromptConfig::new()
            .with_global_agents_md_content("# Global rules\nAlways be nice");

        let prompt = config.compose().await;
        assert!(prompt.contains("# Project Instructions (from global AGENTS.md)"));
        assert!(prompt.contains("# Global rules"));
    }

    #[tokio::test]
    async fn test_agents_md_sections_preserve_order() {
        let config = SystemPromptConfig::new()
            .with_global_agents_md_content("Global instructions")
            .with_project_agents_md_content("Project instructions");

        let prompt = config.compose().await;
        let global_pos = prompt.find("Global instructions").unwrap();
        let project_pos = prompt.find("Project instructions").unwrap();
        assert!(global_pos < project_pos);
    }

    #[tokio::test]
    async fn test_empty_agents_md_ignored() {
        let config = SystemPromptConfig::new().with_project_agents_md_content("   \n\n  ");

        let prompt = config.compose().await;
        assert!(!prompt.contains("Project Instructions"));
    }

    #[tokio::test]
    async fn test_agents_md_size_limit() {
        let large_content = "x".repeat(AGENTS_MD_MAX_BYTES + 1000);
        let config = SystemPromptConfig::new().with_project_agents_md_content(&large_content);

        let prompt = config.compose().await;
        let agents_section_start = prompt.find("# Project Instructions").unwrap();
        let agents_content = &prompt[agents_section_start..];
        assert!(agents_content.len() <= AGENTS_MD_MAX_BYTES + 100);
    }

    #[test]
    fn test_normalize_agents_md_content_rejects_empty() {
        assert_eq!(normalize_agents_md_content(" \n\t "), None);
    }
}
