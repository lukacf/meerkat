//! Target-neutral system-prompt policy rendering.
//!
//! Filesystem-capable builds load configured files and project instructions in
//! `prompt_assembly`; embedded/WASM builds deliberately have no such sources.
//! Both paths lower the already-loaded inherited base through this one policy
//! table so `Set`/`Disable`/`Inherit`, section ordering, and separators cannot
//! drift by target.

use crate::SystemPromptOverride;
use meerkat_core::Config;
#[cfg(any(test, target_arch = "wasm32"))]
use meerkat_core::prompt::SystemPromptConfig;

/// Render the canonical prompt from an already-resolved inherited base.
///
/// `inherited_base` is used only for [`SystemPromptOverride::Inherit`]. The
/// caller may therefore pass an empty string when an explicit override lets a
/// filesystem-capable path skip all source loading.
pub(crate) fn render_system_prompt_policy(
    config: &Config,
    prompt_override: &SystemPromptOverride,
    inherited_base: &str,
    extra_sections: &[&str],
    tool_usage_instructions: &str,
) -> String {
    let (base, include_config_tool_instructions) = match prompt_override {
        SystemPromptOverride::Set(prompt) => (prompt.as_str(), false),
        SystemPromptOverride::Disable => ("", true),
        SystemPromptOverride::Inherit => (inherited_base, true),
    };

    let config_tool_sections = include_config_tool_instructions
        .then_some(config.agent.tool_instructions.as_deref())
        .flatten()
        .into_iter()
        .collect::<Vec<_>>();

    append_sections(
        base,
        extra_sections,
        &config_tool_sections,
        tool_usage_instructions,
    )
}

/// Assemble the canonical prompt on a target with no filesystem prompt
/// sources (notably WASM).
///
/// Core owns default/base rendering; this function supplies the inline config
/// source and then routes through the same facade policy renderer as native
/// builds. Browser filesystem omissions stay explicit mechanics rather than a
/// second semantic policy implementation.
#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) fn assemble_system_prompt_without_filesystem(
    config: &Config,
    prompt_override: &SystemPromptOverride,
    extra_sections: &[&str],
    tool_usage_instructions: &str,
) -> String {
    let mut sources = SystemPromptConfig::new();
    if let Some(prompt) = &config.agent.system_prompt {
        sources.system_prompt = Some(prompt.clone());
    }
    let inherited_base = sources.compose_sync();
    render_system_prompt_policy(
        config,
        prompt_override,
        &inherited_base,
        extra_sections,
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
    let push_section = |prompt: &mut String, section: &str| {
        if section.is_empty() {
            return;
        }
        if !prompt.is_empty() {
            prompt.push_str("\n\n");
        }
        prompt.push_str(section);
    };
    for section in extra_sections {
        push_section(&mut prompt, section);
    }
    for section in config_tool_sections {
        push_section(&mut prompt, section);
    }
    push_section(&mut prompt, tool_instructions);
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::prompt::DEFAULT_SYSTEM_PROMPT;

    #[test]
    fn filesystem_free_policy_uses_core_default_and_canonical_section_order() {
        let mut config = Config::default();
        config.agent.tool_instructions = Some("config tools".to_string());

        let prompt = assemble_system_prompt_without_filesystem(
            &config,
            &SystemPromptOverride::Inherit,
            &[
                "skill inventory",
                "preloaded skill",
                "additional instruction",
            ],
            "dispatcher tools",
        );

        assert_eq!(
            prompt,
            format!(
                "{DEFAULT_SYSTEM_PROMPT}\n\nskill inventory\n\npreloaded skill\n\nadditional instruction\n\nconfig tools\n\ndispatcher tools"
            )
        );
    }

    #[test]
    fn filesystem_free_policy_shares_set_disable_inherit_semantics() {
        let mut config = Config::default();
        config.agent.system_prompt = Some("configured base".to_string());
        config.agent.tool_instructions = Some("config tools".to_string());
        let extras = ["skill inventory", "additional instruction"];

        assert_eq!(
            assemble_system_prompt_without_filesystem(
                &config,
                &SystemPromptOverride::Set("request base".to_string()),
                &extras,
                "dispatcher tools",
            ),
            "request base\n\nskill inventory\n\nadditional instruction\n\ndispatcher tools"
        );
        assert_eq!(
            assemble_system_prompt_without_filesystem(
                &config,
                &SystemPromptOverride::Disable,
                &extras,
                "dispatcher tools",
            ),
            "skill inventory\n\nadditional instruction\n\nconfig tools\n\ndispatcher tools"
        );
        assert_eq!(
            assemble_system_prompt_without_filesystem(
                &config,
                &SystemPromptOverride::Inherit,
                &extras,
                "dispatcher tools",
            ),
            "configured base\n\nskill inventory\n\nadditional instruction\n\nconfig tools\n\ndispatcher tools"
        );
    }
}
