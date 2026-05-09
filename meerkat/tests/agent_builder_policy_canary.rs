#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unnecessary_literal_bound,
    clippy::unwrap_used
)]

use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::types::{AssistantBlock, StopReason, Usage};
use meerkat_core::{
    AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult, Message,
    Session, ToolCallView, ToolDef, ToolDispatchOutcome, ToolError,
};

struct CanaryClient;

#[async_trait]
impl AgentLlmClient for CanaryClient {
    async fn stream_response(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: "done".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            Usage::default(),
        ))
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    fn model(&self) -> &str {
        "mock-model"
    }
}

struct CanaryTools;

#[async_trait]
impl AgentToolDispatcher for CanaryTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(&self, _call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::execution_failed(
            "agent builder policy canary does not dispatch tools",
        ))
    }
}

struct CanaryStore;

#[async_trait]
impl AgentSessionStore for CanaryStore {
    async fn save(&self, _session: &Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
        Ok(None)
    }
}

fn repo_root() -> PathBuf {
    let mut path = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    path.pop();
    if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path
    }
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
        if let Some(cargo) = find_runfile_tool(&runfiles_dir, "bin/cargo") {
            env.push(("CARGO", cargo.into_os_string()));
        } else {
            panic!(
                "Bazel downstream AgentBuilder canaries require rules_rust Cargo runfiles; \
                 missing cargo would silently skip downstream spoof fixtures"
            );
        }
    }

    if std::env::var_os("RUSTC").is_none() {
        if let Some(rustc) = find_runfile_tool(&runfiles_dir, "bin/rustc") {
            env.push(("RUSTC", rustc.into_os_string()));
        } else {
            panic!(
                "Bazel downstream AgentBuilder canaries require rules_rust rustc runfiles; \
                 missing rustc would silently skip downstream spoof fixtures"
            );
        }
    }

    if std::env::var_os("CARGO_HOME").is_none()
        && let Some(cargo_home) = std::env::var_os("MEERKAT_HOST_CARGO_HOME")
    {
        env.push(("CARGO_HOME", cargo_home));
    }

    if cfg!(target_os = "linux") {
        env.push((
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER",
            OsString::from("cc"),
        ));
        env.push((
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS",
            OsString::from("-Clink-self-contained=no"),
        ));
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

fn downstream_cargo_command(cargo: OsString) -> Command {
    let mut command = Command::new(cargo);
    command
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS");
    command
}

#[derive(Clone, Copy)]
enum DownstreamCargoAction {
    Check,
    Run,
}

impl DownstreamCargoAction {
    fn arg(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Run => "run",
        }
    }
}

