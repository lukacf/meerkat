//! Source guard of the realm admission fences (`meerkat_core::realm_admission`).
//!
//! - Every durable write primitive of a bound store takes the operation's
//!   admission guard (`PRIMITIVES`), so an unfenced write of such a store does
//!   not compile.
//! - Fences are acquired only at the listed outermost entries
//!   (`ACQUISITION_SITES`), each pinned to the function it sits in: a new
//!   acquisition, or one moved into another function, fails the guard until
//!   it is listed here, and a listed one that is gone fails it too. The
//!   helpers that take a fence count by name, with or without a turbofish.
//! - Only the config runtime calls a store's raw held writes (`set_held`,
//!   `patch_held`, `patch_held_with_warnings`), as a method or by path; a
//!   store's own raw write may delegate to another one.
//!
//! The scan reads shipped code only, with comments and literals blanked and
//! test-only items removed, so a mention in a comment, a string or a test
//! never counts. The defining module, `realm_admission.rs`, is not scanned for
//! acquisitions.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// The bound stores' durable write primitives: the file, the function name,
/// and how many shipped definitions of it the file has. Every one takes an
/// admission guard parameter.
const PRIMITIVES: &[(&str, &str, usize)] = &[
    // The credential store's writes, declared once by the trait every
    // persisted store implements.
    ("crates/meerkat-core/src/auth/token_store.rs", "save", 1),
    ("crates/meerkat-core/src/auth/token_store.rs", "clear", 1),
    ("crates/meerkat-core/src/auth/token_store.rs", "save_for", 1),
    (
        "crates/meerkat-core/src/auth/token_store.rs",
        "clear_for",
        1,
    ),
    ("crates/meerkat-core/src/config_store.rs", "set_held", 4),
    ("crates/meerkat-core/src/config_store.rs", "patch_held", 4),
    (
        "crates/meerkat-core/src/config_store.rs",
        "patch_held_with_warnings",
        3,
    ),
    (
        "crates/meerkat-core/src/config_store.rs",
        "write_content",
        1,
    ),
    (
        "crates/meerkat-core/src/config_runtime.rs",
        "write_generation_at",
        1,
    ),
    ("crates/meerkat-core/src/config_runtime.rs", "set_under", 1),
    (
        "crates/meerkat-core/src/config_runtime.rs",
        "patch_under",
        1,
    ),
    (
        "crates/meerkat-core/src/config_runtime.rs",
        "patch_entered_under",
        1,
    ),
    ("crates/meerkat-core/src/mcp_config.rs", "add_under", 1),
    (
        "crates/meerkat-authorization/src/grants/durable.rs",
        "create",
        3,
    ),
    (
        "crates/meerkat-authorization/src/grants/durable.rs",
        "append",
        3,
    ),
    (
        "crates/meerkat-authorization/src/grants/durable.rs",
        "new_durable",
        1,
    ),
    (
        "crates/meerkat-authorization/src/grants/durable.rs",
        "apply",
        1,
    ),
    (
        "crates/meerkat-authorization/src/grants/mod.rs",
        "issue_root",
        1,
    ),
    (
        "crates/meerkat-authorization/src/grants/mod.rs",
        "issue_child",
        1,
    ),
    (
        "crates/meerkat-authorization/src/grants/mod.rs",
        "issue_or_recover_child_under_exclusion",
        1,
    ),
    (
        "crates/meerkat-authorization/src/grants/mod.rs",
        "issue_or_recover_child_reserved",
        1,
    ),
    (
        "crates/meerkat-authorization/src/grants/mod.rs",
        "revoke",
        1,
    ),
    (
        "crates/meerkat-authorization/src/grants/mod.rs",
        "revoke_expecting",
        1,
    ),
    (
        "crates/meerkat-authorization/src/grants/mod.rs",
        "apply_change",
        1,
    ),
    (
        "crates/meerkat/src/governed_realm/stock/mod.rs",
        "write_stock_grant_file",
        1,
    ),
    ("crates/meerkat-cli/src/main.rs", "set_held", 1),
    ("crates/meerkat-cli/src/main.rs", "patch_held", 1),
    (
        "crates/meerkat-cli/src/main.rs",
        "patch_held_with_warnings",
        1,
    ),
];

