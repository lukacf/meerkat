//! §10.4 secret hygiene: curated gitleaks-class scanning on the memory
//! write path.
//!
//! Deliberately small and high-precision — four pattern classes, matched
//! with plain deterministic scanning (no regex dependency, no entropy
//! heuristics): AWS access key ids, GitHub tokens, private-key headers,
//! and quoted/assigned credential values under well-known key names.
//! A match REFUSES the write with the class named (never a silent
//! redaction, per the architecture doc's §10.4 posture): an agent that saw
//! a live credential in tool output must not be able to persist it into
//! durable memory, where it would re-enter every future build's context
//! and every read surface.
//!
//! Enforced at the staged-validator chokepoint
//! (`staged::check_record_payload` — every write path stages a batch) and
//! at the proposal seam (`propose` inserts a row without staging).

/// Pattern classes, named in refusal errors. Stable strings — callers and
/// tests match on them.
pub const CLASS_AWS_ACCESS_KEY_ID: &str = "aws-access-key-id";
pub const CLASS_GITHUB_TOKEN: &str = "github-token";
pub const CLASS_PRIVATE_KEY: &str = "private-key";
pub const CLASS_CREDENTIAL_ASSIGNMENT: &str = "credential-assignment";

/// Scan one free-text field. Returns the first matching pattern class.
pub fn detect_secret(text: &str) -> Option<&'static str> {
    if has_aws_access_key_id(text) {
        return Some(CLASS_AWS_ACCESS_KEY_ID);
    }
    if has_github_token(text) {
        return Some(CLASS_GITHUB_TOKEN);
    }
    if has_private_key_header(text) {
        return Some(CLASS_PRIVATE_KEY);
    }
    if has_credential_assignment(text) {
        return Some(CLASS_CREDENTIAL_ASSIGNMENT);
    }
    None
}

/// Scan every free-text field of a record payload (title, description,
/// body, tags — everything an author controls).
pub fn detect_record_secret(
    title: &str,
    description: &str,
    body: &str,
    tags: &[String],
) -> Option<&'static str> {
    detect_secret(title)
        .or_else(|| detect_secret(description))
        .or_else(|| detect_secret(body))
        .or_else(|| tags.iter().find_map(|tag| detect_secret(tag)))
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `AKIA`/`ASIA` + 16 uppercase-alphanumeric chars, on word boundaries —
/// the canonical AWS access key id shape (gitleaks `aws-access-key-id`,
/// tightened to the always-uppercase real format).
fn has_aws_access_key_id(text: &str) -> bool {
    for prefix in ["AKIA", "ASIA"] {
        let mut search_from = 0;
        while let Some(offset) = text[search_from..].find(prefix) {
            let start = search_from + offset;
            let rest = &text[start + prefix.len()..];
            let tail: Vec<char> = rest.chars().take(17).collect();
            let boundary_before = text[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !is_word_char(c));
            let body_ok = tail.len() >= 16
                && tail[..16]
                    .iter()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
            let boundary_after = tail.get(16).is_none_or(|c| !is_word_char(*c));
            if boundary_before && body_ok && boundary_after {
                return true;
            }
            search_from = start + prefix.len();
        }
    }
    false
}

/// GitHub token prefixes (`ghp_`/`gho_`/`ghu_`/`ghs_`/`ghr_` + ≥36
/// alphanumerics, `github_pat_` + ≥22 alphanumerics/underscores).
fn has_github_token(text: &str) -> bool {
    let classic = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"];
    for prefix in classic {
        if prefixed_run_at_least(text, prefix, 36, |c| c.is_ascii_alphanumeric()) {
            return true;
        }
    }
    prefixed_run_at_least(text, "github_pat_", 22, |c| {
        c.is_ascii_alphanumeric() || c == '_'
    })
}

