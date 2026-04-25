use std::collections::VecDeque;
use std::path::{Path, PathBuf};

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

fn configure_bazel_trybuild_tools() {
    let Some(runfiles_dir) = std::env::var_os("RUNFILES_DIR").map(PathBuf::from) else {
        return;
    };

    let manifest_dir = runfiles_dir
        .join("_main/test-fixtures/machine-dsl-tests")
        .canonicalize()
        .ok();
    if let Some(manifest_dir) = manifest_dir {
        // SAFETY: see the CARGO setup below.
        unsafe { std::env::set_var("CARGO_MANIFEST_DIR", manifest_dir) };
    }

    if std::env::var_os("CARGO").is_none()
        && let Some(cargo) = find_runfile_tool(&runfiles_dir, "rust_toolchain/bin/cargo")
    {
        // SAFETY: this test configures process-wide tool paths before
        // spawning trybuild/cargo and before starting any test threads.
        unsafe { std::env::set_var("CARGO", cargo) };
    }

    if std::env::var_os("RUSTC").is_none()
        && let Some(rustc) = find_runfile_tool(&runfiles_dir, "rust_toolchain/bin/rustc")
    {
        // SAFETY: see the CARGO setup above.
        unsafe { std::env::set_var("RUSTC", rustc) };
    }

    if std::env::var_os("CARGO_HOME").is_none()
        && let Some(cargo_home) = std::env::var_os("MEERKAT_HOST_CARGO_HOME")
    {
        // SAFETY: see the CARGO setup above.
        unsafe { std::env::set_var("CARGO_HOME", cargo_home) };
    }
}

#[test]
fn compile_fail() {
    configure_bazel_trybuild_tools();
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}
