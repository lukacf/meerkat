//! Skill rendering — inventory sections and injection blocks.
//!
//! Produces XML-format output for system prompt injection:
//! - `<available_skills>` inventory (flat or collection mode)
//! - `<skill id="...">` injection blocks

use meerkat_core::skills::{SkillCollection, SkillDescriptor, SkillKey};
use regex::Regex;
use std::borrow::Cow;
use std::fmt::Write;
use std::sync::OnceLock;

/// Maximum injection content size (32 KiB).
pub const MAX_INJECTION_BYTES: usize = 32 * 1024;

/// Default threshold: skill count <= this → flat listing, > this → collections.
pub const DEFAULT_INVENTORY_THRESHOLD: usize = 12;

/// Regex matching `</skill>` variants: case-insensitive, optional whitespace.
///
/// The pattern is a static literal and compilation cannot fail.
fn closing_tag_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Static pattern — verified by tests. Use match to satisfy no-expect rule.
        match Regex::new(r"(?i)</skill\s*>") {
            Ok(re) => re,
            Err(_) => unreachable!("static regex pattern is valid"),
        }
    })
}

/// Render the skill inventory section for the system prompt.
///
/// When `skills.len() <= threshold`, renders a flat XML listing.
/// When `skills.len() > threshold`, renders a collection summary.
pub fn render_inventory(
    skills: &[SkillDescriptor],
    collections: &[SkillCollection],
    threshold: usize,
) -> String {
    if skills.is_empty() {
        return String::new();
    }

    if skills.len() <= threshold {
        render_inventory_flat(skills)
    } else {
        render_inventory_collections(collections)
    }
}

/// Escape XML-special characters in text content.
/// Note: O(5n) worst case — five sequential replace() calls, each allocating.
/// Acceptable for short strings (skill names/descriptions). For large content,
/// consider a single-pass escaper.
fn escape_xml(s: &str) -> Cow<'_, str> {
    if s.contains('&') || s.contains('<') || s.contains('>') || s.contains('"') || s.contains('\'')
    {
        Cow::Owned(
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&apos;"),
        )
    } else {
        Cow::Borrowed(s)
    }
}

/// Flat XML listing of all skills.
fn render_inventory_flat(skills: &[SkillDescriptor]) -> String {
    let mut output = String::from("<available_skills>\n");
    for skill in skills {
        let rendered_id = format!("{}/{}", skill.key.source_uuid, skill.key.skill_name);
        let _ = write!(
            output,
            "  <skill id=\"{}\">\n    <description>{}</description>\n  </skill>\n",
            escape_xml(&rendered_id),
            escape_xml(&skill.description),
        );
    }
    output.push_str("</available_skills>");
    output
}

/// Collection summary with tool hints.
fn render_inventory_collections(collections: &[SkillCollection]) -> String {
    let mut output = String::from("<available_skills mode=\"collections\">\n");
    for coll in collections {
        let source_id = coll.source_uuid.to_string();
        let _ = writeln!(
            output,
            "  <collection source=\"{}\" count=\"{}\">{}</collection>",
            escape_xml(&source_id),
            coll.count,
            escape_xml(&coll.description),
        );
    }
    output.push('\n');
    output.push_str("  Use the browse_skills tool to list skills in a source or search.\n");
    output.push_str("  Use the load_skill tool to activate a skill by SkillKey.\n");
    output.push_str("</available_skills>");
    output
}

/// Render a per-turn skill injection block.
///
/// Escapes `</skill>` variants in the body (case-insensitive, optional whitespace)
/// before truncation. Returns the `<skill id="...">body</skill>` XML block.
pub fn render_injection(key: &SkillKey, body: &str) -> String {
    render_injection_with_limit(key, body, MAX_INJECTION_BYTES)
}

/// Render a per-turn skill injection block with a configurable size limit.
pub fn render_injection_with_limit(key: &SkillKey, body: &str, max_bytes: usize) -> String {
    // 1. Escape closing tags in the body BEFORE wrapping/truncating.
    let escaped_body = escape_closing_tags(body);

    // 2. Wrap in <skill> tags (escape id to prevent attribute breakout).
    let rendered_id = format!("{}/{}", key.source_uuid, key.skill_name);
    let escaped_id = escape_xml(&rendered_id);
    let mut content = format!("<skill id=\"{escaped_id}\">\n{escaped_body}\n</skill>");

    // 3. Truncate if needed (after escaping). Use char boundary to avoid UTF-8 split.
    if content.len() > max_bytes {
        tracing::warn!(
            "Skill injection for '{}' truncated from {} to {} bytes",
            rendered_id,
            content.len(),
            max_bytes,
        );
        let marker = "[truncated]";
        let target = max_bytes - marker.len();
        // Find a valid char boundary at or before the target position.
        let boundary = floor_char_boundary(&content, target);
        content.truncate(boundary);
        content.push_str(marker);
    }

    content
}

