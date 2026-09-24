//! Unified system prompt assembly.
//!
//! Single canonical path for building the final system prompt with clear
//! precedence rules, driven by a typed [`SystemPromptOverride`]:
//!
//! 1. Per-request override ([`SystemPromptOverride::Set`]) — wins outright,
//!    skipping config and AGENTS.md. [`SystemPromptOverride::Disable`]
//!    suppresses *all* prompt sources (no config, no AGENTS.md, no default).
//! 2. Config-level file override (`config.agent.system_prompt_file`).
//! 3. Config-level inline override (`config.agent.system_prompt`).
//! 4. Default system prompt + AGENTS.md files.
//! 5. Config-level tool instructions (`config.agent.tool_instructions`).
//! 6. Dispatcher-provided tool usage instructions (appended last).

use crate::SystemPromptOverride;
use meerkat_core::{
    Config, SystemPromptConfig,
    prompt::{AGENTS_MD_MAX_BYTES, normalize_agents_md_content},
};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
};
use tokio::io::AsyncReadExt;

const AGENTS_MD_INCLUDE_MAX_DEPTH: usize = 4;
const AGENTS_MD_INCLUDE_MAX_FILES: usize = 32;
const AGENTS_MD_READ_SLOP_BYTES: usize = 4;

/// A fault assembling the system prompt from explicitly-configured sources.
///
/// An explicitly-configured prompt source (`config.agent.system_prompt_file`)
/// that the user opted into but that cannot be read must fail closed rather
/// than silently falling through to an alternate prompt — otherwise the agent
/// runs on a prompt the operator never intended. Sources that are merely
/// *absent* (no `AGENTS.md` present, no `system_prompt_file` configured) are
/// legitimately optional and never produce this error.
#[derive(Debug, thiserror::Error)]
pub enum PromptAssemblyError {
    /// An explicitly-configured `system_prompt_file` could not be read.
    #[error("configured system_prompt_file '{path}' is not readable: {source}")]
    SystemPromptFileUnreadable {
        /// The configured path that failed to read.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// A discovered project `AGENTS.md` exists (or its existence could not be
    /// probed) but could not be read. Absence of the file is honest and yields
    /// no error; a file that is present but unreadable is a fault and must not
    /// silently vanish so a lower-precedence prompt becomes authoritative.
    #[error("project AGENTS.md '{path}' is not readable: {source}")]
    AgentsMdUnreadable {
        /// The discovered path that failed to read.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },
}

/// Assemble the final system prompt. Single canonical path.
///
/// `prompt_override` is the typed per-request policy from
/// `AgentBuildConfig.system_prompt`. `extra_sections` carries the factory's
/// appended content (skill inventory, preloaded skills, additional
/// instructions); the factory gates the skill-inventory section on
/// `AgentBuildConfig.host_prompt_sections` (A1/ADJ-17 — `SpecPinned`
/// mob-materialized builds suppress it so spec-built prompts stay
/// byte-pinned across placements).
///
/// Returns [`PromptAssemblyError`] when an explicitly-configured prompt source
/// is present but unusable (e.g. an unreadable `system_prompt_file`). Optional
/// sources that are simply absent never fail.
pub async fn assemble_system_prompt(
    config: &Config,
    prompt_override: &SystemPromptOverride,
    context_root: Option<&Path>,
    extra_sections: &[&str],
    tool_usage_instructions: &str,
) -> Result<String, PromptAssemblyError> {
    // Explicit Set/Disable skips filesystem prompt sources. The target-neutral
    // renderer remains the only owner of the actual tri-state decision and
    // section ordering.
    if prompt_override.is_explicit() {
        return Ok(crate::prompt_policy::render_system_prompt_policy(
            config,
            prompt_override,
            "",
            extra_sections,
            tool_usage_instructions,
        ));
    }

    // 2-4. This crate owns filesystem reads and prompt precedence, then passes
    // already-loaded content into core's pure renderer.
    let mut spc = SystemPromptConfig::new();

    // 2. Config-level file override → feeds into SystemPromptConfig.
    //    The operator explicitly pointed at this file, so an unreadable file
    //    is a fault and fails closed — it must never silently fall through to
    //    the inline/default prompt the operator did not ask for.
    if let Some(ref path) = config.agent.system_prompt_file {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => spc.system_prompt = Some(content),
            Err(source) => {
                return Err(PromptAssemblyError::SystemPromptFileUnreadable {
                    path: path.display().to_string(),
                    source,
                });
            }
        }
    }

    // 3. Config-level inline override (lower precedence than file).
    if spc.system_prompt.is_none()
        && let Some(ref prompt) = config.agent.system_prompt
    {
        spc.system_prompt = Some(prompt.clone());
    }

    // AGENTS.md is resolved only from explicit context roots. Absence is
    // honest (`None`); an existing-but-unreadable file propagates as a typed
    // fault instead of disappearing so a lower-precedence prompt never
    // silently becomes authoritative.
    if let Some(context) = context_root
        && let Some(content) = load_project_agents_md_in(context).await?
    {
        spc = spc.with_project_agents_md_content(content);
    }

    // 4. compose() uses the override if set, otherwise DEFAULT_SYSTEM_PROMPT.
    //    Either way, AGENTS.md files are appended (global + project).
    let inherited_base = spc.compose().await;

    Ok(crate::prompt_policy::render_system_prompt_policy(
        config,
        prompt_override,
        &inherited_base,
        extra_sections,
        tool_usage_instructions,
    ))
}

async fn load_project_agents_md_in(dir: &Path) -> Result<Option<String>, PromptAssemblyError> {
    let canonical_root = tokio::fs::canonicalize(dir).await.map_err(|source| {
        PromptAssemblyError::AgentsMdUnreadable {
            path: dir.display().to_string(),
            source,
        }
    })?;
    for candidate in [dir.join("AGENTS.md"), dir.join(".rkat/AGENTS.md")] {
        if let Some(content) = load_agents_md_file(&candidate, &canonical_root).await? {
            return Ok(Some(content));
        }
    }
    Ok(None)
}

/// Load one discovered AGENTS.md candidate.
///
/// `Ok(None)` means honest absence (file missing, or present but empty after
/// normalization). Every I/O fault — an existence probe that errors, or a
/// file that exists but cannot be read (permissions, invalid UTF-8) — is a
/// typed [`PromptAssemblyError::AgentsMdUnreadable`], never laundered into
/// absence.
async fn load_agents_md_file(
    path: &Path,
    canonical_root: &Path,
) -> Result<Option<String>, PromptAssemblyError> {
    let exists = tokio::fs::try_exists(path).await.map_err(|source| {
        PromptAssemblyError::AgentsMdUnreadable {
            path: path.display().to_string(),
            source,
        }
    })?;
    if !exists {
        return Ok(None);
    }

    let canonical_path = tokio::fs::canonicalize(path).await.map_err(|source| {
        PromptAssemblyError::AgentsMdUnreadable {
            path: path.display().to_string(),
            source,
        }
    })?;
    if !canonical_path.starts_with(canonical_root) {
        return Err(PromptAssemblyError::AgentsMdUnreadable {
            path: path.display().to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{} resolves outside context root {}",
                    canonical_path.display(),
                    canonical_root.display()
                ),
            ),
        });
    }

    let mut remaining_files = AGENTS_MD_INCLUDE_MAX_FILES;
    let mut stack = Vec::new();
    let content = match expand_agents_md_file(
        canonical_path.clone(),
        canonical_root,
        &mut stack,
        &mut remaining_files,
        0,
    )
    .await
    {
        Ok(content) => content,
        Err(reason)
            if reason.starts_with(&format!("failed to read {}:", canonical_path.display())) =>
        {
            return Err(PromptAssemblyError::AgentsMdUnreadable {
                path: path.display().to_string(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, reason),
            });
        }
        Err(reason) => {
            return Err(PromptAssemblyError::AgentsMdUnreadable {
                path: path.display().to_string(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, reason),
            });
        }
    };
    Ok(normalize_agents_md_content(&content))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MarkdownFence {
    ch: char,
    len: usize,
}

