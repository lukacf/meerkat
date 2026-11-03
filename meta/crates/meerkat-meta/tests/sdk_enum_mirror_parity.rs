//! Cross-artifact parity gate for the SDK mirrors of UPSTREAM enums.
//!
//! Sibling of `sdk_error_category_parity.rs`, and the reason it is separate is
//! the thing that makes these two mirrors more fragile than `ErrorCategory`:
//! `MobRunStatus` and `MobMemberStatus` are owned by **meerkat**, not by
//! meta. Drift therefore arrives with an upstream version bump and no meta
//! commit involved - which means the repin is exactly when it appears, and
//! nothing in a code review of the repin diff would show it.
//!
//! Meta's SDKs are entirely hand-maintained: no codegen, no schema pipeline,
//! so every one of these declarations is a hand-copied seed.
//!
//! What each drift would cost a host:
//!   - Python `MobRunStatus.parse` and TypeScript `parseMobRunStatus` fall back
//!     to `"pending"` for any value they do not know. A new upstream variant is
//!     therefore not reported as unknown, it is reported as NOT STARTED - a
//!     wrong, actionable value rather than an absent one.
//!   - `MobMemberStatus` is `#[non_exhaustive]` upstream and both SDKs
//!     deliberately pass unknown values through, so drift there costs a typed
//!     arm rather than a wrong answer. Gated anyway: the declaration is still
//!     what a host writes its `switch` against.
//!
//! As in the sibling gate, the authoritative Rust set is read back out of serde
//! (probe with a bogus value, parse the `expected one of` list) rather than
//! scanned out of source text, so a `#[serde(rename)]` cannot slip past it.
//! Every scan is anchored by a second test, so a scan that silently matches
//! nothing fails loudly instead of reporting the whole Rust set as missing.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

/// A value that must never be a real variant of either enum.
const PROBE_VALUE: &str = "__sdk_parity_probe__";

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

/// Pulls serde's variant list out of an unknown-variant error.
fn wire_values_from_probe<T>(enum_name: &str, anchor: &str) -> BTreeSet<String>
where
    T: serde::de::DeserializeOwned,
{
    let probe = serde_json::Value::String(PROBE_VALUE.to_string());
    let error = serde_json::from_value::<T>(probe)
        .err()
        .unwrap_or_else(|| panic!("probe value must not be a real {enum_name} variant"));
    let rendered = error.to_string();

    let listed = rendered
        .split("expected one of ")
        .nth(1)
        .unwrap_or_else(|| {
            panic!(
                "serde no longer enumerates {enum_name} variants in its unknown-variant error, so \
                 this gate can no longer read the Rust vocabulary. Rendered error: {rendered:?}. \
                 Update wire_values_from_probe() in {}.",
                file!()
            )
        });

    let values: BTreeSet<String> = listed
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();

    assert!(
        values.contains(anchor),
        "extracted {enum_name} value set {values:?} is missing the `{anchor}` anchor, so the \
         extraction in {} is no longer reading serde's variant list correctly",
        file!()
    );

    values
}

/// The `NAME = "wire"` members of a Python `(str, Enum)` class.
fn python_enum_members(source: &str, class_name: &str) -> BTreeSet<(String, String)> {
    let header = format!("class {class_name}(str, Enum):");
    let body = source
        .split(&header)
        .nth(1)
        .unwrap_or_else(|| panic!("{PYTHON_SDK_PATH} should declare `{header}`"))
        .split("\nclass ")
        .next()
        .expect("enum body");

    body.lines()
        .filter_map(|line| line.trim().split_once(" = "))
        .filter_map(|(name, value)| quoted_member(name, value))
        .collect()
}

/// The string-literal arms of a TypeScript union type alias:
/// `export type X =\n  | "a"\n  | "b";`
fn typescript_union_arms(source: &str, type_name: &str) -> BTreeSet<String> {
    let header = format!("export type {type_name} =");
    let body = source
        .split(&header)
        .nth(1)
        .unwrap_or_else(|| panic!("{TYPESCRIPT_SDK_PATH} should declare `{header}`"))
        .split(';')
        .next()
        .expect("union body");

    quoted_strings(body)
}

/// The runtime allowlist inside `parseMobRunStatus`, which is a THIRD mirror of
/// the same vocabulary and the one that actually decides what a host observes:
/// a value missing from it is coerced to `"pending"`, not merely left untyped.
fn typescript_run_status_runtime_allowlist(source: &str) -> BTreeSet<String> {
    let body = source
        .split("const known: readonly MobRunStatus[] = [")
        .nth(1)
        .unwrap_or_else(|| {
            panic!(
                "{TYPESCRIPT_SDK_PATH} should declare the parseMobRunStatus runtime allowlist \
                 `const known: readonly MobRunStatus[] = [`"
            )
        })
        .split(']')
        .next()
        .expect("allowlist body");

    quoted_strings(body)
}