/// True when `prefix` occurs on a word boundary followed by at least
/// `min_len` chars accepted by `allowed`.
fn prefixed_run_at_least(
    text: &str,
    prefix: &str,
    min_len: usize,
    allowed: fn(char) -> bool,
) -> bool {
    let mut search_from = 0;
    while let Some(offset) = text[search_from..].find(prefix) {
        let start = search_from + offset;
        let boundary_before = text[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let run = text[start + prefix.len()..]
            .chars()
            .take_while(|c| allowed(*c))
            .count();
        if boundary_before && run >= min_len {
            return true;
        }
        search_from = start + prefix.len();
    }
    false
}

/// PEM private-key header (`-----BEGIN … PRIVATE KEY-----`, any algorithm
/// qualifier). The closing `PRIVATE KEY-----` run only ever appears in the
/// armor markers themselves.
fn has_private_key_header(text: &str) -> bool {
    text.contains("PRIVATE KEY-----") && text.contains("-----BEGIN")
}

/// Well-known credential key names assigned a long token-shaped value:
/// `api_key = "zXy1…"`, `client_secret: abc123…`. High-precision by
/// construction: the value must be ≥16 token chars AND contain a digit
/// (prose like "the api_key lives in the vault" and placeholders like
/// "your-api-key-here" never match).
fn has_credential_assignment(text: &str) -> bool {
    const KEY_NAMES: &[&str] = &[
        "api_key",
        "api-key",
        "apikey",
        "secret_key",
        "secret-key",
        "client_secret",
        "access_token",
        "access-token",
        "auth_token",
        "auth-token",
        "aws_secret_access_key",
        "private_token",
    ];
    let lower = text.to_ascii_lowercase();
    for name in KEY_NAMES {
        let mut search_from = 0;
        while let Some(offset) = lower[search_from..].find(name) {
            let start = search_from + offset;
            search_from = start + name.len();
            // `-` counts as a boundary so header-style names (`x-api-key:`)
            // still match.
            let boundary_before = lower[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !is_word_char(c));
            if !boundary_before {
                continue;
            }
            // Original-case tail (the value's case matters for precision).
            let tail = &text[start + name.len()..];
            if assignment_value_follows(tail) {
                return true;
            }
        }
    }
    false
}

/// After the key name: optional spaces, `=` or `:`, optional spaces,
/// optional quote, then ≥16 consecutive token chars including ≥1 digit.
fn assignment_value_follows(tail: &str) -> bool {
    let mut chars = tail.chars().peekable();
    while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
        chars.next();
    }
    if !chars.peek().is_some_and(|c| *c == '=' || *c == ':') {
        return false;
    }
    chars.next();
    while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
        chars.next();
    }
    if chars.peek().is_some_and(|c| *c == '"' || *c == '\'') {
        chars.next();
    }
    let mut len = 0usize;
    let mut has_digit = false;
    for c in chars {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '+' | '=' | '.') {
            len += 1;
            has_digit |= c.is_ascii_digit();
        } else {
            break;
        }
    }
    len >= 16 && has_digit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_access_key_ids_detected_on_boundaries_only() {
        assert_eq!(
            detect_secret("found AKIAIOSFODNN7EXAMPLE in the logs"),
            Some(CLASS_AWS_ACCESS_KEY_ID)
        );
        assert_eq!(
            detect_secret("ASIA0123456789ABCDEF"),
            Some(CLASS_AWS_ACCESS_KEY_ID)
        );
        // Embedded in a longer word / too short / lowercase: no match.
        assert_eq!(detect_secret("xAKIAIOSFODNN7EXAMPLE"), None);
        assert_eq!(detect_secret("AKIAIOSFODNN7EXAMPLEX"), None);
        assert_eq!(detect_secret("AKIA too short"), None);
        assert_eq!(detect_secret("akiaiosfodnn7example"), None);
    }

    #[test]
    fn github_tokens_detected() {
        assert_eq!(
            detect_secret("use ghp_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789"),
            Some(CLASS_GITHUB_TOKEN)
        );
        assert_eq!(
            detect_secret("github_pat_11ABCDEFG0123456789abcdef_more"),
            Some(CLASS_GITHUB_TOKEN)
        );
        assert_eq!(detect_secret("ghp_tooshort"), None);
        assert_eq!(detect_secret("the ghp_ prefix marks classic tokens"), None);
    }

    #[test]
    fn private_key_headers_detected() {
        assert_eq!(
            detect_secret("-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk..."),
            Some(CLASS_PRIVATE_KEY)
        );
        assert_eq!(
            detect_secret("-----BEGIN PRIVATE KEY-----"),
            Some(CLASS_PRIVATE_KEY)
        );
        // Talking ABOUT private keys is fine.
        assert_eq!(detect_secret("rotate the private key monthly"), None);
        assert_eq!(detect_secret("-----BEGIN CERTIFICATE-----"), None);
    }

    #[test]
    fn credential_assignments_detected_with_precision() {
        assert_eq!(
            detect_secret("api_key = \"zXy1aB2cD3eF4gH5iJ6k\""),
            Some(CLASS_CREDENTIAL_ASSIGNMENT)
        );
        assert_eq!(
            detect_secret("CLIENT_SECRET: 9f8e7d6c5b4a39281706abcd"),
            Some(CLASS_CREDENTIAL_ASSIGNMENT)
        );
        assert_eq!(
            detect_secret("export ACCESS_TOKEN='tok4567890123456789'"),
            Some(CLASS_CREDENTIAL_ASSIGNMENT)
        );
        // Prose, placeholders (no digit), and short values pass.
        assert_eq!(detect_secret("the api_key lives in the vault"), None);
        assert_eq!(detect_secret("api_key = your-api-key-goes-here"), None);
        assert_eq!(detect_secret("api_key = abc123"), None);
        assert_eq!(detect_secret("set secret_key rotation to quarterly"), None);
    }

    #[test]
    fn record_scan_covers_every_field() {
        let tags = vec!["ok".to_string(), "AKIAIOSFODNN7EXAMPLE".to_string()];
        assert_eq!(
            detect_record_secret("t", "d", "b", &tags),
            Some(CLASS_AWS_ACCESS_KEY_ID)
        );
        assert_eq!(
            detect_record_secret("-----BEGIN EC PRIVATE KEY-----", "", "b", &[]),
            Some(CLASS_PRIVATE_KEY)
        );
        assert_eq!(
            detect_record_secret("t", "api_key = \"zXy1aB2cD3eF4gH5iJ6k\"", "b", &[]),
            Some(CLASS_CREDENTIAL_ASSIGNMENT)
        );
        assert_eq!(detect_record_secret("t", "d", "b", &[]), None);
    }
}
