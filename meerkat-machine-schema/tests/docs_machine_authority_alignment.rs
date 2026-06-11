#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Docs drift gate (Dogma Rule 9 / Generated Theater).
//!
//! `docs/reference/machine-authority.mdx` hand-maintains a "Canonical Machines"
//! table. The canonical authority is `canonical_machine_schemas()` in
//! `meerkat-machine-schema/src/catalog/mod.rs`. Without this gate the doc table
//! silently drifts from the registry (it previously listed 7 of 10 machines).
//! This test binds the table to the registry: every canonical machine must be
//! documented and the table must not list a machine the registry does not own.

use std::path::{Path, PathBuf};

use meerkat_machine_schema::{canonical_composition_schemas, canonical_machine_schemas};

/// Extract the first backtick-quoted token on each markdown table row whose
/// token ends in `Machine` — i.e. the first column of the Canonical Machines
/// table (`| `MeerkatMachine` | ... |`). Owner/scope cells and other tables do
/// not put a `*Machine` identifier in the leading cell.
fn documented_machine_names(mdx: &str) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    for line in mdx.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('|') {
            continue;
        }
        // First cell content between the leading `|` and the next `|`.
        let first_cell = trimmed[1..].split('|').next().unwrap_or("").trim();
        if let Some(inner) = first_cell
            .strip_prefix('`')
            .and_then(|s| s.strip_suffix('`'))
            && inner.ends_with("Machine")
        {
            names.insert(inner.to_string());
        }
    }
    names
}

#[test]
fn machine_authority_doc_table_matches_canonical_registry() {
    let mdx_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../docs/reference/machine-authority.mdx"
    );
    let mdx =
        std::fs::read_to_string(mdx_path).unwrap_or_else(|err| panic!("read {mdx_path}: {err}"));

    let documented = documented_machine_names(&mdx);
    let canonical: std::collections::BTreeSet<String> = canonical_machine_schemas()
        .into_iter()
        .map(|schema| schema.machine.as_str().to_owned())
        .collect();

    let missing_from_docs: Vec<&String> = canonical.difference(&documented).collect();
    let extra_in_docs: Vec<&String> = documented.difference(&canonical).collect();

    assert!(
        missing_from_docs.is_empty(),
        "machine-authority.mdx is missing canonical machines from the registry \
         (canonical_machine_schemas()): {missing_from_docs:?}"
    );
    assert!(
        extra_in_docs.is_empty(),
        "machine-authority.mdx lists machines not in the canonical registry: {extra_in_docs:?}"
    );
}

/// Recursively collect `.md`/`.mdx` files under `dir`.
fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|err| panic!("read_dir {}: {err}", dir.display()));
    for entry in entries {
        let entry =
            entry.unwrap_or_else(|err| panic!("read_dir entry in {}: {err}", dir.display()));
        let path = entry.path();
        if path.is_dir() {
            markdown_files(&path, out);
        } else if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("md" | "mdx")
        ) {
            out.push(path);
        }
    }
}

/// Parse a doc token as a roster-count claim ("7", "seven", ...).
fn claimed_count(token: &str) -> Option<usize> {
    let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    if let Ok(n) = token.parse::<usize>() {
        return Some(n);
    }
    const SPELLED: [(&str, usize); 12] = [
        ("one", 1),
        ("two", 2),
        ("three", 3),
        ("four", 4),
        ("five", 5),
        ("six", 6),
        ("seven", 7),
        ("eight", 8),
        ("nine", 9),
        ("ten", 10),
        ("eleven", 11),
        ("twelve", 12),
    ];
    SPELLED
        .iter()
        .find(|(word, _)| token.eq_ignore_ascii_case(word))
        .map(|(_, n)| *n)
}

/// Scan one line for `<count> canonical machine(s)` / `<count> canonical
/// composition(s)` claims and return `(claimed, canonical, kind)` mismatches.
fn count_claim_mismatches(
    line: &str,
    machine_count: usize,
    composition_count: usize,
) -> Vec<(usize, usize, &'static str)> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut mismatches = Vec::new();
    for window in words.windows(3) {
        let [count_word, canonical_word, noun] = window else {
            continue;
        };
        if !canonical_word.eq_ignore_ascii_case("canonical") {
            continue;
        }
        let noun = noun.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let canonical =
            if noun.eq_ignore_ascii_case("machine") || noun.eq_ignore_ascii_case("machines") {
                (machine_count, "machine")
            } else if noun.eq_ignore_ascii_case("composition")
                || noun.eq_ignore_ascii_case("compositions")
            {
                (composition_count, "composition")
            } else {
                continue;
            };
        let Some(claimed) = claimed_count(count_word) else {
            continue;
        };
        if claimed != canonical.0 {
            mismatches.push((claimed, canonical.0, canonical.1));
        }
    }
    mismatches
}

/// Doctrine docs must not hand-maintain a stale machine/composition count.
/// Any `N canonical machine(s)`/`N canonical composition(s)` claim in
/// `docs/`, `docs-internal/`, or `.claude/skills/` must equal the registry
/// count (`canonical_machine_schemas()` / `canonical_composition_schemas()`).
#[test]
fn doctrine_count_claims_match_canonical_registry() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let machine_count = canonical_machine_schemas().len();
    let composition_count = canonical_composition_schemas().len();

    let mut files = Vec::new();
    for tree in ["docs", "docs-internal", ".claude/skills"] {
        let dir = repo_root.join(tree);
        if dir.is_dir() {
            markdown_files(&dir, &mut files);
        }
    }
    assert!(!files.is_empty(), "no doctrine markdown files found");

    let mut stale = Vec::new();
    for path in files {
        // Archived snapshots are frozen history, not live doctrine.
        if path.components().any(|c| c.as_os_str() == "archive") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        for (lineno, line) in text.lines().enumerate() {
            for (claimed, canonical, kind) in
                count_claim_mismatches(line, machine_count, composition_count)
            {
                stale.push(format!(
                    "{}:{}: claims {claimed} canonical {kind}s but the registry owns {canonical}",
                    path.display(),
                    lineno + 1,
                ));
            }
        }
    }

    assert!(
        stale.is_empty(),
        "doctrine docs hand-maintain stale canonical machine/composition counts \
         (point at the registry instead of copying its count):\n{}",
        stale.join("\n")
    );
}
