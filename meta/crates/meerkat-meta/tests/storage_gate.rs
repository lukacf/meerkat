//! M5 anti-regression gate for the storage-unification plan.
//!
//! Scans `crates/meerkat-meta/src` production code (skipping `#[cfg(test)]`
//! regions) and fails on:
//!
//! 1. **Ambient root resolution** — `env::var`/`env::var_os` reads of
//!    `HOME`/`XDG_*`/`LOCALAPPDATA`/`TMPDIR`, `std::env::temp_dir()`, and
//!    `dirs::`-style home derivations — anywhere outside
//!    `src/storage_layout.rs`, the single sanctioned path authority
//!    (`default_gateway_home`, `default_ephemeral_scratch_root`).
//! 2. **Duplicate canonical-locator construction** — the known database
//!    file-name literals (`sessions.sqlite3`, `continuity.db`, ...)
//!    appearing in string literals outside `storage_layout.rs`. Feature
//!    crates own *relative* names beneath the layout's roots (the M2
//!    boundary); the canonical top-level spellings are decided exactly
//!    once, in `MetaStorageLayout`.
//!
//! Legitimate uses are allowlisted below, one documented reason per entry.
//! If this gate fails on your change, compose paths through
//! `meerkat_meta::storage_layout::MetaStorageLayout` instead of
//! deriving them locally — or, for a genuinely feature-owned name or a
//! migration/census table, add a narrowly scoped allowlist entry with a
//! reason.
//!
//! This is a text-level scan (no `syn`): it strips comments, tracks string
//! literals, and skips `#[cfg(test)]` items by brace matching. Precise
//! enough for a gate; the allowlist absorbs the rest.

#![allow(clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The single file allowed to resolve ambient roots and to spell the
/// canonical locators.
const SANCTIONED_PATH_AUTHORITY: &str = "storage_layout.rs";

/// Ambient env-var names whose reads constitute root resolution.
const AMBIENT_ROOT_ENV_VARS: [&str; 4] = ["\"HOME\"", "\"XDG_", "\"LOCALAPPDATA\"", "\"TMPDIR\""];

/// Canonical/legacy top-level locator spellings owned by
/// `MetaStorageLayout` (matching inside string literals only). The
/// spellings cover both canonical and legacy forms: `"sessions.sqlite"`
/// also catches `"sessions.sqlite3"`, `"meta_metadata"` catches both
/// metadata spellings, and so on.
const CANONICAL_LOCATOR_NEEDLES: [&str; 13] = [
    "sessions.sqlite",
    "sessions.db",
    "runtime.sqlite",
    "schedule.sqlite",
    "workgraph.sqlite",
    "continuity.sqlite",
    "continuity.db",
    "identity_continuity",
    // The dot keeps SQL *table* names (`meta_metadata`) out of scope:
    // the gate polices file locators, and the `.sqlite` prefix matches the
    // canonical `.sqlite3`.
    "meta_metadata.sqlite",
    "meta_console.sqlite",
    "agent-memory-sqlite",
    "event_log.sqlite",
    "tux-runtimes.json",
];

#[derive(Debug, Clone, Copy)]
struct AllowlistEntry {
    /// Path suffix relative to `src/` (e.g. `"mobpack.rs"`); matches the
    /// whole file when `pattern` is empty.
    file: &'static str,
    /// Substring the offending line must contain (empty = any line).
    pattern: &'static str,
    /// Maximum number of lines this entry may absorb.
    max: usize,
    /// Why this use is legitimate.
    reason: &'static str,
    /// Entries for sibling-owned files may match zero lines without being
    /// reported as stale.
    optional: bool,
}

const fn entry(
    file: &'static str,
    pattern: &'static str,
    max: usize,
    reason: &'static str,
) -> AllowlistEntry {
    AllowlistEntry {
        file,
        pattern,
        max,
        reason,
        optional: false,
    }
}

