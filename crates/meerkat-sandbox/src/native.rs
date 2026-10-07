//! Native macOS spawning and exclusive ownership of one child PID.
//!
//! This module has no policy lookup or process-group supervisor. The caller
//! owns group containment; this handle kills and reaps its direct child on drop.

#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{ExitStatus, Output};
use std::ptr;

use nix::libc;
use tokio::io::AsyncReadExt;
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};
use tokio::signal::unix::{Signal, SignalKind, signal};

unsafe extern "C" {
    fn posix_spawn_file_actions_addchdir_np(
        actions: *mut libc::posix_spawn_file_actions_t,
        path: *const libc::c_char,
    ) -> libc::c_int;
}

/// The only descriptors a standalone confined child may inherit are stdio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdioMode {
    /// Read EOF or discard output through `/dev/null`.
    Null,
    /// Give the caller an asynchronous pipe endpoint.
    Piped,
    /// Duplicate the corresponding standard descriptor of the host.
    Inherit,
}

/// Standard streams for a native launch. This does not change bound launch data.
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

/// Exclusive ownership of one native child and its optional standard pipes.
///
/// Do not reap this child through another process owner or install SIGCHLD
/// auto-reaping. Its unreaped PID is the identity used for direct-child signals.
/// Dropping a wait future keeps ownership here. Dropping the handle kills and
/// reaps the direct child; descendant containment remains the caller's job.
pub struct NativeChild {
    pid: Option<libc::pid_t>,
    status: Option<ExitStatus>,
    sigchld: Signal,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

impl std::fmt::Debug for NativeChild {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeChild")
            .field("pid", &self.pid)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl NativeChild {
    /// Returns the leader PID until this owner reaps it or loses wait ownership.
    #[must_use]
    pub fn id(&self) -> Option<u32> {
        self.pid.map(|pid| pid as u32)
    }

    /// Reap only this child if it exited; repeated calls return cached status.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        let pid = self
            .pid
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ECHILD))?;
        loop {
            let mut status = 0;
            // SAFETY: This handle exclusively owns this unreaped child PID.
            let result =
                unsafe { libc::waitpid(pid, std::ptr::addr_of_mut!(status), libc::WNOHANG) };
            if result == 0 {
                return Ok(None);
            }
            if result == pid {
                self.pid = None;
                self.status = Some(ExitStatus::from_raw(status));
                return Ok(self.status);
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.raw_os_error() == Some(libc::ECHILD) {
                // Another owner or auto-reaping invalidated our identity. Never
                // signal this numeric PID again after losing wait ownership.
                self.pid = None;
            }
            return Err(error);
        }
    }

    /// Close retained stdin and await exit without transferring PID ownership.
    /// Cancelling this future leaves the handle available for another wait/kill.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        drop(self.stdin.take());
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            self.sigchld
                .recv()
                .await
                .ok_or_else(|| io::Error::other("child exit notification closed"))?;
        }
    }

    /// Request direct-child termination. Reaping still requires wait or drop.
    pub fn start_kill(&mut self) -> io::Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        if let Some(pid) = self.pid {
            // SAFETY: Only this owner reaps the child, so its PID cannot be reused.
            if unsafe { libc::kill(pid, libc::SIGKILL) } == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    /// Request direct-child termination and reap it. Cancellation retains ownership.
    pub async fn kill(&mut self) -> io::Result<()> {
        self.start_kill()?;
        self.wait().await.map(|_| ())
    }

    /// Close stdin, drain both output pipes concurrently, and reap the child.
    /// Cancelling this owning future drops the handle and triggers child cleanup.
    pub async fn wait_with_output(mut self) -> io::Result<Output> {
        drop(self.stdin.take());
        let stdout = self.stdout.take();
        let stderr = self.stderr.take();
        let read_stdout = async move {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = stdout {
                pipe.read_to_end(&mut bytes).await?;
            }
            Ok::<_, io::Error>(bytes)
        };
        let read_stderr = async move {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = stderr {
                pipe.read_to_end(&mut bytes).await?;
            }
            Ok::<_, io::Error>(bytes)
        };
        let (status, stdout, stderr) = tokio::try_join!(self.wait(), read_stdout, read_stderr)?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

impl Drop for NativeChild {
    fn drop(&mut self) {
        let _ = self.try_wait();
        let Some(pid) = self.pid.take() else { return };
        // SAFETY: The PID remains exclusively owned and unreaped until transfer
        // to the cleanup waiter below. This signals only the direct child.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        // Cleanup also works during Tokio shutdown. There is exactly one reaper
        // for this PID, with no wildcard wait or process-wide child registry.
        if std::thread::Builder::new()
            .name("meerkat-child-cleanup".to_owned())
            .spawn(move || reap(pid))
            .is_err()
        {
            // Resource exhaustion cannot discard wait ownership. This rare
            // fallback may block until the signalled direct child exits.
            reap(pid);
        }
    }
}

fn reap(pid: libc::pid_t) {
    loop {
        // SAFETY: Drop transferred sole ownership of this unreaped child PID.
        let result = unsafe { libc::waitpid(pid, ptr::null_mut(), 0) };
        if result != -1 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            break;
        }
    }
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
        Ok(NativeChild {
            pid: Some(pid),
            status: None,
            sigchld,
            stdin,
            stdout,
            stderr,
        })
    }
}

fn input(mode: StdioMode, inherited: Option<OwnedFd>) -> io::Result<(OwnedFd, Option<ChildStdin>)> {
    match mode {
        StdioMode::Null => Ok((child_fd(File::open("/dev/null")?.into())?, None)),
        StdioMode::Inherit => Ok((
            inherited.ok_or_else(|| io::Error::other("missing inherited stdin"))?,
            None,
        )),
        StdioMode::Piped => {
            let (reader, writer) = io::pipe()?;
            let reader = child_fd(reader.into())?;
            let stdin = ChildStdin::from_std(OwnedFd::from(writer).into())?;
            Ok((reader, Some(stdin)))
        }
    }
}

fn output(mode: StdioMode, inherited: Option<OwnedFd>) -> io::Result<(OwnedFd, Option<OwnedFd>)> {
    match mode {
        StdioMode::Null => Ok((
            child_fd(OpenOptions::new().write(true).open("/dev/null")?.into())?,
            None,
        )),
        StdioMode::Inherit => Ok((
            inherited.ok_or_else(|| io::Error::other("missing inherited output"))?,
            None,
        )),
        StdioMode::Piped => {
            let (reader, writer) = io::pipe()?;
            Ok((child_fd(writer.into())?, Some(reader.into())))
        }
    }
}

fn inherit(mode: StdioMode, descriptor: libc::c_int) -> io::Result<Option<OwnedFd>> {
    if mode == StdioMode::Inherit {
        duplicate(descriptor).map(Some)
    } else {
        Ok(None)
    }
}

fn child_fd(descriptor: OwnedFd) -> io::Result<OwnedFd> {
    duplicate(descriptor.as_raw_fd())
}

fn duplicate(descriptor: libc::c_int) -> io::Result<OwnedFd> {
    // SAFETY: fcntl returns a newly owned descriptor; keeping all child sources
    // above stdio prevents sequential dup2 actions from overwriting a source.
    let duplicate = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 3) };
    syscall(duplicate)?;
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
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
