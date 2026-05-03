#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LIVE_WORKSPACE_RUNFILES: &str = "required";

fn repo_root() -> PathBuf {
    assert_eq!(LIVE_WORKSPACE_RUNFILES, "required");
    xtask::public_contracts::repo_root().expect("resolve repo root")
}

fn read(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn read_utf8(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn walk_files(root: &Path, files: &mut Vec<PathBuf>) {
    if !root.exists() {
        return;
    }
    for entry in
        fs::read_dir(root).unwrap_or_else(|err| panic!("read dir {}: {err}", root.display()))
    {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if entry.file_type().expect("file type").is_dir() {
            if matches!(
                name,
                ".git" | ".rct" | "bazel-bin" | "bazel-out" | "bazel-testlogs" | "node_modules"
            ) || name.starts_with("target")
                || name.starts_with("bazel-")
            {
                continue;
            }
            walk_files(&path, files);
        } else {
            files.push(path);
        }
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn quoted_after(text: &str, needle: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(idx) = rest.find(needle) {
        rest = &rest[idx + needle.len()..];
        if let Some(first) = rest.find('"') {
            let after_first = &rest[first + 1..];
            if let Some(second) = after_first.find('"') {
                out.insert(after_first[..second].to_string());
                rest = &after_first[second + 1..];
            }
        }
    }
    out
}

fn find_all_between(text: &str, start: &str, end: &str) -> Option<String> {
    let start_idx = text.find(start)? + start.len();
    let rest = &text[start_idx..];
    let end_idx = rest.find(end)?;
    Some(rest[..end_idx].to_string())
}

fn workspace_member_dirs(root: &Path) -> Vec<PathBuf> {
    let workspace: toml::Value = read(root.join("Cargo.toml"))
        .parse()
        .expect("parse Cargo.toml");
    workspace["workspace"]["members"]
        .as_array()
        .expect("workspace members")
        .iter()
        .filter_map(|member| member.as_str())
        .filter(|member| !member.contains('*'))
        .map(|member| root.join(member))
        .collect()
}

#[test]
fn buildbuddy_machine_authority_lane_runs_tlc_machine_verify() {
    let root = repo_root();
    let launcher = read(root.join("scripts/buildbuddy-bazel-poc"));
    let build = read(root.join("xtask/BUILD.bazel"));
    let wrapper = read(root.join("xtask/tests/machine_verify_all_tlc_test.sh"));
    let doctor = read(root.join("scripts/buildbuddy-doctor"));

    let lane = find_all_between(&launcher, "machine-authority-rbe)", ";;")
        .expect("machine-authority-rbe lane block");
    assert!(
        lane.contains("default_target=\"//xtask:machine_verify_all_tlc_test "),
        "machine-authority-rbe must start with explicit machine-verify TLC target; block:\n{lane}"
    );
    assert!(
        build.contains("name = \"machine_verify_all_tlc_test\""),
        "xtask BUILD must declare machine_verify_all_tlc_test"
    );
    assert!(
        build.contains("args = [\"$(rootpath :xtask_bin)\"]")
            && build.contains("\":xtask_bin\"")
            && build.contains("\"//:workspace_runfiles\"")
            && build.contains(
                "\"@@rules_rust++rust+rustfmt_nightly-2026-04-16__aarch64-apple-darwin_tools//:rustfmt_bin\"",
            )
            && build.contains(
                "\"@@rules_rust++rust+rustfmt_nightly-2026-04-16__aarch64-apple-darwin_tools//:rustc_lib\"",
            )
            && build.contains(
                "\"RUSTFMT\": \"$(rootpath @@rules_rust++rust+rustfmt_nightly-2026-04-16__aarch64-apple-darwin_tools//:rustfmt_bin)\"",
            ),
        "machine_verify_all_tlc_test must run xtask with workspace runfiles and hermetic rustfmt"
    );
    assert!(
        wrapper.contains("machine-verify --all") && wrapper.contains("export RUSTFMT"),
        "machine_verify_all_tlc_test wrapper must normalize RUSTFMT and run machine-verify --all"
    );
    assert!(
        doctor.contains("machine_verify_all_tlc_test")
            && doctor.contains("machine-verify --all")
            && doctor.contains("hermetic rustfmt"),
        "buildbuddy-doctor must guard the machine-authority TLC mapping and rustfmt runfiles"
    );
}

#[test]
fn rust_sources_are_formatted() {
    let root = repo_root();
    let rustfmt = std::env::var_os("RUSTFMT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("rustfmt"));
    let mut files = Vec::new();
    for member in workspace_member_dirs(&root) {
        walk_files(&member, &mut files);
    }
    let mut rust_files: Vec<_> = files
        .into_iter()
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("rs"))
        .filter(|path| !relative(&root, path).contains("/tests/compile_fail/"))
        .collect();
    rust_files.sort();
    assert!(!rust_files.is_empty(), "no Rust files found");

    let output = Command::new(rustfmt)
        .args(["--edition", "2024", "--check"])
        .args(&rust_files)
        .output()
        .expect("run rustfmt --check");
    assert!(
        output.status.success(),
        "rustfmt --check failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn public_version_surfaces_are_in_sync() {
    let root = repo_root();
    let workspace: toml::Value = read(root.join("Cargo.toml"))
        .parse()
        .expect("parse Cargo.toml");
    let cargo_version = workspace["workspace"]["package"]["version"]
        .as_str()
        .expect("workspace package version");

    let pyproject: toml::Value = read(root.join("sdks/python/pyproject.toml"))
        .parse()
        .expect("parse pyproject");
    let python_version = pyproject["project"]["version"]
        .as_str()
        .expect("python sdk version");

    let ts_package: serde_json::Value =
        serde_json::from_str(&read(root.join("sdks/typescript/package.json")))
            .expect("parse TypeScript package.json");
    let ts_version = ts_package["version"]
        .as_str()
        .expect("typescript sdk version");

    let web_package: serde_json::Value =
        serde_json::from_str(&read(root.join("sdks/web/package.json")))
            .expect("parse web package.json");
    let web_version = web_package["version"].as_str().expect("web sdk version");

    assert_eq!(python_version, cargo_version, "Python SDK version mismatch");
    assert_eq!(ts_version, cargo_version, "TypeScript SDK version mismatch");
    assert_eq!(web_version, cargo_version, "Web SDK version mismatch");

    let version_rs = read(root.join("meerkat-contracts/src/version.rs"));
    let mut parts = Vec::new();
    let current = find_all_between(&version_rs, "pub const CURRENT: Self = Self {", "};")
        .expect("ContractVersion::CURRENT");
    for key in ["major", "minor", "patch"] {
        let needle = format!("{key}:");
        let idx = current
            .find(&needle)
            .unwrap_or_else(|| panic!("missing {key}"));
        let digits: String = current[idx + needle.len()..]
            .chars()
            .skip_while(char::is_ascii_whitespace)
            .take_while(char::is_ascii_digit)
            .collect();
        assert!(!digits.is_empty(), "missing numeric {key}");
        parts.push(digits);
    }
    let contract_version = parts.join(".");
    assert_eq!(contract_version, cargo_version, "contract version mismatch");

    let schema_version: serde_json::Value =
        serde_json::from_str(&read(root.join("artifacts/schemas/version.json")))
            .expect("parse schema version");
    assert_eq!(
        schema_version["contract_version"].as_str(),
        Some(cargo_version),
        "schema contract version mismatch"
    );

    let py_types = read(root.join("sdks/python/meerkat/generated/types.py"));
    assert!(
        py_types.contains(&format!("CONTRACT_VERSION = \"{cargo_version}\"")),
        "Python generated CONTRACT_VERSION is stale"
    );
    let ts_types = read(root.join("sdks/typescript/src/generated/types.ts"));
    assert!(
        ts_types.contains(&format!(
            "export const CONTRACT_VERSION = \"{cargo_version}\";"
        )),
        "TypeScript generated CONTRACT_VERSION is stale"
    );

    let dependencies = workspace["workspace"]["dependencies"]
        .as_table()
        .expect("workspace dependencies");
    for (name, value) in dependencies {
        if name.starts_with("meerkat") {
            let Some(dep_version) = value.get("version").and_then(|v| v.as_str()) else {
                continue;
            };
            assert_eq!(
                dep_version, cargo_version,
                "internal dependency {name} has stale version"
            );
        }
    }
}

#[test]
fn rpc_catalog_router_docs_and_sdk_wrappers_are_aligned() {
    let root = repo_root();
    let router = read(root.join("meerkat-rpc/src/router.rs"));
    let catalog = read(root.join("meerkat-contracts/src/rpc_catalog.rs"));
    let docs = read(root.join("docs/api/rpc.mdx"));

    let mut router_methods = BTreeSet::new();
    for line in router.lines().filter(|line| line.contains("=>")) {
        if let Some(first) = line.find('"')
            && let Some(second) = line[first + 1..].find('"')
        {
            let method = &line[first + 1..first + 1 + second];
            if method == "initialize"
                || (method.starts_with(char::is_alphabetic) && method.contains('/'))
            {
                router_methods.insert(method.to_string());
            }
        }
    }
    router_methods.remove("initialized");

    // Router-only compatibility shims intentionally remain absent from the public
    // catalog and docs. Keep this in sync with scripts/verify-rpc-surface-alignment.sh.
    router_methods.remove("skills/inspect");

    let mut catalog_methods = BTreeSet::new();
    for needle in [
        "RpcMethodDescriptor::basic(",
        "RpcMethodDescriptor::typed(",
        "RpcMethodDescriptor::params_only(",
        "RpcMethodDescriptor::result_only(",
    ] {
        catalog_methods.extend(quoted_after(&catalog, needle));
    }

    assert_eq!(
        router_methods, catalog_methods,
        "router/catalog RPC method drift"
    );

    let overview =
        find_all_between(&docs, "## Method overview", "## Protocol").expect("RPC method overview");
    let mut docs_methods = BTreeSet::new();
    for line in overview.lines() {
        let line = line.trim_start();
        if !line.starts_with("| `") {
            continue;
        }
        if let Some(first) = line.find('`')
            && let Some(second) = line[first + 1..].find('`')
        {
            let method = &line[first + 1..first + 1 + second];
            if method == "initialize" || method.contains('/') {
                docs_methods.insert(method.to_string());
            }
        }
    }
    assert_eq!(
        docs_methods, catalog_methods,
        "docs/catalog RPC method drift"
    );

    let internal_exclusions: BTreeSet<_> = [
        "initialize",
        "tools/register",
        "session/stream_open",
        "session/stream_close",
        "mob/stream_open",
        "mob/stream_close",
    ]
    .into_iter()
    .collect();
    let mut files = Vec::new();
    walk_files(&root.join("sdks/typescript/src"), &mut files);
    let ts_blob = files
        .iter()
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("ts"))
        .map(read)
        .collect::<Vec<_>>()
        .join("\n");
    files.clear();
    walk_files(&root.join("sdks/python/meerkat"), &mut files);
    let py_blob = files
        .iter()
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("py"))
        .map(read)
        .collect::<Vec<_>>()
        .join("\n");

    let missing_ts: Vec<_> = catalog_methods
        .iter()
        .filter(|method| !internal_exclusions.contains(method.as_str()))
        .filter(|method| !ts_blob.contains(method.as_str()))
        .collect();
    let missing_py: Vec<_> = catalog_methods
        .iter()
        .filter(|method| !internal_exclusions.contains(method.as_str()))
        .filter(|method| !py_blob.contains(method.as_str()))
        .collect();
    assert!(
        missing_ts.is_empty(),
        "TypeScript SDK missing methods: {missing_ts:?}"
    );
    assert!(
        missing_py.is_empty(),
        "Python SDK missing methods: {missing_py:?}"
    );
}

#[test]
fn retired_public_surface_tokens_stay_out_of_unowned_paths() {
    let root = repo_root();
    let terms = [
        "event/push",
        "send_message",
        "send_request",
        "send_response",
        "list_peers",
        "inject_with_subscription",
        "EventInjector",
        "SubscribableInjector",
        "interaction_subscriber",
        "event_injector",
    ];
    let scan_paths = [
        "docs",
        ".claude/skills/meerkat-platform",
        "sdks",
        "CHANGELOG.md",
        "meerkat/src",
        "meerkat-cli",
        "meerkat-core",
        "meerkat-comms",
        "meerkat-rest",
        "meerkat-rpc",
        "meerkat-session",
        "meerkat-tools",
        "meerkat-contracts/src/version.rs",
    ];
    let allowed_prefixes = [
        "docs/",
        ".claude/skills/meerkat-platform/",
        "sdks/",
        "CHANGELOG.md",
        "meerkat/",
        "meerkat-cli/",
        "meerkat-core/",
        "meerkat-comms/",
        "meerkat-rpc/",
        "meerkat-rest/",
        "meerkat-session/",
        "meerkat-tools/",
        "meerkat-contracts/src/version.rs",
    ];

    let mut blocked = Vec::new();
    for scan_path in scan_paths {
        let path = root.join(scan_path);
        let mut files = if path.is_file() {
            vec![path]
        } else {
            Vec::new()
        };
        if root.join(scan_path).is_dir() {
            walk_files(&root.join(scan_path), &mut files);
        }
        for file in files {
            let rel = relative(&root, &file);
            let Some(text) = read_utf8(&file) else {
                continue;
            };
            if terms.iter().any(|term| text.contains(term))
                && !allowed_prefixes
                    .iter()
                    .any(|prefix| rel.starts_with(prefix))
            {
                blocked.push(rel);
            }
        }
    }
    assert!(
        blocked.is_empty(),
        "legacy public-surface tokens outside allowlist: {blocked:?}"
    );
}

#[test]
fn retired_session_control_names_are_absent() {
    let root = repo_root();
    let terms = [
        "session/runtime_state",
        "session/accept_input",
        "session/retire_runtime",
        "session/reset_runtime",
        "session/input_state",
        "session/inputs",
        "/sessions/{id}/runtime-state",
        "/sessions/{id}/accept-input",
        "/sessions/{id}/retire-runtime",
        "/sessions/{id}/reset-runtime",
        "/sessions/{id}/inputs",
        "/sessions/{session_id}/inputs/{input_id}",
    ];
    let scan_paths = [
        "docs",
        "sdks",
        "artifacts",
        "meerkat-contracts",
        "meerkat-rest",
        "meerkat-rpc",
        "tools/sdk-codegen",
    ];
    let mut matches = Vec::new();
    for scan_path in scan_paths {
        let mut files = Vec::new();
        walk_files(&root.join(scan_path), &mut files);
        for file in files {
            let rel = relative(&root, &file);
            if rel.starts_with("docs/dogma-")
                || rel.starts_with("docs/wave-")
                || rel.starts_with("artifacts/")
            {
                continue;
            }
            let Some(text) = read_utf8(&file) else {
                continue;
            };
            for term in terms {
                if text.contains(term) {
                    matches.push(format!("{rel}: {term}"));
                }
            }
        }
    }
    assert!(
        matches.is_empty(),
        "retired session-control names remain: {matches:?}"
    );
}

#[test]
fn deprecated_backend_references_stay_rejected_only() {
    let root = repo_root();
    let mut files = Vec::new();
    walk_files(&root, &mut files);
    let mut matches = Vec::new();
    for file in files {
        let rel = relative(&root, &file);
        if rel == "CHANGELOG.md"
            || rel == "scripts/deprecated_backend_scan.sh"
            || rel.starts_with("artifacts/")
            || rel.starts_with(".rct/")
            || rel.starts_with(".rct-")
            || rel.starts_with(".claude/skills/")
            || rel.contains("/node_modules/")
        {
            continue;
        }
        let Some(text) = read_utf8(&file) else {
            continue;
        };
        if !(text.contains("redb")
            || text.contains("Redb")
            || text.contains("sessions_redb_path")
            || text.contains("memory.redb")
            || text.contains("session_index.redb")
            || text.contains("redb-store"))
        {
            continue;
        }
        let allowed = text.contains("rejects_unsupported_redb_backend")
            || text.contains("\"backend\": \"redb\"")
            || text.contains("redb backend must be rejected")
            || text.contains("unsupported") && text.contains("redb")
            || text.contains("avoids opening redb at")
            || text.contains("only checks the redb store");
        if !allowed {
            matches.push(rel);
        }
    }
    assert!(
        matches.is_empty(),
        "deprecated backend references remain: {matches:?}"
    );
}

#[test]
fn bridge_code_does_not_reinterpret_response_status() {
    let root = repo_root();
    let bridge_files = [
        "meerkat-mob/src/runtime/supervisor_bridge.rs",
        "meerkat-mob/src/runtime/local_bridge.rs",
        "meerkat-contracts/src/wire/supervisor_bridge.rs",
    ];
    let mut violations = Vec::new();
    for rel in bridge_files {
        let text = read(root.join(rel));
        let mut in_test = false;
        for (idx, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("mod tests") {
                in_test = true;
            }
            if in_test || trimmed.starts_with("//") || trimmed.starts_with("///") {
                continue;
            }
            if line.contains("ResponseStatus::Completed")
                || line.contains("ResponseStatus::Failed")
                || line.contains("ResponseStatus::Accepted")
                || (line.contains("match") && line.contains("ResponseStatus"))
            {
                violations.push(format!("{rel}:{}: {line}", idx + 1));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "bridge code reinterprets ResponseStatus directly: {violations:?}"
    );
}

#[test]
fn supervisor_bridge_protocol_version_checks_stay_in_typed_owner() {
    let root = repo_root();
    let owner = "meerkat-contracts/src/wire/supervisor_bridge.rs";
    let source_roots = [
        root.join("meerkat-contracts/src"),
        root.join("meerkat-mob/src"),
        root.join("meerkat-runtime/src"),
    ];
    let mut files = Vec::new();
    for source_root in source_roots {
        walk_files(&source_root, &mut files);
    }

    let mut violations = Vec::new();
    for path in files {
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let rel = relative(&root, &path);
        if rel == owner || rel.ends_with("/tests.rs") || rel.contains("/tests/") {
            continue;
        }
        let text = read(&path);
        let mut in_test = false;
        for (idx, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("mod tests") {
                in_test = true;
            }
            if in_test || trimmed.starts_with("//") || trimmed.starts_with("///") {
                continue;
            }
            let raw_field = line.contains("protocol_version: u32");
            let raw_supported_check =
                line.contains("supervisor_bridge_protocol_version_supported(");
            let raw_ordering_check = line.contains("protocol_version <")
                || line.contains("protocol_version >")
                || line.contains("protocol_version ==")
                || line.contains("protocol_version !=");
            if raw_field || raw_supported_check || raw_ordering_check {
                violations.push(format!("{rel}:{}: {line}", idx + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "supervisor bridge protocol version checks must route through BridgeProtocolVersion in {owner}: {violations:?}"
    );
}

#[test]
fn generated_header_truthfulness_is_clean() {
    let root = repo_root();
    let emit_paths = xtask::audit_generated_headers::live_emit_paths();
    let marker = xtask::audit_generated_headers::generated_marker();
    let mut violations = Vec::new();

    for rel in &emit_paths {
        let path = root.join(rel);
        if !path.exists() {
            continue;
        }
        let contents = read(&path);
        if !contents.lines().take(8).any(|line| line.contains(&marker)) {
            violations.push(format!("missing marker: {}", rel.display()));
        }
    }

    let mut files = Vec::new();
    walk_files(&root, &mut files);
    for path in files {
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let rel = path.strip_prefix(&root).unwrap_or(&path).to_path_buf();
        if emit_paths.contains(&rel) {
            continue;
        }
        let Some(contents) = read_utf8(&path) else {
            continue;
        };
        if contents.lines().take(8).any(|line| line.contains(&marker)) {
            violations.push(format!("forbidden marker: {}", rel.display()));
        }
    }

    assert!(
        violations.is_empty(),
        "generated header audit violations: {violations:?}"
    );
}

#[test]
fn e2e_smoke_lane_launchers_allow_parallel_test_processes() {
    let root = repo_root();
    let cargo_config = read(root.join(".cargo/config.toml"));
    let launcher = read(root.join("scripts/buildbuddy-bazel-poc"));
    let readme = read(root.join("tests/live_smoke/README.md"));

    let smoke_alias = cargo_config
        .lines()
        .find(|line| line.starts_with("e2e-smoke = "))
        .expect("e2e-smoke cargo alias");
    assert!(
        !smoke_alias.contains("--test-threads=1"),
        "cargo e2e-smoke must not serialize the whole smoke lane: {smoke_alias}"
    );

    let smoke_rbe_block =
        find_all_between(&launcher, "e2e-smoke-rbe)", ";;").expect("e2e-smoke-rbe block");
    assert!(
        !smoke_rbe_block.contains("--test_arg=--test-threads=1"),
        "BuildBuddy e2e-smoke-rbe must not serialize the whole smoke lane: {smoke_rbe_block}"
    );

    let serialized_smoke_doc = readme
        .lines()
        .find(|line| line.contains("e2e_smoke_lane") && line.contains("--test-threads=1"));
    assert!(
        serialized_smoke_doc.is_none(),
        "smoke lane docs must not recommend whole-lane serialization: {serialized_smoke_doc:?}"
    );
}
