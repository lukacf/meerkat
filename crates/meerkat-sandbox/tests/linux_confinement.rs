//! Linux OS probes for the LinuxLandlockSeccompV1 profile (ADR-001 slice 8).
//!
//! Every denial is checked by a native child that reaches its own assertion,
//! paired with a permitted control in the same test. A typed refusal of a
//! requirement the profile cannot enforce is asserted together with a positive
//! control for the remaining supported profile, so an absent backend is RED,
//! never green. Backend or setup failure is a test failure, never a skip.
//!
//! There is no helper binary: the forked child installs the compiled
//! restrictions on itself and executes the launch under the same PID, so no
//! installation needs protecting from a target's writes.
#![cfg(target_os = "linux")]
#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Output;

use meerkat_core::confinement::{
    ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess, PathAccess,
    PlatformBaseline,
};
use meerkat_sandbox::{
    CompiledConfinement, ConfinementBackend, ConfinementRefusal, PreparedConfinement,
    ProcessLaunchSpec, SpawnIo, prepare,
};
use nix::libc;

const PASSED: &str = "linux native probe passed";
const CANARY: &str = "private canary\n";

fn test_executable() -> PathBuf {
    std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap()
}

struct Fixture {
    _root: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let work = root.join("work");
        std::fs::create_dir(&work).unwrap();
        let outside = root.join("outside");
        std::fs::write(&outside, CANARY).unwrap();
        Self {
            _root: temp,
            root,
            work,
            outside,
        }
    }

    fn spec(&self) -> ConfinementSpec {
        ConfinementSpec {
            baseline: PlatformBaseline::CommandRuntimeV1,
            read: FilesystemAccess::Paths(vec![PathAccess::Subtree(self.work.clone())]),
            write: FilesystemAccess::Paths(vec![PathAccess::Subtree(self.work.clone())]),
            deny_read: vec![],
            deny_write: vec![],
            network: IpNetworkAccess::Denied,
            unix_connect: vec![],
            require_descendant_termination: false,
        }
    }

    fn launch(
        &self,
        program: PathBuf,
        arguments: Vec<OsString>,
        env: &[(&str, &str)],
    ) -> ProcessLaunchSpec {
        ProcessLaunchSpec::new(
            program,
            arguments,
            self.work.clone(),
            env.iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value)))
                .collect(),
        )
        .unwrap()
    }

    fn prepare_shell(
        &self,
        spec: ConfinementSpec,
        script: &str,
        args: &[OsString],
    ) -> Result<PreparedConfinement, ConfinementRefusal> {
        let mut arguments = vec![
            OsString::from("-c"),
            OsString::from(script),
            OsString::from("probe"),
        ];
        arguments.extend_from_slice(args);
        prepare(
            &ExecutionConfinement::try_from(spec).unwrap(),
            self.launch(
                PathBuf::from("/bin/sh"),
                arguments,
                &[("PATH", "/usr/bin:/bin")],
            ),
        )
    }

    async fn shell(&self, spec: ConfinementSpec, script: &str, args: &[OsString]) -> Output {
        self.prepare_shell(spec, script, args)
            .unwrap_or_else(prepare_failure)
            .output()
            .await
            .unwrap()
    }

    fn prepare_probe(
        &self,
        mut spec: ConfinementSpec,
        mode: &str,
        argument: &str,
    ) -> Result<PreparedConfinement, ConfinementRefusal> {
        let executable = test_executable();
        if let FilesystemAccess::Paths(paths) = &mut spec.read {
            // This exact test executable, not its build directory.
            paths.push(PathAccess::Literal(executable.clone()));
        }
        prepare(
            &ExecutionConfinement::try_from(spec).unwrap(),
            self.launch(
                executable,
                ["--ignored", "--exact", "linux_probe_process", "--nocapture"]
                    .into_iter()
                    .map(OsString::from)
                    .collect(),
                &[
                    ("MEERKAT_LINUX_PROBE", mode),
                    ("MEERKAT_PROBE_ARGUMENT", argument),
                    // Proxy variables are data, never network permission.
                    ("HTTP_PROXY", "http://127.0.0.1:9"),
                    ("ALL_PROXY", "socks5://127.0.0.1:9"),
                    ("NO_PROXY", "*"),
                ],
            ),
        )
    }

    fn probe(&self, spec: ConfinementSpec, mode: &str, argument: &str) -> PreparedConfinement {
        self.prepare_probe(spec, mode, argument)
            .unwrap_or_else(prepare_failure)
    }

    async fn assert_probe(&self, spec: ConfinementSpec, mode: &str, argument: &str) {
        assert_probe(self.probe(spec, mode, argument).output().await.unwrap());
    }

    fn assert_refused(&self, spec: ConfinementSpec, expected: ConfinementRefusal) {
        let refusal = self
            .prepare_shell(spec, "printf 'must not run'", &[])
            .expect_err("an unenforceable requirement must be refused at setup");
        assert_eq!(refusal, expected);
    }
}

fn assert_probe(output: Output) {
    assert!(output.status.success(), "native probe failed: {output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(PASSED),
        "native probe did not reach its assertions: {output:?}"
    );
}

fn assert_completed(output: &Output, marker: &[u8]) {
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, marker, "{output:?}");
}

/// The kernel's Landlock ABI from a real `landlock_create_ruleset` version
/// query, or `None` when Landlock is absent.
fn kernel_landlock_abi() -> Option<i64> {
    const VERSION: u32 = 1;
    // SAFETY: the version query reads no attribute and creates no descriptor.
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<u8>(),
            0usize,
            VERSION,
        )
    };
    (abi > 0).then_some(abi)
}

/// The minimum Landlock ABI the profile needs (signal and abstract-socket
/// scopes); below it the backend is BackendUnavailable by contract, and these
/// tests are UNVERIFIED rather than passed (see `verified`).
const REQUIRED_LANDLOCK_ABI: i64 = 6;

/// Set on hosts where Linux confinement support is claimed: an insufficient
/// kernel is then a hard failure instead of an UNVERIFIED early return. The
/// value is the minimum ABI to demand (never below the profile's own minimum).
const REQUIRE_ABI_ENV: &str = "MEERKAT_REQUIRE_LANDLOCK_ABI";

fn required_abi() -> (i64, bool) {
    match std::env::var(REQUIRE_ABI_ENV) {
        Ok(value) => {
            let demanded: i64 = value
                .parse()
                .unwrap_or_else(|_| panic!("{REQUIRE_ABI_ENV} must be an integer ABI"));
            (demanded.max(REQUIRED_LANDLOCK_ABI), true)
        }
        Err(_) => (REQUIRED_LANDLOCK_ABI, false),
    }
}

