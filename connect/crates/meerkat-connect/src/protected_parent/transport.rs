//! Linux descriptor custody for the protected parent protocol.
//!
//! This layer verifies the original kernel peer and sealed input object. It
//! transports bytes only; the parent module owns the closed codec, original
//! reservation, activation, native fence and joined-disposal state machine.
//! No process, provider, helper or native caller is constructed here.
#![cfg(target_os = "linux")]

use rustix::{
    fs::{self, FileType, OFlags, SealFlags},
    net::{
        self, AddressFamily, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags,
        SendFlags, SocketType, sockopt,
    },
};
use sha2::{Digest, Sha256};
use std::{
    io::{self, IoSliceMut},
    mem::{MaybeUninit, size_of},
    os::fd::{AsFd, BorrowedFd, OwnedFd},
};
use tokio::io::unix::AsyncFd;

pub(super) const MAX_PACKET: usize = 4096;
const MAX_MAPPING: usize = 65536;
const CREDENTIAL_SPACE: usize = rustix::cmsg_space!(ScmCredentials(1));
// rustix skips unknown cmsgs. With one complete credential present, even an
// empty extra header cannot fit without CTRUNC. Do not enlarge this buffer.
const _: () = assert!(
    CREDENTIAL_SPACE
        < rustix::cmsg_aligned_space!(ScmCredentials(1))
            + size_of::<usize>()
            + 2 * size_of::<i32>()
);

fn refused(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
pub(super) fn protect(fd: BorrowedFd<'_>, nonblocking: bool) -> io::Result<()> {
    let flags = rustix::io::fcntl_getfd(fd)?;
    rustix::io::fcntl_setfd(fd, flags | rustix::io::FdFlags::CLOEXEC)?;
    if nonblocking {
        let flags = fs::fcntl_getfl(fd)?;
        fs::fcntl_setfl(fd, flags | OFlags::NONBLOCK)?;
    }
    Ok(())
}

/// Kernel observation only. No constructor accepts forwarded packet fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct VerifiedParentPeer {
    pid: i32,
    uid: u32,
}
impl VerifiedParentPeer {
    pub(super) fn pid(&self) -> i32 {
        self.pid
    }
    pub(super) fn uid(&self) -> u32 {
        self.uid
    }
}

/// Constructed only after a complete packet passes kernel credential checks.
/// The codec must still reject malformed, foreign or out-of-order contents.
pub(super) struct VerifiedParentPacket {
    bytes: Vec<u8>,
    peer: VerifiedParentPeer,
}
impl VerifiedParentPacket {
    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(super) fn peer(&self) -> VerifiedParentPeer {
        self.peer
    }
}

/// One original socket owner. No Clone, descriptor export or split reader.
/// The caller retains/drives this owner under its original protocol deadline;
/// transport success never acknowledges a native fence or cleanup.
pub(super) struct ParentChannel {
    socket: AsyncFd<OwnedFd>,
    peer: VerifiedParentPeer,
    failed: bool,
}
impl ParentChannel {
    pub(super) fn adopt(fd: OwnedFd) -> io::Result<Self> {
        // Set flags on the exact descriptor before any async registration or
        // later helper creation. Every refusal drops this original OwnedFd.
        protect(fd.as_fd(), true)?;
        require_unprivileged_child()?;
        let peer = inspect_parent(fd.as_fd())?;
        sockopt::set_socket_passcred(&fd, true)?;
        if !sockopt::socket_passcred(&fd)? {
            return Err(refused("Parent credentials are unavailable"));
        }
        // The protocol sends HELLO only after this adoption. The original root
        // must not send an attestation before receiving that HELLO.
        Ok(Self {
            socket: crate::owned_fd::register(fd)?,
            peer,
            failed: false,
        })
    }

