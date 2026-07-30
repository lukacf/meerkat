//! Panic-payload recovery and bounded periodic log gating for
//! `catch_unwind` boundaries.
//!
//! Shared by the meerkat-runtime machine attachment boundaries
//! (`meerkat-runtime/src/panic_boundary.rs`) and the meerkat-mob actor task
//! boundaries (`meerkat-mob/src/runtime/panic_capture.rs`). std-only on
//! purpose: this must stay reachable from every crate that catches a panic.
//!
//! WHY (field incident, 2026-07-29): a panic inside a member-provisioning
//! transaction was caught at an actor-task `catch_unwind` boundary with the
//! payload discarded, converted to an opaque "task panicked" error, and
//! immediately retried — 40+ minutes at 99% CPU across three releases, with
//! each iteration paying for DWARF symbolication (gimli, ~280MB binary)
//! inside the panic hook, whose stderr line the deployment piped to
//! /dev/null. A swallowed payload plus an eager retry is a furnace; every
//! catch site must recover the payload, redact credentials, log it immediately
//! and then only at bounded repeat checkpoints, and feed a typed error.
//!
//! panic=abort compatibility: under `panic = "abort"` `catch_unwind` never
//! observes a panic, so these helpers only run when unwinding actually
//! delivered a payload; nothing here assumes a panic happened otherwise.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// Maximum UTF-8 byte length retained from one panic payload.
///
/// Panic payloads can contain attacker-controlled input, secrets, or an
/// accidentally enormous debug rendering. Every catch boundary uses this
/// helper before the detail reaches logs, error strings, or durable status.
pub const PANIC_PAYLOAD_DETAIL_MAX_BYTES: usize = 512;

/// Maximum number of independent catch-boundary contexts retained by one
/// [`PanicPayloadLogGate`].
///
/// The gate is process-local diagnostics only. Bounding it prevents a stream
/// of one-shot identities or request ids from becoming an unbounded memory
/// sink while preserving transition-based suppression for the recent working
/// set.
pub const PANIC_PAYLOAD_LOG_GATE_MAX_CONTEXTS: usize = 1_024;

const TRUNCATION_MARKER: &str = "…";
const NON_STRING_PANIC_PAYLOAD: &str = "non-string panic payload";
const REDACTION_MARKER: &str = "[REDACTED]";
const PANIC_PAYLOAD_REDACTION_SCAN_MAX_BYTES: usize = PANIC_PAYLOAD_DETAIL_MAX_BYTES * 8;

/// Normalize only a fixed source window. The returned byte count is retained
/// so cost-shape regressions can prove that even discarded whitespace/control
/// input does not turn this diagnostic boundary back into O(payload).
fn normalized_for_redaction_counted(raw: &str) -> (String, bool, usize) {
    let mut normalized =
        String::with_capacity(raw.len().min(PANIC_PAYLOAD_REDACTION_SCAN_MAX_BYTES));
    let mut pending_space = false;
    let mut source_truncated = false;
    let mut inspected_source_bytes = 0_usize;
    for character in raw.chars() {
        let character_bytes = character.len_utf8();
        if inspected_source_bytes.saturating_add(character_bytes)
            > PANIC_PAYLOAD_REDACTION_SCAN_MAX_BYTES
        {
            source_truncated = true;
            break;
        }
        inspected_source_bytes += character_bytes;
        if character.is_control() || character.is_whitespace() {
            pending_space = !normalized.is_empty();
            continue;
        }
        let required_bytes = usize::from(pending_space) + character.len_utf8();
        if normalized.len().saturating_add(required_bytes) > PANIC_PAYLOAD_REDACTION_SCAN_MAX_BYTES
        {
            source_truncated = true;
            break;
        }
        if pending_space {
            normalized.push(' ');
            pending_space = false;
        }
        normalized.push(character);
    }
    if normalized.is_empty() {
        normalized.push_str(NON_STRING_PANIC_PAYLOAD);
    }
    source_truncated |= inspected_source_bytes < raw.len();
    (normalized, source_truncated, inspected_source_bytes)
}

fn normalized_for_redaction(raw: &str) -> (String, bool) {
    let (normalized, source_truncated, _) = normalized_for_redaction_counted(raw);
    (normalized, source_truncated)
}

