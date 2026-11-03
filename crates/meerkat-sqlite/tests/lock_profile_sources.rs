//! The places that set the bundled SQLite's build flags carry the one value
//! meerkat-sqlite declares ([`meerkat_sqlite::REQUIRED_LIBSQLITE3_FLAGS`]),
//! and every host binary turns SQLite's allocator statistics off as the first
//! statement of `main`, for builds made without those flags.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use meerkat_sqlite::REQUIRED_LIBSQLITE3_FLAGS;

/// The host binaries and their `main` files. Every binary target the
/// workspace manifests declare is either here, and switches allocator
/// statistics off as the first statement of `main`, or in
/// [`EXEMPT_BINARIES`].
const HOST_BINARIES: &[(&str, &str)] = &[
    ("rkat", "crates/meerkat-cli/src/main.rs"),
    ("rkat-mcp", "crates/meerkat-mcp-server/src/main.rs"),
    ("rkat-mob-mcp", "crates/meerkat-mob-mcp/src/main.rs"),
    ("rkat-rest", "crates/meerkat-rest/src/main.rs"),
    ("rkat-rpc", "crates/meerkat-rpc/src/main.rs"),
];

/// Binary targets that do not switch, each with the reason. A reason that
/// says "never shipped" is checked against the binary's manifest: its package
/// has `publish = false`.
const EXEMPT_BINARIES: &[(&str, &str)] = &[
    ("e2e_artifacts", "integration-test tool, never shipped"),
    ("embedded_min", "surface build test fixture, never shipped"),
    (
        "emit-schemas",
        "writes the JSON schema files and opens no SQLite store",
    ),
    ("mcp-test-server", "MCP server test fixture, never shipped"),
    (
        "runtime_backed_min",
        "surface build test fixture, never shipped",
    ),
    ("voice_fixtures", "integration-test tool, never shipped"),
    ("xtask", "repository tooling, never shipped"),
];

/// One binary target: its source file relative to the workspace root, and
/// whether its package publishes (no `publish = false`).
#[derive(Debug)]
struct Binary {
    source: String,
    published: bool,
}

/// The call each host `main` makes first.
const SWITCH_CALL: &str = "meerkat_sqlite::disable_memory_statistics_at_process_start()";

/// These tests read the repository itself (`.cargo/config.toml`,
/// `MODULE.bazel`, every crate's `main.rs`); the Bazel BUILD generator gives a
/// test that names this marker the whole workspace as runfiles.
const LIVE_WORKSPACE_RUNFILES: &str = "required";

fn workspace_root() -> PathBuf {
    assert_eq!(LIVE_WORKSPACE_RUNFILES, "required");
    std::env::var_os("WORKSPACE_ROOT").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        PathBuf::from,
    )
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
    let config: toml::Table = fs::read_to_string(root.join(".cargo/config.toml"))
        .unwrap()
        .parse()
        .unwrap();
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
    let module = fs::read_to_string(root.join("MODULE.bazel")).unwrap();
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

