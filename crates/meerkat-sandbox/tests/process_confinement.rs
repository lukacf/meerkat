//! Actual OS probes. Backend/setup failure is a test failure, never a skip.
#![cfg(target_os = "macos")]
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