fn is_ascii_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn has_word_boundary(bytes: &[u8], start: usize, end: usize) -> bool {
    (start == 0 || !is_ascii_word_byte(bytes[start - 1]))
        && (end == bytes.len() || !is_ascii_word_byte(bytes[end]))
}

fn ascii_key_at(input: &str, start: usize) -> Option<usize> {
    const SECRET_KEYS: &[&str] = &[
        "api_key",
        "api-key",
        "apikey",
        "access_token",
        "access-token",
        "token",
        "secret",
        "client_secret",
        "client-secret",
        "password",
        "passwd",
    ];

    let bytes = input.as_bytes();
    SECRET_KEYS.iter().find_map(|key| {
        let end = start.checked_add(key.len())?;
        let candidate = bytes.get(start..end)?;
        (candidate.eq_ignore_ascii_case(key.as_bytes()) && has_word_boundary(bytes, start, end))
            .then_some(end)
    })
}

fn bearer_at(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let end = start.checked_add("bearer".len())?;
    let candidate = bytes.get(start..end)?;
    (candidate.eq_ignore_ascii_case(b"bearer")
        && has_word_boundary(bytes, start, end)
        && bytes.get(end).is_some_and(u8::is_ascii_whitespace))
    .then_some(end)
}

fn provider_credential_prefix_at(input: &str, start: usize) -> bool {
    const CASE_INSENSITIVE_PREFIXES: &[&str] = &[
        "sk-",
        "sk_",
        "rk-",
        "rk_",
        "xai-",
        "xai_",
        "ghp_",
        "github_pat_",
        "hf_",
    ];
    const CASE_SENSITIVE_PREFIXES: &[&str] = &["AIza", "AKIA"];

    let tail = &input.as_bytes()[start..];
    CASE_INSENSITIVE_PREFIXES.iter().any(|prefix| {
        tail.get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix.as_bytes()))
    }) || CASE_SENSITIVE_PREFIXES
        .iter()
        .any(|prefix| tail.starts_with(prefix.as_bytes()))
}

fn assignment_value_start(input: &str, key_end: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut cursor = key_end;
    if bytes
        .get(cursor)
        .is_some_and(|byte| matches!(*byte, b'\'' | b'"'))
    {
        cursor += 1;
    }
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    if !bytes
        .get(cursor)
        .is_some_and(|byte| matches!(*byte, b'=' | b':'))
    {
        return None;
    }
    cursor += 1;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    (cursor < bytes.len()).then_some(cursor)
}

fn secret_value_end(input: &str, start: usize) -> (usize, Option<u8>) {
    let bytes = input.as_bytes();
    if let Some(quote @ (b'\'' | b'"')) = bytes.get(start).copied() {
        let mut cursor = start + 1;
        while cursor < bytes.len() {
            if bytes[cursor] == b'\\' {
                // Quoted JSON/debug/shell-like text commonly escapes an
                // embedded quote. Treat the escaped byte as value material,
                // never as the delimiter; a trailing escape is malformed,
                // so fail closed by retaining nothing after it.
                if cursor + 1 >= bytes.len() {
                    return (bytes.len(), Some(quote));
                }
                cursor += 2;
                continue;
            }
            if bytes[cursor] == quote {
                return (cursor + 1, Some(quote));
            }
            cursor += 1;
        }
        return (bytes.len(), Some(quote));
    }

    let mut cursor = start;
    while cursor < bytes.len()
        && !bytes[cursor].is_ascii_whitespace()
        && !matches!(bytes[cursor], b',' | b';' | b')' | b']' | b'}')
    {
        cursor += 1;
    }
    (cursor, None)
}

fn push_redacted_value(output: &mut String, input: &str, start: usize) -> usize {
    let (end, quote) = secret_value_end(input, start);
    if let Some(quote) = quote {
        output.push(char::from(quote));
        output.push_str(REDACTION_MARKER);
        if input.as_bytes().get(end.saturating_sub(1)) == Some(&quote) {
            output.push(char::from(quote));
        }
    } else {
        output.push_str(REDACTION_MARKER);
    }
    end
}

