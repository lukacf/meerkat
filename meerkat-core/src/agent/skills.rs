//! Skill reference detection for per-turn activation.
//!
//! Detects `/skill-ref` patterns at the start of user messages and extracts
//! the skill ID and remaining message text.

use regex::Regex;
use std::sync::OnceLock;

/// Regex for detecting skill references at the start of a message.
/// Matches: `/segment` or `/segment/segment/...` followed by whitespace or end of string.
fn skill_ref_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        match Regex::new(r"^/([a-z0-9-]+(?:/[a-z0-9-]+)*)(?:\s+(.*))?$") {
            Ok(re) => re,
            Err(_) => unreachable!("static regex pattern is valid"),
        }
    })
}

/// Detect a `/skill-ref` at the start of a message.
///
/// Returns `Some((skill_id, remaining_text))` if the message starts with a
/// valid skill reference. The skill_id is the canonical ID (without leading `/`).
/// The remaining_text is the message content after the skill reference (trimmed).
///
/// Returns `None` if the message doesn't start with a skill reference.
pub fn detect_skill_ref(message: &str) -> Option<(&str, &str)> {
    let captures = skill_ref_regex().captures(message)?;
    let skill_id = captures.get(1)?.as_str();
    let remaining = captures.get(2).map(|m| m.as_str()).unwrap_or("");
    Some((skill_id, remaining))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_skill_ref_simple() {
        let result = detect_skill_ref("/pdf-processing hello");
        assert_eq!(result, Some(("pdf-processing", "hello")));
    }

    #[test]
    fn test_detect_skill_ref_namespaced() {
        let result = detect_skill_ref("/extraction/email hello");
        assert_eq!(result, Some(("extraction/email", "hello")));
    }

    #[test]
    fn test_detect_skill_ref_deep() {
        let result = detect_skill_ref("/a/b/c rest of the message");
        assert_eq!(result, Some(("a/b/c", "rest of the message")));
    }

    #[test]
    fn test_detect_skill_ref_none() {
        let result = detect_skill_ref("hello world");
        assert_eq!(result, None);
    }

    #[test]
    fn test_detect_skill_ref_midsentence() {
        let result = detect_skill_ref("use /extraction/email for this");
        assert_eq!(result, None); // Must be at start
    }

    #[test]
    fn test_detect_skill_ref_only() {
        let result = detect_skill_ref("/pdf-processing");
        assert_eq!(result, Some(("pdf-processing", "")));
    }

    #[test]
    fn test_strip_skill_ref() {
        let (skill_id, remaining) =
            detect_skill_ref("/extraction/email extract stuff").unwrap();
        assert_eq!(skill_id, "extraction/email");
        assert_eq!(remaining, "extract stuff");
    }
}
