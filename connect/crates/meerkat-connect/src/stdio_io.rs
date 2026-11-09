//! Bounded cancellation-safe framing over the inherited private pipe.
use super::wire::MAX_FRAME;
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsFd, OwnedFd},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf, unix::AsyncFd};

pub(super) struct Pipe(AsyncFd<File>);
impl Pipe {
    pub fn inherited(fd: impl AsFd) -> io::Result<Self> {
        // Duplicate only the inherited child endpoint. No global stdin reader or
        // blocking thread remains alive after an explicit close with stdin open.
        let duplicate = nix::unistd::dup(fd.as_fd()).map_err(io::Error::from)?;
        Self::owned(duplicate)
    }

    pub fn owned(fd: OwnedFd) -> io::Result<Self> {
        // Adopt this exact endpoint, with no second descriptor left outside
        // invocation custody. On every error the owned descriptor is dropped.
        let descriptor_flags = nix::fcntl::fcntl(fd.as_fd(), nix::fcntl::FcntlArg::F_GETFD)
            .map_err(io::Error::from)?;
        nix::fcntl::fcntl(
            fd.as_fd(),
            nix::fcntl::FcntlArg::F_SETFD(
                nix::fcntl::FdFlag::from_bits_truncate(descriptor_flags)
                    | nix::fcntl::FdFlag::FD_CLOEXEC,
            ),
        )
        .map_err(io::Error::from)?;
        let flags = nix::fcntl::fcntl(fd.as_fd(), nix::fcntl::FcntlArg::F_GETFL)
            .map_err(io::Error::from)?;
        nix::fcntl::fcntl(
            fd.as_fd(),
            nix::fcntl::FcntlArg::F_SETFL(
                nix::fcntl::OFlag::from_bits_truncate(flags) | nix::fcntl::OFlag::O_NONBLOCK,
            ),
        )
        .map_err(io::Error::from)?;
        let file = File::from(fd);
        Ok(Self(crate::owned_fd::register(file)?))
    }

    pub fn owned_pair(input: OwnedFd, output: OwnedFd) -> io::Result<(Self, Self)> {
        fn require_fifo(fd: &OwnedFd, access: nix::fcntl::OFlag) -> io::Result<()> {
            let stat = nix::sys::stat::fstat(fd).map_err(io::Error::from)?;
            let flags =
                nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETFL).map_err(io::Error::from)?;
            if nix::sys::stat::SFlag::from_bits_truncate(stat.st_mode)
                & nix::sys::stat::SFlag::S_IFMT
                != nix::sys::stat::SFlag::S_IFIFO
                || nix::fcntl::OFlag::from_bits_truncate(flags) & nix::fcntl::OFlag::O_ACCMODE
                    != access
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "owned transport requires a read-only input FIFO and write-only output FIFO",
                ));
            }
            Ok(())
        }
        // Validate both original endpoints before registration or flag changes.
        // Inherited stdio intentionally keeps its existing wider descriptor API.
        require_fifo(&input, nix::fcntl::OFlag::O_RDONLY)?;
        require_fifo(&output, nix::fcntl::OFlag::O_WRONLY)?;
        let input = Self::owned(input)?;
        let output = Self::owned(output)?;
        Ok((input, output))
    }
}
impl AsyncRead for Pipe {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut ready = std::task::ready!(self.0.poll_read_ready(cx))?;
            match ready.try_io(|fd| {
                let mut file = fd.get_ref();
                file.read(buf.initialize_unfilled())
            }) {
                Ok(Ok(n)) => {
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(e)) => return Poll::Ready(Err(e)),
                Err(_) => continue,
            }
        }
    }
}
impl AsyncWrite for Pipe {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut ready = std::task::ready!(self.0.poll_write_ready(cx))?;
            match ready.try_io(|fd| {
                let mut file = fd.get_ref();
                file.write(buf)
            }) {
                Ok(v) => return Poll::Ready(v),
                Err(_) => continue,
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub(super) struct Frames<R> {
    reader: R,
    pending: Vec<u8>,
    chunk: [u8; 8192],
    start: usize,
    end: usize,
}
impl<R: AsyncRead + Unpin> Frames<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            pending: Vec::new(),
            chunk: [0; 8192],
            start: 0,
            end: 0,
        }
    }
    /// All partial bytes live in this object before any await. Cancelling next()
    /// for a control wakeup cannot erase or concatenate frame fragments.
    pub async fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            if self.start == self.end {
                self.start = 0;
                // Clear both indices before awaiting. A cancelled refill must
                // not make the consumed previous chunk readable again.
                self.end = 0;
                self.end = self.reader.read(&mut self.chunk).await?;
                if self.end == 0 {
                    return if self.pending.is_empty() {
                        Ok(None)
                    } else {
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "partial JSONL frame",
                        ))
                    };
                }
            }
            let bytes = &self.chunk[self.start..self.end];
            let take = bytes
                .iter()
                .position(|b| *b == b'\n')
                .map(|n| n + 1)
                .unwrap_or(bytes.len());
            if self.pending.len() + take > MAX_FRAME + 1 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "JSONL frame limit",
                ));
            }
            self.pending.extend_from_slice(&bytes[..take]);
            self.start += take;
            if self.pending.last() == Some(&b'\n') {
                self.pending.pop();
                return Ok(Some(std::mem::take(&mut self.pending)));
            }
        }
    }
}

/// Refuse before the serialized bytes exceed the native frame ceiling.
pub(super) struct BoundedFrame {
    pub bytes: Vec<u8>,
    pub limit: usize,
}
impl Write for BoundedFrame {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSONL frame limit",
            ));
        }
        self.bytes
            .try_reserve_exact(bytes.len())
            .map_err(io::Error::other)?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "stdio_owned_pipe_tests.rs"]
mod owned_pipe_tests;
