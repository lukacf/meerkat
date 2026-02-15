//! Unified system prompt assembly.
//!
//! Single canonical path for building the final system prompt with clear
//! precedence rules:
//!
//! 1. Per-request override (via `per_request_prompt`) — wins outright.
//! 2. Config-level file override (`config.agent.system_prompt_file`).
//! 3. Config-level inline override (`config.agent.system_prompt`).
//! 4. Default system prompt + AGENTS.md files.
//! 5. Config-level tool instructions (`config.agent.tool_instructions`).
//! 6. Dispatcher-provided tool usage instructions (appended last).

use meerkat_core::{Config, SystemPromptConfig};
use std::path::Path;

/// Assemble the final system prompt. Single canonical path.
///
/// `per_request_prompt` is the per-request override from `AgentBuildConfig.system_prompt`.
/// `extra_sections` is a forward-compatible slot for additional content
/// (e.g., skill inventory in Phase 3). For now callers pass `&[]`.
pub async fn assemble_system_prompt(
    config: &Config,
    per_request_prompt: Option<&str>,
    context_root: Option<&Path>,
    extra_sections: &[&str],
    tool_usage_instructions: &str,
) -> String {
    // 1. Per-request override wins outright (skips AGENTS.md and config).
    if let Some(prompt) = per_request_prompt {
        return append_sections(prompt, extra_sections, &[], tool_usage_instructions);
    }

    // 2-4. Use SystemPromptConfig to compose the base prompt.
    //
    // Wire config overrides INTO SystemPromptConfig rather than bypassing it.
    // This preserves AGENTS.md loading when config-level overrides are set —
    // a user setting `agent.system_prompt` in config wants to override the
    // DEFAULT prompt, not lose AGENTS.md content.
    let mut spc = SystemPromptConfig::new();

    // 2. Config-level file override → feeds into SystemPromptConfig.
    if let Some(ref path) = config.agent.system_prompt_file {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => spc.system_prompt = Some(content),
            Err(e) => tracing::warn!("system_prompt_file not readable: {}: {e}", path.display()),
        }
    }

    // 3. Config-level inline override (lower precedence than file).
    if spc.system_prompt.is_none() {
        if let Some(ref prompt) = config.agent.system_prompt {
            spc.system_prompt = Some(prompt.clone());
        }
    }

    // AGENTS.md is resolved only from explicit context roots.
    if let Some(context) = context_root {
        if let Some(project_agents) = meerkat_core::prompt::find_project_agents_md_in(context) {
            spc = spc.with_project_agents_md(project_agents);
        } else {
            spc = spc.with_project_agents_md(context.join("AGENTS.md"));
        }
        // Disable global AGENTS.md lookup.
        spc = spc.with_global_agents_md(context.join(".rkat/__disabled_global_agents__.md"));
    } else {
        spc = spc.without_agents_md();
    }

    // 4. compose() uses the override if set, otherwise DEFAULT_SYSTEM_PROMPT.
    //    Either way, AGENTS.md files are appended (global + project).
    let base = spc.compose().await;

    // 5. Append config-level tool instructions (if any) before dispatcher instructions.
    let config_tool_sections: Vec<&str> = config
        .agent
        .tool_instructions
        .as_deref()
        .into_iter()
        .collect();

    append_sections(
        &base,
        extra_sections,
        &config_tool_sections,
        tool_usage_instructions,
    )
}

