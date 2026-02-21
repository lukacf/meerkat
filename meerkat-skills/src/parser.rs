//! SKILL.md frontmatter parser.
//!
//! Uses `serde_yml` for robust YAML parsing of the frontmatter block.

use indexmap::IndexMap;
use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillId, SkillName, SkillScope,
};
use serde_yml::Value;

/// Parsed frontmatter from a SKILL.md file.
#[derive(Debug, serde::Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default)]
    requires_capabilities: Vec<String>,
    #[serde(default)]
    metadata: IndexMap<String, String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(flatten)]
    extensions: IndexMap<String, Value>,
}

/// Parse a SKILL.md file into a `SkillDocument`.
///
/// The file format is:
/// ```text
/// ---
/// name: Shell Patterns
/// description: "Background job workflows: patterns and tips"
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
    expected_skill_name: Option<&str>,
) -> Result<SkillDocument, SkillError> {
    let (frontmatter_str, body) = split_frontmatter(content)?;
    let fm: Frontmatter = serde_yml::from_str(&frontmatter_str)
        .map_err(|e| SkillError::Parse(format!("frontmatter parse error: {e}").into()))?;
    validate_frontmatter(&fm, expected_skill_name)?;

    let mut metadata = fm.metadata;
    if let Some(version) = fm.version {
        metadata.insert("version".to_string(), version);
    }

    let mut extensions = IndexMap::new();
    for (key, value) in fm.extensions {
        let serialized = if let Some(s) = value.as_str() {
            s.to_string()
        } else {
            serde_yml::to_string(&value)
                .map_err(|e| SkillError::Parse(format!("extension serialize error: {e}").into()))?
                .trim()
                .to_string()
        };
        extensions.insert(key, serialized);
    }

    Ok(SkillDocument {
        descriptor: SkillDescriptor {
            id,
            name: fm.name,
            description: fm.description,
            scope,
            requires_capabilities: fm.requires_capabilities,
            metadata,
            ..Default::default()
        },
        body: body.to_string(),
        extensions,
    })
}

fn validate_frontmatter(
    fm: &Frontmatter,
    expected_skill_name: Option<&str>,
) -> Result<(), SkillError> {
    let skill_name = SkillName::parse(&fm.name)?;
    if let Some(expected) = expected_skill_name
        && skill_name.as_str() != expected
    {
        return Err(SkillError::Parse(
            format!(
                "frontmatter name '{}' must match directory slug '{}'",
                fm.name, expected
            )
            .into(),
        ));
    }

    for key in fm.extensions.keys() {
        let Some((namespace, suffix)) = key.split_once('.') else {
            return Err(SkillError::Parse(
                format!(
                    "unknown frontmatter field '{key}': extension keys must be namespaced as 'vendor.key'"
                )
                .into(),
            ));
        };
        if namespace.is_empty() || suffix.is_empty() {
            return Err(SkillError::Parse(
                format!(
                    "invalid extension key '{key}': expected non-empty namespace and key segments"
                )
                .into(),
            ));
        }
    }

    for (key, value) in &fm.extensions {
        let Some((namespace, suffix)) = key.split_once('.') else {
            continue;
        };
        if suffix == "version" {
            continue;
        }
        let nontrivial = matches!(value, Value::Mapping(_) | Value::Sequence(_));
        if nontrivial && !fm.extensions.contains_key(&format!("{namespace}.version")) {
            return Err(SkillError::Parse(
                format!(
                    "extension '{key}' requires companion '{namespace}.version' for nontrivial values"
                )
                .into(),
            ));
        }
    }

    Ok(())
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_skill() {
        let content = r#"---
name: shell-patterns
description: Background job workflows
requires_capabilities: [builtins, shell]
---

# shell-patterns

When running background jobs..."#;

        let doc = parse_skill_md(
            SkillId("shell-patterns".to_string()),
            SkillScope::Builtin,
            content,
            None,
        )
        .unwrap();

        assert_eq!(doc.descriptor.name, "shell-patterns");
        assert_eq!(doc.descriptor.description, "Background job workflows");
        assert_eq!(
            doc.descriptor.requires_capabilities,
            vec!["builtins", "shell"]
        );
        assert!(doc.body.contains("# shell-patterns"));
    }

    #[test]
    fn test_parse_no_capabilities() {
        let content =
            "---\nname: mcp-setup\ndescription: Configure MCP servers\n---\n\nContent here";
        let doc = parse_skill_md(
            SkillId("mcp-setup".to_string()),
            SkillScope::Project,
            content,
            None,
        )
        .unwrap();
        assert!(doc.descriptor.requires_capabilities.is_empty());
    }

    #[test]
    fn test_parse_description_with_colon() {
        let content = "---\nname: test\ndescription: \"Use: this tool carefully\"\n---\n\nBody";
        let doc = parse_skill_md(
            SkillId("test".to_string()),
            SkillScope::Builtin,
            content,
            None,
        )
        .unwrap();
        assert_eq!(doc.descriptor.description, "Use: this tool carefully");
    }

    #[test]
    fn test_missing_frontmatter() {
        let content = "# No frontmatter";
        let result = parse_skill_md(
            SkillId("test".to_string()),
            SkillScope::Builtin,
            content,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_unnamespaced_unknown_field() {
        let content = r#"---
name: test-skill
description: d
custom_field: true
---
body"#;
        let result = parse_skill_md(
            SkillId("test-skill".to_string()),
            SkillScope::Builtin,
            content,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_nontrivial_extension_without_version() {
        let content = r#"---
name: test-skill
description: d
acme.config:
  mode: strict
---
body"#;
        let result = parse_skill_md(
            SkillId("test-skill".to_string()),
            SkillScope::Builtin,
            content,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_accepts_namespaced_extension_with_version() {
        let content = r#"---
name: test-skill
description: d
acme.version: "1"
acme.config:
  mode: strict
---
body"#;
        let doc = parse_skill_md(
            SkillId("test-skill".to_string()),
            SkillScope::Builtin,
            content,
            None,
        )
        .unwrap();
        assert!(doc.extensions.contains_key("acme.config"));
        assert_eq!(
            doc.extensions.get("acme.version").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn test_rejects_name_directory_mismatch() {
        let content = r#"---
name: wrong-name
description: d
---
body"#;
        let result = parse_skill_md(
            SkillId("skills/correct-name".to_string()),
            SkillScope::Builtin,
            content,
            Some("correct-name"),
        );
        assert!(result.is_err());
    }
}
