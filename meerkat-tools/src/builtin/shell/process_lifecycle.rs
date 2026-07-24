#[cfg(unix)]
use std::sync::Arc;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;
use tokio::process::Child;
use tokio::task::JoinHandle;
use tracing::warn;

#[cfg(unix)]
const TERM_GRACE: Duration = Duration::from_secs(2);
#[cfg(unix)]
const GROUP_POLL_INTERVAL: Duration = Duration::from_millis(20);
#[cfg(unix)]
const KILL_SETTLE_TIMEOUT: Duration = Duration::from_millis(500);
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

/// Ownership guard for the process group created for one shell invocation.
///
/// The group identity is captured before the leader can be reaped because
/// `Child::id()` becomes unavailable after completion. Keeping the guard armed
/// across every await ensures task cancellation force-kills the complete group.
pub(super) struct OwnedProcessGroup {
    #[cfg(unix)]
    pgid: Option<i32>,
    #[cfg(unix)]
    control: Arc<dyn ProcessGroupControl>,
    #[cfg(unix)]
    term_grace: Duration,
    #[cfg(unix)]
    poll_interval: Duration,
    #[cfg(unix)]
    kill_settle_timeout: Duration,
    #[cfg(unix)]
    kill_already_requested: bool,
}

impl OwnedProcessGroup {
    pub(super) fn new(child: &Child) -> Self {
        Self {
            #[cfg(unix)]
            pgid: child.id().map(|pid| pid as i32),
            #[cfg(unix)]
            control: Arc::new(UnixProcessGroupControl),
            #[cfg(unix)]
            term_grace: TERM_GRACE,
            #[cfg(unix)]
            poll_interval: GROUP_POLL_INTERVAL,
            #[cfg(unix)]
            kill_settle_timeout: KILL_SETTLE_TIMEOUT,
            #[cfg(unix)]
            kill_already_requested: false,
        }
    }

    #[cfg(all(unix, test))]
    pub(super) fn with_control(
        child: &Child,
        control: Arc<dyn ProcessGroupControl>,
        term_grace: Duration,
        kill_settle_timeout: Duration,
    ) -> Self {
        Self {
            pgid: child.id().map(|pid| pid as i32),
            control,
            term_grace,
            poll_interval: Duration::from_millis(1),
            kill_settle_timeout,
            kill_already_requested: false,
        }
    }

    /// TERM the owned group, allow the fixed grace window, then KILL any
    /// survivors. The leader may already have exited; group ownership remains
    /// valid while descendants retain the process-group id.
    #[cfg(unix)]
    pub(super) async fn terminate(&mut self, child: &mut Child) -> std::io::Result<()> {
        let Some(pgid) = self.pgid else {
            return Ok(());
        };
        match self.control.exists(pgid) {
            Ok(false) => {
                self.disarm();
                return Ok(());
            }
            Ok(true) => {}
            Err(error)
                if self.kill_already_requested
                    && error.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                // Drop-time cancellation already delivered an accepted
                // SIGKILL fence to this owned group. On macOS, probing that
                // numeric group after its leader exits can return EPERM rather
                // than ESRCH once the identity is no longer signalable as our
                // group. Reap the child and retire the stale identity; EPERM
                // without the prior accepted fence remains a hard failure.
                let _ = tokio::time::timeout(self.kill_settle_timeout, child.wait()).await;
                self.disarm();
                return Ok(());
            }
            Err(error) => return Err(error),
        }

        if !self.kill_already_requested {
            if !self.control.signal(pgid, ProcessGroupSignal::Term)? {
                self.disarm();
                return Ok(());
            }
            let deadline = Instant::now() + self.term_grace;
            while Instant::now() < deadline {
                // Reap the leader as soon as it exits so its zombie does not make
                // an otherwise-empty process group appear live for the full grace.
                let _ = child.try_wait()?;
                if !self.control.exists(pgid)? {
                    self.disarm();
                    return Ok(());
                }
                tokio::time::sleep(self.poll_interval).await;
            }
        }

