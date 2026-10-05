//! LinuxLandlockSeccompV1: Landlock filesystem rules, Landlock scopes and a
//! seccomp filter, installed by the forked child before it executes the
//! launch. The host thread is never restricted and no supervisor process is
//! added: the child keeps the PID the caller's custody recorded.
//!
//! Landlock is allow-only. A requirement this profile cannot represent
//! exactly is refused (`UnsupportedRequirement`), never widened:
//! - a literal directory, or a grant whose path is missing, has a symlink
//!   component, or is not a file or directory;
//! - an exclusion inside a grant (allow-minus-deny), including the baseline;
//! - exact IP endpoints (Landlock TCP rules are port-only and miss UDP);
//! - any `unix_connect` grant (this kernel interface has no pathname-socket
//!   right; with none requested, AF_UNIX socket creation is denied);
//! - descendant termination (no whole-tree ownership without namespaces).
//!
//! Absent kernel facilities (Landlock ABI below 6, seccomp filters,
//! `close_range` with CLOSE_RANGE_CLOEXEC) are `BackendUnavailable`.

#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use meerkat_core::confinement::{
    ConfinementRefusal, ExecutionConfinement, FilesystemAccess, IpNetworkAccess, PathAccess,
};
use nix::libc;
use tokio::process::{Child, Command};

/// The only descriptors a confined child may inherit are stdio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdioMode {
    /// Read EOF or discard output through `/dev/null`.
    Null,
    /// Give the caller an asynchronous pipe endpoint.
    Piped,
    /// Duplicate the corresponding standard descriptor of the host.
    Inherit,
}

/// Standard streams for a launch. This does not change bound launch data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpawnIo {
    pub stdin: StdioMode,
    pub stdout: StdioMode,
    pub stderr: StdioMode,
}

impl Default for SpawnIo {
    fn default() -> Self {
        Self {
            stdin: StdioMode::Null,
            stdout: StdioMode::Piped,
            stderr: StdioMode::Piped,
        }
    }
}

fn stdio(mode: StdioMode) -> Stdio {
    match mode {
        StdioMode::Null => Stdio::null(),
        StdioMode::Piped => Stdio::piped(),
        StdioMode::Inherit => Stdio::inherit(),
    }
}

// Landlock UAPI (include/uapi/linux/landlock.h).
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
const LANDLOCK_RULE_PATH_BENEATH: libc::c_int = 1;
const REQUIRED_ABI: libc::c_long = 6;

const FS_EXECUTE: u64 = 1 << 0;
const FS_WRITE_FILE: u64 = 1 << 1;
const FS_READ_FILE: u64 = 1 << 2;
const FS_READ_DIR: u64 = 1 << 3;
const FS_REMOVE_DIR: u64 = 1 << 4;
const FS_REMOVE_FILE: u64 = 1 << 5;
const FS_MAKE_CHAR: u64 = 1 << 6;
const FS_MAKE_DIR: u64 = 1 << 7;
const FS_MAKE_REG: u64 = 1 << 8;
const FS_MAKE_SOCK: u64 = 1 << 9;
const FS_MAKE_FIFO: u64 = 1 << 10;
const FS_MAKE_BLOCK: u64 = 1 << 11;
const FS_MAKE_SYM: u64 = 1 << 12;
const FS_REFER: u64 = 1 << 13;
const FS_TRUNCATE: u64 = 1 << 14;
const FS_IOCTL_DEV: u64 = 1 << 15;
/// Every filesystem right of ABI 5 and later is handled: an unhandled right
/// would be silently unrestricted.
const FS_HANDLED: u64 = (1 << 16) - 1;
const NET_BIND_TCP: u64 = 1 << 0;
const NET_CONNECT_TCP: u64 = 1 << 1;
const SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
const SCOPE_SIGNAL: u64 = 1 << 1;

/// Rights a rule on a non-directory may carry.
const FILE_RIGHTS: u64 = FS_EXECUTE | FS_WRITE_FILE | FS_READ_FILE | FS_TRUNCATE | FS_IOCTL_DEV;
/// EXECUTE is not a boundary for readable files: the loader can map them.
const READ_RIGHTS: u64 = FS_READ_FILE | FS_READ_DIR | FS_EXECUTE;
const WRITE_RIGHTS: u64 = FS_WRITE_FILE
    | FS_TRUNCATE
    | FS_REMOVE_DIR
    | FS_REMOVE_FILE
    | FS_MAKE_CHAR
    | FS_MAKE_DIR
    | FS_MAKE_REG
    | FS_MAKE_SOCK
    | FS_MAKE_FIFO
    | FS_MAKE_BLOCK
    | FS_MAKE_SYM
    | FS_REFER
    | FS_IOCTL_DEV;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

