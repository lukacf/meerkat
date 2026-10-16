//! `Debug` views for secret-bearing configuration.
//!
//! Config types carrying env values, header values, command arguments or URLs
//! implement `Debug` by hand with these views, so a `{:?}` in a log line or
//! error never prints credentials. Names stay visible; values do not.
//! [`REDACTED`] and [`RedactedUrl`] are public so other crates' credential
//! types print the same marker and URL shape.

use std::fmt;

/// Marker printed in place of a secret value.
pub const REDACTED: &str = "<redacted>";

/// A secret-bearing map's keys in sorted order, each value shown as
/// [`REDACTED`].
pub(crate) struct RedactedValues<'a>(Vec<&'a str>);

impl<'a> RedactedValues<'a> {
    pub(crate) fn of(keys: impl IntoIterator<Item = &'a String>) -> Self {
        let mut keys: Vec<&str> = keys.into_iter().map(String::as_str).collect();
        keys.sort_unstable();
        Self(keys)
    }
}

impl fmt::Debug for RedactedValues<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.0.iter().map(|key| (key, REDACTED)))
            .finish()
    }
}

/// A secret-bearing list: its length, every value shown as [`REDACTED`].
pub(crate) struct RedactedList(pub(crate) usize);

impl fmt::Debug for RedactedList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(std::iter::repeat_n(REDACTED, self.0))
            .finish()
    }
}

/// A URL with scheme, host and path kept; userinfo, query and fragment
/// replaced by [`REDACTED`].
pub struct RedactedUrl<'a>(pub &'a str);

impl fmt::Debug for RedactedUrl<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&redacted_url(self.0), f)
    }
}

/// `url` with scheme, host and path kept; userinfo, query and fragment
/// replaced by [`REDACTED`].
pub(crate) fn redacted_url(url: &str) -> String {
    let (base, suffix) = match url.find(['?', '#']) {
        Some(index) => (&url[..index], Some(&url[index..=index])),
        None => (url, None),
    };
    let (scheme, rest) = match base.find("://") {
        Some(index) => base.split_at(index + 3),
        None => ("", base),
    };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let mut shown = String::with_capacity(url.len());
    shown.push_str(scheme);
    match authority.rfind('@') {
        Some(index) => {
            shown.push_str(REDACTED);
            shown.push_str(&authority[index..]);
        }
        None => shown.push_str(authority),
    }
    shown.push_str(path);
    if let Some(separator) = suffix {
        shown.push_str(separator);
        shown.push_str(REDACTED);
    }
    shown
}

/// An optional secret: `None`, or `Some("<redacted>")`.
pub(crate) fn optional<T>(value: &Option<T>) -> Option<&'static str> {
    value.as_ref().map(|_| REDACTED)
}

/// Marker [`redact_secret_shapes`] (and exact-value redaction of the same
/// diagnostics) puts in place of a redacted value.
pub const SECRET_REDACTION_MARKER: &str = "[REDACTED]";

/// Replace obvious secret shapes in multi-line diagnostic text, keeping its
/// line structure: credentials after `Bearer`/`Basic` and in authorization
/// headers, values assigned to secret-looking keys (`api_key=`, `token:`,
/// `*_TOKEN=`, `*_SECRET=`, `*PASSWORD=` and similar), well-known token
/// prefixes, JSON web tokens, URL userinfo and PEM private-key blocks. Each
/// is replaced by [`SECRET_REDACTION_MARKER`].
///
/// Defence in depth only: this does not make arbitrary text safe to show. A
/// value that matches no rule passes through unchanged, so treat the output
/// with the same care as its input.
pub fn redact_secret_shapes(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut in_private_key = false;
    for line in text.split_inclusive('\n') {
        let (content, newline) = match line.strip_suffix('\n') {
            Some(content) => (content, "\n"),
            None => (line, ""),
        };
        if in_private_key {
            // Every line of the block, its end marker included, is dropped:
            // the block's begin line already became the marker.
            in_private_key = !is_private_key_end(content);
            continue;
        }
        if is_private_key_begin(content) {
            output.push_str(SECRET_REDACTION_MARKER);
            output.push_str(newline);
            // An unterminated block fails closed: nothing after it shows.
            in_private_key = !is_private_key_end(content);
            continue;
        }
        redact_line(content, &mut output);
        output.push_str(newline);
    }
    output
}