/// The calls that take an admission fence, matched by name: every use of one
/// in shipped code other than its definition or an import counts (a call,
/// with or without a turbofish, or the function itself passed on).
const ACQUISITIONS: &[&str] = &[
    "AdmissionGuard::acquire",
    "AdmissionGuard::try_acquire",
    "AdmissionHold::acquire",
    "AdmissionHold::try_acquire",
    "take_realm_fence",
    "take_realm_fence_now",
    "acquire_fences",
    "realm_removal_fence",
    "copy_admission_fence",
];

/// Every fence acquisition in shipped code, pinned to the function it sits
/// in: the file, the function, the acquisition and how many times that
/// function has it. These are the outermost entries of the fenced
/// operations, each before any other lock of its own, or without waiting
/// where it already holds one; everything they call passes their guard down.
/// An acquisition moved into another function (after a custody that function
/// takes, say) fails the guard until it is listed there.
const ACQUISITION_SITES: &[(&str, &str, &str, usize)] = &[
    // The config runtime's fences, through its helper: each of its own
    // writes takes them first, before the document's custody (the file
    // store's public writes run through a runtime of their own).
    (
        "crates/meerkat-core/src/config_runtime.rs",
        "acquire_fences",
        "AdmissionGuard::acquire",
        1,
    ),
    (
        "crates/meerkat-core/src/config_runtime.rs",
        "set",
        "acquire_fences",
        1,
    ),
    (
        "crates/meerkat-core/src/config_runtime.rs",
        "patch",
        "acquire_fences",
        1,
    ),
    (
        "crates/meerkat-core/src/config_runtime.rs",
        "patch_with_warnings",
        "acquire_fences",
        1,
    ),
    (
        "crates/meerkat-core/src/config_runtime.rs",
        "patch_entered",
        "acquire_fences",
        1,
    ),
    // The credential coordinator's entries (refresh and exclusive
    // mutation), after in-process coalescing and before the per-key lock
    // file and the closure, within the lock file's wait.
    (
        "crates/meerkat-auth-core/src/auth_store/refresh.rs",
        "admit",
        "AdmissionGuard::acquire",
        1,
    ),
    // The user document's create-on-read: without waiting.
    (
        "crates/meerkat-core/src/config_store.rs",
        "create_if_unheld",
        "AdmissionGuard::try_acquire",
        1,
    ),
    // Child issuance by the work owner (the dispatch and entered paths),
    // beside its non-waiting reservation: without waiting.
    (
        "crates/meerkat-authorization/src/work.rs",
        "issue_child_if_current_within",
        "AdmissionGuard::try_acquire",
        1,
    ),
    // Child issuance on the prepared path: without waiting.
    (
        "crates/meerkat/src/governed_realm/helper_child_issuance.rs",
        "issue_or_recover",
        "AdmissionGuard::try_acquire",
        1,
    ),
    // The stock commands' realm fence, through these two helpers.
    (
        "crates/meerkat/src/governed_realm/stock/mod.rs",
        "take_realm_fence",
        "AdmissionGuard::acquire",
        1,
    ),
    (
        "crates/meerkat/src/governed_realm/stock/mod.rs",
        "take_realm_fence_now",
        "AdmissionGuard::try_acquire",
        1,
    ),
    // Each stock command, first on its owned task, before its custody.
    (
        "crates/meerkat/src/governed_realm/stock/mod.rs",
        "govern_stock_realm",
        "take_realm_fence",
        1,
    ),
    (
        "crates/meerkat/src/governed_realm/stock/mod.rs",
        "record_stock_grant_selection",
        "take_realm_fence",
        1,
    ),
    (
        "crates/meerkat/src/governed_realm/stock/issuance.rs",
        "grant_stock_agent",
        "take_realm_fence",
        1,
    ),
    (
        "crates/meerkat/src/governed_realm/stock/issuance.rs",
        "grant_stock_agent_for_reviewed_installation",
        "take_realm_fence",
        1,
    ),
    (
        "crates/meerkat/src/governed_realm/stock/issuance.rs",
        "revoke_stock_agent",
        "take_realm_fence",
        1,
    ),
    // The staged grant, under the stage's custody: without waiting.
    (
        "crates/meerkat/src/governed_realm/stock/issuance.rs",
        "grant_under_staged_custody",
        "take_realm_fence_now",
        1,
    ),
    // The connector-client writers: the realm document's fences and the
    // credential fence, before the coordinator's entry and every write.
    (
        "crates/meerkat/src/connector_clients.rs",
        "guarded_profile_update",
        "AdmissionGuard::acquire",
        1,
    ),
    // The reviewed connector finalization, first on its owned task, before
    // its custody.
    (
        "crates/meerkat/src/governed_realm/stock/finalize.rs",
        "run",
        "AdmissionGuard::acquire",
        1,
    ),
    // Realm delete and prune, through their helper, first: before the
    // lease check, without waiting.
    (
        "crates/meerkat-cli/src/main.rs",
        "realm_removal_fence",
        "AdmissionGuard::try_acquire",
        1,
    ),
    (
        "crates/meerkat-cli/src/main.rs",
        "delete_realm",
        "realm_removal_fence",
        1,
    ),
    (
        "crates/meerkat-cli/src/main.rs",
        "prune_realms_inner",
        "realm_removal_fence",
        1,
    ),
    // The storage-migrate split-brain archive, through its helper: every
    // copy's fence before each maintenance fence, without waiting.
    (
        "crates/meerkat-cli/src/storage_migrate.rs",
        "copy_admission_fence",
        "AdmissionGuard::try_acquire",
        1,
    ),
    (
        "crates/meerkat-cli/src/storage_migrate.rs",
        "resolve_split_brain",
        "copy_admission_fence",
        1,
    ),
];