/// Append extra sections and tool instructions to a base prompt.
fn append_sections(
    base: &str,
    extra_sections: &[&str],
    config_tool_sections: &[&str],
    tool_instructions: &str,
) -> String {
    let mut prompt = base.to_string();
    for section in extra_sections {
        if !section.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(section);
        }
    }
    for section in config_tool_sections {
        if !section.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(section);
        }
    }
    if !tool_instructions.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(tool_instructions);
    }
    prompt
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::prompt::DEFAULT_SYSTEM_PROMPT;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn default_config() -> Config {
        Config::default()
    }

    // --- Precedence level 1: per-request override ---

    #[tokio::test]
    async fn test_per_request_override_wins() {
        let config = default_config();
        let result =
            assemble_system_prompt(&config, Some("Per-request prompt"), None, &[], "").await;
        assert_eq!(result, "Per-request prompt");
    }

    #[tokio::test]
    async fn test_per_request_override_skips_config_fields() {
        let mut config = default_config();
        config.agent.system_prompt = Some("Config inline prompt".to_string());
        config.agent.tool_instructions = Some("Config tool instructions".to_string());

        let result =
            assemble_system_prompt(&config, Some("Per-request prompt"), None, &[], "").await;
        // Per-request override skips config.agent.system_prompt and tool_instructions
        assert_eq!(result, "Per-request prompt");
        assert!(!result.contains("Config inline"));
        assert!(!result.contains("Config tool instructions"));
    }

    #[tokio::test]
    async fn test_per_request_override_still_appends_dispatcher_tools() {
        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            Some("Per-request prompt"),
            None,
            &[],
            "Dispatcher tool instructions",
        )
        .await;
        assert!(result.starts_with("Per-request prompt"));
        assert!(result.contains("Dispatcher tool instructions"));
    }

    // --- Precedence level 2: config file override ---

    #[tokio::test]
    async fn test_config_file_override() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("prompt.txt");
        tokio::fs::write(&file_path, "File-based prompt")
            .await
            .unwrap();

        let mut config = default_config();
        config.agent.system_prompt_file = Some(file_path);

        let result = assemble_system_prompt(&config, None, None, &[], "").await;
        assert!(result.contains("File-based prompt"));
        assert!(!result.contains(DEFAULT_SYSTEM_PROMPT));
    }

    #[tokio::test]
    async fn test_config_file_beats_inline() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("prompt.txt");
        tokio::fs::write(&file_path, "File-based prompt")
            .await
            .unwrap();

        let mut config = default_config();
        config.agent.system_prompt_file = Some(file_path);
        config.agent.system_prompt = Some("Inline prompt".to_string());

        let result = assemble_system_prompt(&config, None, None, &[], "").await;
        assert!(result.contains("File-based prompt"));
        assert!(!result.contains("Inline prompt"));
    }

    #[tokio::test]
    async fn test_config_file_unreadable_falls_through() {
        let mut config = default_config();
        config.agent.system_prompt_file = Some(PathBuf::from("/nonexistent/path/prompt.txt"));
        config.agent.system_prompt = Some("Inline fallback".to_string());

        let result = assemble_system_prompt(&config, None, None, &[], "").await;
        // File unreadable → falls through to inline
        assert!(result.contains("Inline fallback"));
    }

    // --- Precedence level 3: config inline override ---

    #[tokio::test]
    async fn test_config_inline_override() {
        let mut config = default_config();
        config.agent.system_prompt = Some("Inline prompt".to_string());

        let result = assemble_system_prompt(&config, None, None, &[], "").await;
        assert!(result.contains("Inline prompt"));
        assert!(!result.contains(DEFAULT_SYSTEM_PROMPT));
    }

    // --- Precedence level 4: default prompt ---

    #[tokio::test]
    async fn test_default_prompt_when_no_overrides() {
        let config = default_config();
        let result = assemble_system_prompt(&config, None, None, &[], "").await;
        assert!(result.contains(DEFAULT_SYSTEM_PROMPT));
    }

    // --- Precedence level 5: config tool instructions ---

    #[tokio::test]
    async fn test_config_tool_instructions_appended() {
        let mut config = default_config();
        config.agent.tool_instructions = Some("Use tools carefully".to_string());

        let result = assemble_system_prompt(&config, None, None, &[], "").await;
        assert!(result.contains("Use tools carefully"));
    }

    #[tokio::test]
    async fn test_config_tool_instructions_before_dispatcher() {
        let mut config = default_config();
        config.agent.tool_instructions = Some("Config tools".to_string());

        let result = assemble_system_prompt(&config, None, None, &[], "Dispatcher tools").await;
        let config_pos = result.find("Config tools").unwrap();
        let dispatcher_pos = result.find("Dispatcher tools").unwrap();
        assert!(
            config_pos < dispatcher_pos,
            "Config tool instructions should come before dispatcher tool instructions"
        );
    }

    // --- Precedence level 6: dispatcher tool instructions ---

    #[tokio::test]
    async fn test_dispatcher_tool_instructions_appended() {
        let config = default_config();
        let result =
            assemble_system_prompt(&config, None, None, &[], "Dispatcher tool instructions").await;
        assert!(result.contains("Dispatcher tool instructions"));
    }

    // --- Extra sections (forward-compatible for skills) ---

    #[tokio::test]
    async fn test_extra_sections_appended() {
        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            None,
            None,
            &["## Available Skills\n- /task-workflow"],
            "",
        )
        .await;
        assert!(result.contains("## Available Skills"));
        assert!(result.contains("/task-workflow"));
    }

    #[tokio::test]
    async fn test_extra_sections_before_tool_instructions() {
        let mut config = default_config();
        config.agent.tool_instructions = Some("Config tools".to_string());

        let result =
            assemble_system_prompt(&config, None, None, &["Skills section"], "Dispatcher tools")
                .await;
        let skills_pos = result.find("Skills section").unwrap();
        let config_tools_pos = result.find("Config tools").unwrap();
        let dispatcher_pos = result.find("Dispatcher tools").unwrap();
        assert!(skills_pos < config_tools_pos);
        assert!(config_tools_pos < dispatcher_pos);
    }

    #[tokio::test]
    async fn test_empty_extra_sections_no_double_newlines() {
        let config = default_config();
        let result = assemble_system_prompt(&config, None, None, &["", ""], "").await;
        // Should not have extra blank sections
        assert!(!result.contains("\n\n\n\n"));
    }

    // --- Integration: all layers together ---

    #[tokio::test]
    async fn test_full_precedence_chain() {
        let mut config = default_config();
        config.agent.system_prompt = Some("Inline base".to_string());
        config.agent.tool_instructions = Some("Config tools".to_string());

        let result = assemble_system_prompt(
            &config,
            None,
            None,
            &["Skills inventory"],
            "Dispatcher tools",
        )
        .await;

        assert!(result.contains("Inline base"));
        assert!(result.contains("Skills inventory"));
        assert!(result.contains("Config tools"));
        assert!(result.contains("Dispatcher tools"));

        // Verify ordering
        let base_pos = result.find("Inline base").unwrap();
        let skills_pos = result.find("Skills inventory").unwrap();
        let config_pos = result.find("Config tools").unwrap();
        let dispatcher_pos = result.find("Dispatcher tools").unwrap();

        assert!(base_pos < skills_pos);
        assert!(skills_pos < config_pos);
        assert!(config_pos < dispatcher_pos);
    }
}