fn is_private_key_begin(line: &str) -> bool {
    line.contains("-----BEGIN ") && line.contains("PRIVATE KEY-----")
}

fn is_private_key_end(line: &str) -> bool {
    line.contains("-----END ") && line.contains("PRIVATE KEY-----")
}

/// Bytes that continue an identifier or a token.
fn is_token_byte(byte: u8) -> bool {
    crate::panic_payload::is_ascii_word_byte(byte) || byte == b'.'
}

fn at_left_boundary(bytes: &[u8], start: usize) -> bool {
    start == 0 || !is_token_byte(bytes[start - 1])
}

/// One line: every rule applies at each position, left to right; a quoted
/// value never reaches past the line.
fn redact_line(line: &str, output: &mut String) {
    let bytes = line.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if at_left_boundary(bytes, cursor) {
            // An authorization header: its whole value (scheme and
            // credentials) to the end of the line.
            if let Some(value_start) = authorization_header_at(line, cursor) {
                output.push_str(&line[cursor..value_start]);
                output.push_str(SECRET_REDACTION_MARKER);
                return;
            }
            if let Some(value_start) = credential_scheme_at(line, cursor) {
                output.push_str(&line[cursor..value_start]);
                cursor = crate::panic_payload::push_redacted_value(output, line, value_start);
                continue;
            }
            if let Some(key_end) = secret_key_at(line, cursor)
                && let Some(value_start) =
                    crate::panic_payload::assignment_value_start(line, key_end)
            {
                output.push_str(&line[cursor..value_start]);
                cursor = crate::panic_payload::push_redacted_value(output, line, value_start);
                continue;
            }
            if let Some(end) = secret_token_at(line, cursor) {
                output.push_str(SECRET_REDACTION_MARKER);
                cursor = end;
                continue;
            }
        }
        if let Some((userinfo_start, userinfo_end)) = url_userinfo_at(line, cursor) {
            output.push_str(&line[cursor..userinfo_start]);
            output.push_str(SECRET_REDACTION_MARKER);
            cursor = userinfo_end;
            continue;
        }
        let Some(character) = line[cursor..].chars().next() else {
            break;
        };
        output.push(character);
        cursor += character.len_utf8();
    }
}

/// `authorization` or `proxy-authorization` (any case) assigned with `:` or
/// `=`: the start of its value.
fn authorization_header_at(line: &str, start: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    ["proxy-authorization", "authorization"]
        .iter()
        .find_map(|name| {
            let end = start.checked_add(name.len())?;
            let candidate = bytes.get(start..end)?;
            (candidate.eq_ignore_ascii_case(name.as_bytes())
                && !bytes.get(end).copied().is_some_and(is_token_byte))
            .then_some(end)
        })
        .and_then(|key_end| crate::panic_payload::assignment_value_start(line, key_end))
}

/// `Bearer` or `Basic` (any case) followed by a credential of at least
/// eight token bytes: the credential's start. Shorter words after them
/// ("basic auth") are ordinary text.
fn credential_scheme_at(line: &str, start: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let scheme_end = crate::panic_payload::bearer_at(line, start).or_else(|| {
        let end = start.checked_add("basic".len())?;
        (bytes.get(start..end)?.eq_ignore_ascii_case(b"basic")
            && !bytes.get(end).copied().is_some_and(is_token_byte)
            && bytes.get(end).is_some_and(u8::is_ascii_whitespace))
        .then_some(end)
    })?;
    let mut value_start = scheme_end;
    while bytes.get(value_start).is_some_and(u8::is_ascii_whitespace) {
        value_start += 1;
    }
    let credential = bytes[value_start..]
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || b"-._~+/=".contains(byte))
        .count();
    (credential >= 8).then_some(value_start)
}

/// The words a secret-looking key is or ends with.
const SECRET_KEY_WORDS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "pwd",
    "key",
    "apikey",
    "credential",
    "credentials",
];

