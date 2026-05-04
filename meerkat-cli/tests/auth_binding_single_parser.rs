#![allow(clippy::manual_assert, clippy::panic)]

//! Tripwire for wave-c (Section 1.5 #4). Flipped green by **C-12**
//! (CLI consolidation to a single `AuthBindingRef`-parsing call site).
//!
//! Invariant: exactly one function in `meerkat-cli/src/**/*.rs` is
//! responsible for parsing user-supplied freeform text into a
//! `AuthBindingRef` (realm / binding / profile triple). The canonical
//! name is `parse_auth_binding_user_input`. Multiple ad-hoc parsers
//! are the very regression C-12 exists to prevent.
//!
//! Scope: we scan only `meerkat-cli/src/**/*.rs` for functions whose
//! name matches the canonical parser. `split_once(':')` elsewhere in
//! the CLI is not a violation — `main.rs:3798` parses a `TokenKey`
//! and `mcp.rs:~193` splits an HTTP header; neither is a
//! `AuthBindingRef` parse. The scope restriction is the whole point of
//! this tripwire (adversarial review flaw 7).
//!
//! Predicted failure today: no function named
//! `parse_auth_binding_user_input` exists; ad-hoc parsing lives
//! inline across CLI handlers. The test asserts exactly one such
//! function exists, so it fails today with `found 0`.

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

#[test]
fn exactly_one_parse_auth_binding_user_input_in_cli_src() {
    let root = workspace_root();
    let cli_src = root.join("meerkat-cli/src");
    assert!(
        cli_src.is_dir(),
        "expected meerkat-cli/src to exist at {}",
        cli_src.display()
    );

    let mut files = Vec::new();
    collect_rs_files(&cli_src, &mut files);

    // Count lines declaring a function named `parse_auth_binding_user_input`.
    // Match both `fn parse_auth_binding_user_input` and
    // `pub(...) fn parse_auth_binding_user_input`.
    let needle = "fn parse_auth_binding_user_input";
    let mut hits: Vec<(PathBuf, usize)> = Vec::new();
    for path in &files {
        let Ok(body) = fs::read_to_string(path) else {
            continue;
        };
        for (lineno, line) in body.lines().enumerate() {
            if line.contains(needle) {
                hits.push((path.clone(), lineno + 1));
            }
        }
    }

    assert_eq!(
        hits.len(),
        1,
        "expected exactly one `fn parse_auth_binding_user_input` in \
         meerkat-cli/src, found {}. The CLI must have a single \
         AuthBindingRef parser at the input boundary (C-12). Hits:\n{}",
        hits.len(),
        hits.iter()
            .map(|(p, ln)| format!("  {}:{}", p.display(), ln))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