fn expand_agents_md_file<'a>(
    canonical_path: PathBuf,
    canonical_root: &'a Path,
    stack: &'a mut Vec<PathBuf>,
    remaining_files: &'a mut usize,
    depth: usize,
) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
    Box::pin(async move {
        if depth > AGENTS_MD_INCLUDE_MAX_DEPTH {
            return Err(format!(
                "include depth exceeded at {} via {}",
                canonical_path.display(),
                format_include_chain(stack, Some(&canonical_path))
            ));
        }
        if stack.iter().any(|path| path == &canonical_path) {
            return Err(format!(
                "cyclic include at {} via {}",
                canonical_path.display(),
                format_include_chain(stack, Some(&canonical_path))
            ));
        }
        if !canonical_path.starts_with(canonical_root) {
            return Err(format!(
                "{} resolves outside context root {}",
                canonical_path.display(),
                canonical_root.display()
            ));
        }
        let metadata = tokio::fs::metadata(&canonical_path)
            .await
            .map_err(|source| {
                format!("failed to inspect {}: {source}", canonical_path.display())
            })?;
        if !metadata.is_file() {
            return Err(format!("{} is not a file", canonical_path.display()));
        }
        if *remaining_files == 0 {
            return Err(format!(
                "include file count exceeded at {} via {}",
                canonical_path.display(),
                format_include_chain(stack, Some(&canonical_path))
            ));
        }
        *remaining_files -= 1;

        stack.push(canonical_path.clone());
        let read_limit = AGENTS_MD_MAX_BYTES
            .saturating_add(AGENTS_MD_READ_SLOP_BYTES)
            .saturating_add(1);
        let content = read_utf8_file_capped(&canonical_path, read_limit)
            .await
            .map_err(|source| format!("failed to read {}: {source}", canonical_path.display()))?;
        let mut remaining_bytes = AGENTS_MD_MAX_BYTES;
        let expanded = expand_agents_md_content(
            &canonical_path,
            canonical_root,
            stack,
            &mut remaining_bytes,
            remaining_files,
            &content,
        )
        .await;
        stack.pop();
        expanded
    })
}