/// An identifier that is, or ends with, a secret word (`api_key`,
/// `DB_PASSWORD`, `MY_SERVICE_TOKEN`, `accessToken`): its end. The suffix
/// must start a part of the identifier: after `_`, `-` or `.`, or as a
/// camel-case word.
fn secret_key_at(line: &str, start: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let len = bytes[start..]
        .iter()
        .take_while(|byte| is_token_byte(**byte))
        .count();
    if len == 0 {
        return None;
    }
    let end = start + len;
    let identifier = &bytes[start..end];
    SECRET_KEY_WORDS
        .iter()
        .any(|word| {
            let word = word.as_bytes();
            let Some(suffix_start) = identifier.len().checked_sub(word.len()) else {
                return false;
            };
            if !identifier[suffix_start..].eq_ignore_ascii_case(word) {
                return false;
            }
            suffix_start == 0
                || matches!(identifier[suffix_start - 1], b'_' | b'-' | b'.')
                || (identifier[suffix_start].is_ascii_uppercase()
                    && identifier[suffix_start - 1].is_ascii_lowercase())
        })
        .then_some(end)
}

/// Well-known token prefixes: the token must be at least this much longer
/// than its prefix, so ordinary words sharing a prefix (`sk-learn`) stay.
const MIN_TOKEN_BODY_BYTES: usize = 8;

const SECRET_TOKEN_PREFIXES: &[&str] = &[
    "sk-",
    "sk_",
    "rk-",
    "rk_",
    "xai-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "hf_",
    "npm_",
    "xoxa-",
    "xoxb-",
    "xoxo-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
    "AIza",
    "AKIA",
    "ASIA",
];

/// A token with a well-known secret prefix, or a JSON web token: its end.
fn secret_token_at(line: &str, start: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let token_len = bytes[start..]
        .iter()
        .take_while(|byte| is_token_byte(**byte) || matches!(**byte, b'/' | b'+' | b'='))
        .count();
    let token = &bytes[start..start + token_len];
    let prefixed = SECRET_TOKEN_PREFIXES.iter().any(|prefix| {
        token.starts_with(prefix.as_bytes()) && token.len() >= prefix.len() + MIN_TOKEN_BODY_BYTES
    });
    (prefixed || is_json_web_token(token)).then_some(start + token_len)
}

/// `eyJ` then three dot-separated base64url parts, each non-empty.
fn is_json_web_token(token: &[u8]) -> bool {
    if !token.starts_with(b"eyJ") {
        return false;
    }
    let mut parts = token.split(|byte| *byte == b'.');
    let base64url = |part: &[u8]| {
        !part.is_empty()
            && part
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_'))
    };
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(header), Some(payload), Some(signature), None)
            if base64url(header) && base64url(payload) && base64url(signature)
    )
}

