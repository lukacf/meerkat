use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path
}

fn repo_file(relative: &str) -> String {
    let mut path = repo_root();
    path.push(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

fn try_repo_file(relative: &str) -> Option<String> {
    let mut path = repo_root();
    path.push(relative);
    fs::read_to_string(&path).ok()
}

fn workspace_manifest_for_members(members: &[&str]) -> String {
    let manifest = repo_file("Cargo.toml");
    let start = manifest
        .find("members = [")
        .expect("workspace manifest must declare members");
    let exclude = manifest[start..]
        .find("\nexclude = ")
        .map(|offset| start + offset)
        .expect("workspace manifest must declare exclude after members");
    let members = members
        .iter()
        .map(|member| format!("    \"{member}\",\n"))
        .collect::<String>();
    format!(
        "{}members = [\n{}]\n{}",
        &manifest[..start],
        members,
        &manifest[exclude..]
    )
}

fn copy_dir_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        if source.is_dir() {
            copy_dir_recursive(&source, &destination)?;
        } else {
            fs::copy(&source, &destination)?;
        }
    }
    Ok(())
}

fn find_runfile_tool(root: &Path, suffix: &str) -> Option<PathBuf> {
    let mut queue = VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                queue.push_back(path);
            } else if path.to_string_lossy().ends_with(suffix) {
                return Some(path);
            }
        }
    }
    None
}

fn bazel_cargo_check_env() -> Vec<(&'static str, OsString)> {
    let mut env = Vec::new();
    let Some(runfiles_dir) = std::env::var_os("RUNFILES_DIR").map(PathBuf::from) else {
        return env;
    };

    let manifest_dir = runfiles_dir.join("_main/meerkat").canonicalize().ok();
    if let Some(manifest_dir) = manifest_dir {
        env.push(("CARGO_MANIFEST_DIR", manifest_dir.into_os_string()));
    }

    if std::env::var_os("CARGO").is_none() {
        if let Some(cargo) = find_runfile_tool(&runfiles_dir, "rust_toolchain/bin/cargo") {
            env.push(("CARGO", cargo.into_os_string()));
        } else {
            env.push(("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO", OsString::from("1")));
        }
    }

    if std::env::var_os("RUSTC").is_none()
        && let Some(rustc) = find_runfile_tool(&runfiles_dir, "rust_toolchain/bin/rustc")
    {
        env.push(("RUSTC", rustc.into_os_string()));
    }

    if std::env::var_os("CARGO_HOME").is_none()
        && let Some(cargo_home) = std::env::var_os("MEERKAT_HOST_CARGO_HOME")
    {
        env.push(("CARGO_HOME", cargo_home));
    }

    env
}

fn run_in_configured_bazel_child(
    test_name: &'static str,
    env: Vec<(&'static str, OsString)>,
) -> std::io::Result<bool> {
    if env.is_empty() || std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_ENV_CONFIGURED").is_some() {
        return Ok(false);
    }

    let current_exe = std::env::current_exe()?;
    let status = Command::new(current_exe)
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env("MEERKAT_DOWNSTREAM_CANARY_ENV_CONFIGURED", "1")
        .envs(env)
        .status()?;
    assert!(
        status.success(),
        "configured downstream canary child failed: {status}"
    );
    Ok(true)
}

fn rust_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in
        fs::read_dir(dir).unwrap_or_else(|err| panic!("failed to read {}: {err}", dir.display()))
    {
        let entry = entry.unwrap_or_else(|err| panic!("failed to read dir entry: {err}"));
        let path = entry.path();
        if path.is_dir() {
            rust_files_under(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_test_source(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "tests")
        || path.file_name().is_some_and(|name| name == "tests.rs")
}

fn public_standalone_build_is_test_only(source: &str) -> bool {
    let Some(pos) = source.find("pub async fn build_standalone") else {
        return !source.contains("pub async unsafe fn build_standalone");
    };
    let cfg_window = source[..pos]
        .lines()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .join("\n");
    cfg_window.contains("#[cfg(test)]")
        && !source.contains("pub async unsafe fn build_standalone")
        && !source.contains("#[cfg(all(not(test), feature = \"standalone-agent-builder\"))]")
}

fn public_factory_policy_finalizer_is_absent(source: &str) -> bool {
    !source.contains("pub async fn build_agent_after_factory_policy")
        && !source.contains("pub async unsafe fn build_agent_after_factory_policy")
        && !source.contains("pub unsafe fn build_agent_after_factory_policy")
}

fn factory_authority_crate_exposes_no_minting_api(source: &str) -> bool {
    !source.contains("new_for_agent_factory")
        && !source.contains("export_name")
        && !source.contains("link_name")
        && !source.contains("extern \"Rust\"")
        && !source.contains("#[repr(C)]")
        && !source.contains("#[repr(transparent)]")
        && !source.contains("NonZeroUsize")
        && !source.contains("pub unsafe fn")
        && !source.contains("pub const unsafe fn")
        && !source.contains("pub fn new")
        && !source.contains("pub fn mint")
        && !source.contains("is_canonical_factory_authority")
        && !source.contains("CANONICAL_AUTHORITY")
        && !source.contains("AuthoritySeal")
        && !source.contains("words: [u64; 4]")
        && !source.contains("__meerkat_agent_factory_build_authority_validate")
        && source.matches("pub fn ").count() == 0
        && !source.contains("TypeId")
        && !source.contains("AGENT_FACTORY_BUILD_AUTHORITY_WITNESS_TYPE")
        && !source.contains("AgentFactoryBuildAuthorityRegistration")
        && !source.contains("inventory::collect!")
        && !source.contains("inventory::iter")
}

fn bazel_target_block<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("    name = \"{name}\",");
    let name_pos = source.find(&needle)?;
    let start = source[..name_pos]
        .rfind("\nrust_")
        .map(|pos| pos + 1)
        .unwrap_or(0);
    let end = source[name_pos..]
        .find("\n)\n")
        .map(|offset| name_pos + offset + 3)
        .unwrap_or(source.len());
    Some(&source[start..end])
}

#[test]
fn downstream_safe_code_cannot_forge_factory_policy_finalizer() -> std::io::Result<()> {
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(());
    }

    if run_in_configured_bazel_child(
        "downstream_safe_code_cannot_forge_factory_policy_finalizer",
        bazel_cargo_check_env(),
    )? {
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "agent-builder-policy-downstream"
version = "0.0.0"
edition = "2024"

[dependencies]
meerkat-core = {{ path = "{}" }}
"#,
            repo_root().join("meerkat-core").display()
        ),
    )?;
    fs::write(
        src_dir.join("main.rs"),
        include_str!("fixtures/agent_builder_policy/downstream_forged_factory_policy.rs"),
    )?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .arg("check")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;
    assert!(
        !output.status.success(),
        "downstream core-only fixture unexpectedly compiled; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("build_agent_after_factory_policy")
            && (stderr.contains("this function takes")
                || stderr.contains("unsafe function")
                || stderr.contains("requires unsafe")
                || stderr.contains("expected")
                || stderr.contains("AgentFactoryBuildAuthority")
                || stderr.contains("not found")
                || stderr.contains("cannot find")),
        "downstream fixture failed for the wrong reason:\n{stderr}"
    );
    Ok(())
}

