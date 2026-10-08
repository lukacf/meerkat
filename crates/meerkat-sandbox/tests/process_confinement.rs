//! Actual OS probes. Backend/setup failure is a test failure, never a skip.
#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use meerkat_core::confinement::{
    ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess, PathAccess,
    PlatformBaseline,
};
use meerkat_sandbox::{ProcessLaunchSpec, prepare};

struct Fixture {
    _root: tempfile::TempDir,
    work: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let outside = root.path().join("outside");
        std::fs::write(&outside, "private canary\n").unwrap();
        Self {
            _root: root,
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
            unix_connect: vec![],
            network: IpNetworkAccess::Denied,
            require_descendant_termination: false,
        }
    }

    async fn shell(
        &self,
        spec: ConfinementSpec,
        script: &str,
        args: &[OsString],
    ) -> std::process::Output {
        let mut arguments = vec![
            OsString::from("-c"),
            OsString::from(script),
            OsString::from("probe"),
        ];
        arguments.extend_from_slice(args);
        let launch = ProcessLaunchSpec::new(
            PathBuf::from("/bin/sh"),
            arguments,
            self.work.clone(),
            BTreeMap::from([(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))]),
        )
        .unwrap();
        prepare(&ExecutionConfinement::try_from(spec).unwrap(), launch)
            .unwrap()
            .output()
            .await
            .unwrap()
    }
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn allowed_command_runs_but_outside_read_write_and_symlink_escape_fail() {
    let fixture = Fixture::new();
    let output = fixture
        .shell(
            fixture.spec(),
            r#"
        printf 'allowed' > inside || exit 10
        test "$(cat inside)" = allowed || exit 11
        if cat "$1"; then exit 12; fi
        if printf 'changed' > "$1"; then exit 13; fi
        ln -s "$1" alias || exit 14
        if cat alias; then exit 15; fi
        printf 'completed'
    "#,
            &[fixture.outside.as_os_str().to_owned()],
        )
        .await;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"completed");
    assert_eq!(
        std::fs::read_to_string(&fixture.outside).unwrap(),
        "private canary\n"
    );
}

#[tokio::test]
#[cfg(target_os = "macos")]
async fn protected_descendant_cannot_be_moved_outside_its_exclusion() {
    let fixture = Fixture::new();
    let parent = fixture.work.join("parent");
    std::fs::create_dir(&parent).unwrap();
    let protected = parent.join("secret");
    std::fs::write(&protected, "do not disclose").unwrap();
    let mut spec = fixture.spec();
    spec.deny_read.push(PathAccess::Literal(protected.clone()));
    let output = fixture
        .shell(
            spec,
            r"
        if mv parent renamed; then exit 20; fi
        if cat parent/secret; then exit 21; fi
        printf 'preserved'
    ",
            &[],
        )
        .await;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"preserved");
    assert_eq!(
        std::fs::read_to_string(protected).unwrap(),
        "do not disclose"
    );
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn child_has_only_explicit_environment() {
    let fixture = Fixture::new();
    let output = fixture
        .shell(
            fixture.spec(),
            r#"
        test -z "${HOME+x}" || exit 30
        test -z "${USER+x}" || exit 31
        test "$PATH" = /usr/bin:/bin || exit 32
        printf 'exact'
    "#,
            &[],
        )
        .await;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"exact");
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn inherited_descriptor_cannot_bypass_filesystem_denial() {
    use nix::libc;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let fixture = Fixture::new();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&fixture.outside)
        .unwrap();
    // A fresh deliberately inheritable descriptor, without changing the flags
    // of any descriptor owned by other test threads.
    let descriptor = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 200) };
    assert!(
        descriptor >= 200,
        "fixture could not allocate descriptor: {}",
        std::io::Error::last_os_error()
    );
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, 0);
    let output = fixture
        .shell(
            fixture.spec(),
            r#"
        if /bin/bash -c 'printf leaked >&"$1"' probe "$1"; then exit 40; fi
        printf 'sealed'
    "#,
            &[OsString::from(descriptor.as_raw_fd().to_string())],
        )
        .await;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"sealed");
    assert_eq!(
        std::fs::read_to_string(&fixture.outside).unwrap(),
        "private canary\n"
    );
    assert_eq!(
        unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) },
        flags
    );
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn denial_does_not_prevent_a_later_permitted_launch() {
    let fixture = Fixture::new();
    let mut unsupported = fixture.spec();
    unsupported.require_descendant_termination = true;
    let launch = ProcessLaunchSpec::new(
        PathBuf::from("/bin/sh"),
        vec![],
        fixture.work.clone(),
        BTreeMap::new(),
    )
    .unwrap();
    assert!(matches!(
        prepare(
            &ExecutionConfinement::try_from(unsupported).unwrap(),
            launch,
        ),
        Err(meerkat_sandbox::ConfinementRefusal::UnsupportedRequirement)
    ));
    let allowed = fixture
        .shell(fixture.spec(), "printf 'still usable'", &[])
        .await;
    assert!(allowed.status.success(), "{allowed:?}");
    assert_eq!(allowed.stdout, b"still usable");
}

