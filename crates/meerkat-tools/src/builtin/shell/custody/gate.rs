//! Spawn gate: a custody-bound tool cannot run its command until the host
//! has durably recorded the tool's identity.
//!
//! The tool is spawned as a `/bin/sh` prologue that reads one line from
//! descriptor 3, the read end of a pipe whose only writer is the host. The
//! prologue then `exec`s the configured shell with the original arguments,
//! keeping the same pid, start stamp, and process group. If the host dies
//! first, the kernel closes the writer, the read sees EOF, and the prologue
//! exits without running anything.

#![allow(unsafe_code)]

use std::io::Write as _;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;

use tokio::process::Command;

/// Descriptor number the prologue reads the release byte from.
const GATE_FD: i32 = 3;
/// Exit status of a prologue whose gate was never released.
#[cfg(test)]
pub(in crate::builtin::shell) const GATE_NOT_RELEASED_EXIT: i32 = 125;
const GATE_SHELL: &str = "/bin/sh";
/// `$0` is the configured shell and `$@` its arguments. Descriptor 3 is
/// closed before the exec so the tool never inherits it.
const GATE_PROLOGUE: &str =
    "IFS= read -r meerkat_custody_gate <&3 || exit 125; exec 3<&-; exec \"$0\" \"$@\"";

pub(in crate::builtin::shell) struct SpawnGate {
    read: Option<OwnedFd>,
    write: OwnedFd,
}

impl std::fmt::Debug for SpawnGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnGate").finish_non_exhaustive()
    }
}

fn cloexec_pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    #[cfg(target_os = "linux")]
    {
        nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).map_err(std::io::Error::from)
    }
    #[cfg(not(target_os = "linux"))]
    {
        use nix::fcntl::{FcntlArg, FdFlag, fcntl};
        let (read, write) = nix::unistd::pipe().map_err(std::io::Error::from)?;
        for fd in [&read, &write] {
            fcntl(fd.as_raw_fd(), FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))
                .map_err(std::io::Error::from)?;
        }
        Ok((read, write))
    }
}

impl SpawnGate {
    pub(in crate::builtin::shell) fn new() -> std::io::Result<Self> {
        let (read, write) = cloexec_pipe()?;
        Ok(Self {
            read: Some(read),
            write,
        })
    }

    /// Build the gated command that will run `shell_path -c command`.
    pub(in crate::builtin::shell) fn command(
        &self,
        shell_path: &Path,
        command: &str,
    ) -> std::io::Result<Command> {
        let read_fd = self
            .read
            .as_ref()
            .map(AsRawFd::as_raw_fd)
            .ok_or_else(|| std::io::Error::other("spawn gate already consumed"))?;
        let mut cmd = Command::new(GATE_SHELL);
        cmd.arg("-c")
            .arg(GATE_PROLOGUE)
            .arg(shell_path)
            .arg("-c")
            .arg(command);
        // SAFETY: the closure runs in the forked child before exec and calls
        // only async-signal-safe functions (dup2, fcntl) on integer
        // descriptors; it allocates nothing and touches no shared state.
        unsafe {
            cmd.pre_exec(move || {
                if read_fd == GATE_FD {
                    // dup2 onto itself would keep FD_CLOEXEC; clear it.
                    if nix::libc::fcntl(GATE_FD, nix::libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                } else if nix::libc::dup2(read_fd, GATE_FD) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(cmd)
    }

    /// Drop the host's copy of the read end once the child holds its own.
    pub(in crate::builtin::shell) fn spawned(&mut self) {
        self.read = None;
    }

    /// Let the gated prologue exec the tool. Call only after the tool's
    /// identity is durably recorded.
    pub(in crate::builtin::shell) fn release(self) -> std::io::Result<()> {
        let mut writer = std::fs::File::from(self.write);
        writer.write_all(b"\n")
    }
}