/// CommandRuntimeV1 on Linux: loaders, system libraries and executables,
/// the system configuration standard tools fail without, and the null and
/// random devices. No /proc, /sys, home directory or shared scratch. Entries
/// absent from this host, or reached through a symlink, are skipped: merged
/// /bin, /sbin and /lib resolve beneath /usr.
const BASELINE_READ: [&str; 15] = [
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/etc/ld.so.cache",
    "/etc/localtime",
    "/etc/nsswitch.conf",
    "/etc/passwd",
    "/etc/group",
    // OpenSSL configuration and trust anchors (node fails to start without
    // openssl.cnf) and git's system configuration (EACCES on it is fatal).
    "/etc/ssl",
    "/etc/gitconfig",
    "/dev/zero",
];
const BASELINE_DEVICES_READ: [&str; 2] = ["/dev/random", "/dev/urandom"];
const BASELINE_READ_WRITE: [&str; 1] = ["/dev/null"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// Skipped when absent or symlinked on this host.
    Baseline,
    /// Required: absence or a changed object type refuses the launch.
    Requirement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Directory,
    NonDirectory,
}

#[derive(Debug, Clone)]
struct Rule {
    path: PathBuf,
    shape: Shape,
    origin: Origin,
    read: bool,
    write: bool,
}

impl Rule {
    fn rights(&self) -> u64 {
        let mut rights = 0;
        if self.read {
            rights |= READ_RIGHTS;
        }
        if self.write {
            rights |= WRITE_RIGHTS;
        }
        match self.shape {
            Shape::Directory => rights,
            Shape::NonDirectory => rights & FILE_RIGHTS,
        }
    }

    /// Whether this rule covers `path` (a directory rule covers descendants).
    fn reaches(&self, path: &Path) -> bool {
        match self.shape {
            Shape::Directory => path.starts_with(&self.path),
            Shape::NonDirectory => path == self.path,
        }
    }
}

/// Immutable lowering shared by every launch bound from one compilation.
pub(crate) struct LinuxPolicy {
    rules: Vec<Rule>,
    handled_access_net: u64,
    filter: Box<[libc::sock_filter]>,
}

impl std::fmt::Debug for LinuxPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LinuxPolicy([REDACTED])")
    }
}

/// Per-launch installation: a ruleset over the objects the policy paths name
/// at bind time, plus the compiled filter.
pub(crate) struct LinuxInstallation {
    ruleset: OwnedFd,
    policy: Arc<LinuxPolicy>,
}

fn errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn landlock_abi() -> Option<libc::c_long> {
    // SAFETY: The version query reads no attribute and creates no descriptor.
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<RulesetAttr>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    (abi > 0).then_some(abi)
}

fn seccomp_filters_available() -> bool {
    // SAFETY: Read-only prctl query of this thread's seccomp mode.
    if unsafe { libc::prctl(libc::PR_GET_SECCOMP) } < 0 {
        return false;
    }
    [libc::SECCOMP_RET_KILL_PROCESS, libc::SECCOMP_RET_ERRNO]
        .into_iter()
        .all(|action| {
            // SAFETY: The kernel reads one u32 action from this live local.
            unsafe {
                libc::syscall(
                    libc::SYS_seccomp,
                    libc::SECCOMP_GET_ACTION_AVAIL,
                    0u32,
                    std::ptr::from_ref(&action),
                ) == 0
            }
        })
}

fn close_range_cloexec_available() -> bool {
    let Ok(file) = std::fs::File::open("/dev/null") else {
        return false;
    };
    let fd = file.as_raw_fd() as libc::c_uint;
    // SAFETY: Marks only this owned descriptor close-on-exec (already set).
    unsafe { libc::syscall(libc::SYS_close_range, fd, fd, libc::CLOSE_RANGE_CLOEXEC) == 0 }
}

