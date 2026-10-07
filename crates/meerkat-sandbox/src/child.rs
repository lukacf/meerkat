//! Mechanical child handle used by the existing process owners.

use std::io;
use std::process::ExitStatus;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};

/// One exclusively owned child. Process-group containment and durable custody
/// remain with the caller.
#[derive(Debug)]
pub struct ProcessChild {
    child: ChildKind,
}

#[derive(Debug)]
enum ChildKind {
    Tokio(Child),
    #[cfg(target_os = "macos")]
    Native(crate::NativeChild),
}

impl From<Child> for ProcessChild {
    fn from(child: Child) -> Self {
        Self {
            child: ChildKind::Tokio(child),
        }
    }
}

#[cfg(target_os = "macos")]
impl From<crate::NativeChild> for ProcessChild {
    fn from(child: crate::NativeChild) -> Self {
        Self {
            child: ChildKind::Native(child),
        }
    }
}

impl ProcessChild {
    #[must_use]
    pub fn id(&self) -> Option<u32> {
        match &self.child {
            ChildKind::Tokio(child) => child.id(),
            #[cfg(target_os = "macos")]
            ChildKind::Native(child) => child.id(),
        }
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        match &mut self.child {
            ChildKind::Tokio(child) => child.stdin.take(),
            #[cfg(target_os = "macos")]
            ChildKind::Native(child) => child.stdin.take(),
        }
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        match &mut self.child {
            ChildKind::Tokio(child) => child.stdout.take(),
            #[cfg(target_os = "macos")]
            ChildKind::Native(child) => child.stdout.take(),
        }
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        match &mut self.child {
            ChildKind::Tokio(child) => child.stderr.take(),
            #[cfg(target_os = "macos")]
            ChildKind::Native(child) => child.stderr.take(),
        }
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match &mut self.child {
            ChildKind::Tokio(child) => child.try_wait(),
            #[cfg(target_os = "macos")]
            ChildKind::Native(child) => child.try_wait(),
        }
    }

    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        match &mut self.child {
            ChildKind::Tokio(child) => child.wait().await,
            #[cfg(target_os = "macos")]
            ChildKind::Native(child) => child.wait().await,
        }
    }

    pub fn start_kill(&mut self) -> io::Result<()> {
        match &mut self.child {
            ChildKind::Tokio(child) => child.start_kill(),
            #[cfg(target_os = "macos")]
            ChildKind::Native(child) => child.start_kill(),
        }
    }

    pub async fn kill(&mut self) -> io::Result<()> {
        match &mut self.child {
            ChildKind::Tokio(child) => child.kill().await,
            #[cfg(target_os = "macos")]
            ChildKind::Native(child) => child.kill().await,
        }
    }
}