/// Every test that needs the profile's kernel facilities, for the summary.
const GATED_TESTS: &[&str] = &[
    "exact_argv_cwd_and_environment_reach_a_shell_and_a_rust_child",
    "permitted_files_work_while_outside_symlink_rename_and_hardlink_escapes_fail",
    "literal_file_grant_is_exact_and_never_widens_to_its_directory",
    "literal_directory_and_missing_literal_grants_are_refused_with_a_positive_control",
    "exclusions_inside_a_grant_are_refused_because_landlock_cannot_subtract",
    "exclusions_outside_every_grant_are_enforced",
    "mutable_symlink_in_a_granted_path_is_refused_with_a_positive_control",
    "baseline_runs_declared_commands_but_grants_no_user_data",
    "ip_denied_blocks_live_tcp_udp_families_and_io_uring_with_unrestricted_control",
    "exact_connect_endpoints_are_refused_with_a_positive_control",
    "unix_sockets_are_independent_of_ip_and_of_filesystem_grants",
    "exact_unix_socket_grant_is_refused_with_a_positive_control",
    "inherited_descriptors_do_not_survive_and_parent_state_is_unchanged",
    "simultaneous_agents_with_disjoint_roots_stay_isolated",
    "refusal_is_operation_local_and_a_later_permitted_launch_works",
    "target_cannot_signal_trace_or_read_the_host_or_a_sibling_sandbox",
    "custody_gate_waits_for_release_keeps_the_pid_and_never_runs_on_eof",
    "one_compilation_binds_distinct_launches_with_an_exact_capability_report",
    "replaced_or_retargeted_grant_paths_refuse_the_launch_instead_of_widening",
    "write_unrestricted_runs_without_reaching_other_processes_or_the_network",
    "exec_chain_keeps_the_pid_and_reports_the_targets_own_outcome",
    "absent_landlock_is_backend_unavailable_and_the_present_kernel_still_works",
    "profile_works_where_unprivileged_user_namespaces_are_unusable",
    "python_multiprocessing_runs_with_an_explicit_shared_memory_grant_only",
    "node_child_processes_run_under_the_baseline",
    "git_init_add_and_commit_run_under_the_baseline",
];

/// Whether this kernel can exercise the profile. Below the required ABI the
/// test prints one UNVERIFIED line and the caller returns early: that is not
/// a pass claim (the summary counts it), and in required mode it fails.
fn verified(test: &str) -> bool {
    assert!(
        GATED_TESTS.contains(&test),
        "{test} missing from GATED_TESTS"
    );
    let (required, required_mode) = required_abi();
    match kernel_landlock_abi() {
        Some(abi) if abi >= required => true,
        abi => {
            let line = format!(
                "UNVERIFIED: Landlock ABI {} below required {required} ({test})",
                abi.map_or_else(|| "absent".to_owned(), |abi| abi.to_string())
            );
            assert!(
                !required_mode,
                "{line}; {REQUIRE_ABI_ENV} demands verification"
            );
            println!("{line}");
            false
        }
    }
}

/// A failed preparation of a profile this verified kernel must support.
fn prepare_failure(refusal: ConfinementRefusal) -> PreparedConfinement {
    panic!("the supported Landlock/seccomp profile must prepare: {refusal:?}")
}

/// Counts verified against unverified gated tests for this kernel; in
/// required mode an unverifiable kernel fails here too.
#[test]
fn verification_summary() {
    let (required, required_mode) = required_abi();
    let abi = kernel_landlock_abi();
    let verified = abi.is_some_and(|abi| abi >= required);
    let (verified_count, unverified_count) = if verified {
        (GATED_TESTS.len(), 0)
    } else {
        (0, GATED_TESTS.len())
    };
    println!(
        "linux confinement verification: verified={verified_count} \
         unverified={unverified_count} (Landlock ABI {abi:?}, required {required}, \
         required mode {required_mode})"
    );
    assert!(
        verified || !required_mode,
        "{REQUIRE_ABI_ENV} is set but this kernel cannot verify the profile"
    );
}

