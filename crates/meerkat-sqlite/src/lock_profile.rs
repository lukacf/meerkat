//! The process-wide lock profile of the bundled SQLite.
//!
//! Three build-time settings of the bundled SQLite each put one process-wide
//! mutex in front of every connection in the process, so many sessions in one
//! host process queue on each other:
//!
//! - allocator statistics (`SQLITE_DEFAULT_MEMSTATUS` defaults to 1): every
//!   `sqlite3_malloc` and `sqlite3_free` takes `SQLITE_MUTEX_STATIC_MEM`.
//!   Opening a connection and parsing its schema is the heavy case: thousands
//!   of allocations per open.
//! - the shared page cache: `-DSQLITE_ENABLE_MEMORY_MANAGEMENT` puts every
//!   connection's pages in one cache group behind `SQLITE_MUTEX_STATIC_LRU`,
//!   taken on every page fetch, unpin and release. SQLite itself leaves it
//!   off, and so does `libsqlite3-sys` 0.39; earlier `libsqlite3-sys`
//!   releases built with it.
//! - page-cache overflow statistics: every page buffer allocated or freed on
//!   the heap takes `SQLITE_MUTEX_STATIC_PMEM` unless
//!   `-DSQLITE_DISABLE_PAGECACHE_OVERFLOW_STATS` is set.
//!
//! [`SqliteLockProfile::REQUIRED`] has all three off, and
//! [`REQUIRED_LIBSQLITE3_FLAGS`] is the one build setting that produces it.
//! The repository's `.cargo/config.toml` and `MODULE.bazel` carry exactly that
//! string; a build outside the repository (`cargo install`, an embedding
//! application) sets the `LIBSQLITE3_FLAGS` environment variable to it.
//! Nothing Meerkat calls reads what these switches turn off (memory usage and
//! page-cache overflow statistics, heap limits, `sqlite3_release_memory`).
//!
//! Allocator statistics are the only one of the three with a runtime switch:
//! [`disable_memory_statistics_at_process_start`] turns them off in a build
//! without the flags, when it runs before anything uses SQLite. The host
//! binaries call it as the first statement of `main`.
//!
//! A process whose profile is not the required one says so twice: once as a
//! warning at the first connection opened through [`crate::profile`], and in
//! `rkat storage doctor`, which reports the profile it runs with.

use std::ffi::CStr;
use std::fmt;
use std::os::raw::c_int;
use std::sync::{Once, OnceLock};

use rusqlite::ffi;

/// The `LIBSQLITE3_FLAGS` value that builds the bundled SQLite with
/// [`SqliteLockProfile::REQUIRED`]: allocator statistics off, one page cache
/// per connection, page-cache overflow statistics off.
pub const REQUIRED_LIBSQLITE3_FLAGS: &str = "-DSQLITE_DEFAULT_MEMSTATUS=0 \
     -USQLITE_ENABLE_MEMORY_MANAGEMENT -DSQLITE_DISABLE_PAGECACHE_OVERFLOW_STATS";

/// Which of SQLite's three process-wide locks this process takes. `true` means
/// the lock is in use: every connection in the process queues on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SqliteLockProfile {
    /// Allocator statistics: `SQLITE_MUTEX_STATIC_MEM` on every allocation.
    pub memory_statistics: bool,
    /// One page-cache group for every connection: `SQLITE_MUTEX_STATIC_LRU`
    /// on every page operation.
    pub shared_page_cache: bool,
    /// Page-cache overflow statistics: `SQLITE_MUTEX_STATIC_PMEM` on every
    /// heap page buffer.
    pub page_cache_overflow_statistics: bool,
}

impl SqliteLockProfile {
    /// The profile Meerkat requires: none of the three locks.
    pub const REQUIRED: Self = Self {
        memory_statistics: false,
        shared_page_cache: false,
        page_cache_overflow_statistics: false,
    };