    /// Real same-process kernel transport for lifecycle tests only. It cannot
    /// admit a production parent: root authentication remains exercised only
    /// through adopt/inspect_parent and separate root/nonroot-child acceptance.
    #[cfg(test)]
    pub(super) fn unprivileged_test_pair() -> io::Result<(Self, Self)> {
        require_unprivileged_child()?;
        let (left, right) = net::socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            net::SocketFlags::CLOEXEC | net::SocketFlags::NONBLOCK,
            None,
        )?;
        let adopt_test = |fd: OwnedFd| -> io::Result<Self> {
            protect(fd.as_fd(), true)?;
            let observed = sockopt::socket_peercred(&fd)?;
            if observed.pid.as_raw_pid() <= 1
                || observed.pid.as_raw_pid() != nix::unistd::getpid().as_raw()
                || observed.uid.as_raw() != nix::unistd::getuid().as_raw()
            {
                return Err(refused(
                    "Lifecycle test peer is not this unprivileged process",
                ));
            }
            sockopt::set_socket_passcred(&fd, true)?;
            if !sockopt::socket_passcred(&fd)? {
                return Err(refused("Test credentials unavailable"));
            }
            Ok(Self {
                socket: crate::owned_fd::register(fd)?,
                peer: VerifiedParentPeer {
                    pid: observed.pid.as_raw_pid(),
                    uid: observed.uid.as_raw(),
                },
                failed: false,
            })
        };
        // Both passcred endpoints are configured before the pair can send.
        Ok((adopt_test(left)?, adopt_test(right)?))
    }

    pub(super) async fn receive(&mut self) -> io::Result<VerifiedParentPacket> {
        if self.failed {
            return Err(refused("Original parent channel is unavailable"));
        }
        let outcome = async {
            loop {
                let mut ready = self.socket.readable().await?;
                match ready.try_io(|socket| receive_one(socket.get_ref().as_fd(), self.peer)) {
                    Err(_) => (), // WouldBlock clears readiness, no packet consumed.
                    Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {
                        tokio::task::yield_now().await;
                    }
                    Ok(result) => return result,
                }
            }
        }
        .await;
        if outcome.is_err() {
            self.failed = true;
        }
        outcome
    }

    pub(super) async fn send(&mut self, packet: &[u8]) -> io::Result<()> {
        if self.failed {
            return Err(refused("Original parent channel is unavailable"));
        }
        let outcome = async {
            if packet.is_empty() || packet.len() > MAX_PACKET {
                return Err(refused("Parent packet size refused"));
            }
            loop {
                let mut ready = self.socket.writable().await?;
                match ready.try_io(|socket| {
                    net::send(
                        socket.get_ref(),
                        packet,
                        SendFlags::DONTWAIT | SendFlags::NOSIGNAL,
                    )
                    .map_err(io::Error::from)
                }) {
                    Err(_) => (),
                    Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {
                        tokio::task::yield_now().await;
                    }
                    Ok(Ok(count)) if count == packet.len() => return Ok(()),
                    Ok(Ok(_)) => return Err(refused("Parent packet write was incomplete")),
                    Ok(Err(error)) => return Err(error),
                }
            }
        }
        .await;
        if outcome.is_err() {
            self.failed = true;
        }
        outcome
    }
}

fn require_unprivileged_child() -> io::Result<()> {
    let ids = nix::unistd::getresuid().map_err(io::Error::from)?;
    if ids.real.is_root() || ids.effective != ids.real || ids.saved != ids.real {
        return Err(refused(
            "Protected native child must have one nonroot user identity",
        ));
    }
    Ok(())
}
pub(super) fn inspect_parent(fd: BorrowedFd<'_>) -> io::Result<VerifiedParentPeer> {
    if sockopt::socket_domain(fd)? != AddressFamily::UNIX
        || sockopt::socket_type(fd)? != SocketType::SEQPACKET
        || sockopt::socket_acceptconn(fd)?
        || !net::getpeername(fd)?.is_some_and(|peer| peer.address_family() == AddressFamily::UNIX)
    {
        return Err(refused(
            "Protected parent requires a connected Unix seqpacket endpoint",
        ));
    }
    let peer = sockopt::socket_peercred(fd)?;
    if peer.uid.as_raw() != 0 || peer.pid.as_raw_pid() <= 1 {
        return Err(refused("Protected parent root creator refused"));
    }
    Ok(VerifiedParentPeer {
        pid: peer.pid.as_raw_pid(),
        uid: peer.uid.as_raw(),
    })
}

