//! A tool dispatcher that takes its catalog capabilities from another
//! dispatcher answers `tool_catalog_control_plane` from it too.
//!
//! `AgentToolDispatcher::tool_catalog_control_plane` has a default: `Always`
//! when the dispatcher's capabilities set `may_require_catalog_control_plane`,
//! `Never` otherwise. A wrapper that forwards
//! `tool_catalog_capabilities` and leaves the control plane on that default
//! compiles, and turns an inner `OnDemand` into `Always`: the catalog control
//! tools and their system prompt line are then in every request of a session
//! whose deferrable tools are all in the request. So every wrapper forwards
//! the method (one inner dispatcher), or takes the strongest answer of its
//! parts (`ToolCatalogControlPlane` is ordered `Never < OnDemand < Always`).
//!
//! `WRAPPERS` lists every such wrapper of this crate's shipped code. A wrapper that is not listed
//! fails the guard until it is listed, a listed one that is gone fails it
//! too, and so does a wrapper whose `tool_catalog_control_plane` does not
//! read another dispatcher's answer.
//!
//! The scan reads shipped code only, with comments and literals blanked and
//! test-only items removed.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Every shipped dispatcher that takes its catalog capabilities from another
/// dispatcher: the file and the implementing type.
const WRAPPERS: &[(&str, &str)] = &[
    ("src/service_install.rs", "CheckedInstalls"),
    ("src/terminal_connector.rs", "PlainRequestTool"),
    (
        "src/terminal_connector/private_service.rs",
        "PrivateRequestTool",
    ),
];

/// Every `.rs` file of this crate's `src` tree, with its path relative to
/// the crate. The crate directory is read at run time (cargo and nextest
/// set `CARGO_MANIFEST_DIR`), so an archived test binary never carries the
/// builder's path.
fn crate_sources() -> Vec<(String, PathBuf)> {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set for test processes"),
    );
    let mut out = Vec::new();
    let mut pending = vec![manifest_dir.join("src")];
    while let Some(dir) = pending.pop() {
        let entries =
            fs::read_dir(&dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = relative_to(&manifest_dir, &path);
                out.push((relative, path));
            }
        }
    }
    out.sort();
    out
}

/// `path` relative to `root`, with forward slashes.
fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Whether a crate-relative source path is shipped code: not a test-only
/// file (`tests.rs`, `*_tests.rs`, anything under a `tests` directory).
fn is_shipped_source(relative: &str) -> bool {
    let file = relative.rsplit('/').next().unwrap_or(relative);
    let stem = file.strip_suffix(".rs").unwrap_or(file);
    !(stem == "tests"
        || stem.ends_with("_tests")
        || relative.split('/').any(|part| part == "tests"))
}

/// Every shipped source file of this crate, with its crate-relative path.
fn shipped_sources() -> Vec<(String, PathBuf)> {
    crate_sources()
        .into_iter()
        .filter(|(relative, _)| is_shipped_source(relative))
        .collect()
}

/// `source` with comments and literals blanked and every test-only item
/// (`#[cfg(test)]`, `#[cfg(all(test, ..))]`, `#[test]` and `#[*::test]`)
/// removed, so only code a shipped build compiles remains.
fn shipped_code(source: &str) -> String {
    let code = blank_comments_and_literals(source);
    let bytes = code.as_bytes();
    let mut out = String::with_capacity(code.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'#'
            && let Some((attribute, end)) = attribute_at(&code, index)
        {
            if gates_test_only(attribute) {
                index = item_end(bytes, end);
                continue;
            }
            out.push_str(&code[index..end]);
            index = end;
            continue;
        }
        let character = code[index..].chars().next().unwrap();
        out.push(character);
        index += character.len_utf8();
    }
    out
}

/// The attribute starting at `start` (`#[...]` or `#![...]`) and the index
/// after it.
fn attribute_at(code: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = code.as_bytes();
    let mut index = start + 1;
    if bytes.get(index) == Some(&b'!') {
        index += 1;
    }
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    let mut depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&code[start..=index], index + 1));
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Whether an attribute makes its item test-only.
fn gates_test_only(attribute: &str) -> bool {
    let compact: String = attribute.chars().filter(|c| !c.is_whitespace()).collect();
    compact == "#[test]"
        || (compact.starts_with("#[") && compact.ends_with("::test]"))
        || (compact.contains("::test(") && !compact.starts_with("#[cfg"))
        || compact == "#[cfg(test)]"
        || compact.starts_with("#[cfg(all(test,")
        || (compact.starts_with("#[cfg(all(") && compact.contains(",test,"))
        || (compact.starts_with("#[cfg(all(") && compact.contains(",test))"))
}

