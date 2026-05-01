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

fn public_standalone_build_is_cfg_gated(source: &str) -> bool {
    let Some(pos) = source.find("pub async fn build_standalone") else {
        return true;
    };
    let cfg_window = source[..pos]
        .lines()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .join("\n");
    cfg_window.contains(r#"#[cfg(any(test, feature = "standalone-agent-builder"))]"#)
}

fn public_factory_policy_finalizer_requires_authority_before_builder(source: &str) -> bool {
    let Some(pos) = source.find("pub async fn build_agent_after_factory_policy") else {
        return true;
    };
    let signature_window = source[pos..].lines().take(8).collect::<Vec<_>>().join("\n");
    signature_window.contains("meerkat_agent_build_authority::AgentFactoryBuildAuthority")
        && signature_window.find("authority:").unwrap_or(usize::MAX)
            < signature_window.find("builder: AgentBuilder").unwrap_or(0)
}

fn public_factory_policy_finalizer_requires_typed_authority(source: &str) -> bool {
    let Some(pos) = source.find("pub async fn build_agent_after_factory_policy") else {
        return true;
    };
    let signature_window = source[pos..].lines().take(8).collect::<Vec<_>>().join("\n");
    signature_window.contains("meerkat_agent_build_authority::AgentFactoryBuildAuthority")
}

fn factory_authority_crate_exposes_no_minting_api(source: &str) -> bool {
    !source.contains("new_for_agent_factory")
        && !source.contains("export_name")
        && !source.contains("link_name")
        && !source.contains("extern \"Rust\"")
        && !source.contains("#[repr(C)]")
        && !source.contains("NonZeroUsize")
        && !source.contains("pub unsafe fn")
        && !source.contains("pub const unsafe fn")
        && !source.contains("pub fn new")
        && !source.contains("pub fn mint")
        && !source.contains("CANONICAL_AUTHORITY")
        && !source.contains("AuthoritySeal")
        && !source.contains("words: [u64; 4]")
        && source.matches("pub fn ").count() == 1
        && source.contains("pub fn is_canonical_factory_authority(&self) -> bool")
        && source.contains("source_type: TypeId")
        && source.contains("inventory::collect!(AgentFactoryBuildAuthorityRegistration)")
        && source.contains("registrations.next().is_some()")
}

fn agent_mod_reexport_is_doc_hidden(source: &str) -> bool {
    let Some(pos) = source.find("pub use builder::build_agent_after_factory_policy") else {
        return true;
    };
    let cfg_window = source[..pos]
        .lines()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .join("\n");
    cfg_window.contains("#[doc(hidden)]")
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
            || stderr.contains("link_name"),
        "downstream direct-authority fixture failed for the wrong reason:\n{stderr}"
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
            || stderr.contains("assertion failed"),
        "downstream direct-authority transmute fixture failed for the wrong reason:\n{stderr}"
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
            || stderr.contains("cannot transmute between types of different sizes"),
        "downstream direct-authority mirror fixture failed for the wrong reason:\n{stderr}"
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
        public_standalone_build_is_cfg_gated(&builder),
        "meerkat_core::AgentBuilder must not expose public standalone \
         build(client, tools, store) in its default production API; low-level \
         standalone construction must be test/embedding opt-in"
    );
    assert!(
        public_factory_policy_finalizer_requires_authority_before_builder(&builder),
        "the factory-policy finalizer must require typed facade authority before \
         accepting an AgentBuilder; downstream callers must not be able to enter \
         the finalizer with only public SessionMetadata, SessionBuildState, and a \
         public turn-state handle"
    );
    assert!(
        public_factory_policy_finalizer_requires_typed_authority(&builder),
        "the feature-gated factory-policy finalizer must require a typed \
         authority that is not minted or re-exported from meerkat-core; a \
         feature-unified downstream crate must not be able to call the \
         finalizer with only public SessionMetadata, SessionBuildState, and \
         a public turn-state handle"
    );
    assert!(
        !builder.contains("pub async fn build_after_factory_policy"),
        "meerkat_core::AgentBuilder must not expose a public factory-policy \
         build method; canonical factory authority must live outside the \
         public builder impl"
    );
    assert!(
        builder.contains("validate_factory_policy()?")
            && builder.contains("is_canonical_factory_authority()")
            && builder.contains("AgentBuildPolicyError::InvalidBuildAuthority")
            && !builder.contains("pub fn from_registered_source<A: 'static>")
            && !builder.contains("pub const fn canonical_factory")
            && !builder.contains("pub const fn test_harness")
            && !builder.contains("pub struct AgentFactoryPolicyAuthorityRegistration")
            && !builder.contains("pub enum AgentFactoryPolicyAuthorityRegistrationKind"),
        "canonical factory construction must cross into core through a \
         validating factory-policy seam that does not expose a downstream \
         registration or authority-minting API"
    );
    assert!(
        !builder.contains("Location::caller")
            && !builder.contains("is_allowed_factory_policy_callsite")
            && !builder.contains("validate_factory_policy_caller"),
        "core factory authority must not trust source file path suffixes; \
         downstream crates can spoof callsites by placing code under matching \
         paths"
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
        agent_mod_reexport_is_doc_hidden(&agent_mod),
        "meerkat_core::agent must not re-export the factory-policy finalizer \
         as a normal public construction surface"
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
    let facade_build = repo_file("meerkat/build.rs");

    assert!(
        !authority_build.contains("cargo:metadata=")
            && !authority_build.contains("cargo:word_")
            && !authority_build.contains("MEERKAT_AGENT_BUILD_AUTHORITY_WORD_"),
        "authority build script must not publish canonical factory authority \
         seal words through Cargo metadata or rustc environment"
    );
    assert!(
        !facade_build.contains("DEP_MEERKAT_AGENT_BUILD_AUTHORITY_WORD_")
            && !facade_build.contains("MEERKAT_AGENT_BUILD_AUTHORITY_WORD_"),
        "facade build script must not consume leaked authority seal words from \
         dependency metadata; direct downstream dependents can read the same \
         DEP_* values"
    );
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
            && !public_core.contains("\"standalone-agent-builder\""),
        "public //meerkat-core:meerkat_core must not expose standalone build \
         features or internal feature selectors to Bazel consumers"
    );

    let internal_core = bazel_target_block(&core_bazel, "meerkat_core_agent_factory_build")
        .expect("meerkat-core BUILD.bazel must contain a non-public AgentFactory core variant");
    assert!(
        internal_core.contains("meerkat_agent_build_authority")
            && !internal_core.contains("//visibility:public"),
        "Bazel AgentFactory core bridge must be a non-public target carrying \
         the authority dependency"
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
                !source.contains("build_agent_after_factory_policy(")
                    && !source.contains(".build_after_factory_policy("),
                "production source must not bypass AgentFactory through the \
                 core factory-authority seam: {}",
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
        factory.contains("build_agent_after_factory_policy(")
            && !factory.contains("agent_factory_policy_authority()")
            && !factory.contains(".build_after_factory_policy("),
        "AgentFactory must enter meerkat_core through the explicit \
         factory-policy seam rather than a public AgentBuilder method or \
         publicly mintable authority helper"
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
