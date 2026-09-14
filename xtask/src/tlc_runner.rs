//! Shared, non-retryable wall-clock bound for every TLC subprocess.

use std::{
    fmt,
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

pub(crate) const TLC_WALL_LIMIT: Duration = Duration::from_secs(1200);

#[derive(Debug)]
pub(crate) struct TlcTimeout {
    pub(crate) limit: Duration,
    pub(crate) pid: u32,
    pub(crate) status: ExitStatus,
    pub(crate) stdout_path: PathBuf,
    pub(crate) stderr_path: PathBuf,
}

impl fmt::Display for TlcTimeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TLC timed out after {:?}; killed and reaped child {} ({}); \
             incomplete verification, NOT PASS; retained stdout={} stderr={}",
            self.limit,
            self.pid,
            self.status,
            self.stdout_path.display(),
            self.stderr_path.display(),
        )
    }
}

impl std::error::Error for TlcTimeout {}

#[derive(Debug)]
struct TlcCleanupFailure {
    pid: u32,
    status: ExitStatus,
}

impl fmt::Display for TlcCleanupFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TLC process-tree cleanup failed for {}: {}",
            self.pid, self.status
        )
    }
}

impl std::error::Error for TlcCleanupFailure {}

pub(crate) fn run_tlc(command: &mut Command, log_dir: &Path) -> Result<Output> {
    run_with_deadline(command, log_dir, TLC_WALL_LIMIT)
}

fn run_with_deadline(command: &mut Command, log_dir: &Path, limit: Duration) -> Result<Output> {
    if limit.is_zero() || limit > TLC_WALL_LIMIT {
        bail!("TLC wall-clock deadline must be greater than zero and at most 1200 seconds");
    }
    fs::create_dir_all(log_dir)
        .with_context(|| format!("create TLC log directory {}", log_dir.display()))?;
    let stdout_path = log_dir.join("tlc.stdout.log");
    let stderr_path = log_dir.join("tlc.stderr.log");
    let stdout = File::create(&stdout_path).context("create TLC stdout log")?;
    let stderr = File::create(&stderr_path).context("create TLC stderr log")?;
    command.stdin(Stdio::null()).stdout(stdout).stderr(stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // A shell launcher and its Java child share this owned process group.
        command.process_group(0);
    }
    let started = Instant::now();
    let mut child = command.spawn().context("spawn bounded TLC process")?;
    let pid = child.id();
    loop {
        if started.elapsed() >= limit {
            let status = kill_and_reap(&mut child).with_context(|| {
                format!(
                    "TLC deadline expired; cleanup failed; retained logs in {}",
                    log_dir.display()
                )
            })?;
            return Err(TlcTimeout {
                limit,
                pid,
                status,
                stdout_path,
                stderr_path,
            }
            .into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                #[cfg(unix)]
                {
                    let cleanup = kill_process_group(pid)?;
                    if cleanup.success() {
                        bail!(
                            "TLC launcher {pid} exited with surviving descendants; killed its \
                             process group; incomplete verification; retained logs in {}",
                            log_dir.display()
                        );
                    }
                    require_process_group_absent(pid, cleanup)
                        .with_context(|| format!("retained TLC logs in {}", log_dir.display()))?;
                }
                return Ok(Output {
                    status,
                    stdout: fs::read(&stdout_path).context("read TLC stdout log")?,
                    stderr: fs::read(&stderr_path).context("read TLC stderr log")?,
                });
            }
            Ok(None) => thread::sleep(
                Duration::from_millis(10).min(limit.saturating_sub(started.elapsed())),
            ),
            Err(error) => {
                kill_and_reap(&mut child).context("clean up TLC after wait failure")?;
                return Err(error).context("poll bounded TLC process");
            }
        }
    }
}

#[cfg(unix)]
fn kill_process_group(pid: u32) -> Result<ExitStatus> {
    Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("force-kill owned TLC process group")
}

#[cfg(unix)]
fn require_process_group_absent(pid: u32, status: ExitStatus) -> Result<()> {
    // kill's nonzero status alone cannot distinguish ESRCH from permission or
    // utility failure. Independently observe the exact owned group.
    let output = Command::new("/bin/ps")
        .args(["-A", "-o", "pgid="])
        .stdin(Stdio::null())
        .output()
        .context("inspect TLC process group after failed kill")?;
    if !output.status.success() {
        bail!(
            "cannot confirm absence of TLC process group {pid}: {}",
            output.status
        );
    }
    for group in String::from_utf8(output.stdout)
        .context("decode process groups")?
        .split_whitespace()
    {
        if group.parse::<u32>().context("parse process group id")? == pid {
            return Err(TlcCleanupFailure { pid, status }.into());
        }
    }
    Ok(())
}

fn kill_and_reap(child: &mut Child) -> Result<ExitStatus> {
    kill_and_reap_with(child, kill_tree)
}

fn kill_tree(pid: u32) -> Result<ExitStatus> {
    #[cfg(unix)]
    let result = kill_process_group(pid);
    #[cfg(windows)]
    let result = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("force-kill owned TLC process tree");
    #[cfg(not(any(unix, windows)))]
    let result = anyhow::bail!("TLC tree cleanup is unsupported on this target (pid {pid})");
    result
}