// A native probe never treats a missing command or unrelated error as a policy
// denial. It runs only as a confined child (or, for the facility probes, as an
// unconfined child that removes a kernel facility from itself first).
#[test]
#[ignore = "subprocess entrypoint for Linux confinement probes"]
fn linux_probe_process() {
    use std::time::Duration;
    fn denied<T: std::fmt::Debug>(what: &str, result: std::io::Result<T>) {
        match result {
            Err(error) if matches!(error.raw_os_error(), Some(libc::EPERM | libc::EACCES)) => {}
            Err(error) => panic!("{what}: unrelated failure instead of policy denial: {error}"),
            Ok(value) => panic!("{what}: forbidden operation succeeded: {value:?}"),
        }
    }
    fn errno_result(value: libc::c_long) -> std::io::Result<libc::c_long> {
        if value < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(value)
        }
    }
    fn raw_socket(domain: libc::c_int, kind: libc::c_int) -> std::io::Result<libc::c_long> {
        // SAFETY: plain syscall; a returned descriptor is closed.
        let fd = errno_result(unsafe { libc::socket(domain, kind | libc::SOCK_CLOEXEC, 0) }.into());
        if let Ok(fd) = fd {
            unsafe { libc::close(fd as i32) };
        }
        fd
    }
    let mode = std::env::var("MEERKAT_LINUX_PROBE").expect("probe mode");
    let argument = std::env::var("MEERKAT_PROBE_ARGUMENT").unwrap_or_default();
    match mode.as_str() {
        "tcp-denied" => {
            let endpoint = argument.parse().unwrap();
            denied(
                "tcp connect",
                std::net::TcpStream::connect_timeout(&endpoint, Duration::from_secs(2)),
            );
            denied("tcp bind", std::net::TcpListener::bind("127.0.0.1:0"));
        }
        "tcp-allowed" => {
            std::net::TcpStream::connect_timeout(
                &argument.parse().unwrap(),
                Duration::from_secs(2),
            )
            .expect("unrestricted IP connection must work");
        }
        "udp-denied" => match std::net::UdpSocket::bind("127.0.0.1:0") {
            Ok(socket) => denied("udp send", socket.send_to(b"forbidden", &argument)),
            Err(error) => denied::<()>("udp socket", Err(error)),
        },
        "udp-allowed" => {
            let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("udp socket");
            socket
                .send_to(b"allowed", &argument)
                .expect("unrestricted IP datagram must send");
        }
        "ip-families-denied" => {
            // Every IP-capable family, not only the TCP that Landlock's net
            // rights cover; the Unix control proves sockets work at all.
            for (name, domain, kind) in [
                ("inet stream", libc::AF_INET, libc::SOCK_STREAM),
                ("inet dgram", libc::AF_INET, libc::SOCK_DGRAM),
                ("inet6 stream", libc::AF_INET6, libc::SOCK_STREAM),
                ("inet6 dgram", libc::AF_INET6, libc::SOCK_DGRAM),
                ("packet", libc::AF_PACKET, libc::SOCK_DGRAM),
                ("netlink", libc::AF_NETLINK, libc::SOCK_RAW),
            ] {
                denied(name, raw_socket(domain, kind));
            }
            let mut pair = [0; 2];
            // SAFETY: socketpair writes two descriptors into `pair`.
            assert_eq!(
                unsafe {
                    libc::socketpair(
                        libc::AF_UNIX,
                        libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
                        0,
                        pair.as_mut_ptr(),
                    )
                },
                0,
                "socketpair control"
            );
        }
        "io-uring-denied" => {
            // IORING_OP_SOCKET/CONNECT would bypass a socket(2) filter.
            let mut params = [0u8; 120];
            // SAFETY: io_uring_setup reads and writes the 120-byte params block.
            denied(
                "io_uring_setup",
                errno_result(unsafe {
                    libc::syscall(libc::SYS_io_uring_setup, 4u32, params.as_mut_ptr())
                }),
            );
        }
        "unix-denied" => denied(
            "unix pathname connect",
            std::os::unix::net::UnixStream::connect(&argument),
        ),
        "unix-abstract-denied" => {
            use std::os::linux::net::SocketAddrExt;
            let address =
                std::os::unix::net::SocketAddr::from_abstract_name(argument.as_bytes()).unwrap();
            denied(
                "unix abstract connect",
                std::os::unix::net::UnixStream::connect_addr(&address),
            );
        }
        "fd-closed" => {
            for descriptor in argument.split(',') {
                let descriptor: i32 = descriptor.parse().unwrap();
                // SAFETY: read-only descriptor query.
                let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
                assert_eq!(
                    flags, -1,
                    "descriptor {descriptor} survived into the target"
                );
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::EBADF)
                );
            }
        }
        "host-process-denied" => {
            let host: libc::pid_t = argument.parse().unwrap();
            // SAFETY: signal 0 only checks permission.
            denied(
                "signal host",
                errno_result(unsafe { libc::kill(host, 0) }.into()),
            );
            let mut buffer = [0u8; 8];
            let local = libc::iovec {
                iov_base: buffer.as_mut_ptr().cast(),
                iov_len: buffer.len(),
            };
            let remote = libc::iovec {
                iov_base: std::ptr::null_mut(),
                iov_len: buffer.len(),
            };
            // SAFETY: the kernel refuses before touching `remote`; `local` is ours.
            denied(
                "process_vm_readv host",
                errno_result(unsafe {
                    libc::process_vm_readv(
                        host,
                        std::ptr::from_ref(&local),
                        1,
                        std::ptr::from_ref(&remote),
                        1,
                        0,
                    )
                } as libc::c_long),
            );
            // SAFETY: PTRACE_ATTACH with no data pointer.
            denied(
                "ptrace host",
                errno_result(unsafe { libc::ptrace(libc::PTRACE_ATTACH, host, 0, 0) }),
            );
            // SAFETY: signal 0 to self is the permitted control.
            assert_eq!(
                unsafe { libc::kill(libc::getpid(), 0) },
                0,
                "self signal control"
            );
        }
        "read-denied" => denied("read", std::fs::read(&argument)),
        "write-denied" => denied("write", std::fs::write(&argument, b"forbidden")),
        "pid" => {
            println!("\nMEERKAT_PROBE_PID={}", std::process::id());
        }
        "landlock-unavailable" => {
            // Unconfined child: remove Landlock from this process the way a
            // kernel booted without it reports (ENOSYS), then demand the exact
            // typed refusal from real setup. The work directory is the argument.
            install_enosys_for_landlock();
            let work = PathBuf::from(&argument);
            let spec = ConfinementSpec {
                baseline: PlatformBaseline::CommandRuntimeV1,
                read: FilesystemAccess::Paths(vec![PathAccess::Subtree(work.clone())]),
                write: FilesystemAccess::Paths(vec![PathAccess::Subtree(work.clone())]),
                deny_read: vec![],
                deny_write: vec![],
                network: IpNetworkAccess::Denied,
                unix_connect: vec![],
                require_descendant_termination: false,
            };
            let launch =
                ProcessLaunchSpec::new(PathBuf::from("/bin/true"), vec![], work, BTreeMap::new())
                    .unwrap();
            assert_eq!(
                prepare(&ExecutionConfinement::try_from(spec).unwrap(), launch).err(),
                Some(ConfinementRefusal::BackendUnavailable),
                "absent Landlock must be BackendUnavailable, never an unconfined launch"
            );
        }
        _ => panic!("unknown native probe"),
    }
    print!("{PASSED}");
    std::io::stdout().flush().unwrap();
    std::process::exit(0);
}

/// Makes the three Landlock syscalls fail with ENOSYS for this process only.
fn install_enosys_for_landlock() {
    const RET_ERRNO: u32 = 0x0005_0000;
    const RET_ALLOW: u32 = 0x7fff_0000;
    let nr_offset = 0u32; // offsetof(struct seccomp_data, nr)
    let stmt = |code: u16, k: u32| libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    };
    let jeq = |k: u32, jt: u8, jf: u8| libc::sock_filter {
        code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
        jt,
        jf,
        k,
    };
    let mut program = [
        stmt(
            (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
            nr_offset,
        ),
        jeq(libc::SYS_landlock_create_ruleset as u32, 3, 0),
        jeq(libc::SYS_landlock_add_rule as u32, 2, 0),
        jeq(libc::SYS_landlock_restrict_self as u32, 1, 0),
        stmt((libc::BPF_RET | libc::BPF_K) as u16, RET_ALLOW),
        stmt(
            (libc::BPF_RET | libc::BPF_K) as u16,
            RET_ERRNO | libc::ENOSYS as u32,
        ),
    ];
    let filter = libc::sock_fprog {
        len: program.len() as u16,
        filter: program.as_mut_ptr(),
    };
    // SAFETY: a valid classic BPF program for this process only.
    unsafe {
        assert_eq!(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0), 0);
        assert_eq!(
            libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                std::ptr::from_ref(&filter)
            ),
            0
        );
    }
}

// 1. Exact argv, cwd and environment; a shell and a Rust child both start.

