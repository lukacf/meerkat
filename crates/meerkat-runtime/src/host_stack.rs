//! One worker-stack budget and worker count for every shipping Meerkat host
//! process.
//!
//! `rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp` and `rkat-mob-mcp` all run the
//! same runtime machinery, so they run it on the same stack budget, from one
//! constant, and the main future runs on a budgeted thread as well rather
//! than on the platform main thread (8 MiB on Linux and macOS, 1 MiB on
//! Windows).
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
//!
//! The two runtimes this crate keeps for the whole process, the machine
//! cleanup runtime and the runtime-loop serving runtime, run session and mob
//! work in every process that uses the runtime, a host binary or an
//! embedding. That work includes what an embedding host composes: the
//! cleanup runtime polls the turns of runtime-backed mob members, with the
//! host's tools and whatever those spawn. So their threads must not have
//! less stack than the host's own workers, and they get twice the budget,
//! 16 MiB. They follow the same knobs: `RKAT_WORKER_STACK_BYTES` sets their
//! thread stack exactly, and without it they take the 16 MiB, or
//! `RUST_MIN_STACK` when that asks for more. Their threads have an explicit
//! size, and std applies `RUST_MIN_STACK` only to threads without one, so
//! this is how an embedding whose workers or debug build need more than
//! 16 MiB raises them: with the variable it already sets for its own
//! threads.
//!
//! Worker count. The runtime starts one worker per available CPU
//! (`std::thread::available_parallelism`: cpusets and cgroup CPU quotas
//! shrink it, a pids limit does not). `RKAT_WORKER_THREADS` sets the count,
//! and Tokio's own `TOKIO_WORKER_THREADS` is read after it through the same
//! validation, so a bad value is refused here instead of panicking in Tokio.
//! A host whose CPU count exceeds its thread limit (a container pids limit,
//! `RLIMIT_NPROC`) cannot start every worker. Tokio queues the workers it
//! could not spawn without reporting them, so the first refusal anyone sees
//! is the `{name}-main` thread; [`HostStackError::ThreadLimit`] reports it
//! with the worker count, the limits Linux exposes, and the knob.

use std::future::Future;
use std::num::NonZeroUsize;

/// Worker and main-future stack budget for every Meerkat host binary.
pub const HOST_WORKER_STACK_BUDGET: usize = 8 * 1024 * 1024;

/// Environment variable that overrides [`HOST_WORKER_STACK_BUDGET`].
pub const HOST_WORKER_STACK_ENV: &str = "RKAT_WORKER_STACK_BYTES";

/// Smallest override accepted; anything below cannot run a Tokio worker.
pub const HOST_WORKER_STACK_FLOOR: usize = 64 * 1024;

/// Environment variable that sets the Tokio worker count of a host runtime.
pub const HOST_WORKER_THREADS_ENV: &str = "RKAT_WORKER_THREADS";

/// The variable std reads for the stack of every thread it sizes itself.
const RUST_MIN_STACK_ENV: &str = "RUST_MIN_STACK";

/// Stack of each thread of the two runtimes this crate keeps for the whole
/// process (machine cleanup and runtime-loop serving) when
/// [`HOST_WORKER_STACK_ENV`] is unset, twice [`HOST_WORKER_STACK_BUDGET`];
/// `RUST_MIN_STACK` raises it and never lowers it. These threads run Core's
/// own machine cleanup and runtime-loop work; the turns of sessions and mob
/// members, with the tools an embedding host gives them, run on the host's
/// runtime. The default keeps the margin those turns needed when the cleanup
/// runtime polled them.
const PROCESS_RUNTIME_STACK_BUDGET: usize = 16 * 1024 * 1024;

/// Tokio's own worker-count variable, read after [`HOST_WORKER_THREADS_ENV`].
const TOKIO_WORKER_THREADS_ENV: &str = "TOKIO_WORKER_THREADS";

/// A host failed before its main future could run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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
    /// A worker-count variable holds something other than a whole number of
    /// at least 1.
    #[error(
        "{name}={raw:?} is not a worker thread count >= 1; refusing to guess. \
         Unset it to start one worker per available CPU."
    )]
    #[non_exhaustive]
    InvalidWorkerThreads { name: String, raw: String },
    /// The OS refused the main thread with EAGAIN after the runtime launched
    /// its workers: the process is, almost always, at its thread limit.
    #[error(
        "failed to spawn the {name} main thread: {source}. The OS refuses new \
         threads, usually because the process is at its thread limit; the \
         runtime asks for {} ({}). {}{} {}",
        worker_threads_phrase(*.workers),
        workers_origin(.workers_set_by.as_deref()),
        limit_sentence(.observed_limit.as_deref()),
        stack_sentence(.stack_overridden_by.as_deref(), *.stack_bytes),
        remedy_sentence(*.workers, .workers_set_by.as_deref())
    )]
    #[non_exhaustive]
    ThreadLimit {
        name: String,
        /// Workers the runtime was built with (not all of them may run).
        workers: usize,
        /// The variable that set the worker count; `None` is one worker per
        /// available CPU.
        workers_set_by: Option<String>,
        /// Stack bytes per thread.
        stack_bytes: usize,
        /// The variable that overrode the stack budget, if any: mapping an
        /// oversized stack fails with EAGAIN as well.
        stack_overridden_by: Option<String>,
        /// The limits Linux exposes (cgroup `pids.max`, `RLIMIT_NPROC`), when
        /// readable and finite.
        observed_limit: Option<String>,
        #[source]
        source: std::io::Error,
    },
}

