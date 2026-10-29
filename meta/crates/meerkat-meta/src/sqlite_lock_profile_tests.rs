//! The bundled SQLite this workspace links is built without its process-wide
//! locks (allocator statistics, one page cache shared by every connection and
//! page-cache overflow statistics), and every place that sets its build flags
//! carries the one value meerkat-sqlite declares,
//! `meerkat_sqlite::REQUIRED_LIBSQLITE3_FLAGS`: `.cargo/config.toml` for
//! Cargo and `MODULE.bazel` for Bazel. Cargo reads the configuration of the
//! directory it builds in, never a dependency's, so Meerkat's own settings do
//! not reach this workspace. Every host binary also turns SQLite's allocator
//! statistics off as the first statement of `main`, for builds made without
//! the flags. These rows mirror meerkat-sqlite's `lock_profile` and
//! `lock_profile_sources` tests.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use meerkat_sqlite::{REQUIRED_LIBSQLITE3_FLAGS, SqliteLockProfile};

/// The host binaries and their `main` files. release.yml's gateway build step
/// ships `meta_gateway`, `rpc_gateway` and `meta_repair`; the flow editor is a
/// long-running server too. Every binary target the workspace manifests
/// declare is either here, and switches allocator statistics off as the first
/// statement of `main`, or in `EXEMPT_BINARIES`.
const HOST_BINARIES: &[(&str, &str)] = &[
    (
        "meta_flow_editor",
        "crates/meerkat-meta/src/bin/meta_flow_editor.rs",
    ),
    (
        "meta_gateway",
        "crates/meerkat-meta/src/bin/meta_gateway.rs",
    ),
    ("meta_repair", "crates/meerkat-meta/src/bin/meta_repair.rs"),
    ("rpc_gateway", "crates/meerkat-meta/src/bin/rpc_gateway.rs"),
];

/// Binary targets that do not switch, each with the reason. A reason that
/// says it is not a release binary is checked against the binaries
/// release.yml builds.
const EXEMPT_BINARIES: &[(&str, &str)] = &[
    (
        "baseline_check",
        "checks Meerkat's baseline symbols in source files and opens no SQLite store; \
         not a release binary",
    ),
    (
        "distiller-eval",
        "calibration harness: one fixture per process on a current-thread runtime, so no \
         concurrent SQLite connections; not a release binary",
    ),
    (
        "governance_check",
        "validates the .rct governance files and opens no SQLite store; not a release binary",
    ),
    (
        "hygienist-eval",
        "calibration harness: one fixture per process on a current-thread runtime, so no \
         concurrent SQLite connections; not a release binary",
    ),
    (
        "mcp_fixture",
        "MCP server test fixture that opens no SQLite store; not a release binary",
    ),
    (
        "steward-eval",
        "calibration harness: one fixture per process on a current-thread runtime, so no \
         concurrent SQLite connections; not a release binary",
    ),
];

/// The words of an exemption reason that release.yml must agree with.
const NOT_A_RELEASE_BINARY: &str = "not a release binary";

/// The call each host `main` makes first.
const SWITCH_CALL: &str = "meerkat_sqlite::disable_memory_statistics_at_process_start()";

/// The lint allowance each host switch carries. Spelled in two parts so that
/// this file is not one of the allowances it counts.
const ALLOW_UNSAFE_CODE: &str = concat!("#[allow(", "unsafe_code)]");

fn workspace_root() -> PathBuf {
    std::env::var_os("WORKSPACE_ROOT").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        PathBuf::from,
    )
}