    /// The profile the linked SQLite was compiled with. Reads SQLite's
    /// compile-option table only; it never initializes SQLite.
    #[must_use]
    pub fn compiled() -> Self {
        Self {
            memory_statistics: !compile_option_used(c"DEFAULT_MEMSTATUS=0"),
            shared_page_cache: compile_option_used(c"ENABLE_MEMORY_MANAGEMENT"),
            page_cache_overflow_statistics: !compile_option_used(
                c"DISABLE_PAGECACHE_OVERFLOW_STATS",
            ),
        }
    }

    /// The profile this process runs with: [`Self::compiled`], with allocator
    /// statistics off when [`disable_memory_statistics_at_process_start`]
    /// applied.
    #[must_use]
    pub fn effective() -> Self {
        let mut profile = Self::compiled();
        if memory_statistics_outcome() == Some(MemoryStatisticsOutcome::Applied) {
            profile.memory_statistics = false;
        }
        profile
    }

    /// Whether this is [`Self::REQUIRED`].
    #[must_use]
    pub fn is_required(self) -> bool {
        self == Self::REQUIRED
    }
}

impl fmt::Display for SqliteLockProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = |on: bool| if on { "on" } else { "off" };
        write!(
            f,
            "memory statistics {}, shared page cache {}, page-cache overflow statistics {}",
            state(self.memory_statistics),
            state(self.shared_page_cache),
            state(self.page_cache_overflow_statistics),
        )
    }
}

/// The process runs SQLite with at least one process-wide lock in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "SQLite runs with process-wide locks ({effective}); build with \
     LIBSQLITE3_FLAGS=\"{REQUIRED_LIBSQLITE3_FLAGS}\""
)]
pub struct SqliteLockProfileMismatch {
    /// What this process runs with.
    pub effective: SqliteLockProfile,
}

/// Check that this process runs SQLite with [`SqliteLockProfile::REQUIRED`].
///
/// # Errors
///
/// [`SqliteLockProfileMismatch`] naming the effective profile when any of the
/// three locks is in use.
pub fn check_lock_profile() -> Result<(), SqliteLockProfileMismatch> {
    let effective = SqliteLockProfile::effective();
    if effective.is_required() {
        Ok(())
    } else {
        Err(SqliteLockProfileMismatch { effective })
    }
}

/// What [`disable_memory_statistics_at_process_start`] did: SQLite either
/// accepted the switch (and allocator statistics were on or already off) or
/// refused it.
///
/// Exhaustive on purpose: SQLite accepts or refuses the switch, and a build
/// has the statistics on or off, so these three are every outcome there is,
/// and a new one would fail to compile at each exhaustive match on it (the
/// storage doctor's report is one) instead of falling into a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryStatisticsOutcome {
    /// This build has allocator statistics on; they are now off.
    Applied,
    /// This build compiles allocator statistics off; nothing changed.
    AlreadyOff,
    /// SQLite was already in use, so it refused the switch: allocator
    /// statistics stay as built. A degrade, never a startup failure.
    TooLate,
}

impl fmt::Display for MemoryStatisticsOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Applied => "applied",
            Self::AlreadyOff => "already off",
            Self::TooLate => "too late",
        })
    }
}

static MEMORY_STATISTICS_OUTCOME: OnceLock<MemoryStatisticsOutcome> = OnceLock::new();

