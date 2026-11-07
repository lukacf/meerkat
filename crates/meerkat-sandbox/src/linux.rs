//! Native Linux lowering of the existing immutable confinement requirement.
//!
//! Restricted paths use an empty private mount view, not content-only Landlock
//! grants. No host procfs is exposed. This module owns no permission policy or
//! child supervisor; it adapts the retained requirement and transfers the one
//! spawned child into NativeChild.
//!
//! Every granted path is canonical, with one exception: a read-only literal
//! file grant may name a symbolic link as its final component
//! (`/etc/resolv.conf` on a systemd-resolved host), when the link's own text
//! alone names its regular-file target, in one hop, outside `/proc`, `/sys`
//! and `/dev`, and only root or the effective uid can replace the link
//! (`grantable_link_text`). The target is pinned at compile time and bound
//! read-only at the link's own path. Every launch checks that the link keeps
//! its text and still resolves to the pinned target. A target replaced under
//! its own name (a new file renamed into place, as systemd-resolved rewrites
//! its stub file) is captured again by the same rules and pinned in its place
//! (`Mount::bind`); a link replaced with other text, or a target replaced by
//! anything those rules refuse, refuses the launch. Setup reopens the target
//! only while it is the object the launch validated, which the launch holds
//! until its setup is done, so no other object can take its identity
//! meanwhile.
//!
//! Every granted path, the baseline included, is opened by its canonical name
//! with no symbolic link in any component (`open_no_symlinks`): when the
//! requirement compiles, when a final link's target is captured again, and at
//! setup. A writer of a directory partway down cannot swap a link in and get
//! another object pinned or bound.
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use meerkat_core::confinement::{FilesystemAccess, IpNetworkAccess, PathAccess};
use nix::libc;
use tokio::process::{ChildStderr, ChildStdout};
use tokio::signal::unix::{SignalKind, signal};

use crate::native_stdio::{duplicate, inherit, input, output};
use crate::{ConfinementRefusal, ExecutionConfinement, NativeChild, ProcessLaunchSpec, SpawnIo};

// These are compatibility resources, not a bind of the host root. Missing
// optional distribution paths are omitted; /bin/sh and the target still have
// to resolve inside this view at actual exec. No /proc, /sys, home or scratch.
const BASELINE_DIRECTORIES: &[&str] = &[
    "/bin",
    "/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/lib",
    "/lib64",
    "/usr/lib",
    "/usr/lib64",
];
const BASELINE_FILES: &[&str] = &[
    "/etc/ld.so.cache",
    "/dev/null",
    "/dev/random",
    "/dev/urandom",
];
const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;
// Linux UAPI asm-generic/fcntl.h; libc does not expose these on both of the
// supported 64-bit architectures. These are syscall command numbers only.
const F_SETOWN_EX: libc::c_int = 15;
const F_SETSIG: libc::c_int = 10;
// A probe child whose setup step fails exits with this base plus the step's
// number, so the refused step can be read from its exit status (for example
// in `strace -f`). The refusal itself stays BackendUnavailable.
const PROBE_STEP_STATUS_BASE: i32 = 64;

/// The steps of `LinuxPolicy::apply`, numbered for the probe's exit status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupStep {
    NoNewPrivileges = 1,
    Unshare = 2,
    Setgroups = 3,
    UidMap = 4,
    GidMap = 5,
    PrivatePropagation = 6,
    EnterRoot = 7,
    MountSource = 8,
    MountIdentity = 9,
    StageMount = 10,
    StageOpen = 11,
    MountTarget = 12,
    MountBind = 13,
    MountRemount = 14,
    StageRemount = 15,
    Chroot = 16,
    EnterChroot = 17,
    Securebits = 18,
    Capabilities = 19,
    CloseRange = 20,
    Seccomp = 21,
}

impl SetupStep {
    fn probe_status(self) -> i32 {
        PROBE_STEP_STATUS_BASE + self as i32
    }

    fn failed(self) -> impl Fn(i32) -> SetupFailure {
        move |errno| SetupFailure { step: self, errno }
    }
}

/// A failed setup step and its OS error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SetupFailure {
    step: SetupStep,
    errno: i32,
}

struct Mount {
    path: PathBuf,
    // A read-only file grant whose final component is a symbolic link (see
    // `grantable_link_text`): the link's text at capture, which the link must
    // keep at every launch.
    link: Option<PathBuf>,
    canonical: PathBuf,
    // The canonical name, opened again in the new mount namespace at setup.
    source: CString,
    // The canonical name's components, prebuilt for the symlink-free walk
    // setup falls back to on a kernel without openat2 (`open_no_symlinks`).
    // `components` holds the granted path's, which differ for a final link
    // and a baseline alias.
    source_components: Vec<CString>,
    components: Vec<CString>,
    destination: CString,
    directory: bool,
    writable: bool,
    device: bool,
    // The pinned object. Only a final link's target replaced under its own
    // name is ever pinned again (`Mount::bind`); a launch's `Bound` keeps the
    // object it validated alive past that.
    pin: Mutex<Pin>,
}

/// The host object a mount pins.
struct Pin {
    file: Arc<File>,
    identity: (u64, u64),
    // fstat (st_dev, st_ino, st_rdev) of the pinned object. st_rdev also
    // tells the three baseline devices apart; it is zero for directories and
    // files.
    pinned: (libc::dev_t, libc::ino_t, libc::dev_t),
    ancestors: Vec<(PathBuf, u64, u64)>,
    // The host mount is noexec, which the bind remount must keep.
    noexec: bool,
}

/// What one launch binds of a mount, copied from its pin when the launch is
/// validated, before fork, so setup reads no lock: the identity the reopened
/// source must have, and the host mount's noexec. It also holds the pinned
/// object itself until the launch drops it, after its setup is done, so the
/// identity cannot pass to another object meanwhile: once a re-capture drops
/// the pin of a replaced file, the file's last reference would otherwise go,
/// and a filesystem may give its inode number to the next object made there
/// (ext4 often does).
#[derive(Clone, Debug)]
struct Bound {
    _held: Arc<File>,
    pinned: (libc::dev_t, libc::ino_t, libc::dev_t),
    noexec: bool,
}

/// Mechanical compiled resources. Every file descriptor pins the exact host
/// object selected at setup. Per-launch checks cannot retarget these handles;
/// the one object they pin again is a final link's target replaced under its
/// own name, by the capture-time rules (`Mount::bind`). Setup binds a source
/// opened again by name in the new mount namespace, and only while it is
/// still the object the launch validated.
pub(super) struct LinuxPolicy {
    mounts: Vec<Mount>,
    filter: Vec<libc::sock_filter>,
    uid_map: Vec<u8>,
    gid_map: Vec<u8>,
}

impl LinuxPolicy {
    pub(super) fn compile(
        requirement: &ExecutionConfinement,
    ) -> Result<Arc<Self>, ConfinementRefusal> {
        let spec = requirement.specification();
        let architecture = native_audit_arch().ok_or(ConfinementRefusal::UnsupportedRequirement)?;
        if spec.require_descendant_termination
            || !spec.deny_read.is_empty()
            || !spec.deny_write.is_empty()
            || !spec.unix_connect.is_empty()
            || matches!(&spec.network, IpNetworkAccess::Connect(values) if !values.is_empty())
        {
            return Err(ConfinementRefusal::UnsupportedRequirement);
        }
        let mut mounts = Vec::new();
        match (&spec.read, &spec.write) {
            (FilesystemAccess::Paths(read), FilesystemAccess::Paths(write)) => {
                // This first backend supports whole, existing disjoint read
                // roots. A writable root must be exactly one read subtree.
                // It does not reinterpret write-only grants as read grants.
                for writable in write {
                    if !matches!(writable, PathAccess::Subtree(_)) || !read.contains(writable) {
                        return Err(ConfinementRefusal::UnsupportedRequirement);
                    }
                }
                for access in read {
                    let path = access.path();
                    if reserved_read_root(path) {
                        return Err(ConfinementRefusal::UnsupportedRequirement);
                    }
                    let mount = Mount::capture(path, false, write.contains(access))?;
                    if matches!(access, PathAccess::Subtree(_)) != mount.directory {
                        return Err(ConfinementRefusal::UnsupportedRequirement);
                    }
                    push_disjoint(&mut mounts, mount)?;
                }
                for path in BASELINE_DIRECTORIES.iter().chain(BASELINE_FILES) {
                    if Path::new(path)
                        .try_exists()
                        .map_err(|_| ConfinementRefusal::BackendUnavailable)?
                    {
                        push_disjoint(&mut mounts, Mount::capture(Path::new(path), true, false)?)?;
                    }
                }
            }
            // Unrestricted host procfs would reintroduce /proc/<pid>/mem and
            // descriptor/process access outside the syscall restrictions.
            // Mixed dimensions also need an exact metadata/write lowering.
            _ => return Err(ConfinementRefusal::UnsupportedRequirement),
        }
        let policy = Arc::new(Self {
            mounts,
            filter: filter(
                architecture,
                matches!(spec.network, IpNetworkAccess::Unrestricted),
            ),
            // Only fixed decimal mapping data is read after fork.
            uid_map: format!("0 {} 1\n", unsafe { libc::geteuid() }).into_bytes(),
            gid_map: format!("0 {} 1\n", unsafe { libc::getegid() }).into_bytes(),
        });
        let bound = policy.bind_mounts()?;
        policy.probe(&bound)?;
        Ok(policy)
    }

    /// Checks every mount for a launch (see [`Mount::bind`]) and the staging
    /// root, and returns what the launch binds of each mount.
    fn bind_mounts(&self) -> Result<Vec<Bound>, ConfinementRefusal> {
        let bound = self
            .mounts
            .iter()
            .map(Mount::bind)
            .collect::<Result<Vec<_>, _>>()?;
        {
            let stage = std::fs::symlink_metadata("/tmp")
                .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
            if !stage.is_dir() || stage.file_type().is_symlink() {
                return Err(ConfinementRefusal::UnsupportedRequirement);
            }
        }
        Ok(bound)
    }

    pub(super) fn validate_launch(
        &self,
        launch: &ProcessLaunchSpec,
    ) -> Result<(), ConfinementRefusal> {
        self.launch_bindings(launch)?;
        Ok(())
    }

    /// [`Self::bind_mounts`] for `launch`, whose program and working
    /// directory must be visible in its view.
    fn launch_bindings(
        &self,
        launch: &ProcessLaunchSpec,
    ) -> Result<Vec<Bound>, ConfinementRefusal> {
        let bound = self.bind_mounts()?;
        {
            let visible = |path: &Path| {
                self.mounts.iter().any(|mount| {
                    path == mount.path || (mount.directory && path.starts_with(&mount.path))
                })
            };
            if !visible(&launch.program) || !visible(&launch.directory) {
                return Err(ConfinementRefusal::UnsupportedRequirement);
            }
        }
        Ok(bound)
    }

