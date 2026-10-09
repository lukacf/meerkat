//! Exclusive ownership of one native Unix child PID.
//!
//! This is the existing child owner shared by platform launch adapters. It has
//! no policy lookup or process-group supervisor. The caller owns containment.

#![allow(unsafe_code)]

use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Output};
use std::ptr;

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

/// Exclusive ownership of one native child and its optional standard pipes.
///
/// Do not reap this child through another process owner or install SIGCHLD
/// auto-reaping. Its unreaped PID is the identity used for direct-child signals.
/// Dropping a wait future keeps ownership here. Dropping the handle kills and
/// reaps the direct child; descendant containment remains the caller's job.
pub struct NativeChild {
    pid: Option<libc::pid_t>,
    status: Option<ExitStatus>,
    sigchld: Signal,
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
            .finish_non_exhaustive()
    }
}

impl NativeChild {
    // This constructor is infallible: acquisition of the exclusively owned PID
    // must be the first parent action after a successful OS spawn.
    pub(super) fn from_spawned_pid(
        pid: libc::pid_t,
        sigchld: Signal,
        stdin: Option<ChildStdin>,
        stdout: Option<ChildStdout>,
        stderr: Option<ChildStderr>,
    ) -> Self {
        Self {
            pid: Some(pid),
            status: None,
            sigchld,
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
        if let Some(pid) = self.pid {
            // SAFETY: Only this owner reaps the child, so its PID cannot be reused.
            if unsafe { libc::kill(pid, libc::SIGKILL) } == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    /// Request direct-child termination and reap it. Cancellation retains ownership.
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
        let Some(pid) = self.pid.take() else { return };
        // SAFETY: The PID remains exclusively owned and unreaped until transfer
        // to the cleanup waiter below. This signals only the direct child.
        unsafe { libc::kill(pid, libc::SIGKILL) };
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

fn reap(pid: libc::pid_t) {
    loop {
        // SAFETY: Drop transferred sole ownership of this unreaped child PID.
        let result = unsafe { libc::waitpid(pid, ptr::null_mut(), 0) };
        if result != -1 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            break;
        }
    }
}