#[test]
fn downstream_meerkat_graph_cannot_forge_factory_policy_finalizer() -> std::io::Result<()> {
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(());
    }

    if run_in_configured_bazel_child(
        "downstream_meerkat_graph_cannot_forge_factory_policy_finalizer",
        bazel_cargo_check_env(),
    )? {
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "agent-builder-policy-downstream-meerkat"
version = "0.0.0"
edition = "2024"

[dependencies]
meerkat = {{ path = "{}", default-features = false }}
meerkat-core = {{ path = "{}" }}
"#,
            repo_root().join("meerkat").display(),
            repo_root().join("meerkat-core").display()
        ),
    )?;
    fs::write(
        src_dir.join("main.rs"),
        include_str!(
            "fixtures/agent_builder_policy/downstream_feature_unified_forged_factory_policy.rs"
        ),
    )?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .arg("check")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;
    assert!(
        !output.status.success(),
        "downstream meerkat + core fixture unexpectedly compiled; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("build_agent_after_factory_policy"),
        "downstream fixture failed for the wrong reason:\n{stderr}"
    );
    Ok(())
}

#[test]
fn downstream_direct_authority_dep_cannot_forge_factory_policy_finalizer() -> std::io::Result<()> {
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(());
    }

    if run_in_configured_bazel_child(
        "downstream_direct_authority_dep_cannot_forge_factory_policy_finalizer",
        bazel_cargo_check_env(),
    )? {
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "agent-builder-policy-downstream-direct-authority"
version = "0.0.0"
edition = "2024"

[dependencies]
meerkat = {{ path = "{}", default-features = false }}
meerkat-agent-build-authority = {{ path = "{}" }}
meerkat-core = {{ path = "{}" }}
"#,
            repo_root().join("meerkat").display(),
            repo_root().join("meerkat-agent-build-authority").display(),
            repo_root().join("meerkat-core").display()
        ),
    )?;
    fs::write(
        src_dir.join("main.rs"),
        include_str!(
            "fixtures/agent_builder_policy/downstream_direct_authority_forged_factory_policy.rs"
        ),
    )?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;
    assert!(
        !output.status.success(),
        "downstream direct-authority fixture unexpectedly compiled; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("__meerkat_agent_factory_build_authority_new")
            || stderr.contains("mint_agent_factory_build_authority")
            || stderr.contains("link_name")
            || stderr.contains("this function takes")
            || stderr.contains("unsafe function")
            || stderr.contains("requires unsafe")
            || stderr.contains("no method named"),
        "downstream direct-authority fixture failed for the wrong reason:\n{stderr}"
    );
    Ok(())
}

#[test]
fn downstream_unsafe_code_cannot_enter_factory_policy_finalizer() -> std::io::Result<()> {
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(());
    }

    if run_in_configured_bazel_child(
        "downstream_unsafe_code_cannot_enter_factory_policy_finalizer",
        bazel_cargo_check_env(),
    )? {
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "agent-builder-policy-downstream-unsafe-finalizer"
version = "0.0.0"
edition = "2024"

[dependencies]
async-trait = "0.1"
futures = "0.3"
inventory = "0.3"
meerkat-core = {{ path = "{}" }}
"#,
            repo_root().join("meerkat-core").display()
        ),
    )?;
    fs::write(
        src_dir.join("main.rs"),
        include_str!("fixtures/agent_builder_policy/downstream_unsafe_factory_policy_finalizer.rs"),
    )?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("unsafe downstream finalizer rejected forged bridge token"),
            "downstream unsafe finalizer fixture passed for the wrong reason; stdout:\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(());
    }
    assert!(
        !String::from_utf8_lossy(&output.stderr)
            .contains("unsafe downstream finalizer call constructed an agent"),
        "downstream unsafe finalizer reached the live factory-policy bridge and constructed an agent:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AgentFactoryPolicyBridgeRegistration")
            || stderr.contains("__meerkat_agent_factory_policy_build_v3")
            || stderr.contains("exported_agent_factory_policy_build")
            || stderr.contains("link_name")
            || stderr.contains("couldn't read")
            || stderr.contains("No such file or directory"),
        "downstream unsafe finalizer fixture failed for the wrong reason:\n{stderr}"
    );
    Ok(())
}

