//! Store-level session hosting and the cross-process delivery wake (#1813).
//!
//! The hosting vocabulary (claims, capability, cold-delivery ownership) lives
//! in [`meerkat_core::session_hosting`]; this module adds what needs native
//! runtime machinery:
//!
//! - [`os_lock_if_trusted`]: a store's capability, downgraded when the OS
//!   lock cannot be trusted on its filesystem;
//! - `watch_delivery_store`: the shared SQLite change watch
//!   (`meerkat_sqlite::watch`, the contract the mob event bus uses) over a
//!   shared store's database file. Its ticks are a wake HINT: a coalesced
//!   file notification, or the bounded sweep when none arrived. A delivery
//!   owner answers a tick with cheap reads (the store's delivery generation,
//!   a try of the cold-delivery lock, the local routes of recipients it is
//!   waiting on) and reconciles only when one of them moved. A tick never
//!   grants permission, custody or expiry.

#[cfg(not(target_arch = "wasm32"))]
pub use meerkat_core::session_hosting::spawn_blocking_holding_claim;
pub use meerkat_core::session_hosting::{
    ColdDeliveryOwnership, HostingCapability, HostingClaim, HostingClaimUnavailable, HostingOwner,
    HostingPaths, HostingRefused, ServedElsewhere, SessionHostingAuthority, SessionServing,
    grant_session_hosting, local_session_serving, try_cold_delivery_ownership, with_write_hosting,
};

