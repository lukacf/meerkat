//! Linux/macOS one-shot boundary. The caller is the only child reaper.

use std::io::{self, Read};
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};

use super::ProcessBoundaryError;

type ExitObserver = JoinHandle<io::Result<()>>;

pub(super) fn run(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    timeout: Duration,
) -> Result<String, ProcessBoundaryError> {
    let started = Instant::now();
    let child = Command::new(command)
        .args(args)
        .envs(env.iter().map(|(key, value)| (key, value)))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| ProcessBoundaryError::SpawnFailed(error.to_string()))?;
    run_child(child, started, timeout)
}

fn run_child(
    mut child: Child,
    started: Instant,
    timeout: Duration,
) -> Result<String, ProcessBoundaryError> {
    #[cfg(test)]
    OPERATION_STARTED.with(|value| value.set(Some(started)));
    let Some(mut stdout) = child.stdout.take() else {
        settle_child(&mut child, None, true)?;
        return Err(ProcessBoundaryError::MissingStdout);
    };
    if let Err(error) = set_nonblocking(&stdout) {
        drop(stdout);
        settle_child(&mut child, None, true)?;
        return Err(ProcessBoundaryError::Io(format!(
            "failed to configure stdout: {error}"
        )));
    }
    let (exit_notification, observer) = match observe_exit(Pid::from_child(&child)) {
        Ok(observer) => observer,
        Err(error) => {
            drop(stdout);
            settle_child(&mut child, None, true)?;
            return Err(ProcessBoundaryError::Io(format!(
                "failed to observe child exit: {error}"
            )));
        }
    };
    let mut observer = Some(observer);
    let mut exited = false;
    let result = read_until_exit(
        &mut stdout,
        &exit_notification,
        &mut observer,
        &mut exited,
        started,
        timeout,
    );
    // A descendant can retain its write end. Closing this exact reader does not
    // require descendant cooperation and leaves no blocked reader thread.
    drop(stdout);
    drop(exit_notification);
    settle_child(&mut child, observer, !exited)?;
    let bytes = result?;
    if bytes.is_empty() {
        return Err(ProcessBoundaryError::EmptyOutput);
    }
    let mut line =
        String::from_utf8(bytes).map_err(|error| ProcessBoundaryError::Io(error.to_string()))?;
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    serde_json::from_str::<serde_json::Value>(&line)
        .map_err(|_| ProcessBoundaryError::InvalidJsonLine)?;
    Ok(line)
}

fn set_nonblocking(stdout: &ChildStdout) -> io::Result<()> {
    let flags = fcntl_getfl(stdout)?;
    fcntl_setfl(stdout, flags | OFlags::NONBLOCK)?;
    Ok(())
}

fn observe_exit(pid: Pid) -> io::Result<(UnixStream, ExitObserver)> {
    // This pair is internal notification only. The child's stdout remains a
    // pipe. Closing the observer's endpoint wakes poll even on observer error;
    // there is no notification write that can block behind a full buffer.
    let (notification, notifier) = UnixStream::pair()?;
    let observer = std::thread::Builder::new()
        .name("meta-process-exit".to_owned())
        .spawn(move || {
            let result = loop {
                match waitid(
                    WaitId::Pid(pid),
                    WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
                ) {
                    Ok(Some(_)) => break Ok(()),
                    Ok(None) => {
                        break Err(io::Error::other("blocking waitid returned no exit status"));
                    }
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(error) => break Err(error.into()),
                }
            };
            // NOWAIT leaves the child waitable and reserves its PID until the
            // caller alone reaps it. Notification never transfers that owner.
            drop(notifier);
            result
        })?;
    Ok((notification, observer))
}

fn join_observer(observer: ExitObserver) -> io::Result<()> {
    observer
        .join()
        .map_err(|_| io::Error::other("child-exit observer panicked"))?
}