/// Open the exact object a policy path names, refusing every symlink
/// component (including magic links) instead of granting its current target.
fn open_exact(path: &Path) -> io::Result<OwnedFd> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: open_how is plain data; zeroed covers the reserved mode field.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (libc::O_PATH | libc::O_CLOEXEC) as u64;
    how.resolve = libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS;
    // SAFETY: Both pointers are live for the call; size matches the struct.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD,
            path.as_ptr(),
            std::ptr::from_ref(&how),
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat2 returned a new descriptor owned by nobody else.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
}

fn shape_of(fd: &OwnedFd) -> io::Result<Option<Shape>> {
    // SAFETY: fstat writes into this live, correctly sized local.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), std::ptr::from_mut(&mut stat)) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(match stat.st_mode & libc::S_IFMT {
        libc::S_IFDIR => Some(Shape::Directory),
        libc::S_IFREG | libc::S_IFCHR => Some(Shape::NonDirectory),
        _ => None,
    })
}

fn requirement_shape(path: &Path) -> Result<Shape, ConfinementRefusal> {
    let fd = open_exact(path).map_err(|error| match error.raw_os_error() {
        // Missing, symlinked or non-directory components cannot be bound to
        // an exact existing object without widening to a parent.
        Some(libc::ENOENT | libc::ELOOP | libc::ENOTDIR | libc::EXDEV) => {
            ConfinementRefusal::UnsupportedRequirement
        }
        _ => ConfinementRefusal::PreparationFailed,
    })?;
    shape_of(&fd)
        .map_err(|_| ConfinementRefusal::PreparationFailed)?
        .ok_or(ConfinementRefusal::UnsupportedRequirement)
}

fn baseline_shape(path: &Path) -> Option<Shape> {
    open_exact(path)
        .ok()
        .and_then(|fd| shape_of(&fd).ok().flatten())
}

/// Exclusions are matched lexically against grant paths, so an exclusion
/// must not traverse a symlink that could place it inside a grant.
fn validate_exclusion(path: &Path) -> Result<(), ConfinementRefusal> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ConfinementRefusal::UnsupportedRequirement);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(ConfinementRefusal::PreparationFailed),
        }
    }
    Ok(())
}

fn add_requirement_rules(
    rules: &mut Vec<Rule>,
    access: &FilesystemAccess,
    read: bool,
) -> Result<(), ConfinementRefusal> {
    let paths = match access {
        FilesystemAccess::Unrestricted => {
            rules.push(Rule {
                path: PathBuf::from("/"),
                shape: Shape::Directory,
                origin: Origin::Requirement,
                read,
                write: !read,
            });
            return Ok(());
        }
        FilesystemAccess::Paths(paths) => paths,
    };
    for access in paths {
        let shape = requirement_shape(access.path())?;
        if matches!((access, shape), (PathAccess::Literal(_), Shape::Directory)) {
            // A directory rule always covers descendants.
            return Err(ConfinementRefusal::UnsupportedRequirement);
        }
        rules.push(Rule {
            path: access.path().to_owned(),
            shape,
            origin: Origin::Requirement,
            read,
            write: !read,
        });
    }
    Ok(())
}

/// Apply one exclusion to the rules of its access class. An exclusion that
/// covers a whole rule removes that right from it; one that falls inside a
/// rule cannot be subtracted by Landlock and is refused.
fn exclude(
    rules: &mut [Rule],
    exclusion: &PathAccess,
    read_class: bool,
) -> Result<(), ConfinementRefusal> {
    let denied = exclusion.path();
    for rule in rules.iter_mut() {
        // A read exclusion also denies writes, so the resource cannot be
        // renamed or linked into a readable place.
        let applies = if read_class {
            rule.read || rule.write
        } else {
            rule.write
        };
        if !applies {
            continue;
        }
        let covers = match exclusion {
            PathAccess::Subtree(root) => rule.path.starts_with(root),
            PathAccess::Literal(path) => rule.shape == Shape::NonDirectory && rule.path == *path,
        };
        if covers {
            rule.write = false;
            if read_class {
                rule.read = false;
            }
        } else if rule.reaches(denied) {
            return Err(ConfinementRefusal::UnsupportedRequirement);
        }
    }
    Ok(())
}