#[test]
fn downstream_package_spoof_cannot_enter_factory_policy_finalizer() -> std::io::Result<()> {
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(());
    }

    if run_in_configured_bazel_child(
        "downstream_package_spoof_cannot_enter_factory_policy_finalizer",
        bazel_cargo_check_env(),
    )? {
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let workspace_dir = temp.path().join("repo");
    fs::create_dir_all(&workspace_dir)?;
    fs::write(
        workspace_dir.join("Cargo.toml"),
        workspace_manifest_for_members(&["meerkat-core", "meerkat"]),
    )?;
    copy_dir_recursive(
        &repo_root().join("meerkat-core"),
        &workspace_dir.join("meerkat-core"),
    )?;

    let project_dir = workspace_dir.join("meerkat");
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_dir.join("Cargo.toml"),
        r#"[package]
name = "meerkat"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
homepage.workspace = true
documentation.workspace = true
keywords.workspace = true
categories.workspace = true

[dependencies]
async-trait = { workspace = true }
futures = { workspace = true }
inventory = { workspace = true }
meerkat-core = { path = "../meerkat-core" }
"#,
    )?;
    fs::write(
        src_dir.join("main.rs"),
        "mod factory;\nfn main() { factory::run(); }\n",
    )?;
    let fixture =
        include_str!("fixtures/agent_builder_policy/downstream_unsafe_factory_policy_finalizer.rs")
            .replace(
                "fn main() {",
                r#"pub fn run() {
    std::fs::remove_file(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/factory.rs"))
        .expect("remove spoofed factory source before bridge validation");
"#,
            );
    fs::write(src_dir.join("factory.rs"), fixture)?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(project_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("unsafe downstream finalizer rejected forged bridge token"),
            "downstream package-spoof finalizer fixture passed for the wrong reason; stdout:\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(());
    }
    assert!(
        !String::from_utf8_lossy(&output.stderr)
            .contains("unsafe downstream finalizer call constructed an agent"),
        "downstream package-spoof finalizer reached the live factory-policy bridge and constructed an agent:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn downstream_direct_bridge_registration_cannot_build_agent() -> std::io::Result<()> {
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(());
    }

    if run_in_configured_bazel_child(
        "downstream_direct_bridge_registration_cannot_build_agent",
        bazel_cargo_check_env(),
    )? {
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let workspace_dir = temp.path().join("repo");
    fs::create_dir_all(&workspace_dir)?;
    fs::write(
        workspace_dir.join("Cargo.toml"),
        workspace_manifest_for_members(&["meerkat-core", "meerkat"]),
    )?;
    copy_dir_recursive(
        &repo_root().join("meerkat-core"),
        &workspace_dir.join("meerkat-core"),
    )?;

    let project_dir = workspace_dir.join("meerkat");
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_dir.join("Cargo.toml"),
        r#"[package]
name = "meerkat"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
homepage.workspace = true
documentation.workspace = true
keywords.workspace = true
categories.workspace = true

[dependencies]
async-trait = { workspace = true }
futures = { workspace = true }
inventory = { workspace = true }
meerkat-core = { path = "../meerkat-core" }
"#,
    )?;
    fs::write(
        src_dir.join("main.rs"),
        "mod factory;\nfn main() { factory::run(); }\n",
    )?;
    let fixture =
        include_str!("fixtures/agent_builder_policy/downstream_unsafe_factory_policy_finalizer.rs")
            .replace("fn main() {", "pub fn run() {");
    let fixture = fixture.replace(
        r#"inventory::submit! {
    meerkat_core::__meerkat_agent_factory_policy_bridge_registration!(
        forged_agent_factory_policy_bridge_token_type_id
    )
}

"#,
        r#"inventory::submit! {
    meerkat_core::agent::AgentFactoryPolicyBridgeRegistration::__facade_from_compile_env(
        "meerkat",
        env!("CARGO_MANIFEST_DIR"),
        0xa9de_0aae_2b8a_98aa,
        forged_agent_factory_policy_bridge_token_type_id,
    )
}

"#,
    );
    fs::write(src_dir.join("factory.rs"), fixture)?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(project_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;
    assert!(
        output.status.success(),
        "downstream direct bridge-registration fixture must compile and run to bridge rejection; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("unsafe downstream finalizer rejected forged bridge token"),
        "downstream direct bridge-registration fixture passed for the wrong reason; stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn downstream_direct_authority_dep_cannot_transmute_factory_policy_finalizer() -> std::io::Result<()>
{
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(());
    }

    if run_in_configured_bazel_child(
        "downstream_direct_authority_dep_cannot_transmute_factory_policy_finalizer",
        bazel_cargo_check_env(),
    )? {
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "agent-builder-policy-downstream-direct-authority-transmute"
version = "0.0.0"
edition = "2024"

[dependencies]
meerkat = {{ path = "{}", default-features = false }}
meerkat-agent-build-authority = {{ path = "{}" }}
meerkat-core = {{ path = "{}" }}
"#,
            repo_root().join("meerkat").display(),
            repo_root().join("meerkat-agent-build-authority").display(),
            repo_root().join("meerkat-core").display()
        ),
    )?;
    fs::write(
        src_dir.join("main.rs"),
        include_str!(
            "fixtures/agent_builder_policy/downstream_transmute_authority_forged_factory_policy.rs"
        ),
    )?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;
    assert!(
        !output.status.success(),
        "downstream direct-authority transmute fixture unexpectedly compiled and ran; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("transmute")
            || stderr.contains("is_canonical_factory_authority")
            || stderr.contains("this function takes")
            || stderr.contains("unsafe function")
            || stderr.contains("requires unsafe")
            || stderr.contains("no method named")
            || stderr.contains("InvalidBuildAuthority")
            || stderr.contains("assertion failed"),
        "downstream direct-authority transmute fixture failed for the wrong reason:\n{stderr}"
    );
    Ok(())
}

