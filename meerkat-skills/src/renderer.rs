//! Skill rendering — inventory sections and injection blocks.
//!
//! Produces XML-format output for system prompt injection:
//! - `<available_skills>` inventory (flat or collection mode)
//! - `<skill id="...">` injection blocks

use meerkat_core::skills::{SkillCollection, SkillDescriptor};
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
        let _ = write!(
            output,
            "  <skill id=\"{}\">\n    <description>{}</description>\n  </skill>\n",
            escape_xml(&skill.id.0), escape_xml(&skill.description),
        );
    }
    output.push_str("</available_skills>");
    output
}

/// Collection summary with tool hints.
fn render_inventory_collections(collections: &[SkillCollection]) -> String {
    let mut output = String::from("<available_skills mode=\"collections\">\n");
    for coll in collections {
        let _ = writeln!(
            output,
            "  <collection path=\"{}\" count=\"{}\">{}</collection>",
            escape_xml(&coll.path), coll.count, escape_xml(&coll.description),
        );
    }
    output.push('\n');
    output.push_str(
        "  Use the browse_skills tool to list skills in a collection or search.\n",
    );
    output.push_str(
        "  Use the load_skill tool or /collection/skill-name to activate a skill.\n",
    );
    output.push_str("</available_skills>");
    output
}

/// Render a per-turn skill injection block.
///
/// Escapes `</skill>` variants in the body (case-insensitive, optional whitespace)
/// before truncation. Returns the `<skill id="...">body</skill>` XML block.
pub fn render_injection(id: &str, body: &str) -> String {
    render_injection_with_limit(id, body, MAX_INJECTION_BYTES)
}

/// Render a per-turn skill injection block with a configurable size limit.
pub fn render_injection_with_limit(id: &str, body: &str, max_bytes: usize) -> String {
    // 1. Escape closing tags in the body BEFORE wrapping/truncating.
    let escaped_body = escape_closing_tags(body);

    // 2. Wrap in <skill> tags.
    let mut content = format!("<skill id=\"{id}\">\n{escaped_body}\n</skill>");

    // 3. Truncate if needed (after escaping). Use char boundary to avoid UTF-8 split.
    if content.len() > max_bytes {
        tracing::warn!(
            "Skill injection for '{}' truncated from {} to {} bytes",
            id,
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
    use meerkat_core::skills::{SkillId, SkillScope};

    fn make_descriptor(id: &str, desc: &str) -> SkillDescriptor {
        SkillDescriptor {
            id: SkillId(id.into()),
            name: id.rsplit('/').next().unwrap_or(id).into(),
            description: desc.into(),
            scope: SkillScope::Builtin,
            ..Default::default()
        }
    }

    fn make_collection(path: &str, desc: &str, count: usize) -> SkillCollection {
        SkillCollection {
            path: path.into(),
            description: desc.into(),
            count,
        }
    }

    // --- Inventory tests ---

    #[test]
    fn test_render_inventory_flat_xml() {
        let skills = vec![
            make_descriptor("extraction/email-extractor", "Extract entities from emails"),
            make_descriptor("formatting/markdown", "Markdown output"),
        ];
        let collections = vec![];
        let output = render_inventory(&skills, &collections, 12);

        assert!(output.starts_with("<available_skills>"));
        assert!(output.ends_with("</available_skills>"));
        assert!(output.contains("<skill id=\"extraction/email-extractor\">"));
        assert!(output.contains("<description>Extract entities from emails</description>"));
        assert!(output.contains("<skill id=\"formatting/markdown\">"));
        // Should NOT have mode attribute
        assert!(!output.contains("mode="));
    }

    #[test]
    fn test_render_inventory_collections_xml() {
        // Create 15 skills (> threshold of 12)
        let skills: Vec<SkillDescriptor> = (0..15)
            .map(|i| make_descriptor(&format!("coll/skill-{i}"), &format!("Skill {i}")))
            .collect();
        let collections = vec![
            make_collection("extraction", "Entity extraction", 8),
            make_collection("formatting", "Output formatting", 3),
        ];
        let output = render_inventory(&skills, &collections, 12);

        assert!(output.starts_with("<available_skills mode=\"collections\">"));
        assert!(output.ends_with("</available_skills>"));
        assert!(output.contains("<collection path=\"extraction\" count=\"8\">"));
        assert!(output.contains("Entity extraction</collection>"));
        assert!(output.contains("browse_skills"));
        assert!(output.contains("load_skill"));
        // Should NOT contain individual skill elements
        assert!(!output.contains("<skill id="));
    }

    #[test]
    fn test_render_inventory_empty() {
        let output = render_inventory(&[], &[], 12);
        assert!(output.is_empty());
    }

    #[test]
    fn test_inventory_threshold_boundary() {
        let make_skills = |n: usize| -> Vec<SkillDescriptor> {
            (0..n)
                .map(|i| make_descriptor(&format!("coll/skill-{i}"), &format!("Skill {i}")))
                .collect()
        };
        let collections = vec![make_collection("coll", "Collection", 5)];

        // Exactly at threshold → flat
        let skills_at = make_skills(12);
        let output_at = render_inventory(&skills_at, &collections, 12);
        assert!(output_at.starts_with("<available_skills>"));
        assert!(!output_at.contains("mode="));

        // One above threshold → collections
        let skills_above = make_skills(13);
        let output_above = render_inventory(&skills_above, &collections, 12);
        assert!(output_above.starts_with("<available_skills mode=\"collections\">"));
    }

    // --- Injection tests ---

    #[test]
    fn test_render_injection_xml() {
        let output = render_injection(
            "extraction/email-extractor",
            "# Email Extractor\n\nExtract entities.",
        );
        assert!(output.starts_with("<skill id=\"extraction/email-extractor\">"));
        assert!(output.ends_with("</skill>"));
        assert!(output.contains("# Email Extractor"));
    }

    #[test]
    fn test_injection_escapes_closing_tag() {
        let body = "Some text </skill> more text";
        let output = render_injection("test/skill", body);
        assert!(output.contains(r"<\/skill>"));
        assert!(!output.contains("</skill> more"));
    }

    #[test]
    fn test_injection_escapes_whitespace_tag() {
        let body = "Text </skill  > end";
        let output = render_injection("test/skill", body);
        assert!(output.contains(r"<\/skill>"));
        assert!(!output.contains("</skill  >"));
    }

    #[test]
    fn test_injection_escapes_case_insensitive() {
        let body = "Text </SKILL> end";
        let output = render_injection("test/skill", body);
        assert!(output.contains(r"<\/skill>"));
        assert!(!output.contains("</SKILL>"));
    }

    #[test]
    fn test_injection_truncation() {
        let body = "x".repeat(MAX_INJECTION_BYTES + 1000);
        let output = render_injection("test/skill", &body);
        assert!(output.len() <= MAX_INJECTION_BYTES);
        assert!(output.ends_with("[truncated]"));
    }

    #[test]
    fn test_escape_before_truncate() {
        // Create body that's near MAX size and contains </skill> near the end.
        // Escaping changes "</skill>" (8 chars) to "<\/skill>" (10 chars),
        // so the escape must happen before truncation check.
        let padding = "a".repeat(MAX_INJECTION_BYTES - 100);
        let body = format!("{padding}</skill>end");
        let output = render_injection("test/skill", &body);
        // The escaped content should contain <\/skill> (the escape happened)
        // and the output should be truncated (escape made it longer)
        assert!(output.contains(r"<\/skill>") || output.ends_with("[truncated]"));
    }
}
