//! Operator termination signal for long-lived Meta binaries.
//!
//! # Why this module exists
//!
//! [`tokio::signal::ctrl_c`] listens for **SIGINT and nothing else**. Both
//! gateway binaries waited on it alone, which is correct for an interactive
//! ctrl-c and wrong for every other way a long-lived process is stopped:
//! Kubernetes, systemd and `docker stop` all send **SIGTERM** first and
//! escalate to SIGKILL only after a grace period. SIGTERM's default
//! disposition terminates the process immediately, so a `ctrl_c`-only wait
//! means the graceful shutdown path never runs on an ordinary deploy.
//!
//! That was survivable until meerkat 0.8.22 introduced the schedule executor
//! lease, which made an ungraceful exit *cost* something durable. The lease is
//! released only on the graceful path (`ScheduleHostHandle::shutdown`). Skip
//! it and the row keeps a future `expires_at_utc`, so the replacement process
//! gets `AcquireScheduleExecutorLeaseOutcome::Busy`, its tick returns without
//! calling `claim_due_occurrences`, and **schedules do not fire for up to
//! `lease_duration` (60s by default) after every restart**. The claim watchdog
//! cannot see it either: its overdue threshold is 2 minutes, longer than the
//! window it would need to observe.
//!
//! So this is not a tidiness fix. On a container platform the pre-0.8.22
//! behaviour was "an ungraceful stop loses nothing"; after 0.8.22 it is "every
//! deploy silently stops firing schedules for a minute".
//!
//! # Contract
//!
//! Resolves on the FIRST of SIGINT or SIGTERM and reports which one arrived as
//! a [`ShutdownSignal`]. Both mean "an operator or supervisor is stopping this
//! process, run the shutdown sequence", and no caller behaves differently on
//! the two; the name exists for the exit log line. A gateway that vanished
//! used to leave no trace of why, and a supervisor's SIGTERM after a slow
//! console operation was indistinguishable from a crash. Callers keep owning
//! what shutdown means; this only decides when, and says what asked.
//!
//! If the SIGTERM handler cannot be installed the function degrades to SIGINT
//! only rather than failing the process. A binary that refuses to start
//! because it could not register a signal handler is strictly worse than one
//! that starts and handles fewer signals.
//!
//! # One listener per wait loop
//!
//! tokio delivers a signal only to the listeners that exist when it
//! delivers it: a listener created afterwards never sees it. Once any
//! listener has been created, tokio's handler stays installed for the rest
//! of the process, so the signal's default action no longer ends the process
//! either. A loop that creates a listener for each wait therefore loses every
//! signal that lands between two waits, and the process runs on until it is
//! killed. A loop that waits for the signal again and again creates one
//! crate-internal `ShutdownSignals` before it starts and calls its `recv`
//! for each wait. [`shutdown_signal`] is for a single wait.

/// Which termination signal ended the wait in [`shutdown_signal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownSignal {
    /// SIGINT: an interactive ctrl-c, or `kill -INT`.
    Interrupt,
    /// SIGTERM: the stop signal every container platform and init system
    /// sends first.
    Terminate,
}

impl ShutdownSignal {
    /// The conventional signal name, as `kill -l` prints it. This is the
    /// token the gateway exit log line carries in its `signal=` field.
    pub fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
        }
    }
}

impl std::fmt::Display for ShutdownSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// SIGINT and SIGTERM listeners, registered when [`ShutdownSignals::listen`]
/// returns and kept until the value is dropped.
///
/// A signal delivered while no [`ShutdownSignals::recv`] is waiting is kept
/// for the next one, so a loop that holds one value across its iterations
/// loses no signal between two waits (see the module docs).
pub(crate) struct ShutdownSignals {
    #[cfg(unix)]
    interrupt: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
    #[cfg(not(unix))]
    interrupt: Option<tokio::signal::windows::CtrlC>,
}