#[test]
fn downstream_direct_authority_dep_cannot_spoof_validator_symbol_for_factory_policy_finalizer()
-> std::io::Result<()> {
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(());
    }

    if run_in_configured_bazel_child(
        "downstream_direct_authority_dep_cannot_spoof_validator_symbol_for_factory_policy_finalizer",
        bazel_cargo_check_env(),
    )? {
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "agent-builder-policy-downstream-validator-symbol"
version = "0.0.0"
edition = "2024"

[dependencies]
meerkat-agent-build-authority = {{ path = "{}" }}
meerkat-core = {{ path = "{}" }}
"#,
            repo_root().join("meerkat-agent-build-authority").display(),
            repo_root().join("meerkat-core").display()
        ),
    )?;
    fs::write(
        src_dir.join("main.rs"),
        include_str!(
            "fixtures/agent_builder_policy/downstream_validator_symbol_forged_factory_policy.rs"
        ),
    )?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;
    assert!(
        !output.status.success(),
        "downstream validator-symbol fixture unexpectedly compiled and ran; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        (stderr.contains("assertion failed") && stderr.contains("is_canonical_factory_authority"))
            || stderr.contains("InvalidBuildAuthority")
            || stderr.contains("this function takes")
            || stderr.contains("unsafe function")
            || stderr.contains("requires unsafe")
            || stderr.contains("no method named"),
        "downstream validator-symbol fixture failed for the wrong reason:\n{stderr}"
    );
    Ok(())
}

#[test]
fn downstream_cannot_feature_enable_standalone_builder_bypass() -> std::io::Result<()> {
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(());
    }

    if run_in_configured_bazel_child(
        "downstream_cannot_feature_enable_standalone_builder_bypass",
        bazel_cargo_check_env(),
    )? {
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "agent-builder-policy-downstream-standalone-feature"
version = "0.0.0"
edition = "2024"

[dependencies]
meerkat-core = {{ path = "{}", features = ["standalone-agent-builder"] }}
"#,
            repo_root().join("meerkat-core").display()
        ),
    )?;
    fs::write(
        src_dir.join("main.rs"),
        include_str!("fixtures/agent_builder_policy/downstream_standalone_feature_build.rs"),
    )?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .arg("check")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;
    assert!(
        !output.status.success(),
        "downstream standalone-feature fixture unexpectedly compiled; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("standalone-agent-builder")
            || stderr.contains("build_standalone")
            || stderr.contains("this function takes")
            || stderr.contains("unsafe function")
            || stderr.contains("requires unsafe")
            || stderr.contains("feature"),
        "downstream standalone-feature fixture failed for the wrong reason:\n{stderr}"
    );
    Ok(())
}

#[test]
fn downstream_direct_authority_dep_cannot_mirror_factory_policy_finalizer() -> std::io::Result<()> {
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(());
    }

    if run_in_configured_bazel_child(
        "downstream_direct_authority_dep_cannot_mirror_factory_policy_finalizer",
        bazel_cargo_check_env(),
    )? {
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "agent-builder-policy-downstream-direct-authority-mirror"
version = "0.0.0"
edition = "2024"

[dependencies]
meerkat = {{ path = "{}", default-features = false }}
meerkat-agent-build-authority = {{ path = "{}" }}
meerkat-core = {{ path = "{}" }}
inventory = "0.3"
"#,
            repo_root().join("meerkat").display(),
            repo_root().join("meerkat-agent-build-authority").display(),
            repo_root().join("meerkat-core").display()
        ),
    )?;
    fs::write(
        src_dir.join("main.rs"),
        include_str!(
            "fixtures/agent_builder_policy/downstream_mirror_authority_forged_factory_policy.rs"
        ),
    )?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;
    assert!(
        !output.status.success(),
        "downstream direct-authority mirror fixture unexpectedly compiled and ran; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("assertion failed")
            || stderr.contains("InvalidBuildAuthority")
            || stderr.contains("is_canonical_factory_authority")
            || stderr.contains("this function takes")
            || stderr.contains("unsafe function")
            || stderr.contains("requires unsafe")
            || stderr.contains("no method named")
            || stderr.contains("cannot transmute between types of different sizes")
            || stderr.contains("cannot find type")
            || stderr.contains("not found in")
            || stderr.contains("private"),
        "downstream direct-authority mirror fixture failed for the wrong reason:\n{stderr}"
    );
    Ok(())
}

#[test]
fn downstream_direct_authority_dep_cannot_steal_inventory_factory_policy_finalizer()
-> std::io::Result<()> {
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(());
    }

    if run_in_configured_bazel_child(
        "downstream_direct_authority_dep_cannot_steal_inventory_factory_policy_finalizer",
        bazel_cargo_check_env(),
    )? {
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "agent-builder-policy-downstream-direct-authority-inventory-steal"
version = "0.0.0"
edition = "2024"

[dependencies]
meerkat = {{ path = "{}", default-features = false }}
meerkat-agent-build-authority = {{ path = "{}" }}
meerkat-core = {{ path = "{}" }}
inventory = "0.3"
"#,
            repo_root().join("meerkat").display(),
            repo_root().join("meerkat-agent-build-authority").display(),
            repo_root().join("meerkat-core").display()
        ),
    )?;
    fs::write(
        src_dir.join("main.rs"),
        include_str!(
            "fixtures/agent_builder_policy/downstream_inventory_steal_authority_forged_factory_policy.rs"
        ),
    )?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;
    assert!(
        !output.status.success(),
        "downstream inventory-steal fixture unexpectedly compiled and ran; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("assertion failed")
            || stderr.contains("InvalidBuildAuthority")
            || stderr.contains("is_canonical_factory_authority")
            || stderr.contains("this function takes")
            || stderr.contains("unsafe function")
            || stderr.contains("requires unsafe")
            || stderr.contains("no method named")
            || stderr.contains("cannot find type")
            || stderr.contains("not found in")
            || stderr.contains("private"),
        "downstream inventory-steal fixture failed for the wrong reason:\n{stderr}"
    );
    Ok(())
}