    // Capability setup runs once per compiled host profile. It never executes
    // the requested target. Only the exact probe PID is waited/killed here.
    // `bound` is what the probe binds of each mount, validated before fork;
    // the caller holds it until the probe returns, its child gone.
    fn probe(&self, bound: &[Bound]) -> Result<(), ConfinementRefusal> {
        // One slot per mount for setup's reopened sources, allocated before
        // fork: the child only writes its own copy.
        let mut sources = vec![-1; self.mounts.len()];
        // A launch site of the confinement backend: the capability probe.
        #[allow(clippy::disallowed_methods)]
        let pid = unsafe { libc::fork() };
        if pid == -1 {
            return Err(ConfinementRefusal::BackendUnavailable);
        }
        if pid == 0 {
            let status = match self.apply(bound, &mut sources) {
                Ok(()) => 0,
                Err(failure) => failure.step.probe_status(),
            };
            unsafe { libc::_exit(status) };
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut status = 0;
        loop {
            let result = unsafe { libc::waitpid(pid, &raw mut status, libc::WNOHANG) };
            if result == pid {
                return if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
                    Ok(())
                } else {
                    Err(ConfinementRefusal::BackendUnavailable)
                };
            }
            if result == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ECHILD) {
                    // Lost wait ownership: never signal the numeric PID again.
                    return Err(ConfinementRefusal::BackendUnavailable);
                }
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        unsafe { libc::kill(pid, libc::SIGKILL) };
        loop {
            if unsafe { libc::waitpid(pid, &raw mut status, 0) } != -1
                || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
            {
                break;
            }
        }
        Err(ConfinementRefusal::BackendUnavailable)
    }

    // Called only between fork and exec, or in the disposable capability probe.
    // This path performs raw syscalls and reads preallocated data only. No
    // allocation, host lock, callback, Rust destructor or async operation.
    // `sources` is preallocated with one slot per mount, and `bound` holds
    // what the launch validated of each mount.
    fn apply(&self, bound: &[Bound], sources: &mut [i32]) -> Result<(), SetupFailure> {
        raw(unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) })
            .map_err(SetupStep::NoNewPrivileges.failed())?;
        {
            raw(unsafe {
                libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS | libc::CLONE_NEWIPC)
            })
            .map_err(SetupStep::Unshare.failed())?;
            write_file(c"/proc/self/setgroups", b"deny").map_err(SetupStep::Setgroups.failed())?;
            write_file(c"/proc/self/uid_map", &self.uid_map).map_err(SetupStep::UidMap.failed())?;
            write_file(c"/proc/self/gid_map", &self.gid_map).map_err(SetupStep::GidMap.failed())?;
            // Private propagation precedes every bind or filesystem mutation.
            raw(unsafe {
                libc::mount(
                    std::ptr::null(),
                    c"/".as_ptr(),
                    std::ptr::null(),
                    libc::MS_REC | libc::MS_PRIVATE,
                    std::ptr::null(),
                )
            })
            .map_err(SetupStep::PrivatePropagation.failed())?;
            raw(unsafe { libc::chdir(c"/".as_ptr()) }).map_err(SetupStep::EnterRoot.failed())?;
            // The kernel binds only mounts of the caller's own namespace, so
            // a descriptor pinned in the host namespace cannot be a bind
            // source here. Each source is opened again by its canonical name,
            // before the staging tmpfs covers /tmp, and is used only while it
            // is still the object the launch validated.
            if sources.len() != self.mounts.len() || bound.len() != self.mounts.len() {
                return Err(SetupFailure {
                    step: SetupStep::MountSource,
                    errno: libc::EINVAL,
                });
            }
            for ((source, mount), bound) in sources.iter_mut().zip(&self.mounts).zip(bound) {
                *source = mount.reopen(bound.pinned)?;
            }
            raw(unsafe {
                libc::mount(
                    c"tmpfs".as_ptr(),
                    c"/tmp".as_ptr(),
                    c"tmpfs".as_ptr(),
                    libc::MS_NOSUID | libc::MS_NODEV,
                    c"mode=0755,size=16m".as_ptr().cast(),
                )
            })
            .map_err(SetupStep::StageMount.failed())?;
            let root = raw(unsafe {
                libc::open(
                    c"/tmp".as_ptr(),
                    libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
                )
            })
            .map_err(SetupStep::StageOpen.failed())?;
            for ((mount, &source), bound) in self.mounts.iter().zip(sources.iter()).zip(bound) {
                mount.install(root, source, bound.noexec)?;
            }
            raw(unsafe { libc::close(root) }).map_err(SetupStep::StageOpen.failed())?;
            // The synthetic ancestors are not writable scratch. Child mounts
            // keep their separately assigned read/write flags.
            raw(unsafe {
                libc::mount(
                    std::ptr::null(),
                    c"/tmp".as_ptr(),
                    std::ptr::null(),
                    libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
                    std::ptr::null(),
                )
            })
            .map_err(SetupStep::StageRemount.failed())?;
            raw(unsafe { libc::chroot(c"/tmp".as_ptr()) }).map_err(SetupStep::Chroot.failed())?;
            raw(unsafe { libc::chdir(c"/".as_ptr()) }).map_err(SetupStep::EnterChroot.failed())?;
        }
        // Namespaced root may not regain capabilities at exec.
        {
            const LOCKED_NO_ROOT_OR_AMBIENT: libc::c_ulong = 1 | 2 | 4 | 8 | 32 | 64 | 128;
            raw(unsafe {
                libc::prctl(libc::PR_SET_SECUREBITS, LOCKED_NO_ROOT_OR_AMBIENT, 0, 0, 0)
            })
            .map_err(SetupStep::Securebits.failed())?;
        }
        #[repr(C)]
        struct CapHeader {
            version: u32,
            pid: i32,
        }
        #[repr(C)]
        struct CapData {
            effective: u32,
            permitted: u32,
            inheritable: u32,
        }
        let header = CapHeader {
            version: 0x2008_0522,
            pid: 0,
        };
        let data = [
            CapData {
                effective: 0,
                permitted: 0,
                inheritable: 0,
            },
            CapData {
                effective: 0,
                permitted: 0,
                inheritable: 0,
            },
        ];
        raw_long(unsafe {
            libc::syscall(libc::SYS_capset, std::ptr::addr_of!(header), data.as_ptr())
        })
        .map_err(SetupStep::Capabilities.failed())?;
        // Mark, rather than close, std::process's private exec-error pipe.
        // It must survive failed setup and disappear with all ambient handles
        // at the first exec. A declared custody fd3 is installed afterward.
        raw_long(unsafe {
            libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, CLOSE_RANGE_CLOEXEC)
        })
        .map_err(SetupStep::CloseRange.failed())?;
        let program = libc::sock_fprog {
            len: self.filter.len() as u16,
            filter: self.filter.as_ptr().cast_mut(),
        };
        raw(unsafe {
            libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                std::ptr::addr_of!(program),
            )
        })
        .map_err(SetupStep::Seccomp.failed())?;
        Ok(())
    }
}

fn push_disjoint(mounts: &mut Vec<Mount>, mount: Mount) -> Result<(), ConfinementRefusal> {
    if mounts
        .iter()
        .any(|old| old.path.starts_with(&mount.path) || mount.path.starts_with(&old.path))
    {
        return Err(ConfinementRefusal::UnsupportedRequirement);
    }
    mounts.push(mount);
    Ok(())
}

/// Whether a read grant of `path` is refused whatever it holds: `/` itself,
/// and `/proc`, `/sys`, `/dev` and anything in them. A granted path is held
/// to it when the requirement compiles, and a final link's target when the
/// link is captured.
fn reserved_read_root(path: &Path) -> bool {
    path == Path::new("/")
        || ["/proc", "/sys", "/dev"]
            .iter()
            .any(|root| path.starts_with(root))
}

fn validate_baseline_kind(
    path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<(), ConfinementRefusal> {
    let minor = if path == Path::new("/dev/null") {
        Some(3)
    } else if path == Path::new("/dev/random") {
        Some(8)
    } else if path == Path::new("/dev/urandom") {
        Some(9)
    } else {
        None
    };
    let valid = if let Some(minor) = minor {
        metadata.file_type().is_char_device() && metadata.rdev() == libc::makedev(1, minor)
    } else if BASELINE_DIRECTORIES
        .iter()
        .any(|expected| path == Path::new(expected))
    {
        metadata.is_dir()
    } else {
        path == Path::new("/etc/ld.so.cache") && metadata.is_file()
    };
    if valid {
        Ok(())
    } else {
        Err(ConfinementRefusal::UnsupportedRequirement)
    }
}

/// The text of the symbolic link at `path`, the final component of a
/// read-only file grant, when the link may stand for `canonical`, the path it
/// resolves to:
///
/// - One hop: the link's own text, resolved lexically from the link's
///   directory, is `canonical` itself, so the text alone names the target.
///   A text the kernel resolves elsewhere through another link (a link to a
///   link, a target under a linked directory) is refused, whoever can
///   replace that link.
/// - Only root or the effective uid can retarget the link: the link and
///   every directory from `/` down to its own are owned by one of them, and
///   none of those directories is writable by group or others unless it is
///   sticky (in a sticky directory only an entry's owner, the directory's
///   owner or root can rename or remove the entry).
///
/// Both rules read owners, modes and link text as the filesystem reports
/// them. On a FUSE or network filesystem those are whatever its server
/// says, so a link there is trusted as far as that server is.
///
/// `None` when the link may not, and on any metadata error.
fn grantable_link_text(path: &Path, canonical: &Path) -> Option<PathBuf> {
    grantable_link_text_for(path, canonical, unsafe { libc::geteuid() })
}

/// [`grantable_link_text`] with `user` as the one trusted owner besides root.
fn grantable_link_text_for(path: &Path, canonical: &Path, user: libc::uid_t) -> Option<PathBuf> {
    const STICKY: u32 = 0o1000;
    const GROUP_OR_OTHER_WRITE: u32 = 0o022;
    let directory = path.parent()?;
    if !path
        .components()
        .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return None;
    }
    let text = std::fs::read_link(path).ok()?;
    let mut target = directory.to_path_buf();
    for component in text.components() {
        match component {
            Component::RootDir => target = PathBuf::from("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                // `/..` is `/`, as for the kernel.
                target.pop();
            }
            Component::Normal(name) => target.push(name),
            Component::Prefix(_) => return None,
        }
    }
    if target != canonical {
        return None;
    }
    let trusted_owner =
        |metadata: &std::fs::Metadata| metadata.uid() == 0 || metadata.uid() == user;
    let trusted = std::fs::symlink_metadata(path)
        .is_ok_and(|link| link.file_type().is_symlink() && trusted_owner(&link))
        && directory.ancestors().all(|ancestor| {
            std::fs::symlink_metadata(ancestor).is_ok_and(|metadata| {
                metadata.is_dir()
                    && trusted_owner(&metadata)
                    && (metadata.mode() & GROUP_OR_OTHER_WRITE == 0
                        || metadata.mode() & STICKY != 0)
            })
        });
    trusted.then_some(text)
}