fn redact_credentials(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len().min(PANIC_PAYLOAD_DETAIL_MAX_BYTES));
    let mut cursor = 0;
    while cursor < bytes.len() {
        if let Some(bearer_end) = bearer_at(input, cursor) {
            output.push_str(&input[cursor..bearer_end]);
            let mut value_start = bearer_end;
            while bytes.get(value_start).is_some_and(u8::is_ascii_whitespace) {
                output.push(char::from(bytes[value_start]));
                value_start += 1;
            }
            cursor = push_redacted_value(&mut output, input, value_start);
            continue;
        }

        if let Some(key_end) = ascii_key_at(input, cursor)
            && let Some(value_start) = assignment_value_start(input, key_end)
        {
            output.push_str(&input[cursor..value_start]);
            cursor = push_redacted_value(&mut output, input, value_start);
            continue;
        }

        if provider_credential_prefix_at(input, cursor) {
            cursor = push_redacted_value(&mut output, input, cursor);
            continue;
        }

        let Some(character) = input[cursor..].chars().next() else {
            break;
        };
        output.push(character);
        cursor += character.len_utf8();
    }
    output
}

fn bound_redacted_detail(mut redacted: String, source_truncated: bool) -> String {
    let output_truncated = source_truncated || redacted.len() > PANIC_PAYLOAD_DETAIL_MAX_BYTES;
    if !output_truncated {
        return redacted;
    }

    let content_budget = PANIC_PAYLOAD_DETAIL_MAX_BYTES.saturating_sub(TRUNCATION_MARKER.len());
    let mut truncate_at = content_budget.min(redacted.len());
    while truncate_at > 0 && !redacted.is_char_boundary(truncate_at) {
        truncate_at -= 1;
    }
    redacted.truncate(truncate_at);
    redacted.push_str(TRUNCATION_MARKER);
    redacted
}

/// Normalize, credential-redact, and byte-bound untrusted diagnostic text.
///
/// Redaction happens before the final truncation so a secret that crosses the
/// output boundary cannot leak as a retained prefix. This is intentionally a
/// conservative textual safety net, not an authorization or audit primitive.
pub fn panic_safe_detail(raw: &str) -> String {
    let (normalized, source_truncated) = normalized_for_redaction(raw);
    bound_redacted_detail(redact_credentials(&normalized), source_truncated)
}

/// Best-effort human-readable panic payload: `&str` and `String` payloads
/// (everything `panic!`/`assert!` produce) are recovered, normalized to one
/// line, credential-redacted, and byte-bounded; anything else (a `panic_any`
/// value) degrades to a fixed marker rather than disappearing.
pub fn panic_payload_detail(payload: &(dyn std::any::Any + Send)) -> String {
    let raw = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or(NON_STRING_PANIC_PAYLOAD);
    panic_safe_detail(raw)
}

#[derive(Debug)]
struct PanicPayloadLogGateEntry {
    detail: String,
    last_seen_sequence: u64,
    repeated_sightings: u64,
}

#[derive(Debug, Default)]
struct PanicPayloadLogGateState {
    entries: BTreeMap<String, PanicPayloadLogGateEntry>,
    next_sequence: u64,
}

/// Bounded periodic payload log gate, keyed by a caller-chosen context key
/// (member identity, `boundary:session`, ...).
///
/// Retry loops can legally re-run a panicking operation indefinitely; the
/// payload must be logged, and a per-iteration `error!` would be its own
/// flood. This records the last payload per key, reports transitions
/// immediately, then emits power-of-two checkpoints for a continuing
/// incident. A success at the same boundary should [`clear`](Self::clear) the
/// key so the next incident logs fresh.
#[derive(Debug, Default)]
pub struct PanicPayloadLogGate {
    state: Mutex<PanicPayloadLogGateState>,
}

/// Logging decision for one panic-payload observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanicPayloadLogDecision {
    /// Log this observation. True for a new/changed payload and at bounded
    /// power-of-two repeat checkpoints.
    pub should_log: bool,
    /// Number of identical repeats observed after the first sighting.
    pub repeated_sightings: u64,
}

