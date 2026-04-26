use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_runfile_tool(root: &Path, suffix: &str) -> Option<PathBuf> {
    let mut queue = VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
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

fn bazel_trybuild_env() -> Vec<(&'static str, OsString)> {
    let mut env = Vec::new();
    let Some(runfiles_dir) = std::env::var_os("RUNFILES_DIR").map(PathBuf::from) else {
        return env;
    };

    let manifest_dir = runfiles_dir
        .join("_main/test-fixtures/machine-dsl-tests")
        .canonicalize()
        .ok();
    if let Some(manifest_dir) = manifest_dir {
        env.push(("CARGO_MANIFEST_DIR", manifest_dir.into_os_string()));
    }

    if std::env::var_os("CARGO").is_none()
        && let Some(cargo) = find_runfile_tool(&runfiles_dir, "rust_toolchain/bin/cargo")
    {
        env.push(("CARGO", cargo.into_os_string()));
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

fn run_in_configured_bazel_child(env: Vec<(&'static str, OsString)>) -> std::io::Result<bool> {
    if env.is_empty() || std::env::var_os("MEERKAT_TRYBUILD_ENV_CONFIGURED").is_some() {
        return Ok(false);
    }

    let current_exe = std::env::current_exe()?;
    let status = Command::new(current_exe)
        .arg("--exact")
        .arg("compile_fail")
        .arg("--nocapture")
        .env("MEERKAT_TRYBUILD_ENV_CONFIGURED", "1")
        .envs(env)
        .status()?;
    assert!(
        status.success(),
        "configured trybuild child failed: {status}"
    );
    Ok(true)
}

#[test]
fn compile_fail() -> std::io::Result<()> {
    if run_in_configured_bazel_child(bazel_trybuild_env())? {
        return Ok(());
    }

    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
    Ok(())
}
