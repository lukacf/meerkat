//! The no-wait rule at this crate's own waiting sites.
//!
//! Meta and Core call into this crate from inside their protected sections
//! (`meerkat_core::protected_section`): the governed bind, the native
//! admission readers, the historical admission callback, the store
//! transaction. Code there never takes a lock another actor may hold across
//! I/O, never spins and never sleeps. Each such site in this crate names
//! itself here before it may wait: inside a protected section that is a
//! violation, which a debug build reports.
//!
//! [`KNOWN_SITES`] lists the sites that still wait inside a protected
//! section, each with the change that removes it. A listed site is not
//! reported. The list only shrinks: the source guard in this module's tests
//! fails for a listed site that no longer exists, and for a site that names
//! itself without being listed while a row reaches it inside a section.

/// Sites that still wait inside a protected section, with the change that
/// removes each (`site`, `owner`).
pub(crate) const KNOWN_SITES: &[(&str, &str)] = &[];

fn known(site: &str) -> bool {
    KNOWN_SITES.iter().any(|(known, _)| *known == site)
}

/// About to take a lock another actor may hold across I/O (or to spin or
/// sleep) at `site`.
#[track_caller]
pub(crate) fn may_wait(site: &'static str) {
    if !known(site) {
        meerkat_core::protected_section::assert_may_wait(site);
    }
}

/// Run `wait`, which takes the shared publication writer (or another owner's
/// waiting lock) at `site`. A listed site runs as a known wait.
pub(crate) fn wait_at<T>(site: &'static str, wait: impl FnOnce() -> T) -> T {
    if known(site) {
        meerkat_core::protected_section::known_wait(site, wait)
    } else {
        wait()
    }
}

