//! The value encoding of the canonical rendering.
//!
//! Every value the rendering shows is escaped reversibly, so that it can
//! never break the rendering's structure, look like another field, or hide
//! text in a presenter that interprets markup:
//!
//! - a backslash is written `\\`;
//! - a markup character (`` ` `` `*` `_` `~` `|` `<` `>` `[` `]` `&`) is
//!   written with a backslash before it, so no link, image, spoiler, tag,
//!   code span or emphasis can form;
//! - a character that is invisible, reorders text or breaks a line (control
//!   characters, Unicode format characters such as bidirectional and
//!   zero-width ones, variation selectors, line and paragraph separators,
//!   spaces other than U+0020, invisible fillers, private-use and
//!   noncharacters) is written `\u{XXXX}` with at least four uppercase hex
//!   digits;
//! - in a quoted value, `"` is written `\"`.
//!
//! Decoding accepts exactly the encoder's output: a raw character the
//! encoder would have escaped, or an escape it would not have written, is
//! refused.

/// Whether `character` can form markup in a presenter that interprets it.
pub(crate) fn is_markup(character: char) -> bool {
    matches!(
        character,
        '`' | '*' | '_' | '~' | '|' | '<' | '>' | '[' | ']' | '&'
    )
}

/// Whether `character` is invisible, reorders text, breaks a line, or
/// renders as something its code does not say.
pub(crate) fn is_hidden(character: char) -> bool {
    let code = u32::from(character);
    character.is_control()
        || (code & 0xFFFE) == 0xFFFE
        || matches!(
            code,
            0x00A0
                | 0x00AD
                | 0x034F
                | 0x0600..=0x0605
                | 0x061C
                | 0x06DD
                | 0x070F
                | 0x0890..=0x0891
                | 0x08E2
                | 0x115F..=0x1160
                | 0x1680
                | 0x17B4..=0x17B5
                | 0x180B..=0x180F
                | 0x2000..=0x200F
                | 0x2028..=0x202F
                | 0x205F..=0x206F
                | 0x2800
                | 0x3000
                | 0x3164
                | 0xE000..=0xF8FF
                | 0xFDD0..=0xFDEF
                | 0xFE00..=0xFE0F
                | 0xFEFF
                | 0xFFA0
                | 0xFFF9..=0xFFFB
                | 0x110BD
                | 0x110CD
                | 0x13430..=0x1343F
                | 0x1BCA0..=0x1BCA3
                | 0x1D173..=0x1D17A
                | 0xE0000..=0xE0FFF
                | 0xF0000..=0x10FFFF
        )
}

fn escape_into(out: &mut String, text: &str, quoted: bool) {
    for character in text.chars() {
        if character == '\\' || (quoted && character == '"') || is_markup(character) {
            out.push('\\');
            out.push(character);
        } else if is_hidden(character) {
            out.push_str(&format!("\\u{{{:04X}}}", u32::from(character)));
        } else {
            out.push(character);
        }
    }
}

/// `text` as a quoted value: `"`, the escaped text, `"`.
pub(crate) fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    escape_into(&mut out, text, true);
    out.push('"');
    out
}

/// One line of free text, escaped. The caller splits the text at `\n`.
pub(crate) fn escaped_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    escape_into(&mut out, text, false);
    out
}

/// Decode one escape after its backslash, from `rest`. Returns the
/// character and what follows it, or `None` for an escape the encoder never
/// writes.
fn decode_escape(rest: &str, quoted: bool) -> Option<(char, &str)> {
    let mut chars = rest.chars();
    let next = chars.next()?;
    if next == '\\' || (quoted && next == '"') || is_markup(next) {
        return Some((next, chars.as_str()));
    }
    if next != 'u' {
        return None;
    }
    let body = chars.as_str().strip_prefix('{')?;
    let close = body.find('}')?;
    let hex = body.get(..close)?;
    if hex.len() < 4
        || hex.len() > 6
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
    {
        return None;
    }
    let code = u32::from_str_radix(hex, 16).ok()?;
    let character = char::from_u32(code)?;
    if !is_hidden(character) || format!("{code:04X}") != hex {
        return None;
    }
    Some((character, body.get(close + 1..)?))
}

/// Whether the encoder writes `character` as itself.
fn is_plain(character: char, quoted: bool) -> bool {
    character != '\\'
        && character != '\n'
        && (!quoted || character != '"')
        && !is_markup(character)
        && !is_hidden(character)
}

/// Decode one escaped line of free text, exactly as the encoder wrote it.
pub(crate) fn unescape_line(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(character) = rest.chars().next() {
        let after = rest.get(character.len_utf8()..)?;
        if character == '\\' {
            let (decoded, remaining) = decode_escape(after, false)?;
            out.push(decoded);
            rest = remaining;
        } else if is_plain(character, false) {
            out.push(character);
            rest = after;
        } else {
            return None;
        }
    }
    Some(out)
}

/// Decode a quoted value at the start of `text`, exactly as the encoder
/// wrote it. Returns the decoded text and what follows the closing quote.
pub(crate) fn unquote(text: &str) -> Option<(String, &str)> {
    let mut rest = text.strip_prefix('"')?;
    let mut out = String::new();
    loop {
        let character = rest.chars().next()?;
        let after = rest.get(character.len_utf8()..)?;
        if character == '"' {
            return Some((out, after));
        }
        if character == '\\' {
            let (decoded, remaining) = decode_escape(after, true)?;
            out.push(decoded);
            rest = remaining;
        } else if is_plain(character, true) {
            out.push(character);
            rest = after;
        } else {
            return None;
        }
    }
}
