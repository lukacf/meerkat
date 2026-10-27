//! One owner for a started module process.
//!
//! A module process is started for its first event line and, when healthy, kept
//! as the module's live child. Every path that stops owning it ends in this
//! owner: a failed start, a failed bootstrap after modules started, an aborted
//! respawn, shutdown and dropping the runtime handle. Releasing an owner whose
//! termination was not confirmed kills the direct child and waits for its exit,
//! so no module process is left running or unreaped, and no reaper thread is
//! detached. Descendants of the module are not owned.
//!
//! A process whose termination was not confirmed during shutdown or an aborted
//! respawn stays with its owner in [`RetainedModuleCleanup`] instead of being
//! released. Only the owner's successful `try_wait`/`wait` marks it reaped;
//! [`ModuleCleanupObservation`] projects these owners and is never settled
//! while one is retained.
//!
//! On Linux and macOS the first line is read on the calling thread from a
//! non-blocking stdout, bounded by the start timeout, and the read end is
//! closed on return. No reader thread exists, so none can stay blocked on a
//! pipe a descendant still holds. Other targets keep the legacy reader thread.

use std::io;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::process::ProcessBoundaryError;

/// Where retained module cleanup stands. Read-only projection of the process
/// owners below, never a replacement for their handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleCleanupPhase {
    /// A retained process has not been confirmed reaped yet.
    Pending,
    /// A kill or wait failed. Cleanup custody is still held; this is not
    /// terminal settlement.
    Failed,
    /// No process is retained: every one was confirmed reaped by its owner's
    /// successful `try_wait`/`wait`.
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleCleanupOperation {
    Kill,
    Wait,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleCleanupFailure {
    pub operation: ModuleCleanupOperation,
    pub message: String,
}

/// One retained, not yet confirmed module process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleCleanupProcessObservation {
    pub module_id: String,
    pub process_id: u32,
    /// A kill was requested and the signal call returned success.
    pub kill_sent: bool,
    /// The owner's `try_wait`/`wait` confirmed the exit.
    pub child_reaped: bool,
    /// A first-output reader may still hold the module's stdout. Always false
    /// on Linux and macOS, where no reader thread exists; true on other
    /// targets, whose legacy reader thread is never joined.
    pub reader_pending: bool,
    /// The most recent kill or wait failure, if any.
    pub failure: Option<ModuleCleanupFailure>,
}

/// A process whose exit the owner confirmed during this cleanup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleCleanupReapedProcess {
    pub module_id: String,
    pub process_id: u32,
}

/// Retained module cleanup as observed from its process owners. Direct
/// children only: descendants of a module are not owned or settled here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleCleanupObservation {
    pub phase: ModuleCleanupPhase,
    pub outstanding: Vec<ModuleCleanupProcessObservation>,
    pub reaped: Vec<ModuleCleanupReapedProcess>,
}

impl ModuleCleanupObservation {
    /// Nothing retained and nothing reaped.
    pub fn settled() -> Self {
        Self {
            phase: ModuleCleanupPhase::Settled,
            outstanding: Vec::new(),
            reaped: Vec::new(),
        }
    }
}

/// How a start attempt's first stdout line ended.
#[derive(Debug)]
pub(super) enum FirstLine {
    Line(String),
    /// Stdout reached end of file before a complete line.
    Closed,
    /// The start timeout elapsed first.
    TimedOut,
    ReadFailed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessState {
    Running,
    Reaped,
}

#[derive(Debug)]
pub(super) struct ModuleProcess {
    module_id: String,
    child: Child,
    state: ProcessState,
    kill_sent: bool,
    failure: Option<ModuleCleanupFailure>,
}

impl ModuleProcess {
    /// Spawn with piped stdout. The returned stdout is the module's only
    /// output channel; pass it to [`read_first_line`].
    pub(super) fn spawn(
        module_id: &str,
        mut command: Command,
    ) -> Result<(Self, ChildStdout), ProcessBoundaryError> {
        let child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| ProcessBoundaryError::SpawnFailed(err.to_string()))?;
        let mut process = Self {
            module_id: module_id.to_string(),
            child,
            state: ProcessState::Running,
            kill_sent: false,
            failure: None,
        };
        // `process` owns the child from here on, so an early return reaps it.
        let stdout = process
            .child
            .stdout
            .take()
            .ok_or(ProcessBoundaryError::MissingStdout)?;
        Ok((process, stdout))
    }