/// Waiting calls the source guard knows inside the protected impls
/// (`file`, `type`, `call`, `owner`): each still waits and names the change
/// that removes it.
#[cfg(test)]
const KNOWN_CALLS: &[(&str, &str, &str, &str)] = &[];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// The traits whose methods Meta or Core call inside a protected
    /// section.
    const PROTECTED_TRAITS: &[&str] = &[
        "GovernedInputBinder",
        "GovernedEntryFence",
        "GovernedEntryPermit",
        "GovernedSettlement",
        "PeerIngressBinder",
        "AuthenticatedOriginalSource",
        "NativeInputAdmissionCustody",
    ];

    /// Protected traits whose implementing type is a whole source owner, not
    /// a decision: only the trait impl's own body is scanned, never the
    /// owner's other impls (its writers run outside every section, and the
    /// runtime witness covers what the trait methods reach).
    const OWNER_TRAITS: &[&str] = &["AuthenticatedOriginalSource"];

    /// Calls that can wait for another actor: a blocking lock or writer, a
    /// spin, a sleep, an awaited writer or wait helper. Each is matched at a
    /// call boundary, so its non-waiting `try_` form never matches.
    const WAITING_CALLS: &[&str] = &[
        "begin_owner_change(",
        "wait_for_lock(",
        "enroll(",
        "source_writer(",
        "bounded_source_writer(",
        "check_admitted_helper_until(",
        "wait_out_held(",
        "wait_for_control(",
        "wait_until_free(",
        "block_on(",
        "blocking_lock(",
        "blocking_recv(",
        "blocking_send(",
        "thread::sleep(",
        "time::sleep(",
    ];

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if path.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    name != "tests" && !name.ends_with("_tests")
                }) {
                    rust_files(&path, out);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs") && !is_test_file(&path) {
                out.push(path);
            }
        }
    }

    /// Test modules and fixtures, which may wait on purpose.
    fn is_test_file(path: &Path) -> bool {
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        name == "tests"
            || name.ends_with("_tests")
            || name.contains("test_support")
            || name.contains("test_bridge")
            || name.ends_with("_fixture")
    }

    /// `text` with every comment and string or char literal blanked to
    /// spaces (newlines kept), so braces and calls inside them are ignored.
    fn blank(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        let keep = |c: char| if c == '\n' { '\n' } else { ' ' };
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if c == '/' && next == Some('/') {
                while i < chars.len() && chars[i] != '\n' {
                    out.push(' ');
                    i += 1;
                }
            } else if c == '/' && next == Some('*') {
                let mut depth = 0usize;
                while i < chars.len() {
                    if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                        depth += 1;
                        out.push_str("  ");
                        i += 2;
                    } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        out.push_str("  ");
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        out.push(keep(chars[i]));
                        i += 1;
                    }
                }
            } else if (c == 'r' || c == 'b')
                && !chars
                    .get(i.wrapping_sub(1))
                    .is_some_and(|p| p.is_alphanumeric() || *p == '_')
                && {
                    let mut j = i + 1;
                    if c == 'b' && chars.get(j) == Some(&'r') {
                        j += 1;
                    }
                    while chars.get(j) == Some(&'#') {
                        j += 1;
                    }
                    chars.get(j) == Some(&'"')
                        && (c == 'r' || j > i + 1 || chars.get(i + 1) == Some(&'"'))
                }
            {
                // A raw or byte string: r"..", r#".."#, br#".."#, b"..".
                let mut j = i + 1;
                if c == 'b' && chars.get(j) == Some(&'r') {
                    j += 1;
                }
                let raw = c == 'r' || (c == 'b' && chars.get(i + 1) == Some(&'r'));
                let mut hashes = 0;
                while chars.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                for _ in i..=j {
                    out.push(' ');
                }
                i = j + 1;
                while i < chars.len() {
                    if !raw && chars[i] == '\\' {
                        out.push(' ');
                        out.push(keep(chars.get(i + 1).copied().unwrap_or(' ')));
                        i += 2;
                        continue;
                    }
                    if chars[i] == '"' && (0..hashes).all(|k| chars.get(i + 1 + k) == Some(&'#')) {
                        for _ in 0..=hashes {
                            out.push(' ');
                        }
                        i += 1 + hashes;
                        break;
                    }
                    out.push(keep(chars[i]));
                    i += 1;
                }
            } else if c == '"' {
                out.push(' ');
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\\' {
                        out.push(' ');
                        out.push(keep(chars.get(i + 1).copied().unwrap_or(' ')));
                        i += 2;
                        continue;
                    }
                    if chars[i] == '"' {
                        out.push(' ');
                        i += 1;
                        break;
                    }
                    out.push(keep(chars[i]));
                    i += 1;
                }
            } else if c == '\'' {
                // A char literal ('x', '\n', '\'', '\u{..}'); a lifetime is
                // kept. An escape's own character is never its close.
                let close = if next == Some('\\') {
                    (i + 3..chars.len().min(i + 12)).find(|&k| chars[k] == '\'')
                } else if chars.get(i + 2) == Some(&'\'') {
                    Some(i + 2)
                } else {
                    None
                };
                match close {
                    Some(end) => {
                        for _ in i..=end {
                            out.push(' ');
                        }
                        i = end + 1;
                    }
                    None => {
                        out.push(c);
                        i += 1;
                    }
                }
            } else {
                out.push(c);
                i += 1;
            }
        }
        out
    }

    /// One `impl` block: its header (before `{`) and body byte range.
    struct ImplBlock {
        header: String,
        body: std::ops::Range<usize>,
    }

    fn impl_blocks(code: &str) -> Vec<ImplBlock> {
        let bytes = code.as_bytes();
        let mut blocks = Vec::new();
        let mut at = 0;
        while let Some(found) = code[at..].find("impl") {
            let start = at + found;
            at = start + 4;
            let before = start.checked_sub(1).map(|k| bytes[k]);
            let after = bytes.get(start + 4).copied();
            let boundary =
                |b: Option<u8>| b.is_none_or(|b| !(b.is_ascii_alphanumeric() || b == b'_'));
            if !boundary(before) || !boundary(after) {
                continue;
            }
            let Some(open) = code[start..].find(['{', ';']).map(|k| start + k) else {
                continue;
            };
            if bytes[open] == b';' {
                continue;
            }
            let mut depth = 0usize;
            let mut end = open;
            for (k, b) in bytes.iter().enumerate().skip(open) {
                match b {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = k;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            blocks.push(ImplBlock {
                header: code[start + 4..open]
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                body: open..end,
            });
        }
        blocks
    }

    /// The last path segment of a type or trait, without generics.
    fn last_segment(path: &str) -> String {
        let path = path.split('<').next().unwrap_or(path);
        let path = path.split(" where ").next().unwrap_or(path);
        path.rsplit("::").next().unwrap_or(path).trim().to_string()
    }

    /// The header's trait and type: `impl<..> Trait for Type` or `impl Type`.
    fn header_names(header: &str) -> (Option<String>, String) {
        let header = header.trim();
        let header = if header.starts_with('<') {
            let mut depth = 0usize;
            let mut cut = header.len();
            for (k, c) in header.char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            cut = k + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            header[cut..].trim()
        } else {
            header
        };
        match header.split_once(" for ") {
            Some((trait_path, ty)) => (Some(last_segment(trait_path)), last_segment(ty)),
            None => (None, last_segment(header)),
        }
    }

    fn calls_in(code: &str, body: &std::ops::Range<usize>, call: &str) -> Vec<usize> {
        let bytes = code.as_bytes();
        let mut found = Vec::new();
        let mut at = body.start;
        while let Some(k) = code[at..body.end].find(call) {
            let start = at + k;
            at = start + call.len();
            let before = start.checked_sub(1).map(|p| bytes[p]);
            if before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
                continue;
            }
            // A definition of a method by that name is not a call.
            if code[..start].trim_end().ends_with("fn") {
                continue;
            }
            found.push(start);
        }
        found
    }

    /// Every waiting call inside a protected impl, and inside the inherent
    /// impls of a type that implements a protected decision trait in the
    /// same file: (`file`, `type`, `call`, `line`).
    fn waiting_calls() -> Vec<(String, String, String, usize)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        rust_files(&root.join("src"), &mut files);
        let mut hits = Vec::new();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let code = blank(&text);
            let blocks = impl_blocks(&code);
            let deciders: BTreeSet<String> = blocks
                .iter()
                .filter_map(|block| match header_names(&block.header) {
                    (Some(trait_name), ty)
                        if PROTECTED_TRAITS.contains(&trait_name.as_str())
                            && !OWNER_TRAITS.contains(&trait_name.as_str()) =>
                    {
                        Some(ty)
                    }
                    _ => None,
                })
                .collect();
            let file = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            for block in &blocks {
                let (trait_name, ty) = header_names(&block.header);
                let scanned = match &trait_name {
                    Some(trait_name) => PROTECTED_TRAITS.contains(&trait_name.as_str()),
                    None => deciders.contains(&ty),
                };
                if !scanned {
                    continue;
                }
                for call in WAITING_CALLS {
                    for offset in calls_in(&code, &block.body, call) {
                        let line = code[..offset].matches('\n').count() + 1;
                        hits.push((file.clone(), ty.clone(), (*call).to_string(), line));
                    }
                }
            }
        }
        hits
    }

    /// The source guard: no waiting call inside a protected impl except a
    /// known one, and no known entry that no longer exists.
    #[test]
    fn protected_impls_make_no_unknown_waiting_call() {
        let hits = waiting_calls();
        let unknown: Vec<_> = hits
            .iter()
            .filter(|(file, ty, call, _)| {
                !super::KNOWN_CALLS
                    .iter()
                    .any(|(f, t, c, _)| f == file && t == ty && c == call)
            })
            .collect();
        assert!(
            unknown.is_empty(),
            "waiting calls inside protected impls (file, type, call, line): {unknown:#?}"
        );
        let stale: Vec<_> = super::KNOWN_CALLS
            .iter()
            .filter(|(f, t, c, _)| {
                !hits
                    .iter()
                    .any(|(file, ty, call, _)| f == file && t == ty && c == call)
            })
            .collect();
        assert!(
            stale.is_empty(),
            "known waiting calls that no longer exist: {stale:#?}"
        );
    }

    /// Every known runtime site still names itself in the source.
    #[test]
    fn every_known_site_still_names_itself() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        rust_files(&root.join("src"), &mut files);
        let sources: String = files
            .iter()
            .filter(|path| !path.ends_with("no_wait.rs"))
            .filter_map(|path| std::fs::read_to_string(path).ok())
            .collect();
        let stale: Vec<_> = super::KNOWN_SITES
            .iter()
            .filter(|(site, _)| !sources.contains(&format!("\"{site}\"")))
            .collect();
        assert!(
            stale.is_empty(),
            "known sites that no longer exist: {stale:#?}"
        );
    }

    /// The lexer blanks comments and literals and keeps lifetimes.
    #[test]
    fn the_guard_lexer_ignores_comments_and_literals() {
        let code = blank(
            "fn f<'a>() { let _ = \"enroll(\"; // enroll(\n /* wait_for_lock( */ x.enroll(); \
             let c = '{'; let q = '\\''; let r = r#\"block_on(\"#; }",
        );
        assert_eq!(code.matches("enroll(").count(), 1);
        assert!(code.contains("<'a>"));
        assert!(!code.contains("'{'"));
        assert!(!code.contains("block_on("));
        assert_eq!(code.matches('{').count(), code.matches('}').count());
        let blocks = impl_blocks(
            "impl<T> crate::a::GovernedInputBinder for Wrapper<T> { fn enroll(&self) {} fn b() { x.enroll(); } }",
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            header_names(&blocks[0].header),
            (
                Some("GovernedInputBinder".to_string()),
                "Wrapper".to_string()
            )
        );
        let code = "impl<T> crate::a::GovernedInputBinder for Wrapper<T> { fn enroll(&self) {} fn b() { x.enroll(); } }";
        assert_eq!(calls_in(code, &blocks[0].body, "enroll(").len(), 1);
    }
}
