//! Linux process identity, group enumeration, signalling, and exit
//! notification.
//!
//! Identity is `(boot id, start time in clock ticks since boot)` read from
//! `/proc/<pid>/stat`; a recycled pid can never carry the same pair. Exit
//! notification and signalling use pidfds: a pidfd pins one process, becomes
//! readable only when that whole thread group has exited, and
//! `pidfd_send_signal` can never reach a process that later reuses the pid.
//!
//! Every observation that shows a pid cannot be one of ours (absent, hidden
//! by `hidepid`, not readable, owned by another user, a thread id rather
//! than a process) is a typed [`ProcessProbe`] or a not-running answer,
//! never an I/O error. Ownership is the effective uid from the `Uid` key of
//! `/proc/<pid>/status`, which (unlike the `/proc/<pid>` owner) does not
//! depend on the process being dumpable.

#![allow(unsafe_code)]

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::time::Instant;

use uuid::Uuid;

use super::{
    ExitWaitOutcome, HostEnvironment, ProcessIdentity, ProcessProbe, ProcessSnapshot,
    ProcessStartStamp,
};

const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";

fn boot_id() -> io::Result<Uuid> {
    let raw = std::fs::read_to_string(BOOT_ID_PATH)?;
    Uuid::parse_str(raw.trim()).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Parsed fields of `/proc/<pid>/stat` that custody needs.
struct ProcStat {
    state: u8,
    pgrp: i32,
    start_ticks: u64,
}

/// Parse `/proc/<pid>/stat`. The `comm` field is parenthesised and may itself
/// contain spaces or parentheses, so the fixed fields start after the last
/// `)`. Field numbering follows proc(5): state is field 3, pgrp field 5,
/// starttime field 22.
fn parse_stat(raw: &str) -> io::Result<ProcStat> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "malformed /proc stat record");
    let close = raw.rfind(')').ok_or_else(invalid)?;
    let mut fields = raw
        .get(close + 1..)
        .ok_or_else(invalid)?
        .split_ascii_whitespace();
    let state = fields
        .next()
        .and_then(|field| field.bytes().next())
        .ok_or_else(invalid)?;
    // Skip ppid (field 4).
    fields.next().ok_or_else(invalid)?;
    let pgrp = fields
        .next()
        .and_then(|field| field.parse::<i32>().ok())
        .ok_or_else(invalid)?;
    // Fields 6..=21 precede starttime; pgrp was field 5.
    let start_ticks = fields
        .nth(21 - 5)
        .and_then(|field| field.parse::<u64>().ok())
        .ok_or_else(invalid)?;
    Ok(ProcStat {
        state,
        pgrp,
        start_ticks,
    })
}

/// Outcome of reading `/proc/<pid>/stat`.
enum StatRead {
    Absent,
    /// Not readable by us (`hidepid`, another user's hardened process): it
    /// cannot be a process this host spawned and may signal.
    Foreign,
    Stat(ProcStat),
}

/// Classify a `/proc/<pid>/stat` read.
fn classify_stat_read(read: io::Result<String>) -> io::Result<StatRead> {
    match read {
        Ok(raw) => parse_stat(&raw).map(StatRead::Stat),
        Err(error) => match error.raw_os_error() {
            Some(nix::libc::ENOENT | nix::libc::ESRCH) => Ok(StatRead::Absent),
            Some(nix::libc::EACCES | nix::libc::EPERM) => Ok(StatRead::Foreign),
            _ if error.kind() == io::ErrorKind::NotFound => Ok(StatRead::Absent),
            _ => Err(error),
        },
    }
}

fn read_stat(pid: i32) -> io::Result<StatRead> {
    classify_stat_read(std::fs::read_to_string(format!("/proc/{pid}/stat")))
}

/// This process's effective uid.
fn own_effective_uid() -> u32 {
    // SAFETY: geteuid takes no arguments, cannot fail, and touches no caller
    // memory.
    unsafe { nix::libc::geteuid() }
}

/// Parse the effective uid (second field of the `Uid` key) of a
/// `/proc/<pid>/status` record.
fn parse_effective_uid(status: &str) -> io::Result<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|value| value.split_ascii_whitespace().nth(1))
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "status record lacks Uid"))
}

/// Outcome of reading a process's owner.
enum OwnerRead {
    Absent,
    Foreign,
    Ours,
}