fn expand_agents_md_content<'a>(
    source_path: &'a Path,
    canonical_root: &'a Path,
    stack: &'a mut Vec<PathBuf>,
    remaining_bytes: &'a mut usize,
    remaining_files: &'a mut usize,
    content: &'a str,
) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
    Box::pin(async move {
        let mut output = String::new();
        let mut fence: Option<MarkdownFence> = None;
        for raw_line in content.split_inclusive('\n') {
            let line_without_lf = raw_line.strip_suffix('\n').unwrap_or(raw_line);
            let line_body = line_without_lf
                .strip_suffix('\r')
                .unwrap_or(line_without_lf);
            if let Some(marker) = markdown_fence_marker(line_body) {
                if let Some(open) = fence {
                    if marker.ch == open.ch
                        && marker.len >= open.len
                        && markdown_fence_closer_has_no_info(line_body, marker)
                    {
                        fence = None;
                    }
                } else {
                    fence = Some(marker);
                }
                append_with_budget(&mut output, remaining_bytes, raw_line);
                continue;
            }

            if fence.is_none()
                && let Some(include_path) = parse_agents_md_include_directive(line_body)
            {
                let base = source_path.parent().unwrap_or_else(|| Path::new("."));
                let raw_target = base.join(include_path);
                let canonical_target =
                    tokio::fs::canonicalize(&raw_target)
                        .await
                        .map_err(|source| {
                            format!(
                                "failed to resolve include `{}` from {}: {source}",
                                include_path,
                                source_path.display()
                            )
                        })?;
                if !canonical_target.starts_with(canonical_root) {
                    return Err(format!(
                        "include `{}` from {} resolves outside context root {}",
                        include_path,
                        source_path.display(),
                        canonical_root.display()
                    ));
                }
                let included = expand_agents_md_file(
                    canonical_target,
                    canonical_root,
                    stack,
                    remaining_files,
                    stack.len(),
                )
                .await?;
                append_with_budget(&mut output, remaining_bytes, &included);
                if raw_line.ends_with('\n') && !included.ends_with('\n') {
                    append_with_budget(&mut output, remaining_bytes, "\n");
                }
            } else {
                append_with_budget(&mut output, remaining_bytes, raw_line);
            }
        }
        Ok(output)
    })
}

fn parse_agents_md_include_directive(line: &str) -> Option<&str> {
    let trimmed = line.trim_matches(|ch| ch == ' ' || ch == '\t');
    let path = trimmed.strip_prefix('@')?;
    if path.is_empty()
        || path.starts_with('@')
        || path.starts_with('/')
        || path.starts_with('~')
        || path.contains(char::is_whitespace)
        || path.contains('#')
    {
        return None;
    }
    Some(path)
}

fn markdown_fence_marker(line: &str) -> Option<MarkdownFence> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let leading_spaces = line.len().saturating_sub(trimmed.len());
    if leading_spaces > 3 {
        return None;
    }
    let ch = trimmed.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let len = trimmed
        .chars()
        .take_while(|candidate| *candidate == ch)
        .count();
    if len < 3 {
        return None;
    }
    Some(MarkdownFence { ch, len })
}

fn markdown_fence_closer_has_no_info(line: &str, marker: MarkdownFence) -> bool {
    let trimmed = line.trim_start_matches([' ', '\t']);
    trimmed[marker.len..]
        .chars()
        .all(|ch| ch == ' ' || ch == '\t')
}

