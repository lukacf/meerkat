//! Process boundary types for gateway binary communication.

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use std::io::{BufRead, BufReader};
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use std::process::{Child, Command, Stdio};
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use std::sync::mpsc;
use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessBoundaryError {
    SpawnFailed(String),
    MissingStdout,
    Io(String),
    Timeout {
        timeout_ms: u64,
    },
    EmptyOutput,
    InvalidJsonLine,
    /// The operating system did not confirm that the direct child was reaped.
    CleanupUnconfirmed {
        pid: u32,
        source: String,
    },
}

impl std::fmt::Display for ProcessBoundaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpawnFailed(msg) => write!(f, "spawn failed: {msg}"),
            Self::MissingStdout => write!(f, "missing stdout handle"),
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
            Self::Timeout { timeout_ms } => write!(f, "timed out after {timeout_ms}ms"),
            Self::EmptyOutput => write!(f, "empty output"),
            Self::InvalidJsonLine => write!(f, "invalid JSON line"),
            Self::CleanupUnconfirmed { pid, source } => {
                write!(f, "cleanup unconfirmed for process {pid}: {source}")
            }
        }
    }
}

impl std::error::Error for ProcessBoundaryError {}

/// Run a process and return its first JSON line after its direct child exits.
///
/// On Linux and macOS, one deadline covers stdout and child exit. Timeout or
/// read failure closes the owned reader, terminates the direct child, and waits
/// for reaping. Cleanup can exceed the operation deadline if termination fails.
/// No reader or exit-observer thread is detached. Descendants are not owned.
/// The caller must not install an external reaper for this owned child (including
/// SIGCHLD auto-reaping). An OS wait failure is `CleanupUnconfirmed`, not proof
/// of settlement; the value-only API cannot transfer a still-unconfirmed child.
///
/// Other targets, including Windows, retain the legacy first-read timeout and
/// blocking reader/exit behavior; they do not provide the Linux/macOS guarantee.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn run_process_json_line(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    timeout: Duration,
) -> Result<String, ProcessBoundaryError> {
    unix::run(command, args, env, timeout)
}

/// Legacy process boundary. Its timeout covers the first read only.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn run_process_json_line(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    timeout: Duration,
) -> Result<String, ProcessBoundaryError> {
    let mut child = Command::new(command)
        .args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| ProcessBoundaryError::SpawnFailed(err.to_string()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or(ProcessBoundaryError::MissingStdout)?;
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(|err| err.to_string());
        let _ = tx.send((read, line));
    });

    match rx.recv_timeout(timeout) {
        Ok((Ok(0), _)) => {
            wait_with_context(&mut child, "failed to wait for process after empty output")?;
            Err(ProcessBoundaryError::EmptyOutput)
        }
        Ok((Ok(_), mut line)) => {
            wait_with_context(
                &mut child,
                "failed to wait for process after reading output",
            )?;
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            if serde_json::from_str::<serde_json::Value>(&line).is_err() {
                return Err(ProcessBoundaryError::InvalidJsonLine);
            }
            Ok(line)
        }
        Ok((Err(err), _)) => {
            wait_with_context(
                &mut child,
                "failed to wait for process after stdout read failure",
            )?;
            Err(ProcessBoundaryError::Io(err))
        }
        Err(_) => {
            let timeout_ms = timeout.as_millis() as u64;
            cleanup_timeout_with_process(&mut child, timeout_ms)?;
            Err(ProcessBoundaryError::Timeout { timeout_ms })
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn wait_with_context(child: &mut Child, context: &str) -> Result<(), ProcessBoundaryError> {
    child
        .wait()
        .map(|_| ())
        .map_err(|err| ProcessBoundaryError::Io(format!("{context}: {err}")))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn cleanup_timeout_with_process(
    child: &mut Child,
    timeout_ms: u64,
) -> Result<(), ProcessBoundaryError> {
    match child.try_wait() {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {}
        Err(error) => {
            return Err(ProcessBoundaryError::Io(format!(
                "failed to probe process status after timeout({timeout_ms}ms): {error}"
            )));
        }
    }

    if let Err(kill_error) = child.kill() {
        return match child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(ProcessBoundaryError::Io(format!(
                "failed to kill process after timeout({timeout_ms}ms): {kill_error}"
            ))),
            Err(probe_error) => Err(ProcessBoundaryError::Io(format!(
                "failed to kill process after timeout({timeout_ms}ms): {kill_error}; failed to probe process status: {probe_error}"
            ))),
        };
    }

    child.wait().map(|_| ()).map_err(|error| {
        ProcessBoundaryError::Io(format!(
            "failed to wait for process after timeout kill({timeout_ms}ms): {error}"
        ))
    })
}

#[cfg(all(test, not(any(target_os = "linux", target_os = "macos"))))]
fn cleanup_timeout_with_ops<FTryWait, FKill, FWait>(
    timeout_ms: u64,
    mut try_wait: FTryWait,
    mut kill: FKill,
    mut wait: FWait,
) -> Result<(), ProcessBoundaryError>
where
    FTryWait: FnMut() -> std::io::Result<Option<()>>,
    FKill: FnMut() -> std::io::Result<()>,
    FWait: FnMut() -> std::io::Result<()>,
{
    match try_wait() {
        Ok(Some(())) => return Ok(()),
        Ok(None) => {}
        Err(error) => {
            return Err(ProcessBoundaryError::Io(format!(
                "failed to probe process status after timeout({timeout_ms}ms): {error}"
            )));
        }
    }

    if let Err(kill_error) = kill() {
        return match try_wait() {
            Ok(Some(())) => Ok(()),
            Ok(None) => Err(ProcessBoundaryError::Io(format!(
                "failed to kill process after timeout({timeout_ms}ms): {kill_error}"
            ))),
            Err(probe_error) => Err(ProcessBoundaryError::Io(format!(
                "failed to kill process after timeout({timeout_ms}ms): {kill_error}; failed to probe process status: {probe_error}"
            ))),
        };
    }

    wait().map_err(|error| {
        ProcessBoundaryError::Io(format!(
            "failed to wait for process after timeout kill({timeout_ms}ms): {error}"
        ))
    })
}

#[cfg(all(test, not(any(target_os = "linux", target_os = "macos"))))]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::io;

    use super::{ProcessBoundaryError, cleanup_timeout_with_ops};

    #[test]
    fn timeout_cleanup_handles_kill_race_without_type_drift() {
        let mut try_wait_results = vec![Ok(None), Ok(Some(()))].into_iter();
        let mut kill_attempts = 0;
        let result = cleanup_timeout_with_ops(
            25,
            || try_wait_results.next().expect("try_wait result"),
            || {
                kill_attempts += 1;
                Err(io::Error::new(io::ErrorKind::NotFound, "already exited"))
            },
            || panic!("wait must not run when process already exited"),
        );

        assert_eq!(kill_attempts, 1);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn timeout_cleanup_returns_io_on_fatal_kill_failure() {
        let mut try_wait_results = vec![Ok(None), Ok(None)].into_iter();
        let result = cleanup_timeout_with_ops(
            25,
            || try_wait_results.next().expect("try_wait result"),
            || {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "permission denied",
                ))
            },
            || Ok(()),
        );

        assert!(matches!(
            result,
            Err(ProcessBoundaryError::Io(message))
                if message.contains("failed to kill process after timeout(25ms)")
        ));
    }
}