#[test]
fn downstream_direct_authority_dep_without_facade_cannot_register_factory_policy_finalizer()
-> std::io::Result<()> {
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(());
    }

    if run_in_configured_bazel_child(
        "downstream_direct_authority_dep_without_facade_cannot_register_factory_policy_finalizer",
        bazel_cargo_check_env(),
    )? {
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "agent-builder-policy-downstream-direct-authority-no-facade"
version = "0.0.0"
edition = "2024"

[dependencies]
meerkat-agent-build-authority = {{ path = "{}" }}
meerkat-core = {{ path = "{}" }}
inventory = "0.3"
"#,
            repo_root().join("meerkat-agent-build-authority").display(),
            repo_root().join("meerkat-core").display()
        ),
    )?;
    fs::write(
        src_dir.join("main.rs"),
        include_str!(
            "fixtures/agent_builder_policy/downstream_no_facade_registration_forged_factory_policy.rs"
        ),
    )?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;
    assert!(
        !output.status.success(),
        "downstream no-facade forged-registration fixture unexpectedly compiled and ran; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("assertion failed")
            || stderr.contains("InvalidBuildAuthority")
            || stderr.contains("is_canonical_factory_authority")
            || stderr.contains("this function takes")
            || stderr.contains("unsafe function")
            || stderr.contains("requires unsafe")
            || stderr.contains("no method named")
            || stderr.contains("cannot find type")
            || stderr.contains("not found in")
            || stderr.contains("private"),
        "downstream no-facade fixture failed for the wrong reason:\n{stderr}"
    );
    Ok(())
}

#[test]
fn core_agent_builder_does_not_expose_public_build_bypass() {
    let builder = repo_file("meerkat-core/src/agent/builder.rs");

    assert!(
        !builder.contains("pub async fn build<"),
        "meerkat_core::AgentBuilder must not expose a public unqualified \
         build(client, tools, store) seam; production-facing construction must \
         route through AgentFactory policy or an explicitly named low-level seam"
    );
    assert!(
        public_standalone_build_is_test_only(&builder),
        "meerkat_core::AgentBuilder must not expose public standalone \
         build(client, tools, store) outside core tests; low-level standalone \
         construction must not be a downstream feature opt-in"
    );
    assert!(
        !builder.contains("pub async unsafe fn build_standalone"),
        "the standalone builder must not be a public unsafe feature-gated \
         escape hatch; downstream unsafe code can opt into public features"
    );
    assert!(
        !builder.contains("StandaloneAgentBuildAuthority"),
        "the standalone feature must not pretend a private zero-sized argument \
         is an authority; downstream unsafe code can fabricate inferred private \
         argument types"
    );
    assert!(
        public_factory_policy_finalizer_is_absent(&builder),
        "meerkat_core must not expose a public Rust \
         build_agent_after_factory_policy function; downstream callers can \
         otherwise reach the factory-policy finalizer through doc-hidden paths"
    );
    assert!(
        !builder.contains("pub async fn build_after_factory_policy"),
        "meerkat_core::AgentBuilder must not expose a public factory-policy \
         build method; canonical factory authority must live outside the \
         public builder impl"
    );
    assert!(
        builder.contains("validate_factory_policy()?")
            && builder.contains("validate_factory_bridge_token(factory_bridge_token)?")
            && builder.contains("AgentFactoryPolicyBridgeRegistration")
            && builder.contains("__meerkat_agent_factory_policy_bridge_registration")
            && builder.contains("InvalidFactoryBridgeToken")
            && builder.contains("ForgedFactoryBridgeTokenRegistration")
            && builder.contains("FACADE_FACTORY_SOURCE_FINGERPRINT")
            && builder.contains("__facade_from_compile_env")
            && builder.contains("__meerkat_agent_factory_policy_source_fingerprint")
            && builder.contains("source_content_fingerprint_matches")
            && builder.contains("source_file_content_fingerprint")
            && builder.contains("core::panic::Location::caller")
            && builder.contains("source_file_matches")
            && builder.contains("inventory::collect!(AgentFactoryPolicyBridgeRegistration)")
            && !builder.contains("AgentFactoryPolicyBridgeRegistration::new")
            && !builder.contains("pub const fn new(")
            && !builder.contains("pub const fn __from_compile_env")
            && !builder.contains("CoreHooksTest")
            && !builder.contains("__core_hooks_test_from_compile_env")
            && !builder
                .contains("__meerkat_core_hooks_test_agent_factory_policy_bridge_registration")
            && !builder.contains(".is_none_or(")
            && !builder.contains("pub enum AgentFactoryPolicyBridgeRegistrationKind")
            && !builder.contains("MEERKAT_AGENT_FACTORY_POLICY_BUILD_SYMBOL")
            && !builder.contains("MEERKAT_AGENT_FACTORY_POLICY_BUILD_PROOF")
            && !builder.contains("__meerkat_core_agent_factory_policy_build")
            && builder.contains("pub(crate) unsafe extern \"Rust\" fn")
            && !builder.contains("is_canonical_factory_authority()")
            && !builder.contains("AgentBuildPolicyError::InvalidBuildAuthority")
            && !builder.contains("pub fn from_registered_source<A: 'static>")
            && !builder.contains("pub const fn canonical_factory")
            && !builder.contains("pub const fn test_harness")
            && !builder.contains("pub struct AgentFactoryPolicyAuthorityRegistration")
            && !builder.contains("pub enum AgentFactoryPolicyAuthorityRegistrationKind"),
        "canonical factory construction must cross into core through a \
         private validating factory-policy seam that does not expose a \
         downstream Rust registration or authority-minting API"
    );
    assert!(
        !builder.contains("std::panic::Location::caller")
            && !builder.contains("validate_factory_policy_caller")
            && !builder.contains("canonical_factory_source_path")
            && !builder.contains("workspace_source_path")
            && !builder.contains("normalize_source_path"),
        "core factory authority must not trust runtime source file paths; \
         packaged/vendored Cargo layouts change source paths and downstream \
         callers can shape source locations"
    );
    assert!(
        !builder.contains("pub struct AgentFactoryBuildToken")
            && !builder.contains("new_unchecked_for_canonical_factory"),
        "factory build authority must not be publicly mintable from \
         meerkat_core::AgentBuilder"
    );
}

