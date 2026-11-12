//! Source guard and rows for Meta's one answer to "which identity is this
//! member" ([`crate::member_comms_id::resolve_member_identity`]).
//!
//! The source reading below (`production`, `code_only` and their helpers)
//! follows the helpers of `identity_first/runtime/seat_tests.rs`, which are
//! private to that module: comments and literals are blanked before code is
//! matched, and every `#[cfg(test)]` module is removed.

#![allow(clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};

use crate::member_comms_id::{mob_member_id, resolve_member_identity};

/// The resolver's own module, by its path from `src/`.
const RESOLVER_MODULE: &str = "member_comms_id.rs";

/// A production file that may read the label outside the resolver.
struct AllowedReader {
    /// The file, by its path from `src/`.
    file: &'static str,
    /// How many label reads the file may hold: `Some(n)` is exact, so a new
    /// read fails the guard and a removed one asks for this row's removal.
    /// `None` leaves the count open for a file its owner is changing.
    reads: Option<usize>,
    /// Why the read is no reader of "which identity is this member", or why
    /// it could not be switched yet.
    why: &'static str,
}

/// Every production file that reads the `agent_identity` label outside the
/// resolver's module, and why.
const ALLOWED_READERS: &[AllowedReader] = &[
    AllowedReader {
        file: "extension_state.rs",
        reads: None,
        why: "refuses a principal: the label there only tells that a member is an \
              identity's, to deny it a document operation; it resolves no identity",
    },
    AllowedReader {
        file: "identity_first/runtime.rs",
        reads: None,
        why: "reads a binding's spawn labels from Core's member history (which binding an \
              identity's spawn made), the question `binding_materialized_by_roster` asks; \
              `membership_is_read_from_a_binding_s_label_only_where_allowed` bounds it",
    },
    AllowedReader {
        file: "identity_first/bridge.rs",
        reads: None,
        why: "the session bridge is what answers the resolver's entry question; its own \
              roster reads pair the label with that answer in place and are its owner's \
              to switch",
    },
    AllowedReader {
        file: "console_spawn.rs",
        reads: Some(1),
        why: "`spawn_seed_console_identity` judges a spawn from its arguments, before or \
              without a roster row: Core holds no entry to ask, and a spawn tool never \
              seats an identity entry's member",
    },
    AllowedReader {
        file: "http_console.rs",
        reads: Some(1),
        why: "`console_member_console_identity` reads a `ConsoleMember`, a detached \
              projection (several mobs, the alias space) with no mob handle; its fallback \
              is the member's alias, which is what the resolver answers for an identity \
              entry's member",
    },
    AllowedReader {
        file: "runtime/console_ingress.rs",
        reads: Some(2),
        why: "`console_member_console_identity` and `console_member_access_attributes` \
              read a `ConsoleMember` or its fields, a detached projection with no mob \
              handle; their fallback is the member's alias",
    },
    AllowedReader {
        file: "console_aggregator/mod.rs",
        reads: Some(2),
        why: "`identity_record_prefer` reads a `ConsoleIdentityRecord`, a serialized \
              projection with no mob to ask, only to prefer a labelled roster row over a \
              durable row listed under the very same id (a member seated under the \
              generated alias itself). Which roster row realizes a binding is no label \
              read: `live_record_realizes_durable_binding` and `dedupe_identity_records` \
              compare the row's roster id with Core's member id of the binding, which \
              the durable record carries, so an unlabelled store-owned row answers as a \
              labelled one does",
    },
];

/// The label reads the resolver's own module holds: the label helper's
/// definition, the resolver's call of it, and `binding_materialized_by_roster`,
/// which asks a different question (whether a roster pass materialized the
/// member of a binding in Core's member history, by its spawn labels).
const RESOLVER_MODULE_CALLS: usize = 3;