fn classify_owner(read: io::Result<String>, own_uid: u32) -> io::Result<OwnerRead> {
    match read {
        Ok(status) => Ok(if parse_effective_uid(&status)? == own_uid {
            OwnerRead::Ours
        } else {
            OwnerRead::Foreign
        }),
        Err(error) => Ok(match classify_stat_read(Err(error))? {
            StatRead::Absent => OwnerRead::Absent,
            StatRead::Foreign | StatRead::Stat(_) => OwnerRead::Foreign,
        }),
    }
}

fn read_owner(pid: i32) -> io::Result<OwnerRead> {
    classify_owner(
        std::fs::read_to_string(format!("/proc/{pid}/status")),
        own_effective_uid(),
    )
}

/// Stat plus ownership: a readable process of another user is foreign.
fn read_own_stat(pid: i32) -> io::Result<StatRead> {
    let stat = read_stat(pid)?;
    if !matches!(stat, StatRead::Stat(_)) {
        return Ok(stat);
    }
    Ok(match read_owner(pid)? {
        OwnerRead::Ours => stat,
        OwnerRead::Absent => StatRead::Absent,
        OwnerRead::Foreign => StatRead::Foreign,
    })
}

fn snapshot_from_stat(stat: &ProcStat, boot_id: Uuid) -> ProcessSnapshot {
    ProcessSnapshot {
        pgid: stat.pgrp,
        start: ProcessStartStamp::LinuxBoot {
            boot_id,
            start_ticks: stat.start_ticks,
        },
    }
}

pub(super) fn probe(pid: i32) -> io::Result<ProcessProbe> {
    let boot_id = boot_id()?;
    Ok(match read_own_stat(pid)? {
        StatRead::Absent => ProcessProbe::Absent,
        StatRead::Foreign => ProcessProbe::Foreign,
        StatRead::Stat(stat) => ProcessProbe::Observed(snapshot_from_stat(&stat, boot_id)),
    })
}

/// Members of group `pgid`. Unreadable processes are omitted (their group
/// cannot be known); readable members owned by another user are reported as
/// [`ProcessProbe::Foreign`]. Callers cross-check an empty result against
/// `kill(-pgid, 0)`.
pub(super) fn group_members(pgid: i32) -> io::Result<Vec<(i32, ProcessProbe)>> {
    let boot_id = boot_id()?;
    let mut members = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        if let StatRead::Stat(stat) = read_stat(pid)?
            && stat.pgrp == pgid
        {
            match read_owner(pid)? {
                OwnerRead::Absent => {}
                OwnerRead::Foreign => members.push((pid, ProcessProbe::Foreign)),
                OwnerRead::Ours => members.push((
                    pid,
                    ProcessProbe::Observed(snapshot_from_stat(&stat, boot_id)),
                )),
            }
        }
    }
    Ok(members)
}

/// The boot and pid namespace this process observes. Pids and start stamps
/// are only comparable within one environment.
pub(super) fn host_environment() -> io::Result<HostEnvironment> {
    let namespace = std::fs::metadata("/proc/self/ns/pid").ok();
    Ok(HostEnvironment::Linux {
        boot_id: boot_id()?,
        pid_namespace_dev: namespace.as_ref().map(MetadataExt::dev),
        pid_namespace_ino: namespace.as_ref().map(MetadataExt::ino),
    })
}

/// Outcome of `pidfd_open(2)`.
enum PidfdOpen {
    Opened(OwnedFd),
    /// ESRCH: no such process.
    Gone,
    /// EINVAL, or ENOENT on kernels with `PIDFD_THREAD` (6.9+): the id is
    /// not a thread-group leader (for example a thread id that reused a
    /// recorded pid), so it is not the recorded process.
    NotAProcess,
    /// ENOSYS or EPERM: pidfds are unavailable (pre-5.3 kernel, seccomp).
    Unsupported,
}

fn classify_pidfd_error(error: io::Error) -> io::Result<PidfdOpen> {
    match error.raw_os_error() {
        Some(nix::libc::ESRCH) => Ok(PidfdOpen::Gone),
        Some(nix::libc::EINVAL | nix::libc::ENOENT) => Ok(PidfdOpen::NotAProcess),
        Some(nix::libc::ENOSYS | nix::libc::EPERM) => Ok(PidfdOpen::Unsupported),
        _ => Err(error),
    }
}