// A native probe avoids treating a missing command or unrelated connection error
// as evidence that the sandbox denied an operation. It is run only as a child.
#[test]
#[ignore = "subprocess entrypoint for confinement probes"]
fn confinement_probe_process() {
    use nix::libc;
    use std::io::Write;
    use std::time::Duration;
    fn denied<T>(result: std::io::Result<T>) {
        match result {
            Err(error) if matches!(error.raw_os_error(), Some(libc::EPERM | libc::EACCES)) => {}
            Err(error) => panic!("unexpected failure instead of policy denial: {error}"),
            Ok(_) => panic!("forbidden operation succeeded"),
        }
    }
    let mode = std::env::var("MEERKAT_CONFINEMENT_PROBE").expect("probe mode");
    let argument = std::env::var("MEERKAT_PROBE_ARGUMENT").unwrap_or_default();
    match mode.as_str() {
        "tcp-denied" => {
            let endpoint = argument.parse().unwrap();
            denied(std::net::TcpStream::connect_timeout(
                &endpoint,
                Duration::from_secs(2),
            ));
        }
        "tcp-selective" => {
            let (allowed, forbidden) = argument.split_once(',').unwrap();
            std::net::TcpStream::connect_timeout(&allowed.parse().unwrap(), Duration::from_secs(2))
                .expect("explicit endpoint must work");
            denied(std::net::TcpStream::connect_timeout(
                &forbidden.parse().unwrap(),
                Duration::from_secs(2),
            ));
            denied(std::net::TcpListener::bind("127.0.0.1:0"));
        }
        "tcp-allowed" => {
            std::net::TcpStream::connect_timeout(
                &argument.parse().unwrap(),
                Duration::from_secs(2),
            )
            .expect("explicit unrestricted IP connection must work");
        }
        "udp-denied" => {
            // No bind or outbound packet may escape a deny-IP contract.
            match std::net::UdpSocket::bind("127.0.0.1:0") {
                Ok(socket) => denied(socket.send_to(b"forbidden", &argument)),
                Err(error) => denied::<()>(Err(error)),
            }
        }
        "unix-denied" => denied(std::os::unix::net::UnixStream::connect(&argument)),
        #[cfg(target_os = "linux")]
        "unix-pair-denied" => {
            use std::os::fd::{FromRawFd, OwnedFd};
            let mut descriptors = [-1; 2];
            let result = unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
                    0,
                    descriptors.as_mut_ptr(),
                )
            };
            if result == -1 {
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::EPERM)
                );
            } else {
                let sender =
                    unsafe { std::os::unix::net::UnixDatagram::from_raw_fd(descriptors[0]) };
                let _peer = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
                // A Unix pair is not a permanent peer binding for datagrams.
                // This must not acquire a host Unix destination via sendto.
                denied(sender.send_to(b"forbidden-pair", &argument));
            }
        }
        "unix-selective" => {
            let (allowed, forbidden) = argument.split_once(',').unwrap();
            std::os::unix::net::UnixStream::connect(allowed).expect("explicit socket must work");
            denied(std::os::unix::net::UnixStream::connect(forbidden));
        }
        "fd-closed" => {
            let descriptor: i32 = argument.parse().unwrap();
            // SAFETY: Read-only descriptor query; it owns no borrowed object.
            assert_eq!(unsafe { libc::fcntl(descriptor, libc::F_GETFD) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EBADF)
            );
        }
        "connected-fd-closed" => {
            let descriptor: i32 = argument.parse().unwrap();
            let byte = b'x';
            // The parent deliberately left this connected socket inheritable.
            // It must be closed, not merely hidden from pathname operations.
            // write remains allowed on byte streams; send may be denied by
            // the IP filter before the kernel checks descriptor validity.
            assert_eq!(
                unsafe { libc::write(descriptor, std::ptr::addr_of!(byte).cast(), 1,) },
                -1
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EBADF)
            );
        }
        #[cfg(target_os = "linux")]
        "filesystem-metadata" => {
            use std::os::unix::fs::PermissionsExt;
            fn hidden<T>(result: std::io::Result<T>) {
                match result {
                    Err(error)
                        if matches!(
                            error.raw_os_error(),
                            Some(libc::ENOENT | libc::EACCES | libc::EPERM)
                        ) => {}
                    Err(error) => panic!("unexpected metadata failure: {error}"),
                    Ok(_) => panic!("outside metadata was accessible"),
                }
            }
            assert!(std::fs::metadata("inside").unwrap().is_file());
            hidden(std::fs::metadata(&argument));
            hidden(std::fs::symlink_metadata(&argument));
            std::os::unix::fs::symlink(&argument, "metadata-alias").unwrap();
            hidden(std::fs::metadata("metadata-alias"));
            hidden(std::fs::set_permissions(
                &argument,
                std::fs::Permissions::from_mode(0o777),
            ));
        }
        #[cfg(target_os = "linux")]
        "namespace-escape-denied" => {
            assert_eq!(unsafe { libc::unshare(libc::CLONE_NEWNS) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EPERM)
            );
            assert_eq!(unsafe { libc::chroot(c"/".as_ptr()) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EPERM)
            );
            // Neither the host's root nor ambient process descriptors are
            // visible through a procfs escape after the supported mount view.
            assert!(std::fs::metadata(format!("/proc/self/root{argument}")).is_err());
        }
        #[cfg(target_os = "linux")]
        "host-process-mutation-denied" => {
            let host: libc::pid_t = argument.parse().unwrap();
            assert!(host > 0 && host != unsafe { libc::getpid() });
            let refused = |result: libc::c_long| {
                assert_eq!(result, -1);
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::EPERM)
                );
            };
            let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
            info.si_code = libc::SI_QUEUE;
            // Signal zero checks the real host target without delivering a
            // signal if an older backend incorrectly permits these calls.
            refused(unsafe {
                libc::syscall(libc::SYS_rt_sigqueueinfo, host, 0, std::ptr::addr_of!(info))
            });
            refused(unsafe {
                libc::syscall(
                    libc::SYS_rt_tgsigqueueinfo,
                    host,
                    host,
                    0,
                    std::ptr::addr_of!(info),
                )
            });
            let mut limits = unsafe { std::mem::zeroed::<libc::rlimit64>() };
            assert_eq!(
                unsafe {
                    libc::prlimit64(0, libc::RLIMIT_NOFILE, std::ptr::null(), &raw mut limits)
                },
                0
            );
            assert_eq!(
                unsafe {
                    libc::prlimit64(
                        0,
                        libc::RLIMIT_NOFILE,
                        &raw const limits,
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            // Null new_limit is a harmless query, but still must not cross the
            // process boundary. Resource operations on pid zero stay usable.
            refused(unsafe {
                libc::syscall(
                    libc::SYS_prlimit64,
                    host,
                    libc::RLIMIT_NOFILE,
                    std::ptr::null::<libc::rlimit64>(),
                    std::ptr::addr_of_mut!(limits),
                )
            });
            let mut parameter = unsafe { std::mem::zeroed::<libc::sched_param>() };
            assert_eq!(unsafe { libc::sched_getparam(host, &raw mut parameter) }, 0);
            refused(unsafe {
                libc::syscall(
                    libc::SYS_sched_setparam,
                    host,
                    std::ptr::addr_of!(parameter),
                )
            });
            let priority = unsafe { libc::getpriority(libc::PRIO_PROCESS, host as _) };
            refused(unsafe {
                libc::syscall(libc::SYS_setpriority, libc::PRIO_PROCESS, host, priority)
            });
            let (reader, _writer) = std::io::pipe().unwrap();
            use std::os::fd::AsRawFd;
            let descriptor = reader.as_raw_fd();
            // Harmless owner assignment is refused before O_ASYNC is ever
            // enabled. These setters must not become another host signal API.
            refused(unsafe { libc::fcntl(descriptor, libc::F_SETOWN, host) } as _);
            #[repr(C)]
            struct Owner {
                kind: i32,
                pid: libc::pid_t,
            }
            let owner = Owner { kind: 1, pid: host }; // F_OWNER_PID
            // Linux UAPI commands absent from libc's supported target modules.
            const F_SETOWN_EX: libc::c_int = 15;
            const F_SETSIG: libc::c_int = 10;
            refused(
                unsafe { libc::fcntl(descriptor, F_SETOWN_EX, std::ptr::addr_of!(owner)) } as _,
            );
            refused(unsafe { libc::fcntl(descriptor, F_SETSIG, libc::SIGUSR1) } as _);
            let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
            assert!(flags >= 0);
            refused(unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_ASYNC) } as _);
            assert_eq!(
                unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) },
                0
            );
            let duplicate = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 3) };
            assert!(duplicate >= 3);
            assert_eq!(unsafe { libc::close(duplicate) }, 0);
        }
        "pid" => {
            println!("\nMEERKAT_PROBE_PID={}", std::process::id());
            std::io::stdout().flush().unwrap();
            std::process::exit(0);
        }
        _ => panic!("unknown native probe"),
    }
    print!("native probe passed");
    std::io::stdout().flush().unwrap();
    std::process::exit(0);
}

