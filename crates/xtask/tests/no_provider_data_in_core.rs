//! Recurrence pin: meerkat-core must contain ZERO provider-specific model
//! data.
//!
//! The B2 split (2026-04-18) wrongly folded the model catalog back into
//! `meerkat_core::model_profile`, reverting the 0.5x core/data separation.
//! The 0.7.1 extraction re-established the invariant: core owns the
//! vocabulary types and `ModelCatalog` mechanics; the `meerkat-models` crate
//! owns the canonical data and injects it as an explicit parameter
//! (dependency direction is strictly `meerkat-models -> meerkat-core`).
//!
//! These tests pin both halves of that invariant so the separation cannot be
//! silently reverted a second time.

#![allow(clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read directory {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("cannot read entry in {}: {e}", dir.display()));
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// (a) meerkat-core must never depend on meerkat-models. The dependency
/// direction is models -> core; the reverse would let provider data leak
/// back into the core contract surface.
#[test]
fn core_does_not_depend_on_meerkat_models() {
    let manifest_path = repo_root().join("meerkat-core/Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", manifest_path.display()));
    assert!(
        !manifest.contains("meerkat-models"),
        "meerkat-core/Cargo.toml references meerkat-models.\n\
         \n\
         PRINCIPLE: meerkat-core owns the model-catalog vocabulary and the\n\
         ModelCatalog mechanics, never the provider data. The data lives in\n\
         meerkat-models, which depends on meerkat-core — the reverse edge is\n\
         forbidden. Inject catalog data as an explicit `ModelCatalog`\n\
         parameter (canonically `meerkat_models::canonical()`) instead of\n\
         depending on the data crate from core."
    );
}

/// (b) No file under meerkat-core/src may contain a provider model-name
/// literal (`"gpt-…"`, `"claude-…"`, `"gemini-…"`). This is a
/// tombstone-class textual ban: the presence of such a literal anywhere in
/// core IS the violation, regardless of context — real model names are
/// provider data and belong in meerkat-models (tests included; core tests
/// use the synthetic `model_profile::test_catalog` fixture).
#[test]
fn core_sources_contain_no_provider_model_literals() {
    // `"(gpt|claude|gemini)-[0-9]` without regex dependencies: a literal
    // opening quote, a provider prefix, a dash, then an ASCII digit.
    fn first_violation(text: &str) -> Option<String> {
        for (idx, _) in text.match_indices('"') {
            let rest = &text[idx + 1..];
            for prefix in ["gpt-", "claude-", "gemini-"] {
                if let Some(tail) = rest.strip_prefix(prefix)
                    && tail.chars().next().is_some_and(|c| c.is_ascii_digit())
                {
                    let line = text[..idx].lines().count();
                    let snippet: String = text[idx..].chars().take(40).collect();
                    return Some(format!("line {line}: {snippet}"));
                }
            }
        }
        None
    }

    let src_root = repo_root().join("meerkat-core/src");
    let mut files = Vec::new();
    collect_files(&src_root, &mut files);
    assert!(!files.is_empty(), "meerkat-core/src must not be empty");

    let mut violations = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue; // non-UTF-8 assets cannot carry the literal pattern we ban
        };
        if let Some(found) = first_violation(&text) {
            violations.push(format!("{} ({found})", path.display()));
        }
    }

    assert!(
        violations.is_empty(),
        "provider model-name literals found under meerkat-core/src:\n  {}\n\
         \n\
         PRINCIPLE: meerkat-core must contain ZERO provider-specific model\n\
         data — including model-name string literals in tests, fixtures, and\n\
         templates. Real model rows, defaults, and names live in the\n\
         meerkat-models crate and are injected through the explicit\n\
         `ModelCatalog` parameter. Use obviously-fake ids (e.g.\n\
         `test-anthropic-default` from model_profile::test_catalog) in core\n\
         unit tests, or move the test to meerkat-models.",
        violations.join("\n  ")
    );
}