fn pidfd_open(pid: i32) -> io::Result<PidfdOpen> {
    // SAFETY: pidfd_open takes a pid and a flags word and returns a new file
    // descriptor or -1; it touches no caller memory.
    let raw = unsafe { nix::libc::syscall(nix::libc::SYS_pidfd_open, pid, 0) };
    if raw < 0 {
        return classify_pidfd_error(io::Error::last_os_error());
    }
    let fd = i32::try_from(raw)
        .map_err(|_| io::Error::other("pidfd_open returned an out-of-range descriptor"))?;
    // SAFETY: `fd` was just returned by pidfd_open and has no other owner.
    Ok(PidfdOpen::Opened(unsafe { OwnedFd::from_raw_fd(fd) }))
}

fn stamp_matches(identity: &ProcessIdentity) -> io::Result<bool> {
    Ok(matches!(
        probe(identity.pid)?,
        ProcessProbe::Observed(current) if current.start == identity.start
    ))
}

/// Whether `pid` is a thread-group leader (a process), from the `Tgid` key
/// of `/proc/<pid>/status`. Used only without pidfd support, where EINVAL is
/// not available to reject thread ids.
fn is_thread_group_leader(pid: i32) -> io::Result<bool> {
    let status = match std::fs::read_to_string(format!("/proc/{pid}/status")) {
        Ok(status) => status,
        Err(error) => {
            return match classify_stat_read(Err(error))? {
                StatRead::Absent | StatRead::Foreign | StatRead::Stat(_) => Ok(false),
            };
        }
    };
    let tgid = status
        .lines()
        .find_map(|line| line.strip_prefix("Tgid:"))
        .and_then(|value| value.trim().parse::<i32>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "status record lacks Tgid"))?;
    Ok(tgid == pid)
}

/// A handle on exactly `identity`, or why there is none.
enum Pinned {
    Pidfd(OwnedFd),
    /// The recorded process is gone, or the pid names something else.
    NotRunning,
    /// Pidfds are unavailable; the stamp matched and the pid is a process.
    StampOnly,
}

/// Pin `identity`: compare stamps from `/proc` first (so a foreign or absent
/// pid is classified before any pidfd call), then open a pidfd and re-check
/// the stamp, which the pidfd now pins.
fn pin(identity: &ProcessIdentity) -> io::Result<Pinned> {
    if !stamp_matches(identity)? {
        return Ok(Pinned::NotRunning);
    }
    match pidfd_open(identity.pid)? {
        PidfdOpen::Opened(pidfd) => Ok(if stamp_matches(identity)? {
            Pinned::Pidfd(pidfd)
        } else {
            Pinned::NotRunning
        }),
        PidfdOpen::Gone | PidfdOpen::NotAProcess => Ok(Pinned::NotRunning),
        PidfdOpen::Unsupported => Ok(if is_thread_group_leader(identity.pid)? {
            Pinned::StampOnly
        } else {
            Pinned::NotRunning
        }),
    }
}

/// Poll until readiness or the deadline (`None`: a non-blocking probe). The
/// timeout is recomputed after an interrupted call so EINTR cannot stretch
/// the wait past the deadline.
fn poll_until(pollfds: &mut [nix::libc::pollfd], deadline: Option<Instant>) -> io::Result<()> {
    let nfds = nix::libc::nfds_t::try_from(pollfds.len())
        .map_err(|_| io::Error::other("too many exit watches"))?;
    loop {
        let timeout_ms = match deadline {
            None => 0,
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX)
            }
        };
        // SAFETY: `pollfds` is a live, correctly sized array of pollfd
        // records for the duration of the call.
        let ready = unsafe { nix::libc::poll(pollfds.as_mut_ptr(), nfds, timeout_ms) };
        if ready >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Fallback liveness from `/proc` when pidfds are unavailable: a zombie
