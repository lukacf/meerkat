//! macOS process identity, group enumeration, signalling, and exit
//! notification.
//!
//! Identity is the kernel's absolute process start time from
//! `proc_pidinfo(PROC_PIDTBSDINFO)`; a recycled pid can never carry the same
//! start time. Exit notification uses a kqueue `EVFILT_PROC`/`NOTE_EXIT`
//! registration, which works for processes this process did not spawn.
//!
//! Another user's process can never be one this host spawned and may
//! signal: `PROC_PIDTBSDINFO` either reports its uid (compared with our
//! effective uid) or refuses with EPERM. Both are a typed
//! [`ProcessProbe::Foreign`], never an I/O error.
//! (`PROC_PIDT_SHORTBSDINFO` is readable for every process but carries no
//! start time, so it cannot establish identity.)

#![allow(unsafe_code)]

use std::collections::BTreeSet;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Instant;

use nix::libc;
use uuid::Uuid;

use super::{
    ExitWaitOutcome, HostEnvironment, ProcessIdentity, ProcessProbe, ProcessSnapshot,
    ProcessStartStamp,
};

/// Reset errno so a failure is never misread from a stale value.
fn clear_errno() {
    // SAFETY: `__error` returns this thread's errno slot, valid for writes.
    unsafe { *libc::__error() = 0 };
}

/// Outcome of `PROC_PIDTBSDINFO` for one pid.
enum BsdInfo {
    Absent,
    Foreign,
    Ours(libc::proc_bsdinfo),
}

fn bsd_info(pid: i32) -> io::Result<BsdInfo> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
        .map_err(|_| io::Error::other("proc_bsdinfo size out of range"))?;
    clear_errno();
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
        match error.raw_os_error() {
            Some(libc::EPERM | libc::EACCES) => return Ok(BsdInfo::Foreign),
            // ESRCH: the kernel no longer describes the pid as a live
            // process. It is gone, or it is a zombie - PROC_PIDTBSDINFO does
            // not describe zombies on macOS even though kill(pid, 0) still
            // succeeds on them. Either way it can never run again.
            Some(libc::ESRCH) => return Ok(BsdInfo::Absent),
            _ => {}
        }
        // Any other failure: believe absence only when kill(2) confirms it.
        return match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
            Err(nix::errno::Errno::ESRCH) => Ok(BsdInfo::Absent),
            Err(nix::errno::Errno::EPERM) => Ok(BsdInfo::Foreign),
            Ok(()) => Err(if error.raw_os_error().unwrap_or(0) == 0 {
                io::Error::other("proc_pidinfo failed for an existing process")
            } else {
                error
            }),
            Err(other) => Err(io::Error::from(other)),
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
    let info = unsafe { info.assume_init() };
    // SAFETY: geteuid takes no arguments, cannot fail, and touches no
    // caller memory.
    let effective_uid = unsafe { libc::geteuid() };
    if info.pbi_uid != effective_uid {
        return Ok(BsdInfo::Foreign);
    }
    Ok(BsdInfo::Ours(info))
}

fn snapshot_from_info(info: &libc::proc_bsdinfo) -> io::Result<ProcessSnapshot> {
    Ok(ProcessSnapshot {
        pgid: i32::try_from(info.pbi_pgid)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "pgid out of range"))?,
        start: ProcessStartStamp::Darwin {
            start_sec: info.pbi_start_tvsec,
            start_usec: info.pbi_start_tvusec,
        },
    })
}

pub(super) fn probe(pid: i32) -> io::Result<ProcessProbe> {
    Ok(match bsd_info(pid)? {
        BsdInfo::Absent => ProcessProbe::Absent,
        BsdInfo::Foreign => ProcessProbe::Foreign,
        BsdInfo::Ours(info) => ProcessProbe::Observed(snapshot_from_info(&info)?),
    })
}

/// An empty group listing is authoritative on macOS: listing failures are
/// detected through errno, and the kernel may not list zombies, which still
/// answer `kill(-pgid, 0)` although they can never run again.
pub(super) const EMPTY_LISTING_IS_AUTHORITATIVE: bool = true;

