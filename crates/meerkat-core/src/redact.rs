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
    redact_shapes(text, ShapeRules::Diagnostic)
}

/// Shortest credential value [`redact_credentials`] replaces by exact match.
/// A real credential is far longer; a shorter value is a placeholder
/// (`none`, `dummy`, `EMPTY`) or a common word, and replacing every
/// occurrence of it would mangle ordinary text. It is also the length the
/// shape rules require of a credential after `Bearer` or `Basic`.
pub const MIN_REDACTED_CREDENTIAL_BYTES: usize = 8;

/// Remove credentials from `diagnostic`, text read from a response (a body,
/// a message decoded from one, a log field): first every value
/// `credentials` gives its visitor, by exact match, then credential
/// shapes. Each is replaced by [`SECRET_REDACTION_MARKER`], which shows
/// neither a value's length nor any part of it. Applying it again never
/// shows what the first application removed and adds no text; it can
/// replace more (an identifier left of a replaced value can itself read as
/// a token).
///
/// A value is what was sent: a key, a token, or a whole header value. A
/// header value with a scheme (`Bearer <token>`, `Basic <pair>`) is also
/// replaced without the scheme, so the credential goes whether a response
/// repeats the header or the bare value. A value is matched as sent and as
/// an encoder that wrote the response can spell it: every byte other than
/// a letter, a digit, `-`, `.`, `_` and `~` also as its JSON escape (`\/`,
/// `\"`, `\\`, `\u00XX`) or percent-encoded (`%XX`), hex in either case,
/// alone or mixed. A value shorter than [`MIN_REDACTED_CREDENTIAL_BYTES`]
/// is not replaced by exact match.
///
/// The shape rules are the ones [`redact_secret_shapes`] applies, bounded
/// for text a provider wrote for a reader: after `Bearer` or `Basic`, in an
/// `authorization` header and after a secret-looking key (`api_key=`,
/// `x-api-key:`, `token:`), only a value that looks like a credential is
/// replaced (at least eight credential bytes with a digit among them, or at
/// least 32), and only that value, never the rest of the line. So "an
/// Authorization header using Bearer auth (i.e. Authorization: Bearer
/// YOUR_KEY)" stays as it is. Well-known token prefixes, JSON web tokens,
/// URL userinfo and PEM private-key blocks are replaced as there.
///
/// Two limits. A named value is found in the spellings above and in no
/// other form: not as base64 of it, not as the decoded pair of a `Basic`
/// value, not cut short or masked. And the shape rules are a second line
/// only: a secret no caller names is replaced only in the positions they
/// read, and one made of letters only and shorter than 32 bytes is not.
///
/// This is the one owner of response redaction:
/// [`crate::HttpAuthorizer::redact_response_diagnostic`] calls it with the
/// credentials the authorizer signed with, and a provider client that signs
/// requests with a key of its own calls it with that key.
pub fn redact_credentials(diagnostic: &mut String, credentials: impl FnOnce(&mut dyn FnMut(&str))) {
    credentials(&mut |credential| redact_credential_value(diagnostic, credential));
    let shaped = redact_shapes(diagnostic, ShapeRules::ResponseText);
    if shaped != *diagnostic {
        *diagnostic = shaped;
    }
}

/// Replace `credential` in `text` as sent and, when it is a header value
/// with a scheme, without the scheme.
fn redact_credential_value(text: &mut String, credential: &str) {
    let credential = credential.trim();
    replace_credential(text, credential);
    if let Some((scheme, value)) = credential.split_once(char::is_whitespace)
        && !scheme.is_empty()
        && scheme.bytes().all(is_credential_byte)
    {
        replace_credential(text, value.trim_start());
    }
}

fn replace_credential(text: &mut String, credential: &str) {
    if credential.len() < MIN_REDACTED_CREDENTIAL_BYTES {
        return;
    }
    if text.contains(credential) {
        *text = text.replace(credential, SECRET_REDACTION_MARKER);
    }
    // A value of plain bytes has one spelling; any other can come back as
    // an encoder wrote it.
    if credential.bytes().all(is_plain_byte) {
        return;
    }
    if let Some(replaced) = replace_spelled(text, credential.as_bytes()) {
        *text = replaced;
    }
}

/// Bytes no encoder rewrites: letters, digits, `-`, `.`, `_` and `~`.
fn is_plain_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// `text` with `credential` replaced wherever it stands in an encoder's
/// spelling (see [`spelling_len`]); `None` when it stands nowhere.
fn replace_spelled(text: &str, credential: &[u8]) -> Option<String> {
    let bytes = text.as_bytes();
    let mut output: Option<String> = None;
    let mut copied = 0;
    let mut at = 0;
    while at < bytes.len() {
        // A spelling starts and ends at a character boundary: it begins
        // with the value's first byte or an ASCII escape, and repeats a
        // character of the value whole.
        match spelled_end(bytes, at, credential) {
            Some(end) => {
                let output = output.get_or_insert_with(|| String::with_capacity(text.len()));
                output.push_str(&text[copied..at]);
                output.push_str(SECRET_REDACTION_MARKER);
                copied = end;
                at = end;
            }
            None => at += 1,
        }
    }
    let mut output = output?;
    output.push_str(&text[copied..]);
    Some(output)
}

