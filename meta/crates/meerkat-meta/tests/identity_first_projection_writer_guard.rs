//! Source guard for the 0.9.4 projection-writer removal.
//!
//! Core owns every member's session, and a 0.9.3 directory moves into Core's
//! stores with `meta_gateway storage-migrate-native`, which strips
//! `continuity.sqlite3`'s session tables. A runtime path that wrote session
//! rows into Meta's continuity projection again would make the next start read
//! the directory as unmigrated. So no runtime composition may construct the
//! continuity session adapter, hand one to a bridge, or write continuity rows.
//! The adapter and the store types stay for the native migration's census,
//! which reads 0.9.3 continuity on scratch copies; only their writers are
//! fenced here. Test code is out of scope: the scan skips `#[cfg(test)]`
//! items, and it reads code only, never string, char or comment contents.

#![allow(clippy::panic)]

const SOURCES: &[(&str, &str)] = &[
    ("src/lib.rs", include_str!("../src/lib.rs")),
    (
        "src/gateway_wiring.rs",
        include_str!("../src/gateway_wiring.rs"),
    ),
    ("src/rpc_gateway.rs", include_str!("../src/rpc_gateway.rs")),
    ("src/rpc.rs", include_str!("../src/rpc.rs")),
    (
        "src/http_console.rs",
        include_str!("../src/http_console.rs"),
    ),
    (
        "src/mob_handle_runtime.rs",
        include_str!("../src/mob_handle_runtime.rs"),
    ),
    (
        "src/storage_provider.rs",
        include_str!("../src/storage_provider.rs"),
    ),
    (
        "src/topology_control.rs",
        include_str!("../src/topology_control.rs"),
    ),
    (
        "src/unified_runtime/mod.rs",
        include_str!("../src/unified_runtime/mod.rs"),
    ),
    (
        "src/unified_runtime/builder.rs",
        include_str!("../src/unified_runtime/builder.rs"),
    ),
    (
        "src/identity_first/runtime.rs",
        include_str!("../src/identity_first/runtime.rs"),
    ),
    (
        "src/identity_first/bridge.rs",
        include_str!("../src/identity_first/bridge.rs"),
    ),
    (
        "src/bin/meta_gateway.rs",
        include_str!("../src/bin/meta_gateway.rs"),
    ),
    (
        "src/bin/rpc_gateway.rs",
        include_str!("../src/bin/rpc_gateway.rs"),
    ),
    (
        "src/bin/meta_repair.rs",
        include_str!("../src/bin/meta_repair.rs"),
    ),
];

/// Calls that construct or feed the continuity projection's writers.
const WRITERS: &[&str] = &[
    "ContinuitySessionStoreAdapter::new(",
    "with_continuity_session_store(",
    ".save_session_snapshot(",
    ".save_session_snapshot_owned(",
    ".upsert_continuity_record(",
    ".delete_continuity_record(",
    ".rollback_continuity_record(",
];

fn is_ident(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// The source with every comment and every string or char literal blanked
/// to spaces, newlines kept, so braces and calls are read from code only.
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let blank = |character: char| if character == '\n' { '\n' } else { ' ' };
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        let next = chars.get(index + 1).copied();
        if character == '/' && next == Some('/') {
            while index < chars.len() && chars[index] != '\n' {
                out.push(' ');
                index += 1;
            }
            continue;
        }
        if character == '/' && next == Some('*') {
            let mut depth = 0usize;
            while index < chars.len() {
                let pair = (chars[index], chars.get(index + 1).copied());
                if pair == ('/', Some('*')) {
                    depth += 1;
                    out.push_str("  ");
                    index += 2;
                } else if pair == ('*', Some('/')) {
                    depth -= 1;
                    out.push_str("  ");
                    index += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(chars[index]));
                    index += 1;
                }
            }
            continue;
        }
        let starts_raw = character == 'r'
            && (index == 0
                || !is_ident(chars[index - 1])
                || (chars[index - 1] == 'b' && (index < 2 || !is_ident(chars[index - 2]))));
        if starts_raw {
            let mut hashes = 0;
            while chars.get(index + 1 + hashes) == Some(&'#') {
                hashes += 1;
            }
            if chars.get(index + 1 + hashes) == Some(&'"') {
                let open = 2 + hashes;
                out.push_str(&" ".repeat(open));
                index += open;
                while index < chars.len() {
                    if chars[index] == '"'
                        && (1..=hashes).all(|offset| chars.get(index + offset) == Some(&'#'))
                    {
                        out.push_str(&" ".repeat(1 + hashes));
                        index += 1 + hashes;
                        break;
                    }
                    out.push(blank(chars[index]));
                    index += 1;
                }
                continue;
            }
        }
        if character == '"' {
            out.push(' ');
            index += 1;
            while index < chars.len() {
                match chars[index] {
                    '\\' => {
                        out.push(' ');
                        if let Some(escaped) = chars.get(index + 1) {
                            out.push(blank(*escaped));
                        }
                        index += 2;
                    }
                    '"' => {
                        out.push(' ');
                        index += 1;
                        break;
                    }
                    other => {
                        out.push(blank(other));
                        index += 1;
                    }
                }
            }
            continue;
        }
        if character == '\'' {
            // A char literal, not a lifetime: `'x'`, or an escape such as
            // `'\''`, `'\n'` or `'\u{7b}'`.
            let end = if next == Some('\\') {
                (index + 2..chars.len().min(index + 12))
                    .find(|&at| chars[at] == '\'' && at > index + 2)
            } else if chars.get(index + 2) == Some(&'\'') {
                Some(index + 2)
            } else {
                None
            };
            if let Some(end) = end {
                out.push_str(&" ".repeat(end + 1 - index));
                index = end + 1;
                continue;
            }
        }
        out.push(character);
        index += 1;
    }
    out
}