fn run_downstream_cargo_fixture(
    test_name: &'static str,
    package_name: &str,
    dependencies: &str,
    source: &str,
    action: DownstreamCargoAction,
) -> std::io::Result<Option<Output>> {
    if std::env::var_os("MEERKAT_DOWNSTREAM_CANARY_SKIP_CARGO").is_some() {
        return Ok(None);
    }

    if run_in_configured_bazel_child(test_name, bazel_cargo_check_env())? {
        return Ok(None);
    }

    let temp = tempfile::tempdir()?;
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "{package_name}"
version = "0.0.0"
edition = "2024"

[dependencies]
{dependencies}
"#,
        ),
    )?;
    fs::write(src_dir.join("main.rs"), source)?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = downstream_cargo_command(cargo)
        .arg(action.arg())
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .output()?;

    Ok(Some(output))
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
    let dependencies = format!(
        r#"meerkat-core = {{ path = "{}" }}"#,
        repo_root().join("meerkat-core").display()
    );
    let Some(output) = run_downstream_cargo_fixture(
        "downstream_safe_code_cannot_forge_factory_policy_finalizer",
        "agent-builder-policy-downstream",
        &dependencies,
        &repo_file(
            "meerkat/tests/fixtures/agent_builder_policy/downstream_forged_factory_policy.rs",
        ),
        DownstreamCargoAction::Check,
    )?
    else {
        return Ok(());
    };
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
fn downstream_unsafe_code_cannot_enter_factory_policy_finalizer() -> std::io::Result<()> {
    let dependencies = format!(
        r#"async-trait = "0.1"
futures = "0.3"
inventory = "0.3"
meerkat-core = {{ path = "{}" }}"#,
        repo_root().join("meerkat-core").display()
    );
    let Some(output) = run_downstream_cargo_fixture(
        "downstream_unsafe_code_cannot_enter_factory_policy_finalizer",
        "agent-builder-policy-downstream-unsafe-finalizer",
        &dependencies,
        &repo_file(
            "meerkat/tests/fixtures/agent_builder_policy/downstream_unsafe_factory_policy_finalizer.rs",
        ),
        DownstreamCargoAction::Run,
    )?
    else {
        return Ok(());
    };
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
            || stderr.contains("__meerkat_agent_factory_policy_bridge_registration")
            || stderr.contains("__meerkat_agent_factory_policy_build_v3")
            || stderr.contains("exported_agent_factory_policy_build")
            || stderr.contains("link_name")
            || stderr.contains("undefined symbol")
            || stderr.contains("not found in")
            || stderr.contains("couldn't read")
            || stderr.contains("No such file or directory"),
        "downstream unsafe finalizer fixture failed for the wrong reason:\n{stderr}"
    );
    Ok(())
}