fn read_until_exit(
    stdout: &mut ChildStdout,
    exit_notification: &UnixStream,
    observer: &mut Option<ExitObserver>,
    exited: &mut bool,
    started: Instant,
    timeout: Duration,
) -> Result<Vec<u8>, ProcessBoundaryError> {
    let mut first_line = Vec::new();
    let mut line_complete = false;
    let mut stdout_eof = false;
    loop {
        let observed = Instant::now();
        let Some(remaining) = timeout.checked_sub(observed.duration_since(started)) else {
            return Err(timeout_error(timeout));
        };
        if remaining.is_zero() {
            return Err(timeout_error(timeout));
        }
        // Completion is an observation after the final read/join, not merely
        // readiness before that work. It must fit the same original budget.
        if line_complete && *exited {
            return Ok(first_line);
        }
        // macOS poll has a signed-millisecond ceiling. Long waits can be split
        // only at that OS limit; each pass still uses the same caller deadline.
        let wait = remaining.min(Duration::from_millis(i32::MAX as u64));
        #[cfg(test)]
        POLL_BUDGETS.with_borrow_mut(|budgets| {
            budgets.push(PollBudget {
                observed,
                wait,
                after_first_line: line_complete,
            });
        });
        let wait = Timespec::try_from(wait)
            .map_err(|error| ProcessBoundaryError::Io(error.to_string()))?;
        let mut fds = Vec::with_capacity(2);
        if !stdout_eof {
            fds.push(PollFd::new(stdout, PollFlags::IN));
        }
        if !*exited {
            fds.push(PollFd::new(exit_notification, PollFlags::IN));
        }
        match poll(&mut fds, Some(&wait)) {
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(ProcessBoundaryError::Io(error.to_string())),
        }
        let stdout_ready = !stdout_eof && !fds[0].revents().is_empty();
        let exit_ready = !*exited && fds.last().is_some_and(|fd| !fd.revents().is_empty());
        drop(fds);
        // A timed-out poll or a late event cannot create a fresh read/exit
        // budget. Even an already-exited child remains unreaped at this point.
        if started.elapsed() >= timeout {
            return Err(timeout_error(timeout));
        }
        if exit_ready {
            if let Some(observer) = observer.take() {
                join_observer(observer)
                    .map_err(|error| ProcessBoundaryError::Io(error.to_string()))?;
            }
            *exited = true;
        }
        if stdout_ready {
            let mut chunk = [0_u8; 8192];
            // One bounded read per pass checks the deadline even when a child
            // writes indefinitely. Continue draining after retaining line one.
            match stdout.read(&mut chunk) {
                Ok(0) => {
                    stdout_eof = true;
                    line_complete = true;
                }
                Ok(length) => {
                    if !line_complete {
                        let bytes = &chunk[..length];
                        if let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
                            first_line.extend_from_slice(&bytes[..=end]);
                            line_complete = true;
                        } else {
                            first_line.extend_from_slice(bytes);
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(ProcessBoundaryError::Io(error.to_string())),
            }
        }
        #[cfg(test)]
        if line_complete && *exited {
            pause_completed_observation(started, timeout);
        }
    }
}

fn timeout_error(timeout: Duration) -> ProcessBoundaryError {
    ProcessBoundaryError::Timeout {
        timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
    }
}

fn settle_child(
    child: &mut Child,
    observer: Option<ExitObserver>,
    terminate: bool,
) -> Result<(), ProcessBoundaryError> {
    let pid = child.id();
    let kill_error = if terminate {
        #[cfg(test)]
        record_action(pid, "kill");
        child.kill().err()
    } else {
        None
    };
    // Complete the non-reaping observation before the sole physical wait. If
    // kill failed while the child is alive, this retains both owners until
    // natural exit. The operation deadline does not bound OS cleanup.
    let observer_error = observer.and_then(|observer| {
        let result = join_observer(observer);
        #[cfg(test)]
        record_action(pid, "observer_join");
        result.err()
    });
    #[cfg(test)]
    record_action(pid, "wait");
    match child.wait() {
        Ok(_) => {
            #[cfg(test)]
            record_action(pid, "reaped");
            if kill_error.is_some() || observer_error.is_some() {
                tracing::warn!(
                    pid,
                    ?kill_error,
                    ?observer_error,
                    "child reaped after cleanup error"
                );
            }
            Ok(())
        }
        Err(error) => {
            let source = format!(
                "wait failed: {error}; kill error: {kill_error:?}; observer error: {observer_error:?}"
            );
            tracing::error!(pid, %source, "process cleanup unconfirmed");
            Err(ProcessBoundaryError::CleanupUnconfirmed { pid, source })
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
struct PollBudget {
    observed: Instant,
    wait: Duration,
    after_first_line: bool,
}

#[cfg(test)]
thread_local! {
    static OPERATION_STARTED: std::cell::Cell<Option<Instant>> = const {
        std::cell::Cell::new(None)
    };
    static POLL_BUDGETS: std::cell::RefCell<Vec<PollBudget>> = const {
        std::cell::RefCell::new(Vec::new())
    };
    static COMPLETION_GATE: std::cell::RefCell<Option<(
        std::sync::mpsc::Sender<Instant>,
        std::sync::mpsc::Receiver<()>,
    )>> = const { std::cell::RefCell::new(None) };
    static ACTIONS: std::cell::RefCell<Vec<(u32, &'static str)>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

#[cfg(test)]
fn pause_completed_observation(started: Instant, timeout: Duration) {
    let gate = COMPLETION_GATE.with_borrow_mut(Option::take);
    if let Some((entered, release)) = gate {
        if let Some(deadline) = started.checked_add(timeout) {
            let _ = entered.send(deadline);
        }
        let _ = release.recv();
    }
}

#[cfg(test)]
fn record_action(pid: u32, action: &'static str) {
    ACTIONS.with_borrow_mut(|actions| actions.push((pid, action)));
}

#[cfg(test)]
#[path = "deadline_tests.rs"]
mod tests;