        // Do this even when the group leader exited during the grace period:
        // descendants can ignore TERM and continue to own inherited pipes.
        if !self.control.signal(pgid, ProcessGroupSignal::Kill)? {
            self.disarm();
            return Ok(());
        }
        self.kill_already_requested = true;
        let _ = tokio::time::timeout(self.kill_settle_timeout, child.wait()).await;

        let settle_deadline = Instant::now() + self.kill_settle_timeout;
        while Instant::now() < settle_deadline {
            if !self.control.exists(pgid)? {
                self.disarm();
                return Ok(());
            }
            tokio::time::sleep(self.poll_interval).await;
        }
        if self.control.exists(pgid)? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("process group {pgid} survived TERM/KILL containment"),
            ));
        }
        self.disarm();
        Ok(())
    }

    #[cfg(not(unix))]
    pub(super) async fn terminate(&mut self, child: &mut Child) -> std::io::Result<()> {
        child.kill().await
    }

    /// Best-effort synchronous fencing for cancellation during an admission
    /// future's drop path, where awaiting graceful acknowledgement is
    /// impossible.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn force_kill_now(&mut self) {
        #[cfg(unix)]
        {
            if let Some(pgid) = self.pgid {
                match self.control.signal(pgid, ProcessGroupSignal::Kill) {
                    Ok(true) => self.kill_already_requested = true,
                    Ok(false) => self.disarm(),
                    Err(_) => {}
                }
            }
        }
    }

    pub(super) fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.pgid = None;
            self.kill_already_requested = false;
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Copy)]
pub(super) enum ProcessGroupSignal {
    Term,
    Kill,
}

#[cfg(unix)]
pub(super) trait ProcessGroupControl: Send + Sync {
    /// Returns false only when the kernel proves the group no longer exists.
    fn signal(&self, pgid: i32, signal: ProcessGroupSignal) -> std::io::Result<bool>;
    fn exists(&self, pgid: i32) -> std::io::Result<bool>;
}

#[cfg(unix)]
struct UnixProcessGroupControl;

#[cfg(unix)]
impl ProcessGroupControl for UnixProcessGroupControl {
    fn signal(&self, pgid: i32, signal: ProcessGroupSignal) -> std::io::Result<bool> {
        use nix::errno::Errno;
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;

        let signal = match signal {
            ProcessGroupSignal::Term => Signal::SIGTERM,
            ProcessGroupSignal::Kill => Signal::SIGKILL,
        };
        match killpg(Pid::from_raw(pgid), signal) {
            Ok(()) => Ok(true),
            Err(Errno::ESRCH) => Ok(false),
            Err(error) => Err(std::io::Error::from_raw_os_error(error as i32)),
        }
    }

    fn exists(&self, pgid: i32) -> std::io::Result<bool> {
        process_group_exists(pgid)
    }
}

#[cfg(unix)]
fn process_group_exists(pgid: i32) -> std::io::Result<bool> {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    match kill(Pid::from_raw(-pgid), None) {
        Ok(()) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(error) => Err(std::io::Error::from_raw_os_error(error as i32)),
    }
}

#[cfg(unix)]
impl Drop for OwnedProcessGroup {
    fn drop(&mut self) {
        if let Some(pgid) = self.pgid.take() {
            let _ = self.control.signal(pgid, ProcessGroupSignal::Kill);
        }
    }
}

