//! One worker-stack budget for every shipping Meerkat host process.
//!
//! `rkat`, `rkat-rpc`, `rkat-rest` and `rkat-mcp` all run the same runtime
//! machinery, so they run it on the same stack budget, from one constant, and
//! the main future runs on a budgeted thread as well rather than on the
//! platform main thread (8 MiB on Linux and macOS, 1 MiB on Windows).
//!
//! Why 8 MiB. Measured on the meerkat-rpc router harness (session/create,
//! turn/start, session/archive through the real `MethodRouter`) on x86_64
//! Linux, rustc 1.94: a release build fits 1 MiB and overflows at 512 KiB; a
//! debug build (opt-level 0, no stack-slot colouring, so a frame is the sum of
//! its branches) fits 4 MiB and overflows at 3 MiB. The e2e lanes run debug
//! binaries, so the budget must hold for both. 8 MiB is 2x the debug
//! measurement and 8x the release one, and a quarter of the 32 MiB `rkat-rpc`
//! shipped with before this budget existed. The budget is virtual address
//! space per thread; pages are committed only when touched.
//!
//! Two gates keep the numbers honest:
//! `rpc_dispatch_path_fits_debug_worker_stack_budget` (meerkat-rpc, 4 MiB at
//! opt-level 0, unit lane) and the nightly `make stack-budget-release`
//! (1 MiB, release). Raise this constant only with a new measurement; lower
//! frames with [`crate::stack_relief`] instead.
//!
//! `RKAT_WORKER_STACK_BYTES` overrides the budget for stack-overflow
//! diagnosis: sweeping it tells a bounded large frame from unbounded recursion.
//! A value that is not a byte count of at least [`HOST_WORKER_STACK_FLOOR`] is
//! refused rather than guessed at.

use std::future::Future;

/// Worker and main-future stack budget for every Meerkat host binary.
pub const HOST_WORKER_STACK_BUDGET: usize = 8 * 1024 * 1024;

/// Environment variable that overrides [`HOST_WORKER_STACK_BUDGET`].
pub const HOST_WORKER_STACK_ENV: &str = "RKAT_WORKER_STACK_BYTES";

/// Smallest override accepted; anything below cannot run a Tokio worker.
pub const HOST_WORKER_STACK_FLOOR: usize = 64 * 1024;

/// A host failed before its main future could run.
#[derive(Debug, thiserror::Error)]
pub enum HostStackError {
    /// The override variable holds something other than a byte count of at
    /// least [`HOST_WORKER_STACK_FLOOR`].
    #[error(
        "{name}={raw:?} is not a byte count >= {floor}; refusing to guess. \
         Unset it to use the {default} byte default."
    )]
    InvalidOverride {
        name: String,
        raw: String,
        floor: usize,
        default: usize,
    },
    /// The Tokio runtime could not be built.
    #[error("failed to build the {name} runtime: {source}")]
    Runtime {
        name: String,
        #[source]
        source: std::io::Error,
    },
    /// The budgeted main thread could not be spawned.
    #[error("failed to spawn the {name} main thread: {source}")]
    Thread {
        name: String,
        #[source]
        source: std::io::Error,
    },
}

/// The resolved stack budget for one host process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostStackBudget {
    bytes: usize,
    overridden_by: Option<String>,
}

impl HostStackBudget {
    /// The documented default, no override.
    #[must_use]
    pub const fn default_budget() -> Self {
        Self {
            bytes: HOST_WORKER_STACK_BUDGET,
            overridden_by: None,
        }
    }

    /// Resolve from the process environment. `names` are consulted in order
    /// and the first one that is set decides; hosts that once had their own
    /// variable pass it after [`HOST_WORKER_STACK_ENV`] so old invocations
    /// keep working.
    pub fn from_env(names: &[&str]) -> Result<Self, HostStackError> {
        Self::from_lookup(names, |name| std::env::var(name).ok())
    }

    /// [`Self::from_env`] with an injected environment, for tests.
    pub fn from_lookup(
        names: &[&str],
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, HostStackError> {
        for name in names {
            let Some(raw) = lookup(name) else {
                continue;
            };
            return match raw.trim().parse::<usize>() {
                Ok(bytes) if bytes >= HOST_WORKER_STACK_FLOOR => Ok(Self {
                    bytes,
                    overridden_by: Some((*name).to_string()),
                }),
                _ => Err(HostStackError::InvalidOverride {
                    name: (*name).to_string(),
                    raw,
                    floor: HOST_WORKER_STACK_FLOOR,
                    default: HOST_WORKER_STACK_BUDGET,
                }),
            };
        }
        Ok(Self::default_budget())
    }

    /// Stack bytes per worker thread and for the main future's thread.
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.bytes
    }

    /// The variable that overrode the default, if any.
    #[must_use]
    pub fn overridden_by(&self) -> Option<&str> {
        self.overridden_by.as_deref()
    }

    /// Build a multi-thread Tokio runtime whose workers and blocking threads
    /// all get this budget.
    pub fn build_runtime(&self, name: &str) -> Result<tokio::runtime::Runtime, HostStackError> {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(self.bytes)
            .build()
            .map_err(|source| HostStackError::Runtime {
                name: name.to_string(),
                source,
            })
    }

