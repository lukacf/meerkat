//! Native Linux lowering of the existing immutable confinement requirement.
//!
//! Restricted paths use an empty private mount view, not content-only Landlock
//! grants. No host procfs is exposed. This module owns no permission policy or
//! child supervisor; it adapts the retained requirement and transfers the one
//! spawned child into NativeChild.
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, OsStr};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
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

struct Mount {
    path: PathBuf,
    canonical: PathBuf,
    file: File,
    identity: (u64, u64),
    ancestors: Vec<(PathBuf, u64, u64)>,
    components: Vec<CString>,
    destination: CString,
    directory: bool,
    writable: bool,
    device: bool,
}

/// Mechanical compiled resources. Every file descriptor pins the exact host
/// object selected at setup. Per-launch checks cannot retarget these handles.
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
                    if path == Path::new("/")
                        || ["/proc", "/sys", "/dev"]
                            .iter()
                            .any(|root| path.starts_with(root))
                    {
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
        policy.validate()?;
        policy.probe()?;
        Ok(policy)
    }

    pub(super) fn validate(&self) -> Result<(), ConfinementRefusal> {
        for mount in &self.mounts {
            mount.validate()?;
        }
        {
            let stage = std::fs::symlink_metadata("/tmp")
                .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
            if !stage.is_dir() || stage.file_type().is_symlink() {
                return Err(ConfinementRefusal::UnsupportedRequirement);
            }
        }
        Ok(())
    }

    pub(super) fn validate_launch(
        &self,
        launch: &ProcessLaunchSpec,
    ) -> Result<(), ConfinementRefusal> {
        self.validate()?;
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
        Ok(())
    }

    // Capability setup runs once per compiled host profile. It never executes
    // the requested target. Only the exact probe PID is waited/killed here.
    fn probe(&self) -> Result<(), ConfinementRefusal> {
        let pid = unsafe { libc::fork() };
        if pid == -1 {
            return Err(ConfinementRefusal::BackendUnavailable);
        }
        if pid == 0 {
            let result = self.apply();
            unsafe { libc::_exit(if result.is_ok() { 0 } else { 125 }) };
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
    fn apply(&self) -> Result<(), i32> {
        raw(unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) })?;
        {
            raw(unsafe {
                libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS | libc::CLONE_NEWIPC)
            })?;
            write_file(c"/proc/self/setgroups", b"deny")?;
            write_file(c"/proc/self/uid_map", &self.uid_map)?;
            write_file(c"/proc/self/gid_map", &self.gid_map)?;
            // Private propagation precedes every bind or filesystem mutation.
            raw(unsafe {
                libc::mount(
                    std::ptr::null(),
                    c"/".as_ptr(),
                    std::ptr::null(),
                    libc::MS_REC | libc::MS_PRIVATE,
                    std::ptr::null(),
                )
            })?;
            raw(unsafe { libc::chdir(c"/".as_ptr()) })?;
            raw(unsafe {
                libc::mount(
                    c"tmpfs".as_ptr(),
                    c"/tmp".as_ptr(),
                    c"tmpfs".as_ptr(),
                    libc::MS_NOSUID | libc::MS_NODEV,
                    c"mode=0755,size=16m".as_ptr().cast(),
                )
            })?;
            let root = raw(unsafe {
                libc::open(
                    c"/tmp".as_ptr(),
                    libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
                )
            })?;
            for mount in &self.mounts {
                mount.install(root)?;
            }
            raw(unsafe { libc::close(root) })?;
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
            })?;
            raw(unsafe { libc::chroot(c"/tmp".as_ptr()) })?;
            raw(unsafe { libc::chdir(c"/".as_ptr()) })?;
        }
        // Namespaced root may not regain capabilities at exec.
        {
            const LOCKED_NO_ROOT_OR_AMBIENT: libc::c_ulong = 1 | 2 | 4 | 8 | 32 | 64 | 128;
            raw(unsafe {
                libc::prctl(libc::PR_SET_SECUREBITS, LOCKED_NO_ROOT_OR_AMBIENT, 0, 0, 0)
            })?;
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
        })?;
        // Mark, rather than close, std::process's private exec-error pipe.
        // It must survive failed setup and disappear with all ambient handles
        // at the first exec. A declared custody fd3 is installed afterward.
        raw_long(unsafe {
            libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, CLOSE_RANGE_CLOEXEC)
        })?;
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
        })?;
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