fn receive_one(
    fd: BorrowedFd<'_>,
    expected: VerifiedParentPeer,
) -> io::Result<VerifiedParentPacket> {
    let mut bytes = [0; MAX_PACKET + 1];
    let mut space = [MaybeUninit::uninit(); CREDENTIAL_SPACE];
    let mut ancillary = RecvAncillaryBuffer::new(&mut space);
    let received = net::recvmsg(
        fd,
        &mut [IoSliceMut::new(&mut bytes)],
        &mut ancillary,
        RecvFlags::DONTWAIT | RecvFlags::CMSG_CLOEXEC,
    )?;
    let mut credential = None;
    let mut unexpected = false;
    // Drain before any payload interpretation, size/EOF/flag refusal or await.
    // RecvAncillaryBuffer and rights iterators also own disposal on early error.
    for message in ancillary.drain() {
        match message {
            RecvAncillaryMessage::ScmCredentials(peer) => {
                if credential.is_some() {
                    unexpected = true;
                }
                credential = Some(VerifiedParentPeer {
                    pid: peer.pid.as_raw_pid(),
                    uid: peer.uid.as_raw(),
                });
            }
            RecvAncillaryMessage::ScmRights(rights) => {
                unexpected = true;
                drop(rights);
            }
            _ => unexpected = true,
        }
    }
    if received
        .flags
        .intersects(ReturnFlags::TRUNC | ReturnFlags::CTRUNC)
        || !(received.flags & !(ReturnFlags::EOR | ReturnFlags::CMSG_CLOEXEC)).is_empty()
        || unexpected
    {
        return Err(refused("Parent ancillary data or truncation refused"));
    }
    if received.bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Original parent endpoint ended",
        ));
    }
    if received.bytes > MAX_PACKET {
        return Err(refused("Parent packet size refused"));
    }
    if credential != Some(expected) {
        return Err(refused("Original parent sender credentials changed"));
    }
    Ok(VerifiedParentPacket {
        bytes: bytes[..received.bytes].to_vec(),
        peer: expected,
    })
}