/// Drain a reader task without allowing a leaked/escaped writer to hold the
/// shell waiter forever.
pub(super) async fn join_output_bounded(
    mut handle: JoinHandle<std::io::Result<Vec<u8>>>,
    label: &str,
) -> Vec<u8> {
    match tokio::time::timeout(OUTPUT_DRAIN_TIMEOUT, &mut handle).await {
        Ok(Ok(Ok(buf))) => buf,
        Ok(Ok(Err(error))) => {
            warn!("Failed to read {}: {}", label, error);
            Vec::new()
        }
        Ok(Err(error)) => {
            warn!("{} reader task failed: {}", label, error);
            Vec::new()
        }
        Err(_) => {
            handle.abort();
            let _ = handle.await;
            warn!("{} reader task exceeded bounded drain timeout", label);
            Vec::new()
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::process::Command;

    struct FailingControl {
        fail_probe: bool,
        fail_signal: bool,
        always_exists: bool,
        signals: AtomicUsize,
    }

    struct PermissionDeniedAfterKillFenceControl {
        signals: AtomicUsize,
    }

    impl ProcessGroupControl for PermissionDeniedAfterKillFenceControl {
        fn signal(&self, _pgid: i32, _signal: ProcessGroupSignal) -> std::io::Result<bool> {
            self.signals.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }

        fn exists(&self, _pgid: i32) -> std::io::Result<bool> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "stale process-group identity",
            ))
        }
    }

    impl ProcessGroupControl for FailingControl {
        fn signal(&self, _pgid: i32, _signal: ProcessGroupSignal) -> std::io::Result<bool> {
            self.signals.fetch_add(1, Ordering::SeqCst);
            if self.fail_signal {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected signal failure",
                ))
            } else {
                Ok(true)
            }
        }

        fn exists(&self, _pgid: i32) -> std::io::Result<bool> {
            if self.fail_probe {
                Err(std::io::Error::other("injected probe failure"))
            } else {
                Ok(self.always_exists)
            }
        }
    }

    async fn short_lived_child() -> Child {
        Command::new("/usr/bin/true")
            .spawn()
            .expect("spawn short-lived child")
    }

    #[tokio::test]
    async fn terminate_stays_armed_when_group_probe_fails() {
        let mut child = short_lived_child().await;
        let control = Arc::new(FailingControl {
            fail_probe: true,
            fail_signal: false,
            always_exists: true,
            signals: AtomicUsize::new(0),
        });
        let mut group =
            OwnedProcessGroup::with_control(&child, control, Duration::ZERO, Duration::ZERO);

        assert!(group.terminate(&mut child).await.is_err());
        assert!(
            group.pgid.is_some(),
            "probe failure must leave the guard armed"
        );
    }

    #[tokio::test]
    async fn terminate_stays_armed_when_group_signal_fails() {
        let mut child = short_lived_child().await;
        let control = Arc::new(FailingControl {
            fail_probe: false,
            fail_signal: true,
            always_exists: true,
            signals: AtomicUsize::new(0),
        });
        let mut group =
            OwnedProcessGroup::with_control(&child, control, Duration::ZERO, Duration::ZERO);

        assert!(group.terminate(&mut child).await.is_err());
        assert!(
            group.pgid.is_some(),
            "signal failure must leave the guard armed"
        );
    }

    #[tokio::test]
    async fn accepted_drop_kill_fence_allows_stale_permission_denied_identity_to_retire() {
        let mut child = short_lived_child().await;
        let control = Arc::new(PermissionDeniedAfterKillFenceControl {
            signals: AtomicUsize::new(0),
        });
        let mut group = OwnedProcessGroup::with_control(
            &child,
            control.clone(),
            Duration::ZERO,
            Duration::ZERO,
        );

        group.force_kill_now();
        group
            .terminate(&mut child)
            .await
            .expect("accepted kill fence must retire a stale unsignalable group identity");

        assert!(group.pgid.is_none());
        assert_eq!(
            control.signals.load(Ordering::SeqCst),
            1,
            "the accepted drop-time kill is the required ordering witness"
        );
    }

    #[tokio::test]
    async fn terminate_fails_and_stays_armed_while_group_survives_kill() {
        let mut child = short_lived_child().await;
        let control = Arc::new(FailingControl {
            fail_probe: false,
            fail_signal: false,
            always_exists: true,
            signals: AtomicUsize::new(0),
        });
        let mut group = OwnedProcessGroup::with_control(
            &child,
            control.clone(),
            Duration::ZERO,
            Duration::ZERO,
        );

        let error = group
            .terminate(&mut child)
            .await
            .expect_err("a surviving group is not contained");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            group.pgid.is_some(),
            "surviving group must leave the guard armed"
        );
        assert!(
            control.signals.load(Ordering::SeqCst) >= 2,
            "TERM and KILL must both be attempted"
        );
    }
}