impl Mount {
    /// Captures one granted path. Every granted path is canonical, except
    /// that a read-only file grant may name a symbolic link as its final
    /// component (`/etc/resolv.conf` on a systemd-resolved host): its
    /// regular-file target, outside the reserved roots a granted path is
    /// held to ([`reserved_read_root`]), is pinned here and bound read-only
    /// at the link's own path, when [`grantable_link_text`] accepts the link,
    /// whose text is kept.
    fn capture(path: &Path, baseline: bool, writable: bool) -> Result<Self, ConfinementRefusal> {
        let canonical =
            std::fs::canonicalize(path).map_err(|_| ConfinementRefusal::UnsupportedRequirement)?;
        let link = if !baseline && canonical != path {
            if writable || reserved_read_root(&canonical) {
                return Err(ConfinementRefusal::UnsupportedRequirement);
            }
            Some(
                grantable_link_text(path, &canonical)
                    .ok_or(ConfinementRefusal::UnsupportedRequirement)?,
            )
        } else {
            None
        };
        // The canonical name and its components, for the symlink-free opens
        // here, at every re-capture and at setup.
        let source = CString::new(canonical.as_os_str().as_bytes())
            .map_err(|_| ConfinementRefusal::UnsupportedRequirement)?;
        let source_components =
            normal_components(&canonical).ok_or(ConfinementRefusal::UnsupportedRequirement)?;
        // No symbolic link in any component: a writer of a directory partway
        // down cannot swap a link in after canonicalize and get another
        // object pinned.
        let file = open_no_symlinks(
            &source,
            &source_components,
            libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
        .map(|descriptor| unsafe { File::from_raw_fd(descriptor) })
        .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
        let metadata = file
            .metadata()
            .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
        if baseline {
            validate_baseline_kind(path, &metadata)?;
        }
        let device = baseline
            && ["/dev/null", "/dev/random", "/dev/urandom"]
                .iter()
                .any(|device| path == Path::new(device));
        if !metadata.is_dir() && !metadata.is_file() && !device {
            return Err(ConfinementRefusal::UnsupportedRequirement);
        }
        // A final link stands only for a regular file.
        if (metadata.is_file() && metadata.nlink() != 1) || (link.is_some() && !metadata.is_file())
        {
            return Err(ConfinementRefusal::UnsupportedRequirement);
        }
        let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
        if unsafe { libc::fstatfs(file.as_raw_fd(), filesystem.as_mut_ptr()) } == -1 {
            return Err(ConfinementRefusal::BackendUnavailable);
        }
        let filesystem = unsafe { filesystem.assume_init() };
        // Do not expose a procfs/sysfs/control filesystem under a renamed host
        // mount path. Ordinary descendants are bound non-recursively below.
        if matches!(
            filesystem.f_type as u64,
            0x9fa0
                | 0x6265_6572
                | 0x6367_7270
                | 0x0027_e0eb
                | 0x6462_6720
                | 0x7472_6163
                | 0xcafe_4a11
        ) {
            return Err(ConfinementRefusal::UnsupportedRequirement);
        }
        let pinned =
            stat_identity(file.as_raw_fd()).map_err(|_| ConfinementRefusal::BackendUnavailable)?;
        // A namespaced remount cannot clear a flag the host mount has, so
        // the bind keeps the host mount's noexec.
        let mut mount_flags = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::fstatvfs(file.as_raw_fd(), mount_flags.as_mut_ptr()) } == -1 {
            return Err(ConfinementRefusal::BackendUnavailable);
        }
        let noexec = (unsafe { mount_flags.assume_init() }.f_flag & libc::ST_NOEXEC) != 0;
        let mut ancestors = Vec::new();
        for ancestor in canonical.ancestors().skip(1) {
            let value = std::fs::symlink_metadata(ancestor)
                .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
            if !value.is_dir() || value.file_type().is_symlink() {
                return Err(ConfinementRefusal::UnsupportedRequirement);
            }
            ancestors.push((ancestor.to_path_buf(), value.dev(), value.ino()));
        }
        let components = path
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(CString::new(value.as_bytes())),
                Component::RootDir => None,
                _ => Some(CString::new([0_u8])),
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ConfinementRefusal::UnsupportedRequirement)?;
        let destination = CString::new(
            Path::new("/tmp")
                .join(
                    path.strip_prefix("/")
                        .map_err(|_| ConfinementRefusal::UnsupportedRequirement)?,
                )
                .as_os_str()
                .as_bytes(),
        )
        .map_err(|_| ConfinementRefusal::UnsupportedRequirement)?;
        Ok(Self {
            path: path.to_path_buf(),
            link,
            canonical,
            source,
            source_components,
            components,
            destination,
            directory: metadata.is_dir(),
            writable,
            device,
            pin: Mutex::new(Pin {
                file: Arc::new(file),
                identity: (metadata.dev(), metadata.ino()),
                pinned,
                ancestors,
                noexec,
            }),
        })
    }

    /// Checks this mount for a launch and returns what the launch binds.
    ///
    /// A final link whose target was replaced under its own name (a new file
    /// renamed into place, as systemd-resolved rewrites its stub file) is
    /// captured again by the capture-time rules, unchanged, and the new
    /// target is pinned in its place, for this launch and every later one.
    /// Every other failed check refuses the launch: a link replaced with
    /// other text, a link that no longer reaches the same canonical path in
    /// one hop, a target replaced by anything the capture-time rules refuse,
    /// and any other grant whose object or directories were replaced.
    fn bind(&self) -> Result<Bound, ConfinementRefusal> {
        let mut pin = self.pin.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(refusal) = self.check(&pin) {
            *pin = self.recapture(&pin).ok_or(refusal)?;
        }
        Ok(Bound {
            _held: Arc::clone(&pin.file),
            pinned: pin.pinned,
            noexec: pin.noexec,
        })
    }

    /// Whether this mount still names `pin`'s object as captured: the
    /// granted path resolves to the same canonical path, a final link keeps
    /// its text, the canonical path's ancestors are the same directories, and
    /// the object at the canonical path is the pinned one.
    fn check(&self, pin: &Pin) -> Result<(), ConfinementRefusal> {
        let current = std::fs::canonicalize(&self.path)
            .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
        if current != self.canonical {
            return Err(ConfinementRefusal::BackendUnavailable);
        }
        if let Some(link) = &self.link
            && std::fs::read_link(&self.path).ok().as_ref() != Some(link)
        {
            return Err(ConfinementRefusal::BackendUnavailable);
        }
        for (path, device, inode) in &pin.ancestors {
            let current = std::fs::symlink_metadata(path)
                .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
            if !current.is_dir()
                || current.file_type().is_symlink()
                || (current.dev(), current.ino()) != (*device, *inode)
            {
                return Err(ConfinementRefusal::BackendUnavailable);
            }
        }
        let named = std::fs::symlink_metadata(&self.canonical)
            .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
        let pinned = pin
            .file
            .metadata()
            .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
        if named.file_type().is_symlink()
            || (named.dev(), named.ino()) != pin.identity
            || (pinned.dev(), pinned.ino()) != pin.identity
        {
            return Err(ConfinementRefusal::BackendUnavailable);
        }
        Ok(())
    }

    /// A new pin for a final link whose canonical path no longer names the
    /// pinned object: the link captured again by the capture-time rules,
    /// unchanged, accepted only while the link keeps its captured text and
    /// reaches the same canonical path. `None` for every other mount, and
    /// while the canonical path still names the pinned object.
    fn recapture(&self, pin: &Pin) -> Option<Pin> {
        let link = self.link.as_ref()?;
        let named = std::fs::symlink_metadata(&self.canonical).ok()?;
        if (named.dev(), named.ino()) == pin.identity {
            return None;
        }
        let fresh = Self::capture(&self.path, false, self.writable).ok()?;
        (fresh.link.as_ref() == Some(link) && fresh.canonical == self.canonical).then(|| {
            fresh
                .pin
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner)
        })
    }

    // Between fork and exec, in the new mount namespace: opens this mount's
    // object again by its canonical name, following no symbolic link in any
    // component, and returns it only while it is still `pinned`, the object
    // the launch validated.
    fn reopen(&self, pinned: (libc::dev_t, libc::ino_t, libc::dev_t)) -> Result<i32, SetupFailure> {
        let kind = if self.directory { libc::O_DIRECTORY } else { 0 };
        let source = open_no_symlinks(
            &self.source,
            &self.source_components,
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC | kind,
        )
        .map_err(SetupStep::MountSource.failed())?;
        match stat_identity(source) {
            Ok(identity) if identity == pinned => Ok(source),
            outcome => {
                unsafe { libc::close(source) };
                Err(SetupFailure {
                    step: SetupStep::MountIdentity,
                    errno: outcome.err().unwrap_or(libc::ESTALE),
                })
            }
        }
    }

    // Binds `source`, this mount's reopened object, at its destination and
    // closes it. `noexec` is the host mount's flag the launch validated.
    fn install(&self, root: i32, source: i32, noexec: bool) -> Result<(), SetupFailure> {
        let mut parent = raw(unsafe { libc::fcntl(root, libc::F_DUPFD_CLOEXEC, 3) })
            .map_err(SetupStep::MountTarget.failed())?;
        for (index, name) in self.components.iter().enumerate() {
            let final_component = index + 1 == self.components.len();
            let directory = !final_component || self.directory;
            if directory {
                let result = unsafe { libc::mkdirat(parent, name.as_ptr(), 0o755) };
                if result == -1 && errno() != libc::EEXIST {
                    return Err(SetupFailure {
                        step: SetupStep::MountTarget,
                        errno: errno(),
                    });
                }
            }
            let flags = if directory {
                libc::O_PATH | libc::O_DIRECTORY
            } else {
                libc::O_RDONLY | libc::O_CREAT | libc::O_EXCL
            };
            let next = raw(unsafe {
                libc::openat(
                    parent,
                    name.as_ptr(),
                    flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    0o600,
                )
            })
            .map_err(SetupStep::MountTarget.failed())?;
            raw(unsafe { libc::close(parent) }).map_err(SetupStep::MountTarget.failed())?;
            parent = next;
        }
        let mut path = [0_u8; 32];
        let path = fd_path(source, &mut path);
        // These target nodes were created in this private empty tmpfs. Roots
        // are disjoint, so no mounted source can replace an ancestor here.
        // Resolve the fresh path for remount rather than an FD to the covered
        // pre-bind inode, which would still refer to the old tmpfs mount.
        let destination = self.destination.as_ptr();
        raw(unsafe {
            libc::mount(
                path,
                destination,
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        })
        .map_err(SetupStep::MountBind.failed())?;
        raw(unsafe { libc::close(source) }).map_err(SetupStep::MountBind.failed())?;
        // Nonrecursive binds do not import hidden host submounts. The mounted
        // root cannot be replaced or renamed by the target. Read-only remount
        // also blocks metadata writes, not only file data writes.
        raw(unsafe {
            libc::mount(
                std::ptr::null(),
                destination,
                std::ptr::null(),
                Self::remount_flags(self.device, self.writable, noexec),
                std::ptr::null(),
            )
        })
        .map_err(SetupStep::MountRemount.failed())?;
        raw(unsafe { libc::close(parent) }).map_err(SetupStep::MountTarget.failed())?;
        Ok(())
    }

    // nosuid always, nodev except for the three devices, read-only unless
    // granted write. In this unprivileged namespace the host mount's flags
    // are locked and a remount that would clear one fails, so a noexec host
    // mount (such as the noexec /dev of stock Ubuntu) keeps noexec.
    fn remount_flags(device: bool, writable: bool, noexec: bool) -> libc::c_ulong {
        let mut flags = libc::MS_BIND | libc::MS_REMOUNT | libc::MS_NOSUID;
        if !device {
            flags |= libc::MS_NODEV;
        }
        if !writable {
            flags |= libc::MS_RDONLY;
        }
        if noexec {
            flags |= libc::MS_NOEXEC;
        }
        flags
    }
}