/// Exact root-selected sealed mapping, still uninterpreted by this layer.
/// Keep this owner with its channel/reservation; a digest is not a caller mint.
pub(super) struct SealedMapping {
    _fd: OwnedFd,
    bytes: Vec<u8>,
    sha256: [u8; 32],
}
impl SealedMapping {
    /// Validate before constructing a runtime or spawning any helper. The
    /// parent peer is independently inspected by the same input owner.
    pub(super) fn read_descriptors(
        fd: OwnedFd,
        parent: BorrowedFd<'_>,
        input: BorrowedFd<'_>,
        output: BorrowedFd<'_>,
    ) -> io::Result<Self> {
        protect(fd.as_fd(), false)?;
        require_distinct(&[fd.as_fd(), parent, input, output])?;
        let before = fs::fstat(&fd)?;
        require_mapping_metadata(&before, fs::fcntl_getfl(&fd)?, fs::fcntl_get_seals(&fd)?)?;
        let bytes = read_mapping_bytes(fd.as_fd(), before.st_size as i64)?;
        let after = fs::fstat(&fd)?;
        require_mapping_metadata(&after, fs::fcntl_getfl(&fd)?, fs::fcntl_get_seals(&fd)?)?;
        if (
            before.st_dev,
            before.st_ino,
            before.st_size,
            before.st_uid,
            before.st_mode,
        ) != (
            after.st_dev,
            after.st_ino,
            after.st_size,
            after.st_uid,
            after.st_mode,
        ) {
            return Err(refused("Sealed mapping metadata changed"));
        }
        let sha256 = Sha256::digest(&bytes).into();
        Ok(Self {
            _fd: fd,
            bytes,
            sha256,
        })
    }
    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(super) fn sha256(&self) -> [u8; 32] {
        self.sha256
    }
}
pub(super) fn read_original_sealed(fd: BorrowedFd<'_>, limit: usize) -> io::Result<Vec<u8>> {
    protect(fd, false)?;
    let before = fs::fstat(fd)?;
    let flags = fs::fcntl_getfl(fd)?;
    let seals = fs::fcntl_get_seals(fd)?;
    require_sealed_metadata(&before, flags, seals, limit)?;
    let bytes = read_bounded_bytes(fd, before.st_size as i64, limit)?;
    let after = fs::fstat(fd)?;
    let after_flags = fs::fcntl_getfl(fd)?;
    let after_seals = fs::fcntl_get_seals(fd)?;
    require_sealed_metadata(&after, after_flags, after_seals, limit)?;
    if flags != after_flags
        || seals != after_seals
        || (
            before.st_dev,
            before.st_ino,
            before.st_size,
            before.st_uid,
            before.st_mode,
        ) != (
            after.st_dev,
            after.st_ino,
            after.st_size,
            after.st_uid,
            after.st_mode,
        )
    {
        return Err(refused("Original sealed input changed"));
    }
    Ok(bytes)
}
fn require_mapping_metadata(info: &fs::Stat, flags: OFlags, seals: SealFlags) -> io::Result<()> {
    require_sealed_metadata(info, flags, seals, MAX_MAPPING)
}
fn require_sealed_metadata(
    info: &fs::Stat,
    flags: OFlags,
    seals: SealFlags,
    limit: usize,
) -> io::Result<()> {
    let required = SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL;
    if FileType::from_raw_mode(info.st_mode) != FileType::RegularFile
        || info.st_uid != 0
        || info.st_mode & 0o7777 != 0o400
        || flags.intersects(OFlags::PATH)
        || flags & OFlags::ACCMODE != OFlags::RDONLY
        || !seals.contains(required)
        || info.st_size <= 0
        || info.st_size > limit as _
    {
        return Err(refused("Immutable root mapping descriptor refused"));
    }
    Ok(())
}
pub(super) fn require_distinct(fds: &[BorrowedFd<'_>]) -> io::Result<()> {
    let mut identities = Vec::with_capacity(fds.len());
    for fd in fds {
        let info = fs::fstat(fd)?;
        let identity = (info.st_dev, info.st_ino);
        if identities.contains(&identity) {
            return Err(refused("Protected input descriptors alias"));
        }
        identities.push(identity);
    }
    Ok(())
}
fn read_mapping_bytes(fd: BorrowedFd<'_>, size: i64) -> io::Result<Vec<u8>> {
    read_bounded_bytes(fd, size, MAX_MAPPING)
}
fn read_bounded_bytes(fd: BorrowedFd<'_>, size: i64, limit: usize) -> io::Result<Vec<u8>> {
    if size <= 0 || size > limit as i64 {
        return Err(refused("Mapping size refused"));
    }
    let mut bytes = vec![0; size as usize + 1];
    let mut length = 0;
    let mut interruptions = 0;
    while length < bytes.len() {
        match rustix::io::pread(fd, &mut bytes[length..], length as u64) {
            Ok(0) => break,
            Ok(count) => length += count,
            Err(rustix::io::Errno::INTR) => {
                if interruptions == 8 {
                    return Err(refused("Mapping read interruption bound reached"));
                }
                interruptions += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
    if length != size as usize {
        return Err(refused("Sealed mapping size changed"));
    }
    bytes.truncate(length);
    Ok(bytes)
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