/// The end of `credential` at `at`, when `text` holds it there with each
/// byte in one of its spellings.
fn spelled_end(text: &[u8], at: usize, credential: &[u8]) -> Option<usize> {
    credential.iter().try_fold(at, |at, byte| {
        Some(at + spelling_len(text.get(at..)?, *byte)?)
    })
}

/// The length of the spelling of `byte` that `rest` starts with. A plain
/// byte has one spelling, itself. Any other ASCII byte can also stand as
/// its JSON escape (`\/`, `\"`, `\\`, or `\u00XX`) or percent-encoded
/// (`%XX`), hex in either case; the escaped form is read first.
fn spelling_len(rest: &[u8], byte: u8) -> Option<usize> {
    if byte.is_ascii() && !is_plain_byte(byte) {
        match rest {
            [b'\\', escaped, ..] if *escaped == byte && matches!(byte, b'/' | b'"' | b'\\') => {
                return Some(2);
            }
            [b'\\', b'u', b'0', b'0', high, low, ..] if hex_byte(*high, *low) == Some(byte) => {
                return Some(6);
            }
            [b'%', high, low, ..] if hex_byte(*high, *low) == Some(byte) => return Some(3),
            _ => {}
        }
    }
    (rest.first() == Some(&byte)).then_some(1)
}

/// The byte two hex digits spell.
fn hex_byte(high: u8, low: u8) -> Option<u8> {
    let digit = |byte: u8| char::from(byte).to_digit(16);
    u8::try_from(digit(high)? * 16 + digit(low)?).ok()
}

/// Which text the shape rules read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShapeRules {
    /// Process diagnostics (a server's stderr): every value in a secret
    /// position goes, an authorization header's to the end of its line.
    Diagnostic,
    /// Text a provider wrote for a reader: in a secret position only a
    /// value that looks like a credential goes, and only that value.
    ResponseText,
}