#[test]
fn core_factory_authority_token_is_not_reexported() {
    let agent_mod = repo_file("meerkat-core/src/agent.rs");
    let lib = repo_file("meerkat-core/src/lib.rs");
    let facade_lib = repo_file("meerkat/src/lib.rs");
    let authority = repo_file("meerkat-agent-build-authority/src/lib.rs");

    assert!(
        !agent_mod.contains("pub use builder::build_agent_after_factory_policy")
            && !agent_mod.contains("__agent_factory_build_bridge"),
        "meerkat_core::agent must not expose a public doc-hidden bridge for \
         the factory-policy finalizer; downstream callers can name doc-hidden \
         public modules"
    );
    assert!(
        !agent_mod.contains("AgentFactoryBuildToken"),
        "meerkat_core::agent must not re-export a factory token that downstream \
         crates can mint or pass"
    );
    assert!(
        !agent_mod.contains("AgentFactoryBuildAuthority"),
        "meerkat_core::agent must not re-export the concrete factory build \
         authority; downstream crates must not obtain it from core feature \
         unification"
    );
    assert!(
        !lib.contains("AgentFactoryBuildToken"),
        "meerkat_core must not re-export a factory token that downstream crates \
         can mint or pass"
    );
    assert!(
        !lib.contains("AgentFactoryBuildAuthority"),
        "meerkat_core must not re-export the concrete factory build authority \
         that the facade owns through a separate direct dependency"
    );
    assert!(
        !agent_mod.contains("AgentFactoryPolicyAuthorityRegistration")
            && !agent_mod.contains("AgentFactoryPolicyAuthorityRegistrationKind")
            && !agent_mod.contains("AgentFactoryPolicyAuthority,"),
        "meerkat_core::agent must not re-export factory authority registration \
         or minting types that downstream crates can use to bypass AgentFactory"
    );
    assert!(
        !lib.contains("AgentFactoryPolicyAuthorityRegistration")
            && !lib.contains("AgentFactoryPolicyAuthority,"),
        "meerkat_core must not re-export factory authority registration or \
         minting types that downstream crates can use to bypass AgentFactory"
    );
    assert!(
        !facade_lib.contains("CoreAgentBuilder") && !facade_lib.contains("StandaloneAgentBuilder"),
        "meerkat facade must not publicly re-export the core builder as a \
         standalone construction shortcut"
    );
    assert!(
        factory_authority_crate_exposes_no_minting_api(&authority),
        "direct dependency on the authority crate must not provide any public \
         constructor or minting helper that downstream code can use to call the \
         core finalizer, including public unsafe helpers"
    );
}

#[test]
fn bazel_factory_authority_target_is_not_publicly_visible() {
    let Some(authority_bazel) = try_repo_file("meerkat-agent-build-authority/BUILD.bazel") else {
        // Cargo-only source layouts may not include generated Bazel files.
        return;
    };
    let authority_library = bazel_target_block(&authority_bazel, "meerkat_agent_build_authority")
        .expect("authority BUILD.bazel must contain its rust_library target");

    assert!(
        !authority_library.contains("//visibility:public"),
        "the Bazel authority rust_library must not be public; otherwise a \
         downstream Bazel target can depend on the public core target, add the \
         authority target directly, and call the factory-policy finalizer"
    );
    for label in ["//:__pkg__", "//meerkat-core:__pkg__", "//meerkat:__pkg__"] {
        assert!(
            authority_library.contains(label),
            "authority target visibility must still allow the canonical \
             facade/core bridge: missing {label}"
        );
    }
}

#[test]
fn authority_build_scripts_do_not_leak_factory_seal_metadata() {
    let authority_build = repo_file("meerkat-agent-build-authority/build.rs");
    let core_build = try_repo_file("meerkat-core/build.rs");
    let facade_build = try_repo_file("meerkat/build.rs");

    assert!(
        !authority_build.contains("cargo:metadata=")
            && !authority_build.contains("cargo:word_")
            && !authority_build.contains("MEERKAT_AGENT_BUILD_AUTHORITY_WORD_"),
        "authority build script must not publish canonical factory authority \
         seal words through Cargo metadata or rustc environment"
    );
    if let Some(facade_build) = facade_build.as_ref() {
        assert!(
            !facade_build.contains("DEP_MEERKAT_AGENT_BUILD_AUTHORITY_WORD_")
                && !facade_build.contains("MEERKAT_AGENT_BUILD_AUTHORITY_WORD_"),
            "facade build script must not consume leaked authority seal words \
             from dependency metadata; direct downstream dependents can read \
             the same DEP_* values"
        );
    }
    for (name, build_script) in [
        ("meerkat-core/build.rs", core_build.as_deref()),
        ("meerkat/build.rs", facade_build.as_deref()),
    ] {
        let Some(build_script) = build_script else {
            continue;
        };
        assert!(
            !build_script.contains("rerun-if-env-changed={SYMBOL_ENV}")
                && !build_script.contains("env::var(SYMBOL_ENV)")
                && !build_script
                    .contains("env::var(\"MEERKAT_AGENT_FACTORY_POLICY_BUILD_SYMBOL\")"),
            "{name} must not accept caller-supplied factory bridge symbols; a \
             downstream build can otherwise choose the exported symbol and link \
             the core finalizer directly"
        );
    }
}