/// Members of group `pgid` as listed by the kernel. A listed pid that probes
/// as absent (a zombie, or a process that exited while being read) is
/// reported as [`ProcessProbe::Absent`], so callers can tell a group whose
/// only remaining members can never run from an empty listing.
pub(super) fn group_members(pgid: i32) -> io::Result<Vec<(i32, ProcessProbe)>> {
    let mut capacity = 256usize;
    let pids = loop {
        let mut buffer = vec![0 as libc::pid_t; capacity];
        let bytes = i32::try_from(capacity * std::mem::size_of::<libc::pid_t>())
            .map_err(|_| io::Error::other("process group too large"))?;
        clear_errno();
        // SAFETY: `buffer` is a live pid_t array of exactly `bytes` bytes.
        let count = unsafe {
            libc::proc_listpgrppids(pgid, buffer.as_mut_ptr().cast::<libc::c_void>(), bytes)
        };
        // libproc reports some failures as a zero count; errno tells them
        // apart from an empty group. ESRCH means no process has this group
        // id any more: the group is empty (for example right after every
        // member of a killed group exited). Callers also cross-check an
        // empty listing against kill(-pgid, 0).
        let error = io::Error::last_os_error();
        if count <= 0 {
            match error.raw_os_error() {
                Some(libc::ESRCH) => break Vec::new(),
                Some(0) | None if count == 0 => break Vec::new(),
                _ => return Err(error),
            }
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
        match probe(pid)? {
            ProcessProbe::Absent => members.push((pid, ProcessProbe::Absent)),
            ProcessProbe::Foreign => members.push((pid, ProcessProbe::Foreign)),
            // proc_listpgrppids is a point-in-time listing; re-check.
            ProcessProbe::Observed(snapshot) if snapshot.pgid == pgid => {
                members.push((pid, ProcessProbe::Observed(snapshot)));
            }
            ProcessProbe::Observed(_) => {}
        }
    }
    Ok(members)
}

/// The current boot, from `kern.bootsessionuuid`. `None` when the sysctl is
/// unavailable; recovery then treats boot identity as unknown and relies on
/// per-process identity checks.
fn boot_session() -> Option<Uuid> {
    let name = c"kern.bootsessionuuid";
    let mut buffer = [0u8; 64];
    let mut length: libc::size_t = buffer.len();
    // SAFETY: `name` is NUL-terminated; `buffer` and `length` describe a live
    // writable region; no new value is set.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr().cast::<libc::c_void>(),
            &raw mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    let raw = buffer.get(..length.min(buffer.len()))?;
    let text = std::str::from_utf8(raw).ok()?.trim_end_matches('\0');
    Uuid::parse_str(text.trim()).ok()
}

/// macOS has no pid namespaces; start stamps are absolute wall-clock times.
pub(super) fn host_environment() -> io::Result<HostEnvironment> {
    Ok(HostEnvironment::Darwin {
        boot_session: boot_session(),
    })
}

/// Whether exactly this process is still running (ours, not a zombie).
pub(super) fn is_running(identity: &ProcessIdentity) -> io::Result<bool> {
    let BsdInfo::Ours(info) = bsd_info(identity.pid)? else {
        return Ok(false);
    };
    Ok(snapshot_from_info(&info)?.start == identity.start && info.pbi_status != libc::SZOMB)
}

/// kqueue `EVFILT_PROC` is always available on macOS.
#[allow(clippy::unnecessary_wraps)]
pub(super) fn exit_notification_available() -> io::Result<bool> {
    Ok(true)
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
    /// SIGKILL group `pgid`. macOS has no pidfds; the caller has just
    /// verified the group's ownership and registered an exit watch on each
    /// member. Returns how many watched members the kernel refused to let us
    /// signal: `killpg` fails with EPERM only when no member could be
    /// signalled.
    pub(super) fn kill(&self, pgid: i32) -> io::Result<usize> {
        match nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pgid),
            nix::sys::signal::Signal::SIGKILL,
        ) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(0),
            Err(nix::errno::Errno::EPERM) => Ok(self.pending.len()),
            Err(error) => Err(io::Error::from(error)),
        }
    }

    pub(super) fn new(members: &[ProcessIdentity]) -> io::Result<Self> {
        // SAFETY: kqueue() takes no arguments and returns a descriptor or -1.
        let raw = unsafe { libc::kqueue() };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` was just returned by kqueue() and has no other owner.
        let kqueue = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut pending = BTreeSet::new();
        for member in members {
            let pid = member.pid;
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
                match error.raw_os_error() {
                    // Already exited (zombies are not attachable), or not a
                    // process we may observe: either way not a live member.
                    Some(libc::ESRCH | libc::EPERM | libc::EACCES) => continue,
                    _ => return Err(error),
                }
            }
            // The registration pins the process it attached to. If the pid
            // was reused before registration, the recorded member is gone;
            // events for the stranger are ignored.
            if matches!(
                probe(pid)?,
                ProcessProbe::Observed(current) if current.start == member.start
            ) {
                pending.insert(pid);
            }
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