impl PanicPayloadLogGate {
    /// Record one payload observation.
    ///
    /// New/changed payloads log immediately. Identical repeats are suppressed
    /// except at power-of-two checkpoints beginning with the eighth repeat,
    /// providing periodic proof that the incident remains active without
    /// returning to per-iteration flooding.
    pub fn observe(&self, key: &str, detail: &str) -> PanicPayloadLogDecision {
        let key = panic_safe_detail(key);
        let detail = panic_safe_detail(detail);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.next_sequence = state.next_sequence.saturating_add(1);
        let sequence = state.next_sequence;
        if let Some(entry) = state.entries.get_mut(&key) {
            entry.last_seen_sequence = sequence;
            if entry.detail == detail {
                entry.repeated_sightings = entry.repeated_sightings.saturating_add(1);
                let should_log =
                    entry.repeated_sightings >= 8 && entry.repeated_sightings.is_power_of_two();
                return PanicPayloadLogDecision {
                    should_log,
                    repeated_sightings: entry.repeated_sightings,
                };
            }
            entry.detail = detail;
            entry.repeated_sightings = 0;
            return PanicPayloadLogDecision {
                should_log: true,
                repeated_sightings: 0,
            };
        }

        if state.entries.len() >= PANIC_PAYLOAD_LOG_GATE_MAX_CONTEXTS
            && let Some(oldest_key) = state
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_seen_sequence)
                .map(|(key, _)| key.clone())
        {
            state.entries.remove(&oldest_key);
        }
        state.entries.insert(
            key,
            PanicPayloadLogGateEntry {
                detail,
                last_seen_sequence: sequence,
                repeated_sightings: 0,
            },
        );
        PanicPayloadLogDecision {
            should_log: true,
            repeated_sightings: 0,
        }
    }

    /// Forget the key's recorded payload (called on success) so a later
    /// recurrence of the same panic logs again as a new incident.
    pub fn clear(&self, key: &str) {
        let key = panic_safe_detail(key);
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .remove(&key);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn payload_detail_recovers_str_string_and_degrades_other_types() {
        assert_eq!(panic_payload_detail(&"boom"), "boom");
        assert_eq!(panic_payload_detail(&"boom".to_string()), "boom");
        assert_eq!(panic_payload_detail(&42_u32), "non-string panic payload");
    }

    #[test]
    fn payload_detail_is_single_line_and_byte_bounded() {
        assert_eq!(
            panic_payload_detail(&" secret\n\tvalue\u{0000} ".to_string()),
            "secret value",
        );
        let detail = panic_payload_detail(&"å".repeat(PANIC_PAYLOAD_DETAIL_MAX_BYTES));
        assert!(detail.ends_with(TRUNCATION_MARKER));
        assert!(detail.len() <= PANIC_PAYLOAD_DETAIL_MAX_BYTES);
        assert!(detail.is_char_boundary(detail.len()));
    }

    #[test]
    fn payload_detail_bounds_source_scan_for_multi_megabyte_whitespace_and_controls() {
        let raw = " \n\t\u{0000}".repeat(1024 * 1024);
        let (normalized, source_truncated, inspected_source_bytes) =
            normalized_for_redaction_counted(&raw);

        assert_eq!(normalized, NON_STRING_PANIC_PAYLOAD);
        assert!(source_truncated);
        assert_eq!(
            inspected_source_bytes,
            PANIC_PAYLOAD_REDACTION_SCAN_MAX_BYTES
        );

        let detail = panic_safe_detail(&raw);
        assert!(detail.ends_with(TRUNCATION_MARKER));
        assert!(detail.len() <= PANIC_PAYLOAD_DETAIL_MAX_BYTES);
    }

    #[test]
    fn payload_detail_redacts_credentials_before_retention() {
        let raw = concat!(
            "Bearer bearer-value ",
            "api_key=\"api value\" ",
            "token=token-value ",
            "secret:secret-value ",
            "password='password value' ",
            "sk-proj-provider-value ",
            "xai-provider-value ",
            "AKIAIOSFODNN7EXAMPLE"
        );
        let detail = panic_payload_detail(&raw);

        for secret in [
            "bearer-value",
            "api value",
            "token-value",
            "secret-value",
            "password value",
            "sk-proj-provider-value",
            "xai-provider-value",
            "AKIAIOSFODNN7EXAMPLE",
        ] {
            assert!(
                !detail.contains(secret),
                "credential survived panic detail redaction: {secret}"
            );
        }
        assert!(detail.matches(REDACTION_MARKER).count() >= 8);
    }

    #[test]
    fn payload_detail_does_not_leak_secret_prefix_at_truncation_boundary() {
        let raw = format!("api_key={}", "credential-fragment-".repeat(1_000));
        let detail = panic_payload_detail(&raw);

        assert!(!detail.contains("credential-fragment"));
        assert!(detail.contains(REDACTION_MARKER));
        assert!(detail.ends_with(TRUNCATION_MARKER));
        assert!(detail.len() <= PANIC_PAYLOAD_DETAIL_MAX_BYTES);
    }

    #[test]
    fn payload_detail_does_not_end_redaction_at_escaped_quotes() {
        let raw = r#"api_key="before\\\"double-suffix" password='before\'single-suffix'"#;
        let detail = panic_payload_detail(&raw);

        for secret in ["before", "double-suffix", "single-suffix", r#"\\\""#, r"\'"] {
            assert!(
                !detail.contains(secret),
                "escaped quoted credential material survived redaction: {secret}"
            );
        }
        assert_eq!(detail.matches(REDACTION_MARKER).count(), 2);
    }

    #[test]
    fn payload_detail_fails_closed_on_trailing_quoted_escape() {
        let detail = panic_payload_detail(&r#"api_key="secret-suffix\"#);

        assert!(!detail.contains("secret-suffix"));
        assert!(detail.contains(REDACTION_MARKER));
    }

    #[test]
    fn gate_redacts_context_keys_and_payloads_before_storing_them() {
        let gate = PanicPayloadLogGate::default();
        let decision = gate.observe(
            "member:api_key=context-secret",
            "Bearer payload-secret password=another-secret",
        );
        assert!(decision.should_log);

        let state = gate
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (key, entry) = state.entries.iter().next().expect("stored gate entry");
        for retained in [key.as_str(), entry.detail.as_str()] {
            assert!(!retained.contains("context-secret"));
            assert!(!retained.contains("payload-secret"));
            assert!(!retained.contains("another-secret"));
        }
    }

    #[test]
    fn gate_reports_transitions_only_and_clears() {
        let gate = PanicPayloadLogGate::default();
        assert!(gate.observe("k", "a").should_log);
        assert!(!gate.observe("k", "a").should_log);
        assert!(gate.observe("k", "b").should_log);
        assert!(gate.observe("other", "b").should_log);
        gate.clear("k");
        assert!(gate.observe("k", "b").should_log);
    }

    #[test]
    fn gate_reports_repeats_at_bounded_power_of_two_checkpoints() {
        let gate = PanicPayloadLogGate::default();
        assert_eq!(
            gate.observe("k", "a"),
            PanicPayloadLogDecision {
                should_log: true,
                repeated_sightings: 0,
            }
        );
        for repeated_sightings in 1..8 {
            assert_eq!(
                gate.observe("k", "a"),
                PanicPayloadLogDecision {
                    should_log: false,
                    repeated_sightings,
                }
            );
        }
        assert_eq!(
            gate.observe("k", "a"),
            PanicPayloadLogDecision {
                should_log: true,
                repeated_sightings: 8,
            }
        );
        for repeated_sightings in 9..16 {
            assert!(!gate.observe("k", "a").should_log, "{repeated_sightings}");
        }
        assert_eq!(gate.observe("k", "a").repeated_sightings, 16);
    }

    #[test]
    fn gate_evicts_oldest_context_at_its_fixed_capacity() {
        let gate = PanicPayloadLogGate::default();
        for index in 0..PANIC_PAYLOAD_LOG_GATE_MAX_CONTEXTS {
            assert!(gate.observe(&format!("key-{index}"), "same").should_log);
        }
        assert!(!gate.observe("key-0", "same").should_log);
        assert!(gate.observe("overflow", "same").should_log);
        assert!(
            gate.observe("key-1", "same").should_log,
            "the oldest context must have been evicted"
        );
        assert!(
            !gate.observe("key-0", "same").should_log,
            "a recently refreshed context must remain admitted"
        );
    }
}