/// `1 worker thread`, `8 worker threads`.
fn worker_threads_phrase(workers: usize) -> String {
    if workers == 1 {
        "1 worker thread".to_string()
    } else {
        format!("{workers} worker threads")
    }
}

/// Where a worker count came from, for [`HostStackError::ThreadLimit`].
fn workers_origin(set_by: Option<&str>) -> String {
    match set_by {
        Some(variable) => format!("set by {variable}"),
        None => "one per available CPU".to_string(),
    }
}

/// The limit sentence of [`HostStackError::ThreadLimit`]'s message.
fn limit_sentence(observed: Option<&str>) -> String {
    match observed {
        Some(limits) => format!("Observed limits: {limits}."),
        None => {
            "The likely limit is a container pids limit or RLIMIT_NPROC (`ulimit -u`).".to_string()
        }
    }
}

/// The last sentence of [`HostStackError::ThreadLimit`]'s message. With one
/// worker there is nothing to lower: 0 is refused.
fn remedy_sentence(workers: usize, set_by: Option<&str>) -> String {
    if workers == 1 {
        "Raise the limit.".to_string()
    } else {
        format!(
            "Set {} to fewer workers, or raise the limit.",
            set_by.unwrap_or(HOST_WORKER_THREADS_ENV)
        )
    }
}

/// The stack sentence of [`HostStackError::ThreadLimit`]'s message: empty
/// unless a variable overrode the stack, whose mapping can be what failed.
fn stack_sentence(overridden_by: Option<&str>, bytes: usize) -> String {
    match overridden_by {
        Some(variable) => format!(
            " {variable} sets each thread's stack to {bytes} bytes, and mapping a \
             stack that size can also make the OS refuse the thread."
        ),
        None => String::new(),
    }
}

/// The resolved stack budget for one host process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostStackBudget {
    bytes: usize,
    overridden_by: Option<String>,
    worker_threads: Option<WorkerThreads>,
}

/// A worker count and the variable that set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkerThreads {
    count: NonZeroUsize,
    set_by: &'static str,
}

impl HostStackBudget {
    /// The documented default, no override.
    #[must_use]
    pub const fn default_budget() -> Self {
        Self {
            bytes: HOST_WORKER_STACK_BUDGET,
            overridden_by: None,
            worker_threads: None,
        }
    }

    /// Resolve from the process environment. `names` are consulted in order
    /// and the first one that is set decides; hosts that once had their own
    /// variable pass it after [`HOST_WORKER_STACK_ENV`] so old invocations
    /// keep working. The worker count comes from [`HOST_WORKER_THREADS_ENV`],
    /// then `TOKIO_WORKER_THREADS`. A value that is not valid UTF-8 is
    /// refused like any other value that is not a number.
    pub fn from_env(names: &[&str]) -> Result<Self, HostStackError> {
        Self::from_lookup(names, env_lookup)
    }

