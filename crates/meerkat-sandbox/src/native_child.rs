//! Exclusive ownership of one native Unix child PID.
//!
//! This is the existing child owner shared by platform launch adapters. It has
//! no policy lookup or process-group supervisor. The caller owns containment.

#![allow(unsafe_code)]

use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Output};
use std::ptr;
#[cfg(target_os = "linux")]
use std::time::Duration;

use nix::libc;
use tokio::io::AsyncReadExt;
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};
use tokio::signal::unix::Signal;

/// The only descriptors a standalone confined child may inherit are stdio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdioMode {
    /// Read EOF or discard output through `/dev/null`.
    Null,
    /// Give the caller an asynchronous pipe endpoint.
    Piped,
    /// Duplicate the corresponding standard descriptor of the host.
    /// Linux Required confinement accepts anonymous pipes and `/dev/null`;
    /// directories, regular files, sockets and other devices are unsupported.
    Inherit,
}

/// Standard streams for a native launch. This does not change bound launch data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpawnIo {
    pub stdin: StdioMode,
    pub stdout: StdioMode,
    pub stderr: StdioMode,
}

impl Default for SpawnIo {
    fn default() -> Self {
        Self {
            stdin: StdioMode::Null,
            stdout: StdioMode::Piped,
            stderr: StdioMode::Piped,
        }
    }
}

/// How a [`NativeChild`] ends its direct child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Termination {
    /// SIGKILL the direct child. The macOS backend's launches.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Kill,
    /// The direct child is the Linux backend's relay, which on `signal`
    /// kills and reaps the pid namespace it launched and then exits. A relay
    /// that has not exited `bound` after the request gets SIGKILL, the last
    /// resort: its pid namespace then dies by init's parent-death signal,
    /// but init is left to whoever adopts it.
    #[cfg(target_os = "linux")]
    Teardown {
        signal: libc::c_int,
        bound: Duration,
    },
}

/// Exclusive ownership of one native child and its optional standard pipes.
///
/// Do not reap this child through another process owner or install SIGCHLD
/// auto-reaping. Its unreaped PID is the identity used for direct-child signals.
/// Dropping a wait future keeps ownership here. Dropping the handle ends
/// (see [`Termination`]) and reaps the direct child; descendant containment
/// remains the caller's job.
pub struct NativeChild {
    pid: Option<libc::pid_t>,
    status: Option<ExitStatus>,
    sigchld: Signal,
    termination: Termination,
    // A teardown is requested at most once; its watchdog holds the bound.
    #[cfg(target_os = "linux")]
    teardown_requested: bool,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

impl std::fmt::Debug for NativeChild {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeChild")
            .field("pid", &self.pid)
            .field("status", &self.status)
            .field("termination", &self.termination)
            .finish_non_exhaustive()
    }
}

impl NativeChild {
    // This constructor is infallible: acquisition of the exclusively owned PID
    // must be the first parent action after a successful OS spawn.
    pub(super) fn from_spawned_pid(
        pid: libc::pid_t,
        termination: Termination,
        sigchld: Signal,
        stdin: Option<ChildStdin>,
        stdout: Option<ChildStdout>,
        stderr: Option<ChildStderr>,
    ) -> Self {
        Self {
            pid: Some(pid),
            status: None,
            sigchld,
            termination,
            #[cfg(target_os = "linux")]
            teardown_requested: false,
            stdin,
            stdout,
            stderr,
        }
    }

    /// Returns the leader PID until this owner reaps it or loses wait ownership.
    #[must_use]
    pub fn id(&self) -> Option<u32> {
        self.pid.map(|pid| pid as u32)
    }