impl ShutdownSignals {
    /// Register the listeners now. Call it inside a tokio runtime.
    ///
    /// A listener that cannot be registered never fires: the process then
    /// handles fewer signals, it does not fail (see the module docs).
    #[cfg(unix)]
    pub(crate) fn listen() -> Self {
        use tokio::signal::unix::{SignalKind, signal};

        let interrupt = match signal(SignalKind::interrupt()) {
            Ok(stream) => Some(stream),
            Err(error) => {
                // `eprintln!` for the reason given on the SIGTERM case below.
                eprintln!(
                    "warning: could not install a SIGINT handler ({error}); an interactive \
                     ctrl-c will not run the graceful shutdown."
                );
                None
            }
        };
        let terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => Some(stream),
            Err(error) => {
                // Degrade, do not abort: SIGINT coverage is still better than
                // exiting here, and an operator ctrl-c must keep working.
                //
                // `eprintln!` rather than `tracing::warn!` is deliberate - please
                // do not "fix" it. This fires at most once per process, and it
                // reports that a shutdown path is degraded, which is exactly the
                // class of message a log filter must not be able to swallow. A
                // downstream fleet lost 13 days to a leftover `RUST_LOG=warn`
                // discarding the only lines that named a silent failure; a WARN
                // here would survive that filter but not `RUST_LOG=error`, and
                // tracing may not even be initialised at this point in startup.
                eprintln!(
                    "warning: could not install a SIGTERM handler ({error}); shutting down cleanly on \
                     SIGINT only. A container stop signal will terminate this process without \
                     releasing the schedule executor lease."
                );
                None
            }
        };
        Self {
            interrupt,
            terminate,
        }
    }

    /// Non-Unix fallback: SIGTERM has no equivalent, so this is ctrl-c only.
    #[cfg(not(unix))]
    pub(crate) fn listen() -> Self {
        let interrupt = match tokio::signal::windows::ctrl_c() {
            Ok(stream) => Some(stream),
            Err(error) => {
                eprintln!(
                    "warning: could not install a ctrl-c handler ({error}); ctrl-c will not run \
                     the graceful shutdown."
                );
                None
            }
        };
        Self { interrupt }
    }

    /// Listeners that never fire, for tests that drive a wait loop without a
    /// process signal handler.
    #[cfg(test)]
    pub(crate) fn never() -> Self {
        Self {
            interrupt: None,
            #[cfg(unix)]
            terminate: None,
        }
    }

    /// Resolve on the first SIGINT or SIGTERM delivered since [`Self::listen`]
    /// and not yet returned, and report which.
    ///
    /// Cancel safe: a `recv` dropped before it resolves loses no signal, the
    /// next one returns it.
    pub(crate) async fn recv(&mut self) -> ShutdownSignal {
        #[cfg(unix)]
        {
            tokio::select! {
                () = next_unix_signal(&mut self.interrupt) => ShutdownSignal::Interrupt,
                () = next_unix_signal(&mut self.terminate) => ShutdownSignal::Terminate,
            }
        }
        #[cfg(not(unix))]
        {
            if let Some(stream) = self.interrupt.as_mut()
                && stream.recv().await.is_some()
            {
                return ShutdownSignal::Interrupt;
            }
            std::future::pending().await
        }
    }
}

/// The next delivery to `listener`; never, for a listener that could not be
/// registered or whose runtime's signal driver is gone.
#[cfg(unix)]
async fn next_unix_signal(listener: &mut Option<tokio::signal::unix::Signal>) {
    if let Some(stream) = listener.as_mut()
        && stream.recv().await.is_some()
    {
        return;
    }
    std::future::pending::<()>().await;
}

/// Resolve when the process is asked to terminate, by SIGINT or SIGTERM, and
/// report which.
///
/// Called inside a tokio runtime, it registers its listeners at once: a
/// signal delivered after the call, even before the returned future is first
/// polled, resolves it. A binary calls it before it reports itself ready, so
/// a stop signal from then on runs its graceful shutdown; registered at the
/// first poll instead, a signal in between found no handler installed and
/// its default action ended the process. Called outside a runtime, it
/// registers them at the first poll. Inside a runtime the call installs
/// tokio's process-wide SIGINT and SIGTERM handlers at once, even if the
/// future is then dropped unpolled, and neither signal takes its default
/// action in this process after that. A runtime built without the signal
/// driver panics at the call, not at the first poll. The listeners are
/// dropped with the future: for a single wait only. A loop that waits more
/// than once holds one listener across its waits instead (see the module
/// docs).
///
/// See the module docs for why waiting on [`tokio::signal::ctrl_c`] alone is
/// insufficient for any process that will be deployed in a container.
pub fn shutdown_signal() -> impl std::future::Future<Output = ShutdownSignal> + Send + 'static {
    let registered = tokio::runtime::Handle::try_current()
        .is_ok()
        .then(ShutdownSignals::listen);
    async move {
        let mut signals = registered.unwrap_or_else(ShutdownSignals::listen);
        signals.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exit log line's `signal=` token is what an operator greps for and
    /// what the gateway exit-reason tests assert on; pin the exact spelling.
    #[test]
    fn signal_names_are_the_conventional_kill_l_tokens() {
        assert_eq!(ShutdownSignal::Interrupt.name(), "SIGINT");
        assert_eq!(ShutdownSignal::Terminate.name(), "SIGTERM");
        assert_eq!(ShutdownSignal::Interrupt.to_string(), "SIGINT");
        assert_eq!(ShutdownSignal::Terminate.to_string(), "SIGTERM");
    }

    /// Called inside a runtime, `shutdown_signal` registers at once: a
    /// SIGTERM delivered after the call and before the returned future is
    /// first polled resolves it. Registered at the first poll instead, the
    /// signal finds no handler installed and its default action ends the
    /// process.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::expect_used, clippy::unwrap_used)]
    fn a_sigterm_before_the_first_poll_resolves_the_wait() {
        // It signals its own process: run as the only test of a child process.
        if !crate::test_process_isolation::run_body_in_isolated_process(
            module_path!(),
            "a_sigterm_before_the_first_poll_resolves_the_wait",
        ) {
            return;
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let wait = shutdown_signal();
            let killed = std::process::Command::new("kill")
                .args(["-TERM", &std::process::id().to_string()])
                .status()
                .unwrap();
            assert!(killed.success(), "kill -TERM failed: {killed}");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let signal = tokio::time::timeout(std::time::Duration::from_secs(10), wait)
                .await
                .expect("the SIGTERM delivered before the first poll resolves the wait");
            assert_eq!(signal, ShutdownSignal::Terminate);
        });
    }
}