/// Source guard: no production file reads the `agent_identity` label for a
/// member's identity outside the resolver's module, except the allowlisted
/// readers. A reader of "which identity is this member" calls
/// [`resolve_member_identity`], so a member Core holds as an identity entry
/// (seated with no such label) is never missed.
#[test]
fn only_the_resolver_reads_a_member_s_identity_label() {
    let sources = crate_production_sources();
    assert!(
        sources.len() > 100,
        "suspiciously few production sources: {}",
        sources.len()
    );
    let mut found: BTreeMap<String, usize> = BTreeMap::new();
    for (file, source) in &sources {
        let (code, text) = production(source);
        let squeezed: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
        let reads = code.matches("durable_identity_label(").count()
            + squeezed.matches("labels.get(\"agent_identity\")").count()
            + squeezed.matches("labels().get(\"agent_identity\")").count();
        if reads > 0 {
            found.insert(file.clone(), reads);
        }
        if file == RESOLVER_MODULE {
            for item in [
                "fn resolve_member_identity<",
                "fn binding_materialized_by_roster(",
            ] {
                assert_eq!(
                    item_body(&code, item)
                        .matches("durable_identity_label(")
                        .count(),
                    1,
                    "`{item}` reads the label once"
                );
            }
            assert_eq!(
                code.matches("durable_identity_label(").count(),
                RESOLVER_MODULE_CALLS,
                "{RESOLVER_MODULE} reads the label in its definition, the resolver and \
                 `binding_materialized_by_roster` only"
            );
        }
    }
    assert!(
        found.remove(RESOLVER_MODULE).is_some(),
        "the resolver's module reads the label"
    );
    for allowed in ALLOWED_READERS {
        let reads = found.remove(allowed.file);
        if let Some(expected) = allowed.reads {
            assert_eq!(
                reads,
                Some(expected),
                "{} holds exactly {expected} allowed label read(s) ({}); switch a new \
                 one to `resolve_member_identity`, and drop this row when none is left",
                allowed.file,
                allowed.why
            );
        }
    }
    assert!(
        found.is_empty(),
        "these production files read a member's `agent_identity` label themselves; ask \
         `member_comms_id::resolve_member_identity` instead, so a member Core holds as \
         an identity entry is resolved too: {found:?}"
    );
}

/// The resolver's three answers, with no mob: the label wins, an identity
/// entry's member with no label is its identity's, and every other member
/// is no identity's.
#[test]
fn a_member_resolves_by_its_label_else_by_core_s_identity_entry() {
    let unlabelled = BTreeMap::new();
    let labelled =
        |identity: &str| BTreeMap::from([("agent_identity".to_string(), identity.to_string())]);
    let entries = BTreeSet::from([
        mob_member_id("keeper"),
        mob_member_id("review:keeper"),
        mob_member_id("rt:review:elder:3"),
    ]);
    let none = BTreeSet::new();
    let resolved = |entries: &BTreeSet<meerkat_mob::ids::AgentIdentity>,
                    alias: &str,
                    labels: &BTreeMap<String, String>| {
        resolve_member_identity(entries, &mob_member_id(alias), labels)
            .map(std::borrow::Cow::into_owned)
    };

    // A store-owned identity's member: no label, an identity entry.
    assert_eq!(
        resolved(&entries, "keeper", &unlabelled).as_deref(),
        Some("keeper")
    );
    assert_eq!(
        resolved(&entries, "review:keeper", &unlabelled).as_deref(),
        Some("review:keeper"),
        "the identity is the one the encoded member id decodes to"
    );
    assert_eq!(
        resolved(&entries, "rt:review:elder:3", &unlabelled).as_deref(),
        Some("review:elder"),
        "a generated-alias member id names its durable identity"
    );
    // The configured twin: the label answers, with or without an entry.
    for held in [&entries, &none] {
        assert_eq!(
            resolved(held, "review:keeper", &labelled("review:twin")).as_deref(),
            Some("review:twin"),
            "the label wins"
        );
    }
    // The label's own filter stands: an unusable label is no answer, and
    // the entry then is.
    for unusable in ["", "  ", "rt:review:keeper:0"] {
        assert_eq!(resolved(&none, "worker", &labelled(unusable)), None);
        assert_eq!(
            resolved(&entries, "keeper", &labelled(unusable)).as_deref(),
            Some("keeper")
        );
    }
    // A plain worker: no label, no entry.
    assert_eq!(resolved(&entries, "worker", &unlabelled), None);
    assert_eq!(resolved(&none, "keeper", &unlabelled), None);
    // No Core read in reach answers by the label alone.
    let no_core: Option<&BTreeSet<meerkat_mob::ids::AgentIdentity>> = None;
    assert_eq!(
        resolve_member_identity(&no_core, &mob_member_id("keeper"), &unlabelled),
        None
    );
    assert_eq!(
        resolve_member_identity(&Some(&entries), &mob_member_id("keeper"), &unlabelled).as_deref(),
        Some("keeper")
    );
}

