#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn run_cargo(args: &[&str]) -> std::process::Output {
    Command::new(cargo_bin())
        .current_dir(workspace_root())
        .args(args)
        .output()
        .expect("run cargo")
}

#[test]
fn feature_graph_contract_minimal_lane_excludes_optional_surface_crates() {
    let check = run_cargo(&[
        "check",
        "-p",
        "meerkat",
        "--no-default-features",
        "--features",
        "openai",
    ]);
    assert!(
        check.status.success(),
        "minimal cargo check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let tree = run_cargo(&[
        "tree",
        "-p",
        "meerkat",
        "--no-default-features",
        "--features",
        "openai",
    ]);
    assert!(
        tree.status.success(),
        "minimal cargo tree failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&tree.stdout),
        String::from_utf8_lossy(&tree.stderr)
    );

    let stdout = String::from_utf8_lossy(&tree.stdout);
    assert!(
        !stdout.contains("meerkat-tools v"),
        "minimal lane still resolves meerkat-tools:\n{stdout}"
    );
    assert!(
        !stdout.contains("meerkat-hooks v"),
        "minimal lane still resolves meerkat-hooks:\n{stdout}"
    );
    assert!(
        !stdout.contains("meerkat-skills v"),
        "minimal lane still resolves meerkat-skills:\n{stdout}"
    );
}

#[test]
fn feature_graph_contract_tool_error_remains_available_from_facade() {
    let err: meerkat::ToolError = meerkat::ToolError::not_found("missing_tool");
    assert!(err.to_string().contains("missing_tool"));
}

#[test]
fn feature_graph_contract_rkat_default_lane_explicitly_requests_hooks_and_builtin_tools() {
    let tree = run_cargo(&["tree", "-p", "rkat", "-e", "features", "-i", "meerkat"]);
    assert!(
        tree.status.success(),
        "rkat feature tree failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&tree.stdout),
        String::from_utf8_lossy(&tree.stderr)
    );

    let stdout = String::from_utf8_lossy(&tree.stdout);
    assert!(
        stdout.contains("rkat feature \"hooks\""),
        "rkat must explicitly request facade hooks support instead of relying on transitive unification:\n{stdout}"
    );
    assert!(
        stdout.contains("rkat feature \"builtin-tools\""),
        "rkat must explicitly request facade builtin tool support instead of relying on transitive unification:\n{stdout}"
    );
}

#[cfg(feature = "builtin-tools")]
#[test]
fn feature_graph_contract_default_lane_exposes_builtin_surface_types() {
    let _ = std::any::TypeId::of::<meerkat::BuiltinToolConfig>();
    let _ = std::any::TypeId::of::<meerkat::CompositeDispatcherError>();
}