#[test]
fn facade_cargo_does_not_feature_unify_standalone_builder_by_default() {
    let cargo = repo_file("meerkat/Cargo.toml");

    assert!(
        !cargo.contains("meerkat-core/standalone-agent-builder"),
        "the facade Cargo manifest must not expose any public feature that \
         enables meerkat-core/standalone-agent-builder through feature \
         unification"
    );
    for line in cargo.lines() {
        if line.trim_start().starts_with("meerkat-core") {
            assert!(
                !line.contains("standalone-agent-builder"),
                "the facade Cargo graph must not enable \
                 meerkat-core/standalone-agent-builder through its normal or \
                 dev meerkat-core dependency"
            );
        }
    }
}

#[test]
fn public_bazel_core_target_does_not_expose_build_bypass_features() {
    let Some(core_bazel) = try_repo_file("meerkat-core/BUILD.bazel") else {
        // Cargo-only source layouts may not include generated Bazel files.
        return;
    };
    let public_core = bazel_target_block(&core_bazel, "meerkat_core")
        .expect("meerkat-core BUILD.bazel must contain public core rust_library");

    assert!(
        public_core.contains("visibility = [\"//visibility:public\"]"),
        "the canonical public Bazel core library must remain public"
    );
    assert!(
        !public_core.contains("\"internal-agent-factory-build\"")
            && !public_core.contains("\"standalone-agent-builder\"")
            && !public_core.contains("MEERKAT_AGENT_FACTORY_POLICY_BUILD_SYMBOL")
            && !public_core.contains("MEERKAT_AGENT_FACTORY_POLICY_BUILD_PROOF"),
        "public //meerkat-core:meerkat_core must not expose standalone build \
         features, internal feature selectors, or recoverable factory bridge \
         proof material in Bazel builds"
    );

    assert!(
        bazel_target_block(&core_bazel, "meerkat_core_agent_factory_build").is_none(),
        "Bazel must not publish a second core crate graph for AgentFactory; \
         split core graphs make public API types diverge"
    );
}

#[test]
fn ordinary_bazel_core_dependents_do_not_use_internal_factory_variant() {
    for (build_file, target) in [
        ("meerkat-comms/BUILD.bazel", "meerkat_comms"),
        ("meerkat-rest/BUILD.bazel", "meerkat_rest"),
        ("meerkat-rpc/BUILD.bazel", "meerkat_rpc"),
        ("meerkat-runtime/BUILD.bazel", "meerkat_runtime"),
    ] {
        let Some(bazel) = try_repo_file(build_file) else {
            // Cargo-only source layouts may not include generated Bazel files.
            continue;
        };
        let library = bazel_target_block(&bazel, target)
            .unwrap_or_else(|| panic!("{build_file} must contain {target} rust_library"));

        assert!(
            library.contains("\"//meerkat-core:meerkat_core\""),
            "{target} must depend on the public core Bazel target, not the \
             internal factory-build variant"
        );
        assert!(
            !library.contains("meerkat_core_agent_factory_build"),
            "{target} must not expose the internal AgentFactory core variant \
             through an ordinary public production target"
        );
    }
}

#[test]
fn core_factory_authority_is_not_publicly_forgeable() {
    let builder = repo_file("meerkat-core/src/agent/builder.rs");
    let agent_mod = repo_file("meerkat-core/src/agent.rs");
    let factory = repo_file("meerkat/src/factory.rs");

    assert!(
        !builder.contains("std::any::type_name")
            && !builder.contains("CANONICAL_AGENT_FACTORY_POLICY_AUTHORITY_TYPE")
            && !builder.contains("TEST_AGENT_FACTORY_POLICY_AUTHORITY_TYPES")
            && !builder.contains("meerkat::factory::AgentFactoryPolicyAuthority"),
        "core factory authority must not trust stringified Rust type paths; a \
         downstream crate can spoof a module/type path and satisfy a \
         type_name-based public authority check"
    );
    assert!(
        !builder.contains("pub trait AgentFactoryPolicyAuthority"),
        "core factory authority must not be a public trait; downstream crates \
         can implement public traits and mint authority unless the seam is \
         tied to a non-forgeable factory-owned value"
    );
    assert!(
        !builder.contains("pub fn agent_factory_policy_authority")
            && !builder.contains("pub const fn new_unchecked")
            && !builder.contains("pub fn new_unchecked")
            && !builder.contains("pub fn from_registered_source")
            && !builder.contains("pub const fn canonical_factory")
            && !builder.contains("pub const fn test_harness")
            && !builder.contains("inventory::collect!(AgentFactoryPolicyAuthorityRegistration)"),
        "core factory authority must not expose a safe public minting helper \
         or unchecked constructor; downstream callers can otherwise mint the \
         opaque authority and call the factory-policy seam after synthesizing \
         public metadata"
    );
    assert!(
        !builder
            .contains("__agent_factory_policy_authority(&self) -> factory_policy_private::Seal {"),
        "factory authority must not provide a default seal method; default \
         trait methods make authority forgeable by empty downstream impls"
    );
    assert!(
        !agent_mod.contains("agent_factory_policy_authority"),
        "meerkat_core::agent must not re-export a safe public factory-authority \
         minting helper that downstream crates can call"
    );
    assert!(
        !factory.contains("impl meerkat_core::agent::AgentFactoryPolicyAuthority for AgentFactory"),
        "AgentFactory should own a non-forgeable authority value rather than \
         implementing a public core authority trait"
    );
}