/// Every production source of this crate: each `.rs` file under `src/` (its
/// binaries included), by its path from `src/`, except the test files
/// (`*_tests.rs`, `tests.rs`, and the files of a `tests` or `*_tests`
/// directory). A file added later is read too.
fn crate_production_sources() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("the crate's sources list") {
            let path = entry.expect("a source entry reads").path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            if path.is_dir() {
                if name != "tests" && !name.ends_with("_tests") {
                    dirs.push(path);
                }
                continue;
            }
            if !name.ends_with(".rs") || name == "tests.rs" || name.ends_with("_tests.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("a source reads");
            let relative = path
                .strip_prefix(&root)
                .expect("a source under src")
                .to_string_lossy()
                .replace('\\', "/");
            sources.push((relative, source));
        }
    }
    sources.sort();
    sources
}

/// The production part of `source`, twice: as code (comments and literals
/// blanked to spaces) and as text (comments and literals kept), each with
/// every `#[cfg(test)]` module removed, inline or out of line.
fn production(source: &str) -> (String, String) {
    const TEST_GATE: &[u8] = b"#[cfg(test)]";
    let code = code_only(source);
    let raw = source.as_bytes();
    let mut kept_code = Vec::with_capacity(code.len());
    let mut kept_text = Vec::with_capacity(raw.len());
    let mut copied = 0;
    let mut at = 0;
    while let Some(start) = find_from(&code, TEST_GATE, at) {
        match test_module_end(&code, start + TEST_GATE.len()) {
            Some(end) => {
                kept_code.extend_from_slice(&code[copied..start]);
                kept_text.extend_from_slice(&raw[copied..start]);
                copied = end;
                at = end;
            }
            None => at = start + 1,
        }
    }
    kept_code.extend_from_slice(&code[copied..]);
    kept_text.extend_from_slice(&raw[copied..]);
    (
        String::from_utf8_lossy(&kept_code).into_owned(),
        String::from_utf8_lossy(&kept_text).into_owned(),
    )
}

/// The body of the item whose signature starts with `signature` in `code`
/// (comments and literals blanked): from its first `{` to the matching `}`.
/// Empty when the item is absent.
fn item_body<'a>(code: &'a str, signature: &str) -> &'a str {
    let bytes = code.as_bytes();
    code.find(signature)
        .and_then(|start| find_from(bytes, b"{", start))
        .and_then(|open| matching_close(bytes, open, b'{', b'}').map(|close| &code[open..close]))
        .unwrap_or_default()
}

/// `source` with every comment and every string, raw string and character
/// literal blanked to spaces (newlines kept), byte for byte: what is left is
/// the code, at the same offsets.
fn code_only(source: &str) -> Vec<u8> {
    let bytes = source.as_bytes();
    let mut code = bytes.to_vec();
    let mut at = 0;
    while at < bytes.len() {
        let end = match bytes[at] {
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                find_from(bytes, b"\n", at).unwrap_or(bytes.len())
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => block_comment_end(bytes, at),
            b'"' => string_end(bytes, at),
            b'r' | b'b' => match raw_string_end(bytes, at) {
                Some(end) => end,
                None => {
                    at += 1;
                    continue;
                }
            },
            b'\'' => match char_literal_end(source, at) {
                Some(end) => end,
                None => {
                    at += 1;
                    continue;
                }
            },
            _ => {
                at += 1;
                continue;
            }
        };
        let end = end.min(bytes.len());
        for byte in &mut code[at..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        at = end;
    }
    code
}

/// The first position of `needle` in `haystack` at or after `from`.
fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| from + position)
}

/// Where the block comment opening at `from` ends: block comments nest.
fn block_comment_end(bytes: &[u8], from: usize) -> usize {
    let mut depth = 0_usize;
    let mut at = from;
    while at < bytes.len() {
        if bytes[at..].starts_with(b"/*") {
            depth += 1;
            at += 2;
        } else if bytes[at..].starts_with(b"*/") {
            depth = depth.saturating_sub(1);
            at += 2;
            if depth == 0 {
                return at;
            }
        } else {
            at += 1;
        }
    }
    bytes.len()
}

