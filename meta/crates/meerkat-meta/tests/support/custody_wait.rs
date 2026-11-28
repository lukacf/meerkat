//! The wait a reopen in the same process makes for its predecessor's
//! execution custody.
//!
//! # Why this file exists
//!
//! A row that shuts a runtime down and builds the next one on the same state
//! directory, in the same process, waits here until the first lifetime's
//! execution custody (the lock on `runtime.sqlite.execution-custody`) is
//! released, so that its next boot starts on a free lock.
//!
//! # The contract
//!
//! A test that reopens a state directory in its own process reads the first
//! lifetime's shutdown report with [`teardown_settled`], and calls
//! [`predecessor_custody_released`] after it dropped the first lifetime's
//! handles and before the next boot. The wait is for a release that comes
//! late after a teardown that settled. A teardown that failed or did not
//! settle fails the row at its report, before any wait.
//!
//! # What the wait reads
//!
//! The kernel's lock table (`/proc/locks`, Linux), as the probe does. It takes
//! no lock, does not open the custody file and writes nothing. A try-lock
//! would tell as much, but for an instant it would be a holder itself, and
//! Core takes its claim without waiting. Where there is no such table, or the
//! table cannot name the file (btrfs subvolumes, overlayfs), nothing can be
//! read without taking the lock, and the wait returns at once.
//!
//! # What the wait reports
//!
//! [`predecessor_custody_wait`] is the same wait and returns what its own
//! readings saw, a [`CustodyWait`]. Only [`CustodyWait::HeldThenReleased`]
//! says a release was observed. A caller that checks the wait itself reads
//! that report; a reading it makes on its own, before or after, can differ
//! from what the wait read. [`wait_for_release`] is the loop over any source
//! of readings, for a row that scripts them.

use std::path::Path;
use std::time::{Duration, Instant};

/// Suffix of Core's execution custody lock file beside its runtime database.
const EXECUTION_CUSTODY_SUFFIX: &str = ".execution-custody";

/// How long a reopen waits. A lock still held then is left to the boot, which
/// answers for itself.
const CUSTODY_WAIT: Duration = Duration::from_secs(10);

/// The pause between two readings while the lock is held.
const CUSTODY_POLL: Duration = Duration::from_millis(10);

/// The readings in a row that must name no holder, as many as the probe
/// makes. One read of the table can skip a held entry (the kernel hands it
/// out a page at a time), so one reading without a holder is not yet a free
/// lock.
const FREE_READINGS: usize = 3;

/// Wait, at most [`CUSTODY_WAIT`], until nothing holds the execution custody
/// of the runtime store in `state_dir`. Call it after the predecessor's
/// handles dropped and before the successor's boot. It never fails: a lock
/// still held at the bound, or one that cannot be read, leaves the answer to
/// the boot's own probe.
pub async fn predecessor_custody_released(state_dir: &Path) {
    predecessor_custody_wait(state_dir).await;
}

/// What a wait saw of the execution custody, by its own readings and nothing
/// else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustodyWait {
    /// A reading named a holder, and later [`FREE_READINGS`] readings in a
    /// row named none: the wait saw the lock held and then free.
    HeldThenReleased,
    /// No reading named a holder: the first [`FREE_READINGS`] readings named
    /// none. Nothing held the lock, or the table cannot name the file, or
    /// each of those readings skipped a held entry. No release was observed.
    NeverSeen,
    /// A reading could not be made, and the wait ended at once, whatever it
    /// had read before. No release was observed.
    Unavailable,
    /// A holder was still named at the bound.
    TimedOut,
}

/// [`predecessor_custody_released`], returning what the wait's own readings
/// saw.
pub async fn predecessor_custody_wait(state_dir: &Path) -> CustodyWait {
    let mut custody = state_dir
        .join(meerkat_meta::storage_layout::RUNTIME_DB_FILE_NAME)
        .into_os_string();
    custody.push(EXECUTION_CUSTODY_SUFFIX);
    let custody = Path::new(&custody);
    wait_for_release(CUSTODY_WAIT, || custody_holder_named(custody)).await
}