fn kill_and_reap_with(
    child: &mut Child,
    kill_tree: impl FnOnce(u32) -> Result<ExitStatus>,
) -> Result<ExitStatus> {
    let pid = child.id();
    let kill_result = kill_tree(pid);
    // Always reap our exact child, including a natural-exit race at the deadline.
    if !matches!(&kill_result, Ok(status) if status.success())
        && let Err(error) = child.kill()
        && child.try_wait().context("check TLC kill race")?.is_none()
    {
        return Err(error).context("kill TLC child after tree cleanup failure");
    }
    let status = child.wait().context("reap terminated TLC child")?;
    let cleanup_status = kill_result.context("TLC child reaped but process-tree cleanup failed")?;
    if !cleanup_status.success() {
        return Err(TlcCleanupFailure {
            pid,
            status: cleanup_status,
        }
        .into());
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tlc_deadline_rejects_zero_and_larger_limits_before_spawn() -> Result<()> {
        let dir = tempfile::tempdir()?;
        for limit in [Duration::ZERO, TLC_WALL_LIMIT + Duration::from_nanos(1)] {
            let error = run_with_deadline(
                &mut Command::new("must-not-spawn-invalid-tlc"),
                dir.path(),
                limit,
            )
            .err()
            .context("invalid deadline was accepted")?;
            assert!(error.to_string().contains("at most 1200"));
        }
        assert_eq!(TLC_WALL_LIMIT, Duration::from_secs(1200));
        assert_eq!(fs::read_dir(dir.path())?.count(), 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn tlc_deadline_preserves_success_failure_and_large_file_output() -> Result<()> {
        let dir = tempfile::tempdir()?;
        for code in [0, 7] {
            let mut command = Command::new("/bin/sh");
            command.args([
                "-c",
                &format!(
                    "i=0; while [ \"$i\" -lt 10000 ]; do printf 'output-line\\n'; \
                     i=$((i+1)); done; printf 'diagnostic\\n' >&2; exit {code}"
                ),
            ]);
            let output = run_tlc(&mut command, dir.path())?;
            assert_eq!(output.status.code(), Some(code));
            assert_eq!(output.stdout.len(), 120_000);
            assert_eq!(output.stderr, b"diagnostic\n");
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn tlc_deadline_kills_wrapper_and_child_retaining_logs_without_retry() -> Result<()> {
        use std::os::unix::process::ExitStatusExt as _;

        let dir = tempfile::tempdir()?;
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "printf 'one-attempt\\n'; printf 'before-timeout\\n' >&2; sleep 30 & echo $!; wait",
        ]);
        let started = Instant::now();
        let error = run_with_deadline(&mut command, dir.path(), Duration::from_millis(200))
            .err()
            .context("sleeping TLC did not time out")?;
        assert!(started.elapsed() < Duration::from_secs(5));
        let timeout = error
            .downcast_ref::<TlcTimeout>()
            .context("typed TLC timeout")?;
        assert_eq!(timeout.status.signal(), Some(9));
        assert_eq!(timeout.limit, Duration::from_millis(200));
        assert!(timeout.to_string().contains("NOT PASS"));
        let stdout = fs::read_to_string(&timeout.stdout_path)?;
        assert_eq!(stdout.matches("one-attempt").count(), 1);
        assert_eq!(
            fs::read_to_string(&timeout.stderr_path)?,
            "before-timeout\n"
        );
        let descendant = stdout.lines().nth(1).context("wrapper child pid")?;
        let output = Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", descendant])
            .output()?;
        let state = String::from_utf8(output.stdout)?;
        // The grandchild can be briefly zombie until the OS reaps it.
        assert!(
            state.trim().is_empty() || state.trim().starts_with('Z'),
            "{state}"
        );
        let parent = Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &timeout.pid.to_string()])
            .output()?;
        assert!(parent.stdout.is_empty(), "direct child was not reaped");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn tlc_deadline_error_prevents_postcheck_and_rejects_early_wrapper_exit() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut reached_postcheck = false;
        let result = (|| -> Result<()> {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "exec sleep 30"]);
            run_with_deadline(&mut command, dir.path(), Duration::from_millis(50))?;
            reached_postcheck = true;
            Ok(())
        })();
        assert!(
            result
                .err()
                .context("deadline did not refuse postcheck")?
                .is::<TlcTimeout>()
        );
        assert!(!reached_postcheck);

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & echo $!; exit 0"]);
        let error = run_tlc(&mut command, dir.path())
            .err()
            .context("orphaned verifier accepted")?;
        assert!(error.to_string().contains("surviving descendants"));
        let descendant = fs::read_to_string(dir.path().join("tlc.stdout.log"))?;
        let output = Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", descendant.trim()])
            .output()?;
        let state = String::from_utf8(output.stdout)?;
        assert!(
            state.trim().is_empty() || state.trim().starts_with('Z'),
            "{state}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn tlc_deadline_failed_tree_kill_is_typed_even_when_direct_child_is_reaped() -> Result<()> {
        use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
        let dir = tempfile::tempdir()?;
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exec sleep 30"])
            .process_group(0)
            .stdout(File::create(dir.path().join("stdout"))?)
            .stderr(File::create(dir.path().join("stderr"))?)
            .spawn()?;
        let pid = child.id();
        let failed_status = ExitStatus::from_raw(7 << 8);
        let observed = require_process_group_absent(pid, failed_status);
        let cleanup = kill_and_reap_with(&mut child, |_| Ok(failed_status));
        assert!(
            observed
                .err()
                .context("live group misclassified absent")?
                .is::<TlcCleanupFailure>()
        );
        assert!(
            cleanup
                .err()
                .context("nonzero cleanup accepted")?
                .is::<TlcCleanupFailure>()
        );
        let status = child.try_wait()?.context("direct child still running")?;
        assert_eq!(status.signal(), Some(9));
        require_process_group_absent(pid, failed_status)?;
        Ok(())
    }
}