/// Turn SQLite's allocator statistics off for this process, and record the
/// outcome ([`memory_statistics_outcome`]). Only the first call acts; later
/// calls return the first outcome.
///
/// For builds without [`REQUIRED_LIBSQLITE3_FLAGS`]: allocator statistics are
/// the one process-wide lock with a runtime switch, and SQLite accepts the
/// switch only before it initializes. The shared page cache and the
/// page-cache overflow statistics need the build flags.
///
/// # Safety
///
/// `sqlite3_config` is not thread-safe. No other thread may use SQLite (open,
/// use or close a connection, or call any `sqlite3_*` function) while this
/// runs. Call it as the first statement of `main`, before any thread exists.
#[allow(unsafe_code)]
pub unsafe fn disable_memory_statistics_at_process_start() -> MemoryStatisticsOutcome {
    *MEMORY_STATISTICS_OUTCOME.get_or_init(|| {
        // SAFETY: the caller guarantees that no other thread uses SQLite
        // while this runs; SQLITE_CONFIG_MEMSTATUS takes one `int`.
        let rc = unsafe { ffi::sqlite3_config(ffi::SQLITE_CONFIG_MEMSTATUS, 0 as c_int) };
        if rc != ffi::SQLITE_OK {
            MemoryStatisticsOutcome::TooLate
        } else if SqliteLockProfile::compiled().memory_statistics {
            MemoryStatisticsOutcome::Applied
        } else {
            MemoryStatisticsOutcome::AlreadyOff
        }
    })
}

/// The recorded outcome of [`disable_memory_statistics_at_process_start`];
/// `None` when this process never called it.
#[must_use]
pub fn memory_statistics_outcome() -> Option<MemoryStatisticsOutcome> {
    MEMORY_STATISTICS_OUTCOME.get().copied()
}

static FIRST_OPEN: Once = Once::new();

/// At the first connection [`crate::profile`] opens in this process, warn
/// when SQLite runs with any of the three process-wide locks.
pub(crate) fn check_at_first_open() {
    FIRST_OPEN.call_once(|| {
        if let Some(warning) =
            lock_profile_warning(SqliteLockProfile::effective(), memory_statistics_outcome())
        {
            tracing::warn!("{warning}");
        }
    });
}

/// The first-open warning for `effective`, or `None` for the required
/// profile. It names the build setting that fixes it.
fn lock_profile_warning(
    effective: SqliteLockProfile,
    switch: Option<MemoryStatisticsOutcome>,
) -> Option<String> {
    if effective.is_required() {
        return None;
    }
    let switch = switch.map_or_else(|| "not called".to_string(), |outcome| outcome.to_string());
    Some(format!(
        "{}; concurrent connections in this process queue on these locks \
         (allocator statistics switch at process start: {switch})",
        SqliteLockProfileMismatch { effective }
    ))
}

fn compile_option_used(name: &CStr) -> bool {
    // SAFETY: sqlite3_compileoption_used only reads SQLite's static
    // compile-option table (it never initializes SQLite), and `name` is a
    // NUL-terminated string that outlives the call.
    #[allow(unsafe_code)]
    let used = unsafe { ffi::sqlite3_compileoption_used(name.as_ptr()) };
    used != 0
}

/// Whether this process has run the first-open check.
#[cfg(test)]
pub(crate) fn checked_at_first_open() -> bool {
    FIRST_OPEN.is_completed()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn first_open_warning_names_the_build_setting_unless_the_profile_is_required() {
        assert_eq!(
            lock_profile_warning(SqliteLockProfile::REQUIRED, None),
            None
        );

        let shared_page_cache = SqliteLockProfile {
            shared_page_cache: true,
            ..SqliteLockProfile::REQUIRED
        };
        let warning =
            lock_profile_warning(shared_page_cache, Some(MemoryStatisticsOutcome::Applied))
                .expect("a shared page cache is warned about");
        for expected in [
            "memory statistics off, shared page cache on, page-cache overflow statistics off",
            &format!("LIBSQLITE3_FLAGS=\"{REQUIRED_LIBSQLITE3_FLAGS}\""),
            "allocator statistics switch at process start: applied",
        ] {
            assert!(warning.contains(expected), "{expected:?} in {warning:?}");
        }

        let memory_statistics = SqliteLockProfile {
            memory_statistics: true,
            ..SqliteLockProfile::REQUIRED
        };
        let warning = lock_profile_warning(memory_statistics, None)
            .expect("allocator statistics are warned about");
        assert!(
            warning.contains("allocator statistics switch at process start: not called"),
            "{warning:?}"
        );
    }
}
