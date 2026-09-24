//! External contract for offline MCP commands and CLI convention roots.

#![cfg(feature = "mcp")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;
use std::process::{Command, Output};

fn rkat_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        return path.into();
    }
    let mut path = std::env::current_exe().expect("current test executable");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(if cfg!(windows) { "rkat.exe" } else { "rkat" });
    path
}

fn run_rkat(
    binary: &std::path::Path,
    context_root: &std::path::Path,
    user_root: &std::path::Path,
    ambient_cwd: &std::path::Path,
    ambient_home: &std::path::Path,
    args: &[&str],
) -> Output {
    Command::new(binary)
        .current_dir(ambient_cwd)
        .env("HOME", ambient_home)
        .arg("--context-root")
        .arg(context_root)
        .arg("--user-config-root")
        .arg(user_root)
        .args(args)
        .output()
        .expect("run rkat")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "rkat failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn offline_mcp_commands_honor_explicit_convention_roots() {
    let temp = tempfile::TempDir::new().unwrap();
    let context_root = temp.path().join("explicit-context");
    let user_root = temp.path().join("explicit-user");
    let ambient_cwd = temp.path().join("ambient-cwd");
    let ambient_home = temp.path().join("ambient-home");
    for dir in [&context_root, &user_root, &ambient_cwd, &ambient_home] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let binary = rkat_binary();

    let add_user = run_rkat(
        &binary,
        &context_root,
        &user_root,
        &ambient_cwd,
        &ambient_home,
        &[
            "mcp",
            "add",
            "explicit-user",
            "--scope",
            "user",
            "--",
            "echo",
            "user",
        ],
    );
    assert_success(&add_user);
    let user_config = user_root.join(".rkat/mcp.toml");
    assert!(user_config.exists());
    assert!(!ambient_home.join(".rkat/mcp.toml").exists());

    let add_project = run_rkat(
        &binary,
        &context_root,
        &user_root,
        &ambient_cwd,
        &ambient_home,
        &[
            "mcp",
            "add",
            "explicit-project",
            "--scope",
            "project",
            "--",
            "echo",
            "project",
        ],
    );
    assert_success(&add_project);
    let project_config = context_root.join(".rkat/mcp.toml");
    assert!(project_config.exists());
    assert!(!ambient_cwd.join(".rkat/mcp.toml").exists());

    let list_user = run_rkat(
        &binary,
        &context_root,
        &user_root,
        &ambient_cwd,
        &ambient_home,
        &["mcp", "list", "--scope", "user", "--json"],
    );
    assert_success(&list_user);
    let listed = String::from_utf8(list_user.stdout).unwrap();
    assert!(listed.contains("explicit-user"));
    assert!(!listed.contains("explicit-project"));

    let get_project = run_rkat(
        &binary,
        &context_root,
        &user_root,
        &ambient_cwd,
        &ambient_home,
        &[
            "mcp",
            "get",
            "explicit-project",
            "--scope",
            "project",
            "--json",
        ],
    );
    assert_success(&get_project);
    assert!(
        String::from_utf8(get_project.stdout)
            .unwrap()
            .contains("explicit-project")
    );

    let remove_user = run_rkat(
        &binary,
        &context_root,
        &user_root,
        &ambient_cwd,
        &ambient_home,
        &["mcp", "remove", "explicit-user", "--scope", "user"],
    );
    assert_success(&remove_user);
    let remove_project = run_rkat(
        &binary,
        &context_root,
        &user_root,
        &ambient_cwd,
        &ambient_home,
        &["mcp", "remove", "explicit-project", "--scope", "project"],
    );
    assert_success(&remove_project);

    assert!(
        !std::fs::read_to_string(user_config)
            .unwrap()
            .contains("explicit-user")
    );
    assert!(
        !std::fs::read_to_string(project_config)
            .unwrap()
            .contains("explicit-project")
    );
}