    /// Reap only this child if it exited; repeated calls return cached status.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        let pid = self
            .pid
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ECHILD))?;
        loop {
            let mut status = 0;
            // SAFETY: This handle exclusively owns this unreaped child PID.
            let result =
                unsafe { libc::waitpid(pid, std::ptr::addr_of_mut!(status), libc::WNOHANG) };
            if result == 0 {
                return Ok(None);
            }
            if result == pid {
                self.pid = None;
                self.status = Some(ExitStatus::from_raw(status));
                return Ok(self.status);
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.raw_os_error() == Some(libc::ECHILD) {
                // Another owner or auto-reaping invalidated our identity. Never
                // signal this numeric PID again after losing wait ownership.
                self.pid = None;
            }
            return Err(error);
        }
    }

    /// Close retained stdin and await exit without transferring PID ownership.
    /// Cancelling this future leaves the handle available for another wait/kill.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        drop(self.stdin.take());
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            self.sigchld
                .recv()
                .await
                .ok_or_else(|| io::Error::other("child exit notification closed"))?;
        }
    }

    /// Request direct-child termination. Reaping still requires wait or drop.
    pub fn start_kill(&mut self) -> io::Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        self.start_kill_without_reap()
    }

    /// Request direct-child termination without reaping an exited child, so
    /// its PID, and any process group it leads, stays held until wait or drop.
    /// A relay child gets its teardown request (see [`Termination`]), with
    /// SIGKILL after the bound if it has not exited by then.
    pub fn start_kill_without_reap(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        match self.pid {
            Some(pid) => self.terminate(pid),
            None => Ok(()),
        }
    }

    // The PID must be this owner's, unreaped.
    fn terminate(&mut self, pid: libc::pid_t) -> io::Result<()> {
        match self.termination {
            Termination::Kill => signal_owned(pid, libc::SIGKILL),
            #[cfg(target_os = "linux")]
            Termination::Teardown { signal, bound } => {
                if std::mem::replace(&mut self.teardown_requested, true) {
                    return Ok(());
                }
                request_teardown(pid, signal, bound)
            }
        }
    }

    /// Request direct-child termination and reap it: a relay's teardown
    /// reaps its launch before the relay exits. Cancellation retains ownership.
    pub async fn kill(&mut self) -> io::Result<()> {
        self.start_kill()?;
        self.wait().await.map(|_| ())
    }

    /// Close stdin, drain both output pipes concurrently, and reap the child.
    /// Cancelling this owning future drops the handle and triggers child cleanup.
    pub async fn wait_with_output(mut self) -> io::Result<Output> {
        drop(self.stdin.take());
        let stdout = self.stdout.take();
        let stderr = self.stderr.take();
        let read_stdout = async move {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = stdout {
                pipe.read_to_end(&mut bytes).await?;
            }
            Ok::<_, io::Error>(bytes)
        };
        let read_stderr = async move {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = stderr {
                pipe.read_to_end(&mut bytes).await?;
            }
            Ok::<_, io::Error>(bytes)
        };
        let (status, stdout, stderr) = tokio::try_join!(self.wait(), read_stdout, read_stderr)?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

impl Drop for NativeChild {
    fn drop(&mut self) {
        let _ = self.try_wait();
        let Some(pid) = self.pid else { return };
        // The PID remains exclusively owned and unreaped until transfer to
        // the cleanup waiter below. This signals only the direct child: a
        // relay is asked to tear down and reap its launch, and gets SIGKILL
        // from the teardown's watchdog if it has not exited within the bound.
        let _ = self.terminate(pid);
        self.pid = None;
        // Cleanup also works during Tokio shutdown. There is exactly one reaper
        // for this PID, with no wildcard wait or process-wide child registry.
        if std::thread::Builder::new()
            .name("meerkat-child-cleanup".to_owned())
            .spawn(move || reap(pid))
            .is_err()
        {
            // Resource exhaustion cannot discard wait ownership. This rare
            // fallback may block until the signalled direct child exits.
            reap(pid);
        }
    }
}

// Signals this owner's unreaped child; an exited one is not an error.
fn signal_owned(pid: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
    // SAFETY: Only this owner reaps the child, so its PID cannot be reused.
    if unsafe { libc::kill(pid, signal) } == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    Ok(())
}

// Sends the relay its teardown signal and arms a watchdog that SIGKILLs the
// relay if it has not exited `bound` later. The watchdog holds a pidfd, not
// the numeric PID, so it can never signal a reused PID after the owner
// reaps. Without a pidfd or a watchdog thread this falls back to SIGKILL now.
#[cfg(target_os = "linux")]
fn request_teardown(pid: libc::pid_t, signal: libc::c_int, bound: Duration) -> io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // SAFETY: The PID is this owner's, unreaped, so it names the relay.
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    let Ok(pidfd) = i32::try_from(pidfd) else {
        return signal_owned(pid, libc::SIGKILL);
    };
    if pidfd < 0 {
        return signal_owned(pid, libc::SIGKILL);
    }
    // SAFETY: pidfd_open returned this new descriptor, owned only here.
    let pidfd = unsafe { OwnedFd::from_raw_fd(pidfd) };
    if let Err(error) = signal_owned(pid, signal) {
        signal_owned(pid, libc::SIGKILL)?;
        return Err(error);
    }
    let deadline = std::time::Instant::now() + bound;
    let watchdog = move || {
        let mut exited = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            // Rounded up, so the wait never ends before the deadline.
            let left =
                libc::c_int::try_from(left.as_micros().div_ceil(1000)).unwrap_or(libc::c_int::MAX);
            // SAFETY: One pollfd over the descriptor this closure owns.
            let ready = unsafe { libc::poll(&raw mut exited, 1, left) };
            if ready == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if ready == 0 && std::time::Instant::now() < deadline {
                continue;
            }
            if ready == 0 {
                // SAFETY: A pidfd signal reaches only this process, or none
                // once it is reaped.
                unsafe {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        pidfd.as_raw_fd(),
                        libc::SIGKILL,
                        ptr::null::<libc::siginfo_t>(),
                        0,
                    )
                };
            }
            break;
        }
    };
    if std::thread::Builder::new()
        .name("meerkat-relay-teardown".to_owned())
        .spawn(watchdog)
        .is_err()
    {
        return signal_owned(pid, libc::SIGKILL);
    }
    Ok(())
}