fn read(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("the path sits in the workspace")
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn bundled_sqlite_is_built_without_process_wide_locks() {
    let compiled = SqliteLockProfile::compiled();
    assert_eq!(
        compiled,
        SqliteLockProfile::REQUIRED,
        "the linked SQLite runs with process-wide locks ({compiled}): this build did not \
         set LIBSQLITE3_FLAGS=\"{REQUIRED_LIBSQLITE3_FLAGS}\""
    );
    assert_eq!(SqliteLockProfile::effective(), SqliteLockProfile::REQUIRED);
    meerkat_sqlite::check_lock_profile().expect("the required SQLite lock profile");
}

/// The `crate.annotation(...)` block in `MODULE.bazel` for `krate`.
fn annotation_block<'a>(module: &'a str, krate: &str) -> &'a str {
    let crate_line = format!("crate = \"{krate}\",");
    let at = module
        .find(&crate_line)
        .unwrap_or_else(|| panic!("MODULE.bazel has no annotation for {krate}"));
    let start = module[..at]
        .rfind("crate.annotation(")
        .expect("the crate line sits in a crate.annotation block");
    let end = at
        + module[at..]
            .find("\n)")
            .expect("the annotation block closes");
    &module[start..end]
}

#[test]
fn cargo_and_bazel_set_the_declared_sqlite_flags() {
    let root = workspace_root();

    // Cargo: `.cargo/config.toml` [env], forced over a value the environment
    // already carries, so every repository build follows the declaration.
    let config: toml::Table = read(&root, ".cargo/config.toml")
        .parse()
        .expect(".cargo/config.toml parses");
    let entry = config
        .get("env")
        .and_then(|env| env.get("LIBSQLITE3_FLAGS"))
        .expect(".cargo/config.toml [env] sets LIBSQLITE3_FLAGS")
        .as_table()
        .expect("LIBSQLITE3_FLAGS is a { value, force } table");
    assert_eq!(
        entry.get("value").and_then(toml::Value::as_str),
        Some(REQUIRED_LIBSQLITE3_FLAGS)
    );
    assert_eq!(
        entry.get("force").and_then(toml::Value::as_bool),
        Some(true)
    );

    // Bazel: exactly one LIBSQLITE3_FLAGS setting, in the libsqlite3-sys
    // annotation of the "crates" repository.
    let module = read(&root, "MODULE.bazel");
    let setting = format!("\"LIBSQLITE3_FLAGS\": \"{REQUIRED_LIBSQLITE3_FLAGS}\",");
    let settings: Vec<&str> = module
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("\"LIBSQLITE3_FLAGS\""))
        .collect();
    assert_eq!(
        settings,
        [setting.as_str()],
        "MODULE.bazel LIBSQLITE3_FLAGS settings"
    );
    let annotation = annotation_block(&module, "libsqlite3-sys");
    assert!(annotation.contains(&setting), "{annotation}");
    assert!(
        annotation.contains("repositories = [\"crates\"]"),
        "{annotation}"
    );
}

/// Every workspace member directory: the root manifest's
/// `[workspace].members`.
fn workspace_members(root: &Path) -> Vec<PathBuf> {
    let manifest: toml::Table = read(root, "Cargo.toml")
        .parse()
        .expect("the root manifest parses");
    let members = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .expect("[workspace].members is an array");
    let mut dirs: Vec<PathBuf> = members
        .iter()
        .map(|member| {
            let member = member.as_str().expect("a workspace member is a path");
            assert!(!member.contains('*'), "unsupported member glob {member}");
            root.join(member)
        })
        .collect();
    dirs.sort();
    dirs
}

/// Cargo's default source of a `[[bin]]` without a `path`.
fn default_bin_path(dir: &Path, package_name: &str, name: &str) -> PathBuf {
    let main = dir.join("src/main.rs");
    let file = dir.join("src/bin").join(format!("{name}.rs"));
    if name == package_name && main.is_file() {
        main
    } else if file.is_file() {
        file
    } else {
        dir.join("src/bin").join(name).join("main.rs")
    }
}

/// The binaries Cargo discovers in a member: `src/main.rs` (named after the
/// package), `src/bin/*.rs` and `src/bin/*/main.rs`.
fn discovered_binaries(dir: &Path, package_name: &str) -> Vec<(String, PathBuf)> {
    let mut discovered = Vec::new();
    let main = dir.join("src/main.rs");
    if main.is_file() {
        discovered.push((package_name.to_string(), main));
    }
    let Ok(entries) = fs::read_dir(dir.join("src/bin")) else {
        return discovered;
    };
    for entry in entries {
        let path = entry.expect("a src/bin entry").path();
        let stem = path
            .file_stem()
            .expect("a src/bin entry has a name")
            .to_string_lossy()
            .into_owned();
        if path.extension().is_some_and(|extension| extension == "rs") {
            discovered.push((stem, path));
        } else if path.join("main.rs").is_file() {
            discovered.push((stem, path.join("main.rs")));
        }
    }
    discovered
}

