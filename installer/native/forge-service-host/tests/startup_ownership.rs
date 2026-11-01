//! Actual Linux descriptor checks for the private entry function. Each case
//! executes in a fresh, single-thread process with no libtest or stdio wrappers.
//! This does not invoke Connect adoption or establish installed parent custody.
#![deny(unsafe_code)]

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
#[path = "../src/bin/forge-service-host/startup.rs"]
mod startup;

#[cfg(target_os = "linux")]
mod linux {
    use super::startup;
    use std::{
        ffi::OsString,
        os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    const CASES: [&str; 6] = [
        "original_numbers_objects_cloexec_and_drop",
        "malformed_first_attempt_remains_consumed",
        "closed_parent_refuses_before_transfer",
        "closed_mapping_refuses_before_transfer",
        "closed_input_refuses_before_transfer",
        "closed_output_refuses_before_transfer",
    ];

    type Identity = (libc::dev_t, libc::ino_t, libc::mode_t, libc::dev_t, i32);

    fn arguments(parent: RawFd, mapping: RawFd) -> impl Iterator<Item = OsString> {
        [
            "--connect-protected-service".to_owned(),
            "--parent-fd".to_owned(),
            parent.to_string(),
            "--mapping-fd".to_owned(),
            mapping.to_string(),
        ]
        .into_iter()
        .map(OsString::from)
    }

    #[allow(unsafe_code)]
    fn flags(fd: RawFd) -> Result<i32, i32> {
        // F_GETFD accepts any descriptor number and never transfers ownership.
        let value = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if value < 0 {
            Err(std::io::Error::last_os_error().raw_os_error().unwrap())
        } else {
            Ok(value)
        }
    }

    #[allow(unsafe_code)]
    fn identity(fd: RawFd) -> Identity {
        // fstat initializes this POD value on success. F_GETFL only observes.
        unsafe {
            let mut value: libc::stat = std::mem::zeroed();
            assert_eq!(libc::fstat(fd, &mut value), 0);
            let mode = libc::fcntl(fd, libc::F_GETFL);
            assert!(mode >= 0);
            (
                value.st_dev,
                value.st_ino,
                value.st_mode,
                value.st_rdev,
                mode,
            )
        }
    }

    fn open_descriptors() -> Vec<RawFd> {
        // Drop the directory iterator before filtering its now-closed FD.
        // The fixture is single-threaded, so no concurrent descriptor reuse is
        // possible between enumeration and this observational F_GETFD filter.
        let mut fds: Vec<RawFd> = std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .map(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_str()
                    .unwrap()
                    .parse()
                    .unwrap()
            })
            .collect();
        fds.retain(|fd| match flags(*fd) {
            Ok(_) => true,
            Err(error) => {
                assert_eq!(error, libc::EBADF);
                false
            }
        });
        fds.sort_unstable();
        fds
    }

    #[allow(unsafe_code)]
    fn inherited_pipe() -> [OwnedFd; 2] {
        // pipe2 creates two distinct originals. Each is wrapped exactly once;
        // only its read end is deliberately inherited by the fixture child.
        unsafe {
            let mut raw = [-1; 2];
            assert_eq!(libc::pipe2(raw.as_mut_ptr(), libc::O_CLOEXEC), 0);
            let result = raw.map(|fd| OwnedFd::from_raw_fd(fd));
            assert!(raw.iter().all(|fd| *fd >= 3));
            assert_eq!(libc::fcntl(raw[0], libc::F_SETFD, 0), 0);
            result
        }
    }

    #[allow(unsafe_code)]
    fn close_inherited(fd: RawFd) {
        // This fresh child has only raw inherited numbers, no Rust owner for
        // this FD. Deliberately close it before the production function runs.
        assert_eq!(unsafe { libc::close(fd) }, 0);
    }

    #[allow(unsafe_code)]
    fn restore_open_number(fd: RawFd) {
        // No Rust owner was created on the failed attempt. Restore a real open
        // descriptor at that number, so a second refusal proves the consumed
        // guard rather than merely rediscovering the same closed descriptor.
        assert_eq!(unsafe { libc::dup2(2, fd) }, fd);
    }