/// Rule 1 allowlist: ambient root resolution outside the layout.
const AMBIENT_ALLOWLIST: [AllowlistEntry; 3] = [
    entry(
        "storage_layout.rs",
        "",
        usize::MAX,
        "the sanctioned path authority: default_gateway_home ($XDG_STATE_HOME/$HOME) and \
         default_ephemeral_scratch_root (OS temp dir) live here and nowhere else",
    ),
    entry(
        "mobpack.rs",
        "std::env::var(\"HOME\")",
        2,
        "skill-source and user-level ~/.rkat/mcp.toml discovery mirror rkat's home-directory \
         conventions; these locate configuration and skill sources, not storage roots",
    ),
    entry(
        "mobpack.rs",
        "std::env::temp_dir",
        2,
        "caller-overridable scratch outputs (mobpack deploy/validate default output dir); the \
         draft store never uses the temp directory: its default routes through \
         storage_layout::default_gateway_home and refuses without a state root",
    ),
];

/// Rule 2 allowlist: canonical locator literals outside the layout.
const LOCATOR_ALLOWLIST: [AllowlistEntry; 7] = [
    entry(
        "storage_layout.rs",
        "",
        usize::MAX,
        "the layout owns the canonical spellings and the legacy probe lists",
    ),
    entry(
        "schedule_wiring.rs",
        "pub const SCHEDULE_STORE_FILE",
        1,
        "feature-owned canonical constant; the layout composes it (DatabaseSlot::Schedule)",
    ),
    entry(
        "workgraph_wiring.rs",
        "pub const WORKGRAPH_STORE_FILE",
        1,
        "feature-owned canonical constant; the layout composes it (DatabaseSlot::Workgraph)",
    ),
    entry(
        "bin/meta_gateway.rs",
        "runtime.sqlite",
        2,
        "the native migration command's resume and restore texts name the runtime database \
         the operator acts on; they compose no path",
    ),
    entry(
        "storage_migrate_native/census.rs",
        "runtime.sqlite",
        13,
        "the native migration census's refusal and remedy texts name the runtime database \
         the operator converts or restores; they compose no path",
    ),
    entry(
        "storage_migrate_native/export_census.rs",
        "runtime.sqlite",
        2,
        "the continuity export census's texts name the runtime database a directory holds; \
         they compose no path",
    ),
    AllowlistEntry {
        file: "storage_doctor.rs",
        pattern: "",
        max: usize::MAX,
        reason: "the doctor's legacy-spelling census enumerates every historical spelling by \
                 design (M1). TODO(storage-M6): once the migration verb lands, fold the \
                 census spellings onto DatabaseSlot::legacy_names so the doctor and the \
                 layout cannot drift",
        optional: true,
    },
];

/// One matched line.
struct Finding {
    file: PathBuf,
    line: usize,
    text: String,
    what: String,
}

/// Per-line scan output: comment-free text plus, per character, whether it
/// sits inside a string literal; and the brace delta outside strings.
struct LineFacts {
    visible: String,
    in_string: Vec<bool>,
    brace_delta: i64,
}

#[derive(Default)]
struct ScanState {
    in_block_comment: bool,
}