pub(crate) fn validate_support(
    requirement: &ExecutionConfinement,
) -> Result<(), ConfinementRefusal> {
    let spec = requirement.specification();
    if spec.require_descendant_termination
        || !spec.unix_connect.is_empty()
        || matches!(&spec.network, IpNetworkAccess::Connect(endpoints) if !endpoints.is_empty())
    {
        return Err(ConfinementRefusal::UnsupportedRequirement);
    }
    Ok(())
}

fn facilities_available() -> bool {
    landlock_abi().is_some_and(|abi| abi >= REQUIRED_ABI)
        && seccomp_filters_available()
        && close_range_cloexec_available()
}

pub(crate) fn compile(
    requirement: &ExecutionConfinement,
) -> Result<Arc<LinuxPolicy>, ConfinementRefusal> {
    validate_support(requirement)?;
    if !facilities_available() {
        return Err(ConfinementRefusal::BackendUnavailable);
    }
    let spec = requirement.specification();
    let mut rules = Vec::new();
    for (paths, write) in [
        (&BASELINE_READ[..], false),
        (&BASELINE_DEVICES_READ[..], false),
        (&BASELINE_READ_WRITE[..], true),
    ] {
        for path in paths {
            let path = Path::new(path);
            if let Some(shape) = baseline_shape(path) {
                rules.push(Rule {
                    path: path.to_owned(),
                    shape,
                    origin: Origin::Baseline,
                    read: true,
                    write,
                });
            }
        }
    }
    add_requirement_rules(&mut rules, &spec.read, true)?;
    add_requirement_rules(&mut rules, &spec.write, false)?;
    for exclusion in &spec.deny_read {
        validate_exclusion(exclusion.path())?;
        exclude(&mut rules, exclusion, true)?;
    }
    for exclusion in &spec.deny_write {
        validate_exclusion(exclusion.path())?;
        exclude(&mut rules, exclusion, false)?;
    }
    rules.retain(|rule| rule.rights() != 0);
    let unrestricted_ip = matches!(spec.network, IpNetworkAccess::Unrestricted);
    let policy = Arc::new(LinuxPolicy {
        rules,
        handled_access_net: if unrestricted_ip {
            0
        } else {
            NET_BIND_TCP | NET_CONNECT_TCP
        },
        filter: seccomp::program(unrestricted_ip).into_boxed_slice(),
    });
    // Prove the whole rule set binds now; launches re-bind the same paths.
    install(&policy)?;
    Ok(policy)
}

/// Build the per-launch ruleset from the immutable lowering. Each policy path
/// is reopened without following symlinks, so a launch grants the object the
/// path names now and a replaced or retargeted requirement path never widens.
pub(crate) fn install(policy: &Arc<LinuxPolicy>) -> Result<LinuxInstallation, ConfinementRefusal> {
    let attr = RulesetAttr {
        handled_access_fs: FS_HANDLED,
        handled_access_net: policy.handled_access_net,
        scoped: SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL,
    };
    // SAFETY: The kernel reads `attr` for the given size and returns a new fd.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::from_ref(&attr),
            std::mem::size_of::<RulesetAttr>(),
            0u32,
        )
    };
    if fd < 0 {
        return Err(match errno() {
            libc::ENOSYS | libc::EOPNOTSUPP => ConfinementRefusal::BackendUnavailable,
            _ => ConfinementRefusal::PreparationFailed,
        });
    }
    // SAFETY: landlock_create_ruleset returned a new descriptor owned here.
    let ruleset = unsafe { OwnedFd::from_raw_fd(fd as RawFd) };
    for rule in &policy.rules {
        let object = match open_exact(&rule.path) {
            Ok(object) => object,
            Err(_) if rule.origin == Origin::Baseline => continue,
            Err(_) => return Err(ConfinementRefusal::PreparationFailed),
        };
        match shape_of(&object) {
            Ok(Some(shape)) if shape == rule.shape => {}
            _ if rule.origin == Origin::Baseline => continue,
            _ => return Err(ConfinementRefusal::PreparationFailed),
        }
        let beneath = PathBeneathAttr {
            allowed_access: rule.rights(),
            parent_fd: object.as_raw_fd(),
        };
        // SAFETY: Both descriptors are live; the kernel reads `beneath`.
        let added = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                ruleset.as_raw_fd(),
                LANDLOCK_RULE_PATH_BENEATH,
                std::ptr::from_ref(&beneath),
                0u32,
            )
        };
        if added != 0 {
            return Err(ConfinementRefusal::PreparationFailed);
        }
    }
    Ok(LinuxInstallation {
        ruleset,
        policy: Arc::clone(policy),
    })
}

