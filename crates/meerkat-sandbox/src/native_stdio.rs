//! Shared explicit stdio preparation before native process creation.
#![allow(unsafe_code)]

use crate::StdioMode;
use nix::libc;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use tokio::process::ChildStdin;

pub(super) fn input(
    mode: StdioMode,
    inherited: Option<OwnedFd>,
) -> io::Result<(OwnedFd, Option<ChildStdin>)> {
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

pub(super) fn output(
    mode: StdioMode,
    inherited: Option<OwnedFd>,
) -> io::Result<(OwnedFd, Option<OwnedFd>)> {
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

pub(super) fn inherit(mode: StdioMode, descriptor: libc::c_int) -> io::Result<Option<OwnedFd>> {
    if mode == StdioMode::Inherit {
        duplicate(descriptor).map(Some)
    } else {
        Ok(None)
    }
}

fn child_fd(descriptor: OwnedFd) -> io::Result<OwnedFd> {
    duplicate(descriptor.as_raw_fd())
}

pub(super) fn duplicate(descriptor: libc::c_int) -> io::Result<OwnedFd> {
    // SAFETY: fcntl returns a newly owned descriptor; keeping all child sources
    // above stdio prevents sequential dup2 actions from overwriting a source.
    let duplicate = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}