    /// Kill (unless already exited) and reap the direct child. On failure the
    /// process stays owned here and is killed and reaped when the owner is
    /// released.
    pub(super) fn terminate(&mut self, force_failure: bool) -> Result<(), String> {
        if self.state == ProcessState::Reaped {
            return Ok(());
        }
        if force_failure || test_seam::termination_fails() {
            let message = "forced terminate failure for testing".to_string();
            self.failure = Some(ModuleCleanupFailure {
                operation: ModuleCleanupOperation::Kill,
                message: message.clone(),
            });
            return Err(message);
        }
        let result = match self.child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => match self.child.kill() {
                Ok(()) => {
                    self.kill_sent = true;
                    self.child.wait().map(|_| ()).map_err(|err| {
                        (
                            ModuleCleanupOperation::Wait,
                            format!("wait after kill failed: {err}"),
                        )
                    })
                }
                Err(kill_err) => match self.child.try_wait() {
                    Ok(Some(_)) => Ok(()),
                    Ok(None) => Err((
                        ModuleCleanupOperation::Kill,
                        format!("kill failed while process still running: {kill_err}"),
                    )),
                    Err(probe_err) => Err((
                        ModuleCleanupOperation::Wait,
                        format!(
                            "kill failed and process status probe failed: {kill_err}; {probe_err}"
                        ),
                    )),
                },
            },
            Err(err) => Err((
                ModuleCleanupOperation::Wait,
                format!("try_wait failed: {err}"),
            )),
        };
        match result {
            Ok(()) => {
                self.state = ProcessState::Reaped;
                Ok(())
            }
            Err((operation, message)) => {
                self.failure = Some(ModuleCleanupFailure {
                    operation,
                    message: message.clone(),
                });
                Err(message)
            }
        }
    }

    /// One non-blocking cleanup step for a retained process: confirm an exit
    /// with `try_wait`, and request a kill once if it is still running. True
    /// only on a confirmed reap.
    pub(super) fn progress_cleanup(&mut self) -> bool {
        if self.state == ProcessState::Reaped {
            return true;
        }
        if test_seam::termination_fails() {
            return false;
        }
        match self.child.try_wait() {
            Ok(Some(_)) => {
                self.state = ProcessState::Reaped;
                true
            }
            Ok(None) => {
                if !self.kill_sent {
                    match self.child.kill() {
                        Ok(()) => self.kill_sent = true,
                        Err(err) => {
                            self.failure = Some(ModuleCleanupFailure {
                                operation: ModuleCleanupOperation::Kill,
                                message: format!("kill failed while process still running: {err}"),
                            });
                        }
                    }
                }
                false
            }
            Err(err) => {
                self.failure = Some(ModuleCleanupFailure {
                    operation: ModuleCleanupOperation::Wait,
                    message: format!("try_wait failed: {err}"),
                });
                false
            }
        }
    }

    pub(super) fn is_reaped(&self) -> bool {
        self.state == ProcessState::Reaped
    }

    pub(super) fn reaped(&self) -> ModuleCleanupReapedProcess {
        ModuleCleanupReapedProcess {
            module_id: self.module_id.clone(),
            process_id: self.child.id(),
        }
    }

    pub(super) fn observation(&self) -> ModuleCleanupProcessObservation {
        ModuleCleanupProcessObservation {
            module_id: self.module_id.clone(),
            process_id: self.child.id(),
            kill_sent: self.kill_sent,
            child_reaped: self.state == ProcessState::Reaped,
            reader_pending: !cfg!(any(target_os = "linux", target_os = "macos")),
            failure: self.failure.clone(),
        }
    }
}

/// Module processes whose termination was not confirmed, still owned here.
/// The same owners as the live children: nothing is dropped as if settled.
#[derive(Debug, Default)]
pub(super) struct RetainedModuleCleanup {
    processes: Vec<ModuleProcess>,
}

impl RetainedModuleCleanup {
    pub(super) fn retain(&mut self, process: ModuleProcess) {
        if !process.is_reaped() {
            self.processes.push(process);
        }
    }

    pub(super) fn observation(
        &self,
        reaped: Vec<ModuleCleanupReapedProcess>,
    ) -> ModuleCleanupObservation {
        let outstanding: Vec<_> = self
            .processes
            .iter()
            .map(ModuleProcess::observation)
            .collect();
        let phase = if outstanding.is_empty() {
            ModuleCleanupPhase::Settled
        } else if outstanding.iter().any(|process| process.failure.is_some()) {
            ModuleCleanupPhase::Failed
        } else {
            ModuleCleanupPhase::Pending
        };
        ModuleCleanupObservation {
            phase,
            outstanding,
            reaped,
        }
    }

    /// One non-blocking step for every retained process; confirmed reaps
    /// leave the retained set and are reported once.
    pub(super) fn progress(&mut self) -> ModuleCleanupObservation {
        let mut reaped = Vec::new();
        self.processes.retain_mut(|process| {
            if process.progress_cleanup() {
                reaped.push(process.reaped());
                false
            } else {
                true
            }
        });
        self.observation(reaped)
    }
}

/// What a failed release-time wait establishes about the direct child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReleaseWaitFailure {
    /// The platform reported no waitable child for this pid (ECHILD): it was
    /// already reaped, so dropping the handle releases no custody.
    NoWaitableChild,
    /// Any other failure: the child's exit is NOT confirmed.
    Unconfirmed,
}

/// Classify a release-time wait error by its actual OS error. Only ECHILD on
/// the platforms whose wait semantics are known here proves there is no
/// waitable child; everything else stays unconfirmed.
pub(super) fn classify_release_wait_error(error: &io::Error) -> ReleaseWaitFailure {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if error.raw_os_error() == Some(rustix::io::Errno::CHILD.raw_os_error()) {
        return ReleaseWaitFailure::NoWaitableChild;
    }
    let _ = error;
    ReleaseWaitFailure::Unconfirmed
}

