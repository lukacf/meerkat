use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

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
        !builder.contains("pub async fn build_after_factory_policy"),
        "meerkat_core::AgentBuilder must not expose a public factory-policy \
         build method; canonical factory authority must live outside the \
         public builder impl"
    );
    assert!(
        builder.contains("pub fn build_agent_after_factory_policy")
            && builder.contains("#[track_caller]")
            && builder.contains(
                "let caller_validation = validate_factory_policy_caller(Location::caller());"
            )
            && builder.contains("validate_factory_policy()?"),
        "canonical factory construction must cross into core through a \
         validating factory-policy seam that rejects non-factory callers"
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

    assert!(
        !agent_mod.contains("AgentFactoryBuildToken"),
        "meerkat_core::agent must not re-export a factory token that downstream \
         crates can mint or pass"
    );
    assert!(
        !lib.contains("AgentFactoryBuildToken"),
        "meerkat_core must not re-export a factory token that downstream crates \
         can mint or pass"
    );
    assert!(
        !facade_lib.contains("CoreAgentBuilder") && !facade_lib.contains("StandaloneAgentBuilder"),
        "meerkat facade must not publicly re-export the core builder as a \
         standalone construction shortcut"
    );
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
        !builder.contains("pub fn agent_factory_policy_authority"),
        "core factory authority must not expose a safe public minting helper; \
         downstream callers can otherwise mint the opaque authority and call \
         the factory-policy seam after synthesizing public metadata"
    );
    assert!(
        !builder
            .contains("__agent_factory_policy_authority(&self) -> factory_policy_private::Seal {"),
        "factory authority must not provide a default seal method; default \
         trait methods make authority forgeable by empty downstream impls"
    );
    assert!(
        !agent_mod.contains("AgentFactoryPolicyAuthority")
            && !agent_mod.contains("agent_factory_policy_authority"),
        "meerkat_core::agent must not re-export a public factory-authority \
         type, trait, or safe minting helper that downstream crates can name, \
         implement, or call"
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