impl Mount {
    fn capture(path: &Path, baseline: bool, writable: bool) -> Result<Self, ConfinementRefusal> {
        let canonical =
            std::fs::canonicalize(path).map_err(|_| ConfinementRefusal::UnsupportedRequirement)?;
        if !baseline && canonical != path {
            return Err(ConfinementRefusal::UnsupportedRequirement);
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&canonical)
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
        if metadata.is_file() && metadata.nlink() != 1 {
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
            canonical,
            file,
            identity: (metadata.dev(), metadata.ino()),
            ancestors,
            components,
            destination,
            directory: metadata.is_dir(),
            writable,
            device,
        })
    }

    fn validate(&self) -> Result<(), ConfinementRefusal> {
        let current = std::fs::canonicalize(&self.path)
            .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
        if current != self.canonical {
            return Err(ConfinementRefusal::BackendUnavailable);
        }
        for (path, device, inode) in &self.ancestors {
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
        let pinned = self
            .file
            .metadata()
            .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
        if named.file_type().is_symlink()
            || (named.dev(), named.ino()) != self.identity
            || (pinned.dev(), pinned.ino()) != self.identity
        {
            return Err(ConfinementRefusal::BackendUnavailable);
        }
        Ok(())
    }

    fn install(&self, root: i32) -> Result<(), i32> {
        let mut parent = raw(unsafe { libc::fcntl(root, libc::F_DUPFD_CLOEXEC, 3) })?;
        for (index, name) in self.components.iter().enumerate() {
            let final_component = index + 1 == self.components.len();
            let directory = !final_component || self.directory;
            if directory {
                let result = unsafe { libc::mkdirat(parent, name.as_ptr(), 0o755) };
                if result == -1 && errno() != libc::EEXIST {
                    return Err(errno());
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
            })?;
            raw(unsafe { libc::close(parent) })?;
            parent = next;
        }
        let mut source = [0_u8; 32];
        let source = fd_path(self.file.as_raw_fd(), &mut source);
        // These target nodes were created in this private empty tmpfs. Roots
        // are disjoint, so no mounted source can replace an ancestor here.
        // Resolve the fresh path for remount rather than an FD to the covered
        // pre-bind inode, which would still refer to the old tmpfs mount.
        let destination = self.destination.as_ptr();
        raw(unsafe {
            libc::mount(
                source,
                destination,
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        })?;
        // Nonrecursive binds do not import hidden host submounts. The mounted
        // root cannot be replaced or renamed by the target. Read-only remount
        // also blocks metadata writes, not only file data writes.
        let mut flags = libc::MS_BIND | libc::MS_REMOUNT | libc::MS_NOSUID;
        if !self.device {
            flags |= libc::MS_NODEV;
        }
        if !self.writable {
            flags |= libc::MS_RDONLY;
        }
        raw(unsafe {
            libc::mount(
                std::ptr::null(),
                destination,
                std::ptr::null(),
                flags,
                std::ptr::null(),
            )
        })?;
        raw(unsafe { libc::close(parent) })?;
        Ok(())
    }
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
        libc::SYS_ioctl,
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
    policy.validate_launch(&launch).map_err(io::Error::other)?;
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
    // SAFETY: The closure invokes only syscall helpers using preallocated
    // strings, policy and FDs. Returning an error uses Command's existing
    // owned error pipe; spawn does not report success after a setup failure.
    unsafe {
        command.pre_exec(move || {
            policy.apply().map_err(io::Error::from_raw_os_error)?;
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
#[allow(clippy::unwrap_used)]
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
}