/// [`HostingCapability::OsLock`] over `paths`, unless the OS lock cannot be
/// trusted on their filesystem (network and userspace filesystems), in which
/// case the store reports [`HostingCapability::None`]: it never claims
/// multi-process safety it cannot prove. Explicit multi-process startup
/// refuses a `None` capability.
pub fn os_lock_if_trusted(paths: HostingPaths) -> HostingCapability {
    #[cfg(not(target_arch = "wasm32"))]
    {
        match native::lock_filesystem_is_local(&paths.hosting_lock_dir) {
            Ok(true) => HostingCapability::OsLock(std::sync::Arc::new(paths)),
            Ok(false) => {
                tracing::warn!(
                    hosting_lock_dir = %paths.hosting_lock_dir.display(),
                    "session hosting locks are unreliable on this filesystem (network or \
                     userspace); the runtime store reports single-process hosting"
                );
                HostingCapability::None
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    hosting_lock_dir = %paths.hosting_lock_dir.display(),
                    "cannot determine whether session hosting locks are reliable here; the \
                     runtime store reports single-process hosting"
                );
                HostingCapability::None
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = paths;
        HostingCapability::None
    }
}

/// The default sweep interval of `watch_delivery_store`: the bound on how
/// long a missed file notification can delay a cross-process delivery.
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-store"))]
pub const DELIVERY_STORE_SWEEP: std::time::Duration = meerkat_sqlite::watch::SQLITE_WATCH_SWEEP;
/// The default sweep interval of `watch_delivery_store` (a build without the
/// SQLite store never establishes the watch).
#[cfg(all(not(target_arch = "wasm32"), not(feature = "sqlite-store")))]
pub const DELIVERY_STORE_SWEEP: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(not(target_arch = "wasm32"))]
pub use native::{DeliveryStoreWatch, watch_delivery_store};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::{Path, PathBuf};

    /// Whether OS file locks under `path` are reliable across processes of
    /// this kernel: a local filesystem, not a network or userspace one.
    #[cfg(target_os = "linux")]
    pub(super) fn lock_filesystem_is_local(path: &Path) -> std::io::Result<bool> {
        const NFS: i64 = 0x6969;
        const SMB: i64 = 0x517B;
        const CIFS: i64 = 0xFF53_4D42;
        const SMB2: i64 = 0xFE53_4D42;
        const V9FS: i64 = 0x0102_1997;
        const AFS: i64 = 0x5346_414F;
        const CEPH: i64 = 0x00C3_6400;
        const FUSE: i64 = 0x6573_5546;
        let probe = existing_ancestor(path)?;
        // `fs_type_t` is a signed or unsigned long depending on the libc target.
        #[allow(clippy::unnecessary_cast)]
        let kind = nix::sys::statfs::statfs(&probe)
            .map_err(std::io::Error::from)?
            .filesystem_type()
            .0 as i64;
        Ok(![NFS, SMB, CIFS, SMB2, V9FS, AFS, CEPH, FUSE].contains(&kind))
    }

    #[cfg(target_os = "macos")]
    pub(super) fn lock_filesystem_is_local(path: &Path) -> std::io::Result<bool> {
        let probe = existing_ancestor(path)?;
        let stat = nix::sys::statfs::statfs(&probe).map_err(std::io::Error::from)?;
        Ok(!matches!(
            stat.filesystem_type_name(),
            "nfs" | "smbfs" | "afpfs" | "webdav" | "cifs" | "macfuse" | "osxfuse"
        ))
    }

    /// Other platforms (Windows): `LockFileEx` locks are released by the OS
    /// when the holding process exits or its handle closes, so the lock is
    /// trusted.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) fn lock_filesystem_is_local(_path: &Path) -> std::io::Result<bool> {
        Ok(true)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn existing_ancestor(path: &Path) -> std::io::Result<PathBuf> {
        let mut candidate = Some(path);
        while let Some(current) = candidate {
            if current.exists() {
                return Ok(current.to_path_buf());
            }
            candidate = current.parent();
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no existing ancestor of {}", path.display()),
        ))
    }

    /// A running wake watch over a shared store's database. Its tick
    /// receiver moves once per coalesced file notification and once per
    /// sweep interval without one. Dropping the watch stops it (the watch
    /// thread exits at its next wait).
    pub struct DeliveryStoreWatch {
        #[cfg(feature = "sqlite-store")]
        _watch: meerkat_sqlite::watch::SqliteChangeWatch,
        ticks: crate::tokio::sync::watch::Receiver<u64>,
    }

    impl std::fmt::Debug for DeliveryStoreWatch {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("DeliveryStoreWatch").finish_non_exhaustive()
        }
    }

    impl DeliveryStoreWatch {
        /// Observe ticks. Each is a hint to re-read durable state, nothing
        /// more.
        pub fn subscribe_ticks(&self) -> crate::tokio::sync::watch::Receiver<u64> {
            self.ticks.clone()
        }
    }

    /// Start the wake watch over `capability`'s store database, ticking at
    /// least every `sweep`. `Ok(None)` for a store without cross-process
    /// hosting; an error when the watch cannot be established (including a
    /// build without this crate's `sqlite-store` feature), which an owner
    /// reports as unavailable cross-process wake and explicit multi-process
    /// startup refuses.
    pub fn watch_delivery_store(
        capability: &meerkat_core::session_hosting::HostingCapability,
        sweep: std::time::Duration,
    ) -> Result<Option<DeliveryStoreWatch>, String> {
        let Some(paths) = capability.paths() else {
            return Ok(None);
        };
        #[cfg(not(feature = "sqlite-store"))]
        {
            let _ = (paths, sweep);
            Err(
                "this build has no SQLite store watch (meerkat-runtime's sqlite-store feature \
                 is off)"
                    .to_string(),
            )
        }
        #[cfg(feature = "sqlite-store")]
        watch_sqlite_delivery_store(paths, sweep)
    }

    #[cfg(feature = "sqlite-store")]
    fn watch_sqlite_delivery_store(
        paths: &meerkat_core::session_hosting::HostingPaths,
        sweep: std::time::Duration,
    ) -> Result<Option<DeliveryStoreWatch>, String> {
        let database = paths
            .database
            .as_ref()
            .ok_or_else(|| "the shared store names no database file to watch".to_string())?;
        let (ticks_tx, ticks) = crate::tokio::sync::watch::channel(0_u64);
        let watch = meerkat_sqlite::watch::start_sqlite_change_watch(
            database,
            "meerkat-delivery-store-watch",
            sweep,
            move |_tick| {
                if ticks_tx.is_closed() {
                    return meerkat_sqlite::watch::SqliteWatchControl::Stop;
                }
                ticks_tx.send_modify(|count| *count = count.wrapping_add(1));
                meerkat_sqlite::watch::SqliteWatchControl::Handled
            },
        )?;
        Ok(Some(DeliveryStoreWatch {
            _watch: watch,
            ticks,
        }))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn paths(root: &std::path::Path, database: std::path::PathBuf) -> HostingPaths {
        HostingPaths {
            hosting_lock_dir: root.join("hosting"),
            cold_delivery_lock: root.join("delivery").join("cold-delivery.lock"),
            database: Some(database),
        }
    }

    #[test]
    fn a_local_store_gets_os_lock_hosting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let database = dir.path().join("runtime.sqlite3");
        assert!(os_lock_if_trusted(paths(dir.path(), database)).is_cross_process());
    }

    #[cfg(feature = "sqlite-store")]
    #[tokio::test]
    async fn a_store_file_write_ticks_the_delivery_store_watch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let database = dir.path().join("runtime.sqlite3");
        std::fs::write(&database, b"").expect("seed file");
        let capability =
            HostingCapability::OsLock(std::sync::Arc::new(paths(dir.path(), database.clone())));
        let watch = watch_delivery_store(&capability, DELIVERY_STORE_SWEEP)
            .expect("watch starts")
            .expect("an OsLock store is watched");
        let mut ticks = watch.subscribe_ticks();
        std::fs::write(&database, b"commit").expect("write");
        tokio::time::timeout(std::time::Duration::from_secs(10), ticks.changed())
            .await
            .expect("the write is observed within the hang guard")
            .expect("watch open");
    }

    #[cfg(feature = "sqlite-store")]
    #[test]
    fn process_local_stores_are_never_watched() {
        assert!(matches!(
            watch_delivery_store(&HostingCapability::ProcessLocal, DELIVERY_STORE_SWEEP),
            Ok(None)
        ));
    }
}
