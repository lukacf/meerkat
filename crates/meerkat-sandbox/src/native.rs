//! Native macOS spawning into the existing exclusive child owner.

#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use nix::libc;
use tokio::process::{ChildStderr, ChildStdout};
use tokio::signal::unix::{SignalKind, signal};

use crate::native_child::{NativeChild, SpawnIo};
use crate::native_stdio::{duplicate, inherit, input, output};

unsafe extern "C" {
    fn posix_spawn_file_actions_addchdir_np(
        actions: *mut libc::posix_spawn_file_actions_t,
        path: *const libc::c_char,
    ) -> libc::c_int;
}

pub(crate) fn spawn(
    program: &Path,
    args: &[OsString],
    directory: &Path,
    environment: &BTreeMap<OsString, OsString>,
    streams: SpawnIo,
    gate: Option<BorrowedFd<'_>>,
) -> io::Result<NativeChild> {
    if !program.is_absolute() || !directory.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "native launch requires absolute paths",
        ));
    }
    let executable = c_string(program.as_os_str())?;
    let directory = c_string(directory.as_os_str())?;
    let arguments = std::iter::once(program.as_os_str())
        .chain(args.iter().map(OsString::as_os_str))
        .map(c_string)
        .collect::<io::Result<Vec<_>>>()?;
    let argv = pointers(&arguments);
    let environment = environment
        .iter()
        .map(|(key, value)| {
            if key.is_empty() || key.as_bytes().contains(&b'=') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid native environment key",
                ));
            }
            let mut entry = key.clone();
            entry.push("=");
            entry.push(value);
            c_string(&entry)
        })
        .collect::<io::Result<Vec<_>>>()?;
    let envp = pointers(&environment);

    // Snapshot inherited stdio before opening anything that could temporarily
    // occupy a host descriptor that was closed when this function was called.
    let inherited_stdin = inherit(streams.stdin, libc::STDIN_FILENO)?;
    let inherited_stdout = inherit(streams.stdout, libc::STDOUT_FILENO)?;
    let inherited_stderr = inherit(streams.stderr, libc::STDERR_FILENO)?;
    // Subscribe before spawn: even an immediately exiting child must be noticed.
    let sigchld = signal(SignalKind::child())?;
    let (stdin_source, stdin) = input(streams.stdin, inherited_stdin)?;
    let (stdout_source, stdout) = output(streams.stdout, inherited_stdout)?;
    let (stderr_source, stderr) = output(streams.stderr, inherited_stderr)?;
    let stdout = stdout
        .map(|fd| ChildStdout::from_std(fd.into()))
        .transpose()?;
    let stderr = stderr
        .map(|fd| ChildStderr::from_std(fd.into()))
        .transpose()?;
    let sources = [stdin_source, stdout_source, stderr_source];
    // Only the fixed release gate receives descriptor 3, which it closes
    // before exec. The host's source remains private and close-on-exec.
    let gate = gate.map(|fd| duplicate(fd.as_raw_fd())).transpose()?;

    let mut actions = FileActions::new()?;
    let mut attributes = Attributes::new()?;
    // SAFETY: Initialized actions/attributes and all C strings remain live
    // through posix_spawn. Each child source is owned and above descriptors 0-2.
    unsafe {
        check(posix_spawn_file_actions_addchdir_np(
            std::ptr::addr_of_mut!(actions.0),
            directory.as_ptr(),
        ))?;
        for (target, source) in sources.iter().enumerate() {
            check(libc::posix_spawn_file_actions_adddup2(
                std::ptr::addr_of_mut!(actions.0),
                source.as_raw_fd(),
                target as i32,
            ))?;
        }
        if let Some(gate) = &gate {
            check(libc::posix_spawn_file_actions_adddup2(
                std::ptr::addr_of_mut!(actions.0),
                gate.as_raw_fd(),
                3,
            ))?;
        }
        check(libc::posix_spawnattr_setpgroup(
            std::ptr::addr_of_mut!(attributes.0),
            0,
        ))?;
        let mut defaults = MaybeUninit::uninit();
        syscall(libc::sigemptyset(defaults.as_mut_ptr()))?;
        let mut defaults = defaults.assume_init();
        syscall(libc::sigaddset(
            std::ptr::addr_of_mut!(defaults),
            libc::SIGPIPE,
        ))?;
        check(libc::posix_spawnattr_setsigdefault(
            std::ptr::addr_of_mut!(attributes.0),
            std::ptr::addr_of!(defaults),
        ))?;
        // Deliberately inherit the calling thread's signal mask, as Command
        // does. Only SIGPIPE's disposition is reset to the child default.
        check(libc::posix_spawnattr_setflags(
            std::ptr::addr_of_mut!(attributes.0),
            (libc::POSIX_SPAWN_CLOEXEC_DEFAULT
                | libc::POSIX_SPAWN_SETPGROUP
                | libc::POSIX_SPAWN_SETSIGDEF) as _,
        ))?;
        let mut pid = 0;
        check(libc::posix_spawn(
            std::ptr::addr_of_mut!(pid),
            executable.as_ptr(),
            std::ptr::addr_of!(actions.0),
            std::ptr::addr_of!(attributes.0),
            argv.as_ptr(),
            envp.as_ptr(),
        ))?;
        // No fallible operation or await may precede acquisition of this PID.
        Ok(NativeChild::from_spawned_pid(
            pid, sigchld, stdin, stdout, stderr,
        ))
    }
}

fn c_string(value: &OsStr) -> io::Result<CString> {
    CString::new(value.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "nul byte in native launch"))
}

fn pointers(values: &[CString]) -> Vec<*mut libc::c_char> {
    values
        .iter()
        .map(|value| value.as_ptr().cast_mut())
        .chain(std::iter::once(ptr::null_mut()))
        .collect()
}

fn check(result: libc::c_int) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(result))
    }
}

fn syscall(result: libc::c_int) -> io::Result<()> {
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

struct FileActions(libc::posix_spawn_file_actions_t);

impl FileActions {
    fn new() -> io::Result<Self> {
        let mut value = MaybeUninit::uninit();
        // SAFETY: Initialization succeeds before ownership is created.
        check(unsafe { libc::posix_spawn_file_actions_init(value.as_mut_ptr()) })?;
        Ok(Self(unsafe { value.assume_init() }))
    }
}

impl Drop for FileActions {
    fn drop(&mut self) {
        // SAFETY: This owner destroys its initialized object exactly once.
        unsafe { libc::posix_spawn_file_actions_destroy(std::ptr::addr_of_mut!(self.0)) };
    }
}

struct Attributes(libc::posix_spawnattr_t);

impl Attributes {
    fn new() -> io::Result<Self> {
        let mut value = MaybeUninit::uninit();
        // SAFETY: Initialization succeeds before ownership is created.
        check(unsafe { libc::posix_spawnattr_init(value.as_mut_ptr()) })?;
        Ok(Self(unsafe { value.assume_init() }))
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: This owner destroys its initialized object exactly once.
        unsafe { libc::posix_spawnattr_destroy(std::ptr::addr_of_mut!(self.0)) };
    }
}