#[tokio::test]
async fn exact_argv_cwd_and_environment_reach_a_shell_and_a_rust_child() {
    if !verified("exact_argv_cwd_and_environment_reach_a_shell_and_a_rust_child") {
        return;
    }
    let fixture = Fixture::new();
    let script = r#"
        test "$1" = 'two words; $(not expanded)' || exit 10
        test "$#" = 1 || exit 11
        test "$(pwd -P)" = "$WORK_EXPECTED" || exit 12
        test -z "${HOME+set}" || exit 13
        test -z "${OPENAI_API_KEY+set}" || exit 14
        printf 'exact'
    "#;
    let work = fixture.work.to_str().unwrap().to_owned();
    let prepared = prepare(
        &ExecutionConfinement::try_from(fixture.spec()).unwrap(),
        fixture.launch(
            PathBuf::from("/bin/sh"),
            vec![
                OsString::from("-c"),
                OsString::from(script),
                OsString::from("probe"),
                OsString::from("two words; $(not expanded)"),
            ],
            &[("PATH", "/usr/bin:/bin"), ("WORK_EXPECTED", &work)],
        ),
    )
    .unwrap_or_else(prepare_failure);
    let output = prepared.output().await.unwrap();
    assert_completed(&output, b"exact");
    // A Rust child initializes std (threads, cwd) under the same baseline.
    fixture.assert_probe(fixture.spec(), "pid", "").await;
}

// 2. Files: permitted read/write beside a forbidden file; symlink, rename,
//    hardlink and missing-descendant attacks; exact literal grants.

#[tokio::test]
async fn permitted_files_work_while_outside_symlink_rename_and_hardlink_escapes_fail() {
    if !verified("permitted_files_work_while_outside_symlink_rename_and_hardlink_escapes_fail") {
        return;
    }
    let fixture = Fixture::new();
    let output = fixture
        .shell(
            fixture.spec(),
            r#"
        printf 'allowed' > inside || exit 20
        test "$(cat inside)" = allowed || exit 21
        mkdir sub && mv sub sub2 || exit 22
        ln inside inside-link || exit 23
        if cat "$1"; then exit 24; fi
        if printf 'changed' > "$1"; then exit 25; fi
        ln -s "$1" alias || exit 26
        if cat alias; then exit 27; fi
        if ln "$1" hard; then exit 28; fi
        if mv "$1" moved; then exit 29; fi
        if ls "$2" >/dev/null; then exit 30; fi
        printf 'completed'
    "#,
            &[
                fixture.outside.as_os_str().to_owned(),
                fixture.root.as_os_str().to_owned(),
            ],
        )
        .await;
    assert_completed(&output, b"completed");
    assert_eq!(std::fs::read_to_string(&fixture.outside).unwrap(), CANARY);
}

#[tokio::test]
async fn literal_file_grant_is_exact_and_never_widens_to_its_directory() {
    if !verified("literal_file_grant_is_exact_and_never_widens_to_its_directory") {
        return;
    }
    let fixture = Fixture::new();
    let shared = fixture.root.join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::write(shared.join("granted"), "granted").unwrap();
    std::fs::write(shared.join("sibling"), "sibling").unwrap();
    let mut spec = fixture.spec();
    if let FilesystemAccess::Paths(paths) = &mut spec.read {
        paths.push(PathAccess::Literal(shared.join("granted")));
    }
    let output = fixture
        .shell(
            spec,
            r#"
        test "$(cat "$1/granted")" = granted || exit 31
        if cat "$1/sibling"; then exit 32; fi
        if ls "$1" >/dev/null; then exit 33; fi
        if printf 'x' >> "$1/granted"; then exit 34; fi
        printf 'exact'
    "#,
            &[shared.as_os_str().to_owned()],
        )
        .await;
    assert_completed(&output, b"exact");
}

#[tokio::test]
async fn literal_directory_and_missing_literal_grants_are_refused_with_a_positive_control() {
    if !verified("literal_directory_and_missing_literal_grants_are_refused_with_a_positive_control")
    {
        return;
    }
    // Landlock's directory rule always covers descendants, and a rule can only
    // bind an object that exists, so neither is the requested exact grant.
    let fixture = Fixture::new();
    let mut directory = fixture.spec();
    directory.read = FilesystemAccess::Paths(vec![PathAccess::Literal(fixture.work.clone())]);
    fixture.assert_refused(directory, ConfinementRefusal::UnsupportedRequirement);
    let mut missing = fixture.spec();
    missing.write = FilesystemAccess::Paths(vec![PathAccess::Literal(fixture.work.join("later"))]);
    fixture.assert_refused(missing, ConfinementRefusal::UnsupportedRequirement);
    let output = fixture
        .shell(fixture.spec(), "printf 'supported'", &[])
        .await;
    assert_completed(&output, b"supported");
}

#[tokio::test]
async fn exclusions_inside_a_grant_are_refused_because_landlock_cannot_subtract() {
    if !verified("exclusions_inside_a_grant_are_refused_because_landlock_cannot_subtract") {
        return;
    }
    // Allow-minus-deny is not representable in an allow-only ruleset, and a
    // directory snapshot is never complete future policy. bwrap is unavailable
    // here, so the only correct result is a typed refusal.
    let fixture = Fixture::new();
    for (deny_read, deny_write) in [
        (
            vec![PathAccess::Literal(fixture.work.join("secret"))],
            vec![],
        ),
        (
            vec![],
            vec![PathAccess::Subtree(fixture.work.join("protected"))],
        ),
        (vec![PathAccess::Subtree(PathBuf::from("/usr/lib"))], vec![]),
    ] {
        let mut spec = fixture.spec();
        spec.deny_read = deny_read;
        spec.deny_write = deny_write;
        fixture.assert_refused(spec, ConfinementRefusal::UnsupportedRequirement);
    }
    let output = fixture
        .shell(fixture.spec(), "printf 'supported'", &[])
        .await;
    assert_completed(&output, b"supported");
}

#[tokio::test]
async fn exclusions_outside_every_grant_are_enforced() {
    if !verified("exclusions_outside_every_grant_are_enforced") {
        return;
    }
    let fixture = Fixture::new();
    let mut spec = fixture.spec();
    spec.deny_read
        .push(PathAccess::Literal(fixture.outside.clone()));
    spec.deny_write
        .push(PathAccess::Literal(fixture.outside.clone()));
    let output = fixture
        .shell(
            spec,
            r#"
        printf 'ok' > inside || exit 35
        if cat "$1"; then exit 36; fi
        if printf 'x' > "$1"; then exit 37; fi
        printf 'enforced'
    "#,
            &[fixture.outside.as_os_str().to_owned()],
        )
        .await;
    assert_completed(&output, b"enforced");
    assert_eq!(std::fs::read_to_string(&fixture.outside).unwrap(), CANARY);
}

#[tokio::test]
async fn mutable_symlink_in_a_granted_path_is_refused_with_a_positive_control() {
    if !verified("mutable_symlink_in_a_granted_path_is_refused_with_a_positive_control") {
        return;
    }
    let fixture = Fixture::new();
    let alias = fixture.work.join("alias");
    std::os::unix::fs::symlink(&fixture.root, &alias).unwrap();
    let mut spec = fixture.spec();
    spec.write = FilesystemAccess::Paths(vec![PathAccess::Subtree(alias.join("work"))]);
    fixture.assert_refused(spec, ConfinementRefusal::UnsupportedRequirement);
    let output = fixture
        .shell(fixture.spec(), "printf 'supported'", &[])
        .await;
    assert_completed(&output, b"supported");
}

