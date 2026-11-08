//! RFC 8785 JSON canonicalization (JCS): one byte sequence per JSON value,
//! so a digest of it names the value whatever its producer's formatting.
//!
//! Objects are written with their members sorted by the UTF-16 code units of
//! their names, no insignificant whitespace, strings in ECMAScript's
//! `JSON.stringify` form (only `"`, `\` and the control characters below
//! U+0020 escaped) and numbers in ECMAScript's `Number.prototype.toString`
//! form (shortest round trip; exponent notation below 1e-6 and from 1e21).
//!
//! A value that has no canonical form answers `None`: an integer that an
//! IEEE 754 double cannot hold exactly. RFC 8785 would serialize the nearest
//! double instead, so two different integers would share one canonical text;
//! a caller that binds an operation by its digest must never let one stand
//! for the other.
//!
//! A number is canonicalized as the double the value holds, so its producer's
//! parser decides it: two producers agree on a digest when they parse alike.
//! Meerkat's producers parse with `serde_json`, whose default parser may
//! round a decimal of more than 17 significant digits to a neighbouring
//! double; two such texts then differ in their digest, never share one they
//! should not.

use std::fmt::Write as _;

/// The RFC 8785 canonical text of `value`, or `None` when it has none (an
/// integer an IEEE 754 double cannot hold exactly).
#[must_use]
pub fn canonical_json(value: &serde_json::Value) -> Option<String> {
    let mut out = String::new();
    write_value(&mut out, value)?;
    Some(out)
}

fn write_value(out: &mut String, value: &serde_json::Value) -> Option<()> {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(true) => out.push_str("true"),
        serde_json::Value::Bool(false) => out.push_str("false"),
        serde_json::Value::Number(number) => out.push_str(&number_text(number)?),
        serde_json::Value::String(text) => write_string(out, text),
        serde_json::Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(out, item)?;
            }
            out.push(']');
        }
        serde_json::Value::Object(members) => {
            let mut sorted: Vec<(&String, &serde_json::Value)> = members.iter().collect();
            sorted.sort_by(|left, right| left.0.encode_utf16().cmp(right.0.encode_utf16()));
            out.push('{');
            for (index, (name, member)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(out, name);
                out.push(':');
                write_value(out, member)?;
            }
            out.push('}');
        }
    }
    Some(())
}

/// ECMAScript `JSON.stringify` string form: `"` and `\` escaped, the control
/// characters below U+0020 as their short escapes or `\u00xx`, every other
/// character as itself.
fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{09}' => out.push_str("\\t"),
            '\u{0a}' => out.push_str("\\n"),
            '\u{0c}' => out.push_str("\\f"),
            '\u{0d}' => out.push_str("\\r"),
            control if u32::from(control) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// The largest integer every smaller integer of which an IEEE 754 double
/// holds exactly (2^53).
const EXACT_INTEGER_LIMIT: u64 = 1 << 53;

fn number_text(number: &serde_json::Number) -> Option<String> {
    if let Some(value) = number.as_u64() {
        return integer_text(value, false);
    }
    if let Some(value) = number.as_i64() {
        return integer_text(value.unsigned_abs(), true);
    }
    number.as_f64().and_then(double_text)
}

/// An integer: as its digits while a double holds it exactly, else as the
/// double, only when that double is exactly the integer.
fn integer_text(magnitude: u64, negative: bool) -> Option<String> {
    if magnitude <= EXACT_INTEGER_LIMIT {
        return Some(if negative && magnitude != 0 {
            format!("-{magnitude}")
        } else {
            magnitude.to_string()
        });
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "the round trip below accepts only an exact conversion"
    )]
    let double = magnitude as f64;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a finite double at most u64::MAX converts back exactly or not at all"
    )]
    let exact = double < 18_446_744_073_709_551_616.0 && double as u64 == magnitude;
    if !exact {
        return None;
    }
    double_text(if negative { -double } else { double })
}