fn append_with_budget(output: &mut String, remaining_bytes: &mut usize, text: &str) {
    if *remaining_bytes == 0 || text.is_empty() {
        return;
    }
    if text.len() <= *remaining_bytes {
        output.push_str(text);
        *remaining_bytes -= text.len();
        return;
    }
    let mut end = *remaining_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    output.push_str(&text[..end]);
    *remaining_bytes = 0;
}

async fn read_utf8_file_capped(path: &Path, max_bytes: usize) -> Result<String, std::io::Error> {
    let file = tokio::fs::File::open(path).await?;
    let mut bytes = Vec::new();
    file.take(max_bytes as u64).read_to_end(&mut bytes).await?;
    match std::str::from_utf8(&bytes) {
        Ok(content) => Ok(content.to_string()),
        Err(error) if bytes.len() == max_bytes && error.error_len().is_none() => {
            let valid_prefix = &bytes[..error.valid_up_to()];
            std::str::from_utf8(valid_prefix)
                .map(str::to_string)
                .map_err(|source| std::io::Error::new(std::io::ErrorKind::InvalidData, source))
        }
        Err(error) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid UTF-8: {error}"),
        )),
    }
}

fn format_include_chain(stack: &[PathBuf], terminal: Option<&Path>) -> String {
    stack
        .iter()
        .map(|path| path.display().to_string())
        .chain(terminal.map(|path| path.display().to_string()))
        .collect::<Vec<_>>()
        .join(" -> ")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::prompt::{AGENTS_MD_MAX_BYTES, DEFAULT_SYSTEM_PROMPT};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn default_config() -> Config {
        Config::default()
    }

    // --- Precedence level 1: per-request override ---

    fn set(prompt: &str) -> SystemPromptOverride {
        SystemPromptOverride::Set(prompt.to_string())
    }

    #[tokio::test]
    async fn test_per_request_override_wins() {
        let config = default_config();
        let result = assemble_system_prompt(&config, &set("Per-request prompt"), None, &[], "")
            .await
            .unwrap();
        assert_eq!(result, "Per-request prompt");
    }

    #[tokio::test]
    async fn test_per_request_override_skips_config_fields() {
        let mut config = default_config();
        config.agent.system_prompt = Some("Config inline prompt".to_string());
        config.agent.tool_instructions = Some("Config tool instructions".to_string());

        let result = assemble_system_prompt(&config, &set("Per-request prompt"), None, &[], "")
            .await
            .unwrap();
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
            &set("Per-request prompt"),
            None,
            &[],
            "Dispatcher tool instructions",
        )
        .await
        .unwrap();
        assert!(result.starts_with("Per-request prompt"));
        assert!(result.contains("Dispatcher tool instructions"));
    }

    // --- Typed override: the three policies are type-distinguishable ---

    #[tokio::test]
    async fn test_prompt_override_disable_suppresses_all_sources() {
        let temp = TempDir::new().unwrap();
        // Provide every prompt source: a configured file, an inline config
        // prompt, and an AGENTS.md context root.
        let file_path = temp.path().join("prompt.txt");
        tokio::fs::write(&file_path, "File-based prompt")
            .await
            .unwrap();
        let agents_path = temp.path().join("AGENTS.md");
        tokio::fs::write(&agents_path, "Context root instructions")
            .await
            .unwrap();

        let mut config = default_config();
        config.agent.system_prompt_file = Some(file_path);
        config.agent.system_prompt = Some("Inline prompt".to_string());

        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Disable,
            Some(temp.path()),
            &[],
            "Dispatcher tool instructions",
        )
        .await
        .unwrap();

        // Disable suppresses EVERY prompt source — config file, config inline,
        // AGENTS.md, and the default prompt — leaving only appended sections.
        assert!(!result.contains("File-based prompt"));
        assert!(!result.contains("Inline prompt"));
        assert!(!result.contains("Context root instructions"));
        assert!(!result.contains(DEFAULT_SYSTEM_PROMPT));
        // Appended dispatcher instructions still apply, with no leading blank.
        assert_eq!(result, "Dispatcher tool instructions");
    }

    #[tokio::test]
    async fn test_prompt_override_set_skips_config_only() {
        let temp = TempDir::new().unwrap();
        let agents_path = temp.path().join("AGENTS.md");
        tokio::fs::write(&agents_path, "Context root instructions")
            .await
            .unwrap();

        let mut config = default_config();
        config.agent.system_prompt = Some("Inline prompt".to_string());
        config.agent.tool_instructions = Some("Config tool instructions".to_string());

        let result = assemble_system_prompt(
            &config,
            &set("Per-request prompt"),
            Some(temp.path()),
            &[],
            "Dispatcher tools",
        )
        .await
        .unwrap();

        // Set wins outright over config inline and AGENTS.md...
        assert!(result.starts_with("Per-request prompt"));
        assert!(!result.contains("Inline prompt"));
        assert!(!result.contains("Context root instructions"));
        assert!(!result.contains(DEFAULT_SYSTEM_PROMPT));
        // ...and also skips config-level tool instructions (only the
        // dispatcher instructions are appended).
        assert!(!result.contains("Config tool instructions"));
        assert!(result.contains("Dispatcher tools"));
    }

    #[tokio::test]
    async fn test_prompt_override_inherit_uses_config_and_default() {
        let mut config = default_config();
        config.agent.system_prompt = Some("Inline prompt".to_string());

        let result = assemble_system_prompt(&config, &SystemPromptOverride::Inherit, None, &[], "")
            .await
            .unwrap();
        // Inherit falls through to the config inline override.
        assert!(result.contains("Inline prompt"));
    }

    #[test]
    fn test_prompt_override_explicitness() {
        // Explicitness drives whether the prompt must be (re)assembled. The
        // tri-state wire/persisted representation (string/null/disable-action)
        // is the type's canonical serde, pinned in `meerkat_core::config`.
        assert!(!SystemPromptOverride::Inherit.is_explicit());
        assert!(set("p").is_explicit());
        assert!(SystemPromptOverride::Disable.is_explicit());
        assert_eq!(set("p").as_set_prompt(), Some("p"));
        assert_eq!(SystemPromptOverride::Inherit.as_set_prompt(), None);
        assert_eq!(SystemPromptOverride::Disable.as_set_prompt(), None);
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

        let result = assemble_system_prompt(&config, &SystemPromptOverride::Inherit, None, &[], "")
            .await
            .unwrap();
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

        let result = assemble_system_prompt(&config, &SystemPromptOverride::Inherit, None, &[], "")
            .await
            .unwrap();
        assert!(result.contains("File-based prompt"));
        assert!(!result.contains("Inline prompt"));
    }

    #[tokio::test]
    async fn test_explicit_prompt_file_unreadable_is_error() {
        let mut config = default_config();
        // The operator explicitly pointed at this file; an unreadable file is
        // a fault and must fail closed rather than silently falling through to
        // the inline/default prompt.
        config.agent.system_prompt_file = Some(PathBuf::from("/nonexistent/path/prompt.txt"));
        config.agent.system_prompt = Some("Inline fallback".to_string());

        let result =
            assemble_system_prompt(&config, &SystemPromptOverride::Inherit, None, &[], "").await;
        let err = result.expect_err("unreadable configured system_prompt_file must error");
        assert!(matches!(
            err,
            PromptAssemblyError::SystemPromptFileUnreadable { .. }
        ));
    }

    // --- Precedence level 3: config inline override ---

    #[tokio::test]
    async fn test_config_inline_override() {
        let mut config = default_config();
        config.agent.system_prompt = Some("Inline prompt".to_string());

        let result = assemble_system_prompt(&config, &SystemPromptOverride::Inherit, None, &[], "")
            .await
            .unwrap();
        assert!(result.contains("Inline prompt"));
        assert!(!result.contains(DEFAULT_SYSTEM_PROMPT));
    }

    #[tokio::test]
    async fn test_context_root_agents_md_appended_to_config_inline() {
        let temp = TempDir::new().unwrap();
        let agents_path = temp.path().join("AGENTS.md");
        tokio::fs::write(&agents_path, "Context root instructions")
            .await
            .unwrap();

        let mut config = default_config();
        config.agent.system_prompt = Some("Inline prompt".to_string());

        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .unwrap();
        assert!(result.contains("Inline prompt"));
        assert!(result.contains("Context root instructions"));
        assert!(!result.contains(DEFAULT_SYSTEM_PROMPT));
        let inline_pos = result.find("Inline prompt").unwrap();
        let agents_pos = result.find("Context root instructions").unwrap();
        assert!(inline_pos < agents_pos);
    }

    #[tokio::test]
    async fn test_context_root_agents_md_size_limit_owned_by_prompt_assembly() {
        let temp = TempDir::new().unwrap();
        let agents_path = temp.path().join("AGENTS.md");
        tokio::fs::write(&agents_path, "x".repeat(AGENTS_MD_MAX_BYTES + 1000))
            .await
            .unwrap();

        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .unwrap();
        let agents_section_start = result.find("# Project Instructions").unwrap();
        let agents_content = &result[agents_section_start..];
        assert!(agents_content.len() <= AGENTS_MD_MAX_BYTES + 100);
    }

    #[tokio::test]
    async fn test_context_root_agents_md_include_near_size_limit_is_charged_once() {
        let temp = TempDir::new().unwrap();
        let included = "x".repeat(AGENTS_MD_MAX_BYTES - 20);
        tokio::fs::write(temp.path().join("AGENTS.md"), "@included.md")
            .await
            .unwrap();
        tokio::fs::write(temp.path().join("included.md"), &included)
            .await
            .unwrap();

        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .unwrap();

        assert!(
            result.contains(&included),
            "included AGENTS content should not be double-charged against the byte budget"
        );
    }

    #[tokio::test]
    async fn test_context_root_agents_md_expands_line_scoped_file_include() {
        let temp = TempDir::new().unwrap();
        tokio::fs::write(temp.path().join("AGENTS.md"), "Before\n@CLAUDE.md\nAfter")
            .await
            .unwrap();
        tokio::fs::write(temp.path().join("CLAUDE.md"), "Included instructions")
            .await
            .unwrap();

        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .unwrap();

        assert!(result.contains("Before\nIncluded instructions\nAfter"));
        assert!(!result.contains("@CLAUDE.md"));
    }

    #[tokio::test]
    async fn test_context_root_agents_md_does_not_expand_inline_or_fenced_mentions() {
        let temp = TempDir::new().unwrap();
        tokio::fs::write(
            temp.path().join("AGENTS.md"),
            "Inline @CLAUDE.md\n```\n@CLAUDE.md\n```\n",
        )
        .await
        .unwrap();
        tokio::fs::write(temp.path().join("CLAUDE.md"), "Included instructions")
            .await
            .unwrap();

        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .unwrap();

        assert!(result.contains("Inline @CLAUDE.md"));
        assert!(result.contains("```\n@CLAUDE.md\n```"));
        assert!(!result.contains("Included instructions"));
    }

    #[tokio::test]
    async fn test_context_root_agents_md_keeps_extended_fences_inert() {
        let temp = TempDir::new().unwrap();
        tokio::fs::write(
            temp.path().join("AGENTS.md"),
            "````markdown\n```\n@CLAUDE.md\n````\n",
        )
        .await
        .unwrap();
        tokio::fs::write(temp.path().join("CLAUDE.md"), "Included instructions")
            .await
            .unwrap();

        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .unwrap();

        assert!(result.contains("````markdown\n```\n@CLAUDE.md\n````"));
        assert!(!result.contains("Included instructions"));
    }

    #[tokio::test]
    async fn test_context_root_agents_md_does_not_close_fence_on_info_string() {
        let temp = TempDir::new().unwrap();
        tokio::fs::write(
            temp.path().join("AGENTS.md"),
            "```\n```rust\n@CLAUDE.md\n```\n",
        )
        .await
        .unwrap();
        tokio::fs::write(temp.path().join("CLAUDE.md"), "Included instructions")
            .await
            .unwrap();

        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .unwrap();

        assert!(result.contains("```\n```rust\n@CLAUDE.md\n```"));
        assert!(!result.contains("Included instructions"));
    }

    #[tokio::test]
    async fn test_dot_rkat_agents_md_expands_include_relative_to_dot_rkat() {
        let temp = TempDir::new().unwrap();
        let rkat_dir = temp.path().join(".rkat");
        tokio::fs::create_dir_all(&rkat_dir).await.unwrap();
        tokio::fs::write(rkat_dir.join("AGENTS.md"), "@nested.md\n@../ROOT.md")
            .await
            .unwrap();
        tokio::fs::write(rkat_dir.join("nested.md"), "Nested instructions")
            .await
            .unwrap();
        tokio::fs::write(temp.path().join("ROOT.md"), "Root instructions")
            .await
            .unwrap();

        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .unwrap();

        assert!(result.contains("Nested instructions"));
        assert!(result.contains("Root instructions"));
    }

    #[tokio::test]
    async fn test_context_root_agents_md_missing_include_fails_closed() {
        let temp = TempDir::new().unwrap();
        tokio::fs::write(temp.path().join("AGENTS.md"), "@MISSING.md")
            .await
            .unwrap();
        let rkat_dir = temp.path().join(".rkat");
        tokio::fs::create_dir_all(&rkat_dir).await.unwrap();
        tokio::fs::write(rkat_dir.join("AGENTS.md"), "Fallback instructions")
            .await
            .unwrap();

        let config = default_config();
        let err = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .expect_err("bad top-level include must not fall through to .rkat/AGENTS.md");

        assert!(matches!(
            err,
            PromptAssemblyError::AgentsMdUnreadable { .. }
        ));
    }

    #[tokio::test]
    async fn test_context_root_agents_md_directory_include_fails_closed() {
        let temp = TempDir::new().unwrap();
        tokio::fs::write(temp.path().join("AGENTS.md"), "@.")
            .await
            .unwrap();

        let config = default_config();
        let err = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .expect_err("directory include must fail closed");

        match err {
            PromptAssemblyError::AgentsMdUnreadable { source, .. } => {
                assert!(source.to_string().contains("is not a file"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_context_root_agents_md_symlink_escape_fails_closed() {
        let temp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        tokio::fs::write(outside.path().join("outside.md"), "Outside instructions")
            .await
            .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("outside.md"),
            temp.path().join("link.md"),
        )
        .unwrap();
        tokio::fs::write(temp.path().join("AGENTS.md"), "@link.md")
            .await
            .unwrap();

        let config = default_config();
        let err = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .expect_err("symlink escape must fail closed");

        match err {
            PromptAssemblyError::AgentsMdUnreadable { source, .. } => {
                assert!(source.to_string().contains("outside context root"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn test_context_root_agents_md_cycle_fails_closed() {
        let temp = TempDir::new().unwrap();
        tokio::fs::write(temp.path().join("AGENTS.md"), "@a.md")
            .await
            .unwrap();
        tokio::fs::write(temp.path().join("a.md"), "@AGENTS.md")
            .await
            .unwrap();

        let config = default_config();
        let err = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .expect_err("cyclic AGENTS include must fail closed");

        assert!(matches!(
            err,
            PromptAssemblyError::AgentsMdUnreadable { .. }
        ));
    }

    #[tokio::test]
    async fn test_context_root_agents_md_depth_limit_fails_closed() {
        let temp = TempDir::new().unwrap();
        tokio::fs::write(temp.path().join("AGENTS.md"), "@a0.md")
            .await
            .unwrap();
        for index in 0..=AGENTS_MD_INCLUDE_MAX_DEPTH {
            tokio::fs::write(
                temp.path().join(format!("a{index}.md")),
                format!("@a{}.md", index + 1),
            )
            .await
            .unwrap();
        }
        tokio::fs::write(
            temp.path()
                .join(format!("a{}.md", AGENTS_MD_INCLUDE_MAX_DEPTH + 1)),
            "too deep",
        )
        .await
        .unwrap();

        let config = default_config();
        let err = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .expect_err("over-deep AGENTS include must fail closed");

        assert!(matches!(
            err,
            PromptAssemblyError::AgentsMdUnreadable { .. }
        ));
    }

    #[tokio::test]
    async fn test_context_root_agents_md_file_count_limit_fails_closed() {
        let temp = TempDir::new().unwrap();
        let includes = (0..AGENTS_MD_INCLUDE_MAX_FILES)
            .map(|index| format!("@file-{index}.md"))
            .collect::<Vec<_>>()
            .join("\n");
        tokio::fs::write(temp.path().join("AGENTS.md"), includes)
            .await
            .unwrap();
        for index in 0..AGENTS_MD_INCLUDE_MAX_FILES {
            tokio::fs::write(temp.path().join(format!("file-{index}.md")), "included")
                .await
                .unwrap();
        }

        let config = default_config();
        let err = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .expect_err("too many AGENTS include files must fail closed");

        assert!(matches!(
            err,
            PromptAssemblyError::AgentsMdUnreadable { .. }
        ));
    }

    #[tokio::test]
    async fn test_context_root_agents_md_unreadable_is_error_not_silent_fallback() {
        // An AGENTS.md that EXISTS but cannot be read (here: invalid UTF-8,
        // so read_to_string fails) is a fault. It must propagate as a typed
        // PromptAssemblyError instead of silently disappearing so that a
        // lower-precedence source (.rkat/AGENTS.md, inline, default) never
        // becomes authoritative over a prompt the operator placed.
        let temp = TempDir::new().unwrap();
        let agents_path = temp.path().join("AGENTS.md");
        tokio::fs::write(&agents_path, [0xff, 0xfe]).await.unwrap();
        let rkat_dir = temp.path().join(".rkat");
        tokio::fs::create_dir_all(&rkat_dir).await.unwrap();
        tokio::fs::write(rkat_dir.join("AGENTS.md"), "Fallback instructions")
            .await
            .unwrap();

        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await;
        let err = result.expect_err("unreadable AGENTS.md must be a typed fault");
        assert!(matches!(
            err,
            PromptAssemblyError::AgentsMdUnreadable { .. }
        ));
    }

    #[tokio::test]
    async fn test_context_root_agents_md_absent_is_honest_absence() {
        // A context root with NO AGENTS.md at all is honest absence: no
        // error, default prompt applies.
        let temp = TempDir::new().unwrap();
        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            Some(temp.path()),
            &[],
            "",
        )
        .await
        .unwrap();
        assert!(result.contains(DEFAULT_SYSTEM_PROMPT));
    }

    // --- Precedence level 4: default prompt ---

    #[tokio::test]
    async fn test_default_prompt_when_no_overrides() {
        let config = default_config();
        let result = assemble_system_prompt(&config, &SystemPromptOverride::Inherit, None, &[], "")
            .await
            .unwrap();
        assert!(result.contains(DEFAULT_SYSTEM_PROMPT));
    }

    // --- Precedence level 5: config tool instructions ---

    #[tokio::test]
    async fn test_config_tool_instructions_appended() {
        let mut config = default_config();
        config.agent.tool_instructions = Some("Use tools carefully".to_string());

        let result = assemble_system_prompt(&config, &SystemPromptOverride::Inherit, None, &[], "")
            .await
            .unwrap();
        assert!(result.contains("Use tools carefully"));
    }

    #[tokio::test]
    async fn test_config_tool_instructions_before_dispatcher() {
        let mut config = default_config();
        config.agent.tool_instructions = Some("Config tools".to_string());

        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            None,
            &[],
            "Dispatcher tools",
        )
        .await
        .unwrap();
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
        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            None,
            &[],
            "Dispatcher tool instructions",
        )
        .await
        .unwrap();
        assert!(result.contains("Dispatcher tool instructions"));
    }

    // --- Extra sections (forward-compatible for skills) ---

    #[tokio::test]
    async fn test_extra_sections_appended() {
        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            None,
            &["## Available Skills\n- /task-workflow"],
            "",
        )
        .await
        .unwrap();
        assert!(result.contains("## Available Skills"));
        assert!(result.contains("/task-workflow"));
    }

    #[tokio::test]
    async fn test_extra_sections_before_tool_instructions() {
        let mut config = default_config();
        config.agent.tool_instructions = Some("Config tools".to_string());

        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            None,
            &["Skills section"],
            "Dispatcher tools",
        )
        .await
        .unwrap();
        let skills_pos = result.find("Skills section").unwrap();
        let config_tools_pos = result.find("Config tools").unwrap();
        let dispatcher_pos = result.find("Dispatcher tools").unwrap();
        assert!(skills_pos < config_tools_pos);
        assert!(config_tools_pos < dispatcher_pos);
    }

    #[tokio::test]
    async fn test_empty_extra_sections_no_double_newlines() {
        let config = default_config();
        let result =
            assemble_system_prompt(&config, &SystemPromptOverride::Inherit, None, &["", ""], "")
                .await
                .unwrap();
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
            &SystemPromptOverride::Inherit,
            None,
            &["Skills inventory"],
            "Dispatcher tools",
        )
        .await
        .unwrap();

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

    // --- Additional instructions via extra_sections ---

    #[tokio::test]
    async fn test_additional_instructions_appear_after_skills_before_tool_instructions() {
        let mut config = default_config();
        config.agent.tool_instructions = Some("Config tools".to_string());

        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            None,
            &[
                "Skills section",
                "Additional instruction 1",
                "Additional instruction 2",
            ],
            "Dispatcher tools",
        )
        .await
        .unwrap();

        let skills_pos = result.find("Skills section").unwrap();
        let instr1_pos = result.find("Additional instruction 1").unwrap();
        let instr2_pos = result.find("Additional instruction 2").unwrap();
        let config_pos = result.find("Config tools").unwrap();
        let dispatcher_pos = result.find("Dispatcher tools").unwrap();

        assert!(skills_pos < instr1_pos, "instructions after skills");
        assert!(instr1_pos < instr2_pos, "instructions preserve order");
        assert!(instr2_pos < config_pos, "instructions before config tools");
        assert!(
            config_pos < dispatcher_pos,
            "config tools before dispatcher"
        );
    }

    #[tokio::test]
    async fn test_additional_instructions_not_after_tool_instructions() {
        let config = default_config();
        let result = assemble_system_prompt(
            &config,
            &SystemPromptOverride::Inherit,
            None,
            &["My additional instruction"],
            "Dispatcher tools block",
        )
        .await
        .unwrap();

        let instruction_pos = result.find("My additional instruction").unwrap();
        let dispatcher_pos = result.find("Dispatcher tools block").unwrap();
        assert!(
            instruction_pos < dispatcher_pos,
            "additional instructions must NOT appear after dispatcher tool instructions"
        );
    }
}