#[tokio::test]
async fn baseline_runs_declared_commands_but_grants_no_user_data() {
    if !verified("baseline_runs_declared_commands_but_grants_no_user_data") {
        return;
    }
    let fixture = Fixture::new();
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home"));
    let output = fixture
        .shell(
            fixture.spec(),
            r#"
        mkdir nested || exit 40
        printf 'beta\nalpha\n' > nested/input || exit 41
        sort nested/input | sed -n '1p' > nested/output || exit 42
        test "$(cat nested/output)" = alpha || exit 43
        /bin/sh -c 'printf child' > nested/child || exit 44
        test "$(cat nested/child)" = child || exit 45
        rm -r nested || exit 46
        test -r /dev/urandom && printf '' > /dev/null || exit 47
        if ls "$1" >/dev/null; then exit 48; fi
        if cat /proc/1/environ; then exit 49; fi
        printf 'compatible'
    "#,
            &[home.into_os_string()],
        )
        .await;
    assert_completed(&output, b"compatible");
}

// 3. IP: Denied blocks TCP, UDP, every IP family and io_uring against live
//    peers; Unrestricted is the independent positive control; exact Connect
//    endpoints are refused because Landlock's TCP rule is port-only.

#[tokio::test]
async fn ip_denied_blocks_live_tcp_udp_families_and_io_uring_with_unrestricted_control() {
    if !verified("ip_denied_blocks_live_tcp_udp_families_and_io_uring_with_unrestricted_control") {
        return;
    }
    let fixture = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp = listener.local_addr().unwrap().to_string();
    let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver.set_nonblocking(true).unwrap();
    let udp = receiver.local_addr().unwrap().to_string();
    fixture
        .assert_probe(fixture.spec(), "tcp-denied", &tcp)
        .await;
    fixture
        .assert_probe(fixture.spec(), "udp-denied", &udp)
        .await;
    fixture
        .assert_probe(fixture.spec(), "ip-families-denied", "")
        .await;
    fixture
        .assert_probe(fixture.spec(), "io-uring-denied", "")
        .await;
    let mut buffer = [0u8; 16];
    assert!(
        receiver.recv(&mut buffer).is_err(),
        "a denied datagram reached the live receiver"
    );
    let mut unrestricted = fixture.spec();
    unrestricted.network = IpNetworkAccess::Unrestricted;
    fixture
        .assert_probe(unrestricted.clone(), "tcp-allowed", &tcp)
        .await;
    fixture
        .assert_probe(unrestricted, "udp-allowed", &udp)
        .await;
    receiver.set_nonblocking(false).unwrap();
    assert_eq!(receiver.recv(&mut buffer).unwrap(), b"allowed".len());
}

#[tokio::test]
async fn exact_connect_endpoints_are_refused_with_a_positive_control() {
    if !verified("exact_connect_endpoints_are_refused_with_a_positive_control") {
        return;
    }
    let fixture = Fixture::new();
    for endpoint in ["127.0.0.1:443", "[::1]:443", "192.0.2.10:8443"] {
        let mut spec = fixture.spec();
        spec.network = IpNetworkAccess::Connect(vec![endpoint.parse().unwrap()]);
        fixture.assert_refused(spec, ConfinementRefusal::UnsupportedRequirement);
    }
    let output = fixture
        .shell(fixture.spec(), "printf 'supported'", &[])
        .await;
    assert_completed(&output, b"supported");
}

// 4. Unix: no grant blocks live pathname and abstract sockets (the pathname one
//    inside the writable work root); IP Unrestricted grants no Unix access;
//    an exact Unix grant is enforced, or refused where the kernel cannot.

#[tokio::test]
async fn unix_sockets_are_independent_of_ip_and_of_filesystem_grants() {
    if !verified("unix_sockets_are_independent_of_ip_and_of_filesystem_grants") {
        return;
    }
    let fixture = Fixture::new();
    let socket = fixture.work.join("host.sock");
    let _pathname = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let name = format!("meerkat-linux-probe-{}", std::process::id());
    let _abstract = {
        use std::os::linux::net::SocketAddrExt;
        std::os::unix::net::UnixListener::bind_addr(
            &std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap(),
        )
        .unwrap()
    };
    fixture
        .assert_probe(fixture.spec(), "unix-denied", socket.to_str().unwrap())
        .await;
    fixture
        .assert_probe(fixture.spec(), "unix-abstract-denied", &name)
        .await;
    let mut ip_only = fixture.spec();
    ip_only.network = IpNetworkAccess::Unrestricted;
    fixture
        .assert_probe(ip_only.clone(), "unix-denied", socket.to_str().unwrap())
        .await;
    fixture
        .assert_probe(ip_only, "unix-abstract-denied", &name)
        .await;
}

#[tokio::test]
async fn exact_unix_socket_grant_is_refused_with_a_positive_control() {
    if !verified("exact_unix_socket_grant_is_refused_with_a_positive_control") {
        return;
    }
    let fixture = Fixture::new();
    let allowed = fixture.work.join("allowed.sock");
    let forbidden = fixture.work.join("forbidden.sock");
    let _allowed = std::os::unix::net::UnixListener::bind(&allowed).unwrap();
    let _forbidden = std::os::unix::net::UnixListener::bind(&forbidden).unwrap();
    // Landlock does not mediate connect(2) to a pathname socket, so an exact
    // socket grant is refused rather than emulated or widened.
    for access in [
        PathAccess::Literal(allowed.clone()),
        PathAccess::Subtree(fixture.work.clone()),
    ] {
        let mut spec = fixture.spec();
        spec.unix_connect.push(access);
        fixture.assert_refused(spec, ConfinementRefusal::UnsupportedRequirement);
    }
    // Positive control: the supported no-Unix profile still launches and
    // still cannot reach either live socket.
    fixture
        .assert_probe(fixture.spec(), "unix-denied", forbidden.to_str().unwrap())
        .await;
    fixture
        .assert_probe(fixture.spec(), "unix-denied", allowed.to_str().unwrap())
        .await;
}

// 5. Descriptors: sparse high, writable file, directory and connected socket
//    descriptors never reach the target; the parent's table is unchanged.

