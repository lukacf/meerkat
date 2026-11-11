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
//!
//! The child-process runner compiles on Linux only, where every test that
//! runs alone lives; elsewhere it would be dead code.
//!
//! A row on the mode of a file the crate writes runs its scenario the same
//! way, once per umask ([`under_each_umask`]): the umask is process-wide, so
//! the shell that starts the child sets it, and the shared test process
//! never changes its own. [`under_each_umask_refusing_writes`] also makes
//! that child unable to write a byte to any regular file, so a file a write
//! created stays as its creation left it.

/// Set in the child process a test runs alone in.
#[cfg(target_os = "linux")]
const ALONE_CHILD: &str = "MEERKAT_CORE_TEST_ALONE";
/// Printed by that child once the test's body has passed.
#[cfg(target_os = "linux")]
const ALONE_CHILD_DONE: &str = "meerkat-core test passed alone";

/// Run `body`, the body of the test `test` (its path in this binary, such as
/// `confinement::tests::a_row`), alone in a child process of this test
/// binary, and require it to pass there.
#[cfg(target_os = "linux")]
pub(crate) fn alone_in_child_process(test: &str, body: impl FnOnce()) {
    if std::env::var_os(ALONE_CHILD).is_some() {
        body();
        println!("\n{ALONE_CHILD_DONE}");
        return;
    }
    run_in_child_process(test);
}

#[cfg(target_os = "linux")]
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

/// The umasks a file mode row runs under: the common default, a shared-group
/// default, and the private one.
#[cfg(unix)]
const UMASKS: [&str; 3] = ["022", "002", "077"];
/// Set to its umask in the child process a file mode row's scenario runs in.
#[cfg(unix)]
const UMASK_CHILD: &str = "MEERKAT_CORE_TEST_UMASK_CHILD";
/// Printed by that child, with its umask, once the scenario passed.
#[cfg(unix)]
const UMASK_CHILD_DONE: &str = "meerkat-core scenario passed under umask";
/// Printed by a child whose platform does not refuse writes under a zero
/// file size limit, in place of running the scenario.
#[cfg(unix)]
const WRITES_NOT_REFUSED: &str =
    "NOT exercised: a zero file size limit does not refuse writes here";

/// What the shell that starts a file mode row's child sets before it
/// executes the test binary.
#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ChildWrites {
    /// Only the umask.
    Allowed,
    /// The umask, a zero file size limit and an ignored `SIGXFSZ`: a file
    /// can be created, and every write of a byte to a regular file fails
    /// (`EFBIG`), so a file stays as its creation left it.
    Refused,
}

/// Run `scenario`, the body of the test `test` in the module `module` (the
/// caller's `module_path!()`), under each of [`UMASKS`]; the scenario gets
/// its umask. The umask is process-wide, so the shared test process never
/// changes it: each run is a child process of this test binary, started by a
/// shell that sets the umask before exec, that runs `test` alone.
#[cfg(unix)]
pub(crate) fn under_each_umask<Scenario, Run>(module: &str, test: &str, scenario: Scenario)
where
    Scenario: FnOnce(u32) -> Run,
    Run: std::future::Future<Output = ()>,
{
    under_each_umask_in(ChildWrites::Allowed, module, test, scenario);
}

/// [`under_each_umask`] in a child whose writes to regular files are all
/// refused, so the scenario sees each file exactly as its creation left it,
/// before any byte. On Linux the limit always refuses the write, so a child
/// whose probe write succeeded fails the row. Elsewhere, where a platform
/// may not enforce the limit this way, the scenario does not run and the
/// row says so on standard error.
#[cfg(unix)]
pub(crate) fn under_each_umask_refusing_writes<Scenario, Run>(
    module: &str,
    test: &str,
    scenario: Scenario,
) where
    Scenario: FnOnce(u32) -> Run,
    Run: std::future::Future<Output = ()>,
{
    under_each_umask_in(ChildWrites::Refused, module, test, scenario);
}