/// Every double-quoted string in a fragment. Used only on fragments that are
/// syntactically a list of string literals.
fn quoted_strings(fragment: &str) -> BTreeSet<String> {
    fragment
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// Keeps `NAME "wire"` pairs and drops prose that happens to contain the
/// separator, so a docstring cannot masquerade as a declared member.
fn quoted_member(name: &str, value: &str) -> Option<(String, String)> {
    let value = value.trim();
    let tag = value.strip_prefix('"')?.strip_suffix('"')?;
    Some((name.trim().to_string(), tag.to_string()))
}

/// Renders one declaration site's drift, or `None` when it is in parity.
fn declaration_drift(
    label: &str,
    declared: &BTreeSet<String>,
    rust: &BTreeSet<String>,
) -> Option<String> {
    let missing: Vec<&str> = rust.difference(declared).map(String::as_str).collect();
    let extra: Vec<&str> = declared.difference(rust).map(String::as_str).collect();

    if missing.is_empty() && extra.is_empty() {
        return None;
    }

    Some(format!(
        "{label}:\n  missing (add these): {missing:?}\n  not in Rust (remove these): {extra:?}"
    ))
}

#[test]
fn sdk_mirrors_cover_every_upstream_mob_run_status() {
    let rust = wire_values_from_probe::<meerkat_mob::MobRunStatus>("MobRunStatus", "pending");

    let python = read_repo_file(PYTHON_SDK_PATH);
    let typescript = read_repo_file(TYPESCRIPT_SDK_PATH);

    let python_values: BTreeSet<String> = python_enum_members(&python, "MobRunStatus")
        .into_iter()
        .map(|(_, value)| value)
        .collect();

    let drifted: Vec<String> = [
        declaration_drift(
            &format!("Python `MobRunStatus` ({PYTHON_SDK_PATH})"),
            &python_values,
            &rust,
        ),
        declaration_drift(
            &format!("TypeScript `MobRunStatus` union ({TYPESCRIPT_SDK_PATH})"),
            &typescript_union_arms(&typescript, "MobRunStatus"),
            &rust,
        ),
        declaration_drift(
            &format!("TypeScript parseMobRunStatus runtime allowlist ({TYPESCRIPT_SDK_PATH})"),
            &typescript_run_status_runtime_allowlist(&typescript),
            &rust,
        ),
    ]
    .into_iter()
    .flatten()
    .collect();

    assert!(
        drifted.is_empty(),
        "SDK `MobRunStatus` declarations have drifted from meerkat's `MobRunStatus`. Both SDKs \
         coerce an unrecognized status to `pending`, so a missing variant is reported to hosts as \
         NOT STARTED rather than as unknown.\n{}",
        drifted.join("\n")
    );
}

#[test]
fn sdk_mirrors_cover_every_upstream_mob_member_status() {
    let rust = wire_values_from_probe::<meerkat_mob::MobMemberStatus>("MobMemberStatus", "active");

    let python = read_repo_file(PYTHON_SDK_PATH);
    let typescript = read_repo_file(TYPESCRIPT_SDK_PATH);

    let python_values: BTreeSet<String> = python_enum_members(&python, "MobMemberStatus")
        .into_iter()
        .map(|(_, value)| value)
        .collect();

    let drifted: Vec<String> = [
        declaration_drift(
            &format!("Python `MobMemberStatus` ({PYTHON_SDK_PATH})"),
            &python_values,
            &rust,
        ),
        declaration_drift(
            &format!("TypeScript `MobMemberStatus` union ({TYPESCRIPT_SDK_PATH})"),
            &typescript_union_arms(&typescript, "MobMemberStatus"),
            &rust,
        ),
    ]
    .into_iter()
    .flatten()
    .collect();

    assert!(
        drifted.is_empty(),
        "SDK `MobMemberStatus` declarations have drifted from meerkat's `MobMemberStatus`, so a \
         host cannot write a typed arm for every status it can receive.\n{}",
        drifted.join("\n")
    );
}

#[test]
fn sdk_source_scans_find_the_declared_members() {
    // Guards the gates above: a scan matching nothing on both sides would let
    // a drifted mirror pass, so pin one known member per declaration site.
    let python = read_repo_file(PYTHON_SDK_PATH);
    let typescript = read_repo_file(TYPESCRIPT_SDK_PATH);

    let run_python = python_enum_members(&python, "MobRunStatus");
    assert!(
        run_python.contains(&("PENDING".to_string(), "pending".to_string())),
        "Python `MobRunStatus` scan found {run_python:?}, missing the PENDING anchor - the scan in \
         {} no longer matches the layout of {PYTHON_SDK_PATH}",
        file!()
    );

    let member_python = python_enum_members(&python, "MobMemberStatus");
    assert!(
        member_python.contains(&("ACTIVE".to_string(), "active".to_string())),
        "Python `MobMemberStatus` scan found {member_python:?}, missing the ACTIVE anchor - the \
         scan in {} no longer matches the layout of {PYTHON_SDK_PATH}",
        file!()
    );

    for (label, found, anchor) in [
        (
            "TypeScript `MobRunStatus` union",
            typescript_union_arms(&typescript, "MobRunStatus"),
            "pending",
        ),
        (
            "TypeScript `MobMemberStatus` union",
            typescript_union_arms(&typescript, "MobMemberStatus"),
            "active",
        ),
        (
            "TypeScript parseMobRunStatus runtime allowlist",
            typescript_run_status_runtime_allowlist(&typescript),
            "pending",
        ),
    ] {
        assert!(
            found.contains(anchor),
            "{label} scan found {found:?}, missing the `{anchor}` anchor - the scan in {} no \
             longer matches the layout of {TYPESCRIPT_SDK_PATH}",
            file!()
        );
    }
}