    /// Run a host's main future to completion on this budget.
    ///
    /// The future is made and polled on a dedicated `{name}-main` thread with
    /// this stack size, so the platform main thread's stack (1 MiB on
    /// Windows) never bounds the host; the runtime's workers get the same
    /// size. `make` rather than a future so nothing large crosses a thread
    /// boundary and the future needs no `Send`. A panic in the future is
    /// resumed on the caller.
    pub fn run<T, F, Fut>(&self, name: &str, make: F) -> Result<T, HostStackError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = T>,
        T: Send + 'static,
    {
        if let Some(variable) = self.overridden_by() {
            eprintln!(
                "{name}: worker stack overridden to {} bytes by {variable}",
                self.bytes
            );
        }
        let runtime = self.build_runtime(name)?;
        let handle = std::thread::Builder::new()
            .name(format!("{name}-main"))
            .stack_size(self.bytes)
            .spawn(move || {
                let output = runtime.block_on(make());
                drop(runtime);
                output
            })
            .map_err(|source| HostStackError::Thread {
                name: name.to_string(),
                source,
            })?;
        match handle.join() {
            Ok(output) => Ok(output),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}

/// Run a host's main future on the documented budget, honouring
/// [`HOST_WORKER_STACK_ENV`].
pub fn run_host<T, F, Fut>(name: &str, make: F) -> Result<T, HostStackError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = T>,
    T: Send + 'static,
{
    HostStackBudget::from_env(&[HOST_WORKER_STACK_ENV])?.run(name, make)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }

    #[test]
    fn unset_environment_yields_the_documented_default() {
        let budget = HostStackBudget::from_lookup(&[HOST_WORKER_STACK_ENV], env(&[])).unwrap();
        assert_eq!(budget, HostStackBudget::default_budget());
        assert_eq!(budget.bytes(), 8 * 1024 * 1024);
        assert_eq!(budget.overridden_by(), None);
    }

    #[test]
    fn first_set_variable_wins_and_is_named() {
        let budget = HostStackBudget::from_lookup(
            &[HOST_WORKER_STACK_ENV, "RKAT_RPC_WORKER_STACK_BYTES"],
            env(&[("RKAT_RPC_WORKER_STACK_BYTES", " 1048576 ")]),
        )
        .unwrap();
        assert_eq!(budget.bytes(), 1024 * 1024);
        assert_eq!(budget.overridden_by(), Some("RKAT_RPC_WORKER_STACK_BYTES"));

        let budget = HostStackBudget::from_lookup(
            &[HOST_WORKER_STACK_ENV, "RKAT_RPC_WORKER_STACK_BYTES"],
            env(&[
                (HOST_WORKER_STACK_ENV, "2097152"),
                ("RKAT_RPC_WORKER_STACK_BYTES", "1048576"),
            ]),
        )
        .unwrap();
        assert_eq!(budget.bytes(), 2 * 1024 * 1024);
        assert_eq!(budget.overridden_by(), Some(HOST_WORKER_STACK_ENV));
    }

    #[test]
    fn overrides_below_the_floor_or_unparsable_are_refused() {
        for raw in ["65535", "0", "-1", "8MiB", ""] {
            let error = HostStackBudget::from_lookup(
                &[HOST_WORKER_STACK_ENV],
                env(&[(HOST_WORKER_STACK_ENV, raw)]),
            )
            .unwrap_err();
            match error {
                HostStackError::InvalidOverride {
                    name,
                    raw: seen,
                    floor,
                    default,
                } => {
                    assert_eq!(name, HOST_WORKER_STACK_ENV);
                    assert_eq!(seen, raw);
                    assert_eq!(floor, HOST_WORKER_STACK_FLOOR);
                    assert_eq!(default, HOST_WORKER_STACK_BUDGET);
                }
                other => panic!("expected InvalidOverride, got {other:?}"),
            }
        }
    }

    #[test]
    fn run_polls_the_main_future_on_a_budgeted_named_thread() {
        let budget = HostStackBudget::from_lookup(
            &[HOST_WORKER_STACK_ENV],
            env(&[(HOST_WORKER_STACK_ENV, "1048576")]),
        )
        .unwrap();
        let (thread_name, inside_runtime) = budget
            .run("rkat-test", || async {
                let name = std::thread::current().name().map(str::to_owned);
                let inside = tokio::runtime::Handle::try_current().is_ok();
                (name, inside)
            })
            .unwrap();
        assert_eq!(thread_name.as_deref(), Some("rkat-test-main"));
        assert!(
            inside_runtime,
            "the main future must run inside the host runtime"
        );
    }

    #[test]
    fn run_resumes_a_main_future_panic_on_the_caller() {
        let outcome = std::panic::catch_unwind(|| {
            HostStackBudget::default_budget().run("rkat-test", || async {
                panic!("host main panicked");
            })
        });
        assert!(outcome.is_err(), "the panic must reach the caller");
    }
}
