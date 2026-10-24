//! Running a test's body alone in a child process of the test binary.
//!
//! A process that a test of this binary spawns gets a copy of every
//! descriptor the binary has open, and keeps it until it executes its
//! program. A test that needs a descriptor of its own gone at a given moment
//! (a program it wrote and now runs) therefore runs where no other test
//! spawns anything: alone, in a child process of the binary, whatever runs
//! the tests (nextest's process per test, or plain `cargo test`'s threads in
//! one process). A file lock needs no such isolation: its holders unlock it
//! before they close it, and a copy keeps nothing unlocked.

/// Set in the child process a test runs alone in.
const ALONE_CHILD: &str = "MEERKAT_CORE_TEST_ALONE";
/// Printed by that child once the test's body has passed.
const ALONE_CHILD_DONE: &str = "meerkat-core test passed alone";

/// Run `body`, the body of the test `test` (its path in this binary, such as
/// `confinement::tests::a_row`), alone in a child process of this test
/// binary, and require it to pass there.
pub(crate) fn alone_in_child_process(test: &str, body: impl FnOnce()) {
    if std::env::var_os(ALONE_CHILD).is_some() {
        body();
        println!("\n{ALONE_CHILD_DONE}");
        return;
    }
    run_in_child_process(test);
}

fn run_in_child_process(test: &str) {
    #[allow(clippy::disallowed_methods)] // the test re-runs its own binary
    let output = std::process::Command::new(
        std::env::current_exe().unwrap_or_else(|error| panic!("the test binary: {error}")),
    )
    .args(["--exact", test, "--nocapture", "--test-threads=1"])
    .env(ALONE_CHILD, "1")
    .output()
    .unwrap_or_else(|error| panic!("{test} could not start alone: {error}"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(ALONE_CHILD_DONE),
        "{test} failed alone in its child process ({}):\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A process this test spawns that holds a copy of one of the test's
/// descriptors, as its standard input, until it is dropped: what any process
/// spawned meanwhile holds until it executes its program, kept for as long as
/// the test needs.
#[cfg(unix)]
pub(crate) struct DescriptorCopy(std::process::Child);

#[cfg(unix)]
impl DescriptorCopy {
    /// Spawn a process holding a copy of `file`'s descriptor.
    pub(crate) fn spawn(file: &std::fs::File) -> Self {
        let copy = file
            .try_clone()
            .unwrap_or_else(|error| panic!("a copy of the descriptor: {error}"));
        #[allow(clippy::disallowed_methods)] // the test's own process, which holds the copy
        let child = std::process::Command::new("sleep")
            .arg("600")
            .stdin(copy)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap_or_else(|error| panic!("a process holding the copy: {error}"));
        Self(child)
    }
}

#[cfg(unix)]
impl Drop for DescriptorCopy {
    fn drop(&mut self) {
        // On request (MEERKAT_TEST_DESCRIPTOR_COPY_LINGER_MS), the copy stays open
        // that long first, so an fd sampler watching the run sees it.
        if let Some(linger) = std::env::var("MEERKAT_TEST_DESCRIPTOR_COPY_LINGER_MS")
            .ok()
            .and_then(|millis| millis.parse::<u64>().ok())
        {
            std::thread::sleep(std::time::Duration::from_millis(linger));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