/// The raw held writes of a config store.
const RAW_HELD_WRITES: &[&str] = &["set_held", "patch_held", "patch_held_with_warnings"];

/// The one caller of a store's raw held writes.
const RAW_HELD_WRITE_CALLER: &str = "crates/meerkat-core/src/config_runtime.rs";

/// The module that defines the fences.
const FENCE_MODULE: &str = "crates/meerkat-core/src/realm_admission.rs";

#[test]
fn bound_store_write_primitives_all_take_an_admission_guard() {
    let root = repo_root();
    let mut problems = Vec::new();
    for (file, name, expected) in PRIMITIVES {
        let code = shipped_code(&read(&root.join(file)));
        let definitions = definitions(&code, name);
        if definitions.len() != *expected {
            problems.push(format!(
                "{file}: {} shipped definition(s) of `{name}`, listed {expected}",
                definitions.len()
            ));
        }
        for parameters in definitions {
            if !parameters.contains("AdmissionGuard") {
                problems.push(format!(
                    "{file}: `{name}` takes no admission guard: ({})",
                    compact(&parameters)
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn fence_acquisition_sites_are_exactly_the_listed_outermost_entries() {
    let root = repo_root();
    let mut found: BTreeMap<(String, String, &str), usize> = BTreeMap::new();
    for path in walk_rust_sources(&root) {
        let relative = relative(&root, &path);
        if !is_shipped_source(&relative) || relative == FENCE_MODULE {
            continue;
        }
        let code = shipped_code(&read(&path));
        let imports = use_declarations(&code);
        for acquisition in ACQUISITIONS {
            for at in uses(&code, acquisition, &imports) {
                let function = enclosing_function(&code, at)
                    .unwrap_or("outside any function")
                    .to_string();
                *found
                    .entry((relative.clone(), function, acquisition))
                    .or_default() += 1;
            }
        }
    }
    let listed: BTreeMap<(String, String, &str), usize> = ACQUISITION_SITES
        .iter()
        .map(|(file, function, acquisition, count)| {
            (
                ((*file).to_string(), (*function).to_string(), *acquisition),
                *count,
            )
        })
        .collect();
    let describe = |((file, function, acquisition), count): (&(String, String, &str), &usize)| {
        format!("{file}: `{acquisition}` in `{function}` x{count}")
    };
    let unlisted: Vec<String> = found
        .iter()
        .filter(|(site, count)| listed.get(*site) != Some(*count))
        .map(describe)
        .collect();
    let gone: Vec<String> = listed
        .iter()
        .filter(|(site, count)| found.get(*site) != Some(*count))
        .map(describe)
        .collect();
    assert!(
        unlisted.is_empty() && gone.is_empty(),
        "fence acquisitions differ from the listed outermost entries\n\
         found but not listed (or another count):\n  {}\n\
         listed but not found (or another count):\n  {}",
        unlisted.join("\n  "),
        gone.join("\n  ")
    );
}

#[test]
fn raw_held_writes_are_called_only_from_config_runtime() {
    let root = repo_root();
    let mut problems = Vec::new();
    let mut runtime_calls = 0usize;
    for path in walk_rust_sources(&root) {
        let relative = relative(&root, &path);
        if !is_shipped_source(&relative) {
            continue;
        }
        let code = shipped_code(&read(&path));
        for name in RAW_HELD_WRITES {
            // A method call, or a call by path (`ConfigStore::set_held(..)`).
            for call in [format!(".{name}("), format!("::{name}(")] {
                for (at, _) in code.match_indices(&call) {
                    if relative == RAW_HELD_WRITE_CALLER {
                        runtime_calls += 1;
                        continue;
                    }
                    // A store's own raw write may delegate to another one.
                    match enclosing_function(&code, at) {
                        Some(function) if RAW_HELD_WRITES.contains(&function) => {}
                        other => problems.push(format!(
                            "{relative}: `{call}` called from {}",
                            other.map_or("outside any function".to_string(), |name| {
                                format!("`{name}`")
                            })
                        )),
                    }
                }
            }
        }
    }
    assert!(
        runtime_calls > 0,
        "the config runtime's raw held writes were not found"
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The parameter lists of every definition of the function `name` in `code`
/// (comments and literals blanked): `fn name(` or `fn name<..>(`.
fn definitions(code: &str, name: &str) -> Vec<String> {
    let bytes = code.as_bytes();
    let needle = format!("fn {name}");
    let mut out = Vec::new();
    for (at, _) in code.match_indices(&needle) {
        if at > 0 && is_identifier_char(char::from(bytes[at - 1])) {
            continue;
        }
        let mut index = at + needle.len();
        if bytes
            .get(index)
            .is_some_and(|byte| is_identifier_char(char::from(*byte)))
        {
            continue;
        }
        // Past any generic parameters to the parameter list.
        while index < bytes.len() && bytes[index] != b'(' {
            index += 1;
        }
        let start = index;
        let mut depth = 0usize;
        while index < bytes.len() {
            match bytes[index] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            index += 1;
        }
        out.push(code[start..=index.min(bytes.len() - 1)].to_string());
    }
    out
}

/// The byte offsets of the uses of `name` in `code` (comments and literals
/// blanked): every occurrence as a whole path segment that is not its
/// definition (`fn name`) or inside one of the `use` declarations `imports`.
fn uses(code: &str, name: &str, imports: &[Range<usize>]) -> Vec<usize> {
    let bytes = code.as_bytes();
    code.match_indices(name)
        .map(|(at, _)| at)
        .filter(|&at| at == 0 || !is_identifier_char(char::from(bytes[at - 1])))
        .filter(|&at| {
            bytes
                .get(at + name.len())
                .is_none_or(|byte| !is_identifier_char(char::from(*byte)))
        })
        .filter(|&at| !is_definition(&code[..at]))
        .filter(|at| !imports.iter().any(|import| import.contains(at)))
        .collect()
}

/// Whether `before`, the code up to a name, ends with the `fn` keyword that
/// defines it.
fn is_definition(before: &str) -> bool {
    let before = before.trim_end();
    before.strip_suffix("fn").is_some_and(|rest| {
        rest.chars()
            .next_back()
            .is_none_or(|character| !is_identifier_char(character))
    })
}

/// The byte ranges of the `use` declarations in `code` (comments and literals
/// blanked), from the keyword to its `;`: a `use` that starts an item or a
/// statement, after any visibility.
fn use_declarations(code: &str) -> Vec<Range<usize>> {
    let bytes = code.as_bytes();
    let mut ranges = Vec::new();
    for (at, _) in code.match_indices("use ") {
        if at > 0 && is_identifier_char(char::from(bytes[at - 1])) {
            continue;
        }
        let before = code[..at].trim_end();
        let before = ["pub(crate)", "pub(super)", "pub"]
            .iter()
            .find_map(|visibility| before.strip_suffix(visibility))
            .map_or(before, str::trim_end);
        if !(before.is_empty() || before.ends_with([';', '{', '}', ']'])) {
            continue;
        }
        if let Some(end) = code[at..].find(';') {
            ranges.push(at..at + end + 1);
        }
    }
    ranges
}

/// The name of the function whose definition most recently precedes `at` in
/// `code`: the function a call at `at` sits in, for code that does not nest
/// functions in function bodies.
fn enclosing_function(code: &str, at: usize) -> Option<&str> {
    let before = &code[..at];
    let start = before
        .match_indices("fn ")
        .filter(|(index, _)| {
            *index == 0 || !is_identifier_char(char::from(before.as_bytes()[index - 1]))
        })
        .map(|(index, _)| index + 3)
        .last()?;
    let rest = &before[start..];
    let end = rest
        .find(|character: char| !is_identifier_char(character))
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// `text` without whitespace.
fn compact(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

/// Whether `character` continues an identifier.
fn is_identifier_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR -> crates/meerkat; parent x2 = workspace root.
    // Read at run time (cargo, nextest and the Bazel test rule set it), so an
    // archived test binary never carries the builder's path.
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set for test processes"),
    );
    let root = manifest_dir
        .parent()
        .expect("crates/meerkat has a parent")
        .parent()
        .expect("crates has a parent (workspace root)")
        .to_path_buf();
    if root.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        root
    }
}

/// The roots of the repository's products other than Core
/// (`ci/products.json`): each holds a product with its own crates and its
/// own Clippy and Cargo configuration, which this test leaves to it. Core's
/// launch confinement, its Clippy configuration and its Cargo configuration
/// are the root's.
fn product_roots(root: &Path) -> Vec<PathBuf> {
    let path = root.join("ci/products.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let registry: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
    registry["products"]
        .as_array()
        .unwrap_or_else(|| panic!("{} lists products", path.display()))
        .iter()
        .filter_map(|product| product["root"].as_str())
        .filter(|product_root| !product_root.is_empty())
        .map(|product_root| root.join(product_root))
        .collect()
}

/// Every `.rs` file under `root`, skipping build output and VCS metadata.
fn walk_rust_sources(root: &Path) -> Vec<PathBuf> {
    walk_files(root, |name| name.ends_with(".rs"))
}

/// Every directory under `root`, `root` included, skipping build output,
/// VCS metadata, Cargo's own directories and the product roots
/// ([`product_roots`]).
fn walk_directories(root: &Path) -> Vec<PathBuf> {
    let products = product_roots(root);
    let mut out = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if entry.file_type().is_ok_and(|file_type| file_type.is_dir())
                    && !matches!(
                        name.as_ref(),
                        "target" | ".git" | "node_modules" | ".cargo" | "dist"
                    )
                    && !name.starts_with("bazel-")
                    && !products.contains(&entry.path())
                {
                    pending.push(entry.path());
                }
            }
        }
        out.push(dir);
    }
    out
}

/// Every file under `root` whose name `keep` accepts, skipping build output
/// and VCS metadata.
fn walk_files(root: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    walk_directories(root)
        .into_iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flat_map(|entries| entries.flatten())
        .filter(|entry| {
            entry.file_type().is_ok_and(|file_type| !file_type.is_dir())
                && keep(&entry.file_name().to_string_lossy())
        })
        .map(|entry| entry.path())
        .collect()
}

/// Whether a repository-relative source path is shipped library or binary
/// code: a crate's `src` tree, without the developer tooling crate, build
/// scripts, and test-only files.
fn is_shipped_source(relative: &str) -> bool {
    let Some(rest) = relative.strip_prefix("crates/") else {
        return false;
    };
    let Some((krate, in_crate)) = rest.split_once('/') else {
        return false;
    };
    if krate == "xtask" || !in_crate.starts_with("src/") {
        return false;
    }
    let file = in_crate.rsplit('/').next().unwrap_or(in_crate);
    let stem = file.strip_suffix(".rs").unwrap_or(file);
    !(stem == "tests"
        || stem.ends_with("_tests")
        || in_crate.split('/').any(|part| part == "tests"))
}

/// `source` with comments and literals blanked and every test-only item
/// (`#[cfg(test)]`, `#[cfg(all(test, ..))]`, `#[test]` and `#[*::test]`)
/// removed, so only code a shipped build compiles remains.
fn shipped_code(source: &str) -> String {
    let code = blank_comments_and_literals(source);
    let bytes = code.as_bytes();
    let mut out = String::with_capacity(code.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'#'
            && let Some((attribute, end)) = attribute_at(&code, index)
        {
            if gates_test_only(attribute) {
                index = item_end(bytes, end);
                continue;
            }
            out.push_str(&code[index..end]);
            index = end;
            continue;
        }
        let character = code[index..].chars().next().unwrap();
        out.push(character);
        index += character.len_utf8();
    }
    out
}

/// The attribute starting at `start` (`#[...]` or `#![...]`) and the index
/// after it.
fn attribute_at(code: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = code.as_bytes();
    let mut index = start + 1;
    if bytes.get(index) == Some(&b'!') {
        index += 1;
    }
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    let mut depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&code[start..=index], index + 1));
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Whether an attribute makes its item test-only.
fn gates_test_only(attribute: &str) -> bool {
    let compact: String = attribute.chars().filter(|c| !c.is_whitespace()).collect();
    compact == "#[test]"
        || (compact.starts_with("#[") && compact.ends_with("::test]"))
        || (compact.contains("::test(") && !compact.starts_with("#[cfg"))
        || compact == "#[cfg(test)]"
        || compact.starts_with("#[cfg(all(test,")
        || (compact.starts_with("#[cfg(all(") && compact.contains(",test,"))
        || (compact.starts_with("#[cfg(all(") && compact.contains(",test))"))
}

/// The index after the item that starts at or after `start`: past any
/// further attributes, through its matching closing brace, or its `;` when
/// it has no body. A match arm, field, variant or argument (anything that
/// does not start with an item or statement keyword) also ends at its `,`,
/// and any item ends where the construct enclosing it closes, so the code
/// after it is scanned.
fn item_end(bytes: &[u8], start: usize) -> usize {
    let separated = !starts_with_item_keyword(bytes, start);
    let mut index = start;
    let mut depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'{' | b'(' | b'[' => depth += 1,
            closing @ (b'}' | b')' | b']') => {
                if depth == 0 {
                    return index;
                }
                depth -= 1;
                if depth == 0 && closing == b'}' {
                    return index + 1;
                }
            }
            b';' if depth == 0 => return index + 1,
            b',' if depth == 0 && separated => return index + 1,
            _ => {}
        }
        index += 1;
    }
    bytes.len()
}

/// Whether the code at `start`, past whitespace and attributes, starts with
/// an item or statement keyword (whose `,` at the top level, as in generic
/// parameters, does not end it).
fn starts_with_item_keyword(bytes: &[u8], start: usize) -> bool {
    const KEYWORDS: &[&str] = &[
        "async",
        "const",
        "enum",
        "extern",
        "fn",
        "impl",
        "let",
        "macro_rules",
        "mod",
        "pub",
        "static",
        "struct",
        "trait",
        "type",
        "union",
        "unsafe",
        "use",
    ];
    let mut index = start;
    loop {
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if bytes.get(index) != Some(&b'#') {
            break;
        }
        let mut depth = 0usize;
        while let Some(&byte) = bytes.get(index) {
            index += 1;
            match byte {
                b'[' => depth += 1,
                b']' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let word_end = bytes[index.min(bytes.len())..]
        .iter()
        .position(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
        .map_or(bytes.len(), |offset| index + offset);
    let word = std::str::from_utf8(&bytes[index.min(bytes.len())..word_end]).unwrap_or("");
    KEYWORDS.contains(&word)
}

/// `source` with every comment and string or character literal replaced by
/// spaces (newlines kept).
fn blank_comments_and_literals(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    let blank = |out: &mut String, character: char| {
        out.push(if character == '\n' { '\n' } else { ' ' });
    };
    while index < chars.len() {
        let character = chars[index];
        let next = chars.get(index + 1).copied();
        if character == '/' && next == Some('/') {
            while index < chars.len() && chars[index] != '\n' {
                blank(&mut out, chars[index]);
                index += 1;
            }
            continue;
        }
        if character == '/' && next == Some('*') {
            let mut depth = 0usize;
            while index < chars.len() {
                if chars[index] == '/' && chars.get(index + 1) == Some(&'*') {
                    depth += 1;
                    blank(&mut out, chars[index]);
                    blank(&mut out, '*');
                    index += 2;
                    continue;
                }
                if chars[index] == '*' && chars.get(index + 1) == Some(&'/') {
                    depth -= 1;
                    blank(&mut out, '*');
                    blank(&mut out, '/');
                    index += 2;
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                blank(&mut out, chars[index]);
                index += 1;
            }
            continue;
        }
        // Raw strings: r"..", r#".."#, br#".."#.
        let raw_start = match (character, next) {
            ('r', Some('"' | '#')) => Some(index + 1),
            ('b', Some('r')) if matches!(chars.get(index + 2), Some('"' | '#')) => Some(index + 2),
            _ => None,
        };
        if let Some(mut cursor) = raw_start
            && !identifier_continues(&chars, index)
        {
            let mut hashes = 0usize;
            while chars.get(cursor) == Some(&'#') {
                hashes += 1;
                cursor += 1;
            }
            if chars.get(cursor) == Some(&'"') {
                cursor += 1;
                while cursor < chars.len() {
                    if chars[cursor] == '"'
                        && (0..hashes).all(|offset| chars.get(cursor + 1 + offset) == Some(&'#'))
                    {
                        cursor += 1 + hashes;
                        break;
                    }
                    cursor += 1;
                }
                for &skipped in &chars[index..cursor.min(chars.len())] {
                    blank(&mut out, skipped);
                }
                index = cursor;
                continue;
            }
        }
        if character == '"' {
            blank(&mut out, character);
            index += 1;
            while index < chars.len() {
                let current = chars[index];
                blank(&mut out, current);
                index += 1;
                if current == '\\' {
                    if index < chars.len() {
                        blank(&mut out, chars[index]);
                        index += 1;
                    }
                } else if current == '"' {
                    break;
                }
            }
            continue;
        }
        if character == '\'' {
            // A character literal ('x', '\n', '\u{..}'); a lifetime has no
            // closing quote right after one character or escape.
            let end = if next == Some('\\') {
                chars
                    .get(index + 3..)
                    .and_then(|rest| rest.iter().position(|&c| c == '\''))
                    .map(|offset| index + 3 + offset)
            } else if chars.get(index + 2) == Some(&'\'') {
                Some(index + 2)
            } else {
                None
            };
            if let Some(end) = end {
                for &skipped in &chars[index..=end] {
                    blank(&mut out, skipped);
                }
                index = end + 1;
                continue;
            }
        }
        out.push(character);
        index += 1;
    }
    out
}

/// Whether the character before `index` continues an identifier, so an `r`
/// or `b` there is not a literal prefix.
fn identifier_continues(chars: &[char], index: usize) -> bool {
    index > 0 && (chars[index - 1].is_alphanumeric() || chars[index - 1] == '_')
}