    fn child(case: &str, parent: RawFd, mapping: RawFd) {
        assert!(parent >= 3 && mapping >= 3 && parent != mapping);
        assert_eq!(std::fs::read_dir("/proc/self/task").unwrap().count(), 1);
        let fds = [parent, mapping, 0, 1];
        let objects = fds.map(identity);
        let original_flags = fds.map(|fd| flags(fd).unwrap());
        let stderr = (identity(2), flags(2));
        let before = open_descriptors();

        match case {
            "original_numbers_objects_cloexec_and_drop" => {
                let original = startup::take_originals_at_entry(arguments(parent, mapping))
                    .unwrap_or_else(|_| panic!("valid open originals refused"));
                assert_eq!(
                    [
                        original.parent.as_raw_fd(),
                        original.mapping.as_raw_fd(),
                        original.input.as_raw_fd(),
                        original.output.as_raw_fd(),
                    ],
                    fds
                );
                assert_eq!(fds.map(identity), objects);
                assert_eq!(open_descriptors(), before);
                for (fd, previous) in fds.into_iter().zip(original_flags) {
                    assert_eq!(flags(fd), Ok(previous | libc::FD_CLOEXEC));
                }
                assert!(startup::take_originals_at_entry(arguments(parent, mapping)).is_err());
                assert_eq!(fds.map(identity), objects);
                assert_eq!(open_descriptors(), before);
                drop(original);
                for fd in fds {
                    assert_eq!(flags(fd), Err(libc::EBADF));
                }
                let expected: Vec<_> = before.into_iter().filter(|fd| !fds.contains(fd)).collect();
                assert_eq!(open_descriptors(), expected);
            }
            "malformed_first_attempt_remains_consumed" => {
                assert!(
                    startup::take_originals_at_entry(std::iter::once(OsString::from("--invalid")))
                        .is_err()
                );
                assert!(startup::take_originals_at_entry(arguments(parent, mapping)).is_err());
                assert_eq!(fds.map(identity), objects);
                assert_eq!(fds.map(|fd| flags(fd).unwrap()), original_flags);
                assert_eq!(open_descriptors(), before);
            }
            _ => {
                let selected = match case {
                    "closed_parent_refuses_before_transfer" => 0,
                    "closed_mapping_refuses_before_transfer" => 1,
                    "closed_input_refuses_before_transfer" => 2,
                    "closed_output_refuses_before_transfer" => 3,
                    _ => panic!("unknown fixture case"),
                };
                let closed = fds[selected];
                close_inherited(closed);
                assert!(startup::take_originals_at_entry(arguments(parent, mapping)).is_err());
                assert_eq!(flags(closed), Err(libc::EBADF));
                for (index, fd) in fds.into_iter().enumerate() {
                    if index != selected {
                        assert_eq!(identity(fd), objects[index]);
                        assert_eq!(flags(fd), Ok(original_flags[index]));
                    }
                }
                let expected: Vec<_> = before.iter().copied().filter(|fd| *fd != closed).collect();
                assert_eq!(open_descriptors(), expected);
                restore_open_number(closed);
                let restored = (fds.map(identity), fds.map(flags));
                assert!(startup::take_originals_at_entry(arguments(parent, mapping)).is_err());
                assert_eq!((fds.map(identity), fds.map(flags)), restored);
                assert_eq!(open_descriptors(), before);
            }
        }
        assert_eq!((identity(2), flags(2)), stderr);
        assert_eq!(std::fs::read_dir("/proc/self/task").unwrap().count(), 1);
        // Do not return through any output harness after original FD 0/1 close.
        std::process::exit(0);
    }

    pub(super) fn run() {
        let args: Vec<_> = std::env::args().skip(1).collect();
        if args.first().is_some_and(|arg| arg == "--entry-fixture") {
            assert_eq!(args.len(), 4);
            child(&args[1], args[2].parse().unwrap(), args[3].parse().unwrap());
            unreachable!();
        }
        if args == ["--list"] {
            for case in CASES {
                println!("{case}: test");
            }
            return;
        }
        let selected: Vec<_> = match args.as_slice() {
            [] => CASES.to_vec(),
            [flag, case] if flag == "--run" && CASES.contains(&case.as_str()) => {
                vec![case.as_str()]
            }
            _ => panic!("expected no arguments, --list, or --run CASE"),
        };
        for case in selected {
            let parent = inherited_pipe();
            let mapping = inherited_pipe();
            let mut process = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--entry-fixture".to_owned(),
                    case.to_owned(),
                    parent[0].as_raw_fd().to_string(),
                    mapping[0].as_raw_fd().to_string(),
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            drop(process.stdin.take());
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                match process.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    outcome => {
                        let _ = process.kill();
                        let joined = process.wait();
                        panic!("{case}: fixture did not finish: {outcome:?}; join: {joined:?}");
                    }
                }
            }
            let output = process.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{case}: {:?}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty(), "child wrote protocol output");
            assert!(output.stderr.is_empty(), "child wrote diagnostics");
            println!("{case}: ok");
        }
    }
}

#[cfg(target_os = "linux")]
fn main() {
    linux::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("startup_ownership requires actual Linux kernel descriptors");
    std::process::exit(77);
}