/// Restrictions the forked child installs on itself before exec. Only raw
/// system calls on integers and pre-built memory: no allocation, no locks.
struct ChildInstall {
    ruleset: RawFd,
    gate: Option<RawFd>,
    installation: Arc<LinuxInstallation>,
}

impl ChildInstall {
    /// Runs between fork and exec in the single-threaded child.
    fn apply(&self) -> io::Result<()> {
        let last_error = || Err(io::Error::last_os_error());
        // SAFETY: Every call below is a raw async-signal-safe system call on
        // integers or on memory owned by `installation`, alive in this child.
        unsafe {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return last_error();
            }
            if libc::syscall(libc::SYS_landlock_restrict_self, self.ruleset, 0u32) != 0 {
                return last_error();
            }
            let filter = &self.installation.policy.filter;
            let program = libc::sock_fprog {
                len: filter.len() as libc::c_ushort,
                filter: filter.as_ptr().cast_mut(),
            };
            if libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                std::ptr::from_ref(&program),
            ) != 0
            {
                return last_error();
            }
            // Seal every descriptor above stdio without a bounded scan: the
            // ruleset, inherited sockets and files close at exec. Close-on-exec
            // keeps the spawn error pipe working until then.
            if libc::syscall(
                libc::SYS_close_range,
                3 as libc::c_uint,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            ) != 0
            {
                return last_error();
            }
            if let Some(gate) = self.gate {
                // The fixed gate prologue alone reads descriptor 3 and closes
                // it before entering the launch.
                if gate == GATE_FD {
                    if libc::fcntl(GATE_FD, libc::F_SETFD, 0) != 0 {
                        return last_error();
                    }
                } else if libc::dup2(gate, GATE_FD) != GATE_FD {
                    return last_error();
                }
            }
        }
        Ok(())
    }
}

const GATE_FD: RawFd = 3;
// Fixed host code only; target arguments remain positional data. Same
// prologue as the macOS gate and the trusted-host custody gate.
const GATE_PROLOGUE: &str = "IFS= read -r meerkat_custody_gate <&3 || exit 125; case $meerkat_custody_gate in \"$1\") ;; *) exit 125 ;; esac; exec 3<&-; shift; exec \"$@\"";

pub(crate) fn spawn(
    installation: LinuxInstallation,
    program: &Path,
    arguments: &[OsString],
    directory: &Path,
    environment: &BTreeMap<OsString, OsString>,
    streams: SpawnIo,
    gate: Option<(BorrowedFd<'_>, &OsStr)>,
) -> io::Result<Child> {
    let mut command = match gate {
        Some((_, token)) => {
            if token.is_empty()
                || token
                    .as_bytes()
                    .iter()
                    .any(|byte| matches!(byte, 0 | b'\n' | b'\r'))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid custody release token",
                ));
            }
            let mut command = Command::new("/bin/sh");
            command
                .arg("-c")
                .arg(GATE_PROLOGUE)
                .arg("meerkat-custody-gate")
                .arg(token)
                .arg(program);
            command
        }
        None => Command::new(program),
    };
    command
        .args(arguments)
        .current_dir(directory)
        .env_clear()
        .envs(environment)
        .stdin(stdio(streams.stdin))
        .stdout(stdio(streams.stdout))
        .stderr(stdio(streams.stderr))
        .process_group(0)
        .kill_on_drop(true);
    let installation = Arc::new(installation);
    let child_install = ChildInstall {
        ruleset: installation.ruleset.as_raw_fd(),
        gate: gate.map(|(reader, _)| reader.as_raw_fd()),
        installation: Arc::clone(&installation),
    };
    // SAFETY: `apply` performs only async-signal-safe system calls on data
    // prepared before fork; it allocates nothing and takes no locks.
    unsafe {
        command.pre_exec(move || child_install.apply());
    }
    let child = command.spawn();
    // The host's ruleset copy closes once the child holds its own.
    drop(command);
    drop(installation);
    child
}

mod seccomp {
    use nix::libc;

