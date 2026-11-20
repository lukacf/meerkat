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
//! A test that reopens a state directory in its own process calls
//! [`predecessor_custody_released`] after it dropped the first lifetime's
//! handles and before the next boot.
//!
//! # What the wait reads
//!
//! The kernel's lock table (`/proc/locks`, Linux), as the probe does. It takes
//! no lock, does not open the custody file and writes nothing. A try-lock
//! would tell as much, but for an instant it would be a holder itself, and
//! Core takes its claim without waiting. Where there is no such table, or the
//! table cannot name the file (btrfs subvolumes, overlayfs), nothing can be
//! read without taking the lock, and the wait returns at once.

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
    let mut custody = state_dir
        .join(meerkat_meta::storage_layout::RUNTIME_DB_FILE_NAME)
        .into_os_string();
    custody.push(EXECUTION_CUSTODY_SUFFIX);
    let custody = Path::new(&custody);
    let started = Instant::now();
    let mut seen_held = false;
    let mut free_readings = 0;
    while free_readings < FREE_READINGS {
        match custody_holder_named(custody) {
            Some(false) => free_readings += 1,
            Some(true) => {
                seen_held = true;
                free_readings = 0;
                if started.elapsed() >= CUSTODY_WAIT {
                    eprintln!(
                        "the predecessor's execution custody was still held after {:?}; the \
                         reopen goes on to the boot's own probe",
                        started.elapsed()
                    );
                    return;
                }
                tokio::time::sleep(CUSTODY_POLL).await;
            }
            None => return,
        }
    }
    if seen_held {
        eprintln!(
            "the reopen waited {:?} for the predecessor's execution custody",
            started.elapsed()
        );
    }
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