#[tokio::test]
async fn inherited_descriptors_do_not_survive_and_parent_state_is_unchanged() {
    if !verified("inherited_descriptors_do_not_survive_and_parent_state_is_unchanged") {
        return;
    }
    use std::os::fd::AsRawFd;
    let fixture = Fixture::new();
    let readable = std::fs::File::open(&fixture.outside).unwrap();
    let writable = std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.outside)
        .unwrap();
    let directory = std::fs::File::open(&fixture.root).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let connected = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    // Deliberately inheritable (no close-on-exec) and sparse, in the host
    // itself: the launch API exposes no pre-exec hook to inject them later.
    const TARGETS: [i32; 4] = [1000, 301, 302, 303];
    for (source, target) in [
        readable.as_raw_fd(),
        writable.as_raw_fd(),
        directory.as_raw_fd(),
        connected.as_raw_fd(),
    ]
    .into_iter()
    .zip(TARGETS)
    {
        // SAFETY: dup2 onto an unused descriptor number owned by this test.
        assert_eq!(unsafe { libc::dup2(source, target) }, target);
        assert_eq!(
            unsafe { libc::fcntl(target, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, std::ptr::from_mut(&mut limit)) },
        0
    );
    let argument = TARGETS.map(|fd| fd.to_string()).join(",");
    let output = fixture
        .probe(fixture.spec(), "fd-closed", &argument)
        .output()
        .await
        .unwrap();
    let mut after = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, std::ptr::from_mut(&mut after)) },
        0
    );
    // The parent's table and flags are unchanged by the child's seal.
    for target in TARGETS {
        assert_eq!(
            unsafe { libc::fcntl(target, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        // SAFETY: closing the duplicates this test created.
        unsafe { libc::close(target) };
    }
    assert_probe(output);
    assert_eq!(
        (after.rlim_cur, after.rlim_max),
        (limit.rlim_cur, limit.rlim_max)
    );
    assert_eq!(std::fs::read_to_string(&fixture.outside).unwrap(), CANARY);
}

// 6. Two agents with disjoint roots run concurrently; a refusal leaves the
//    same owner able to launch an unrelated permitted command; a sandboxed
//    target cannot signal or inspect the host or a sibling sandbox.

#[tokio::test]
async fn simultaneous_agents_with_disjoint_roots_stay_isolated() {
    if !verified("simultaneous_agents_with_disjoint_roots_stay_isolated") {
        return;
    }
    let first = Fixture::new();
    let second = Fixture::new();
    std::fs::write(first.work.join("private"), "first").unwrap();
    std::fs::write(second.work.join("private"), "second").unwrap();
    let script = r#"test "$(cat private)" = "$1" || exit 50
        if cat "$2/private"; then exit 51; fi
        if printf 'overwrite' > "$2/private"; then exit 52; fi
        printf 'isolated'"#;
    let first_args = [OsString::from("first"), second.work.as_os_str().to_owned()];
    let second_args = [OsString::from("second"), first.work.as_os_str().to_owned()];
    let (a, b) = tokio::join!(
        first.shell(first.spec(), script, &first_args),
        second.shell(second.spec(), script, &second_args)
    );
    assert_completed(&a, b"isolated");
    assert_completed(&b, b"isolated");
    assert_eq!(
        std::fs::read_to_string(first.work.join("private")).unwrap(),
        "first"
    );
    assert_eq!(
        std::fs::read_to_string(second.work.join("private")).unwrap(),
        "second"
    );
}

#[tokio::test]
async fn refusal_is_operation_local_and_a_later_permitted_launch_works() {
    if !verified("refusal_is_operation_local_and_a_later_permitted_launch_works") {
        return;
    }
    let fixture = Fixture::new();
    let mut unsupported = fixture.spec();
    unsupported.require_descendant_termination = true;
    fixture.assert_refused(unsupported, ConfinementRefusal::UnsupportedRequirement);
    let output = fixture
        .shell(fixture.spec(), "printf 'still usable'", &[])
        .await;
    assert_completed(&output, b"still usable");
}

#[tokio::test]
async fn target_cannot_signal_trace_or_read_the_host_or_a_sibling_sandbox() {
    if !verified("target_cannot_signal_trace_or_read_the_host_or_a_sibling_sandbox") {
        return;
    }
    let fixture = Fixture::new();
    fixture
        .assert_probe(
            fixture.spec(),
            "host-process-denied",
            &std::process::id().to_string(),
        )
        .await;
    // A long-lived sibling under its own domain: a second target must not
    // reach it either, although both run as the same user.
    let sibling = fixture
        .prepare_shell(fixture.spec(), "printf ready; exec sleep 30", &[])
        .unwrap_or_else(prepare_failure)
        .spawn()
        .unwrap();
    let sibling_pid = sibling.id().unwrap();
    fixture
        .assert_probe(
            fixture.spec(),
            "host-process-denied",
            &sibling_pid.to_string(),
        )
        .await;
    drop(sibling);
}

// 9. Custody: the exec chain keeps the launched PID; exit status, signals and
//    exec failure are the target's own, never a fake completion.

#[tokio::test]
async fn exec_chain_keeps_the_pid_and_reports_the_targets_own_outcome() {
    if !verified("exec_chain_keeps_the_pid_and_reports_the_targets_own_outcome") {
        return;
    }
    use std::os::unix::process::ExitStatusExt;
    let fixture = Fixture::new();
    let child = fixture.probe(fixture.spec(), "pid", "").spawn().unwrap();
    let launched = child.id().unwrap();
    let output = child.wait_with_output().await.unwrap();
    assert_probe(output.clone());
    let reported = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix("MEERKAT_PROBE_PID=")
                .and_then(|v| v.parse::<u32>().ok())
        })
        .expect("probe pid");
    assert_eq!(launched, reported, "exec chain must keep custody's PID");

    let status = fixture.shell(fixture.spec(), "exit 37", &[]).await.status;
    assert_eq!(status.code(), Some(37));

    let mut sleeper = fixture
        .prepare_shell(fixture.spec(), "exec sleep 30", &[])
        .unwrap_or_else(prepare_failure)
        .spawn()
        .unwrap();
    let pid = sleeper.id().unwrap() as libc::pid_t;
    // SAFETY: signals the child this test owns.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
    assert_eq!(sleeper.wait().await.unwrap().signal(), Some(libc::SIGKILL));

    let missing = prepare(
        &ExecutionConfinement::try_from(fixture.spec()).unwrap(),
        fixture.launch(fixture.work.join("does-not-exist"), vec![], &[]),
    )
    .unwrap_or_else(prepare_failure);
    match missing.output().await {
        Err(error) => assert_eq!(error.raw_os_error(), Some(libc::ENOENT), "{error}"),
        Ok(output) => panic!("exec failure must not look like a launch: {output:?}"),
    }
}

// 10. Facilities: absent Landlock is BackendUnavailable (never an unconfined
//     launch); user namespaces are not required by this profile.

#[tokio::test]
async fn absent_landlock_is_backend_unavailable_and_the_present_kernel_still_works() {
    let fixture = Fixture::new();
    let output = tokio::process::Command::new(test_executable())
        .args(["--ignored", "--exact", "linux_probe_process", "--nocapture"])
        .env("MEERKAT_LINUX_PROBE", "landlock-unavailable")
        .env("MEERKAT_PROBE_ARGUMENT", &fixture.work)
        .output()
        .await
        .unwrap();
    assert_probe(output);
    if !verified("absent_landlock_is_backend_unavailable_and_the_present_kernel_still_works") {
        return;
    }
    let output = fixture
        .shell(fixture.spec(), "printf 'supported'", &[])
        .await;
    assert_completed(&output, b"supported");
}