impl Fixture {
    fn probe(
        &self,
        mut spec: ConfinementSpec,
        mode: &str,
        argument: &str,
    ) -> meerkat_sandbox::PreparedConfinement {
        let executable = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        if let FilesystemAccess::Paths(paths) = &mut spec.read {
            // Grant this exact test executable, not its build directory.
            paths.push(PathAccess::Literal(executable.clone()));
        }
        let environment = BTreeMap::from([
            (
                OsString::from("MEERKAT_CONFINEMENT_PROBE"),
                OsString::from(mode),
            ),
            (
                OsString::from("MEERKAT_PROBE_ARGUMENT"),
                OsString::from(argument),
            ),
            // Proxy variables must never broaden an OS endpoint requirement.
            (
                OsString::from("HTTP_PROXY"),
                OsString::from("http://127.0.0.1:9"),
            ),
            (OsString::from("NO_PROXY"), OsString::from("*")),
        ]);
        let launch = ProcessLaunchSpec::new(
            executable,
            [
                "--ignored",
                "--exact",
                "confinement_probe_process",
                "--nocapture",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
            self.work.clone(),
            environment,
        )
        .unwrap();
        prepare(&ExecutionConfinement::try_from(spec).unwrap(), launch).unwrap()
    }
}

fn assert_probe(output: std::process::Output) {
    assert!(output.status.success(), "native probe failed: {output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("native probe passed"),
        "probe did not reach its assertions: {output:?}"
    );
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn deny_ip_blocks_tcp_udp_and_proxy_environment_cannot_bypass_it() {
    let fixture = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    assert_probe(
        fixture
            .probe(
                fixture.spec(),
                "tcp-denied",
                &listener.local_addr().unwrap().to_string(),
            )
            .output()
            .await
            .unwrap(),
    );
    let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    assert_probe(
        fixture
            .probe(
                fixture.spec(),
                "udp-denied",
                &receiver.local_addr().unwrap().to_string(),
            )
            .output()
            .await
            .unwrap(),
    );
    let mut unrestricted = fixture.spec();
    unrestricted.network = IpNetworkAccess::Unrestricted;
    assert_probe(
        fixture
            .probe(
                unrestricted,
                "tcp-allowed",
                &listener.local_addr().unwrap().to_string(),
            )
            .output()
            .await
            .unwrap(),
    );
}

#[test]
fn exact_ip_endpoint_is_refused_instead_of_broadened_to_localhost() {
    let fixture = Fixture::new();
    for endpoint in ["127.0.0.1:443", "[::1]:443", "192.0.2.1:443"] {
        let mut spec = fixture.spec();
        spec.network = IpNetworkAccess::Connect(vec![endpoint.parse().unwrap()]);
        let requirement = ExecutionConfinement::try_from(spec).unwrap();
        assert!(matches!(
            meerkat_sandbox::CompiledConfinement::compile(&requirement,),
            Err(meerkat_sandbox::ConfinementRefusal::UnsupportedRequirement)
        ));
    }
}

#[tokio::test]
#[cfg(target_os = "macos")]
async fn ip_and_unix_socket_permissions_are_separate_and_literal_socket_is_exact() {
    let fixture = Fixture::new();
    let allowed_path = fixture.work.join("allowed.sock");
    let forbidden_path = fixture.work.join("forbidden.sock");
    let _allowed = std::os::unix::net::UnixListener::bind(&allowed_path).unwrap();
    let _forbidden = std::os::unix::net::UnixListener::bind(&forbidden_path).unwrap();
    let mut ip_only = fixture.spec();
    ip_only.network = IpNetworkAccess::Unrestricted;
    assert_probe(
        fixture
            .probe(ip_only, "unix-denied", allowed_path.to_str().unwrap())
            .output()
            .await
            .unwrap(),
    );
    let mut socket_only = fixture.spec();
    socket_only
        .unix_connect
        .push(PathAccess::Literal(allowed_path.clone()));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    assert_probe(
        fixture
            .probe(
                socket_only.clone(),
                "tcp-denied",
                &listener.local_addr().unwrap().to_string(),
            )
            .output()
            .await
            .unwrap(),
    );
    let argument = format!("{},{}", allowed_path.display(), forbidden_path.display());
    assert_probe(
        fixture
            .probe(socket_only, "unix-selective", &argument)
            .output()
            .await
            .unwrap(),
    );
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn sparse_high_descriptor_is_closed_without_changing_parent_limits() {
    use nix::libc;
    let mut before = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: Reads this process's limits into valid storage.
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, before.as_mut_ptr()) },
        0
    );
    let before = unsafe { before.assume_init() };
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "high_descriptor_host_probe",
            "--nocapture",
        ])
        .env_clear()
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("high descriptor host probe passed"));
    let mut after = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, after.as_mut_ptr()) },
        0
    );
    let after = unsafe { after.assume_init() };
    assert_eq!(before.rlim_cur, after.rlim_cur);
    assert_eq!(before.rlim_max, after.rlim_max);
}