/// One member's binary targets, by name: its `[[bin]]` entries and, unless
/// `autobins = false`, the binaries Cargo discovers that no entry declares.
fn member_binaries(dir: &Path) -> Vec<(String, PathBuf)> {
    let manifest: toml::Table = fs::read_to_string(dir.join("Cargo.toml"))
        .expect("a member manifest reads")
        .parse()
        .expect("a member manifest parses");
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .expect("a member has [package]");
    let package_name = package
        .get("name")
        .and_then(toml::Value::as_str)
        .expect("a package has a name");
    let mut declared = Vec::new();
    for bin in manifest
        .get("bin")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = bin
            .get("name")
            .and_then(toml::Value::as_str)
            .expect("a [[bin]] has a name");
        let path = bin.get("path").and_then(toml::Value::as_str).map_or_else(
            || default_bin_path(dir, package_name, name),
            |path| dir.join(path),
        );
        declared.push((name.to_string(), path));
    }
    let autobins = package
        .get("autobins")
        .and_then(toml::Value::as_bool)
        .unwrap_or(true);
    if autobins {
        for (name, path) in discovered_binaries(dir, package_name) {
            let known = declared.iter().any(|(declared_name, declared_path)| {
                *declared_name == name || *declared_path == path
            });
            if !known {
                declared.push((name, path));
            }
        }
    }
    declared
}

/// Every binary target of the workspace, by name, with its source file
/// relative to the workspace root.
fn workspace_binaries(root: &Path) -> BTreeMap<String, String> {
    let mut binaries = BTreeMap::new();
    for dir in workspace_members(root) {
        for (name, path) in member_binaries(&dir) {
            assert!(
                !binaries.contains_key(&name),
                "two workspace binaries named {name}"
            );
            binaries.insert(name, relative_to(root, &path));
        }
    }
    binaries
}

/// The binaries release.yml's gateway build step builds: the `--bin` names of
/// its `cargo build -p meerkat-meta` command, continuation lines included.
fn release_binaries(root: &Path) -> Vec<String> {
    let workflow = read(root, ".github/workflows/release.yml");
    let lines: Vec<&str> = workflow.lines().collect();
    let mut binaries = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.trim_start().starts_with("cargo build -p meerkat-meta") {
            continue;
        }
        let mut command = String::new();
        for part in &lines[index..] {
            let part = part.trim();
            command.push(' ');
            command.push_str(part.trim_end_matches('\\'));
            if !part.ends_with('\\') {
                break;
            }
        }
        let words: Vec<&str> = command.split_whitespace().collect();
        for pair in words.windows(2) {
            if pair[0] == "--bin" {
                binaries.push(pair[1].to_string());
            }
        }
    }
    binaries.sort();
    binaries.dedup();
    binaries
}