/// Escape any `</skill...>` variants in skill body content.
/// Returns `Cow::Borrowed` when no escaping is needed (common case — zero allocation).
fn escape_closing_tags(body: &str) -> Cow<'_, str> {
    closing_tag_regex().replace_all(body, r"<\/skill>")
}

/// Find the largest index `<= pos` that is a char boundary.
/// Equivalent to the unstable `str::floor_char_boundary`.
fn floor_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut i = pos;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use meerkat_core::skills::{SkillName, SkillScope, SourceUuid};

    fn test_key(skill: &str) -> SkillKey {
        SkillKey {
            source_uuid: SourceUuid::builtin(),
            skill_name: SkillName::parse(skill).unwrap(),
        }
    }

    fn make_descriptor(skill: &str, desc: &str) -> SkillDescriptor {
        SkillDescriptor {
            key: test_key(skill),
            name: skill.to_string(),
            description: desc.into(),
            scope: SkillScope::Builtin,
            metadata: IndexMap::new(),
            capability_requirements: Vec::new(),
            source_name: String::new(),
        }
    }

    fn make_collection(desc: &str, count: usize) -> SkillCollection {
        SkillCollection {
            source_uuid: SourceUuid::builtin(),
            description: desc.into(),
            count,
        }
    }

    // --- Inventory tests ---

    #[test]
    fn test_render_inventory_flat_xml() {
        let skills = vec![
            make_descriptor("email-extractor", "Extract entities from emails"),
            make_descriptor("markdown", "Markdown output"),
        ];
        let collections = vec![];
        let output = render_inventory(&skills, &collections, 12);

        assert!(output.starts_with("<available_skills>"));
        assert!(output.ends_with("</available_skills>"));
        assert!(output.contains("email-extractor"));
        assert!(output.contains("<description>Extract entities from emails</description>"));
        assert!(output.contains("markdown"));
        assert!(!output.contains("mode="));
    }

    #[test]
    fn test_render_inventory_collections_xml() {
        let skills: Vec<SkillDescriptor> = (0..15)
            .map(|i| make_descriptor(&format!("skill-{i}"), &format!("Skill {i}")))
            .collect();
        let collections = vec![make_collection("Entity extraction", 8)];
        let output = render_inventory(&skills, &collections, 12);

        assert!(output.starts_with("<available_skills mode=\"collections\">"));
        assert!(output.ends_with("</available_skills>"));
        assert!(output.contains("<collection source="));
        assert!(output.contains("Entity extraction</collection>"));
        assert!(output.contains("browse_skills"));
        assert!(output.contains("load_skill"));
        assert!(!output.contains("<skill id="));
    }

    #[test]
    fn test_render_inventory_empty() {
        let output = render_inventory(&[], &[], 12);
        assert!(output.is_empty());
    }

    // --- Injection tests ---

    #[test]
    fn test_render_injection_xml() {
        let key = test_key("email-extractor");
        let output = render_injection(&key, "# Email Extractor\n\nExtract entities.");
        assert!(output.starts_with("<skill id=\""));
        assert!(output.contains("email-extractor"));
        assert!(output.ends_with("</skill>"));
        assert!(output.contains("# Email Extractor"));
    }

    #[test]
    fn test_injection_escapes_closing_tag() {
        let key = test_key("skill");
        let body = "Some text </skill> more text";
        let output = render_injection(&key, body);
        assert!(output.contains(r"<\/skill>"));
        assert!(!output.contains("</skill> more"));
    }

    #[test]
    fn test_injection_escapes_whitespace_tag() {
        let key = test_key("skill");
        let body = "Text </skill  > end";
        let output = render_injection(&key, body);
        assert!(output.contains(r"<\/skill>"));
        assert!(!output.contains("</skill  >"));
    }

    #[test]
    fn test_injection_escapes_case_insensitive() {
        let key = test_key("skill");
        let body = "Text </SKILL> end";
        let output = render_injection(&key, body);
        assert!(output.contains(r"<\/skill>"));
        assert!(!output.contains("</SKILL>"));
    }

    #[test]
    fn test_injection_truncation() {
        let key = test_key("skill");
        let body = "x".repeat(MAX_INJECTION_BYTES + 1000);
        let output = render_injection(&key, &body);
        assert!(output.len() <= MAX_INJECTION_BYTES);
        assert!(output.ends_with("[truncated]"));
    }
}
