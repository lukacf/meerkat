//! One SDK build per checkout at a time.
//!
//! The SDK parity, productization and reference-app rows build in this
//! checkout's sdk/ tree: `npm run validate` rebuilds sdk/typescript/dist
//! (build-cjs.js removes and recopies dist/cjs), and `pip install sdk/python`
//! builds in sdk/python. Run together, one row's build removes files another
//! row is reading. Each row holds this exclusive lock for its whole run, so
//! they run one after another whatever the test runner schedules.

use std::hash::{Hash, Hasher};
use std::path::Path;

/// Held for a row's whole run; dropping it lets the next row build.
pub(crate) struct SdkBuildLock {
    _file: std::fs::File,
}

/// Wait for, then take, the exclusive SDK build lock of the checkout at
/// `repo_root`. The lock file is keyed by the checkout's canonical path, so
/// rows of one checkout exclude each other and other checkouts do not wait.
pub(crate) fn lock_sdk_build(repo_root: &Path) -> SdkBuildLock {
    let root = repo_root
        .canonicalize()
        .unwrap_or_else(|err| panic!("resolve the checkout root {}: {err}", repo_root.display()));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hasher);
    let path = std::env::temp_dir().join(format!(
        "meerkat-meta-sdk-build-{:016x}.lock",
        hasher.finish()
    ));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .unwrap_or_else(|err| panic!("open the SDK build lock {}: {err}", path.display()));
    file.lock()
        .unwrap_or_else(|err| panic!("take the SDK build lock {}: {err}", path.display()));
    SdkBuildLock { _file: file }
}