    /// [`Self::from_env`] with an injected environment, for tests.
    pub fn from_lookup(
        names: &[&str],
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, HostStackError> {
        let mut budget = Self::default_budget();
        if let Some((name, raw)) = names
            .iter()
            .find_map(|name| lookup(name).map(|raw| (*name, raw)))
        {
            budget.bytes = parse_stack_override(name, raw, HOST_WORKER_STACK_BUDGET)?;
            budget.overridden_by = Some(name.to_string());
        }
        budget.worker_threads = resolve_worker_threads(&lookup)?;
        Ok(budget)
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

    /// The worker count [`HOST_WORKER_THREADS_ENV`] or `TOKIO_WORKER_THREADS`
    /// set; `None` starts one worker per available CPU.
    #[must_use]
    pub fn worker_threads(&self) -> Option<NonZeroUsize> {
        self.worker_threads.map(|workers| workers.count)
    }

    /// The variable that set the worker count, if any.
    #[must_use]
    pub fn worker_threads_set_by(&self) -> Option<&'static str> {
        self.worker_threads.map(|workers| workers.set_by)
    }

    /// Build a multi-thread Tokio runtime whose workers and blocking threads
    /// all get this budget, with the configured worker count.
    pub fn build_runtime(&self, name: &str) -> Result<tokio::runtime::Runtime, HostStackError> {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.enable_all().thread_stack_size(self.bytes);
        if let Some(workers) = self.worker_threads {
            builder.worker_threads(workers.count.get());
        }
        builder.build().map_err(|source| HostStackError::Runtime {
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
        self.run_with_spawn(name, make, |builder, main| builder.spawn(main))
    }

    /// [`Self::run`] with the `{name}-main` thread spawn injected, so tests
    /// can make the OS refuse it.
    fn run_with_spawn<T, F, Fut, S>(
        &self,
        name: &str,
        make: F,
        spawn: S,
    ) -> Result<T, HostStackError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = T>,
        T: Send + 'static,
        S: FnOnce(
            std::thread::Builder,
            Box<dyn FnOnce() -> T + Send>,
        ) -> std::io::Result<std::thread::JoinHandle<T>>,
    {
        if let Some(variable) = self.overridden_by() {
            eprintln!(
                "{name}: worker stack overridden to {} bytes by {variable}",
                self.bytes
            );
        }
        let runtime = self.build_runtime(name)?;
        let workers = runtime.metrics().num_workers();
        let builder = std::thread::Builder::new()
            .name(format!("{name}-main"))
            .stack_size(self.bytes);
        let handle = spawn(
            builder,
            Box::new(move || {
                let output = runtime.block_on(make());
                drop(runtime);
                output
            }),
        )
        .map_err(|source| self.main_thread_spawn_error(name, workers, source))?;
        match handle.join() {
            Ok(output) => Ok(output),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// A refused `{name}-main` thread. EAGAIN (`WouldBlock`) after the
    /// workers launched means a thread limit, so it is reported with what the
    /// runtime asked for and the limits Linux exposes.
    fn main_thread_spawn_error(
        &self,
        name: &str,
        workers: usize,
        source: std::io::Error,
    ) -> HostStackError {
        if source.kind() != std::io::ErrorKind::WouldBlock {
            return HostStackError::Thread {
                name: name.to_string(),
                source,
            };
        }
        HostStackError::ThreadLimit {
            name: name.to_string(),
            workers,
            workers_set_by: self
                .worker_threads
                .map(|workers| workers.set_by.to_string()),
            stack_bytes: self.bytes,
            stack_overridden_by: self.overridden_by.clone(),
            observed_limit: observed_thread_limits(),
            source,
        }
    }
}

/// The process environment as [`HostStackBudget::from_lookup`] reads it: a
/// value that is not valid UTF-8 is passed on (lossily) to be refused, not
/// treated as unset.
fn env_lookup(name: &str) -> Option<String> {
    std::env::var_os(name).map(|value| value.to_string_lossy().into_owned())
}

/// A stack override's byte count: at least [`HOST_WORKER_STACK_FLOOR`], or
/// refused rather than guessed at, naming the `default` the threads take
/// once it is unset.
fn parse_stack_override(name: &str, raw: String, default: usize) -> Result<usize, HostStackError> {
    match raw.trim().parse::<usize>() {
        Ok(bytes) if bytes >= HOST_WORKER_STACK_FLOOR => Ok(bytes),
        _ => Err(HostStackError::InvalidOverride {
            name: name.to_string(),
            raw,
            floor: HOST_WORKER_STACK_FLOOR,
            default,
        }),
    }
}

/// Stack bytes for each thread of the two runtimes this crate keeps for the
/// whole process: the machine cleanup runtime and the runtime-loop serving
/// runtime. [`HOST_WORKER_STACK_ENV`] sets it exactly, validated as
/// [`HostStackBudget::from_env`] validates it. Without it the threads take
/// [`PROCESS_RUNTIME_STACK_BUDGET`], or `RUST_MIN_STACK` when that asks for
/// more: std applies that variable only to the threads it sizes itself, and
/// these threads have an explicit size.
pub(crate) fn process_runtime_stack_bytes() -> Result<usize, HostStackError> {
    process_runtime_stack_bytes_from(env_lookup)
}

/// [`process_runtime_stack_bytes`] with an injected environment, for tests.
/// `RUST_MIN_STACK` is read as std reads it: a value that is not a byte count
/// is ignored, and it never lowers the default. A refused override names
/// that default.
fn process_runtime_stack_bytes_from(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<usize, HostStackError> {
    let min_stack = lookup(RUST_MIN_STACK_ENV)
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(0);
    let default = PROCESS_RUNTIME_STACK_BUDGET.max(min_stack);
    match lookup(HOST_WORKER_STACK_ENV) {
        Some(raw) => parse_stack_override(HOST_WORKER_STACK_ENV, raw, default),
        None => Ok(default),
    }
}

/// [`HOST_WORKER_THREADS_ENV`], then `TOKIO_WORKER_THREADS`: the first one
/// set decides, and it must be a whole number of at least 1.
fn resolve_worker_threads(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Option<WorkerThreads>, HostStackError> {
    let Some((set_by, raw)) = [HOST_WORKER_THREADS_ENV, TOKIO_WORKER_THREADS_ENV]
        .into_iter()
        .find_map(|name| lookup(name).map(|raw| (name, raw)))
    else {
        return Ok(None);
    };
    match raw.trim().parse::<NonZeroUsize>() {
        Ok(count) => Ok(Some(WorkerThreads { count, set_by })),
        Err(_) => Err(HostStackError::InvalidWorkerThreads {
            name: set_by.to_string(),
            raw,
        }),
    }
}

/// The worker count for a host that builds its own Tokio runtime:
/// [`HOST_WORKER_THREADS_ENV`], then `TOKIO_WORKER_THREADS`, validated as
/// [`HostStackBudget::from_env`] does. `None` keeps Tokio's default of one
/// worker per available CPU; apply `Some` with `Builder::worker_threads`.
pub fn worker_threads_from_env() -> Result<Option<NonZeroUsize>, HostStackError> {
    Ok(resolve_worker_threads(env_lookup)?.map(|workers| workers.count))
}

/// The hint a host adds to a fatal error caused by a refused thread or
/// process spawn: an [`std::io::Error`] of kind `WouldBlock` (EAGAIN)
/// anywhere in `chain`, the error followed by its sources. `None` for any
/// other error. It names the variable that set the worker count, read from
/// the process environment as [`HostStackBudget::from_env`] reads it.
pub fn thread_limit_hint<'e>(
    chain: impl Iterator<Item = &'e (dyn std::error::Error + 'static)>,
) -> Option<String> {
    let default_workers = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    thread_limit_hint_from(chain, env_lookup, default_workers)
}

/// [`thread_limit_hint`] with an injected environment and default worker
/// count (one per available CPU), for tests.
fn thread_limit_hint_from<'e>(
    mut chain: impl Iterator<Item = &'e (dyn std::error::Error + 'static)>,
    lookup: impl Fn(&str) -> Option<String>,
    default_workers: usize,
) -> Option<String> {
    let refused = chain.any(|link| {
        link.downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::WouldBlock)
    });
    if !refused {
        return None;
    }
    // A bad value stops the host before its runtime exists, so an error
    // here cannot have come from that host; treat it as unset. With one
    // worker there is nothing to lower: 0 is refused.
    let workers = match resolve_worker_threads(lookup) {
        Ok(Some(workers)) if workers.count.get() == 1 => format!(
            "The runtime starts 1 worker thread, set by {}; raise the limit.",
            workers.set_by
        ),
        Ok(Some(workers)) => format!(
            "The runtime starts {}, set by {}; lower it, or raise the limit.",
            worker_threads_phrase(workers.count.get()),
            workers.set_by
        ),
        Ok(None) | Err(_) if default_workers == 1 => {
            "The runtime starts 1 worker thread (one per available CPU); raise the limit."
                .to_string()
        }
        Ok(None) | Err(_) => format!(
            "The runtime starts one worker thread per available CPU unless \
             {HOST_WORKER_THREADS_ENV} sets fewer; set it, or raise the limit."
        ),
    };
    Some(format!(
        "hint: the OS refused a new thread or process (EAGAIN), usually because the \
         process is at its thread limit (a container pids limit or RLIMIT_NPROC). \
         {workers}"
    ))
}

/// The limits a refused thread spawn usually hits, as Linux exposes them:
/// the tightest cgroup `pids.max` on the process's cgroup path and the
/// `RLIMIT_NPROC` soft limit. Best effort: `None` when neither is readable
/// and finite. No usage figure: by the time the refusal is reported the
/// runtime that held the threads has shut down.
#[cfg(target_os = "linux")]
fn observed_thread_limits() -> Option<String> {
    observed_thread_limits_from(|path| std::fs::read_to_string(path).ok())
}

#[cfg(not(target_os = "linux"))]
fn observed_thread_limits() -> Option<String> {
    None
}

/// [`observed_thread_limits`] over an injected file reader, for tests.
#[cfg(any(target_os = "linux", test))]
fn observed_thread_limits_from(read: impl Fn(&str) -> Option<String>) -> Option<String> {
    let cgroup = cgroup_pids_limit(&read).map(|max| format!("cgroup pids.max {max}"));
    let nproc = rlimit_nproc(&read).map(|soft| format!("RLIMIT_NPROC {soft}"));
    match (cgroup, nproc) {
        (Some(cgroup), Some(nproc)) => Some(format!("{cgroup}, {nproc}")),
        (cgroup, nproc) => cgroup.or(nproc),
    }
}

/// The tightest finite `pids.max` from the process's cgroup up to the root:
/// cgroup v2 (`0::<path>`) or the v1 `pids` hierarchy.
#[cfg(any(target_os = "linux", test))]
fn cgroup_pids_limit(read: &impl Fn(&str) -> Option<String>) -> Option<u64> {
    let cgroups = read("/proc/self/cgroup")?;
    let (root, path) = cgroups.lines().find_map(|line| {
        let mut fields = line.splitn(3, ':');
        let (_, controllers, path) = (fields.next()?, fields.next()?, fields.next()?);
        if controllers.is_empty() {
            Some(("/sys/fs/cgroup".to_string(), path))
        } else if controllers
            .split(',')
            .any(|controller| controller == "pids")
        {
            Some((format!("/sys/fs/cgroup/{controllers}"), path))
        } else {
            None
        }
    })?;
    let mut tightest: Option<u64> = None;
    let mut dir = path.trim_end_matches('/');
    loop {
        if let Some(max) =
            read(&format!("{root}{dir}/pids.max")).and_then(|value| value.trim().parse().ok())
        {
            tightest = Some(tightest.map_or(max, |seen: u64| seen.min(max)));
        }
        if dir.is_empty() {
            return tightest;
        }
        dir = dir.rfind('/').map_or("", |cut| &dir[..cut]);
    }
}

/// The `RLIMIT_NPROC` soft limit from `/proc/self/limits`, when finite.
#[cfg(any(target_os = "linux", test))]
fn rlimit_nproc(read: &impl Fn(&str) -> Option<String>) -> Option<u64> {
    let limits = read("/proc/self/limits")?;
    let line = limits
        .lines()
        .find(|line| line.starts_with("Max processes"))?;
    line.trim_start_matches("Max processes")
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Run a host's main future on the documented budget, honouring
/// [`HOST_WORKER_STACK_ENV`] and [`HOST_WORKER_THREADS_ENV`].
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
    fn process_runtimes_take_the_override_exactly_else_the_budget_raised_to_rust_min_stack() {
        const MIB: usize = 1024 * 1024;
        let cases: [(&[(&str, &str)], usize); 9] = [
            // Nothing set: 16 MiB, twice the host worker budget, so these
            // threads do not have less stack than an embedding host's own
            // 16 MiB workers.
            (&[], 16 * MIB),
            // RUST_MIN_STACK raises the default and never lowers it, the
            // 8 MiB a test environment gives its own threads included.
            (&[("RUST_MIN_STACK", "33554432")], 32 * MIB),
            (&[("RUST_MIN_STACK", "8388608")], 16 * MIB),
            (&[("RUST_MIN_STACK", "2097152")], 16 * MIB),
            // A RUST_MIN_STACK that std ignores is ignored here too.
            (&[("RUST_MIN_STACK", "32MiB")], 16 * MIB),
            // The override is exact, below the default and RUST_MIN_STACK
            // included, so a diagnosis can sweep under them.
            (
                &[
                    (HOST_WORKER_STACK_ENV, " 1048576 "),
                    ("RUST_MIN_STACK", "33554432"),
                ],
                MIB,
            ),
            (&[(HOST_WORKER_STACK_ENV, "8388608")], 8 * MIB),
            (&[(HOST_WORKER_STACK_ENV, "67108864")], 64 * MIB),
            // Worker counts are a host runtime's concern, not these runtimes'.
            (
                &[
                    ("TOKIO_WORKER_THREADS", "0"),
                    (HOST_WORKER_THREADS_ENV, "two"),
                ],
                16 * MIB,
            ),
        ];
        for (pairs, bytes) in cases {
            assert_eq!(
                process_runtime_stack_bytes_from(env(pairs)).unwrap(),
                bytes,
                "{pairs:?}"
            );
        }

        // The override is validated as a host validates it, whatever
        // RUST_MIN_STACK says, and the refusal names the default these
        // threads take once it is unset.
        for raw in ["65535", "8MiB", ""] {
            for (min_stack, default) in [("2097152", 16 * MIB), ("33554432", 32 * MIB)] {
                match process_runtime_stack_bytes_from(env(&[
                    (HOST_WORKER_STACK_ENV, raw),
                    ("RUST_MIN_STACK", min_stack),
                ])) {
                    Err(HostStackError::InvalidOverride {
                        name,
                        raw: seen,
                        floor,
                        default: named,
                    }) => {
                        assert_eq!(name, HOST_WORKER_STACK_ENV);
                        assert_eq!(seen, raw);
                        assert_eq!(floor, HOST_WORKER_STACK_FLOOR);
                        assert_eq!(named, default, "RUST_MIN_STACK={min_stack}");
                    }
                    other => panic!("expected InvalidOverride for {raw:?}, got {other:?}"),
                }
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

    #[test]
    fn worker_threads_come_from_the_knob_then_tokio_and_are_named() {
        let budget = HostStackBudget::from_lookup(&[HOST_WORKER_STACK_ENV], env(&[])).unwrap();
        assert_eq!(budget.worker_threads(), None);
        assert_eq!(budget.worker_threads_set_by(), None);

        let cases: [(&[(&str, &str)], usize, &str); 3] = [
            (
                &[(HOST_WORKER_THREADS_ENV, " 3 ")],
                3,
                HOST_WORKER_THREADS_ENV,
            ),
            (&[("TOKIO_WORKER_THREADS", "5")], 5, "TOKIO_WORKER_THREADS"),
            (
                &[
                    (HOST_WORKER_THREADS_ENV, "2"),
                    ("TOKIO_WORKER_THREADS", "0"),
                ],
                2,
                HOST_WORKER_THREADS_ENV,
            ),
        ];
        for (pairs, count, set_by) in cases {
            let budget = HostStackBudget::from_lookup(&[HOST_WORKER_STACK_ENV], env(pairs))
                .unwrap_or_else(|error| panic!("{pairs:?} must resolve: {error}"));
            assert_eq!(
                budget.worker_threads(),
                NonZeroUsize::new(count),
                "{pairs:?}"
            );
            assert_eq!(budget.worker_threads_set_by(), Some(set_by), "{pairs:?}");
            assert_eq!(budget.bytes(), HOST_WORKER_STACK_BUDGET, "{pairs:?}");
        }
    }

    #[test]
    fn worker_thread_counts_that_are_not_whole_numbers_above_zero_are_refused() {
        for name in [HOST_WORKER_THREADS_ENV, "TOKIO_WORKER_THREADS"] {
            for raw in ["0", "-1", "two", "", "1.5"] {
                let error =
                    HostStackBudget::from_lookup(&[HOST_WORKER_STACK_ENV], env(&[(name, raw)]))
                        .unwrap_err();
                let message = error.to_string();
                match error {
                    HostStackError::InvalidWorkerThreads {
                        name: seen,
                        raw: value,
                    } => {
                        assert_eq!(seen, name);
                        assert_eq!(value, raw);
                    }
                    other => panic!("expected InvalidWorkerThreads, got {other:?}"),
                }
                assert!(
                    message.starts_with(&format!("{name}={raw:?} is not a worker thread count")),
                    "{message}"
                );
            }
        }
    }

    /// A worker count the host would not pick on its own, so a runtime that
    /// ignores the knob is caught.
    fn worker_count_unlike_the_default() -> usize {
        let default = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        if default == 3 { 2 } else { 3 }
    }

    #[test]
    fn build_runtime_starts_the_configured_worker_count() {
        let wanted = worker_count_unlike_the_default();
        let budget = HostStackBudget::from_lookup(
            &[HOST_WORKER_STACK_ENV],
            env(&[(HOST_WORKER_THREADS_ENV, &wanted.to_string())]),
        )
        .unwrap();
        let runtime = budget.build_runtime("rkat-test").unwrap();
        assert_eq!(runtime.metrics().num_workers(), wanted);

        // Unset, the runtime keeps Tokio's default: one worker per CPU.
        if std::env::var_os("TOKIO_WORKER_THREADS").is_none() {
            let runtime = HostStackBudget::default_budget()
                .build_runtime("rkat-test")
                .unwrap();
            assert_eq!(
                runtime.metrics().num_workers(),
                std::thread::available_parallelism().map_or(1, NonZeroUsize::get)
            );
        }
    }

    fn refuse_main_thread(
        error: std::io::Error,
    ) -> impl FnOnce(
        std::thread::Builder,
        Box<dyn FnOnce() + Send>,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        move |_, _| Err(error)
    }

    #[test]
    fn a_main_thread_refused_with_eagain_reports_the_thread_limit_and_the_knob() {
        let wanted = worker_count_unlike_the_default();
        let budget = HostStackBudget::from_lookup(
            &[HOST_WORKER_STACK_ENV],
            env(&[(HOST_WORKER_THREADS_ENV, &wanted.to_string())]),
        )
        .unwrap();
        let error = budget
            .run_with_spawn(
                "rkat-test",
                || async {},
                refuse_main_thread(std::io::Error::from(std::io::ErrorKind::WouldBlock)),
            )
            .unwrap_err();
        let message = error.to_string();
        match error {
            HostStackError::ThreadLimit {
                name,
                workers,
                workers_set_by,
                stack_bytes,
                stack_overridden_by,
                ..
            } => {
                assert_eq!(name, "rkat-test");
                assert_eq!(workers, wanted);
                assert_eq!(workers_set_by.as_deref(), Some(HOST_WORKER_THREADS_ENV));
                assert_eq!(stack_bytes, HOST_WORKER_STACK_BUDGET);
                assert_eq!(stack_overridden_by, None);
            }
            other => panic!("expected ThreadLimit, got {other:?}"),
        }
        assert!(
            message.starts_with("failed to spawn the rkat-test main thread: "),
            "{message}"
        );
        assert!(
            message.contains(&format!(
                "the runtime asks for {wanted} worker threads (set by RKAT_WORKER_THREADS)"
            )),
            "{message}"
        );
        assert!(
            message.contains("Set RKAT_WORKER_THREADS to fewer workers, or raise the limit."),
            "{message}"
        );
        assert!(!message.contains("stack"), "{message}");

        // Without the knob the count is the runtime's own default, and the
        // OS's own EAGAIN is the same refusal.
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let eagain = std::io::Error::from_raw_os_error(nix::errno::Errno::EAGAIN as i32);
            let error = HostStackBudget::default_budget()
                .run_with_spawn("rkat-test", || async {}, refuse_main_thread(eagain))
                .unwrap_err();
            let message = error.to_string();
            let workers = match error {
                HostStackError::ThreadLimit {
                    workers,
                    workers_set_by,
                    ..
                } => {
                    assert_eq!(workers_set_by, None);
                    workers
                }
                other => panic!("expected ThreadLimit, got {other:?}"),
            };
            assert!(message.contains("(one per available CPU)"), "{message}");
            if workers > 1 {
                assert!(message.contains("Set RKAT_WORKER_THREADS to"), "{message}");
            } else {
                assert!(message.ends_with(" Raise the limit."), "{message}");
            }
        }
    }

    #[test]
    fn a_thread_limit_names_the_variable_that_set_the_count_and_one_worker() {
        let budget = HostStackBudget::from_lookup(
            &[HOST_WORKER_STACK_ENV],
            env(&[("TOKIO_WORKER_THREADS", "1")]),
        )
        .unwrap();
        let message = budget
            .run_with_spawn(
                "rkat-test",
                || async {},
                refuse_main_thread(std::io::Error::from(std::io::ErrorKind::WouldBlock)),
            )
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("the runtime asks for 1 worker thread (set by TOKIO_WORKER_THREADS)"),
            "{message}"
        );
        assert!(message.ends_with(" Raise the limit."), "{message}");
        assert!(!message.contains("fewer workers"), "{message}");
    }

    #[test]
    fn a_thread_limit_under_a_stack_override_names_the_stack_bytes() {
        let budget = HostStackBudget::from_lookup(
            &[HOST_WORKER_STACK_ENV],
            env(&[
                (HOST_WORKER_STACK_ENV, "1048576"),
                (HOST_WORKER_THREADS_ENV, "2"),
            ]),
        )
        .unwrap();
        let error = budget
            .run_with_spawn(
                "rkat-test",
                || async {},
                refuse_main_thread(std::io::Error::from(std::io::ErrorKind::WouldBlock)),
            )
            .unwrap_err();
        let message = error.to_string();
        match error {
            HostStackError::ThreadLimit {
                stack_bytes,
                stack_overridden_by,
                ..
            } => {
                assert_eq!(stack_bytes, 1_048_576);
                assert_eq!(stack_overridden_by.as_deref(), Some(HOST_WORKER_STACK_ENV));
            }
            other => panic!("expected ThreadLimit, got {other:?}"),
        }
        assert!(
            message.contains(
                "RKAT_WORKER_STACK_BYTES sets each thread's stack to 1048576 bytes, and mapping \
                 a stack that size can also make the OS refuse the thread."
            ),
            "{message}"
        );
        assert!(
            message.ends_with("Set RKAT_WORKER_THREADS to fewer workers, or raise the limit."),
            "{message}"
        );
    }

    #[test]
    fn a_main_thread_refused_for_another_reason_stays_a_spawn_error() {
        let error = HostStackBudget::default_budget()
            .run_with_spawn(
                "rkat-test",
                || async {},
                refuse_main_thread(std::io::Error::other("no")),
            )
            .unwrap_err();
        assert!(
            matches!(error, HostStackError::Thread { ref name, .. } if name == "rkat-test"),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "failed to spawn the rkat-test main thread: no"
        );
    }

    fn files<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        env(pairs)
    }

    const LIMITS: &str = "Limit                     Soft Limit           Hard Limit           Units     \n\
                          Max processes             4096                 8192                 processes \n";

    #[test]
    fn observed_limits_name_the_tightest_cgroup_pids_max_and_rlimit_nproc() {
        let scope = "/sys/fs/cgroup/user.slice/app.slice/run-1.scope";
        let observed = observed_thread_limits_from(files(&[
            (
                "/proc/self/cgroup",
                "0::/user.slice/app.slice/run-1.scope\n",
            ),
            (&format!("{scope}/pids.max"), "128\n"),
            ("/sys/fs/cgroup/user.slice/app.slice/pids.max", "4096\n"),
            ("/sys/fs/cgroup/user.slice/pids.max", "max\n"),
            ("/proc/self/limits", LIMITS),
        ]));
        assert_eq!(
            observed.as_deref(),
            Some("cgroup pids.max 128, RLIMIT_NPROC 4096")
        );

        // An ancestor's tighter limit is the one that applies.
        let observed = observed_thread_limits_from(files(&[
            ("/proc/self/cgroup", "0::/kubepods/pod-1/ctr\n"),
            ("/sys/fs/cgroup/kubepods/pod-1/ctr/pids.max", "max\n"),
            ("/sys/fs/cgroup/kubepods/pod-1/pids.max", "64\n"),
        ]));
        assert_eq!(observed.as_deref(), Some("cgroup pids.max 64"));

        // cgroup v1 keeps pids in its own hierarchy.
        let observed = observed_thread_limits_from(files(&[
            (
                "/proc/self/cgroup",
                "12:pids:/docker/abc\n1:name=systemd:/docker/abc\n0::/\n",
            ),
            ("/sys/fs/cgroup/pids/docker/abc/pids.max", "100\n"),
        ]));
        assert_eq!(observed.as_deref(), Some("cgroup pids.max 100"));
    }

    #[test]
    fn observed_limits_are_none_when_nothing_is_finite_or_readable() {
        assert_eq!(observed_thread_limits_from(files(&[])), None);
        let unlimited =
            "Max processes             unlimited            unlimited            processes \n";
        assert_eq!(
            observed_thread_limits_from(files(&[
                ("/proc/self/cgroup", "0::/\n"),
                ("/sys/fs/cgroup/pids.max", "max\n"),
                ("/proc/self/limits", unlimited),
            ])),
            None
        );
        assert_eq!(
            observed_thread_limits_from(files(&[("/proc/self/limits", LIMITS)])).as_deref(),
            Some("RLIMIT_NPROC 4096")
        );
    }

    #[derive(Debug, thiserror::Error)]
    #[error("I/O error: {0}")]
    struct Wrapped(#[source] std::io::Error);

    fn hint_for(
        error: &(dyn std::error::Error + 'static),
        pairs: &[(&str, &str)],
    ) -> Option<String> {
        thread_limit_hint_from(
            std::iter::successors(Some(error), |&link| link.source()),
            env(pairs),
            8,
        )
    }

    #[test]
    fn the_thread_limit_hint_needs_a_would_block_io_error_in_the_chain() {
        let wrapped = Wrapped(std::io::Error::from(std::io::ErrorKind::WouldBlock));
        let hint = hint_for(&wrapped, &[]).unwrap();
        assert!(
            hint.starts_with("hint: the OS refused a new thread"),
            "{hint}"
        );
        assert!(
            hint.ends_with(
                "The runtime starts one worker thread per available CPU unless \
                 RKAT_WORKER_THREADS sets fewer; set it, or raise the limit."
            ),
            "{hint}"
        );

        let bare = std::io::Error::from(std::io::ErrorKind::WouldBlock);
        assert_eq!(hint_for(&bare, &[]), Some(hint));

        assert_eq!(
            hint_for(&Wrapped(std::io::Error::other("disk full")), &[]),
            None
        );
        assert_eq!(hint_for(&std::fmt::Error, &[]), None);
    }

    #[test]
    fn the_thread_limit_hint_names_the_variable_that_set_the_count() {
        let refused = std::io::Error::from(std::io::ErrorKind::WouldBlock);
        let hint = hint_for(&refused, &[(HOST_WORKER_THREADS_ENV, "4")]).unwrap();
        assert!(
            hint.ends_with(
                "The runtime starts 4 worker threads, set by RKAT_WORKER_THREADS; lower it, \
                 or raise the limit."
            ),
            "{hint}"
        );
        let hint = hint_for(&refused, &[("TOKIO_WORKER_THREADS", "1")]).unwrap();
        assert!(
            hint.ends_with(
                "The runtime starts 1 worker thread, set by TOKIO_WORKER_THREADS; raise the \
                 limit."
            ),
            "{hint}"
        );
    }

    #[test]
    fn the_thread_limit_hint_on_a_one_cpu_host_only_says_to_raise_the_limit() {
        let refused = std::io::Error::from(std::io::ErrorKind::WouldBlock);
        let hint = thread_limit_hint_from(
            std::iter::once(&refused as &(dyn std::error::Error + 'static)),
            env(&[]),
            1,
        )
        .unwrap();
        assert!(
            hint.ends_with(
                "The runtime starts 1 worker thread (one per available CPU); raise the limit."
            ),
            "{hint}"
        );
    }
}