fn redact_shapes(text: &str, rules: ShapeRules) -> String {
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
        redact_line(content, &mut output, rules);
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
fn redact_line(line: &str, output: &mut String, rules: ShapeRules) {
    let bytes = line.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if at_left_boundary(bytes, cursor) {
            if let Some(value_start) = authorization_header_at(line, cursor) {
                match rules {
                    // An authorization header: its whole value (scheme and
                    // credentials) to the end of the line.
                    ShapeRules::Diagnostic => {
                        output.push_str(&line[cursor..value_start]);
                        output.push_str(SECRET_REDACTION_MARKER);
                        return;
                    }
                    // Its credential, when the value holds one.
                    ShapeRules::ResponseText => {
                        if let Some((start, end)) = header_credential_at(line, value_start) {
                            output.push_str(&line[cursor..start]);
                            output.push_str(SECRET_REDACTION_MARKER);
                            cursor = end;
                            continue;
                        }
                    }
                }
            }
            if let Some(value_start) = credential_scheme_at(line, cursor) {
                match rules {
                    ShapeRules::Diagnostic => {
                        output.push_str(&line[cursor..value_start]);
                        cursor =
                            crate::panic_payload::push_redacted_value(output, line, value_start);
                        continue;
                    }
                    ShapeRules::ResponseText => {
                        if let Some(end) = credential_like_end(line, value_start) {
                            output.push_str(&line[cursor..value_start]);
                            output.push_str(SECRET_REDACTION_MARKER);
                            cursor = end;
                            continue;
                        }
                    }
                }
            }
            if let Some(key_end) = secret_key_at(line, cursor)
                && let Some(value_start) =
                    crate::panic_payload::assignment_value_start(line, key_end)
            {
                match rules {
                    ShapeRules::Diagnostic => {
                        output.push_str(&line[cursor..value_start]);
                        cursor =
                            crate::panic_payload::push_redacted_value(output, line, value_start);
                        continue;
                    }
                    // The value, quoted or not, when it looks like a
                    // credential: "token: expected a comma" is a sentence.
                    ShapeRules::ResponseText => {
                        let content_start =
                            value_start + usize::from(matches!(bytes[value_start], b'"' | b'\''));
                        if let Some(end) = credential_like_end(line, content_start) {
                            output.push_str(&line[cursor..content_start]);
                            output.push_str(SECRET_REDACTION_MARKER);
                            cursor = end;
                            continue;
                        }
                    }
                }
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
        .take_while(|byte| is_credential_byte(**byte))
        .count();
    (credential >= MIN_REDACTED_CREDENTIAL_BYTES).then_some(value_start)
}

/// Bytes a bearer token, a Basic pair or an API key is made of.
fn is_credential_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~+/=".contains(&byte)
}

/// A run of credential bytes at `start` that looks like a credential, not a
/// word: at least [`MIN_REDACTED_CREDENTIAL_BYTES`] with a digit among them,
/// or at least 32. Its end.
fn credential_like_end(line: &str, start: usize) -> Option<usize> {
    let run = line.as_bytes().get(start..)?;
    let len = run
        .iter()
        .take_while(|byte| is_credential_byte(**byte))
        .count();
    let looks_like_a_credential = len >= 32
        || (len >= MIN_REDACTED_CREDENTIAL_BYTES && run[..len].iter().any(u8::is_ascii_digit));
    looks_like_a_credential.then_some(start + len)
}

/// The credential in an authorization header's value at `value_start`: the
/// value itself, or what follows its scheme word (`Bearer`, `Basic`,
/// `Token`), when that looks like a credential. Its start and end.
fn header_credential_at(line: &str, value_start: usize) -> Option<(usize, usize)> {
    if let Some(end) = credential_like_end(line, value_start) {
        return Some((value_start, end));
    }
    let bytes = line.as_bytes();
    let scheme = bytes[value_start..]
        .iter()
        .take_while(|byte| is_credential_byte(**byte))
        .count();
    let mut start = value_start + scheme;
    if scheme == 0 || !bytes.get(start).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    while bytes.get(start).is_some_and(u8::is_ascii_whitespace) {
        start += 1;
    }
    credential_like_end(line, start).map(|end| (start, end))
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
///
/// Every such token starts with a known prefix, so a run of token bytes is
/// read only where one stands, and then only once. `/`, `+` and `=` belong
/// to a run and are also boundaries inside it: reading a run from each of
/// them made one long line (a megabyte of base64, or a line that repeats a
/// prefix after each of them) cost its length squared.
fn secret_token_at(line: &str, start: usize) -> Option<usize> {
    let rest = &line.as_bytes()[start..];
    if !SECRET_TOKEN_PREFIXES
        .iter()
        .any(|prefix| rest.starts_with(prefix.as_bytes()))
    {
        return json_web_token_len(rest).map(|len| start + len);
    }
    // The whole run. A run long enough for a second reading to matter is a
    // token, and the scan continues after it.
    let token_len = rest
        .iter()
        .take_while(|byte| is_token_byte(**byte) || matches!(**byte, b'/' | b'+' | b'='))
        .count();
    #[cfg(test)]
    tests::note_measured_token_run(token_len);
    let token = &rest[..token_len];
    SECRET_TOKEN_PREFIXES
        .iter()
        .any(|prefix| {
            token.starts_with(prefix.as_bytes())
                && token.len() >= prefix.len() + MIN_TOKEN_BODY_BYTES
        })
        .then_some(start + token_len)
}

/// What the header of a JSON web token starts with (`{"` in base64url).
const JSON_WEB_TOKEN_PREFIX: &[u8] = b"eyJ";

/// The length of the run of token bytes `rest` starts with, when that run
/// is a JSON web token. `/`, `+` and `=` continue a run and cannot be in a
/// web token, so the run is given up at the first of them and no part of it
/// is read again from a later boundary.
fn json_web_token_len(rest: &[u8]) -> Option<usize> {
    if !rest.starts_with(JSON_WEB_TOKEN_PREFIX) {
        return None;
    }
    let mut len = 0;
    for byte in rest {
        match byte {
            b'/' | b'+' | b'=' => return None,
            byte if is_token_byte(*byte) => len += 1,
            _ => break,
        }
    }
    #[cfg(test)]
    tests::note_measured_token_run(len);
    is_json_web_token(&rest[..len]).then_some(len)
}

/// `eyJ` then three dot-separated base64url parts, each non-empty.
fn is_json_web_token(token: &[u8]) -> bool {
    if !token.starts_with(JSON_WEB_TOKEN_PREFIX) {
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
/// `scheme://user:pass@host`): its start and the index of its `@`. The
/// authority ends at whitespace, at `/`, `?` or `#`, and where the text
/// that holds the URL closes it: a quote, a backslash, `<` or `>`. A URL
/// without a path inside a JSON string therefore never reaches a later `@`
/// of the body.
fn url_userinfo_at(line: &str, start: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    if !bytes[start..].starts_with(b"://") {
        return None;
    }
    let authority_start = start + 3;
    let authority_len = bytes[authority_start..]
        .iter()
        .take_while(|byte| {
            !byte.is_ascii_whitespace()
                && !matches!(
                    **byte,
                    b'/' | b'?' | b'#' | b'"' | b'\'' | b'\\' | b'<' | b'>'
                )
        })
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

    /// The credentials a caller passes are replaced by exact value: as
    /// sent, without their scheme, in a header line, alone and in a URL.
    #[test]
    fn redact_credentials_replaces_each_value_as_sent_and_bare() {
        let token = format!("madeup{}", "bearertoken".repeat(3));
        let pair = format!("{}==", "bWFkZXVwOnBhaXI".repeat(2));
        let key = format!("madeup{}", "apikey".repeat(4));
        let values = [
            format!("Bearer {token}"),
            format!("Basic {pair}"),
            key.clone(),
        ];
        for text in [
            format!("rejected request with header authorization: Bearer {token}"),
            format!("rejected request with header proxy-authorization: Basic {pair}"),
            format!("rejected request with header x-api-key: {key}"),
            format!("credential {token} was refused"),
            format!("pair {pair} was refused"),
            format!("GET http://upstream.invalid/v1/models?session={key}&page=2 failed"),
            format!("{token}{key}"),
        ] {
            let mut redacted = text.clone();
            redact_credentials(&mut redacted, |visit| {
                for value in &values {
                    visit(value);
                }
            });
            for secret in [&token, &pair, &key] {
                assert!(
                    !redacted.contains(secret.as_str()),
                    "{text:?} -> {redacted:?}"
                );
            }
            assert!(redacted.contains(SECRET_REDACTION_MARKER), "{redacted:?}");
        }
        let mut redacted =
            format!("GET http://upstream.invalid/v1/models?session={key}&page=2 failed");
        redact_credentials(&mut redacted, |visit| visit(&key));
        assert_eq!(
            redacted,
            "GET http://upstream.invalid/v1/models?session=[REDACTED]&page=2 failed"
        );
    }

    /// The marker is one fixed text: two credentials of different lengths
    /// and prefixes leave the same output.
    #[test]
    fn redact_credentials_marker_shows_no_length_or_prefix() {
        let redact = |credential: &str| {
            let mut text = format!("credential {credential} was refused");
            redact_credentials(&mut text, |visit| visit(credential));
            text
        };
        assert_eq!(
            redact("madeup-short-1"),
            "credential [REDACTED] was refused"
        );
        assert_eq!(
            redact(&"another-much-longer-madeup-value".repeat(4)),
            "credential [REDACTED] was refused"
        );
    }

    /// A value shorter than the minimum is not replaced by exact match: a
    /// placeholder key equal to an ordinary word would otherwise mangle
    /// every text that uses the word. At the minimum it is replaced.
    #[test]
    fn redact_credentials_leaves_values_below_the_minimum() {
        let text = "model none is not served; dummy runs are free; no key";
        for short in ["none", "dummy", "key", "EMPTY", "1234567"] {
            assert!(short.len() < MIN_REDACTED_CREDENTIAL_BYTES);
            let mut redacted = text.to_string();
            redact_credentials(&mut redacted, |visit| visit(short));
            assert_eq!(redacted, text, "{short}");
        }
        let mut redacted = "the value 12345678 was refused".to_string();
        redact_credentials(&mut redacted, |visit| visit("12345678"));
        assert_eq!(redacted, "the value [REDACTED] was refused");
    }

    /// With no credential named, the shape rules still remove a
    /// credential a response repeats in a known position, and only it.
    #[test]
    fn redact_credentials_removes_credential_shapes_and_nothing_else() {
        // (The made-up header value without a scheme is assembled here so no
        // scanner reads the source as a secret.)
        let bare_header = ["authorization: abcdef", "0123456789 (no scheme)"].concat();
        for (text, expected) in [
            (
                r#"{"error":{"message":"rejected request with header authorization: Bearer abcdef0123456789","type":"invalid_request_error"}}"#,
                r#"{"error":{"message":"rejected request with header authorization: Bearer [REDACTED]","type":"invalid_request_error"}}"#,
            ),
            (
                "Authorization: Basic dXNlcjpwYXNzd29yZDEyMw== was refused",
                "Authorization: Basic [REDACTED] was refused",
            ),
            (
                "proxy-authorization: Token zyxwvutsrq987654, then retried",
                "proxy-authorization: Token [REDACTED], then retried",
            ),
            (
                bare_header.as_str(),
                "authorization: [REDACTED] (no scheme)",
            ),
            (
                "header x-api-key: abc123secretvalue; next",
                "header x-api-key: [REDACTED]; next",
            ),
            (
                r#"{"api_key":"abc123secretvalue","model":"m"}"#,
                r#"{"api_key":"[REDACTED]","model":"m"}"#,
            ),
            (
                "GET /v1/models?key=AbCdEfGh12345678&alt=sse",
                "GET /v1/models?key=[REDACTED]&alt=sse",
            ),
            ("used sk-ABCDEFGHIJKLMNOPQRSTU here", "used [REDACTED] here"),
            (
                "connect https://user:pa55word@example.com/path",
                "connect https://[REDACTED]@example.com/path",
            ),
        ] {
            let mut redacted = text.to_string();
            redact_credentials(&mut redacted, |_| {});
            assert_eq!(redacted, expected, "{text:?}");
        }
    }

    /// Ordinary provider errors that only talk about credentials (a
    /// message, a type, a code, a request id, a URL) are returned byte for
    /// byte.
    #[test]
    fn redact_credentials_leaves_ordinary_provider_errors_unchanged() {
        for text in [
            r#"{"error":{"message":"You didn't provide an API key. You need to provide your API key in an Authorization header using Bearer auth (i.e. Authorization: Bearer YOUR_KEY), or as the password field (with blank username) if you're accessing the API from your browser. You can obtain an API key from https://docs.upstream.invalid/account/api-keys.","type":"invalid_request_error","param":null,"code":null}}"#,
            r#"{"error":{"message":"Incorrect API key provided: sk-proj-********************abcd. You can find your API key at https://docs.upstream.invalid/account/api-keys.","type":"invalid_request_error","param":null,"code":"invalid_api_key"},"request_id":"req_0123456789abcdef"}"#,
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"},"request_id":"req_011CSHoEeqs5C35K2UUqR7Fy"}"#,
            "{\n  \"error\": {\n    \"code\": 400,\n    \"message\": \"API key not valid. Please pass a valid API key.\",\n    \"status\": \"INVALID_ARGUMENT\",\n    \"details\": [\n      {\n        \"@type\": \"type.googleapis.com/google.rpc.ErrorInfo\",\n        \"reason\": \"API_KEY_INVALID\"\n      }\n    ]\n  }\n}\n",
            r#"{"error":{"message":"This model's maximum context length is 128000 tokens. However, your messages resulted in 130112 tokens.","type":"invalid_request_error","param":"messages","code":"context_length_exceeded"}}"#,
            r#"{"error":{"message":"Rate limit reached for model in organization org-0123456789 on tokens per min (TPM): Limit 30000, Used 29000, Requested 2000.","type":"tokens","param":null,"code":"rate_limit_exceeded"}}"#,
            "Missing bearer authentication in header; basic authentication is not supported. See https://docs.upstream.invalid/v1/auth?section=2#tokens",
            "Unexpected token: expected a comma. The security token included in the request is invalid. password: must be longer.",
            "Access denied due to invalid subscription key or wrong API endpoint. Make sure to provide a valid key for an active subscription.",
        ] {
            let mut redacted = text.to_string();
            redact_credentials(&mut redacted, |visit| {
                visit("a-credential-that-does-not-occur-0123");
            });
            assert_eq!(redacted, text);
        }
    }

    /// The texts a client redacts twice (an error where it is built, then
    /// again where its stream ends) are unchanged by the second
    /// application. That holds for these, not for every text: see
    /// `redact_credentials_applied_again_never_reveals`.
    #[test]
    fn redact_credentials_applied_again_leaves_a_redacted_error_unchanged() {
        let credential = format!("madeup{}", "bearertoken".repeat(3));
        for text in [
            format!(r#"{{"error":{{"message":"header authorization: Bearer {credential}"}}}}"#),
            format!("api_key={credential} token: {credential}, key=[{credential}]"),
            "authorization: Bearer abcdef0123456789 and x-api-key: abc123secretvalue".to_string(),
            r#"{"api_key":"abc123secretvalue"} https://user:pa55word@example.com/"#.to_string(),
        ] {
            let mut once = text;
            redact_credentials(&mut once, |visit| visit(&credential));
            assert!(!once.contains(&credential), "{once:?}");
            let mut twice = once.clone();
            redact_credentials(&mut twice, |visit| visit(&credential));
            assert_eq!(twice, once);
        }
    }

    /// The diagnostic rules are unchanged by the response-text rules: an
    /// authorization header still loses its whole value, and a secret-key
    /// assignment its value whatever it looks like.
    #[test]
    fn redact_secret_shapes_keeps_the_diagnostic_rules() {
        assert_eq!(
            redact_secret_shapes("Authorization: Bearer YOUR_KEY) and more"),
            "Authorization: [REDACTED]"
        );
        assert_eq!(redact_secret_shapes("token: expected"), "token: [REDACTED]");
    }

    thread_local! {
        /// The bytes of token runs the shape step measured on this thread,
        /// and the most a row allows it to measure.
        static MEASURED_TOKEN_RUNS: std::cell::Cell<(usize, usize)> =
            const { std::cell::Cell::new((0, usize::MAX)) };
    }

    /// Count a token run the shape step measured, and fail the row that set
    /// a budget once the count passes it (so a scan that is quadratic in the
    /// line's length fails at once instead of running for minutes).
    pub(super) fn note_measured_token_run(len: usize) {
        MEASURED_TOKEN_RUNS.with(|measured| {
            let (total, budget) = measured.get();
            let total = total.saturating_add(len);
            measured.set((total, budget));
            assert!(
                total <= budget,
                "the shape step measured {total} bytes of token runs, over the budget of {budget}"
            );
        });
    }

    /// One line of `len` bytes drawn from the base64 alphabet by a fixed
    /// generator: about 2 bytes in 64 are `+` or `/`.
    fn base64_like_line(len: usize) -> String {
        let alphabet: Vec<u8> = (b'A'..=b'Z')
            .chain(b'a'..=b'z')
            .chain(b'0'..=b'9')
            .chain(*b"+/")
            .collect();
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                char::from(alphabet[(state >> 33) as usize % alphabet.len()])
            })
            .collect()
    }

    /// The shape step reads one long line in time proportional to its
    /// length: the token runs it measures add up to a small multiple of the
    /// line, whatever the line holds, a line that repeats a token prefix at
    /// every boundary of one run included. A rejection body is read whole,
    /// and a request with an inline image is a megabyte of base64 on one
    /// line.
    #[test]
    fn redact_credentials_reads_a_long_line_in_linear_time() {
        const LEN: usize = 512 * 1024;
        for (name, line) in [
            ("a/ repeated", "a/".repeat(LEN / 2)),
            ("base64-like text", base64_like_line(LEN)),
            ("a web token prefix repeated", "eyJ/".repeat(LEN / 4)),
        ] {
            let budget = 4 * line.len();
            MEASURED_TOKEN_RUNS.with(|measured| measured.set((0, budget)));
            let mut redacted = line.clone();
            redact_credentials(&mut redacted, |_| {});
            let (measured, _) =
                MEASURED_TOKEN_RUNS.with(|measured| measured.replace((0, usize::MAX)));
            assert!(
                measured <= budget,
                "{name}: measured {measured} bytes of token runs in a {LEN}-byte line"
            );
            assert!(redacted == line, "{name}: a line without a credential");
        }
    }

    /// A named credential is removed when the response repeats it as an
    /// encoder wrote it: `/` as `\/`, a byte as its JSON unicode escape
    /// (hex in either case) or percent-encoded, alone or mixed, wherever it
    /// stands. The text around it is kept.
    #[test]
    fn redact_credentials_replaces_a_value_an_encoder_rewrote() {
        // (Assembled here so no scanner reads the source as a secret.)
        let credential = ["madeUP0", "/slash", "+plusTail99", "=="].concat();
        let rewrite = |pairs: &[(&str, &str)]| {
            pairs.iter().fold(credential.clone(), |value, (from, to)| {
                value.replace(from, to)
            })
        };
        let forms = [
            ("as sent", credential.clone()),
            ("slash as backslash-slash", rewrite(&[("/", "\\/")])),
            ("plus as a unicode escape", rewrite(&[("+", "\\u002B")])),
            ("equals as a unicode escape", rewrite(&[("=", "\\u003d")])),
            ("slash as a unicode escape", rewrite(&[("/", "\\u002f")])),
            (
                "percent-encoded",
                rewrite(&[("/", "%2F"), ("+", "%2B"), ("=", "%3D")]),
            ),
            (
                "percent-encoded, lower-case hex",
                rewrite(&[("/", "%2f"), ("+", "%2b"), ("=", "%3d")]),
            ),
            (
                "mixed",
                rewrite(&[("/", "\\/"), ("+", "\\u002b"), ("=", "%3D")]),
            ),
        ];
        let mut left = Vec::new();
        for (form, echoed) in &forms {
            for (position, text) in [
                ("bare", format!("credential {echoed} was refused")),
                (
                    "header",
                    format!(r#"{{"message":"header authorization: Bearer {echoed}","code":400}}"#),
                ),
                (
                    "URL query",
                    format!("GET http://upstream.invalid/v1/models?session={echoed}&page=2 failed"),
                ),
            ] {
                for scheme in ["", "Bearer "] {
                    // The value named with its scheme goes with the scheme
                    // where the response repeats both.
                    let with_scheme = format!("{scheme}{echoed}");
                    let expected = if text.contains(&with_scheme) {
                        text.replace(&with_scheme, SECRET_REDACTION_MARKER)
                    } else {
                        text.replace(echoed.as_str(), SECRET_REDACTION_MARKER)
                    };
                    let named = format!("{scheme}{credential}");
                    let mut redacted = text.clone();
                    redact_credentials(&mut redacted, |visit| visit(&named));
                    if redacted != expected {
                        left.push(format!("{form}, {position}, named {scheme:?}: {redacted}"));
                    }
                }
            }
        }
        assert!(
            left.is_empty(),
            "an echo kept part of the credential:\n{}",
            left.join("\n")
        );
        // A value no encoder rewrites has one spelling: text that only
        // resembles an escape of it is left alone.
        let plain = format!("madeup-{}", "plainvalue".repeat(2));
        let mut redacted = format!("id {} and {plain}", plain.replace('-', "%2D"));
        redact_credentials(&mut redacted, |visit| visit(&plain));
        assert_eq!(
            redacted,
            format!("id {} and [REDACTED]", plain.replace('-', "%2D"))
        );
    }

    /// A URL without a path ends where its quoting ends: a JSON body that
    /// holds a URL and, later, an address with an `@` stays as it was, and
    /// stays JSON. Userinfo inside a quoted URL is still replaced.
    #[test]
    fn redact_credentials_keeps_text_between_a_bare_host_and_a_later_at_sign() {
        for text in [
            r#"{"docs":"https://platform.upstream.invalid","support":"help@upstream.invalid"}"#,
            r#"{\"docs\":\"https://platform.upstream.invalid\",\"support\":\"help@upstream.invalid\"}"#,
            "see 'https://platform.upstream.invalid','help@upstream.invalid'",
            "see <https://platform.upstream.invalid>,<help@upstream.invalid>",
        ] {
            let mut redacted = text.to_string();
            redact_credentials(&mut redacted, |_| {});
            assert_eq!(redacted, text);
        }
        let mut body =
            r#"{"docs":"https://platform.upstream.invalid","support":"help@upstream.invalid"}"#
                .to_string();
        redact_credentials(&mut body, |_| {});
        assert!(
            body.starts_with('{') && body.ends_with('}') && body.matches('"').count() == 8,
            "the body is still the JSON object the server sent: {body}"
        );
        let mut quoted = r#"{"proxy":"https://user:pa55word@example.com","next":1}"#.to_string();
        redact_credentials(&mut quoted, |_| {});
        assert_eq!(
            quoted,
            r#"{"proxy":"https://[REDACTED]@example.com","next":1}"#
        );
    }

    /// A password or a user name that holds a quoting byte (`'`, `"`, `\`,
    /// `<`, `>`) is still userinfo: both rule sets replace it. The
    /// diagnostic rules (a process's stderr) keep the authority they have
    /// always read, to whitespace, `/`, `?` or `#`. The response-text rules
    /// end it at a quoting byte only where that byte closes a bare host
    /// and another one follows before the `@`.
    #[test]
    fn a_quoting_byte_in_url_userinfo_is_still_userinfo() {
        let replaced = "connect postgres://[REDACTED]@db.internal/app failed";
        let mut userinfos: Vec<String> = ["'", "\"", "\\", "<", ">"]
            .iter()
            .map(|quoting| format!("app:pa{quoting}ss99word"))
            .collect();
        userinfos.extend(
            [
                // Two quoting bytes after a `:` that is no port.
                "app:pa'ss\"99word",
                // A quoting byte in the user name.
                "o'brien:pa55word",
                // A password that starts with digits, as a port would.
                "app:1234'rest",
            ]
            .map(String::from),
        );
        for userinfo in userinfos {
            let text = format!("connect postgres://{userinfo}@db.internal/app failed");
            let mut redacted = text.clone();
            redact_credentials(&mut redacted, |_| {});
            assert_eq!(redacted, replaced, "response text: {text}");
            assert_eq!(redact_secret_shapes(&text), replaced, "diagnostic: {text}");
        }
        // The diagnostic rules are the released ones: a quoting byte ends
        // no authority there.
        assert_eq!(
            redact_secret_shapes(
                r#"{"docs":"https://platform.upstream.invalid","support":"help@upstream.invalid"}"#
            ),
            r#"{"docs":"https://[REDACTED]@upstream.invalid"}"#
        );
        // The declared limit of the response-text rules: userinfo whose
        // start reads as a bare host (no `:`, or `:` and digits only) and
        // that holds two quoting bytes is read as a quoted host. The
        // diagnostic rules replace it.
        for userinfo in ["app:1234're'st", "o'bri'en:pa55word"] {
            let text = format!("connect postgres://{userinfo}@db.internal/app failed");
            let mut redacted = text.clone();
            redact_credentials(&mut redacted, |_| {});
            assert_eq!(redacted, text, "response text: {text}");
            assert_eq!(redact_secret_shapes(&text), replaced, "diagnostic: {text}");
        }
    }

    /// What a second application shows, the first one showed: it never
    /// brings a value back, and it adds no text. It can replace more (an
    /// identifier left of a replaced value can itself look like a token).
    #[test]
    fn redact_credentials_applied_again_never_reveals() {
        // What `text` shows: its parts between markers, kept apart.
        fn shown(text: &str) -> String {
            text.split(SECRET_REDACTION_MARKER)
                .collect::<Vec<_>>()
                .join("\u{1}")
        }
        // Every part `twice` shows is, in order, inside a part `once` shows.
        fn reveals_nothing(once: &str, twice: &str) -> bool {
            let once = shown(once);
            let mut cursor = 0;
            twice.split(SECRET_REDACTION_MARKER).all(|part| {
                once[cursor..].find(part).is_some_and(|found| {
                    cursor += found + part.len();
                    true
                })
            })
        }
        // (The key block markers are assembled here so no scanner reads the
        // source as a secret.)
        let begin = format!("-----BEGIN {}-----", "PRIVATE KEY");
        let end = format!("-----END {}-----", "PRIVATE KEY");
        let named = format!("madeup{}", "namedvalue".repeat(2));
        let atoms = [
            "authorization",
            "proxy-authorization",
            "Bearer",
            "Basic",
            "Token",
            "token",
            "api_key",
            "x-api-key",
            "password",
            "secret",
            "key",
            "sk_",
            "sk-",
            "ghp_",
            "AKIA",
            "eyJ",
            "live",
            "abc12345678",
            "abcdefghijklmnopqrstuvwxyzabcdefgh",
            "0123456789",
            "user",
            "host.invalid",
            "https://",
            "@",
            ":",
            "=",
            " ",
            ".",
            "/",
            "+",
            "_",
            "-",
            "\"",
            "'",
            ",",
            "\n",
            "é",
            "[REDACTED]",
            begin.as_str(),
            end.as_str(),
            named.as_str(),
        ];
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |bound: usize| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize % bound
        };
        let mut changed = 0;
        for _ in 0..40_000 {
            let text: String = (0..=next(12)).map(|_| atoms[next(atoms.len())]).collect();
            let mut once = text.clone();
            redact_credentials(&mut once, |visit| visit(&named));
            assert!(!once.contains(&named), "{text:?} -> {once:?}");
            let mut twice = once.clone();
            redact_credentials(&mut twice, |visit| visit(&named));
            assert!(
                reveals_nothing(&once, &twice),
                "{text:?} -> {once:?} -> {twice:?}"
            );
            changed += usize::from(twice != once);
        }
        // The second application is not a no-op for every text. The
        // smallest form: the value goes first and its identifier stays;
        // once the value is the marker, the identifier (with its `=`) reads
        // as a token.
        assert!(changed > 0, "no generated text changed a second time");
        let mut once = ["sk_", "live_key=abc12345678"].concat();
        redact_credentials(&mut once, |_| {});
        assert_eq!(once, ["sk_", "live_key=[REDACTED]"].concat());
        let mut twice = once.clone();
        redact_credentials(&mut twice, |_| {});
        assert_eq!(twice, "[REDACTED][REDACTED]");
    }

    /// A value of letters only is a credential from 32 bytes up; below
    /// that it reads as a word and stays.
    #[test]
    fn redact_credentials_replaces_a_letters_only_value_from_32_bytes_up() {
        let letters = "abcdefgh".repeat(4);
        for (len, replaced) in [(20, false), (31, false), (32, true)] {
            let value = &letters[..len];
            for (text, expected) in [
                (
                    format!("header authorization: Bearer {value} was sent"),
                    "header authorization: Bearer [REDACTED] was sent",
                ),
                (
                    format!("retry with Bearer {value} failed"),
                    "retry with Bearer [REDACTED] failed",
                ),
                (
                    format!("query api_key={value}&page=2"),
                    "query api_key=[REDACTED]&page=2",
                ),
            ] {
                let mut redacted = text.clone();
                redact_credentials(&mut redacted, |_| {});
                if replaced {
                    assert_eq!(redacted, expected, "{len} letters");
                } else {
                    assert_eq!(redacted, text, "{len} letters");
                }
            }
        }
    }

    /// A Basic pair is replaced after its scheme wherever the scheme
    /// stands, not only in an `authorization` header; "basic" as a word
    /// stays.
    #[test]
    fn redact_credentials_replaces_a_basic_pair_outside_a_header() {
        // (Assembled here so no scanner reads the source as a secret.)
        let pair = ["dXNlcjpwYXNz", "d29yZDEyMw=="].concat();
        let mut redacted = format!("the proxy refused Basic {pair} for this route");
        redact_credentials(&mut redacted, |_| {});
        assert_eq!(
            redacted,
            "the proxy refused Basic [REDACTED] for this route"
        );
        let text = "the proxy accepts basic authentication only";
        let mut redacted = text.to_string();
        redact_credentials(&mut redacted, |_| {});
        assert_eq!(redacted, text);
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
