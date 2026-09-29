//! Linux process identity, group enumeration, and exit notification.
//!
//! Identity is `(boot id, start time in clock ticks since boot)` read from
//! `/proc/<pid>/stat`; a recycled pid can never carry the same pair. Exit
//! notification uses `pidfd_open(2)`: a pidfd becomes readable when its
//! process terminates, whether or not this process is its parent.

#![allow(unsafe_code)]

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt as _;
use std::time::Instant;

use uuid::Uuid;

use super::{
    ExitWaitOutcome, HostEnvironment, ProcessIdentity, ProcessSnapshot, ProcessStartStamp,
};

const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";

fn boot_id() -> io::Result<Uuid> {
    let raw = std::fs::read_to_string(BOOT_ID_PATH)?;
    Uuid::parse_str(raw.trim()).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Parsed fields of `/proc/<pid>/stat` that custody needs.
struct ProcStat {
    #[cfg_attr(not(test), allow(dead_code))]
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

fn read_stat(pid: i32) -> io::Result<Option<ProcStat>> {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(raw) => parse_stat(&raw).map(Some),
        Err(error)
            if matches!(error.kind(), io::ErrorKind::NotFound)
                || error.raw_os_error() == Some(nix::libc::ESRCH) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
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

pub(super) fn snapshot(pid: i32) -> io::Result<Option<ProcessSnapshot>> {
    let boot_id = boot_id()?;
    Ok(read_stat(pid)?.map(|stat| snapshot_from_stat(&stat, boot_id)))
}

pub(super) fn group_members(pgid: i32) -> io::Result<Vec<(i32, ProcessSnapshot)>> {
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
        if let Some(stat) = read_stat(pid)?
            && stat.pgrp == pgid
        {
            members.push((pid, snapshot_from_stat(&stat, boot_id)));
        }
    }
    Ok(members)
}

/// The boot and pid namespace this process observes. Pids and start stamps
/// are only comparable within one environment.
pub(super) fn host_environment() -> io::Result<HostEnvironment> {
    let namespace = std::fs::metadata("/proc/self/ns/pid")?;
    Ok(HostEnvironment::Linux {
        boot_id: boot_id()?,
        pid_namespace_dev: namespace.dev(),
        pid_namespace_ino: namespace.ino(),
    })
}

fn pidfd_open(pid: i32) -> io::Result<Option<OwnedFd>> {
    // SAFETY: pidfd_open takes a pid and a flags word and returns a new file
    // descriptor or -1; it touches no caller memory.
    let raw = unsafe { nix::libc::syscall(nix::libc::SYS_pidfd_open, pid, 0) };
    if raw < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(nix::libc::ESRCH) {
            return Ok(None);
        }
        return Err(error);
    }
    let fd = i32::try_from(raw)
        .map_err(|_| io::Error::other("pidfd_open returned an out-of-range descriptor"))?;
    // SAFETY: `fd` was just returned by pidfd_open and has no other owner.
    Ok(Some(unsafe { OwnedFd::from_raw_fd(fd) }))
}

/// Open an exit handle pinned to exactly `identity`: `None` when that process
/// is already fully gone or the pid now names a different process.
fn pinned_pidfd(identity: &ProcessIdentity) -> io::Result<Option<OwnedFd>> {
    let Some(pidfd) = pidfd_open(identity.pid)? else {
        return Ok(None);
    };
    // The pidfd pins the pid, so the stamp read after opening it describes
    // the process the handle refers to.
    match snapshot(identity.pid)? {
        Some(current) if current.start == identity.start => Ok(Some(pidfd)),
        _ => Ok(None),
    }
}

fn poll_ready(pollfds: &mut [nix::libc::pollfd], timeout_ms: i32) -> io::Result<()> {
    let nfds = nix::libc::nfds_t::try_from(pollfds.len())
        .map_err(|_| io::Error::other("too many exit watches"))?;
    loop {
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

/// Whether exactly this process is still running. A pidfd becomes readable
/// only once the whole thread group has exited, so a zombie main thread with
/// live threads still counts as running.
pub(super) fn is_running(identity: &ProcessIdentity) -> io::Result<bool> {
    let Some(pidfd) = pinned_pidfd(identity)? else {
        return Ok(false);
    };
    let mut pollfd = [nix::libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: nix::libc::POLLIN,
        revents: 0,
    }];
    poll_ready(&mut pollfd, 0)?;
    Ok(pollfd[0].revents == 0)
}

/// Exit notification handles for a set of processes.
pub(super) struct ExitWatch {
    pidfds: Vec<OwnedFd>,
}

impl ExitWatch {
    pub(super) fn new(members: &[ProcessIdentity]) -> io::Result<Self> {
        let mut pidfds = Vec::with_capacity(members.len());
        for member in members {
            if let Some(pidfd) = pinned_pidfd(member)? {
                pidfds.push(pidfd);
            }
        }
        Ok(Self { pidfds })
    }

    pub(super) fn wait_all(mut self, deadline: Instant) -> io::Result<ExitWaitOutcome> {
        while !self.pidfds.is_empty() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(ExitWaitOutcome::DeadlineElapsed);
            }
            let timeout_ms = i32::try_from(remaining.as_millis().max(1)).unwrap_or(i32::MAX);
            let mut pollfds: Vec<nix::libc::pollfd> = self
                .pidfds
                .iter()
                .map(|fd| nix::libc::pollfd {
                    fd: fd.as_raw_fd(),
                    events: nix::libc::POLLIN,
                    revents: 0,
                })
                .collect();
            poll_ready(&mut pollfds, timeout_ms)?;
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
    fn own_process_snapshot_is_stable() {
        let pid = std::process::id() as i32;
        let first = snapshot(pid).unwrap().unwrap();
        let second = snapshot(pid).unwrap().unwrap();
        assert_eq!(first.start, second.start);
        let identity = ProcessIdentity {
            pid,
            start: first.start,
        };
        assert!(is_running(&identity).unwrap());
    }
}
