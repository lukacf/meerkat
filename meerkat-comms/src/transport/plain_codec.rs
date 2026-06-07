//! Plain-text line codec for unauthenticated event ingestion.
//!
//! Uses `LinesCodec` for newline-delimited framing and a typed classifier
//! [`classify_plain_line`] that distinguishes structured (JSON) payloads from
//! raw text. The classifier is the single content authority for plain ingress:
//! it does NOT guess a `body` field out of arbitrary JSON — a structured
//! payload is rendered as its own canonical JSON, and text is text.

use serde_json::Value;

/// Typed classification of a single plain-ingress line.
///
/// Plain ingress is unauthenticated and lossy by design, but the *shape* of an
/// accepted line is explicit rather than inferred: a line is either empty,
/// a structured JSON payload, or raw text. No `body`-field heuristic decides
/// the meaning of a structured payload.
#[derive(Debug, Clone, PartialEq)]
pub enum PlainLine {
    /// The line was empty (or whitespace-only) and carries no event.
    Empty,
    /// The line parsed as a JSON object or array — a structured payload.
    Structured(Value),
    /// The line is raw plain text (including JSON scalars like a bare string
    /// or number, which are not structured payloads).
    Text(String),
}

impl PlainLine {
    /// Render this classified line into the body string carried by the inbox.
    ///
    /// - [`PlainLine::Empty`] → empty string (the listener skips it).
    /// - [`PlainLine::Structured`] → the canonical (pretty) JSON of the payload.
    /// - [`PlainLine::Text`] → the trimmed text verbatim.
    pub fn render(&self) -> String {
        match self {
            PlainLine::Empty => String::new(),
            PlainLine::Structured(value) => {
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            }
            PlainLine::Text(text) => text.clone(),
        }
    }

    /// Whether this classification carries no event payload.
    pub fn is_empty(&self) -> bool {
        matches!(self, PlainLine::Empty)
    }
}

/// Classify a plain-text line into a typed [`PlainLine`].
///
/// Classification rules (no `body`-field folklore):
/// - empty / whitespace-only → [`PlainLine::Empty`]
/// - valid JSON object or array → [`PlainLine::Structured`]
/// - everything else (raw text, JSON scalars, malformed JSON) → [`PlainLine::Text`]
pub fn classify_plain_line(line: &str) -> PlainLine {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return PlainLine::Empty;
    }

    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) if value.is_object() || value.is_array() => PlainLine::Structured(value),
        // JSON scalars (string/number/bool/null) and malformed JSON are not
        // structured payloads — they are raw text.
        _ => PlainLine::Text(trimmed.to_string()),
    }
}

/// Parse a plain-text line into an event body.
///
/// Thin rendering helper over the typed [`classify_plain_line`] authority,
/// retained for callers that consume the rendered body string directly.
pub fn parse_plain_line(line: &str) -> String {
    classify_plain_line(line).render()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_structured_object() {
        let classified = classify_plain_line(r#"{"event":"new_email","from":"john"}"#);
        assert!(matches!(classified, PlainLine::Structured(_)));
        let rendered = classified.render();
        assert!(rendered.contains("new_email"));
        assert!(rendered.contains("john"));
        assert!(rendered.contains('\n')); // Pretty-printed
    }

    /// ROW #241 gate: a structured payload that happens to contain a `body`
    /// field is NOT collapsed to that field — the typed contract renders the
    /// whole structured payload, killing the `body`-field guessing folklore.
    #[test]
    fn test_classify_structured_does_not_extract_body_field() {
        let classified = classify_plain_line(r#"{"body":"hello world","extra":1}"#);
        assert!(matches!(classified, PlainLine::Structured(_)));
        let rendered = classified.render();
        // The whole structured payload survives; we do NOT silently reduce it
        // to just the `body` field's value.
        assert!(rendered.contains("hello world"));
        assert!(rendered.contains("extra"));
        assert_ne!(rendered, "hello world");
    }

    #[test]
    fn test_classify_plain_text() {
        let classified = classify_plain_line("plain text message");
        assert_eq!(
            classified,
            PlainLine::Text("plain text message".to_string())
        );
        assert_eq!(classified.render(), "plain text message");
    }

    #[test]
    fn test_classify_empty() {
        assert_eq!(classify_plain_line(""), PlainLine::Empty);
        assert_eq!(classify_plain_line("   "), PlainLine::Empty);
        assert!(classify_plain_line("").is_empty());
    }

    #[test]
    fn test_classify_trims_whitespace() {
        let classified = classify_plain_line("  hello  ");
        assert_eq!(classified, PlainLine::Text("hello".to_string()));
    }

    #[test]
    fn test_classify_json_string_literal_is_text() {
        // A JSON string literal (a scalar, not an object/array) is plain text.
        let classified = classify_plain_line(r#""just a string""#);
        assert_eq!(
            classified,
            PlainLine::Text(r#""just a string""#.to_string())
        );
    }

    #[test]
    fn test_classify_json_array_is_structured() {
        let classified = classify_plain_line(r"[1, 2, 3]");
        assert!(matches!(classified, PlainLine::Structured(_)));
        let rendered = classified.render();
        assert!(rendered.contains('1'));
        assert!(rendered.contains('2'));
        assert!(rendered.contains('3'));
    }

    #[test]
    fn test_classify_malformed_json_is_text() {
        let classified = classify_plain_line(r#"{"broken": json"#);
        assert_eq!(
            classified,
            PlainLine::Text(r#"{"broken": json"#.to_string())
        );
    }

    #[test]
    fn test_classify_body_scalar_object_is_structured() {
        // An object whose `body` is a non-string is still a structured payload,
        // never a body extraction.
        let classified = classify_plain_line(r#"{"body": 42}"#);
        assert!(matches!(classified, PlainLine::Structured(_)));
        assert!(classified.render().contains("42"));
    }
}