// Raw fstat identity of an open object, (st_dev, st_ino, st_rdev). Usable
// between fork and exec.
fn stat_identity(descriptor: i32) -> Result<(libc::dev_t, libc::ino_t, libc::dev_t), i32> {
    let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
    raw(unsafe { libc::fstat(descriptor, status.as_mut_ptr()) })?;
    let status = unsafe { status.assume_init() };
    Ok((status.st_dev, status.st_ino, status.st_rdev))
}

/// The normal components of the absolute `path`; `None` for a relative
/// path, a `.` or `..` component, or a NUL byte.
fn normal_components(path: &Path) -> Option<Vec<CString>> {
    if !path.is_absolute() {
        return None;
    }
    path.components()
        .filter(|component| !matches!(component, Component::RootDir))
        .map(|component| match component {
            Component::Normal(name) => CString::new(name.as_bytes()).ok(),
            _ => None,
        })
        .collect()
}

/// Opens the absolute canonical name `path`, whose normal `components` are
/// prebuilt, with `flags`, following no symbolic link in any component, so
/// a writer of a directory partway down cannot get another object opened by
/// swapping a link in. It uses openat2 with RESOLVE_NO_SYMLINKS, and on a
/// kernel without openat2 (ENOSYS only) the same walk by hand
/// ([`walk_no_symlinks`]); any other error refuses. A final component that
/// is a symbolic link opens as the link itself under O_PATH | O_NOFOLLOW,
/// both ways, for the caller's type or identity check to refuse. It
/// allocates nothing, so setup calls it between fork and exec. Returns the
/// descriptor or the errno.
fn open_no_symlinks(path: &CStr, components: &[CString], flags: libc::c_int) -> Result<i32, i32> {
    match openat2_no_symlinks(path, flags) {
        Err(libc::ENOSYS) => walk_no_symlinks(components, flags),
        outcome => outcome,
    }
}

/// [`open_no_symlinks`] by openat2 with RESOLVE_NO_SYMLINKS.
fn openat2_no_symlinks(path: &CStr, flags: libc::c_int) -> Result<i32, i32> {
    // `open_how` is non-exhaustive: zeroed, then its fields set.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = flags as u64;
    how.resolve = libc::RESOLVE_NO_SYMLINKS;
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD,
            path.as_ptr(),
            std::ptr::addr_of!(how),
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if descriptor == -1 {
        Err(errno())
    } else {
        Ok(descriptor as i32)
    }
}

