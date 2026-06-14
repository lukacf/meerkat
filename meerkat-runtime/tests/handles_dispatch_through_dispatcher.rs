#![allow(clippy::manual_assert, clippy::panic)]

//! Tripwire for wave-c (Section 1.5 #5). Flipped green by **C-6c**
//! (composition-dispatcher consumer — the meerkat-runtime handle
//! layer).
//!
//! Invariant: every routed-effect-consuming `apply_input(` call site
//! in `meerkat-runtime/src/handles/**/*.rs` must traverse the
//! `CompositionDispatcher` — i.e. it is lexically inside a function
//! whose body references a `dispatcher` binding or
//! `CompositionDispatcher`. B-5 landed the behavioural canary; this
//! tripwire extends it to the handle layer, which still has 87 direct
//! `apply_input` call sites bypassing the dispatcher per the C-6c
//! audit.
//!
//! Predicted failure today: 87 (or more) call sites fail the
//! "dispatcher in scope" check — so the test fails with a long list
//! of violators. After C-6c lands, the list should be empty.
//!
//! Method: scan each `handles/*.rs` file, find `apply_input(` lines,
//! walk backwards to the enclosing `fn` header, scan that function's
//! body for `dispatcher` or `CompositionDispatcher`. Report any
//! function whose body lacks both.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let toml = p.join("Cargo.toml");
        if toml.exists() {
            let text = fs::read_to_string(&toml).unwrap_or_default();
            if text.contains("[workspace]") {
                return p;
            }
        }
        if !p.pop() {
            panic!("could not locate workspace root");
        }
    }
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Walk backwards from `line_idx` to the nearest line starting a
/// function declaration. Returns the index of the `fn ` line, or the
/// file start.
fn find_enclosing_fn_start(lines: &[&str], line_idx: usize) -> usize {
    for i in (0..=line_idx).rev() {
        let trimmed = lines[i].trim_start();
        // A function header: `fn NAME`, `async fn NAME`, `pub fn`,
        // `pub(crate) fn`, etc.
        if trimmed.starts_with("fn ")
            || trimmed.starts_with("async fn ")
            || trimmed.starts_with("pub fn ")
            || trimmed.starts_with("pub(crate) fn ")
            || trimmed.starts_with("pub(super) fn ")
            || trimmed.starts_with("pub async fn ")
            || trimmed.starts_with("pub(crate) async fn ")
        {
            return i;
        }
    }
    0
}

/// True if the function starting at `fn_start` is a unit test
/// (`#[test]` / `#[tokio::test]`). The dispatcher-routing invariant governs
/// *production* handle call sites; unit tests legitimately drive
/// `apply_input` directly to exercise the handle authority in isolation, so
/// they are excluded from the audit. Checks only the attribute lines
/// immediately preceding the `fn` header, so production call sites are never
/// false-skipped.
fn is_test_fn(lines: &[&str], fn_start: usize) -> bool {
    for i in (fn_start.saturating_sub(4)..fn_start).rev() {
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("#[test]") || trimmed.starts_with("#[tokio::test]") {
            return true;
        }
        // Stop scanning once we pass the contiguous attribute/doc block.
        if !trimmed.starts_with('#') && !trimmed.starts_with("//") && !trimmed.is_empty() {
            break;
        }
    }
    false
}

/// Approximate the function body end by scanning forward for the
/// closing brace at the original indent level. Cheap and good enough
/// for the dispatcher-in-scope check.
fn find_fn_end(lines: &[&str], fn_start: usize) -> usize {
    let mut depth: i32 = 0;
    let mut seen_open = false;
    for (idx, line) in lines.iter().enumerate().skip(fn_start) {
        for ch in line.chars() {
            if ch == '{' {
                depth += 1;
                seen_open = true;
            } else if ch == '}' {
                depth -= 1;
                if seen_open && depth == 0 {
                    return idx;
                }
            }
        }
    }
    lines.len().saturating_sub(1)
}

#[test]
fn every_apply_input_in_handles_traverses_dispatcher() {
    let root = workspace_root();
    let handles_dir = root.join("meerkat-runtime/src/handles");
    assert!(
        handles_dir.is_dir(),
        "expected meerkat-runtime/src/handles to exist at {}",
        handles_dir.display()
    );

    let mut files = Vec::new();
    collect_rs_files(&handles_dir, &mut files);

    let mut violations: Vec<String> = Vec::new();

    for path in &files {
        let Ok(body) = fs::read_to_string(path) else {
            continue;
        };
        let lines: Vec<&str> = body.lines().collect();
        for (lineno, line) in lines.iter().enumerate() {
            if !line.contains("apply_input(") {
                continue;
            }
            // Skip comments and obvious doc-lines.
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("//!")
            {
                continue;
            }

            let fn_start = find_enclosing_fn_start(&lines, lineno);
            // Unit tests legitimately call `apply_input` directly; the
            // dispatcher-routing invariant is a production-code contract.
            if is_test_fn(&lines, fn_start) {
                continue;
            }
            let fn_end = find_fn_end(&lines, fn_start);
            let fn_body = lines[fn_start..=fn_end].join("\n");

            let in_scope =
                fn_body.contains("dispatcher") || fn_body.contains("CompositionDispatcher");
            if !in_scope {
                violations.push(format!(
                    "{}:{} — `apply_input(` call site not reached via \
                     `dispatcher` / `CompositionDispatcher` \
                     (enclosing fn starts line {})",
                    path.strip_prefix(&root).unwrap_or(path).display(),
                    lineno + 1,
                    fn_start + 1,
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "expected zero direct `apply_input(` call sites in \
         meerkat-runtime/src/handles/ to bypass the \
         CompositionDispatcher; found {}. Flipped green by C-6c.\n{}",
        violations.len(),
        violations.join("\n")
    );
}