/// thread-group leader still runs while it has other live threads.
fn running_from_proc(pid: i32) -> io::Result<bool> {
    let StatRead::Stat(stat) = read_stat(pid)? else {
        return Ok(false);
    };
    if !matches!(stat.state, b'Z' | b'X') {
        return Ok(true);
    }
    match std::fs::read_dir(format!("/proc/{pid}/task")) {
        Ok(tasks) => Ok(tasks.count() > 1),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Whether exactly this process is still running. A pidfd becomes readable
/// only once the whole thread group has exited, so a zombie main thread with
/// live threads still counts as running.
pub(super) fn is_running(identity: &ProcessIdentity) -> io::Result<bool> {
    match pin(identity)? {
        Pinned::NotRunning => Ok(false),
        Pinned::StampOnly => running_from_proc(identity.pid),
        Pinned::Pidfd(pidfd) => {
            let mut pollfd = [nix::libc::pollfd {
                fd: pidfd.as_raw_fd(),
                events: nix::libc::POLLIN,
                revents: 0,
            }];
            poll_until(&mut pollfd, None)?;
            Ok(pollfd[0].revents == 0)
        }
    }
}

/// Whether kernel exit notification (pidfds) is available to this process.
pub(super) fn exit_notification_available() -> io::Result<bool> {
    let pid =
        i32::try_from(std::process::id()).map_err(|_| io::Error::other("own pid out of range"))?;
    Ok(match pidfd_open(pid)? {
        PidfdOpen::Opened(_) => true,
        PidfdOpen::Unsupported => false,
        PidfdOpen::Gone | PidfdOpen::NotAProcess => {
            return Err(io::Error::other(
                "own pid is not observable through pidfd_open",
            ));
        }
    })
}

/// Exit notification handles for a set of processes.
pub(super) struct ExitWatch {
    pidfds: Vec<OwnedFd>,
}

impl ExitWatch {
    /// SIGKILL every watched member through its own pidfd, so a pid reused
    /// since it was listed can never be signalled. Returns how many members
    /// the kernel refused to let us signal (EPERM).
    pub(super) fn kill(&self, _pgid: i32) -> io::Result<usize> {
        let mut refused = 0;
        for pidfd in &self.pidfds {
            // SAFETY: pidfd_send_signal takes a live pidfd, a signal number,
            // a null siginfo and zero flags; it touches no caller memory.
            let rc = unsafe {
                nix::libc::syscall(
                    nix::libc::SYS_pidfd_send_signal,
                    pidfd.as_raw_fd(),
                    nix::libc::SIGKILL,
                    std::ptr::null::<nix::libc::siginfo_t>(),
                    0,
                )
            };
            if rc < 0 {
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(nix::libc::ESRCH) => {}
                    Some(nix::libc::EPERM) => refused += 1,
                    _ => return Err(error),
                }
            }
        }
        Ok(refused)
    }

    pub(super) fn new(members: &[ProcessIdentity]) -> io::Result<Self> {
        let mut pidfds = Vec::with_capacity(members.len());
        for member in members {
            match pin(member)? {
                Pinned::Pidfd(pidfd) => pidfds.push(pidfd),
                Pinned::NotRunning => {}
                Pinned::StampOnly => {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "kernel exit notification (pidfd) is unavailable",
                    ));
                }
            }
        }
        Ok(Self { pidfds })
    }

    pub(super) fn wait_all(mut self, deadline: Instant) -> io::Result<ExitWaitOutcome> {
        while !self.pidfds.is_empty() {
            if Instant::now() >= deadline {
                return Ok(ExitWaitOutcome::DeadlineElapsed);
            }
            let mut pollfds: Vec<nix::libc::pollfd> = self
                .pidfds
                .iter()
                .map(|fd| nix::libc::pollfd {
                    fd: fd.as_raw_fd(),
                    events: nix::libc::POLLIN,
                    revents: 0,
                })
                .collect();
            poll_until(&mut pollfds, Some(deadline))?;
            let mut index = 0;
            self.pidfds.retain(|_| {
                let exited = pollfds[index].revents != 0;
                index += 1;
                !exited
            });
        }
        Ok(ExitWaitOutcome::AllExited)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parse_stat_handles_parenthesised_comm() {
        let raw = "4242 (we ird) (name)) S 1 4240 4240 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1 2 3";
        let stat = parse_stat(raw).unwrap();
        assert_eq!(stat.state, b'S');
        assert_eq!(stat.pgrp, 4240);
        assert_eq!(stat.start_ticks, 987_654);
    }

    #[test]
    fn unreadable_stat_is_foreign_and_missing_stat_is_absent() {
        for errno in [nix::libc::EACCES, nix::libc::EPERM] {
            assert!(matches!(
                classify_stat_read(Err(io::Error::from_raw_os_error(errno))).unwrap(),
                StatRead::Foreign
            ));
        }
        for errno in [nix::libc::ENOENT, nix::libc::ESRCH] {
            assert!(matches!(
                classify_stat_read(Err(io::Error::from_raw_os_error(errno))).unwrap(),
                StatRead::Absent
            ));
        }
        assert!(classify_stat_read(Err(io::Error::from_raw_os_error(nix::libc::EIO))).is_err());
    }

    #[test]
    fn another_users_readable_process_is_foreign() {
        let status = |uid: u32| format!("Name:\tx\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n");
        assert!(matches!(
            classify_owner(Ok(status(1000)), 1000).unwrap(),
            OwnerRead::Ours
        ));
        assert!(matches!(
            classify_owner(Ok(status(0)), 1000).unwrap(),
            OwnerRead::Foreign
        ));
        // Real uid differs, effective uid is ours: ours.
        assert!(matches!(
            classify_owner(Ok("Uid:\t0\t1000\t0\t1000\n".to_owned()), 1000).unwrap(),
            OwnerRead::Ours
        ));
        for errno in [nix::libc::EACCES, nix::libc::EPERM] {
            assert!(matches!(
                classify_owner(Err(io::Error::from_raw_os_error(errno)), 1000).unwrap(),
                OwnerRead::Foreign
            ));
        }
        assert!(matches!(
            classify_owner(Err(io::Error::from_raw_os_error(nix::libc::ENOENT)), 1000).unwrap(),
            OwnerRead::Absent
        ));
        assert_eq!(parse_effective_uid(&status(42)).unwrap(), 42);
    }

    #[test]
    fn pidfds_are_available_on_the_test_kernel() {
        assert!(exit_notification_available().unwrap());
    }

    #[test]
    fn pidfd_errors_classify_without_failing() {
        let classify = |errno| classify_pidfd_error(io::Error::from_raw_os_error(errno)).unwrap();
        assert!(matches!(classify(nix::libc::ESRCH), PidfdOpen::Gone));
        assert!(matches!(
            classify(nix::libc::EINVAL),
            PidfdOpen::NotAProcess
        ));
        assert!(matches!(
            classify(nix::libc::ENOENT),
            PidfdOpen::NotAProcess
        ));
        assert!(matches!(
            classify(nix::libc::ENOSYS),
            PidfdOpen::Unsupported
        ));
        assert!(matches!(classify(nix::libc::EPERM), PidfdOpen::Unsupported));
        assert!(classify_pidfd_error(io::Error::from_raw_os_error(nix::libc::EMFILE)).is_err());
    }

    #[test]
    fn own_process_is_running_and_stable() {
        let pid = std::process::id() as i32;
        let ProcessProbe::Observed(first) = probe(pid).unwrap() else {
            panic!("own process must be observable");
        };
        let ProcessProbe::Observed(second) = probe(pid).unwrap() else {
            panic!("own process must be observable");
        };
        assert_eq!(first.start, second.start);
        let identity = ProcessIdentity {
            pid,
            start: first.start,
        };
        assert!(is_running(&identity).unwrap());
        assert!(running_from_proc(pid).unwrap());
        assert!(is_thread_group_leader(pid).unwrap());
    }

    #[test]
    fn a_thread_id_is_never_the_recorded_process() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            sender.send(nix::unistd::gettid().as_raw()).unwrap();
            let _ = wait.recv();
        });
        let tid = receiver.recv().unwrap();
        // /proc/<tid>/stat is readable for a thread id, and pidfd_open on it
        // fails with EINVAL; neither may surface as an error, and the thread
        // (and so this process) must never be signalled.
        let ProcessProbe::Observed(thread_stat) = probe(tid).unwrap() else {
            panic!("a live thread id is readable through /proc");
        };
        let as_recorded = ProcessIdentity {
            pid: tid,
            start: thread_stat.start,
        };
        assert!(!is_running(&as_recorded).unwrap());
        assert!(!is_thread_group_leader(tid).unwrap());
        let watch = ExitWatch::new(&[as_recorded]).unwrap();
        assert_eq!(watch.kill(tid).unwrap(), 0);
        assert!(
            watch.pidfds.is_empty(),
            "a thread id is never pinned or signalled"
        );
        release.send(()).unwrap();
        thread.join().unwrap();
    }
}
