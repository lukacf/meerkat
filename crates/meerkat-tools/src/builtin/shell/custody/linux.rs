//! Linux process identity, group enumeration, and exit notification.
//!
//! Identity is `(boot id, start time in clock ticks since boot)` read from
//! `/proc/<pid>/stat`; a recycled pid can never carry the same pair. Exit
//! notification uses `pidfd_open(2)`: a pidfd becomes readable when its
//! process terminates, whether or not this process is its parent.

#![allow(unsafe_code)]

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Instant;

use uuid::Uuid;

use super::{ExitWaitOutcome, ProcessSnapshot, ProcessStartStamp};

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
        // Z (zombie) and X (dead) processes can never execute again.
        exited: matches!(stat.state, b'Z' | b'X'),
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

/// Exit notification handles for a set of processes.
pub(super) struct ExitWatch {
    pidfds: Vec<OwnedFd>,
}

impl ExitWatch {
    pub(super) fn new(pids: &[i32]) -> io::Result<Self> {
        let mut pidfds = Vec::with_capacity(pids.len());
        for &pid in pids {
            // SAFETY: pidfd_open takes a pid and a flags word and returns a new
            // file descriptor or -1; it touches no caller memory.
            let raw = unsafe { nix::libc::syscall(nix::libc::SYS_pidfd_open, pid, 0) };
            if raw < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(nix::libc::ESRCH) {
                    // Already fully gone: nothing to wait for.
                    continue;
                }
                return Err(error);
            }
            let fd = i32::try_from(raw)
                .map_err(|_| io::Error::other("pidfd_open returned an out-of-range descriptor"))?;
            // SAFETY: `fd` was just returned by pidfd_open and is owned by no
            // other handle.
            pidfds.push(unsafe { OwnedFd::from_raw_fd(fd) });
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
            let nfds = nix::libc::nfds_t::try_from(pollfds.len())
                .map_err(|_| io::Error::other("too many exit watches"))?;
            // SAFETY: `pollfds` is a live, correctly sized array of pollfd
            // records for the duration of the call.
            let ready = unsafe { nix::libc::poll(pollfds.as_mut_ptr(), nfds, timeout_ms) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
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
        assert!(!first.exited);
    }
}
