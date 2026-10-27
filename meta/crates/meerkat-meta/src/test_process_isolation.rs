//! Run one unit test as the only test of a child process of its own binary.
//!
//! A few tests assert on meerkat's process-wide cost counters (full
//! transcript graph validations, content-digest bytes) that have no per-test
//! scope in the pinned meerkat release. Under the threaded `cargo test`
//! harness every other test that validates or digests moves those counters,
//! and the SQLite stores under test do that work on blocking worker threads,
//! so a thread-local count would miss the very work being asserted on. Such a
//! test re-runs itself here: in the child it is the only test, so the
//! process-wide counters see its work alone, at full assertion strength, under
//! both `cargo test` and nextest.

use std::process::Command;

/// Environment marker that selects the isolated child's test.
const ISOLATED_TEST_ENV: &str = "MEERKAT_META_ISOLATED_TEST";

/// Whether this process should run the body of the test `name` declared in
/// `module_path` (pass `module_path!()` and the test function's name).
///
/// In the isolated child, selected by the exact test name in the environment
/// marker, this returns `true` and the caller runs its body. Anywhere else it
/// runs that test as the only test of a child process of the same test
/// binary, asserts that the child ran exactly this one test and passed
/// (forwarding the child's stdout and stderr when it did not), and returns
/// `false`: the caller returns without running the body a second time.
pub(crate) fn run_body_in_isolated_process(module_path: &str, name: &str) -> bool {
    run_body_in_isolated_process_with_env(module_path, name, &[])
}

/// [`run_body_in_isolated_process`] with `env` set in the child's
/// environment: a test whose body must leave this process's own
/// environment-derived locations alone (the OS user's configuration
/// directory, for one) points them at directories the caller owns, and
/// inspects those once the child has run.
pub(crate) fn run_body_in_isolated_process_with_env(
    module_path: &str,
    name: &str,
    env: &[(&str, &std::path::Path)],
) -> bool {
    let test = isolated_test_name(module_path, name);
    if std::env::var(ISOLATED_TEST_ENV).is_ok_and(|selected| selected == test) {
        return true;
    }
    let binary = std::env::current_exe()
        .unwrap_or_else(|error| panic!("locate the test binary to isolate `{test}`: {error}"));
    let output = Command::new(&binary)
        .args([test.as_str(), "--exact", "--test-threads=1", "--nocapture"])
        .env(ISOLATED_TEST_ENV, &test)
        .envs(env.iter().copied())
        .output()
        .unwrap_or_else(|error| panic!("spawn the isolated child for `{test}`: {error}"));
    assert_isolated_child_passed(&test, &output);
    false
}

/// [`run_body_in_isolated_process`] for an async test, with a deadline on the
/// child: a child still running at `deadline` is killed and the test fails at
/// once, instead of waiting for the outer job ceiling. Its only callers are the
/// voice host tests, which exist only with `openai-live`.
#[cfg(feature = "openai-live")]
pub(crate) async fn run_body_in_isolated_process_within(
    module_path: &str,
    name: &str,
    deadline: std::time::Duration,
) -> bool {
    let test = isolated_test_name(module_path, name);
    if std::env::var(ISOLATED_TEST_ENV).is_ok_and(|selected| selected == test) {
        return true;
    }
    let binary = std::env::current_exe()
        .unwrap_or_else(|error| panic!("locate the test binary to isolate `{test}`: {error}"));
    let output = tokio::time::timeout(
        deadline,
        tokio::process::Command::new(&binary)
            .args([test.as_str(), "--exact", "--test-threads=1", "--nocapture"])
            .env(ISOLATED_TEST_ENV, &test)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap_or_else(|_| {
        panic!("the isolated child for `{test}` did not finish within {deadline:?}")
    })
    .unwrap_or_else(|error| panic!("spawn the isolated child for `{test}`: {error}"));
    assert_isolated_child_passed(&test, &output);
    false
}

/// The test's path within its crate, as the harness filters it.
fn isolated_test_name(module_path: &str, name: &str) -> String {
    let module = module_path
        .split_once("::")
        .map_or("", |(_crate_name, module)| module);
    if module.is_empty() {
        name.to_string()
    } else {
        format!("{module}::{name}")
    }
}

/// The isolated child ran exactly this one test and passed; otherwise fail
/// with the child's stdout and stderr.
fn assert_isolated_child_passed(test: &str, output: &std::process::Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // `1 passed` also proves the exact filter selected this test: a wrong name
    // would run zero tests and still exit successfully.
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed"),
        "the isolated child for `{test}` did not pass exactly this one test ({})\n\
         --- child stdout ---\n{stdout}\n--- child stderr ---\n{stderr}",
        output.status
    );
}