    #[cfg(target_arch = "x86_64")]
    const AUDIT_ARCH_NATIVE: u32 = 0xC000_003E;
    #[cfg(target_arch = "aarch64")]
    const AUDIT_ARCH_NATIVE: u32 = 0xC000_00B7;
    #[cfg(target_arch = "x86_64")]
    const X32_SYSCALL_BIT: u32 = 0x4000_0000;

    const RET_ALLOW: u32 = libc::SECCOMP_RET_ALLOW;
    const RET_KILL_PROCESS: u32 = libc::SECCOMP_RET_KILL_PROCESS;

    fn ret_errno(errno: i32) -> u32 {
        libc::SECCOMP_RET_ERRNO | (errno as u32 & libc::SECCOMP_RET_DATA)
    }

    // struct seccomp_data: nr (0), arch (4), instruction_pointer (8),
    // args[6] (16 + 8i). The kernel truncates int/unsigned int arguments, so
    // the low little-endian word is the value it acts on.
    const NR: u32 = 0;
    const ARCH: u32 = 4;
    fn arg_low(index: u32) -> u32 {
        16 + 8 * index
    }

    fn load(offset: u32) -> libc::sock_filter {
        stmt((libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16, offset)
    }

    fn ret(value: u32) -> libc::sock_filter {
        stmt((libc::BPF_RET | libc::BPF_K) as u16, value)
    }

    fn stmt(code: u16, k: u32) -> libc::sock_filter {
        libc::sock_filter {
            code,
            jt: 0,
            jf: 0,
            k,
        }
    }

    fn jump(op: u32, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
        libc::sock_filter {
            code: (libc::BPF_JMP | op | libc::BPF_K) as u16,
            jt,
            jf,
            k,
        }
    }

    fn nr(value: libc::c_long) -> u32 {
        value as u32
    }

    /// Deny `syscall` with `errno`, else fall through (accumulator = nr).
    fn deny(program: &mut Vec<libc::sock_filter>, syscall: libc::c_long, errno: i32) {
        program.push(jump(libc::BPF_JEQ, nr(syscall), 0, 1));
        program.push(ret(ret_errno(errno)));
    }

    /// The filter is default-allow: Landlock owns filesystem, signal, ptrace
    /// and abstract-socket policy; this owns socket families and the
    /// interfaces that bypass a socket(2) filter or the native ABI.
    pub(super) fn program(unrestricted_ip: bool) -> Vec<libc::sock_filter> {
        let mut program = vec![
            load(ARCH),
            jump(libc::BPF_JEQ, AUDIT_ARCH_NATIVE, 1, 0),
            ret(RET_KILL_PROCESS),
            load(NR),
        ];
        #[cfg(target_arch = "x86_64")]
        {
            // x32 entry points reuse native numbers with this bit set.
            program.push(jump(libc::BPF_JSET, X32_SYSCALL_BIT, 0, 1));
            program.push(ret(RET_KILL_PROCESS));
        }
        // io_uring can create sockets and open files outside socket(2).
        for syscall in [
            libc::SYS_io_uring_setup,
            libc::SYS_io_uring_enter,
            libc::SYS_io_uring_register,
        ] {
            deny(&mut program, syscall, libc::EPERM);
        }
        // Hardening without a confinement claim: the user's kernel keyrings,
        // new namespaces, BPF and perf.
        for syscall in [
            libc::SYS_keyctl,
            libc::SYS_add_key,
            libc::SYS_request_key,
            libc::SYS_unshare,
            libc::SYS_setns,
            libc::SYS_bpf,
            libc::SYS_perf_event_open,
        ] {
            deny(&mut program, syscall, libc::EPERM);
        }
        // clone3 passes its flags in memory a filter cannot read; libc falls
        // back to clone on ENOSYS, whose flags are checked below.
        deny(&mut program, libc::SYS_clone3, libc::ENOSYS);
        let namespaces = (libc::CLONE_NEWNS
            | libc::CLONE_NEWCGROUP
            | libc::CLONE_NEWUTS
            | libc::CLONE_NEWIPC
            | libc::CLONE_NEWUSER
            | libc::CLONE_NEWPID
            | libc::CLONE_NEWNET) as u32;
        program.extend([
            jump(libc::BPF_JEQ, nr(libc::SYS_clone), 0, 4),
            load(arg_low(0)),
            jump(libc::BPF_JSET, namespaces, 0, 1),
            ret(ret_errno(libc::EPERM)),
            ret(RET_ALLOW),
        ]);
        // Terminal input injection into an inherited host terminal.
        program.extend([
            jump(libc::BPF_JEQ, nr(libc::SYS_ioctl), 0, 6),
            load(arg_low(1)),
            jump(libc::BPF_JEQ, libc::TIOCSTI as u32, 0, 1),
            ret(ret_errno(libc::EPERM)),
            jump(libc::BPF_JEQ, libc::TIOCLINUX as u32, 0, 1),
            ret(ret_errno(libc::EPERM)),
            ret(RET_ALLOW),
        ]);
        // socket(2) family allow-list. AF_UNIX stays denied: no unix_connect
        // grant is supported, and socketpair(2) remains available.
        let allowed: &[i32] = if unrestricted_ip {
            &[libc::AF_INET, libc::AF_INET6]
        } else {
            &[]
        };
        let block = 1 + 2 * allowed.len() + 1;
        program.push(jump(libc::BPF_JEQ, nr(libc::SYS_socket), 0, block as u8));
        program.push(load(arg_low(0)));
        for family in allowed {
            program.push(jump(libc::BPF_JEQ, *family as u32, 0, 1));
            program.push(ret(RET_ALLOW));
        }
        program.push(ret(ret_errno(libc::EPERM)));
        program.push(ret(RET_ALLOW));
        program
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("LinuxLandlockSeccompV1 supports x86_64 and aarch64 only");

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn rule(path: &str, shape: Shape, read: bool, write: bool) -> Rule {
        Rule {
            path: PathBuf::from(path),
            shape,
            origin: Origin::Requirement,
            read,
            write,
        }
    }

    #[test]
    fn exclusion_covering_a_rule_removes_it_and_one_inside_is_refused() {
        let mut rules = vec![
            rule("/work", Shape::Directory, true, true),
            rule("/data/file", Shape::NonDirectory, true, false),
        ];
        exclude(
            &mut rules,
            &PathAccess::Literal(PathBuf::from("/data/file")),
            true,
        )
        .unwrap();
        assert!(!rules[1].read && !rules[1].write);
        assert_eq!(
            exclude(
                &mut rules,
                &PathAccess::Literal(PathBuf::from("/work/secret")),
                true
            ),
            Err(ConfinementRefusal::UnsupportedRequirement)
        );
        let mut rules = vec![rule("/work/pub", Shape::Directory, true, false)];
        exclude(
            &mut rules,
            &PathAccess::Subtree(PathBuf::from("/work")),
            true,
        )
        .unwrap();
        assert_eq!(rules[0].rights(), 0);
    }

    #[test]
    fn write_exclusion_ignores_read_only_rules_but_read_exclusion_covers_writes() {
        let mut rules = vec![rule("/work", Shape::Directory, true, false)];
        exclude(
            &mut rules,
            &PathAccess::Literal(PathBuf::from("/work/x")),
            false,
        )
        .unwrap();
        let mut rules = vec![rule("/work", Shape::Directory, false, true)];
        assert_eq!(
            exclude(
                &mut rules,
                &PathAccess::Literal(PathBuf::from("/work/x")),
                true
            ),
            Err(ConfinementRefusal::UnsupportedRequirement)
        );
    }

    #[test]
    fn file_rules_carry_only_file_rights() {
        let file = rule("/f", Shape::NonDirectory, true, true);
        assert_eq!(file.rights() & !FILE_RIGHTS, 0);
        assert_ne!(file.rights() & FS_WRITE_FILE, 0);
        let dir = rule("/d", Shape::Directory, true, false);
        assert_eq!(dir.rights(), READ_RIGHTS);
    }

    #[test]
    fn filter_jumps_stay_inside_the_program() {
        for unrestricted in [false, true] {
            let program = seccomp::program(unrestricted);
            for (index, instruction) in program.iter().enumerate() {
                if u32::from(instruction.code) & 0x07 == libc::BPF_JMP {
                    let longest = usize::from(instruction.jt.max(instruction.jf));
                    assert!(index + 1 + longest < program.len());
                }
            }
            let last = program.last().unwrap();
            assert_eq!(last.k, libc::SECCOMP_RET_ALLOW);
        }
    }
}
