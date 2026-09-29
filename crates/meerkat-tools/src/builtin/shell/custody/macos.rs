//! macOS process identity, group enumeration, and exit notification.
//!
//! Identity is the kernel's absolute process start time from
//! `proc_pidinfo(PROC_PIDTBSDINFO)`; a recycled pid can never carry the same
//! start time. Exit notification uses a kqueue `EVFILT_PROC`/`NOTE_EXIT`
//! registration, which works for processes this process did not spawn.

#![allow(unsafe_code)]

use std::collections::BTreeSet;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Instant;

use nix::libc;

use super::{ExitWaitOutcome, ProcessSnapshot, ProcessStartStamp};

fn bsd_info(pid: i32) -> io::Result<Option<libc::proc_bsdinfo>> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
        .map_err(|_| io::Error::other("proc_bsdinfo size out of range"))?;
    // SAFETY: the buffer is a live, correctly sized proc_bsdinfo; the kernel
    // writes at most `size` bytes into it.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast::<libc::c_void>(),
            size,
        )
    };
    if written <= 0 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            // Absent, or a zombie the kernel no longer describes.
            Some(libc::ESRCH | 0) | None => Ok(None),
            _ => Err(error),
        };
    }
    if written != size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "short proc_pidinfo(PROC_PIDTBSDINFO) record",
        ));
    }
    // SAFETY: zero-initialised plain-integer record fully written by the
    // kernel (checked above).
    Ok(Some(unsafe { info.assume_init() }))
}

fn snapshot_from_info(info: &libc::proc_bsdinfo) -> io::Result<ProcessSnapshot> {
    Ok(ProcessSnapshot {
        pgid: i32::try_from(info.pbi_pgid)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "pgid out of range"))?,
        start: ProcessStartStamp::Darwin {
            start_sec: info.pbi_start_tvsec,
            start_usec: info.pbi_start_tvusec,
        },
        exited: info.pbi_status == libc::SZOMB,
    })
}

pub(super) fn snapshot(pid: i32) -> io::Result<Option<ProcessSnapshot>> {
    bsd_info(pid)?.as_ref().map(snapshot_from_info).transpose()
}

pub(super) fn group_members(pgid: i32) -> io::Result<Vec<(i32, ProcessSnapshot)>> {
    let mut capacity = 256usize;
    let pids = loop {
        let mut buffer = vec![0 as libc::pid_t; capacity];
        let bytes = i32::try_from(capacity * std::mem::size_of::<libc::pid_t>())
            .map_err(|_| io::Error::other("process group too large"))?;
        // SAFETY: `buffer` is a live pid_t array of exactly `bytes` bytes.
        let count = unsafe {
            libc::proc_listpgrppids(pgid, buffer.as_mut_ptr().cast::<libc::c_void>(), bytes)
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        let count = usize::try_from(count).unwrap_or(0);
        if count < capacity {
            buffer.truncate(count);
            break buffer;
        }
        capacity = capacity.saturating_mul(2);
    };
    let mut members = Vec::with_capacity(pids.len());
    for pid in pids.into_iter().filter(|pid| *pid > 0) {
        if let Some(info) = bsd_info(pid)? {
            let snapshot = snapshot_from_info(&info)?;
            // proc_listpgrppids is a point-in-time listing; re-check.
            if snapshot.pgid == pgid {
                members.push((pid, snapshot));
            }
        }
    }
    Ok(members)
}

/// Exit notification registrations for a set of processes.
pub(super) struct ExitWatch {
    kqueue: OwnedFd,
    pending: BTreeSet<i32>,
}

fn proc_exit_change(pid: i32) -> io::Result<libc::kevent> {
    Ok(libc::kevent {
        ident: libc::uintptr_t::try_from(pid)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "negative pid"))?,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    })
}

impl ExitWatch {
    pub(super) fn new(pids: &[i32]) -> io::Result<Self> {
        // SAFETY: kqueue() takes no arguments and returns a descriptor or -1.
        let raw = unsafe { libc::kqueue() };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` was just returned by kqueue() and has no other owner.
        let kqueue = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut pending = BTreeSet::new();
        for &pid in pids {
            let change = proc_exit_change(pid)?;
            // SAFETY: one live change record, no event buffer, no timeout.
            let rc = unsafe {
                libc::kevent(
                    kqueue.as_raw_fd(),
                    std::ptr::from_ref(&change),
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };
            if rc < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    // Already exited (zombies are not attachable).
                    continue;
                }
                return Err(error);
            }
            pending.insert(pid);
        }
        Ok(Self { kqueue, pending })
    }

    pub(super) fn wait_all(mut self, deadline: Instant) -> io::Result<ExitWaitOutcome> {
        while !self.pending.is_empty() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(ExitWaitOutcome::DeadlineElapsed);
            }
            let timeout = libc::timespec {
                tv_sec: libc::time_t::try_from(remaining.as_secs()).unwrap_or(libc::time_t::MAX),
                tv_nsec: libc::c_long::from(remaining.subsec_nanos()),
            };
            let capacity = self.pending.len();
            let mut events: Vec<libc::kevent> = Vec::with_capacity(capacity);
            let wanted = i32::try_from(capacity).unwrap_or(i32::MAX);
            // SAFETY: `events` has room for `wanted` records; the kernel
            // initialises the first `ready` of them.
            let ready = unsafe {
                libc::kevent(
                    self.kqueue.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    events.as_mut_ptr(),
                    wanted,
                    std::ptr::from_ref(&timeout),
                )
            };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            let ready = usize::try_from(ready).unwrap_or(0).min(capacity);
            // SAFETY: the kernel initialised exactly `ready` records.
            unsafe { events.set_len(ready) };
            for event in &events {
                if event.filter != libc::EVFILT_PROC {
                    continue;
                }
                let Ok(pid) = i32::try_from(event.ident) else {
                    continue;
                };
                let exited = if event.flags & libc::EV_ERROR != 0 {
                    // The only error that proves cessation is ESRCH: the
                    // process exited between registration and delivery.
                    if event.data != libc::ESRCH as libc::intptr_t {
                        return Err(io::Error::from_raw_os_error(
                            i32::try_from(event.data).unwrap_or(libc::EIO),
                        ));
                    }
                    true
                } else {
                    event.fflags & libc::NOTE_EXIT != 0
                };
                if exited {
                    self.pending.remove(&pid);
                }
            }
        }
        Ok(ExitWaitOutcome::AllExited)
    }
}