/// The wait itself, for at most `bound`, over `reading`: `Some(true)` when a
/// holder is named, `Some(false)` when none is, `None` when the reading could
/// not be made. It ends after [`FREE_READINGS`] readings in a row that name
/// no holder, pauses [`CUSTODY_POLL`] after each reading that names one, and
/// ends at once on a reading that could not be made.
pub async fn wait_for_release(
    bound: Duration,
    mut reading: impl FnMut() -> Option<bool>,
) -> CustodyWait {
    let started = Instant::now();
    let mut seen_held = false;
    let mut free_readings = 0;
    while free_readings < FREE_READINGS {
        match reading() {
            Some(false) => free_readings += 1,
            Some(true) => {
                seen_held = true;
                free_readings = 0;
                if started.elapsed() >= bound {
                    eprintln!(
                        "the predecessor's execution custody was still held after {:?}; the \
                         reopen goes on to the boot's own probe",
                        started.elapsed()
                    );
                    return CustodyWait::TimedOut;
                }
                tokio::time::sleep(CUSTODY_POLL).await;
            }
            None => return CustodyWait::Unavailable,
        }
    }
    if seen_held {
        eprintln!(
            "the reopen waited {:?} for the predecessor's execution custody",
            started.elapsed()
        );
        CustodyWait::HeldThenReleased
    } else {
        CustodyWait::NeverSeen
    }
}

/// Fail the row unless `report` is that of a teardown that settled: the mob
/// stopped, its terminal shutdown completed with every member unregistered
/// ([`mob_teardown_settled`]), and the runtime's cleanup completed, which for
/// a runtime with durable child mobs means their root was released. Call it
/// on the report of the shutdown a reopen follows, before the wait: a member
/// the teardown could not settle keeps the execution custody until it ends,
/// and the wait must not turn that into a passing row.
#[track_caller]
pub fn teardown_settled(report: &meerkat_meta::UnifiedRuntimeShutdownReport) {
    mob_teardown_settled(report);
    assert!(
        report.cleanup_completed(),
        "the shutdown before a reopen did not complete its cleanup: {report:?}"
    );
}

/// The mob's part of [`teardown_settled`], for a rig that stands its durable
/// child root down again before it reads the whole report: the mob stopped,
/// and its terminal shutdown completed with every member unregistered.
#[track_caller]
pub fn mob_teardown_settled(report: &meerkat_meta::UnifiedRuntimeShutdownReport) {
    assert!(
        report.mob_stop.is_ok(),
        "the shutdown before a reopen did not stop its mob: {report:?}"
    );
    assert!(
        matches!(
            &report.mob_terminal_shutdown,
            meerkat_meta::MobTerminalShutdownOutcome::Completed(mob) if mob.is_clean()
        ),
        "the shutdown before a reopen did not settle every member: {report:?}"
    );
}

/// Whether the kernel's lock table names a holder of `custody`: `None` when
/// that cannot be read. A custody file that does not exist has no holder.
#[cfg(target_os = "linux")]
pub fn custody_holder_named(custody: &Path) -> Option<bool> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = match std::fs::metadata(custody) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(false),
        Err(_) => return None,
    };
    // How the table names a file: device major and minor (hex) and inode.
    let device = metadata.dev();
    let major = ((device >> 8) & 0xfff) | ((device >> 32) & 0xffff_f000);
    let minor = (device & 0xff) | ((device >> 12) & 0xffff_ff00);
    let file_id = format!("{major:02x}:{minor:02x}:{}", metadata.ino());
    let table = std::fs::read_to_string("/proc/locks").ok()?;
    // "<n>: FLOCK ADVISORY READ <pid> <major>:<minor>:<inode> <start> <end>".
    // A waiter's line has "->" after the index; a waiter holds nothing.
    Some(table.lines().any(|line| {
        let mut fields = line.split_whitespace().skip(1);
        match fields.next() {
            None | Some("->") => false,
            Some(_) => fields.nth(3) == Some(file_id.as_str()),
        }
    }))
}

/// No kernel table of lock holders: nothing reads the lock without taking it.
#[cfg(not(target_os = "linux"))]
fn custody_holder_named(_custody: &Path) -> Option<bool> {
    None
}