#[tokio::test]
async fn profile_works_where_unprivileged_user_namespaces_are_unusable() {
    if !verified("profile_works_where_unprivileged_user_namespaces_are_unusable") {
        return;
    }
    // On this VM AppArmor's unprivileged_userns profile strips capabilities in a
    // new user namespace (uid_map write is refused), so bwrap cannot start.
    // The Landlock/seccomp profile must not depend on namespaces at all.
    let fixture = Fixture::new();
    let output = fixture
        .shell(
            fixture.spec(),
            "printf 'x' > inside && printf 'no-userns-needed'",
            &[],
        )
        .await;
    assert_completed(&output, b"no-userns-needed");
}

// Compatibility controls: common toolchains under the declared baseline. The
// Linux CommandRuntimeV1 baseline must carry what they read at startup
// (/etc/ssl for node's OpenSSL configuration, the literal /etc/gitconfig that
// git treats as fatal when unreadable). POSIX semaphores live in the
// host-global /dev/shm, which no baseline grants: it would be a channel
// between sandboxes. Python multiprocessing therefore needs the host's explicit
// /dev/shm grant, and without it the semaphore is denied.

/// A host tool for a compatibility control. A host without it (a minimal
/// CI executor) reports UNVERIFIED for that control instead of a pass; it
/// is a workload probe, not an enforcement claim.
fn host_tool(name: &str, test: &str) -> Option<PathBuf> {
    let candidate = std::env::var_os(format!("MEERKAT_TEST_{}", name.to_uppercase()))
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join(name))
                    .find(|candidate| candidate.is_file())
            })
        });
    let Some(candidate) = candidate else {
        println!("UNVERIFIED: compatibility control needs `{name}` ({test})");
        return None;
    };
    Some(std::fs::canonicalize(candidate).unwrap())
}

/// A system tool under the /usr baseline (not a user installation), or an
/// UNVERIFIED line on a host without it.
fn system_tool(name: &str) -> Option<PathBuf> {
    let path = Path::new("/usr/bin").join(name);
    match std::fs::canonicalize(&path) {
        Ok(path) if path.starts_with("/usr") => Some(path),
        _ => {
            println!("UNVERIFIED: compatibility control needs /usr/bin/{name}");
            None
        }
    }
}

const MULTIPROCESSING: &str = r#"
import sys
import multiprocessing as mp
def square(x):
    return x * x
if __name__ == "__main__":
    method = sys.argv[1]
    with mp.get_context(method).Pool(2) as pool:
        assert pool.map(square, [1, 2, 3]) == [1, 4, 9], method
    print("multiprocessing ok", end="")
"#;

#[tokio::test]
async fn python_multiprocessing_runs_with_an_explicit_shared_memory_grant_only() {
    if !verified("python_multiprocessing_runs_with_an_explicit_shared_memory_grant_only") {
        return;
    }
    let fixture = Fixture::new();
    std::fs::write(fixture.work.join("mp.py"), MULTIPROCESSING).unwrap();
    // The system interpreter under the /usr baseline, not a user virtualenv.
    let Some(python) = system_tool("python3") else {
        return;
    };
    let run = |spec: ConfinementSpec, method: &str| {
        prepare(
            &ExecutionConfinement::try_from(spec).unwrap(),
            fixture.launch(
                python.clone(),
                vec![OsString::from("mp.py"), OsString::from(method)],
                &[("PATH", "/usr/bin:/bin")],
            ),
        )
        .unwrap_or_else(prepare_failure)
    };
    let denied = run(fixture.spec(), "fork").output().await.unwrap();
    assert!(!denied.status.success(), "{denied:?}");
    assert!(
        String::from_utf8_lossy(&denied.stderr).contains("PermissionError"),
        "the shared-memory semaphore must be a policy denial: {denied:?}"
    );
    let mut granted = fixture.spec();
    for access in [&mut granted.read, &mut granted.write] {
        if let FilesystemAccess::Paths(paths) = access {
            paths.push(PathAccess::Subtree(PathBuf::from("/dev/shm")));
        }
    }
    for method in ["fork", "spawn"] {
        let output = run(granted.clone(), method).output().await.unwrap();
        assert_completed(&output, b"multiprocessing ok");
    }
    // Known profile limit: the forkserver is an in-sandbox Unix server, and
    // with no unix_connect grant AF_UNIX socket creation is denied (as on
    // macOS). It fails as a policy denial, never by reaching a host socket.
    let forkserver = run(granted, "forkserver").output().await.unwrap();
    assert!(!forkserver.status.success(), "{forkserver:?}");
    let stderr = String::from_utf8_lossy(&forkserver.stderr);
    assert!(
        stderr.contains("socket.socket(socket.AF_UNIX)") && stderr.contains("PermissionError"),
        "forkserver must fail at AF_UNIX socket creation: {forkserver:?}"
    );
}

#[tokio::test]
async fn node_child_processes_run_under_the_baseline() {
    if !verified("node_child_processes_run_under_the_baseline") {
        return;
    }
    let fixture = Fixture::new();
    let Some(node) = host_tool("node", "node_child_processes_run_under_the_baseline") else {
        return;
    };
    let mut spec = fixture.spec();
    if let FilesystemAccess::Paths(paths) = &mut spec.read {
        paths.push(PathAccess::Literal(node.clone()));
    }
    let script = r#"
        const { execFileSync, spawnSync } = require("child_process");
        const shell = execFileSync("/bin/sh", ["-c", "printf ok"]).toString();
        const echo = spawnSync("/bin/echo", ["hi"], { encoding: "utf8" });
        if (shell !== "ok" || echo.status !== 0 || echo.stdout !== "hi\n") process.exit(3);
        process.stdout.write("node ok");
    "#;
    let output = prepare(
        &ExecutionConfinement::try_from(spec).unwrap(),
        fixture.launch(
            node,
            vec![OsString::from("-e"), OsString::from(script)],
            &[("PATH", "/usr/bin:/bin")],
        ),
    )
    .unwrap_or_else(prepare_failure)
    .output()
    .await
    .unwrap();
    assert_completed(&output, b"node ok");
}

#[tokio::test]
async fn git_init_add_and_commit_run_under_the_baseline() {
    if !verified("git_init_add_and_commit_run_under_the_baseline") {
        return;
    }
    if system_tool("git").is_none() {
        return;
    }
    let fixture = Fixture::new();
    let output = fixture
        .shell(
            fixture.spec(),
            r#"
        git init -q repo || exit 90
        cd repo || exit 91
        printf 'tracked' > file || exit 92
        git -c user.name=probe -c user.email=probe@example.invalid add file || exit 93
        git -c user.name=probe -c user.email=probe@example.invalid commit -qm probe || exit 94
        test "$(git log --oneline | wc -l)" = 1 || exit 95
        printf 'git ok'
    "#,
            &[],
        )
        .await;
    assert_completed(&output, b"git ok");
}