#[test]
fn downstream_public_cargo_facade_agentbuilder_links_without_repo_cfg() -> std::io::Result<()> {
    let dependencies = format!(
        r#"async-trait = "0.1"
futures = "0.3"
meerkat = {{ path = "{}", default-features = false }}
meerkat-core = {{ path = "{}" }}"#,
        repo_root().join("meerkat").display(),
        repo_root().join("meerkat-core").display()
    );
    let Some(output) = run_downstream_cargo_fixture(
        "downstream_public_cargo_facade_agentbuilder_links_without_repo_cfg",
        "agent-builder-policy-downstream-public-facade-smoke",
        &dependencies,
        &repo_file(
            "meerkat/tests/fixtures/agent_builder_policy/downstream_public_facade_agentbuilder.rs",
        ),
        DownstreamCargoAction::Run,
    )?
    else {
        return Ok(());
    };
    assert!(
        output.status.success(),
        "public downstream facade AgentBuilder build must compile, link, and \
         construct through AgentFactory without repo-injected rustflags; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("public facade AgentBuilder constructed an agent"),
        "public downstream facade AgentBuilder smoke passed for the wrong reason; stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[tokio::test]
async fn public_facade_rejects_forged_session_runtime_binding_authority() {
    let session = Session::default();
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let prepared = runtime
        .prepare_bindings(session.id().clone())
        .await
        .expect("prepare machine-owned bindings");
    let forged = meerkat_core::SessionRuntimeBindings::__from_runtime_authority(
        session.id().clone(),
        prepared.epoch_id().clone(),
        Arc::clone(prepared.ops_lifecycle()),
        Arc::clone(prepared.cursor_state()),
        Arc::clone(prepared.tool_visibility_owner()),
        Arc::clone(prepared.turn_state()),
        Arc::clone(prepared.comms_drain()),
        Arc::clone(prepared.external_tool_surface()),
        Arc::clone(prepared.peer_comms()),
        Arc::clone(prepared.session_admission()),
        Arc::clone(prepared.model_routing()),
        Arc::clone(prepared.auth_lease()),
        Arc::clone(prepared.mcp_server_lifecycle()),
        Arc::clone(prepared.peer_interaction()),
        Arc::clone(prepared.session_context()),
        Arc::clone(prepared.session_claim_handle()),
        Arc::clone(prepared.interaction_stream()),
        Arc::clone(prepared.realtime_product_turn()),
        Arc::new(()),
    );

    let result = meerkat::AgentBuilder::new()
        .resume_session(session)
        .runtime_build_mode(meerkat_core::RuntimeBuildMode::SessionOwned(forged))
        .build(
            Arc::new(CanaryClient),
            Arc::new(CanaryTools),
            Arc::new(CanaryStore),
        )
        .await;

    match result {
        Err(meerkat::BuildAgentError::Config(message)) => assert!(
            message.contains("SessionRuntimeBindings were not prepared by MeerkatMachine"),
            "public facade rejected forged session runtime authority for the wrong reason: {message}"
        ),
        Err(error) => panic!("public facade returned wrong error for forged authority: {error}"),
        Ok(_) => panic!("public facade accepted forged session runtime authority"),
    }
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
            && builder.contains("facade_agent_factory_policy_bridge_token_is_valid")
            && builder.contains("__meerkat_agent_factory_policy_bridge_token_is_valid_v1")
            && builder.contains("InvalidFactoryBridgeToken")
            && !builder.contains("AgentFactoryPolicyBridgeRegistration")
            && !builder.contains("__meerkat_agent_factory_policy_bridge_registration")
            && !builder.contains("FACADE_FACTORY_SOURCE_FINGERPRINT")
            && !builder.contains("__facade_from_compile_env")
            && !builder.contains("__meerkat_agent_factory_policy_source_fingerprint")
            && !builder.contains("source_content_fingerprint_matches")
            && !builder.contains("source_file_content_fingerprint")
            && !builder.contains("core::panic::Location::caller")
            && !builder.contains("source_line")
            && !builder.contains("source_column")
            && !builder.contains("source_location_matches_canonical_facade_site")
            && !builder.contains("source_file_matches")
            && !builder.contains("inventory::collect!(AgentFactoryPolicyBridgeRegistration)")
            && !builder.contains("AgentFactoryPolicyBridgeRegistration::new")
            && !builder.contains("pub const fn new(")
            && !builder.contains("pub const fn __from_compile_env")
            && !builder.contains("CoreHooksTest")
            && !builder.contains("__core_hooks_test_from_compile_env")
            && !builder
                .contains("__meerkat_core_hooks_test_agent_factory_policy_bridge_registration")
            && !builder.contains(".is_none_or(")
            && !builder.contains(
                "#[cfg(target_arch = \"wasm32\")]\n        {\n            true\n        }",
            )
            && !builder.contains("pub enum AgentFactoryPolicyBridgeRegistrationKind")
            && !builder.contains("MEERKAT_AGENT_FACTORY_POLICY_BUILD_SYMBOL")
            && !builder.contains("MEERKAT_AGENT_FACTORY_POLICY_BUILD_PROOF")
            && !builder.contains("feature = \"internal-agent-factory-build\"")
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
        assert!(
            !facade_build.contains("DEP_MEERKAT_CORE_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX"),
            "facade build script must not consume the core factory bridge \
             suffix from public Cargo DEP_* metadata; direct downstream \
             dependents can read the same value and spoof suffixed symbols"
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
        assert!(
            !build_script.contains("cargo:agent_factory_policy_bridge_symbol_suffix"),
            "{name} must not publish the factory bridge suffix through Cargo \
             metadata; public direct dependents can read DEP_* metadata and \
             generate matching validator/finalizer symbols"
        );
        assert!(
            !build_script.contains("join(\"agent_factory_policy_bridge_symbol_suffix\")")
                && !build_script.contains("std::fs::read_to_string(&marker)")
                && !build_script.contains("std::fs::write(&marker"),
            "{name} must not read or write a target-dir factory bridge suffix \
             marker; public direct dependents can scan the same target tree and \
             generate matching validator/finalizer symbols"
        );
    }
}

#[test]
fn facade_build_script_prefers_exact_registry_core_package() {
    let facade_build = repo_file("meerkat/build.rs");

    assert!(
        facade_build.contains("CARGO_PKG_VERSION")
            && facade_build.contains("meerkat-core-{package_version}")
            && facade_build.contains("entries.sort_by_key"),
        "facade build script must prefer the exact same-version \
         meerkat-core registry package before scanning stale sibling versions; \
         otherwise downstream published-crate builds can link the facade \
         against a symbol suffix from an older cached meerkat-core package"
    );
}

#[test]
fn facade_cargo_does_not_feature_unify_standalone_builder_by_default() {
    let cargo = repo_file("meerkat/Cargo.toml");
    let core_cargo = repo_file("meerkat-core/Cargo.toml");
    let repo_cargo = repo_file("scripts/repo-cargo");
    let buildbuddy_cargo_lane = repo_file("tools/buildbuddy/cargo_lane_test.sh");
    let buildbuddy_full_lane = repo_file("tools/buildbuddy/full_lane_test.sh");

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
            assert!(
                !line.contains("internal-agent-factory-build"),
                "the facade Cargo graph must not enable a public \
                 meerkat-core/internal-agent-factory-build feature through \
                 normal feature unification"
            );
        }
    }
    assert!(
        !core_cargo.contains("internal-agent-factory-build"),
        "meerkat-core must not publish an internal factory-build feature; \
         downstream crates can enable public features and compile the hidden \
         finalizer"
    );
    assert!(
        !repo_cargo.contains("meerkat_internal_agent_factory_build"),
        "repo Cargo lanes must not inject the internal AgentFactory cfg into \
         every Cargo invocation; pure public-core checks must exercise the \
         finalizer-free graph"
    );
    for (path, script) in [
        ("tools/buildbuddy/cargo_lane_test.sh", buildbuddy_cargo_lane),
        ("tools/buildbuddy/full_lane_test.sh", buildbuddy_full_lane),
    ] {
        assert!(
            !script.contains("meerkat_internal_agent_factory_build"),
            "{path} must not inject the internal AgentFactory cfg into \
             cargo-equivalent BuildBuddy lanes; those lanes must exercise the \
             same public-core graph as normal Cargo"
        );
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
        !public_core.contains("\"standalone-agent-builder\"")
            && !public_core.contains("\"internal-agent-factory-build\"")
            && !public_core.contains("meerkat_internal_agent_factory_build")
            && !public_core.contains("rustc_flags")
            && !public_core.contains("MEERKAT_AGENT_FACTORY_POLICY_BUILD_SYMBOL")
            && !public_core.contains("MEERKAT_AGENT_FACTORY_POLICY_BUILD_PROOF"),
        "public //meerkat-core:meerkat_core must not expose standalone build \
         features, public factory-build features, internal factory cfg, or \
         recoverable factory bridge proof material in Bazel builds"
    );

    let internal_core = bazel_target_block(&core_bazel, "meerkat_core_agent_factory_build")
        .expect("Bazel must provide a facade-private core target for AgentFactory builds");
    assert!(
        internal_core.contains("meerkat_internal_agent_factory_build")
            && internal_core.contains("rustc_flags")
            && internal_core.contains("\"//meerkat:__pkg__\"")
            && !internal_core.contains("//visibility:public"),
        "the Bazel AgentFactory core variant must carry the internal factory \
         cfg but be visible only to the facade package"
    );
    for forbidden_package in [
        "//meerkat-cli:__pkg__",
        "//meerkat-comms:__pkg__",
        "//meerkat-runtime:__pkg__",
        "//tests/integration:__pkg__",
    ] {
        assert!(
            !internal_core.contains(forbidden_package),
            "the actual Bazel AgentFactory core variant must be visible only \
             to the facade package; {forbidden_package} can otherwise select \
             the internal finalizer graph directly"
        );
    }

    if let Some(facade_bazel) = try_repo_file("meerkat/BUILD.bazel") {
        assert!(
            facade_bazel.contains("name = \"meerkat_core_agent_factory_build\"")
                && facade_bazel
                    .contains("actual = \"//meerkat-core:meerkat_core_agent_factory_build\""),
            "the facade package must own the Bazel alias that routes private \
             core factory-build graph consumers through the canonical facade \
             package"
        );
        assert!(
            !facade_bazel.contains("//meerkat:meerkat_agent_factory_build"),
            "the facade package must not rewrite self-dependencies to a \
             nonexistent private facade variant"
        );
    }
}