/// ECMAScript `Number.prototype.toString` of a finite double: the shortest
/// digits that round-trip, in decimal notation for exponents from -6 to 20
/// and in exponent notation (`1e+21`, `1.5e-7`) beyond.
fn double_text(value: f64) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    if value == 0.0 {
        // Negative zero is "0" too.
        return Some("0".to_string());
    }
    // Rust's `{:e}` form is the shortest round-trip digits: `d[.ddd]e[-]x`.
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e')?;
    let exponent: i32 = exponent.parse().ok()?;
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let count = i32::try_from(digits.len()).ok()?;
    // The value is 0.digits * 10^point.
    let point = exponent + 1;
    let mut out = String::new();
    if value.is_sign_negative() {
        out.push('-');
    }
    if count <= point && point <= 21 {
        out.push_str(digits);
        for _ in 0..(point - count) {
            out.push('0');
        }
    } else if 0 < point && point <= 21 {
        let split = usize::try_from(point).ok()?;
        out.push_str(&digits[..split]);
        out.push('.');
        out.push_str(&digits[split..]);
    } else if -6 < point && point <= 0 {
        out.push_str("0.");
        for _ in 0..(-point) {
            out.push('0');
        }
        out.push_str(digits);
    } else {
        let shown = point - 1;
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if shown >= 0 { '+' } else { '-' });
        let _ = write!(out, "{}", shown.unsigned_abs());
    }
    Some(out)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::canonical_json;

    fn canonical(text: &str) -> Option<String> {
        canonical_json(&serde_json::from_str(text).expect("json"))
    }

    /// RFC 8785's own example, with its first number given in the shortest
    /// form of its double (the RFC's 17-digit form is past the precision
    /// `serde_json`'s default parser rounds exactly).
    #[test]
    fn the_rfc_8785_example_is_canonical() {
        let input = r#"{
            "numbers": [333333333.3333333, 1E30, 4.50, 2e-3, 0.000000000000000000000000001],
            "string": "\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/",
            "literals": [null, true, false]
        }"#;
        assert_eq!(
            canonical(input).as_deref(),
            Some(
                "{\"literals\":[null,true,false],\"numbers\":[333333333.3333333,1e+30,4.5,0.002,1e-27],\"string\":\"\u{20ac}$\\u000f\\nA'B\\\"\\\\\\\\\\\"/\"}"
            )
        );
    }

    #[test]
    fn numbers_take_the_ecmascript_form() {
        for (input, expected) in [
            ("0", "0"),
            ("-0.0", "0"),
            ("1", "1"),
            ("1.0", "1"),
            ("-12", "-12"),
            ("100", "100"),
            ("1e20", "100000000000000000000"),
            ("1e21", "1e+21"),
            ("0.000001", "0.000001"),
            ("0.0000001", "1e-7"),
            ("1.5e-7", "1.5e-7"),
            ("-2.5e+25", "-2.5e+25"),
            ("9007199254740992", "9007199254740992"),
            ("9007199254740994", "9007199254740994"),
        ] {
            assert_eq!(canonical(input).as_deref(), Some(expected), "{input}");
        }
    }

    #[test]
    fn an_integer_a_double_cannot_hold_has_no_canonical_form() {
        assert_eq!(canonical("9007199254740993"), None);
        assert_eq!(canonical("-9007199254740993"), None);
        assert_eq!(canonical("18446744073709551615"), None);
        assert_eq!(canonical("[1, 9007199254740993]"), None);
        assert_eq!(canonical("{\"a\": 9007199254740993}"), None);
    }

    #[test]
    fn members_sort_by_utf16_code_units() {
        // U+FF61 sorts after U+1F600 in UTF-16 (a surrogate pair starts
        // with 0xD83D), though not by code point.
        let input = "{\"\u{ff61}\":1,\"\u{1f600}\":2,\"b\":3,\"a\":{\"z\":1,\"y\":2}}";
        assert_eq!(
            canonical(input).as_deref(),
            Some("{\"a\":{\"y\":2,\"z\":1},\"b\":3,\"\u{1f600}\":2,\"\u{ff61}\":1}")
        );
    }

    #[test]
    fn strings_escape_only_quotes_backslashes_and_controls() {
        let input = "\"\\u001f\u{7f}\u{2028}<>&\"";
        assert_eq!(
            canonical(input).as_deref(),
            Some("\"\\u001f\u{7f}\u{2028}<>&\"")
        );
    }
}