#[test]
fn every_workspace_binary_is_a_host_or_exempt_with_a_reason() {
    let root = workspace_root();
    let mut expected: BTreeMap<String, Option<String>> = HOST_BINARIES
        .iter()
        .map(|(name, path)| ((*name).to_string(), Some((*path).to_string())))
        .collect();
    for (name, reason) in EXEMPT_BINARIES {
        assert!(
            !reason.trim().is_empty(),
            "{name}: an exemption names its reason"
        );
        assert!(
            expected.insert((*name).to_string(), None).is_none(),
            "{name} is listed twice"
        );
    }
    let found: BTreeMap<String, Option<String>> = workspace_binaries(&root)
        .into_iter()
        .map(|(name, source)| {
            let host = HOST_BINARIES.iter().any(|(host, _)| *host == name.as_str());
            (name, host.then_some(source))
        })
        .collect();
    assert_eq!(
        found, expected,
        "the workspace binaries changed: a new binary that opens SQLite is a host (list it in \
         HOST_BINARIES and switch first), any other goes in EXEMPT_BINARIES with its reason"
    );

    // What a release ships is a host, and an exemption that says it is not a
    // release binary is not built by release.yml.
    let released = release_binaries(&root);
    assert!(
        !released.is_empty(),
        "release.yml's gateway build step names its binaries"
    );
    for name in &released {
        assert!(
            HOST_BINARIES.iter().any(|(host, _)| *host == name.as_str()),
            "{name} ships in a release, so it is a host that switches first"
        );
    }
    for (name, reason) in EXEMPT_BINARIES {
        if reason.contains(NOT_A_RELEASE_BINARY) {
            assert!(
                !released.iter().any(|built| built.as_str() == *name),
                "{name}: the reason says {NOT_A_RELEASE_BINARY}, but release.yml builds it"
            );
        }
    }
}

/// The opening of the file's one `fn main`: its leading comment and attribute
/// lines, then its first statement, each trimmed. `None` unless exactly one
/// line of the file starts with `fn main(`.
fn main_opening(source: &str) -> Option<Vec<&str>> {
    let lines: Vec<&str> = source.lines().collect();
    let mut mains = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("fn main("));
    let (index, _) = mains.next()?;
    if mains.next().is_some() {
        return None;
    }
    let mut opening = Vec::new();
    for line in &lines[index + 1..] {
        let trimmed = line.trim();
        opening.push(trimmed);
        if !(trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("#[")) {
            break;
        }
    }
    Some(opening)
}

#[test]
fn every_host_binary_switches_allocator_statistics_off_first() {
    let root = workspace_root();
    for (name, path) in HOST_BINARIES {
        let source = read(&root, path);
        let opening =
            main_opening(&source).unwrap_or_else(|| panic!("{name} ({path}): one fn main"));
        let statement = opening.last().copied().unwrap_or_default();
        assert!(
            statement.contains(SWITCH_CALL),
            "{name} ({path}): the first statement of main must be {SWITCH_CALL}, found \
             {statement:?}"
        );
        assert!(
            opening.iter().any(|line| line.starts_with("// SAFETY:")),
            "{name} ({path}): the call carries a SAFETY comment"
        );
        assert!(
            opening.contains(&ALLOW_UNSAFE_CODE),
            "{name} ({path}): the call carries {ALLOW_UNSAFE_CODE}"
        );
        assert!(
            !opening.iter().any(|line| line.starts_with("#[cfg")),
            "{name} ({path}): the call is unconditional; a cfg could compile it out of a build \
             that opens SQLite"
        );
    }
}

/// Every `.rs` file under `dir`, outside build output and dependency trees.
fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut sources = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                let skipped = path
                    .file_name()
                    .is_some_and(|name| name == "target" || name == "node_modules");
                if !skipped {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push(path);
            }
        }
    }
    sources.sort();
    sources
}

#[test]
fn unsafe_code_is_allowed_only_at_the_host_switches() {
    // The workspace denies `unsafe_code` rather than forbidding it, so that
    // each host switch can allow it; no other Rust source allows it.
    let root = workspace_root();
    let needles = [
        concat!("allow(", "unsafe_code)"),
        concat!("expect(", "unsafe_code)"),
    ];
    let mut allowances = BTreeMap::new();
    for dir in ["crates", "examples"] {
        for file in rust_sources(&root.join(dir)) {
            let text = fs::read_to_string(&file).expect("a Rust source reads");
            let count: usize = needles
                .iter()
                .map(|needle| text.matches(*needle).count())
                .sum();
            if count > 0 {
                allowances.insert(relative_to(&root, &file), count);
            }
        }
    }
    let expected: BTreeMap<String, usize> = HOST_BINARIES
        .iter()
        .map(|(_, path)| ((*path).to_string(), 1))
        .collect();
    assert_eq!(
        allowances, expected,
        "unsafe code is allowed only once, at each host binary's SQLite switch"
    );
}
