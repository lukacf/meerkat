//! SKILL.md frontmatter parser.

use meerkat_core::skills::{SkillDescriptor, SkillDocument, SkillError, SkillId, SkillScope};
use indexmap::IndexMap;

/// Parsed frontmatter from a SKILL.md file.
#[derive(Debug, serde::Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default)]
    requires_capabilities: Vec<String>,
}

/// Parse a SKILL.md file into a `SkillDocument`.
///
/// The file format is:
/// ```text
/// ---
/// name: Shell Patterns
/// description: Background job workflows
/// requires_capabilities: [builtins, shell]
/// ---
///
/// # Shell Patterns
/// ...
/// ```
pub fn parse_skill_md(
    id: SkillId,
    scope: SkillScope,
    content: &str,
) -> Result<SkillDocument, SkillError> {
    let (frontmatter_str, body) = split_frontmatter(content)?;
    let fm: Frontmatter = serde_json::from_str(
        &frontmatter_to_json(&frontmatter_str),
    )
    .map_err(|e| SkillError::Parse(format!("frontmatter parse error: {e}").into()))?;

    Ok(SkillDocument {
        descriptor: SkillDescriptor {
            id,
            name: fm.name,
            description: fm.description,
            scope,
            requires_capabilities: fm.requires_capabilities,
        },
        body: body.to_string(),
        extensions: IndexMap::new(),
    })
}

/// Split content into frontmatter and body, separated by `---` delimiters.
fn split_frontmatter(content: &str) -> Result<(String, &str), SkillError> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(SkillError::Parse("missing frontmatter delimiter".into()));
    }

    let after_first = &trimmed[3..].trim_start_matches('\n');
    let end_pos = after_first
        .find("\n---")
        .ok_or_else(|| SkillError::Parse("missing closing frontmatter delimiter".into()))?;

    let frontmatter = &after_first[..end_pos];
    let body = &after_first[end_pos + 4..];

    Ok((frontmatter.to_string(), body.trim_start_matches('\n')))
}

/// Convert YAML-like frontmatter to JSON for serde_json parsing.
/// Handles simple key: value and key: [list] formats.
fn frontmatter_to_json(fm: &str) -> String {
    let mut pairs = Vec::new();
    for line in fm.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            if value.starts_with('[') && value.ends_with(']') {
                // Array value
                let items: Vec<String> = value[1..value.len() - 1]
                    .split(',')
                    .map(|s| format!("\"{}\"", s.trim()))
                    .collect();
                pairs.push(format!("\"{key}\": [{}]", items.join(", ")));
            } else {
                pairs.push(format!("\"{key}\": \"{}\"", value));
            }
        }
    }
    format!("{{{}}}", pairs.join(", "))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_skill() {
        let content = r#"---
name: Shell Patterns
description: Background job workflows
requires_capabilities: [builtins, shell]
---

# Shell Patterns

When running background jobs..."#;

        let doc = parse_skill_md(
            SkillId("shell-patterns".to_string()),
            SkillScope::Builtin,
            content,
        )
        .unwrap();

        assert_eq!(doc.descriptor.name, "Shell Patterns");
        assert_eq!(doc.descriptor.description, "Background job workflows");
        assert_eq!(doc.descriptor.requires_capabilities, vec!["builtins", "shell"]);
        assert!(doc.body.contains("# Shell Patterns"));
    }

    #[test]
    fn test_parse_no_capabilities() {
        let content = r#"---
name: MCP Setup
description: Configure MCP servers
---

Content here"#;

        let doc = parse_skill_md(
            SkillId("mcp-setup".to_string()),
            SkillScope::Project,
            content,
        )
        .unwrap();

        assert!(doc.descriptor.requires_capabilities.is_empty());
    }

    #[test]
    fn test_missing_frontmatter() {
        let content = "# No frontmatter";
        let result = parse_skill_md(
            SkillId("test".to_string()),
            SkillScope::Builtin,
            content,
        );
        assert!(result.is_err());
    }
}