fn reap(pid: libc::pid_t) {
    loop {
        // SAFETY: Drop transferred sole ownership of this unreaped child PID.
        let result = unsafe { libc::waitpid(pid, ptr::null_mut(), 0) };
        if result != -1 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            break;
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tokio::signal::unix::{SignalKind, signal};

    const TEARDOWN: libc::c_int = libc::SIGUSR2;
    const BOUND: Duration = Duration::from_millis(300);

    // A host child under a teardown-owning handle, once `ready` holds of it.
    fn owned(script: &str, ready: impl Fn(u32) -> bool) -> NativeChild {
        let sigchld = signal(SignalKind::child()).unwrap();
        #[allow(clippy::disallowed_methods)]
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", script])
            .spawn()
            .unwrap();
        let pid = child.id();
        drop(child);
        let owned = NativeChild::from_spawned_pid(
            pid as libc::pid_t,
            Termination::Teardown {
                signal: TEARDOWN,
                bound: BOUND,
            },
            sigchld,
            None,
            None,
            None,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready(pid) {
            assert!(Instant::now() < deadline, "the child never became ready");
            std::thread::sleep(Duration::from_millis(5));
        }
        owned
    }

    fn comm(pid: u32) -> String {
        std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default()
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// A relay is asked to tear down with its teardown signal, not killed:
    /// one that handles it ends the way it chooses, before the bound.
    #[test]
    fn a_teardown_owner_signals_the_relay_its_teardown_request() {
        runtime().block_on(async {
            let mut child = owned(
                &format!("trap 'exit 9' {TEARDOWN}; while :; do sleep 0.01; done"),
                |pid| {
                    comm(pid) == "sh\n"
                        && std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
                            .is_ok_and(|children| !children.trim().is_empty())
                },
            );
            child.kill().await.unwrap();
            assert_eq!(child.wait().await.unwrap().code(), Some(9));
        });
    }

    /// A relay that does not exit within the bound gets SIGKILL, from kill
    /// and from drop alike.
    #[test]
    fn a_relay_that_ignores_its_teardown_is_killed_after_the_bound() {
        let ignoring = format!("trap '' {TEARDOWN}; exec sleep 60");
        let slept = |pid| comm(pid) == "sleep\n";
        runtime().block_on(async {
            let mut child = owned(&ignoring, slept);
            let started = Instant::now();
            child.kill().await.unwrap();
            let elapsed = started.elapsed();
            assert!(elapsed >= BOUND, "{elapsed:?}");
            assert!(elapsed < BOUND + Duration::from_secs(2), "{elapsed:?}");
            assert_eq!(child.wait().await.unwrap().signal(), Some(libc::SIGKILL));

            let child = owned(&ignoring, slept);
            let pid = child.id().unwrap();
            drop(child);
            let deadline = Instant::now() + BOUND + Duration::from_secs(2);
            while std::path::Path::new(&format!("/proc/{pid}")).exists() {
                assert!(
                    Instant::now() < deadline,
                    "the dropped relay outlived the bound"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        });
    }
}