/// At `://`, the userinfo of the URL's authority (`user:pass` in
/// `scheme://user:pass@host`): its start and the index of its `@`.
fn url_userinfo_at(line: &str, start: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    if !bytes[start..].starts_with(b"://") {
        return None;
    }
    let authority_start = start + 3;
    let authority_len = bytes[authority_start..]
        .iter()
        .take_while(|byte| !byte.is_ascii_whitespace() && !matches!(**byte, b'/' | b'?' | b'#'))
        .count();
    let authority = &bytes[authority_start..authority_start + authority_len];
    let at = authority.iter().rposition(|byte| *byte == b'@')?;
    (at > 0).then_some((authority_start, authority_start + at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_secret_shapes_keeps_lines_and_plain_text() {
        let text = "first line\nsecond: task-runner network-error risk-free\n\n  indented\n";
        assert_eq!(redact_secret_shapes(text), text);
    }

    #[test]
    fn redact_secret_shapes_covers_each_rule() {
        let secrets = [
            (
                "Authorization: Bearer abcdef0123456789\nnext",
                "abcdef0123456789",
            ),
            (
                "Authorization: Basic dXNlcjpwYXNzd29yZA==",
                "dXNlcjpwYXNzd29yZA==",
            ),
            (
                "proxy-authorization: token zyxwvutsrq987654",
                "zyxwvutsrq987654",
            ),
            ("api_key=abc123secretvalue", "abc123secretvalue"),
            ("DB_PASSWORD=hunter2hunter2", "hunter2hunter2"),
            ("MY_SERVICE_TOKEN: abcdefgh12345678", "abcdefgh12345678"),
            (
                "{\"client_secret\": \"xyz-secret-value\"}",
                "xyz-secret-value",
            ),
            (
                "using sk-ABCDEFGHIJKLMNOPQRSTU now",
                "sk-ABCDEFGHIJKLMNOPQRSTU",
            ),
            (
                "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
                "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            ),
            (
                "gho_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
                "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            ),
            ("glpat-ABCDEFGHIJKLMNOPQRST", "ABCDEFGHIJKLMNOPQRST"),
            ("slack xoxb-1234567890-ABCDEFGHIJ", "1234567890-ABCDEFGHIJ"),
            ("aws AKIAABCDEFGHIJKLMNOP", "AKIAABCDEFGHIJKLMNOP"),
            (
                "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
                "dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
            ),
            ("connect https://user:pa55word@example.com/path", "pa55word"),
        ];
        for (text, secret) in secrets {
            let redacted = redact_secret_shapes(text);
            assert!(!redacted.contains(secret), "{text:?} -> {redacted:?}");
            assert!(
                redacted.contains(SECRET_REDACTION_MARKER),
                "{text:?} -> {redacted:?}"
            );
        }
        assert_eq!(
            redact_secret_shapes("connect https://user:pa55word@example.com/path"),
            "connect https://[REDACTED]@example.com/path"
        );
    }

    #[test]
    fn redact_secret_shapes_removes_pem_private_key_blocks_and_keeps_the_rest() {
        let text = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAKj34GkxFhD90vcNLYLInFEX6Ppy1tPf9Cnzj4p4WGeKLs1Pt8Qu\nKUpRKfFLfRYC9AIKjbJTWit+CqvjWYzvQwECAwEAAQ==\n-----END RSA PRIVATE KEY-----\nafter\n";
        let redacted = redact_secret_shapes(text);
        assert!(
            !redacted.contains("MIIBOgIBAAJBAKj34GkxFhD90vcNLYLInFEX6Ppy1tPf9Cnzj4p4WGeKLs1Pt8Qu")
        );
        assert!(!redacted.contains("KUpRKfFLfRYC9AIKjbJTWit"));
        assert!(redacted.starts_with("before\n"), "{redacted:?}");
        assert!(redacted.ends_with("after\n"), "{redacted:?}");
        assert!(redacted.contains(SECRET_REDACTION_MARKER));
        // An unterminated block fails closed: nothing after its start shows.
        let open = redact_secret_shapes("x\n-----BEGIN PRIVATE KEY-----\nAAAABBBBCCCC\n");
        assert!(!open.contains("AAAABBBBCCCC"), "{open:?}");
        assert!(open.starts_with("x\n"), "{open:?}");
    }

    #[test]
    fn redact_secret_shapes_bounds_a_quoted_value_to_its_line() {
        let redacted = redact_secret_shapes("password=\"abc-unterminated\nthe exception line\n");
        assert!(!redacted.contains("abc-unterminated"), "{redacted:?}");
        assert!(
            redacted.contains("\nthe exception line\n"),
            "a quote never reaches into the next line: {redacted:?}"
        );
    }

    #[test]
    fn redacted_url_keeps_location_and_hides_credentials() {
        for (url, shown) in [
            (
                "https://u:p@h.example/p?q=1",
                r#""https://<redacted>@h.example/p?<redacted>""#,
            ),
            (
                "http://127.0.0.1:8080/mcp",
                r#""http://127.0.0.1:8080/mcp""#,
            ),
            (
                "https://h.example/p#frag",
                r#""https://h.example/p#<redacted>""#,
            ),
            (
                "git@github.com:org/repo.git",
                r#""<redacted>@github.com:org/repo.git""#,
            ),
        ] {
            assert_eq!(format!("{:?}", RedactedUrl(url)), shown);
        }
    }
}