impl Drop for ModuleProcess {
    fn drop(&mut self) {
        if self.state == ProcessState::Reaped {
            return;
        }
        // Last-resort release only: shutdown and respawn retain unconfirmed
        // processes in `RetainedModuleCleanup`, and the gateway's retained
        // shutdown continuation keeps the runtime (and so these owners) alive
        // until its shutdown completes. A kill error is kept for the record:
        // the wait below is what decides the outcome.
        let kill_error = self.child.kill().err();
        match self.child.wait() {
            Ok(_) => self.state = ProcessState::Reaped,
            Err(err) => match classify_release_wait_error(&err) {
                ReleaseWaitFailure::NoWaitableChild => tracing::warn!(
                    pid = self.child.id(),
                    module_id = %self.module_id,
                    error = %err,
                    kill_error = kill_error.as_ref().map(tracing::field::display),
                    "module process has no waitable child left (already reaped)"
                ),
                ReleaseWaitFailure::Unconfirmed => tracing::error!(
                    pid = self.child.id(),
                    module_id = %self.module_id,
                    error = %err,
                    kill_error = kill_error.as_ref().map(tracing::field::display),
                    "module process released with cleanup unconfirmed: its exit was not observed"
                ),
            },
        }
    }
}

/// Read one line within `timeout`. `stdout` is closed when this returns.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn read_first_line(stdout: ChildStdout, timeout: Duration) -> FirstLine {
    use std::io::Read;
    use std::time::Instant;

    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

    let mut stdout = stdout;
    if let Err(err) =
        fcntl_getfl(&stdout).and_then(|flags| fcntl_setfl(&stdout, flags | OFlags::NONBLOCK))
    {
        return FirstLine::ReadFailed(format!("failed to configure stdout: {err}"));
    }
    let started = Instant::now();
    let mut line = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let Some(remaining) = timeout
            .checked_sub(started.elapsed())
            .filter(|remaining| !remaining.is_zero())
        else {
            return FirstLine::TimedOut;
        };
        // macOS poll has a signed-millisecond ceiling; every pass still uses
        // the same start deadline.
        let wait = match Timespec::try_from(remaining.min(Duration::from_millis(i32::MAX as u64))) {
            Ok(wait) => wait,
            Err(err) => return FirstLine::ReadFailed(err.to_string()),
        };
        let mut fds = [PollFd::new(&stdout, PollFlags::IN)];
        match poll(&mut fds, Some(&wait)) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(err) => return FirstLine::ReadFailed(err.to_string()),
        }
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => return FirstLine::Closed,
                Ok(read) => {
                    let chunk = &chunk[..read];
                    if let Some(end) = chunk.iter().position(|byte| *byte == b'\n') {
                        line.extend_from_slice(&chunk[..=end]);
                        return decode_line(line);
                    }
                    line.extend_from_slice(chunk);
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return FirstLine::ReadFailed(err.to_string()),
            }
        }
    }
}

/// Legacy reader thread. It is not joined: a descendant holding stdout can
/// keep it blocked after the module is reaped.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn read_first_line(stdout: ChildStdout, timeout: Duration) -> FirstLine {
    use std::io::{BufRead, BufReader};

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let result = reader.read_line(&mut line).map_err(|err| err.to_string());
        let _ = tx.send((result, line));
    });
    match rx.recv_timeout(timeout) {
        Ok((Ok(0), _)) => FirstLine::Closed,
        Ok((Ok(_), line)) => FirstLine::Line(line),
        Ok((Err(err), _)) => FirstLine::ReadFailed(err),
        Err(_) => FirstLine::TimedOut,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn decode_line(bytes: Vec<u8>) -> FirstLine {
    match String::from_utf8(bytes) {
        Ok(line) => FirstLine::Line(line),
        Err(err) => FirstLine::ReadFailed(err.to_string()),
    }
}

/// Crate-internal fault injection for terminate paths that the public
/// `supervisor_test_force_terminate_failure` option does not reach.
#[cfg(test)]
pub(super) mod test_seam {
    use std::cell::Cell;

    thread_local! {
        static FAIL_TERMINATIONS: Cell<bool> = const { Cell::new(false) };
    }

    pub(in crate::runtime) fn termination_fails() -> bool {
        FAIL_TERMINATIONS.with(Cell::get)
    }

    /// Make every termination on this thread fail until the guard drops.
    pub(in crate::runtime) fn fail_terminations() -> FailTerminations {
        FAIL_TERMINATIONS.with(|value| value.set(true));
        FailTerminations
    }

    pub(in crate::runtime) struct FailTerminations;

    impl Drop for FailTerminations {
        fn drop(&mut self) {
            FAIL_TERMINATIONS.with(|value| value.set(false));
        }
    }
}

#[cfg(not(test))]
mod test_seam {
    pub(super) fn termination_fails() -> bool {
        false
    }
}