// 9b. The custody gate: nothing runs until the host's exact release token
//     arrives on descriptor 3; EOF or a wrong token exits 125 unrun. The
//     restrictions are installed before the gate shell starts.

#[tokio::test]
async fn custody_gate_waits_for_release_keeps_the_pid_and_never_runs_on_eof() {
    if !verified("custody_gate_waits_for_release_keeps_the_pid_and_never_runs_on_eof") {
        return;
    }
    use std::os::fd::AsFd;
    let fixture = Fixture::new();
    let marker = fixture.work.join("ran");
    let script = r#"printf ran > ran || exit 60; printf '%s' "$$""#;
    for release in [Some("token-1"), Some("wrong"), None] {
        let (reader, writer) = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).unwrap();
        let prepared = fixture
            .prepare_shell(fixture.spec(), script, &[])
            .unwrap_or_else(prepare_failure);
        let child = prepared
            .spawn_behind_gate(
                reader.as_fd(),
                std::ffi::OsStr::new("token-1"),
                SpawnIo::default(),
            )
            .unwrap();
        drop(reader);
        let launched = child.id().unwrap();
        // The gate holds the target until the host writes or closes.
        assert!(!marker.exists(), "the target ran before release");
        let mut writer = std::fs::File::from(writer);
        if let Some(token) = release {
            writer.write_all(format!("{token}\n").as_bytes()).unwrap();
        }
        drop(writer);
        let output = child.wait_with_output().await.unwrap();
        if release == Some("token-1") {
            assert_completed(&output, launched.to_string().as_bytes());
            assert!(marker.exists());
            std::fs::remove_file(&marker).unwrap();
        } else {
            assert_eq!(output.status.code(), Some(125), "{output:?}");
            assert!(output.stdout.is_empty(), "{output:?}");
            assert!(!marker.exists(), "an unreleased gate ran the target");
        }
    }
}

// 8. One immutable compilation binds many launches; the report names the
//    backend and the exact requirement.

#[tokio::test]
async fn one_compilation_binds_distinct_launches_with_an_exact_capability_report() {
    if !verified("one_compilation_binds_distinct_launches_with_an_exact_capability_report") {
        return;
    }
    let fixture = Fixture::new();
    let requirement = ExecutionConfinement::try_from(fixture.spec()).unwrap();
    let compiled = CompiledConfinement::compile(&requirement).unwrap();
    assert_eq!(
        compiled.capabilities().backend(),
        ConfinementBackend::LinuxLandlockSeccompV1
    );
    assert_eq!(compiled.capabilities().requirement(), &requirement);
    let launch = |value: &str| {
        fixture.launch(
            PathBuf::from("/bin/sh"),
            vec![
                OsString::from("-c"),
                OsString::from(r#"printf '%s' "$1" > "$1" && cat "$1""#),
                OsString::from("probe"),
                OsString::from(value),
            ],
            &[],
        )
    };
    let bind = |value: &str| compiled.bind_launch(launch(value)).unwrap().output();
    let (a, b, c) = tokio::join!(bind("alpha"), bind("beta"), bind("gamma"));
    assert_completed(&a.unwrap(), b"alpha");
    assert_completed(&b.unwrap(), b"beta");
    assert_completed(&c.unwrap(), b"gamma");
}

// 8c. A launch binds the objects the policy paths name at bind time, without
//     following symlinks: a retargeted grant root refuses that launch, and a
//     replaced one grants the new directory only.

#[tokio::test]
async fn replaced_or_retargeted_grant_paths_refuse_the_launch_instead_of_widening() {
    if !verified("replaced_or_retargeted_grant_paths_refuse_the_launch_instead_of_widening") {
        return;
    }
    let fixture = Fixture::new();
    let granted = fixture.root.join("granted");
    std::fs::create_dir(&granted).unwrap();
    std::fs::write(granted.join("original"), "original").unwrap();
    let mut spec = fixture.spec();
    for access in [&mut spec.read, &mut spec.write] {
        if let FilesystemAccess::Paths(paths) = access {
            paths.push(PathAccess::Subtree(granted.clone()));
        }
    }
    let compiled =
        CompiledConfinement::compile(&ExecutionConfinement::try_from(spec).unwrap()).unwrap();
    let script = |args: &[&std::ffi::OsStr]| {
        let mut arguments = vec![
            OsString::from("-c"),
            OsString::from(
                r#"test ! -e "$1/original" || exit 70
                if cat "$2/original"; then exit 71; fi
                printf 'fresh' > "$1/fresh" || exit 72
                printf 'rebound'"#,
            ),
            OsString::from("probe"),
        ];
        arguments.extend(args.iter().map(|arg| arg.to_os_string()));
        fixture.launch(PathBuf::from("/bin/sh"), arguments, &[])
    };
    let moved = fixture.root.join("moved");
    std::fs::rename(&granted, &moved).unwrap();
    std::os::unix::fs::symlink(&fixture.root, &granted).unwrap();
    assert_eq!(
        compiled
            .bind_launch(script(&[granted.as_os_str(), moved.as_os_str()]))
            .err(),
        Some(ConfinementRefusal::PreparationFailed),
        "a symlinked grant root must refuse the launch, never grant its target"
    );
    std::fs::remove_file(&granted).unwrap();
    std::fs::create_dir(&granted).unwrap();
    let output = compiled
        .bind_launch(script(&[granted.as_os_str(), moved.as_os_str()]))
        .unwrap()
        .output()
        .await
        .unwrap();
    assert_completed(&output, b"rebound");
    assert_eq!(
        std::fs::read_to_string(granted.join("fresh")).unwrap(),
        "fresh"
    );
}

// Write Unrestricted grants files only: network and other processes stay
// out of reach.

#[tokio::test]
async fn write_unrestricted_runs_without_reaching_other_processes_or_the_network() {
    if !verified("write_unrestricted_runs_without_reaching_other_processes_or_the_network") {
        return;
    }
    let fixture = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut spec = fixture.spec();
    spec.write = FilesystemAccess::Unrestricted;
    fixture
        .assert_probe(
            spec.clone(),
            "tcp-denied",
            &listener.local_addr().unwrap().to_string(),
        )
        .await;
    fixture
        .assert_probe(
            spec.clone(),
            "host-process-denied",
            &std::process::id().to_string(),
        )
        .await;
    let output = fixture
        .shell(spec, "printf 'x' > inside && printf 'written'", &[])
        .await;
    assert_completed(&output, b"written");
}
