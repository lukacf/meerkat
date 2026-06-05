#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Docs drift gate (Dogma Rule 9 / Generated Theater).
//!
//! `docs/reference/machine-authority.mdx` hand-maintains a "Canonical Machines"
//! table. The canonical authority is `canonical_machine_schemas()` in
//! `meerkat-machine-schema/src/catalog/mod.rs`. Without this gate the doc table
//! silently drifts from the registry (it previously listed 7 of 10 machines).
//! This test binds the table to the registry: every canonical machine must be
//! documented and the table must not list a machine the registry does not own.

use meerkat_machine_schema::canonical_machine_schemas;

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