#[test]
fn non_facade_bazel_targets_do_not_directly_select_core_factory_variant() {
    let root = repo_root();
    for entry in
        fs::read_dir(&root).unwrap_or_else(|err| panic!("failed to read {}: {err}", root.display()))
    {
        let entry = entry.unwrap_or_else(|err| panic!("failed to read repo entry: {err}"));
        let path = entry.path().join("BUILD.bazel");
        if path == root.join("meerkat/BUILD.bazel") || path == root.join("meerkat-core/BUILD.bazel")
        {
            continue;
        }
        let Ok(build_file) = fs::read_to_string(&path) else {
            continue;
        };
        assert!(
            !build_file.contains("\"//meerkat-core:meerkat_core_agent_factory_build\""),
            "non-facade Bazel package must not depend on the actual core \
             AgentFactory bridge target directly: {}",
            path.display()
        );
    }
}

#[test]
fn ordinary_bazel_core_dependents_do_not_use_internal_factory_variant() {
    for (build_file, target) in [
        ("meerkat-comms/BUILD.bazel", "meerkat_comms"),
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
fn bazel_facade_consumers_do_not_mix_public_core_with_factory_graph() {
    for (build_file, target) in [
        ("meerkat-cli/BUILD.bazel", "rkat"),
        ("meerkat-cli/BUILD.bazel", "rkat_surface_session_store_bin"),
        (
            "meerkat-cli/BUILD.bazel",
            "rkat_surface_session_store_mcp_bin",
        ),
        (
            "meerkat-cli/BUILD.bazel",
            "rkat_surface_session_store_comms_mcp_bin",
        ),
        ("meerkat-mcp-server/BUILD.bazel", "meerkat_mcp_server"),
        (
            "meerkat-mcp-server/BUILD.bazel",
            "meerkat_mcp_server_surface_min",
        ),
        (
            "meerkat-mcp-server/BUILD.bazel",
            "meerkat_mcp_server_surface_comms",
        ),
        ("meerkat-mcp-server/BUILD.bazel", "rkat_mcp_surface_min_bin"),
        (
            "meerkat-mcp-server/BUILD.bazel",
            "rkat_mcp_surface_comms_bin",
        ),
        ("meerkat-mob/BUILD.bazel", "meerkat_mob"),
        ("meerkat-rest/BUILD.bazel", "meerkat_rest"),
        ("meerkat-rest/BUILD.bazel", "meerkat_rest_surface_min"),
        ("meerkat-rest/BUILD.bazel", "meerkat_rest_surface_comms"),
        ("meerkat-rest/BUILD.bazel", "rkat_rest_surface_min_bin"),
        ("meerkat-rest/BUILD.bazel", "rkat_rest_surface_comms_bin"),
        ("meerkat-rpc/BUILD.bazel", "meerkat_rpc"),
        ("meerkat-rpc/BUILD.bazel", "meerkat_rpc_surface_min"),
        ("meerkat-rpc/BUILD.bazel", "meerkat_rpc_surface_comms_mcp"),
        ("meerkat-rpc/BUILD.bazel", "rkat_rpc_surface_min_bin"),
        ("meerkat-rpc/BUILD.bazel", "rkat_rpc_surface_comms_mcp_bin"),
    ] {
        let Some(bazel) = try_repo_file(build_file) else {
            // Cargo-only source layouts may not include generated Bazel files.
            continue;
        };
        let build_target = bazel_target_block(&bazel, target)
            .unwrap_or_else(|| panic!("{build_file} must contain {target} target"));
        if !build_target.contains("\"//meerkat:meerkat\"") {
            continue;
        }

        assert!(
            build_target.contains("\"//meerkat:meerkat_core_agent_factory_build\""),
            "{target} consumes the facade and must use the same facade-owned \
             private core variant alias as the facade to avoid duplicate \
             meerkat_core types"
        );
        assert!(
            !build_target.contains("\"//meerkat-core:meerkat_core\","),
            "{target} must not mix the public core target with the facade's \
             private factory dependency graph"
        );
    }
}

#[test]
fn public_downstream_bazel_fixtures_do_not_use_private_factory_targets() {
    let Some(bazel) = try_repo_file("test-fixtures/surface-build-fixtures/BUILD.bazel") else {
        // Cargo-only source layouts may not include generated Bazel files.
        return;
    };

    for target in ["embedded_min_bin", "runtime_backed_min_bin"] {
        let build_target = bazel_target_block(&bazel, target)
            .unwrap_or_else(|| panic!("surface-build-fixtures BUILD.bazel must contain {target}"));
        assert!(
            !build_target.contains("_agent_factory_build"),
            "{target} models a public downstream Bazel consumer and must not \
             depend on private AgentFactory build targets"
        );
        assert!(
            build_target.contains("\"//meerkat:meerkat\""),
            "{target} must still exercise the public facade label"
        );
    }
}

#[test]
fn core_factory_authority_is_not_publicly_forgeable() {
    let builder = repo_file("meerkat-core/src/agent/builder.rs");
    let agent_mod = repo_file("meerkat-core/src/agent.rs");
    let runtime_epoch = repo_file("meerkat-core/src/runtime_epoch.rs");
    let factory = repo_file("meerkat/src/factory.rs");
    let runtime = repo_file("meerkat-runtime/src/lib.rs");

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
    assert!(
        !runtime_epoch.contains("pub session_id: SessionId")
            && !runtime_epoch.contains("pub ops_lifecycle: Arc<dyn OpsLifecycleRegistry>")
            && !runtime_epoch.contains("pub turn_state: Arc<dyn TurnStateHandle>")
            && !runtime_epoch
                .contains("pub external_tool_surface: Arc<dyn ExternalToolSurfaceHandle>")
            && runtime_epoch.contains("runtime_authority: Arc<dyn Any + Send + Sync>"),
        "SessionRuntimeBindings must be opaque and carry a runtime-private \
         authority witness; public fields let downstream crates forge \
         RuntimeBuildMode::SessionOwned bindings"
    );
    assert!(
        runtime.contains("struct SessionRuntimeBindingsAuthority")
            && !runtime.contains("pub struct SessionRuntimeBindingsAuthority")
            && runtime.contains("session_runtime_bindings_have_machine_authority"),
        "meerkat-runtime must keep the session binding authority witness private \
         while exposing only validation for the facade factory"
    );
    assert!(
        factory.contains("session_runtime_bindings_have_machine_authority(bindings)"),
        "AgentFactory must validate the runtime-private SessionRuntimeBindings \
         authority before consuming SessionOwned handles"
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
        if path != root.join("meerkat/src/factory.rs")
            && path != root.join("meerkat-core/src/agent/builder.rs")
        {
            assert!(
                !source.contains("__meerkat_agent_factory_policy_build_v3")
                    && !source.contains("__meerkat_agent_factory_policy_bridge_token_is_valid_v1"),
                "production source must not name AgentFactory bridge symbols \
                 outside the canonical facade factory/core bridge: {}",
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
        "@rules_rust//rust/toolchain:current_cargo_files",
        "@rules_rust//rust/toolchain:current_rust_stdlib_files",
        "@rules_rust//rust/toolchain:current_rustc_files",
        "@rules_rust//rust/toolchain:current_rustc_lib_files",
    ] {
        assert!(
            bazel.contains(label),
            "Bazel canary must explicitly include {label} so BuildBuddy scans \
             production crates that can host factory bypasses and has the \
             Cargo/Rust toolchain needed to execute downstream compile canaries"
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