#[cfg(unix)]
fn under_each_umask_in<Scenario, Run>(
    writes: ChildWrites,
    module: &str,
    test: &str,
    scenario: Scenario,
) where
    Scenario: FnOnce(u32) -> Run,
    Run: std::future::Future<Output = ()>,
{
    if let Some(umask) = std::env::var_os(UMASK_CHILD) {
        let umask = umask.to_string_lossy().into_owned();
        if writes == ChildWrites::Refused && !writes_are_refused() {
            eprintln!("{test}: {WRITES_NOT_REFUSED}");
            println!("\n{WRITES_NOT_REFUSED} {umask}");
            return;
        }
        let bits = u32::from_str_radix(&umask, 8)
            .unwrap_or_else(|error| panic!("the child's umask {umask}: {error}"));
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("the scenario's runtime: {error}"))
            .block_on(scenario(bits));
        println!("\n{UMASK_CHILD_DONE} {umask}");
        return;
    }
    let binary = std::env::current_exe().unwrap_or_else(|error| panic!("the test binary: {error}"));
    let module = module.split_once("::").map_or_else(
        || panic!("{module} is not a module path within the crate"),
        |(_, module)| module,
    );
    let test_path = format!("{module}::{test}");
    let script = match writes {
        ChildWrites::Allowed => "umask \"$1\" && shift && exec \"$@\"",
        ChildWrites::Refused => {
            "trap '' XFSZ && ulimit -f 0 && umask \"$1\" && shift && exec \"$@\""
        }
    };
    for umask in UMASKS {
        #[allow(clippy::disallowed_methods)] // the test re-runs its own binary
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", script, "file-mode-umask", umask])
            .arg(binary.as_os_str())
            .args([
                "--exact",
                test_path.as_str(),
                "--nocapture",
                "--test-threads=1",
            ])
            .env(UMASK_CHILD, umask)
            .output()
            .unwrap_or_else(|error| panic!("{test} could not start under a umask: {error}"));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let not_refused =
            output.status.success() && stdout.contains(&format!("{WRITES_NOT_REFUSED} {umask}"));
        if not_refused && !cfg!(target_os = "linux") {
            eprintln!("{test}: {WRITES_NOT_REFUSED} (umask {umask})");
            continue;
        }
        // On Linux the limit always refuses the write, so a probe write that
        // succeeded there means the recipe broke: a proof that did not run
        // must not pass.
        assert!(
            !not_refused,
            "{test} proved nothing under umask {umask}: {WRITES_NOT_REFUSED}, and Linux always \
             does"
        );
        assert!(
            output.status.success() && stdout.contains(&format!("{UMASK_CHILD_DONE} {umask}")),
            "{test} failed under umask {umask} ({}):\n{stdout}\n{stderr}",
            output.status,
        );
    }
}

/// Whether this process, started with a zero file size limit, is refused a
/// write of one byte to a regular file.
#[cfg(unix)]
fn writes_are_refused() -> bool {
    let probe = tempfile::tempdir().unwrap_or_else(|error| panic!("a probe directory: {error}"));
    std::fs::write(probe.path().join("probe"), b"x").is_err()
}

/// The permission bits of `path` itself (a final symbolic link is not
/// followed), with the set-id and sticky bits.
#[cfg(unix)]
pub(crate) fn mode_bits(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("the metadata of {}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o7777
}

/// Set the permission bits of `path`.
#[cfg(unix)]
pub(crate) fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .unwrap_or_else(|error| panic!("set the mode of {}: {error}", path.display()));
}

/// Create `directory` as one shared through its group on purpose: setgid,
/// mode 2770, and return its group. The group is another group of the test
/// user when it has one, so a file that takes the directory's group shows
/// it; otherwise the row says on standard error that the group is the test
/// user's own.
#[cfg(unix)]
pub(crate) fn create_setgid_directory(directory: &std::path::Path) -> u32 {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::create_dir(directory)
        .unwrap_or_else(|error| panic!("create {}: {error}", directory.display()));
    let own = rustix::process::getegid().as_raw();
    let other = rustix::process::getgroups()
        .unwrap_or_else(|error| panic!("the test user's groups: {error}"))
        .into_iter()
        .map(rustix::fs::Gid::as_raw)
        .find(|group| *group != own);
    match other {
        Some(group) => {
            rustix::fs::chown(directory, None, Some(rustix::fs::Gid::from_raw(group)))
                .unwrap_or_else(|error| {
                    panic!("give {} group {group}: {error}", directory.display())
                });
        }
        None => eprintln!(
            "file mode rows: the shared directory's group is the test user's own (the test \
             user has no second group)"
        ),
    }
    set_mode(directory, 0o2770);
    assert_eq!(mode_bits(directory), 0o2770, "the directory is setgid");
    std::fs::metadata(directory)
        .unwrap_or_else(|error| panic!("the metadata of {}: {error}", directory.display()))
        .gid()
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