/// [`open_no_symlinks`] by a walk from `/`: every directory above the last
/// component is opened O_PATH | O_DIRECTORY | O_NOFOLLOW from its parent's
/// descriptor, so a symbolic link there fails (ENOTDIR), and every
/// intermediate descriptor is closed.
fn walk_no_symlinks(components: &[CString], flags: libc::c_int) -> Result<i32, i32> {
    let Some((last, directories)) = components.split_last() else {
        return Err(libc::EINVAL);
    };
    let mut directory = raw(unsafe {
        libc::open(
            c"/".as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    })?;
    for name in directories {
        let next = unsafe {
            libc::openat(
                directory,
                name.as_ptr(),
                libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        let error = errno();
        unsafe { libc::close(directory) };
        if next == -1 {
            return Err(error);
        }
        directory = next;
    }
    let opened = unsafe { libc::openat(directory, last.as_ptr(), flags | libc::O_NOFOLLOW) };
    let error = errno();
    unsafe { libc::close(directory) };
    if opened == -1 { Err(error) } else { Ok(opened) }
}

fn fd_path(descriptor: i32, bytes: &mut [u8; 32]) -> *const libc::c_char {
    let prefix = b"/proc/self/fd/";
    bytes[..prefix.len()].copy_from_slice(prefix);
    let mut value = descriptor as u32;
    let mut digits = [0_u8; 10];
    let mut index = digits.len();
    loop {
        index -= 1;
        digits[index] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    bytes[prefix.len()..prefix.len() + digits.len() - index].copy_from_slice(&digits[index..]);
    bytes.as_ptr().cast()
}

fn write_file(path: &CStr, bytes: &[u8]) -> Result<(), i32> {
    let descriptor = raw(unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) })?;
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let written =
            unsafe { libc::write(descriptor, remaining.as_ptr().cast(), remaining.len()) };
        if written == -1 {
            if errno() == libc::EINTR {
                continue;
            }
            return Err(errno());
        }
        if written == 0 {
            return Err(libc::EIO);
        }
        remaining = &remaining[written as usize..];
    }
    raw(unsafe { libc::close(descriptor) })?;
    Ok(())
}

fn errno() -> i32 {
    unsafe { *libc::__errno_location() }
}
fn raw(result: i32) -> Result<i32, i32> {
    if result == -1 {
        Err(errno())
    } else {
        Ok(result)
    }
}
fn raw_long(result: libc::c_long) -> Result<libc::c_long, i32> {
    if result == -1 {
        Err(errno())
    } else {
        Ok(result)
    }
}

fn native_audit_arch() -> Option<u32> {
    #[cfg(target_arch = "x86_64")]
    {
        Some(0xc000_003e)
    }
    #[cfg(target_arch = "aarch64")]
    {
        Some(0xc000_00b7)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        None
    }
}

fn filter(architecture: u32, ip: bool) -> Vec<libc::sock_filter> {
    let load = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
    let equal = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
    let bits = (libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K) as u16;
    let ret = (libc::BPF_RET | libc::BPF_K) as u16;
    let statement = |code, k| libc::sock_filter {
        code,
        k,
        jt: 0,
        jf: 0,
    };
    let jump = |code, k, jt, jf| libc::sock_filter { code, k, jt, jf };
    let refuse = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
    let allow = libc::SECCOMP_RET_ALLOW;
    let mut code = vec![
        statement(load, 4),
        jump(equal, architecture, 1, 0),
        statement(ret, refuse),
        statement(load, 0),
        jump(bits, 0x4000_0000, 0, 1),
        statement(ret, refuse),
    ];
    for syscall in [
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_open_tree,
        libc::SYS_move_mount,
        libc::SYS_mount_setattr,
        libc::SYS_fsopen,
        libc::SYS_fsconfig,
        libc::SYS_fsmount,
        libc::SYS_fspick,
        libc::SYS_open_by_handle_at,
        libc::SYS_name_to_handle_at,
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_pidfd_getfd,
        libc::SYS_pidfd_open,
        libc::SYS_pidfd_send_signal,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_register,
        libc::SYS_io_uring_enter,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_userfaultfd,
        libc::SYS_capset,
        libc::SYS_setpgid,
        libc::SYS_setsid,
        libc::SYS_kill,
        libc::SYS_tkill,
        libc::SYS_tgkill,
        libc::SYS_rt_sigqueueinfo,
        libc::SYS_rt_tgsigqueueinfo,
        libc::SYS_process_madvise,
        libc::SYS_process_mrelease,
    ] {
        code.push(jump(equal, syscall as u32, 0, 1));
        code.push(statement(ret, refuse));
    }
    // The host and target do not share filesystem/IPC views, but retain
    // kernel PID numbers. Never let a PID-targeted mutator reach another
    // process. Pid zero means this task and preserves ordinary local setup.
    for syscall in [
        libc::SYS_prlimit64,
        libc::SYS_sched_setparam,
        libc::SYS_sched_setscheduler,
        libc::SYS_sched_setaffinity,
        libc::SYS_sched_setattr,
        libc::SYS_migrate_pages,
        libc::SYS_move_pages,
    ] {
        code.extend([
            jump(equal, syscall as u32, 0, 4),
            statement(load, 16),
            jump(equal, 0, 1, 0),
            statement(ret, refuse),
            statement(ret, allow),
        ]);
    }
    // These two APIs select a process, process group or user first. Only the
    // current process selector is admitted; user/group changes can span hosts.
    for (syscall, process_selector) in [
        (libc::SYS_setpriority, libc::PRIO_PROCESS),
        (libc::SYS_ioprio_set, 1),
    ] {
        code.extend([
            jump(equal, syscall as u32, 0, 7),
            statement(load, 16),
            jump(equal, process_selector, 1, 0),
            statement(ret, refuse),
            statement(load, 24),
            jump(equal, 0, 1, 0),
            statement(ret, refuse),
            statement(ret, allow),
        ]);
    }
    // Async-I/O signal ownership is another signal API. Never assign a host
    // PID/group, select its signal, or activate a preexisting inherited owner.
    // Ordinary FD duplication/flags and O_NONBLOCK remain available.
    code.extend([
        jump(equal, libc::SYS_fcntl as u32, 0, 12),
        statement(load, 24),
        jump(equal, libc::F_SETOWN as u32, 0, 1),
        statement(ret, refuse),
        jump(equal, F_SETOWN_EX as u32, 0, 1),
        statement(ret, refuse),
        jump(equal, F_SETSIG as u32, 0, 1),
        statement(ret, refuse),
        jump(equal, libc::F_SETFL as u32, 0, 3),
        statement(load, 32),
        jump(bits, libc::O_ASYNC as u32, 0, 1),
        statement(ret, refuse),
        statement(ret, allow),
    ]);
    // ioctl reaches terminal, device and filesystem controls (TIOCSTI input
    // injection, FIOASYNC signal ownership and more), so every command is
    // refused except FIONBIO: the ioctl form of O_NONBLOCK on the caller's own
    // descriptor, which std uses for nonblocking sockets. The kernel reads
    // the command as 32 bits, so the low word of the argument decides.
    code.extend([
        jump(equal, libc::SYS_ioctl as u32, 0, 4),
        statement(load, 24),
        jump(equal, libc::FIONBIO as u32, 1, 0),
        statement(ret, refuse),
        statement(ret, allow),
    ]);
    // Multiplexed socketcall belongs to a different ABI on the two supported
    // architectures and is rejected by the arch/x32 checks before dispatch.
    code.extend([
        jump(equal, libc::SYS_clone3 as u32, 0, 1),
        statement(ret, libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32),
        jump(equal, libc::SYS_clone as u32, 0, 4),
        statement(load, 16),
        jump(
            bits,
            (libc::CLONE_NEWUSER
                | libc::CLONE_NEWNS
                | libc::CLONE_NEWNET
                | libc::CLONE_NEWPID
                | libc::CLONE_NEWUTS
                | libc::CLONE_NEWIPC
                | libc::CLONE_NEWCGROUP
                | libc::CLONE_PARENT
                | libc::CLONE_PTRACE) as u32,
            0,
            1,
        ),
        statement(ret, refuse),
        statement(ret, allow),
    ]);
    // Only IP sockets from this process can exist. No AF_UNIX, netlink, packet
    // or alternate-family connection can bypass IP/Unix separation. A Unix
    // datagram pair can be retargeted, so pairs are supported only when every
    // address-bearing operation below is denied. No pair is allowed with IP.
    code.extend([
        jump(equal, libc::SYS_socket as u32, 0, 6),
        statement(load, 16),
        jump(equal, libc::AF_INET as u32, 2, 0),
        jump(equal, libc::AF_INET6 as u32, 1, 0),
        statement(ret, refuse),
        statement(ret, if ip { allow } else { refuse }),
        statement(ret, refuse),
    ]);
    code.extend([
        jump(equal, libc::SYS_socketpair as u32, 0, 4),
        statement(load, 16),
        jump(equal, libc::AF_UNIX as u32, 1, 0),
        statement(ret, refuse),
        statement(ret, if ip { refuse } else { allow }),
    ]);
    if !ip {
        for syscall in [
            libc::SYS_connect,
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_accept,
            libc::SYS_accept4,
            libc::SYS_sendto,
            libc::SYS_sendmsg,
            libc::SYS_sendmmsg,
            libc::SYS_recvmsg,
            libc::SYS_recvmmsg,
        ] {
            code.push(jump(equal, syscall as u32, 0, 1));
            code.push(statement(ret, refuse));
        }
    }
    code.push(statement(ret, allow));
    code
}

fn validate_stdio(descriptor: i32) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(descriptor, status.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let status = unsafe { status.assume_init() };
    if flags & (libc::O_PATH | libc::O_ASYNC) == 0 {
        match status.st_mode & libc::S_IFMT {
            libc::S_IFCHR if status.st_rdev == libc::makedev(1, 3) => return Ok(()),
            libc::S_IFIFO => {
                let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
                if unsafe { libc::fstatfs(descriptor, filesystem.as_mut_ptr()) } == -1 {
                    return Err(io::Error::last_os_error());
                }
                // Anonymous pipes have no writable named host inode. Named
                // FIFOs carry metadata authority and remain unsupported.
                if unsafe { filesystem.assume_init() }.f_type as u64 == 0x5049_5045 {
                    return Ok(());
                }
            }
            _ => {}
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "inherited stdio is not a supported confined byte stream",
    ))
}

pub(super) fn spawn(
    policy: Arc<LinuxPolicy>,
    launch: ProcessLaunchSpec,
    streams: SpawnIo,
    gate: Option<(BorrowedFd<'_>, &OsStr)>,
) -> io::Result<NativeChild> {
    // What this launch binds of each mount, validated here, before fork. The
    // pre-exec closure owns it, and `command` the closure, until
    // `command.spawn()` returns, which std does only after the child's exec
    // or failure: each validated object stays alive through setup.
    let bound = policy.launch_bindings(&launch).map_err(io::Error::other)?;
    let inherited_stdin = inherit(streams.stdin, libc::STDIN_FILENO)?;
    let inherited_stdout = inherit(streams.stdout, libc::STDOUT_FILENO)?;
    let inherited_stderr = inherit(streams.stderr, libc::STDERR_FILENO)?;
    // Snapshot and validate before spawn. A directory can escape chroot via
    // openat/fchdir; a regular procfs mem file or character device is also an
    // authority channel, not merely data. This backend accepts inherited
    // anonymous pipes and ordinary /dev/null only, without O_ASYNC and never
    // silent substitution. fcntl cannot activate a retained host signal owner.
    for descriptor in [&inherited_stdin, &inherited_stdout, &inherited_stderr]
        .into_iter()
        .flatten()
    {
        validate_stdio(descriptor.as_raw_fd())?;
    }
    let sigchld = signal(SignalKind::child())?;
    let (stdin_source, stdin) = input(streams.stdin, inherited_stdin)?;
    let (stdout_source, stdout) = output(streams.stdout, inherited_stdout)?;
    let (stderr_source, stderr) = output(streams.stderr, inherited_stderr)?;
    for descriptor in [&stdin_source, &stdout_source, &stderr_source] {
        validate_stdio(descriptor.as_raw_fd())?;
    }
    let stdout = stdout
        .map(|fd| ChildStdout::from_std(fd.into()))
        .transpose()?;
    let stderr = stderr
        .map(|fd| ChildStderr::from_std(fd.into()))
        .transpose()?;
    let mut command = if let Some((_, token)) = gate {
        const PROLOGUE: &str = "IFS= read -r meerkat_custody_gate <&3 || exit 125; case $meerkat_custody_gate in \"$1\") ;; *) exit 125 ;; esac; exec 3<&-; shift; exec \"$@\"";
        // A launch site of the confinement backend: the gated launch.
        #[allow(clippy::disallowed_methods)]
        let mut command = Command::new("/bin/sh");
        command.args([
            OsStr::new("-c"),
            OsStr::new(PROLOGUE),
            OsStr::new("meerkat-custody-gate"),
            token,
            launch.program.as_os_str(),
        ]);
        command.args(&launch.arguments);
        command
    } else {
        // A launch site of the confinement backend: the confined launch.
        #[allow(clippy::disallowed_methods)]
        let mut command = Command::new(&launch.program);
        command.args(&launch.arguments);
        command
    };
    command
        .env_clear()
        .envs(&launch.environment)
        .process_group(0)
        .stdin(Stdio::from(stdin_source))
        .stdout(Stdio::from(stdout_source))
        .stderr(Stdio::from(stderr_source));
    // Command's own pre-exec cwd would retain an outside directory through
    // chroot. Set cwd only after the complete private view is installed.
    let directory = CString::new(launch.directory.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid confinement cwd"))?;
    let gate = gate
        .map(|(descriptor, _)| duplicate(descriptor.as_raw_fd()))
        .transpose()?;
    // One slot per mount for setup's reopened sources, allocated here: the
    // pre-exec child only writes its own copy.
    let mut sources = vec![-1; policy.mounts.len()];
    // SAFETY: The closure invokes only syscall helpers using preallocated
    // strings, buffers, policy, bindings and FDs. Returning an error uses
    // Command's existing owned error pipe; spawn does not report success after
    // a setup failure.
    unsafe {
        command.pre_exec(move || {
            policy
                .apply(&bound, &mut sources)
                .map_err(|failure| io::Error::from_raw_os_error(failure.errno))?;
            raw(libc::chdir(directory.as_ptr())).map_err(io::Error::from_raw_os_error)?;
            if let Some(gate) = &gate {
                raw(libc::dup2(gate.as_raw_fd(), 3)).map_err(io::Error::from_raw_os_error)?;
                raw(libc::fcntl(3, libc::F_SETFD, 0)).map_err(io::Error::from_raw_os_error)?;
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    // std::process::Child has no drop reaper. Its configured streams were
    // prepared above, so no fallible conversion or await separates successful
    // spawn from this single native PID owner.
    let owned = NativeChild::from_spawned_pid(child.id() as i32, sigchld, stdin, stdout, stderr);
    drop(child);
    Ok(owned)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn spawn_revalidation_preserves_typed_refusal_before_target_entry() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let work = root.join("work");
        std::fs::create_dir(&work).unwrap();
        // This tests the actual pre-spawn validation boundary without probing
        // namespaces or entering a target. No setup field below is consumed.
        let policy = Arc::new(LinuxPolicy {
            mounts: vec![Mount::capture(&work, false, true).unwrap()],
            filter: Vec::new(),
            uid_map: Vec::new(),
            gid_map: Vec::new(),
        });
        let launch = ProcessLaunchSpec::new(
            work.join("target"),
            Vec::new(),
            work.clone(),
            Default::default(),
        )
        .unwrap();
        assert_eq!(policy.validate_launch(&launch), Ok(()));
        std::fs::rename(&work, root.join("retained-work")).unwrap();
        std::fs::create_dir(&work).unwrap();
        let error = spawn(policy, launch, SpawnIo::default(), None)
            .err()
            .unwrap();
        assert_eq!(
            error
                .get_ref()
                .and_then(|source| source.downcast_ref::<ConfinementRefusal>()),
            Some(&ConfinementRefusal::BackendUnavailable),
        );
    }

    #[test]
    fn baseline_device_names_require_character_type_and_exact_kernel_device() {
        let temp = tempfile::tempdir().unwrap();
        let regular = temp.path().join("not-a-device");
        std::fs::write(&regular, b"private").unwrap();
        for metadata in [
            std::fs::metadata(&regular).unwrap(),
            std::fs::metadata(temp.path()).unwrap(),
            std::fs::metadata("/dev/zero").unwrap(),
        ] {
            assert_eq!(
                validate_baseline_kind(Path::new("/dev/null"), &metadata),
                Err(ConfinementRefusal::UnsupportedRequirement)
            );
        }
        for path in ["/dev/null", "/dev/random", "/dev/urandom"] {
            let metadata = std::fs::metadata(path).unwrap();
            assert_eq!(validate_baseline_kind(Path::new(path), &metadata), Ok(()));
        }
    }

    /// Setup opens each mount's source again by name and uses it only while
    /// it is the object pinned at compile time; a renamed-over source is
    /// refused at its identity step, never bound. No namespace is entered.
    #[test]
    fn setup_reopens_a_source_only_while_it_is_the_pinned_object() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let work = root.join("work");
        std::fs::create_dir(&work).unwrap();
        let null = Mount::capture(Path::new("/dev/null"), true, false).unwrap();
        let directory = Mount::capture(&work, false, true).unwrap();
        for mount in [&null, &directory] {
            let pinned = mount.bind().unwrap().pinned;
            let source = mount.reopen(pinned).unwrap();
            assert_eq!(stat_identity(source), Ok(pinned));
            unsafe { libc::close(source) };
        }
        assert_eq!(null.bind().unwrap().pinned.2, libc::makedev(1, 3));

        let pinned = directory.bind().unwrap().pinned;
        std::fs::rename(&work, root.join("retained-work")).unwrap();
        std::fs::create_dir(&work).unwrap();
        assert_eq!(
            directory.reopen(pinned),
            Err(SetupFailure {
                step: SetupStep::MountIdentity,
                errno: libc::ESTALE,
            })
        );
        // Only a final link's target is ever captured again.
        assert_eq!(
            directory.bind().err(),
            Some(ConfinementRefusal::BackendUnavailable)
        );
    }

    // Sets `path`'s permission bits exactly, whatever the umask made them.
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// A read-only file grant may name a symbolic link as its final
    /// component (`/etc/resolv.conf` on a systemd-resolved host) when the
    /// link's own text alone names its regular-file target: the target is
    /// pinned and bound read-only at the link's own path. A link whose text
    /// a second link resolves elsewhere is refused, whoever can replace that
    /// second link. No namespace is entered.
    #[test]
    fn a_final_link_is_granted_only_when_it_names_its_target_in_one_hop() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        // No directory below is group- or other-writable, whatever the umask.
        set_mode(&root, 0o700);
        let etc = root.join("etc");
        let run = root.join("run");
        let shared = root.join("shared");
        for directory in [&etc, &run, &shared] {
            std::fs::create_dir(directory).unwrap();
            set_mode(directory, 0o755);
        }
        let target = run.join("stub-resolv.conf");
        std::fs::write(&target, b"nameserver 127.0.0.53\n").unwrap();
        // One hop, relative, as systemd-resolved installs it.
        let link = etc.join("resolv.conf");
        std::os::unix::fs::symlink("../run/stub-resolv.conf", &link).unwrap();

        let mount = Mount::capture(&link, false, false)
            .expect("a read-only grant of a trusted one-hop final link is captured");
        assert_eq!(mount.path, link);
        assert_eq!(mount.canonical, target);
        assert_eq!(mount.source.as_bytes(), target.as_os_str().as_bytes());
        assert_eq!(
            mount.destination.as_bytes(),
            Path::new("/tmp")
                .join(link.strip_prefix("/").unwrap())
                .as_os_str()
                .as_bytes()
        );
        assert!(!mount.directory && !mount.writable);
        assert_eq!(
            mount.link.as_deref(),
            Some(Path::new("../run/stub-resolv.conf")),
            "the link's text is kept"
        );
        let pinned = mount.bind().unwrap().pinned;
        let source = mount.reopen(pinned).unwrap();
        assert_eq!(stat_identity(source), Ok(pinned));
        unsafe { libc::close(source) };
        // Setup's fallback walk follows the resolved target's components,
        // not the granted path's.
        assert_eq!(
            Some(mount.source_components.clone()),
            normal_components(&target)
        );
        let walked = walk_no_symlinks(
            &mount.source_components,
            libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
        .unwrap();
        assert_eq!(stat_identity(walked), Ok(pinned));
        unsafe { libc::close(walked) };

        let refused = Some(ConfinementRefusal::UnsupportedRequirement);
        // The second hop sits in a directory anyone may write, so its owner
        // could choose what the grant exposes: refused.
        let hop = shared.join("hop");
        std::os::unix::fs::symlink(&target, &hop).unwrap();
        set_mode(&shared, 0o777);
        let chained = etc.join("chained.conf");
        std::os::unix::fs::symlink("../shared/hop", &chained).unwrap();
        assert_eq!(Mount::capture(&chained, false, false).err(), refused);
        // Refused when every link of the chain is trusted, too: the grant's
        // own text alone must name the target.
        set_mode(&shared, 0o755);
        assert_eq!(Mount::capture(&chained, false, false).err(), refused);
    }

    // A directory `root/name` with exactly `mode`, for the final-link rows.
    fn directory(root: &Path, name: &str, mode: u32) -> PathBuf {
        let directory = root.join(name);
        std::fs::create_dir(&directory).unwrap();
        set_mode(&directory, mode);
        directory
    }

    /// A final link is granted only where no less-trusted party can retarget
    /// it: a link in a directory that group or others may write is refused
    /// unless that directory is sticky, and so is a link under such an
    /// ancestor or one owned by another user. No namespace is entered.
    #[test]
    fn a_final_link_is_granted_only_where_no_other_user_can_retarget_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        set_mode(&root, 0o700);
        let shared = directory(&root, "shared", 0o755);
        let run = directory(&root, "run", 0o755);
        let target = run.join("stub-resolv.conf");
        std::fs::write(&target, b"nameserver 127.0.0.53\n").unwrap();
        let link = shared.join("resolv.conf");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let refused = Some(ConfinementRefusal::UnsupportedRequirement);

        assert!(Mount::capture(&link, false, false).is_ok());
        // World-writable without the sticky bit: anyone may replace the link.
        set_mode(&shared, 0o777);
        assert_eq!(Mount::capture(&link, false, false).err(), refused);
        // Group-writable alone is refused too.
        set_mode(&shared, 0o775);
        assert_eq!(Mount::capture(&link, false, false).err(), refused);
        // Sticky: only the link's owner, the directory's owner or root may
        // rename or remove it, as under /tmp.
        set_mode(&shared, 0o1777);
        assert!(Mount::capture(&link, false, false).is_ok());
        // A shared writable ancestor further up refuses as well.
        set_mode(&shared, 0o755);
        set_mode(&root, 0o777);
        assert_eq!(Mount::capture(&link, false, false).err(), refused);
        set_mode(&root, 0o700);
        assert!(Mount::capture(&link, false, false).is_ok());
        // This test's link and directories belong to the effective uid: with
        // another uid trusted, they are another user's.
        let euid = unsafe { libc::geteuid() };
        assert!(grantable_link_text_for(&link, &target, euid).is_some());
        if euid != 0 {
            assert_eq!(
                grantable_link_text_for(&link, &target, euid.wrapping_add(1)),
                None
            );
        }
    }

    /// A path in `directory` that no other test run uses: `prefix`, this
    /// process's id and the time.
    fn unique_path(directory: &Path, prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        directory.join(format!("{prefix}-{}-{nanos}", std::process::id()))
    }

    /// Removes a file or link made outside a temporary directory when the
    /// test ends, passed or failed.
    struct RemoveOnDrop(PathBuf);

    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// The link-owner rule on its own: a link directly in `/tmp`, root's
    /// sticky directory under root's `/`, has every directory above it
    /// trusted, so its own owner decides. It is granted while its owner is
    /// trusted and refused while only root and another uid are, as a link
    /// another user planted there is. No namespace is entered.
    #[test]
    fn a_final_link_in_tmp_is_granted_only_when_its_owner_is_trusted() {
        let trusted_directory = |path: &str, sticky: bool| {
            let metadata = std::fs::symlink_metadata(path).unwrap();
            metadata.is_dir()
                && metadata.uid() == 0
                && (metadata.mode() & 0o022 == 0 || (sticky && metadata.mode() & 0o1000 != 0))
        };
        assert!(
            trusted_directory("/", false) && trusted_directory("/tmp", true),
            "this row needs root's /, and root's sticky /tmp"
        );
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().canonicalize().unwrap().join("app.conf");
        std::fs::write(&target, b"granted").unwrap();
        let link = RemoveOnDrop(unique_path(Path::new("/tmp"), "meerkat-final-link-owner"));
        std::os::unix::fs::symlink(&target, &link.0).unwrap();
        let euid = unsafe { libc::geteuid() };
        // The link's owner, and a uid that is neither root nor that owner.
        // Root's own link is trusted whoever else is, so as root the link
        // goes to another uid.
        let (owner, other) = if euid == 0 {
            let name = CString::new(link.0.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::lchown(name.as_ptr(), 65534, 65534) }, 0);
            (65534, 0)
        } else {
            (euid, euid.wrapping_add(1))
        };
        assert_eq!(
            grantable_link_text_for(&link.0, &target, owner),
            Some(target.clone()),
            "a link whose owner is trusted is granted"
        );
        assert_eq!(
            grantable_link_text_for(&link.0, &target, other),
            None,
            "a link another user owns is refused, though every directory above it is trusted"
        );
        if euid != 0 {
            assert!(Mount::capture(&link.0, false, false).is_ok());
        }
    }

    /// Only a read-only grant of a final link to a regular file with one
    /// name is captured: a writable grant, a link to a directory, a dangling
    /// link, a link under a linked directory and a link to a hard-linked
    /// file are refused. No namespace is entered.
    #[test]
    fn a_final_link_grant_is_read_only_and_names_a_regular_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        set_mode(&root, 0o700);
        let etc = directory(&root, "etc", 0o755);
        let run = directory(&root, "run", 0o755);
        let target = run.join("stub-resolv.conf");
        std::fs::write(&target, b"nameserver 127.0.0.53\n").unwrap();
        let link = etc.join("resolv.conf");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let refused = Some(ConfinementRefusal::UnsupportedRequirement);

        assert!(Mount::capture(&link, false, false).is_ok());
        assert_eq!(Mount::capture(&link, false, true).err(), refused);
        let directory_link = etc.join("run");
        std::os::unix::fs::symlink(&run, &directory_link).unwrap();
        assert_eq!(Mount::capture(&directory_link, false, false).err(), refused);
        let dangling = etc.join("dangling");
        std::os::unix::fs::symlink(run.join("missing"), &dangling).unwrap();
        assert_eq!(Mount::capture(&dangling, false, false).err(), refused);
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&etc, &alias).unwrap();
        assert_eq!(
            Mount::capture(&alias.join("resolv.conf"), false, false).err(),
            refused
        );
        std::fs::hard_link(&target, run.join("second-name")).unwrap();
        assert_eq!(Mount::capture(&link, false, false).err(), refused);
    }

    /// A final link's target is held to the granted path's own rule: never
    /// `/`, and nothing in `/proc`, `/sys` or `/dev`. A trusted one-hop link
    /// to a regular file in `/dev/shm` (tmpfs, which the filesystem-type
    /// check lets through) is refused, as a grant of that file itself is,
    /// before any namespace is probed.
    #[test]
    fn a_final_link_to_a_file_under_dev_is_refused() {
        use meerkat_core::confinement::{ConfinementSpec, PlatformBaseline};
        let target = RemoveOnDrop(unique_path(
            Path::new("/dev/shm"),
            "meerkat-final-link-target",
        ));
        std::fs::write(&target.0, b"nameserver 192.0.2.1\n").unwrap();
        assert_eq!(
            std::fs::canonicalize(&target.0).unwrap(),
            target.0,
            "this row needs a /dev/shm that is no symbolic link"
        );
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        set_mode(&root, 0o700);
        let etc = directory(&root, "etc", 0o755);
        let link = etc.join("resolv.conf");
        std::os::unix::fs::symlink(&target.0, &link).unwrap();
        assert_eq!(
            grantable_link_text(&link, &target.0),
            Some(target.0.clone()),
            "the link itself passes the one-hop and owner rules"
        );
        let refused = Some(ConfinementRefusal::UnsupportedRequirement);
        assert_eq!(Mount::capture(&link, false, false).err(), refused);
        let compile = |path: &Path| {
            let spec = ConfinementSpec {
                baseline: PlatformBaseline::CommandRuntimeV1,
                read: FilesystemAccess::Paths(vec![PathAccess::Literal(path.to_path_buf())]),
                write: FilesystemAccess::Paths(Vec::new()),
                deny_read: Vec::new(),
                deny_write: Vec::new(),
                unix_connect: Vec::new(),
                network: IpNetworkAccess::Unrestricted,
                require_descendant_termination: false,
            };
            LinuxPolicy::compile(&ExecutionConfinement::try_from(spec).unwrap()).err()
        };
        assert_eq!(compile(&target.0), refused, "a grant of the file itself");
        assert_eq!(compile(&link), refused, "a grant of the link to it");
    }

    /// A launch binds the target it validated: once a final link's replaced
    /// target is captured again, setup checks the reopened source against
    /// the new file and refuses the replaced one, and a file swapped in after
    /// the launch validated is refused at setup's identity step as before. No
    /// namespace is entered.
    #[test]
    fn a_launch_binds_the_final_link_target_it_captured_again() {
        let identity_refused = Err(SetupFailure {
            step: SetupStep::MountIdentity,
            errno: libc::ESTALE,
        });
        let fixture = ResolverFixture::new();
        let mount = &fixture.policy.mounts[1];
        let before = mount.bind().unwrap();
        fixture.replace_target(b"nameserver 127.0.0.53\nsearch example.test\n");

        let bound = fixture
            .policy
            .launch_bindings(&fixture.launch)
            .unwrap()
            .swap_remove(1);
        let replacement = std::fs::metadata(&fixture.target).unwrap();
        assert_eq!(
            (bound.pinned.0, bound.pinned.1),
            (replacement.dev(), replacement.ino()),
            "the launch binds the new file"
        );
        assert_ne!(bound.pinned, before.pinned);
        assert_eq!(
            mount.bind().unwrap().pinned,
            bound.pinned,
            "the new file stays pinned"
        );
        let source = mount.reopen(bound.pinned).unwrap();
        assert_eq!(stat_identity(source), Ok(bound.pinned));
        unsafe { libc::close(source) };
        assert_eq!(mount.reopen(before.pinned), identity_refused);

        fixture.replace_target(b"nameserver 192.0.2.2\n");
        assert_eq!(mount.reopen(bound.pinned), identity_refused);
    }

    /// A policy over a work directory and a read-only grant of the link
    /// `etc/resolv.conf`, which names `../run/stub-resolv.conf` in one hop as
    /// systemd-resolved installs it, with a launch in the work directory. No
    /// namespace is probed or entered.
    struct ResolverFixture {
        _temp: tempfile::TempDir,
        run: PathBuf,
        target: PathBuf,
        link: PathBuf,
        policy: LinuxPolicy,
        launch: ProcessLaunchSpec,
    }

    impl ResolverFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            // No directory below is group- or other-writable, whatever the
            // umask.
            set_mode(&root, 0o700);
            let etc = directory(&root, "etc", 0o755);
            let run = directory(&root, "run", 0o755);
            let work = directory(&root, "work", 0o755);
            let target = run.join("stub-resolv.conf");
            std::fs::write(&target, b"nameserver 127.0.0.53\n").unwrap();
            let link = etc.join("resolv.conf");
            std::os::unix::fs::symlink("../run/stub-resolv.conf", &link).unwrap();
            let policy = LinuxPolicy {
                mounts: vec![
                    Mount::capture(&work, false, true).unwrap(),
                    Mount::capture(&link, false, false).unwrap(),
                ],
                filter: Vec::new(),
                uid_map: Vec::new(),
                gid_map: Vec::new(),
            };
            let launch =
                ProcessLaunchSpec::new(work.join("target"), Vec::new(), work, Default::default())
                    .unwrap();
            assert_eq!(policy.validate_launch(&launch), Ok(()));
            Self {
                _temp: temp,
                run,
                target,
                link,
                policy,
                launch,
            }
        }

        /// Replaces the target the way systemd-resolved rewrites its stub
        /// file: a new file written beside it and renamed over its name.
        fn replace_target(&self, contents: &[u8]) {
            let staged = self.run.join(".stub-resolv.conf.new");
            std::fs::write(&staged, contents).unwrap();
            std::fs::rename(&staged, &self.target).unwrap();
        }

        /// Replaces the target the same way with a FIFO, which no capture
        /// accepts.
        fn replace_target_with_fifo(&self) {
            let staged = self.run.join(".stub-resolv.conf.new");
            let name = CString::new(staged.as_os_str().as_bytes()).unwrap();
            assert_eq!(
                unsafe { libc::mkfifo(name.as_ptr(), 0o644) },
                0,
                "{}",
                io::Error::last_os_error()
            );
            std::fs::rename(&staged, &self.target).unwrap();
        }

        /// Replaces the grant's link with one whose text is `text`.
        fn relink(&self, text: &Path) {
            std::fs::remove_file(&self.link).unwrap();
            std::os::unix::fs::symlink(text, &self.link).unwrap();
        }

        fn validate_launch(&self) -> Result<(), ConfinementRefusal> {
            self.policy.validate_launch(&self.launch)
        }
    }

    /// R1: a final link's target replaced under its own name, as
    /// systemd-resolved rewrites its stub file when its DNS settings change,
    /// is captured again at the next launch, and the launch proceeds; so do
    /// the launches after it, and after the next replacement.
    #[test]
    fn a_final_link_target_replaced_by_rename_is_captured_again_at_launch() {
        let fixture = ResolverFixture::new();
        fixture.replace_target(b"nameserver 127.0.0.53\nsearch example.test\n");
        assert_eq!(
            fixture.validate_launch(),
            Ok(()),
            "a target replaced under its name is captured again and the launch proceeds"
        );
        assert_eq!(fixture.validate_launch(), Ok(()));
        fixture.replace_target(b"nameserver 127.0.0.53\n");
        assert_eq!(fixture.validate_launch(), Ok(()));
    }

    /// R2: a target path replaced by a symbolic link is refused at launch,
    /// whatever that link names: the grant's link would reach its target in
    /// two hops, even when the second hop leads to the file pinned before.
    #[test]
    fn a_final_link_target_path_replaced_by_a_symlink_is_refused_at_launch() {
        let refused = Err(ConfinementRefusal::BackendUnavailable);
        // Another file, behind a link at the target's name.
        let fixture = ResolverFixture::new();
        std::fs::write(fixture.run.join("other.conf"), b"nameserver 192.0.2.1\n").unwrap();
        std::fs::remove_file(&fixture.target).unwrap();
        std::os::unix::fs::symlink("other.conf", &fixture.target).unwrap();
        assert_eq!(fixture.validate_launch(), refused);
        // The pinned file itself, moved and named through a link.
        let fixture = ResolverFixture::new();
        std::fs::rename(&fixture.target, fixture.run.join("moved.conf")).unwrap();
        std::os::unix::fs::symlink("moved.conf", &fixture.target).unwrap();
        assert_eq!(fixture.validate_launch(), refused);
    }

    /// R3: a retargeted final link is refused at launch, and so is a link
    /// replaced with other text that still names the same file, with its
    /// target replaced or not: only the target file may be replaced, under
    /// the link as captured.
    #[test]
    fn a_retargeted_final_link_is_refused_at_launch() {
        let refused = Err(ConfinementRefusal::BackendUnavailable);
        let fixture = ResolverFixture::new();
        std::fs::write(fixture.run.join("other.conf"), b"nameserver 192.0.2.1\n").unwrap();
        fixture.relink(Path::new("../run/other.conf"));
        assert_eq!(fixture.validate_launch(), refused);

        let fixture = ResolverFixture::new();
        fixture.relink(&fixture.target);
        assert_eq!(
            fixture.validate_launch(),
            refused,
            "the link was replaced with other text, though it names the same file"
        );

        let fixture = ResolverFixture::new();
        fixture.relink(&fixture.target);
        fixture.replace_target(b"nameserver 127.0.0.53\nsearch example.test\n");
        assert_eq!(fixture.validate_launch(), refused);
    }

    /// R4: only a target that is no longer the pinned file is captured
    /// again. The pinned file moved into a replacement of its directory keeps
    /// its identity, and the replaced directory refuses the launch as before.
    #[test]
    fn a_final_link_target_directory_replaced_around_the_pinned_file_is_refused_at_launch() {
        let fixture = ResolverFixture::new();
        let retired = fixture.run.with_file_name("run.retired");
        std::fs::rename(&fixture.run, &retired).unwrap();
        std::fs::create_dir(&fixture.run).unwrap();
        set_mode(&fixture.run, 0o755);
        std::fs::rename(retired.join("stub-resolv.conf"), &fixture.target).unwrap();
        assert_eq!(
            fixture.validate_launch(),
            Err(ConfinementRefusal::BackendUnavailable)
        );
    }

    /// R5: a launch holds the object it validated until its setup is done.
    /// Launch L1 validates its bindings. The target's directory writer
    /// renames a new file over the target, and launch L2 captures it again,
    /// dropping the pin of the file L1 validated. The writer then renames
    /// another object over the target: a FIFO every other round, which no
    /// capture accepts, else a file. L1's setup refuses it every round, also
    /// on a filesystem that hands a freed inode number to a new object (ext4
    /// often does), where that object would otherwise carry the identity L1
    /// validated.
    #[test]
    fn a_launch_refuses_an_object_put_in_place_after_a_concurrent_recapture() {
        const ROUNDS: u32 = 400;
        let fixture = ResolverFixture::new();
        let mount = &fixture.policy.mounts[1];
        let (mut fifos, mut files) = (0_u32, 0_u32);
        for round in 0..ROUNDS {
            let first = fixture.policy.launch_bindings(&fixture.launch).unwrap();
            fixture.replace_target(&round.to_le_bytes());
            let second = fixture.policy.launch_bindings(&fixture.launch).unwrap();
            assert_ne!(
                second[1].pinned, first[1].pinned,
                "L2 captured the new file again"
            );
            let fifo = round % 2 == 0;
            if fifo {
                fixture.replace_target_with_fifo();
            } else {
                fixture.replace_target(b"nameserver 192.0.2.1\n");
            }
            // L1's setup, while L1 still holds what it validated.
            if let Ok(source) = mount.reopen(first[1].pinned) {
                if fifo {
                    fifos += 1;
                } else {
                    files += 1;
                }
                unsafe { libc::close(source) };
            }
            // Both launches' setup is done.
            drop((first, second));
            if fifo {
                // A regular file again, for the next round's first launch.
                fixture.replace_target(b"nameserver 127.0.0.53\n");
            }
        }
        assert_eq!(
            (fifos, files),
            (0, 0),
            "setup accepted objects no launch validated in {ROUNDS} rounds: \
             {fifos} FIFOs, {files} files"
        );
    }

    /// What one race phase of
    /// `a_racing_intermediate_directory_never_gets_another_object_pinned`
    /// saw.
    #[derive(Debug, Default)]
    struct RaceTally {
        attempts: u64,
        captured: u64,
        wrong_pins: u64,
        exchanges: u64,
    }

    /// Runs `attempt` for `duration` while another thread exchanges the
    /// directory `real` with `flip`, a symbolic link to another directory
    /// (renameat2 RENAME_EXCHANGE), as a writer of their parent could; then
    /// leaves `real` the directory again. `attempt` answers `None` when
    /// nothing was pinned, else whether the other directory's object was.
    fn race(
        real: &Path,
        flip: &Path,
        duration: Duration,
        mut attempt: impl FnMut() -> Option<bool>,
    ) -> RaceTally {
        use std::sync::atomic::{AtomicBool, Ordering};
        let names = [real, flip].map(|path| CString::new(path.as_os_str().as_bytes()).unwrap());
        let exchange = |names: &[CString; 2]| unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                names[0].as_ptr(),
                libc::AT_FDCWD,
                names[1].as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        let stop = Arc::new(AtomicBool::new(false));
        let exchanger = {
            let (names, stop) = (names.clone(), Arc::clone(&stop));
            std::thread::spawn(move || {
                let mut exchanges = 0_u64;
                while !stop.load(Ordering::Relaxed) {
                    if exchange(&names) == 0 {
                        exchanges += 1;
                    }
                }
                exchanges
            })
        };
        let mut tally = RaceTally::default();
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            tally.attempts += 1;
            if let Some(wrong) = attempt() {
                tally.captured += 1;
                tally.wrong_pins += u64::from(wrong);
            }
        }
        stop.store(true, Ordering::Relaxed);
        tally.exchanges = exchanger.join().unwrap();
        if std::fs::symlink_metadata(real)
            .unwrap()
            .file_type()
            .is_symlink()
        {
            assert_eq!(exchange(&names), 0);
        }
        tally
    }

    /// Replaces `name` in the directory open as `directory`: a new file
    /// written beside it and renamed over it, through that descriptor, so
    /// whatever the directory's own name points at meanwhile.
    fn replace_in(directory: &File, name: &CStr, contents: &[u8]) {
        let stage = c".stage";
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                stage.as_ptr(),
                libc::O_CREAT | libc::O_WRONLY | libc::O_TRUNC | libc::O_CLOEXEC,
                0o644,
            )
        };
        assert!(descriptor >= 0, "{}", io::Error::last_os_error());
        let written = unsafe { libc::write(descriptor, contents.as_ptr().cast(), contents.len()) };
        assert_eq!(written, contents.len() as isize);
        unsafe { libc::close(descriptor) };
        let renamed = unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                stage.as_ptr(),
                directory.as_raw_fd(),
                name.as_ptr(),
            )
        };
        assert_eq!(renamed, 0, "{}", io::Error::last_os_error());
    }

    // The (st_dev, st_ino) a mount pins.
    fn pinned_identity(mount: &Mount) -> (u64, u64) {
        let pin = mount.pin.lock().unwrap();
        (pin.pinned.0, pin.pinned.1)
    }

    /// A writer of a directory partway down a granted path exchanges that
    /// directory, again and again, with a symbolic link to another directory
    /// of the host's user while the grant is captured: a direct file grant,
    /// a writable subtree grant, a final link, and a final link captured
    /// again at launch after its target was replaced. No capture ever pins
    /// the other directory's object. Bounded: a little over a second a
    /// phase.
    #[test]
    fn a_racing_intermediate_directory_never_gets_another_object_pinned() {
        const PHASE: Duration = Duration::from_millis(1200);
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        set_mode(&root, 0o700);
        let etc = directory(&root, "etc", 0o755);
        let run = directory(&root, "run", 0o755);
        let real = directory(&run, "real", 0o755);
        directory(&real, "work", 0o755);
        std::fs::write(real.join("app.conf"), b"granted").unwrap();
        let secret = directory(&root, "secret", 0o700);
        directory(&secret, "work", 0o700);
        std::fs::write(secret.join("app.conf"), b"never granted").unwrap();
        let identity = |path: &Path| {
            let metadata = std::fs::symlink_metadata(path).unwrap();
            (metadata.dev(), metadata.ino())
        };
        let secret_file = identity(&secret.join("app.conf"));
        let secret_work = identity(&secret.join("work"));
        let flip = run.join("flip");
        std::os::unix::fs::symlink(&secret, &flip).unwrap();
        let link = etc.join("app.conf");
        std::os::unix::fs::symlink("../run/real/app.conf", &link).unwrap();
        let capture = |grant: &Path, writable: bool, secret: (u64, u64)| {
            Mount::capture(grant, false, writable)
                .ok()
                .map(|mount| pinned_identity(&mount) == secret)
        };

        let mut tallies = Vec::new();
        let file = real.join("app.conf");
        tallies.push((
            "direct file grant",
            race(&real, &flip, PHASE, || capture(&file, false, secret_file)),
        ));
        let work = real.join("work");
        tallies.push((
            "writable subtree grant",
            race(&real, &flip, PHASE, || capture(&work, true, secret_work)),
        ));
        tallies.push((
            "final link",
            race(&real, &flip, PHASE, || capture(&link, false, secret_file)),
        ));
        let mount = Mount::capture(&link, false, false).unwrap();
        let real_directory = File::open(&real).unwrap();
        let mut round = 0_u64;
        tallies.push((
            "final link captured again at launch",
            race(&real, &flip, PHASE, || {
                round += 1;
                replace_in(&real_directory, c"app.conf", &round.to_le_bytes());
                mount
                    .bind()
                    .ok()
                    .map(|bound| (bound.pinned.0, bound.pinned.1) == secret_file)
            }),
        ));

        for (phase, tally) in &tallies {
            eprintln!("RACE {phase}: {tally:?}");
        }
        for (phase, tally) in &tallies {
            assert!(
                tally.exchanges > 0 && tally.captured > 0,
                "{phase}: the race ran and grants were captured: {tally:?}"
            );
            assert_eq!(
                tally.wrong_pins, 0,
                "{phase}: a capture pinned the other directory's object: {tallies:?}"
            );
        }
    }

    /// The symlink-free open refuses a name with a symbolic link in a
    /// directory partway down, by openat2 and by the fallback walk alike,
    /// and opens a symlink-free name to the same object both ways. A final
    /// component that is a link opens as the link itself both ways
    /// (O_PATH | O_NOFOLLOW), for the caller's type or identity check to
    /// refuse. The walk is called directly: no supported kernel lacks
    /// openat2.
    #[test]
    fn the_symlink_free_open_refuses_an_intermediate_link_both_ways() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let real = root.join("real");
        std::fs::create_dir(&real).unwrap();
        let file = real.join("app.conf");
        std::fs::write(&file, b"granted").unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let trailing = root.join("trailing");
        std::os::unix::fs::symlink(&file, &trailing).unwrap();
        let identity = |metadata: std::fs::Metadata| (metadata.dev(), metadata.ino());
        let flags = libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        let openat2 = |path: &Path| {
            openat2_no_symlinks(&CString::new(path.as_os_str().as_bytes()).unwrap(), flags)
        };
        let walk = |path: &Path| walk_no_symlinks(&normal_components(path).unwrap(), flags);
        let ways: [(&str, &dyn Fn(&Path) -> Result<i32, i32>); 2] =
            [("openat2", &openat2), ("walk", &walk)];
        for (way, open) in ways {
            let descriptor = open(&file).expect(way);
            assert_eq!(
                stat_identity(descriptor).map(|status| (status.0, status.1)),
                Ok(identity(std::fs::metadata(&file).unwrap())),
                "{way}: a symlink-free name opens its object"
            );
            unsafe { libc::close(descriptor) };

            let through_link = open(&alias.join("app.conf"));
            assert!(
                through_link.is_err(),
                "{way}: a link partway down is refused, got {through_link:?}"
            );

            let descriptor = open(&trailing).unwrap();
            assert_eq!(
                stat_identity(descriptor).map(|status| (status.0, status.1)),
                Ok(identity(std::fs::symlink_metadata(&trailing).unwrap())),
                "{way}: a final link opens as the link itself"
            );
            unsafe { libc::close(descriptor) };
        }
    }

    /// A bind remount may not clear a flag of the host mount in the
    /// unprivileged namespace, so a noexec host mount keeps noexec; capture
    /// records the host mount's own flag.
    #[test]
    fn bind_remount_keeps_the_host_mounts_noexec() {
        for device in [false, true] {
            for writable in [false, true] {
                for noexec in [false, true] {
                    let flags = Mount::remount_flags(device, writable, noexec);
                    assert_ne!(flags & libc::MS_BIND, 0);
                    assert_ne!(flags & libc::MS_REMOUNT, 0);
                    assert_ne!(flags & libc::MS_NOSUID, 0);
                    assert_eq!((flags & libc::MS_NODEV) != 0, !device);
                    assert_eq!((flags & libc::MS_RDONLY) != 0, !writable);
                    assert_eq!((flags & libc::MS_NOEXEC) != 0, noexec);
                }
            }
        }
        let null = Mount::capture(Path::new("/dev/null"), true, false).unwrap();
        let mut host = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        assert_eq!(
            unsafe { libc::statvfs(c"/dev/null".as_ptr(), host.as_mut_ptr()) },
            0
        );
        let host = unsafe { host.assume_init() };
        assert_eq!(
            null.bind().unwrap().noexec,
            (host.f_flag & libc::ST_NOEXEC) != 0
        );
    }

    // Runs `program` on one native syscall the way the kernel does and
    // returns its action. Covers exactly the instructions `filter` emits.
    fn run_filter(
        program: &[libc::sock_filter],
        architecture: u32,
        nr: u32,
        args: [u64; 6],
    ) -> u32 {
        let load = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
        let equal = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
        let bits = (libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K) as u16;
        let ret = (libc::BPF_RET | libc::BPF_K) as u16;
        // struct seccomp_data: nr, arch, instruction pointer, six arguments.
        let mut data = [0_u8; 64];
        data[0..4].copy_from_slice(&nr.to_ne_bytes());
        data[4..8].copy_from_slice(&architecture.to_ne_bytes());
        for (index, argument) in args.iter().enumerate() {
            data[16 + 8 * index..24 + 8 * index].copy_from_slice(&argument.to_ne_bytes());
        }
        let mut accumulator = 0_u32;
        let mut counter = 0;
        loop {
            let instruction = program[counter];
            let skip = match instruction.code {
                code if code == load => {
                    let offset = instruction.k as usize;
                    accumulator = u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap());
                    0
                }
                code if code == equal => {
                    if accumulator == instruction.k {
                        instruction.jt
                    } else {
                        instruction.jf
                    }
                }
                code if code == bits => {
                    if accumulator & instruction.k != 0 {
                        instruction.jt
                    } else {
                        instruction.jf
                    }
                }
                code if code == ret => return instruction.k,
                code => unreachable!("filter emits no instruction {code:#x}"),
            };
            counter += 1 + usize::from(skip);
        }
    }

    /// ioctl admits only FIONBIO, the ioctl form of O_NONBLOCK that std uses
    /// for nonblocking sockets; every other command stays refused, whatever
    /// the IP policy.
    #[test]
    fn seccomp_admits_only_fionbio_among_ioctl_commands() {
        let Some(architecture) = native_audit_arch() else {
            return;
        };
        let refuse = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
        let fionbio = u64::from(libc::FIONBIO as u32);
        for ip in [false, true] {
            let program = filter(architecture, ip);
            let ioctl = |command: u64| {
                run_filter(
                    &program,
                    architecture,
                    libc::SYS_ioctl as u32,
                    [0, command, 0, 0, 0, 0],
                )
            };
            assert_eq!(ioctl(fionbio), libc::SECCOMP_RET_ALLOW);
            // The kernel takes the command as 32 bits.
            assert_eq!(ioctl((1 << 32) | fionbio), libc::SECCOMP_RET_ALLOW);
            for command in [libc::FIOASYNC, libc::TIOCSTI, libc::FIONREAD, libc::FIOCLEX] {
                assert_eq!(ioctl(u64::from(command as u32)), refuse);
            }
            assert_eq!(
                run_filter(&program, architecture, libc::SYS_getpid as u32, [0; 6]),
                libc::SECCOMP_RET_ALLOW
            );
            assert_eq!(
                run_filter(&program, architecture, libc::SYS_mount as u32, [0; 6]),
                refuse
            );
        }
    }
}
