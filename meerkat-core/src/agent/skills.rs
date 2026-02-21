//! Skill reference detection for per-turn activation.
//!
//! Detects `/skill-ref` patterns at the start of user messages and extracts
//! the skill ID and remaining message text.

/// Detect a `/skill-ref` at the start of a message.
///
/// Returns `Some((skill_id, remaining_text))` if the message starts with a
/// valid skill reference. The skill_id is the canonical ID (without leading `/`).
/// The remaining_text is the message content after the skill reference (trimmed).
///
/// Returns `None` if the message doesn't start with a skill reference.
pub fn detect_skill_ref(message: &str) -> Option<(&str, &str)> {
    let trimmed = message.strip_prefix('/')?;
    let split_at = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let (candidate, trailing) = trimmed.split_at(split_at);
    if candidate.is_empty() {
        return None;
    }
    if !candidate.split('/').all(is_valid_segment) {
        return None;
    }
    Some((candidate, trailing.trim_start()))
}

fn is_valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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
        assert_eq!(
            detect_skill_ref("/extraction/email extract stuff"),
            Some(("extraction/email", "extract stuff"))
        );
    }
}