#[test]
#[ignore = "isolated host subprocess for sparse descriptor setup"]
fn high_descriptor_host_probe() {
    use nix::libc;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let fixture = Fixture::new();
    let file = std::fs::File::open(&fixture.outside).unwrap();
    const HIGH: i32 = 4096;
    // This process is a disposable host launcher. Raising its limit and creating
    // an inheritable descriptor leaves the actual parent's limits/table intact.
    let descriptor = unsafe {
        let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()), 0);
        let mut limit = limit.assume_init();
        assert!(limit.rlim_max > HIGH as libc::rlim_t);
        limit.rlim_cur = limit.rlim_cur.max((HIGH + 1) as libc::rlim_t);
        assert_eq!(
            libc::setrlimit(libc::RLIMIT_NOFILE, std::ptr::addr_of!(limit)),
            0
        );
        assert_eq!(libc::dup2(file.as_raw_fd(), HIGH), HIGH);
        OwnedFd::from_raw_fd(HIGH)
    };
    assert_eq!(
        unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            assert_probe(
                fixture
                    .probe(fixture.spec(), "fd-closed", &HIGH.to_string())
                    .output()
                    .await
                    .unwrap(),
            );
        });
    assert!(unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) } >= 0);
    println!("high descriptor host probe passed");
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn independent_agent_roots_remain_separate_in_simultaneous_launches() {
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
    for output in [a, b] {
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"isolated");
    }
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
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn baseline_supports_declared_shell_commands_and_spawn_preserves_pid() {
    let fixture = Fixture::new();
    let output = fixture
        .shell(
            fixture.spec(),
            r#"
        mkdir nested || exit 60
        printf 'beta\nalpha\n' > nested/input || exit 61
        sort nested/input | sed -n '1p' > nested/output || exit 62
        test "$(cat nested/output)" = alpha || exit 63
        /bin/sh -c 'printf child' > nested/child || exit 64
        test "$(cat nested/child)" = child || exit 65
        mv nested/output nested/renamed || exit 66
        rm nested/input nested/renamed nested/child || exit 67
        rmdir nested || exit 68
        printf 'compatible'
    "#,
            &[],
        )
        .await;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"compatible");
    let child = fixture.probe(fixture.spec(), "pid", "").spawn().unwrap();
    let launched_pid = child.id().unwrap();
    let output = child.wait_with_output().await.unwrap();
    assert!(output.status.success(), "{output:?}");
    let final_pid = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix("MEERKAT_PROBE_PID=")
                .and_then(|value| value.parse::<u32>().ok())
        })
        .expect("probe pid");
    assert_eq!(
        launched_pid, final_pid,
        "exec chain must retain custody's leader PID"
    );
}

