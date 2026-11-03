//! Cross-artifact parity gate for the `ErrorEvent` wire vocabulary.
//!
//! Rust owns the vocabulary: `ErrorEvent` is `#[serde(tag = "category")]`, so
//! the set of category strings a host can ever observe is exactly the set of
//! variant tags serde knows. The SDKs re-declare that set as `ErrorCategory`
//! so hosts can write typed alert arms. When a variant is added on the Rust
//! side and the SDKs are not updated, the event still arrives (both SDKs pass
//! unknown categories through as raw strings) but no host can match on it -
//! silent drift precisely on the events we tell operators to page on.
//!
//! The authoritative Rust set is read back out of serde rather than parsed out
//! of the source text: probing with a bogus tag makes serde enumerate every
//! variant it accepts, which is the real wire contract and survives a
//! per-variant `#[serde(rename = "...")]` that a source scan would miss.
//!
//! The SDK files live outside this crate, so they are read at runtime from the
//! repo root instead of `include_str!` (the Bazel build-metadata generator
//! discards `include_str!` paths outside the package root).

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use meerkat_meta::ErrorEvent;

/// A category string that must never be a real variant.
const PROBE_CATEGORY: &str = "__sdk_parity_probe__";

const RUST_ENUM_PATH: &str = "crates/meerkat-meta/src/unified_runtime/types.rs";
const PYTHON_SDK_PATH: &str = "sdk/python/meerkat_meta/types.py";
const TYPESCRIPT_SDK_PATH: &str = "sdk/typescript/src/types.ts";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("meerkat-meta should sit under crates/ at the repo root")
        .to_path_buf()
}

fn read_repo_file(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {} failed: {err}", path.display()))
}

/// Every category tag serde accepts for `ErrorEvent`, straight from serde.
fn rust_wire_categories() -> BTreeSet<String> {
    let probe = serde_json::json!({ "category": PROBE_CATEGORY });
    let error = serde_json::from_value::<ErrorEvent>(probe)
        .expect_err("probe category must not be a real ErrorEvent variant");
    let rendered = error.to_string();

    let listed = rendered
        .split("expected one of ")
        .nth(1)
        .unwrap_or_else(|| {
            panic!(
                "serde no longer enumerates ErrorEvent variants in its unknown-variant error, so \
                 this gate can no longer read the Rust vocabulary. Rendered error: {rendered:?}. \
                 Update rust_wire_categories() in {}.",
                file!()
            )
        });

    // The list renders as `a`, `b`, `c` - every odd backtick-split segment is a tag.
    let categories: BTreeSet<String> = listed
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();

    assert!(
        categories.contains("spawn_failure"),
        "extracted Rust category set {categories:?} is missing the `spawn_failure` anchor, so the \
         extraction in {} is no longer reading serde's variant list correctly",
        file!()
    );

    categories
}

/// `SPAWN_FAILURE = "spawn_failure"` - the Python `ErrorCategory` members.
fn python_declared_categories(source: &str) -> BTreeSet<(String, String)> {
    let body = source
        .split("class ErrorCategory(str, Enum):")
        .nth(1)
        .unwrap_or_else(|| panic!("{PYTHON_SDK_PATH} should declare `class ErrorCategory`"))
        .split("\nclass ")
        .next()
        .expect("enum body");

    body.lines()
        .filter_map(|line| line.trim().split_once(" = "))
        .filter_map(|(name, value)| quoted_member(name, value))
        .collect()
}

/// `SPAWN_FAILURE: "spawn_failure",` - the TypeScript `ErrorCategory` members.
fn typescript_declared_categories(source: &str) -> BTreeSet<(String, String)> {
    let body = source
        .split("export const ErrorCategory = {")
        .nth(1)
        .unwrap_or_else(|| panic!("{TYPESCRIPT_SDK_PATH} should declare `ErrorCategory`"))
        .split("} as const;")
        .next()
        .expect("object literal body");

    body.lines()
        .filter_map(|line| line.trim().trim_end_matches(',').split_once(": "))
        .filter_map(|(name, value)| quoted_member(name, value))
        .collect()
}

/// Keeps `NAME "wire"` pairs and drops prose that happens to contain the
/// separator, so a docstring cannot masquerade as a declared member.
fn quoted_member(name: &str, value: &str) -> Option<(String, String)> {
    let value = value.trim();
    let tag = value.strip_prefix('"')?.strip_suffix('"')?;
    Some((name.trim().to_string(), tag.to_string()))
}

/// Renders one SDK's drift, or `None` when it is in parity. Every SDK is
/// reported in a single failure so one run names every file to edit.
fn sdk_drift(
    sdk: &str,
    sdk_path: &str,
    declared: &BTreeSet<(String, String)>,
    rust: &BTreeSet<String>,
) -> Option<String> {
    let wire: BTreeSet<String> = declared.iter().map(|(_, value)| value.clone()).collect();

    let missing: Vec<&str> = rust.difference(&wire).map(String::as_str).collect();
    let extra: Vec<&str> = wire.difference(rust).map(String::as_str).collect();

    // A member whose name is not the SCREAMING_SNAKE of its tag reads as a typed
    // arm for one category while matching another.
    let mislabeled: Vec<&str> = declared
        .iter()
        .filter(|(name, value)| name.as_str() != value.to_uppercase())
        .map(|(name, _)| name.as_str())
        .collect();

    if missing.is_empty() && extra.is_empty() && mislabeled.is_empty() {
        return None;
    }

    Some(format!(
        "{sdk} SDK `ErrorCategory` ({sdk_path}):\n  \
         missing from the SDK (add these): {missing:?}\n  \
         not in Rust (remove these): {extra:?}\n  \
         members not named after their wire tag: {mislabeled:?}"
    ))
}

#[test]
fn sdk_error_categories_cover_every_rust_error_event_variant() {
    let rust = rust_wire_categories();

    let drifted: Vec<String> = [
        sdk_drift(
            "Python",
            PYTHON_SDK_PATH,
            &python_declared_categories(&read_repo_file(PYTHON_SDK_PATH)),
            &rust,
        ),
        sdk_drift(
            "TypeScript",
            TYPESCRIPT_SDK_PATH,
            &typescript_declared_categories(&read_repo_file(TYPESCRIPT_SDK_PATH)),
            &rust,
        ),
    ]
    .into_iter()
    .flatten()
    .collect();

    assert!(
        drifted.is_empty(),
        "SDK `ErrorCategory` declarations have drifted from the Rust `ErrorEvent` enum \
         ({RUST_ENUM_PATH}), so a host cannot write a typed arm for every event it can \
         receive.\n{}",
        drifted.join("\n")
    );
}

#[test]
fn sdk_source_scans_find_the_declared_categories() {
    // Guards the gate above: a scan that silently matched nothing would make
    // every Rust variant look "missing" - but a scan that matched nothing on
    // *both* sides would not, so pin a known member on each.
    let python = python_declared_categories(&read_repo_file(PYTHON_SDK_PATH));
    let typescript = typescript_declared_categories(&read_repo_file(TYPESCRIPT_SDK_PATH));

    for (sdk, declared, path) in [
        ("Python", &python, PYTHON_SDK_PATH),
        ("TypeScript", &typescript, TYPESCRIPT_SDK_PATH),
    ] {
        assert!(
            declared.contains(&("SPAWN_FAILURE".to_string(), "spawn_failure".to_string())),
            "{sdk} `ErrorCategory` scan found {declared:?}, which does not include the \
             SPAWN_FAILURE anchor - the scan in {} no longer matches the layout of {path}",
            file!()
        );
    }
}