fn analyze_line(line: &str, state: &mut ScanState) -> LineFacts {
    let mut visible = String::with_capacity(line.len());
    let mut in_string_flags = Vec::with_capacity(line.len());
    let mut brace_delta = 0i64;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    let mut in_string = false;
    let mut string_is_raw = false;
    let mut raw_hashes = 0usize;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if state.in_block_comment {
            if c == '*' && next == Some('/') {
                state.in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if !in_string {
            if c == '/' && next == Some('/') {
                break; // line comment: rest of line invisible
            }
            if c == '/' && next == Some('*') {
                state.in_block_comment = true;
                i += 2;
                continue;
            }
            // Raw string start: r"..." or r#"..."# (any hash count).
            if c == 'r' && matches!(next, Some('"' | '#')) {
                let mut j = i + 1;
                let mut hashes = 0usize;
                while chars.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if chars.get(j) == Some(&'"') {
                    visible.push(c);
                    visible.extend(std::iter::repeat_n('#', hashes));
                    visible.push('"');
                    in_string_flags.extend(std::iter::repeat_n(false, hashes + 2));
                    in_string = true;
                    string_is_raw = true;
                    raw_hashes = hashes;
                    i = j + 1;
                    continue;
                }
            }
            if c == '"' {
                in_string = true;
                string_is_raw = false;
                visible.push(c);
                in_string_flags.push(false);
                i += 1;
                continue;
            }
            if c == '{' {
                brace_delta += 1;
            }
            if c == '}' {
                brace_delta -= 1;
            }
            visible.push(c);
            in_string_flags.push(false);
            i += 1;
            continue;
        }
        // Inside a string literal.
        if string_is_raw {
            if c == '"' {
                let mut j = i + 1;
                let mut hashes = 0usize;
                while hashes < raw_hashes && chars.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if hashes == raw_hashes {
                    visible.push('"');
                    visible.extend(std::iter::repeat_n('#', hashes));
                    in_string_flags.extend(std::iter::repeat_n(false, hashes + 1));
                    in_string = false;
                    i = j;
                    continue;
                }
            }
            visible.push(c);
            in_string_flags.push(true);
            i += 1;
            continue;
        }
        if c == '\\' {
            // Escape: keep both chars flagged as string content.
            visible.push(c);
            in_string_flags.push(true);
            if let Some(n) = next {
                visible.push(n);
                in_string_flags.push(true);
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if c == '"' {
            visible.push(c);
            in_string_flags.push(false);
            in_string = false;
            i += 1;
            continue;
        }
        visible.push(c);
        in_string_flags.push(true);
        i += 1;
    }
    LineFacts {
        visible,
        in_string: in_string_flags,
        brace_delta,
    }
}

/// True if `needle` occurs in `facts.visible` entirely inside a string
/// literal.
fn needle_in_string(facts: &LineFacts, needle: &str) -> bool {
    let bytes: Vec<char> = facts.visible.chars().collect();
    let needle_chars: Vec<char> = needle.chars().collect();
    if needle_chars.is_empty() || bytes.len() < needle_chars.len() {
        return false;
    }
    for start in 0..=(bytes.len() - needle_chars.len()) {
        if bytes[start..start + needle_chars.len()] == needle_chars[..]
            && facts.in_string[start..start + needle_chars.len()]
                .iter()
                .all(|flag| *flag)
        {
            return true;
        }
    }
    false
}

/// Substring match that refuses an identifier character immediately before
/// the needle (so `dirs::` does not match `skill_dirs::`).
fn word_boundary_contains(text: &str, needle: &str) -> bool {
    let mut search_from = 0usize;
    while let Some(pos) = text[search_from..].find(needle) {
        let absolute = search_from + pos;
        let boundary = text[..absolute]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if boundary {
            return true;
        }
        search_from = absolute + needle.len();
    }
    false
}

/// Collect every production (non-`#[cfg(test)]`) line of a file.
fn production_lines(source: &str) -> Vec<(usize, LineFacts)> {
    let mut out = Vec::new();
    let mut state = ScanState::default();
    let mut pending_cfg_test = false;
    let mut skip_depth: Option<i64> = None;
    for (index, raw_line) in source.lines().enumerate() {
        let facts = analyze_line(raw_line, &mut state);
        let trimmed = facts.visible.trim().to_string();
        if let Some(depth) = skip_depth.as_mut() {
            *depth += facts.brace_delta;
            if *depth <= 0 {
                skip_depth = None;
            }
            continue;
        }
        if trimmed.starts_with("#[cfg(test)]") {
            pending_cfg_test = true;
            if facts.brace_delta > 0 {
                skip_depth = Some(facts.brace_delta);
                pending_cfg_test = false;
            }
            continue;
        }
        if pending_cfg_test {
            if trimmed.starts_with("#[") {
                continue; // further attributes on the test item
            }
            if facts.brace_delta > 0 {
                skip_depth = Some(facts.brace_delta);
                pending_cfg_test = false;
                continue;
            }
            // Single-line test item (e.g. `#[cfg(test)] use ...;`).
            pending_cfg_test = false;
            continue;
        }
        out.push((index + 1, facts));
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).expect("read src dir");
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let file_type = entry.file_type().expect("file type");
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    out.sort();
}

fn relative_to_src(path: &Path, src_root: &Path) -> String {
    path.strip_prefix(src_root)
        .expect("path under src")
        .to_string_lossy()
        .into_owned()
}

fn consume_allowlisted(
    allowlist: &[AllowlistEntry],
    used: &mut [usize],
    relative: &str,
    line_text: &str,
) -> bool {
    for (index, entry) in allowlist.iter().enumerate() {
        let file_matches = relative == entry.file || relative.ends_with(entry.file);
        if !file_matches {
            continue;
        }
        if !entry.pattern.is_empty() && !line_text.contains(entry.pattern) {
            continue;
        }
        if used[index] >= entry.max {
            continue;
        }
        used[index] += 1;
        return true;
    }
    false
}

#[test]
fn storage_gate_no_ambient_roots_and_no_duplicate_canonical_locators() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // Sanity: the sanctioned authority still exists and still owns the two
    // ambient derivations the allowlist assumes.
    let layout_source = std::fs::read_to_string(src_root.join(SANCTIONED_PATH_AUTHORITY))
        .expect("storage_layout.rs is the sanctioned path authority; it must exist");
    assert!(
        layout_source.contains("pub fn default_gateway_home")
            && layout_source.contains("pub fn default_ephemeral_scratch_root"),
        "storage_layout.rs no longer defines the sanctioned ambient derivations; \
         update the M5 gate's allowlist to follow them"
    );

    let mut files = Vec::new();
    rust_files(&src_root, &mut files);
    assert!(
        files.len() > 20,
        "suspiciously few source files found; is the walk broken?"
    );

    let mut ambient_used = vec![0usize; AMBIENT_ALLOWLIST.len()];
    let mut locator_used = vec![0usize; LOCATOR_ALLOWLIST.len()];
    let mut findings: Vec<Finding> = Vec::new();

    for path in &files {
        let relative = relative_to_src(path, &src_root);
        let source = std::fs::read_to_string(path).expect("read source file");
        for (line_number, facts) in production_lines(&source) {
            let text = &facts.visible;

            // Rule 1: ambient root resolution.
            let env_read = (text.contains("env::var") || text.contains("env!("))
                && AMBIENT_ROOT_ENV_VARS.iter().any(|var| text.contains(var));
            let temp_dir_read = text.contains("env::temp_dir");
            let dirs_read =
                word_boundary_contains(text, "dirs::") || word_boundary_contains(text, "home_dir(");
            if env_read || temp_dir_read || dirs_read {
                if !consume_allowlisted(&AMBIENT_ALLOWLIST, &mut ambient_used, &relative, text) {
                    findings.push(Finding {
                        file: path.clone(),
                        line: line_number,
                        text: text.trim().to_string(),
                        what: "ambient root resolution (HOME/XDG_*/LOCALAPPDATA/TMPDIR/temp_dir/\
                               dirs)"
                            .to_string(),
                    });
                }
                continue;
            }

            // Rule 2: canonical locator literals in strings.
            if let Some(needle) = CANONICAL_LOCATOR_NEEDLES
                .iter()
                .find(|needle| needle_in_string(&facts, needle))
                && !consume_allowlisted(&LOCATOR_ALLOWLIST, &mut locator_used, &relative, text)
            {
                findings.push(Finding {
                    file: path.clone(),
                    line: line_number,
                    text: text.trim().to_string(),
                    what: format!("canonical locator literal \"{needle}\""),
                });
            }
        }
    }

    // Stale allowlist entries are failures too (except sibling-owned
    // optional entries): a rule that matches nothing should be deleted.
    let mut stale = Vec::new();
    for (list, used) in [
        (&AMBIENT_ALLOWLIST[..], &ambient_used),
        (&LOCATOR_ALLOWLIST[..], &locator_used),
    ] {
        for (entry, count) in list.iter().zip(used.iter()) {
            if *count == 0 && !entry.optional && !entry.pattern.is_empty() {
                stale.push(format!(
                    "{} :: {} (was allowed because: {})",
                    entry.file, entry.pattern, entry.reason
                ));
            }
        }
    }

    if !findings.is_empty() || !stale.is_empty() {
        let mut message = String::new();
        let _ = writeln!(
            message,
            "M5 storage gate failed. Path roots and canonical database locators are owned by \
             meerkat_meta::storage_layout::MetaStorageLayout \
             (src/storage_layout.rs); compose paths through the layout instead of deriving \
             them at the call site.\n"
        );
        for finding in &findings {
            let _ = writeln!(
                message,
                "  {}:{}: {}\n      {}",
                finding.file.display(),
                finding.line,
                finding.what,
                finding.text
            );
        }
        for entry in &stale {
            let _ = writeln!(
                message,
                "  stale allowlist entry (matched nothing — delete it): {entry}"
            );
        }
        panic!("{message}");
    }
}
