//! Skill rendering — inventory sections and injection blocks.

use meerkat_core::skills::SkillDescriptor;

/// Maximum injection content size (32 KiB).
pub const MAX_INJECTION_BYTES: usize = 32 * 1024;

/// Render the skill inventory section for the system prompt.
pub fn render_inventory(skills: &[SkillDescriptor]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut output = String::from("## Available Skills\n");
    for skill in skills {
        output.push_str(&format!(
            "\n- `/{id}` — {desc}",
            id = skill.id,
            desc = skill.description,
        ));
    }
    output
}

/// Render a per-turn skill injection block.
pub fn render_injection(name: &str, body: &str) -> String {
    let mut content = format!("<skill name=\"{name}\">\n{body}\n</skill>");

    if content.len() > MAX_INJECTION_BYTES {
        tracing::warn!(
            "Skill injection for '{}' truncated from {} to {} bytes",
            name,
            content.len(),
            MAX_INJECTION_BYTES,
        );
        content.truncate(MAX_INJECTION_BYTES);
    }

    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::skills::{SkillId, SkillScope};

    #[test]
    fn test_render_inventory() {
        let skills = vec![
            SkillDescriptor {
                id: SkillId("task-workflow".to_string()),
                name: "Task Workflow".to_string(),
                description: "How to use task tools".to_string(),
                scope: SkillScope::Builtin,
                ..Default::default()
            },
            SkillDescriptor {
                id: SkillId("shell-patterns".to_string()),
                name: "Shell Patterns".to_string(),
                description: "Background job workflows".to_string(),
                scope: SkillScope::Builtin,
                ..Default::default()
            },
        ];

        let output = render_inventory(&skills);
        assert!(output.contains("## Available Skills"));
        assert!(output.contains("`/task-workflow`"));
        assert!(output.contains("`/shell-patterns`"));
    }

    #[test]
    fn test_render_inventory_empty() {
        assert!(render_inventory(&[]).is_empty());
    }

    #[test]
    fn test_render_injection() {
        let output = render_injection("shell-patterns", "# Shell Patterns\n\nContent here.");
        assert!(output.starts_with("<skill name=\"shell-patterns\">"));
        assert!(output.ends_with("</skill>"));
        assert!(output.contains("# Shell Patterns"));
    }
}
