//! Plain-text line codec for unauthenticated event ingestion.
//!
//! Uses `LinesCodec` for newline-delimited framing and a pure function
//! `parse_plain_line()` for JSON auto-detection.

/// Parse a plain-text line into an event body.
///
/// Auto-detection logic:
/// - If the line is valid JSON with a `body` field → extract the body string
/// - If the line is valid JSON without a `body` field → pretty-print the whole object
/// - Otherwise → use the raw line as-is
pub fn parse_plain_line(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Try parsing as JSON
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(body) = value.get("body").and_then(|v| v.as_str()) {
            // JSON object with a "body" field — extract it
            return body.to_string();
        }
        if value.is_object() || value.is_array() {
            // JSON object/array without "body" — pretty-print
            return serde_json::to_string_pretty(&value).unwrap_or_else(|_| trimmed.to_string());
        }
    }

    // Plain text
    trimmed.to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_plain_line_json_with_body() {
        let result = parse_plain_line(r#"{"body":"hello world"}"#);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_parse_plain_line_json_without_body() {
        let result = parse_plain_line(r#"{"event":"new_email","from":"john"}"#);
        // Should pretty-print the JSON
        assert!(result.contains("new_email"));
        assert!(result.contains("john"));
        assert!(result.contains('\n')); // Pretty-printed
    }

    #[test]
    fn test_parse_plain_line_plain_text() {
        let result = parse_plain_line("plain text message");
        assert_eq!(result, "plain text message");
    }

    #[test]
    fn test_parse_plain_line_empty() {
        assert_eq!(parse_plain_line(""), "");
        assert_eq!(parse_plain_line("   "), "");
    }

    #[test]
    fn test_parse_plain_line_trims_whitespace() {
        let result = parse_plain_line("  hello  ");
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_parse_plain_line_json_string_literal() {
        // A JSON string literal (not an object) should be treated as plain text
        let result = parse_plain_line(r#""just a string""#);
        assert_eq!(result, r#""just a string""#);
    }

    #[test]
    fn test_parse_plain_line_json_array() {
        let result = parse_plain_line(r"[1, 2, 3]");
        // Arrays should be pretty-printed
        assert!(result.contains('1'));
        assert!(result.contains('2'));
        assert!(result.contains('3'));
    }

    #[test]
    fn test_parse_plain_line_malformed_json() {
        let result = parse_plain_line(r#"{"broken": json"#);
        assert_eq!(result, r#"{"broken": json"#);
    }

    #[test]
    fn test_parse_plain_line_body_not_string() {
        // body field exists but is not a string — pretty-print the whole object
        let result = parse_plain_line(r#"{"body": 42}"#);
        assert!(result.contains("42"));
    }
}