/// The index after the item that starts at or after `start`: past any
/// further attributes, through its matching closing brace, or its `;` when
/// it has no body. A match arm, field, variant or argument (anything that
/// does not start with an item or statement keyword) also ends at its `,`,
/// and any item ends where the construct enclosing it closes, so the code
/// after it is scanned.
fn item_end(bytes: &[u8], start: usize) -> usize {
    let separated = !starts_with_item_keyword(bytes, start);
    let mut index = start;
    let mut depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'{' | b'(' | b'[' => depth += 1,
            closing @ (b'}' | b')' | b']') => {
                if depth == 0 {
                    return index;
                }
                depth -= 1;
                if depth == 0 && closing == b'}' {
                    return index + 1;
                }
            }
            b';' if depth == 0 => return index + 1,
            b',' if depth == 0 && separated => return index + 1,
            _ => {}
        }
        index += 1;
    }
    bytes.len()
}

/// Whether the code at `start`, past whitespace and attributes, starts with
/// an item or statement keyword (whose `,` at the top level, as in generic
/// parameters, does not end it).
fn starts_with_item_keyword(bytes: &[u8], start: usize) -> bool {
    const KEYWORDS: &[&str] = &[
        "async",
        "const",
        "enum",
        "extern",
        "fn",
        "impl",
        "let",
        "macro_rules",
        "mod",
        "pub",
        "static",
        "struct",
        "trait",
        "type",
        "union",
        "unsafe",
        "use",
    ];
    let mut index = start;
    loop {
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if bytes.get(index) != Some(&b'#') {
            break;
        }
        let mut depth = 0usize;
        while let Some(&byte) = bytes.get(index) {
            index += 1;
            match byte {
                b'[' => depth += 1,
                b']' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let word_end = bytes[index.min(bytes.len())..]
        .iter()
        .position(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
        .map_or(bytes.len(), |offset| index + offset);
    let word = std::str::from_utf8(&bytes[index.min(bytes.len())..word_end]).unwrap_or("");
    KEYWORDS.contains(&word)
}

/// `source` with every comment and string or character literal replaced by
/// spaces (newlines kept).
fn blank_comments_and_literals(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    let blank = |out: &mut String, character: char| {
        out.push(if character == '\n' { '\n' } else { ' ' });
    };
    while index < chars.len() {
        let character = chars[index];
        let next = chars.get(index + 1).copied();
        if character == '/' && next == Some('/') {
            while index < chars.len() && chars[index] != '\n' {
                blank(&mut out, chars[index]);
                index += 1;
            }
            continue;
        }
        if character == '/' && next == Some('*') {
            let mut depth = 0usize;
            while index < chars.len() {
                if chars[index] == '/' && chars.get(index + 1) == Some(&'*') {
                    depth += 1;
                    blank(&mut out, chars[index]);
                    blank(&mut out, '*');
                    index += 2;
                    continue;
                }
                if chars[index] == '*' && chars.get(index + 1) == Some(&'/') {
                    depth -= 1;
                    blank(&mut out, '*');
                    blank(&mut out, '/');
                    index += 2;
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                blank(&mut out, chars[index]);
                index += 1;
            }
            continue;
        }
        // Raw strings: r"..", r#".."#, br#".."#.
        let raw_start = match (character, next) {
            ('r', Some('"' | '#')) => Some(index + 1),
            ('b', Some('r')) if matches!(chars.get(index + 2), Some('"' | '#')) => Some(index + 2),
            _ => None,
        };
        if let Some(mut cursor) = raw_start
            && !identifier_continues(&chars, index)
        {
            let mut hashes = 0usize;
            while chars.get(cursor) == Some(&'#') {
                hashes += 1;
                cursor += 1;
            }
            if chars.get(cursor) == Some(&'"') {
                cursor += 1;
                while cursor < chars.len() {
                    if chars[cursor] == '"'
                        && (0..hashes).all(|offset| chars.get(cursor + 1 + offset) == Some(&'#'))
                    {
                        cursor += 1 + hashes;
                        break;
                    }
                    cursor += 1;
                }
                for &skipped in &chars[index..cursor.min(chars.len())] {
                    blank(&mut out, skipped);
                }
                index = cursor;
                continue;
            }
        }
        if character == '"' {
            blank(&mut out, character);
            index += 1;
            while index < chars.len() {
                let current = chars[index];
                blank(&mut out, current);
                index += 1;
                if current == '\\' {
                    if index < chars.len() {
                        blank(&mut out, chars[index]);
                        index += 1;
                    }
                } else if current == '"' {
                    break;
                }
            }
            continue;
        }
        if character == '\'' {
            // A character literal ('x', '\n', '\u{..}'); a lifetime has no
            // closing quote right after one character or escape.
            let end = if next == Some('\\') {
                chars
                    .get(index + 3..)
                    .and_then(|rest| rest.iter().position(|&c| c == '\''))
                    .map(|offset| index + 3 + offset)
            } else if chars.get(index + 2) == Some(&'\'') {
                Some(index + 2)
            } else {
                None
            };
            if let Some(end) = end {
                for &skipped in &chars[index..=end] {
                    blank(&mut out, skipped);
                }
                index = end + 1;
                continue;
            }
        }
        out.push(character);
        index += 1;
    }
    out
}

/// Whether the character before `index` continues an identifier, so an `r`
/// or `b` there is not a literal prefix.
fn identifier_continues(chars: &[char], index: usize) -> bool {
    index > 0 && (chars[index - 1].is_alphanumeric() || chars[index - 1] == '_')
}

/// `text` without whitespace.
fn compact(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

/// Whether `character` continues an identifier.
fn is_identifier_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// The body, between its braces, of the first block that opens at or after
/// `from` in `code` (comments and literals blanked).
fn block_after(code: &str, from: usize) -> Option<&str> {
    let open = from + code[from..].find('{')?;
    let mut depth = 0usize;
    for (offset, byte) in code.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&code[open + 1..open + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

/// One shipped `impl AgentToolDispatcher for ..` block.
#[derive(Debug, PartialEq, Eq)]
struct DispatcherImpl {
    /// The implementing type, without generics or a path.
    type_name: String,
    /// Its `tool_catalog_capabilities` reads another dispatcher's answer.
    takes_capabilities_from_another: bool,
    /// Its `tool_catalog_control_plane` reads another dispatcher's answer.
    forwards_control_plane: bool,
}

/// The body, between its braces, of the method `name` in an impl `block`
/// (comments and literals blanked), if the block defines it.
fn method_body<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("fn {name}");
    let mut from = 0;
    while let Some(offset) = block[from..].find(&needle) {
        let start = from + offset;
        let end = start + needle.len();
        let before = block[..start].chars().next_back();
        let after = block[end..].chars().next();
        if !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char) {
            return block_after(block, end);
        }
        from = end;
    }
    None
}

/// Every `impl .. AgentToolDispatcher for <type>` block in `code` (comments
/// and literals blanked, test-only items removed), with what its two
/// catalog declarations do.
fn dispatcher_impls(code: &str) -> Vec<DispatcherImpl> {
    const TRAIT: &str = "AgentToolDispatcher";
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = code[from..].find(TRAIT) {
        let start = from + offset;
        let end = start + TRAIT.len();
        from = end;
        if code[..start].ends_with(is_identifier_char) {
            continue;
        }
        // `.. AgentToolDispatcher for <type> {`: the trait name, then `for`.
        let rest = code[end..].trim_start();
        let Some(after_for) = rest.strip_prefix("for") else {
            continue;
        };
        if !after_for.starts_with(char::is_whitespace) {
            continue;
        }
        // It is an impl header: the nearest `impl` before it, with no block
        // or statement boundary in between.
        let head = &code[..start];
        let Some(impl_at) = head.rfind("impl") else {
            continue;
        };
        if head[impl_at..].contains(['{', '}', ';']) {
            continue;
        }
        let Some(open) = after_for.find('{') else {
            continue;
        };
        let header_type = after_for[..open].trim();
        let header_type = header_type.split(" where").next().unwrap_or(header_type);
        let header_type = header_type.split('<').next().unwrap_or(header_type).trim();
        let type_name = header_type
            .rsplit("::")
            .next()
            .unwrap_or(header_type)
            .to_string();
        let block_start = code.len() - after_for.len();
        let Some(block) = block_after(code, block_start) else {
            continue;
        };
        let reads_another = |method: &str| {
            method_body(block, method)
                .is_some_and(|body| compact(body).contains(&format!(".{method}(")))
        };
        found.push(DispatcherImpl {
            type_name,
            takes_capabilities_from_another: reads_another("tool_catalog_capabilities"),
            forwards_control_plane: reads_another("tool_catalog_control_plane"),
        });
    }
    found
}

/// The scan on fixtures: a wrapper is a dispatcher whose capabilities read
/// another dispatcher's; a forward is found in either receiver form; a
/// leaf source, a test-only wrapper and a comment are not wrappers.
#[test]
fn the_scan_finds_wrappers_and_their_control_plane_forward() {
    let source = r"
impl AgentToolDispatcher for Leaf {
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities { exact_catalog: true, may_require_catalog_control_plane: true }
    }
}
impl<T: AgentToolDispatcher + ?Sized> meerkat_core::AgentToolDispatcher
    for Forgetful<T>
{
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }
    // fn tool_catalog_control_plane(&self) { self.inner.tool_catalog_control_plane() }
}
impl AgentToolDispatcher for crate::wrappers::Forwarding {
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        let inner = self.primary.tool_catalog_capabilities();
        inner
    }
    fn tool_catalog_control_plane(&self) -> ToolCatalogControlPlane {
        self.primary
            .tool_catalog_control_plane()
            .max(self.fallback.tool_catalog_control_plane())
    }
}
impl AgentToolDispatcher for Literal {
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.0.tool_catalog_capabilities()
    }
    fn tool_catalog_control_plane(&self) -> ToolCatalogControlPlane {
        ToolCatalogControlPlane::Always
    }
}
#[cfg(test)]
mod tests {
    impl AgentToolDispatcher for TestOnly {
        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            self.inner.tool_catalog_capabilities()
        }
    }
}
";
    let found = dispatcher_impls(&shipped_code(source));
    let row = |type_name: &str, takes: bool, forwards: bool| DispatcherImpl {
        type_name: type_name.to_string(),
        takes_capabilities_from_another: takes,
        forwards_control_plane: forwards,
    };
    assert_eq!(
        found,
        vec![
            row("Leaf", false, false),
            row("Forgetful", true, false),
            row("Forwarding", true, true),
            row("Literal", true, false),
        ]
    );
}

#[test]
fn every_connect_dispatcher_wrapper_forwards_the_catalog_control_plane() {
    let mut wrappers: Vec<(String, String, bool)> = Vec::new();
    let mut scanned = 0usize;
    for (relative, path) in shipped_sources() {
        scanned += 1;
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        for dispatcher in dispatcher_impls(&shipped_code(&source)) {
            if dispatcher.takes_capabilities_from_another {
                wrappers.push((
                    relative.clone(),
                    dispatcher.type_name,
                    dispatcher.forwards_control_plane,
                ));
            }
        }
    }
    assert!(scanned > 50, "the sources were not found ({scanned} files)");

    let silent: Vec<_> = wrappers
        .iter()
        .filter(|(_, _, forwards)| !forwards)
        .map(|(file, type_name, _)| (file.as_str(), type_name.as_str()))
        .collect();
    let found: BTreeSet<(&str, &str)> = wrappers
        .iter()
        .map(|(file, type_name, _)| (file.as_str(), type_name.as_str()))
        .collect();
    let listed: BTreeSet<(&str, &str)> = WRAPPERS.iter().copied().collect();
    let unlisted: Vec<_> = found.difference(&listed).collect();
    let gone: Vec<_> = listed.difference(&found).collect();
    assert!(
        silent.is_empty() && unlisted.is_empty() && gone.is_empty(),
        "a tool dispatcher that takes `tool_catalog_capabilities` from another dispatcher \
         answers `tool_catalog_control_plane` from it too (forward the one inner dispatcher's \
         answer, or take the strongest answer of the parts), and WRAPPERS lists it.\n\
         without that forward: {silent:#?}\n\
         found but not listed: {unlisted:#?}\n\
         listed but not found: {gone:#?}"
    );
}
