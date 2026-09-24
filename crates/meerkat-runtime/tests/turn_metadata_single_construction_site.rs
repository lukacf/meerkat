#![allow(clippy::panic)]

//! B-6: enforce that every `RuntimeTurnMetadata` construction inside
//! `meerkat-runtime/src/` routes through the canonical
//! `runtime_loop::for_input` constructor.
//!
//! Grep-based so it stays cheap and runs on every test lane.
//! Struct-literal and `::default()` construction sites are forbidden outside
//! `runtime_loop.rs` — any new caller must either call `for_input` or extend
//! it.

use std::fs;
use std::path::{Path, PathBuf};

fn src_root() -> PathBuf {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_root.join("src")
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, files);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn line_is_test_or_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//")
        || trimmed.starts_with("#[cfg(test)]")
        || trimmed.starts_with("#[test]")
}

#[test]
fn runtime_turn_metadata_has_single_construction_site() {
    let root = src_root();
    let canonical_rel = Path::new("runtime_loop.rs");

    let mut files = Vec::new();
    walk(&root, &mut files);
    assert!(!files.is_empty(), "no .rs files found under {root:?}");

    let needles = ["RuntimeTurnMetadata::default()", "RuntimeTurnMetadata {"];

    let mut offenders: Vec<(PathBuf, usize, String)> = Vec::new();
    for file in &files {
        let file_name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        // Only `runtime_loop.rs` is allowed to construct RuntimeTurnMetadata.
        let is_canonical = file_name == canonical_rel.to_string_lossy();
        if is_canonical {
            continue;
        }
        // Files with `_tests.rs` suffix are only compiled under #[cfg(test)]
        // via an #[path] include; those test fixtures are permitted to
        // construct RuntimeTurnMetadata literals directly.
        if file_name.ends_with("_tests.rs") {
            continue;
        }
        let Ok(contents) = fs::read_to_string(file) else {
            continue;
        };
        let mut cfg_test_pending = false;
        let mut cfg_test_depth: i32 = 0;
        let mut brace_depth: i32 = 0;
        for (lineno, line) in contents.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[cfg(any(test,") {
                cfg_test_pending = true;
            }
            // Track brace depth so we know when we exit the cfg(test) item.
            let opens = line.matches('{').count() as i32;
            let closes = line.matches('}').count() as i32;
            if cfg_test_pending && opens > 0 {
                cfg_test_depth = brace_depth + opens - closes;
                cfg_test_pending = false;
            }
            let in_cfg_test = cfg_test_depth > 0 && brace_depth >= cfg_test_depth;
            brace_depth += opens - closes;
            if cfg_test_depth > 0 && brace_depth < cfg_test_depth {
                cfg_test_depth = 0;
            }

            if in_cfg_test || line_is_test_or_comment(line) {
                continue;
            }
            for needle in &needles {
                if line.contains(needle) {
                    offenders.push((file.clone(), lineno + 1, line.to_string()));
                }
            }
        }
    }

    if !offenders.is_empty() {
        let rendered = offenders
            .into_iter()
            .map(|(path, lineno, line)| format!("  {}:{}  {}", path.display(), lineno, line.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "RuntimeTurnMetadata must only be constructed via `runtime_loop::for_input`. \
             Found forbidden construction sites:\n{rendered}"
        );
    }
}