#[tokio::test]
#[cfg(target_os = "macos")]
async fn protected_missing_subtree_root_cannot_be_created_or_renamed_into_place() {
    let fixture = Fixture::new();
    let protected = fixture.work.join("protected");
    let mut spec = fixture.spec();
    spec.deny_write.push(PathAccess::Subtree(protected.clone()));
    let output = fixture
        .shell(
            spec,
            r"
        if mkdir protected; then exit 70; fi
        mkdir source || exit 71
        if mv source protected; then exit 72; fi
        test ! -e protected || exit 73
        printf 'root protected'
    ",
            &[],
        )
        .await;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"root protected");
    assert!(!protected.exists());
}

#[tokio::test]
#[cfg(target_os = "macos")]
async fn protected_read_cannot_be_reached_by_a_new_hard_link() {
    let fixture = Fixture::new();
    let protected = fixture.work.join("private-data");
    std::fs::write(&protected, "secret").unwrap();
    let mut spec = fixture.spec();
    spec.deny_read.push(PathAccess::Literal(protected));
    let output = fixture
        .shell(
            spec,
            r"
        if ln private-data linked; then
            if cat linked; then exit 80; fi
        fi
        printf 'protected'
    ",
            &[],
        )
        .await;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"protected");
}

#[test]
fn unsafe_bootstrap_environment_and_mutable_policy_paths_are_refused() {
    let fixture = Fixture::new();
    for key in [
        "DYLD_INSERT_LIBRARIES",
        "LD_PRELOAD",
        "ENV",
        "BASH_ENV",
        "BASH_FUNC_probe%%",
        "SHELLOPTS",
        "GLIBC_TUNABLES",
    ] {
        let launch = ProcessLaunchSpec::new(
            PathBuf::from("/bin/sh"),
            vec![],
            fixture.work.clone(),
            BTreeMap::from([(OsString::from(key), OsString::from("injection"))]),
        );
        assert!(
            matches!(
                launch,
                Err(meerkat_sandbox::ConfinementRefusal::UnsupportedRequirement)
            ),
            "{key}"
        );
    }
    let alias = fixture.work.join("alias");
    std::os::unix::fs::symlink(fixture._root.path(), &alias).unwrap();
    let mut spec = fixture.spec();
    spec.write = FilesystemAccess::Paths(vec![PathAccess::Subtree(alias.join("missing"))]);
    let launch = ProcessLaunchSpec::new(
        PathBuf::from("/bin/sh"),
        vec![],
        fixture.work,
        BTreeMap::new(),
    )
    .unwrap();
    assert!(matches!(
        prepare(&ExecutionConfinement::try_from(spec).unwrap(), launch),
        Err(meerkat_sandbox::ConfinementRefusal::UnsupportedRequirement)
    ));
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn inherited_connected_socket_cannot_bypass_network_denial() {
    use nix::libc;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let fixture = Fixture::new();
    let (sender, mut receiver) = std::os::unix::net::UnixStream::pair().unwrap();
    receiver.set_nonblocking(true).unwrap();
    // Only this duplicate is made inheritable. Other tests' descriptors and
    // the original socket retain their existing flags.
    let descriptor = unsafe { libc::fcntl(sender.as_raw_fd(), libc::F_DUPFD, 200) };
    assert!(descriptor >= 200, "{}", std::io::Error::last_os_error());
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    assert_eq!(
        unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    assert_probe(
        fixture
            .probe(
                fixture.spec(),
                "connected-fd-closed",
                &descriptor.as_raw_fd().to_string(),
            )
            .output()
            .await
            .unwrap(),
    );
    assert!(unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) } >= 0);
    let mut byte = [0_u8; 1];
    assert_eq!(
        receiver.read(&mut byte).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn linux_denies_unix_connect_independently_of_ip_permission() {
    let fixture = Fixture::new();
    let socket = fixture.work.join("host-control.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    for network in [IpNetworkAccess::Denied, IpNetworkAccess::Unrestricted] {
        let mut spec = fixture.spec();
        spec.network = network;
        assert_probe(
            fixture
                .probe(spec, "unix-denied", socket.to_str().unwrap())
                .output()
                .await
                .unwrap(),
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_initial_backend_refuses_unsupported_dimensions_before_binding() {
    let fixture = Fixture::new();
    let mut specifications = Vec::new();
    let mut descendants = fixture.spec();
    descendants.require_descendant_termination = true;
    specifications.push(descendants);
    let mut unix = fixture.spec();
    unix.unix_connect
        .push(PathAccess::Literal(fixture.work.join("control.sock")));
    specifications.push(unix);
    let mut exclusion = fixture.spec();
    exclusion
        .deny_read
        .push(PathAccess::Literal(fixture.work.join("secret")));
    specifications.push(exclusion);
    let mut absent_exclusion = fixture.spec();
    absent_exclusion
        .deny_write
        .push(PathAccess::Subtree(fixture.work.join("future-secret")));
    specifications.push(absent_exclusion);
    for spec in specifications {
        assert!(matches!(
            meerkat_sandbox::CompiledConfinement::compile(&spec.try_into().unwrap()),
            Err(meerkat_sandbox::ConfinementRefusal::UnsupportedRequirement)
        ));
    }
}

/// The paired positive control for the refusals above: an ordinary profile
/// must actually compile, so a blanket unsupported implementation cannot
/// satisfy them. It needs an eligible host, so on Linux it runs in the
/// positive confinement acceptance lane.
#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
fn linux_initial_backend_compiles_a_supported_profile() {
    let fixture = Fixture::new();
    let supported = ExecutionConfinement::try_from(fixture.spec()).unwrap();
    let compiled = meerkat_sandbox::CompiledConfinement::compile(&supported).unwrap();
    assert_eq!(compiled.capabilities().requirement(), &supported);
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn linux_strict_paths_cover_metadata_and_namespace_escape() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    std::fs::write(fixture.work.join("inside"), "permitted").unwrap();
    let original_permissions = std::fs::metadata(&fixture.outside)
        .unwrap()
        .permissions()
        .mode();
    for mode in ["filesystem-metadata", "namespace-escape-denied"] {
        assert_probe(
            fixture
                .probe(fixture.spec(), mode, fixture.outside.to_str().unwrap())
                .output()
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        std::fs::metadata(&fixture.outside)
            .unwrap()
            .permissions()
            .mode(),
        original_permissions
    );
    assert_eq!(
        std::fs::read(&fixture.outside).unwrap(),
        b"private canary\n"
    );
    let sibling = fixture
        .shell(fixture.spec(), "printf 'still permitted'", &[])
        .await;
    assert!(sibling.status.success(), "{sibling:?}");
    assert_eq!(sibling.stdout, b"still permitted");
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[tokio::test]
async fn linux_namespace_failure_is_local_and_explicit_trusted_host_work_continues() {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "linux_no_user_namespace_host_probe",
                "--nocapture",
            ])
            .env_clear()
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("isolated no-user-namespace host probe completes")
    .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("no-user-namespace host probe passed")
    );
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[test]
#[ignore = "isolated host subprocess for forbidden user-namespace setup"]
fn linux_no_user_namespace_host_probe() {
    use nix::libc;

    // Restrict only this disposable host process. Deny namespace creation;
    // report clone3 unavailable so libc can use ordinary clone/fork without
    // namespace flags for its own threads. Alternate syscall ABIs are refused.
    // Strict Paths must refuse. Separate explicit trusted-host work remains
    // usable; this is never an automatic fallback for the refused request.
    let statement = |code, k| libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    };
    let jump = |code, k, jt, jf| libc::sock_filter { code, jt, jf, k };
    let load = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
    let equal = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
    let has_bits = (libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K) as u16;
    let ret = (libc::BPF_RET | libc::BPF_K) as u16;
    #[cfg(target_arch = "x86_64")]
    const NATIVE_AUDIT_ARCH: u32 = 0xc000_003e;
    #[cfg(target_arch = "aarch64")]
    const NATIVE_AUDIT_ARCH: u32 = 0xc000_00b7;
    let mut filter = [
        statement(load, 4), // seccomp_data.arch
        jump(equal, NATIVE_AUDIT_ARCH, 1, 0),
        statement(ret, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        statement(load, 0),                // seccomp_data.nr
        jump(has_bits, 0x4000_0000, 0, 1), // reject the x32 syscall-number bit
        statement(ret, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        jump(equal, libc::SYS_unshare as u32, 0, 1),
        statement(ret, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        jump(equal, libc::SYS_clone3 as u32, 0, 1),
        statement(ret, libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32),
        jump(equal, libc::SYS_clone as u32, 0, 3),
        statement(load, 16), // low word of seccomp_data.args[0]
        jump(
            has_bits,
            (libc::CLONE_NEWUSER
                | libc::CLONE_NEWNS
                | libc::CLONE_NEWNET
                | libc::CLONE_NEWPID
                | libc::CLONE_NEWUTS
                | libc::CLONE_NEWIPC
                | libc::CLONE_NEWCGROUP) as u32,
            0,
            1,
        ),
        statement(ret, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        statement(ret, libc::SECCOMP_RET_ALLOW),
    ];
    let program = libc::sock_fprog {
        len: filter.len().try_into().unwrap(),
        filter: filter.as_mut_ptr(),
    };
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
        0
    );
    assert_eq!(
        unsafe {
            libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                std::ptr::addr_of!(program),
            )
        },
        0,
        "{}",
        std::io::Error::last_os_error()
    );
    assert_eq!(unsafe { libc::unshare(libc::CLONE_NEWUSER) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EPERM)
    );
    let fixture = Fixture::new();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let marker = fixture.work.join("must-not-enter");
            let strict = ExecutionConfinement::try_from(fixture.spec()).unwrap();
            let launch = ProcessLaunchSpec::new(
                PathBuf::from("/bin/sh"),
                vec![
                    OsString::from("-c"),
                    OsString::from("printf escaped > must-not-enter"),
                ],
                fixture.work.clone(),
                BTreeMap::new(),
            )
            .unwrap();
            assert!(matches!(
                prepare(&strict, launch),
                Err(meerkat_sandbox::ConfinementRefusal::BackendUnavailable)
            ));
            assert!(!marker.exists(), "refused setup must not enter the target");

            // Explicit trusted-host execution is independent of the refused
            // Required request. This control proves the host/session remains
            // usable; it makes no claim of no-namespace confinement support.
            let output = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                tokio::process::Command::new("/bin/sh")
                    .args([
                        "-c",
                        "printf permitted > inside; printf 'trusted-host sibling'",
                    ])
                    .current_dir(&fixture.work)
                    .env_clear()
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .expect("explicit trusted-host sibling completes")
            .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(output.stdout, b"trusted-host sibling");
            assert_eq!(
                std::fs::read(fixture.work.join("inside")).unwrap(),
                b"permitted"
            );
            assert_eq!(
                std::fs::read(&fixture.outside).unwrap(),
                b"private canary\n"
            );
            assert!(!marker.exists());
        });
    println!("no-user-namespace host probe passed");
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn linux_refuses_directory_and_non_stream_stdio_before_target_entry() {
    use nix::libc;
    use std::os::unix::fs::OpenOptionsExt;
    use std::process::Stdio;

    let fixture = Fixture::new();
    for kind in ["directory", "path-directory", "regular", "device"] {
        let source = match kind {
            "directory" => std::fs::File::open(fixture._root.path()).unwrap(),
            "path-directory" => std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_DIRECTORY)
                .open(fixture._root.path())
                .unwrap(),
            "regular" => std::fs::File::open(&fixture.outside).unwrap(),
            "device" => std::fs::File::open("/dev/zero").unwrap(),
            _ => unreachable!(),
        };
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "linux_unsafe_stdio_host_probe",
                    "--nocapture",
                ])
                .env_clear()
                .env("MEERKAT_STDIO_KIND", kind)
                .stdin(Stdio::from(source))
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("isolated descriptor host completes")
        .unwrap();
        assert!(output.status.success(), "{kind}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("unsafe stdio refused"));
    }
    assert_eq!(
        std::fs::read(&fixture.outside).unwrap(),
        b"private canary\n"
    );
    let permitted = fixture
        .shell(fixture.spec(), "printf 'pipe sibling'", &[])
        .await;
    assert!(permitted.status.success(), "{permitted:?}");
    assert_eq!(permitted.stdout, b"pipe sibling");
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "isolated host subprocess with an explicitly inherited unsafe stdin"]
fn linux_unsafe_stdio_host_probe() {
    use nix::libc;
    use std::io::Read;
    use std::os::fd::FromRawFd;

    let kind = std::env::var("MEERKAT_STDIO_KIND").unwrap();
    if kind == "directory" || kind == "path-directory" {
        // Establish that this actual inherited directory reaches the host
        // canary before confinement. Missing or invalid FDs cannot pass.
        let descriptor =
            unsafe { libc::openat(0, c"outside".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        assert!(descriptor >= 0, "{}", std::io::Error::last_os_error());
        let mut file = unsafe { std::fs::File::from_raw_fd(descriptor) };
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        assert_eq!(text, "private canary\n");
    }
    let fixture = Fixture::new();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let launch = ProcessLaunchSpec::new(
                PathBuf::from("/bin/sh"),
                vec![
                    OsString::from("-c"),
                    OsString::from("printf entered > must-not-enter"),
                ],
                fixture.work.clone(),
                BTreeMap::new(),
            )
            .unwrap();
            let prepared = prepare(&fixture.spec().try_into().unwrap(), launch).unwrap();
            let result = prepared.spawn_with_io(meerkat_sandbox::SpawnIo {
                stdin: meerkat_sandbox::StdioMode::Inherit,
                ..meerkat_sandbox::SpawnIo::default()
            });
            let refused = match result {
                Err(error) => error.kind() == std::io::ErrorKind::Unsupported,
                Ok(mut child) => {
                    // Drain an unexpected child before reporting the regression.
                    child.kill().await.unwrap();
                    false
                }
            };
            assert!(
                refused,
                "unsafe inherited descriptor was not rejected before spawn"
            );
            assert!(!fixture.work.join("must-not-enter").exists());
        });
    println!("unsafe stdio refused");
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn linux_denies_host_signal_and_resource_mutation_equivalents() {
    let fixture = Fixture::new();
    assert_probe(
        fixture
            .probe(
                fixture.spec(),
                "host-process-mutation-denied",
                &std::process::id().to_string(),
            )
            .output()
            .await
            .unwrap(),
    );
    let permitted = fixture
        .shell(fixture.spec(), "printf 'still permitted'", &[])
        .await;
    assert!(permitted.status.success(), "{permitted:?}");
    assert_eq!(permitted.stdout, b"still permitted");
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn linux_late_setup_failure_never_executes_and_reaps_the_failed_child() {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "linux_late_setup_failure_host_probe",
                "--nocapture",
            ])
            .env_clear()
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("isolated late-setup host completes")
    .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("late setup failed without entry"));
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[test]
#[ignore = "isolated host subprocess for post-compilation syscall failure"]
fn linux_late_setup_failure_host_probe() {
    use nix::libc;
    let fixture = Fixture::new();
    let launch = ProcessLaunchSpec::new(
        PathBuf::from("/bin/sh"),
        vec![
            OsString::from("-c"),
            OsString::from("printf entered > must-not-enter"),
        ],
        fixture.work.clone(),
        BTreeMap::new(),
    )
    .unwrap();
    // This host must first support the complete requested isolation. The
    // injected failure is later, at actual child setup, not compile refusal.
    let prepared = prepare(&fixture.spec().try_into().unwrap(), launch).unwrap();
    let statement = |code, k| libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    };
    let jump = |code, k, jt, jf| libc::sock_filter { code, jt, jf, k };
    let load = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
    let equal = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
    let ret = (libc::BPF_RET | libc::BPF_K) as u16;
    #[cfg(target_arch = "x86_64")]
    const ARCH: u32 = 0xc000_003e;
    #[cfg(target_arch = "aarch64")]
    const ARCH: u32 = 0xc000_00b7;
    let mut filter = [
        statement(load, 4),
        jump(equal, ARCH, 1, 0),
        statement(ret, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        statement(load, 0),
        jump(equal, libc::SYS_mount as u32, 0, 1),
        statement(ret, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        statement(ret, libc::SECCOMP_RET_ALLOW),
    ];
    let program = libc::sock_fprog {
        len: filter.len().try_into().unwrap(),
        filter: filter.as_mut_ptr(),
    };
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
        0
    );
    assert_eq!(
        unsafe {
            libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                std::ptr::addr_of!(program),
            )
        },
        0
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let failure = match prepared.spawn() {
                Err(error) => Some(error),
                Ok(mut child) => {
                    child.kill().await.unwrap();
                    None
                }
            };
            assert_eq!(
                failure.and_then(|error| error.raw_os_error()),
                Some(libc::EPERM)
            );
            assert!(!fixture.work.join("must-not-enter").exists());
            let mut status = 0;
            assert_eq!(
                unsafe { libc::waitpid(-1, &raw mut status, libc::WNOHANG) },
                -1
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
            // Explicit TrustedHost execution, independently selected. The failed
            // Required operation is not retried through this path.
            let sibling = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                tokio::process::Command::new("/bin/sh")
                    .args(["-c", "printf 'trusted-host sibling'"])
                    .env_clear()
                    .current_dir(&fixture.work)
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .expect("trusted-host sibling completes")
            .unwrap();
            assert!(sibling.status.success(), "{sibling:?}");
            assert_eq!(sibling.stdout, b"trusted-host sibling");
            assert!(!fixture.work.join("must-not-enter").exists());
        });
    println!("late setup failed without entry");
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn linux_unix_pair_cannot_bypass_unix_denial_when_ip_is_permitted() {
    let fixture = Fixture::new();
    let path = fixture.work.join("host-datagram.sock");
    let receiver = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
    receiver.set_nonblocking(true).unwrap();
    for network in [IpNetworkAccess::Denied, IpNetworkAccess::Unrestricted] {
        let mut spec = fixture.spec();
        spec.network = network;
        assert_probe(
            fixture
                .probe(spec, "unix-pair-denied", path.to_str().unwrap())
                .output()
                .await
                .unwrap(),
        );
        let mut bytes = [0; 64];
        assert_eq!(
            receiver.recv(&mut bytes).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut permitted = fixture.spec();
    permitted.network = IpNetworkAccess::Unrestricted;
    assert_probe(
        fixture
            .probe(
                permitted,
                "tcp-allowed",
                &tcp.local_addr().unwrap().to_string(),
            )
            .output()
            .await
            .unwrap(),
    );
}