/// Every member directory of Core: the root manifest's
/// `[workspace].default-members` (its `members` when it has none), with
/// `dir/*` expanded to the directories under `dir` that hold a `Cargo.toml`.
/// The other products' crates (`meta/`, `connect/`) are members too, but
/// their binaries keep their own lock profile rules.
fn workspace_members(root: &Path) -> Vec<PathBuf> {
    let manifest: toml::Table = fs::read_to_string(root.join("Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let workspace = &manifest["workspace"];
    let members = workspace
        .get("default-members")
        .unwrap_or(&workspace["members"])
        .as_array()
        .expect("[workspace].default-members (or members) is an array");
    let mut dirs = Vec::new();
    for member in members {
        let member = member.as_str().expect("a workspace member is a path");
        if let Some(parent) = member.strip_suffix("/*") {
            assert!(!parent.contains('*'), "unsupported member glob {member}");
            for entry in fs::read_dir(root.join(parent)).unwrap() {
                let dir = entry.unwrap().path();
                if dir.join("Cargo.toml").is_file() {
                    dirs.push(dir);
                }
            }
        } else {
            assert!(!member.contains('*'), "unsupported member glob {member}");
            dirs.push(root.join(member));
        }
    }
    dirs.sort();
    dirs
}

/// Every binary target of the workspace, by name: the `[[bin]]` entries of
/// each member manifest and, unless `autobins = false`, the binaries Cargo
/// discovers (`src/main.rs`, `src/bin/*.rs`, `src/bin/*/main.rs`).
fn workspace_binaries(root: &Path) -> BTreeMap<String, Binary> {
    let relative = |path: &Path| {
        path.strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mut binaries = BTreeMap::new();
    for dir in workspace_members(root) {
        let manifest: toml::Table = fs::read_to_string(dir.join("Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let package = manifest["package"]
            .as_table()
            .expect("a member has [package]");
        let package_name = package["name"].as_str().unwrap();
        let published = package.get("publish").and_then(toml::Value::as_bool) != Some(false);
        let mut declared = Vec::new();
        for bin in manifest
            .get("bin")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = bin["name"].as_str().expect("a [[bin]] has a name");
            let path = bin.get("path").and_then(toml::Value::as_str).map_or_else(
                || {
                    let main = dir.join("src/main.rs");
                    let file = dir.join("src/bin").join(format!("{name}.rs"));
                    if name == package_name && main.is_file() {
                        main
                    } else if file.is_file() {
                        file
                    } else {
                        dir.join("src/bin").join(name).join("main.rs")
                    }
                },
                |path| dir.join(path),
            );
            declared.push((name.to_string(), relative(&path)));
        }
        let autobins = package
            .get("autobins")
            .and_then(toml::Value::as_bool)
            .unwrap_or(true);
        if autobins {
            let mut discovered = Vec::new();
            let main = dir.join("src/main.rs");
            if main.is_file() {
                discovered.push((package_name.to_string(), relative(&main)));
            }
            if let Ok(entries) = fs::read_dir(dir.join("src/bin")) {
                for entry in entries {
                    let path = entry.unwrap().path();
                    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
                    if path.extension().is_some_and(|ext| ext == "rs") {
                        discovered.push((stem, relative(&path)));
                    } else if path.join("main.rs").is_file() {
                        discovered.push((stem, relative(&path.join("main.rs"))));
                    }
                }
            }
            for (name, path) in discovered {
                if !declared.iter().any(|(n, p)| *n == name || *p == path) {
                    declared.push((name, path));
                }
            }
        }
        for (name, source) in declared {
            let binary = Binary { source, published };
            assert!(
                binaries.insert(name.clone(), binary).is_none(),
                "two workspace binaries named {name}"
            );
        }
    }
    binaries
}

/// The first statement of every `fn main` that starts the host runtime,
/// with its leading comment and attribute lines.
fn host_main_openings(source: &str) -> Vec<Vec<&str>> {
    let lines: Vec<&str> = source.lines().collect();
    let mut openings = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.starts_with("fn main(") {
            continue;
        }
        let body_end = lines[index..]
            .iter()
            .position(|line| *line == "}")
            .map_or(lines.len(), |end| index + end);
        let body = &lines[index + 1..body_end];
        if !body.iter().any(|line| line.contains("host_stack::")) {
            continue;
        }
        let mut opening = Vec::new();
        for line in body {
            let trimmed = line.trim();
            opening.push(trimmed);
            if !(trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("#[")) {
                break;
            }
        }
        openings.push(opening);
    }
    openings
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
    let binaries = workspace_binaries(&root);
    let found: BTreeMap<String, Option<String>> = binaries
        .iter()
        .map(|(name, binary)| {
            let host = HOST_BINARIES.iter().any(|(host, _)| host == name);
            (name.clone(), host.then(|| binary.source.clone()))
        })
        .collect();
    assert_eq!(
        found, expected,
        "the workspace binaries changed: a new binary that opens SQLite is a host (list it in \
         HOST_BINARIES and switch first), any other goes in EXEMPT_BINARIES with its reason"
    );
    for (name, reason) in EXEMPT_BINARIES {
        if reason.contains("never shipped") {
            assert!(
                !binaries[*name].published,
                "{name}: the reason says never shipped, but its package publishes"
            );
        }
    }
}

#[test]
fn every_host_binary_switches_allocator_statistics_off_first() {
    let root = workspace_root();
    for (name, path) in HOST_BINARIES {
        let source = fs::read_to_string(root.join(path)).unwrap();
        let openings = host_main_openings(&source);
        assert_eq!(openings.len(), 1, "{name} ({path}): one host main");
        let opening = &openings[0];
        let statement = opening.last().copied().unwrap_or_default();
        assert!(
            statement.contains(SWITCH_CALL),
            "{name} ({path}): the first statement of main must be {SWITCH_CALL}, found {statement:?}"
        );
        assert!(
            opening.iter().any(|line| line.starts_with("// SAFETY:")),
            "{name} ({path}): the call carries a SAFETY comment"
        );
        assert!(
            opening.contains(&"#[allow(unsafe_code)]"),
            "{name} ({path}): the call carries #[allow(unsafe_code)]"
        );
        assert!(
            !opening.iter().any(|line| line.starts_with("#[cfg")),
            "{name} ({path}): the call is unconditional; a cfg could compile it out of a build \
             that opens SQLite"
        );
    }
}

#[test]
fn every_host_binary_depends_on_meerkat_sqlite_unconditionally() {
    let root = workspace_root();
    for (name, path) in HOST_BINARIES {
        let crate_dir = root
            .join(path)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let manifest: toml::Table = fs::read_to_string(crate_dir.join("Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let mut tables: Vec<&toml::Table> = Vec::new();
        if let Some(dependencies) = manifest.get("dependencies").and_then(toml::Value::as_table) {
            tables.push(dependencies);
        }
        for target in manifest
            .get("target")
            .and_then(toml::Value::as_table)
            .into_iter()
            .flat_map(|targets| targets.values())
        {
            if let Some(dependencies) = target.get("dependencies").and_then(toml::Value::as_table) {
                tables.push(dependencies);
            }
        }
        let declarations: Vec<&toml::Value> = tables
            .iter()
            .filter_map(|table| table.get("meerkat-sqlite"))
            .collect();
        assert!(
            !declarations.is_empty(),
            "{name}: the manifest declares meerkat-sqlite as a normal dependency"
        );
        for declaration in declarations {
            let optional = declaration
                .get("optional")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
            assert!(
                !optional,
                "{name}: meerkat-sqlite is not optional: {declaration}"
            );
        }
    }
}
