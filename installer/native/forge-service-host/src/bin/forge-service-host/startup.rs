//! Private, one-time Linux process-entry boundary. Never used by a library,
//! runtime worker, logger, test harness parent or application request handler.
use std::{
    ffi::OsString,
    os::fd::{FromRawFd, OwnedFd, RawFd},
    sync::atomic::{AtomicBool, Ordering},
};

static TAKEN: AtomicBool = AtomicBool::new(false);

pub(super) struct Originals {
    pub(super) parent: OwnedFd,
    pub(super) mapping: OwnedFd,
    pub(super) input: OwnedFd,
    pub(super) output: OwnedFd,
}

fn descriptor(value: &OsString) -> Result<RawFd, ()> {
    let value = value.to_str().ok_or(())?;
    if value.is_empty() || value.len() > 10 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    let fd = value.parse::<RawFd>().map_err(|_| ())?;
    if fd < 3 || fd.to_string() != value {
        return Err(());
    }
    Ok(fd)
}

fn parse(arguments: impl Iterator<Item = OsString>) -> Result<[RawFd; 4], ()> {
    // Bound even malformed argument lists and reject every unknown option.
    let args: Vec<_> = arguments.take(6).collect();
    if args.len() != 5
        || args[0] != "--connect-protected-service"
        || args[1] != "--parent-fd"
        || args[3] != "--mapping-fd"
    {
        return Err(());
    }
    let parent = descriptor(&args[2])?;
    let mapping = descriptor(&args[4])?;
    if parent == mapping {
        return Err(());
    }
    Ok([parent, mapping, 0, 1])
}

pub(super) fn take_originals_at_entry(
    arguments: impl Iterator<Item = OsString>,
) -> Result<Originals, ()> {
    if TAKEN.swap(true, Ordering::SeqCst) {
        return Err(());
    }
    let fds = parse(arguments)?;
    // SAFETY: this module has exactly one production caller, as main's first
    // action before threads, stdio handles, signal handlers, helpers or loggers.
    // All four integers are distinct; fd2 is excluded. F_GETFD accepts raw
    // integers and verifies every descriptor is open before any ownership is
    // created. No code here closes/reopens/duplicates a descriptor. F_SETFD
    // only adds CLOEXEC; it does not change the underlying object or owner.
    // No concurrent code can close these inherited FDs between validation and
    // transfer. FromRawFd takes each original exactly once, and TAKEN prevents
    // a second acquisition even on failure. Connect immediately receives all
    // four, validates their actual objects and retains/drops the owned values.
    unsafe {
        let mut flags = [0; 4];
        for (index, fd) in fds.iter().enumerate() {
            flags[index] = libc::fcntl(*fd, libc::F_GETFD);
            if flags[index] < 0 {
                return Err(());
            }
        }
        for (fd, flags) in fds.iter().zip(flags) {
            if libc::fcntl(*fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
                return Err(());
            }
        }
        Ok(Originals {
            parent: OwnedFd::from_raw_fd(fds[0]),
            mapping: OwnedFd::from_raw_fd(fds[1]),
            input: OwnedFd::from_raw_fd(fds[2]),
            output: OwnedFd::from_raw_fd(fds[3]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(parent: &str, mapping: &str) -> Vec<OsString> {
        [
            "--connect-protected-service",
            "--parent-fd",
            parent,
            "--mapping-fd",
            mapping,
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }

    #[test]
    fn descriptor_arguments_are_closed_and_never_redirect_stdin_stdout_or_stderr() {
        assert_eq!(parse(args("3", "4").into_iter()), Ok([3, 4, 0, 1]));
        assert_eq!(
            parse(args("2147483647", "4").into_iter()),
            Ok([2147483647, 4, 0, 1])
        );
        for invalid in [
            "",
            "0",
            "1",
            "2",
            "-3",
            "+3",
            "03",
            " 3",
            "3 ",
            "3.0",
            "2147483648",
        ] {
            assert!(parse(args(invalid, "4").into_iter()).is_err());
            assert!(parse(args("4", invalid).into_iter()).is_err());
        }
        assert!(parse(args("3", "3").into_iter()).is_err());
    }

    #[test]
    fn unknown_duplicate_missing_or_non_utf8_arguments_are_refused() {
        use std::os::unix::ffi::OsStringExt;
        for index in [0, 1, 3] {
            let mut raw = args("3", "4");
            raw[index] = "--other".into();
            assert!(parse(raw.into_iter()).is_err());
        }
        let mut raw = args("3", "4");
        raw.push("--parent-fd".into());
        assert!(parse(raw.into_iter()).is_err());
        let mut raw = args("3", "4");
        raw.pop();
        assert!(parse(raw.into_iter()).is_err());
        let mut raw = args("3", "4");
        raw[2] = OsString::from_vec(vec![0xff]);
        assert!(parse(raw.into_iter()).is_err());
    }
}