/// The code with every `#[cfg(test)]` item removed: the attribute, any
/// attributes after it, and the item through its closing brace (or its `;`).
fn production_code(source: &str) -> String {
    let code = code_only(source);
    let lines: Vec<&str> = code.lines().collect();
    let mut kept = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim_start();
        if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[cfg(all(test") {
            let mut item = index + 1;
            while item < lines.len() && lines[item].trim_start().starts_with("#[") {
                item += 1;
            }
            // A one-line item: a `use`, a statement, a field or a match arm.
            if item < lines.len() {
                let first = lines[item].trim_end();
                if first.ends_with(';') || (first.ends_with(',') && !first.contains('{')) {
                    index = item + 1;
                    continue;
                }
            }
            let start = item;
            let mut depth: i64 = 0;
            let mut opened = false;
            while item < lines.len() {
                for character in lines[item].chars() {
                    match character {
                        '{' => {
                            depth += 1;
                            opened = true;
                        }
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
                if opened && depth <= 0 {
                    break;
                }
                if !opened && depth < 0 {
                    // The enclosing block closed before the item opened one:
                    // the item was its first line only.
                    item = start;
                    break;
                }
                item += 1;
            }
            index = item + 1;
            continue;
        }
        kept.push(lines[index]);
        index += 1;
    }
    kept.join("\n")
}

fn writers_in(path: &str, source: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in production_code(source).lines() {
        for writer in WRITERS {
            if line.contains(writer) {
                found.push(format!("{path}: {}", line.trim()));
            }
        }
    }
    found
}

#[test]
fn no_runtime_composition_writes_the_continuity_projection() {
    let found: Vec<String> = SOURCES
        .iter()
        .flat_map(|(path, source)| writers_in(path, source))
        .collect();
    assert!(
        found.is_empty(),
        "a runtime composition writes Meta's continuity projection again: {found:#?}"
    );
}

/// The scan must keep seeing production code after a test module whose
/// strings, chars and comments hold unbalanced braces, and must not count a
/// writer that only appears in test code, a string or a comment.
#[test]
fn the_scan_reads_production_code_past_tricky_test_modules() {
    let source = concat!(
        "#[cfg(test)]\n",
        "mod tests {\n",
        "    const OPEN: &str = \"{\";\n",
        "    const RAW: &str = r#\"{ \"quoted\" {\"#;\n",
        "    const BRACE: char = '{';\n",
        "    const QUOTE: char = '\\'';\n",
        "    // a stray { in a comment\n",
        "    /* and { another */\n",
        "    fn t<'a>(s: &'a str) { let _ = ContinuitySessionStoreAdapter::new(s); }\n",
        "}\n",
        "struct Wiring {\n",
        "    #[cfg(test)]\n",
        "    hook: Option<u8>,\n",
        "    real: u8,\n",
        "}\n",
        "fn compose() {\n",
        "    let note = \"ContinuitySessionStoreAdapter::new(\";\n",
        "    // ContinuitySessionStoreAdapter::new( in a comment\n",
        "    let adapter = ContinuitySessionStoreAdapter::new(store);\n",
        "}\n",
    );
    let found = writers_in("fixture.rs", source);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].contains("let adapter"), "{found:#?}");
}