/// Where the string literal opening at `at` ends: after its closing quote.
fn string_end(bytes: &[u8], at: usize) -> usize {
    let mut end = at + 1;
    while let Some(byte) = bytes.get(end) {
        match byte {
            b'\\' => end += 2,
            b'"' => return end + 1,
            _ => end += 1,
        }
    }
    bytes.len()
}

/// Where the raw string literal (`r"..."`, `r#"..."#`, `br"..."`) opening at
/// `at` ends, or `None` when none opens there: a raw identifier such as
/// `r#type`, or an identifier that ends in `r` or `b`.
fn raw_string_end(bytes: &[u8], at: usize) -> Option<usize> {
    if at
        .checked_sub(1)
        .and_then(|before| bytes.get(before))
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return None;
    }
    let mut quote = at;
    if bytes.get(quote) == Some(&b'b') {
        quote += 1;
    }
    if bytes.get(quote) != Some(&b'r') {
        return None;
    }
    quote += 1;
    let hashes = bytes
        .get(quote..)?
        .iter()
        .take_while(|byte| **byte == b'#')
        .count();
    quote += hashes;
    if bytes.get(quote) != Some(&b'"') {
        return None;
    }
    let mut closing = vec![b'"'];
    closing.extend(std::iter::repeat_n(b'#', hashes));
    Some(find_from(bytes, &closing, quote + 1).map_or(bytes.len(), |close| close + closing.len()))
}

/// Where the character literal opening at `at` ends, or `None` when the
/// quote opens a lifetime or a label.
fn char_literal_end(source: &str, at: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(at + 1) == Some(&b'\\') {
        // An escape: the literal ends at the first quote after the escaped
        // character.
        return find_from(bytes, b"'", at + 3).map(|close| close + 1);
    }
    let width = source.get(at + 1..)?.chars().next()?.len_utf8();
    (bytes.get(at + 1 + width) == Some(&b'\'')).then_some(at + 2 + width)
}

/// Where the item a `#[cfg(test)]` that ends at `from` gates ends, when
/// that item is a module: after its `;` (out of line) or after the brace
/// that closes its body. `None` when the gated item is no module.
fn test_module_end(code: &[u8], from: usize) -> Option<usize> {
    let mut at = skip_whitespace(code, from);
    // Further attributes of the same item.
    while code.get(at) == Some(&b'#') {
        at = skip_whitespace(code, matching_close(code, at + 1, b'[', b']')?);
    }
    // Its visibility.
    if code.get(at..)?.starts_with(b"pub") {
        at = skip_whitespace(code, at + 3);
        if code.get(at) == Some(&b'(') {
            at = skip_whitespace(code, matching_close(code, at, b'(', b')')?);
        }
    }
    if !code.get(at..)?.starts_with(b"mod")
        || !code.get(at + 3).is_some_and(u8::is_ascii_whitespace)
    {
        return None;
    }
    at += 3;
    // The module's name, then its `;` or its body.
    while code.get(at).is_some_and(|byte| {
        byte.is_ascii_whitespace() || byte.is_ascii_alphanumeric() || *byte == b'_'
    }) {
        at += 1;
    }
    match code.get(at)? {
        b';' => Some(at + 1),
        b'{' => matching_close(code, at, b'{', b'}'),
        _ => None,
    }
}

/// The position just after the `close` that matches the `open` at `at`.
fn matching_close(code: &[u8], at: usize, open: u8, close: u8) -> Option<usize> {
    if code.get(at) != Some(&open) {
        return None;
    }
    let mut depth = 0_usize;
    for (offset, byte) in code.get(at..)?.iter().enumerate() {
        if *byte == open {
            depth += 1;
        } else if *byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(at + offset + 1);
            }
        }
    }
    None
}

/// The first position at or after `at` that is no whitespace.
fn skip_whitespace(code: &[u8], at: usize) -> usize {
    code.get(at..).map_or(at, |rest| {
        at + rest
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .count()
    })
}