#[test]
fn production_crates_do_not_adopt_standalone_builder_seam() {
    let root = repo_root();
    let mut production_files = Vec::new();
    let mut scanned_crates = BTreeSet::new();

    for entry in
        fs::read_dir(&root).unwrap_or_else(|err| panic!("failed to read {}: {err}", root.display()))
    {
        let entry = entry.unwrap_or_else(|err| panic!("failed to read repo entry: {err}"));
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("meerkat-") || name == "meerkat-core" {
            continue;
        }
        let src = path.join("src");
        if src.is_dir() {
            scanned_crates.insert(name.to_string());
            rust_files_under(&src, &mut production_files);
        }
    }
    scanned_crates.insert("meerkat".to_string());
    rust_files_under(&root.join("meerkat/src"), &mut production_files);

    for required in ["meerkat-runtime", "meerkat-rest", "meerkat-rpc"] {
        assert!(
            scanned_crates.contains(required),
            "production canary did not scan {required}; ensure Cargo and \
             Bazel/BuildBuddy runfiles include the intended production crate set"
        );
    }

    for path in production_files {
        if is_test_source(&path) {
            continue;
        }
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        assert!(
            !source.contains(".build_standalone("),
            "production source must not bypass AgentFactory via build_standalone: {}",
            path.display()
        );

        assert!(
            !source.contains("build_with_factory_policy(")
                && !source.contains("new_unchecked_for_canonical_factory")
                && !source.contains("agent_factory_policy_authority("),
            "production source must not depend on a publicly mintable \
             core factory-token seam: {}",
            path.display()
        );
        if path != root.join("meerkat/src/factory.rs") {
            assert!(
                !source.contains("build_agent_after_factory_policy")
                    && !source.contains("build_after_factory_policy"),
                "production source must not bypass or alias AgentFactory through \
                 the core factory-authority seam: {}",
                path.display()
            );
            assert!(
                !source.contains("AgentFactoryPolicyAuthority"),
                "production source must not mint core factory authority outside \
                 AgentFactory: {}",
                path.display()
            );
        }
    }
}

#[test]
fn bazel_canary_runfiles_include_required_production_crates() {
    let Some(bazel) = try_repo_file("meerkat/BUILD.bazel") else {
        // Cargo-equivalent feature-matrix harnesses may execute this package
        // test from a reduced source layout without generated Bazel metadata.
        // The ordinary Cargo canary and Bazel unit lanes include this file.
        return;
    };

    assert!(
        bazel.contains("//:workspace_runfiles"),
        "Bazel canary must include workspace runfiles so BuildBuddy sees the \
         same source tree as Cargo"
    );

    for label in [
        "//meerkat-runtime:package_runfiles",
        "//meerkat-rest:package_runfiles",
        "//meerkat-rpc:package_runfiles",
    ] {
        assert!(
            bazel.contains(label),
            "Bazel canary must explicitly include {label} so BuildBuddy scans \
             production crates that can host factory bypasses"
        );
    }
}

#[test]
fn bazel_canary_runs_in_required_ci_lanes() {
    let Some(bazel) = try_repo_file("meerkat/BUILD.bazel") else {
        return;
    };
    let canary = bazel_target_block(&bazel, "agent_builder_policy_canary_test")
        .expect("meerkat BUILD.bazel must contain the agent builder policy canary");

    assert!(
        canary.contains("\"fast\"") && canary.contains("\"unit\""),
        "agent_builder_policy_canary_test must be tagged for required \
         BuildBuddy CI selection, not only ad-hoc fast lanes"
    );
}

#[test]
fn production_like_callers_do_not_call_core_builder_build_directly() {
    let factory = repo_file("meerkat/src/factory.rs");
    let comms_agent = repo_file("meerkat-comms/src/agent/mod.rs");

    assert!(
        !factory.contains("builder.build(llm_adapter, tools, store_adapter)"),
        "AgentFactory must not enter meerkat_core through the removed \
         unqualified build seam"
    );
    assert!(
        !factory.contains(".build_standalone("),
        "AgentFactory must not enter meerkat_core through the standalone \
         test/embedding seam"
    );
    assert!(
        factory.contains("core_agent_factory_policy_build(")
            && factory.contains("agent_factory_policy_bridge_token()")
            && !factory.contains("agent_factory_policy_authority()")
            && !factory.contains(".build_after_factory_policy("),
        "AgentFactory must enter meerkat_core through the explicit \
         factory-policy seam with its private bridge token rather than a \
         public AgentBuilder method or publicly mintable authority helper"
    );
    assert!(
        !factory.contains("struct AgentFactoryPolicyAuthority")
            && !factory.contains("pub struct AgentFactoryPolicyAuthority"),
        "AgentFactory must not mint a facade-local authority type that core \
         can only validate through spoofable type metadata"
    );
    assert!(
        !comms_agent.contains("pub struct CommsAgentBuilder"),
        "meerkat-comms must not expose a public builder that can bypass AgentFactory policy"
    );
    assert!(
        !comms_agent.contains(".build(client, tools, store)"),
        "meerkat-comms must not construct core agents through the unqualified build seam"
    );
}
