//! Linux kernel-mechanics controls, not root-parent/native-host acceptance.
//! Tests of receive_one pass actual local credentials to that private parser;
//! they do not call the production root-peer constructor with a bypass.
use super::*;
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SocketFlags};
use std::{io::IoSlice, os::fd::AsRawFd};

fn pair() -> (OwnedFd, OwnedFd) {
    net::socketpair(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )
    .unwrap()
}
fn local_peer(fd: BorrowedFd<'_>) -> VerifiedParentPeer {
    let actual = sockopt::socket_peercred(fd).unwrap();
    VerifiedParentPeer {
        pid: actual.pid.as_raw_pid(),
        uid: actual.uid.as_raw(),
    }
}
fn receive_setup() -> (OwnedFd, OwnedFd, VerifiedParentPeer) {
    let (receive, send) = pair();
    sockopt::set_socket_passcred(&receive, true).unwrap();
    let expected = local_peer(receive.as_fd());
    (receive, send, expected)
}
fn send_packet(fd: &OwnedFd, bytes: &[u8]) {
    assert_eq!(
        net::send(fd, bytes, SendFlags::DONTWAIT | SendFlags::NOSIGNAL).unwrap(),
        bytes.len()
    );
}

#[tokio::test]
async fn production_parent_admission_rejects_an_ordinary_same_process_socketpair() {
    let (receive, send) = pair();
    // Under an ordinary UID the peer is not root. Under root this is not an
    // unprivileged native child. Neither case is a protected parent admission.
    let result = ParentChannel::adopt(receive);
    assert!(result.is_err());
    let mut bytes = [0; 1];
    assert_eq!(
        net::recv(&send, &mut bytes[..], RecvFlags::DONTWAIT).unwrap(),
        (0, 0)
    );
}

#[test]
fn parent_endpoint_shape_refuses_wrong_socket_kind_listener_and_unconnected() {
    for kind in [SocketType::STREAM, SocketType::DGRAM] {
        let (receive, _send) = net::socketpair(
            AddressFamily::UNIX,
            kind,
            SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
            None,
        )
        .unwrap();
        assert!(inspect_parent(receive.as_fd()).is_err());
    }
    let unconnected = net::socket(AddressFamily::UNIX, SocketType::SEQPACKET, None).unwrap();
    assert!(inspect_parent(unconnected.as_fd()).is_err());
    let listener = net::socket(AddressFamily::UNIX, SocketType::SEQPACKET, None).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("cp-")
        .tempdir_in("/tmp")
        .unwrap();
    let address = net::SocketAddrUnix::new(directory.path().join("p")).unwrap();
    net::bind(&listener, &address).unwrap();
    net::listen(&listener, 1).unwrap();
    assert!(inspect_parent(listener.as_fd()).is_err());
    let inet = net::socket(AddressFamily::INET, SocketType::STREAM, None).unwrap();
    assert!(inspect_parent(inet.as_fd()).is_err());
    let file = tempfile::tempfile().unwrap();
    assert!(inspect_parent(file.as_fd()).is_err());
}

#[test]
fn original_descriptor_flags_are_protected_without_replacing_its_identity() {
    let (fd, _peer) = pair();
    rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::empty()).unwrap();
    fs::fcntl_setfl(&fd, fs::fcntl_getfl(&fd).unwrap() & !OFlags::NONBLOCK).unwrap();
    let number = fd.as_raw_fd();
    let before = fs::fstat(&fd).unwrap();
    protect(fd.as_fd(), true).unwrap();
    let after = fs::fstat(&fd).unwrap();
    assert_eq!(number, fd.as_raw_fd());
    assert_eq!((before.st_dev, before.st_ino), (after.st_dev, after.st_ino));
    assert!(
        rustix::io::fcntl_getfd(&fd)
            .unwrap()
            .contains(rustix::io::FdFlags::CLOEXEC)
    );
    assert!(fs::fcntl_getfl(&fd).unwrap().contains(OFlags::NONBLOCK));
}

#[test]
fn actual_seqpacket_credentials_and_whole_packet_boundaries_are_retained() {
    let (receive, send, expected) = receive_setup();
    for body in [
        b"first".as_slice(),
        b"second".as_slice(),
        &[b'x'; MAX_PACKET][..],
    ] {
        send_packet(&send, body);
        let packet = receive_one(receive.as_fd(), expected).unwrap();
        assert_eq!(packet.bytes(), body);
        assert_eq!(packet.peer().pid(), expected.pid());
        assert_eq!(packet.peer().uid(), expected.uid());
    }
    assert_eq!(
        receive_one(receive.as_fd(), expected).err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn actual_packet_refuses_missing_or_foreign_credentials_and_oversize_or_eof() {
    let (receive, send, expected) = receive_setup();
    send_packet(&send, b"pid");
    let wrong_pid = VerifiedParentPeer {
        pid: expected.pid.checked_add(1).unwrap(),
        ..expected
    };
    assert!(receive_one(receive.as_fd(), wrong_pid).is_err());
    send_packet(&send, b"uid");
    let wrong_uid = VerifiedParentPeer {
        uid: if expected.uid == 0 { 1 } else { 0 },
        ..expected
    };
    assert!(receive_one(receive.as_fd(), wrong_uid).is_err());
    send_packet(&send, &[b'x'; MAX_PACKET + 1]);
    assert!(receive_one(receive.as_fd(), expected).is_err());
    send_packet(&send, &[b'x'; MAX_PACKET + 100]);
    assert!(receive_one(receive.as_fd(), expected).is_err());
    send_packet(&send, b"");
    assert!(receive_one(receive.as_fd(), expected).is_err());
    sockopt::set_socket_passcred(&receive, false).unwrap();
    send_packet(&send, b"no credential");
    assert!(receive_one(receive.as_fd(), expected).is_err());
    drop(send);
    assert_eq!(
        receive_one(receive.as_fd(), expected).err().unwrap().kind(),
        io::ErrorKind::UnexpectedEof
    );
}

#[test]
fn delivered_rights_are_closed_on_both_explicit_and_truncated_ancillary_refusal() {
    for with_credentials in [false, true] {
        let (receive, send) = pair();
        sockopt::set_socket_passcred(&receive, with_credentials).unwrap();
        let expected = local_peer(receive.as_fd());
        let (reader, writer) = nix::unistd::pipe().unwrap();
        let rights = [writer.as_fd()];
        let mut storage = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = SendAncillaryBuffer::new(&mut storage);
        assert!(ancillary.push(SendAncillaryMessage::ScmRights(&rights)));
        assert_eq!(
            net::sendmsg(
                &send,
                &[IoSlice::new(b"rights")],
                &mut ancillary,
                SendFlags::DONTWAIT | SendFlags::NOSIGNAL
            )
            .unwrap(),
            6
        );
        drop(writer);
        let result = receive_one(receive.as_fd(), expected);
        // Observe the actual pipe peer, never a reusable raw FD number or a
        // process-wide count. Any leaked delivered writer would prevent EOF.
        protect(reader.as_fd(), true).unwrap();
        let eof = rustix::io::read(&reader, &mut [0u8; 1][..]);
        assert!(result.is_err());
        assert_eq!(
            eof.unwrap(),
            0,
            "received rights must be disposed on refusal"
        );
    }
}

fn sealed_local(bytes: &[u8], seals: SealFlags) -> OwnedFd {
    let writable = fs::memfd_create(
        "connect-mapping-source-control",
        fs::MemfdFlags::CLOEXEC | fs::MemfdFlags::ALLOW_SEALING,
    )
    .unwrap();
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let count = rustix::io::write(&writable, remaining).unwrap();
        assert!(count > 0);
        remaining = &remaining[count..];
    }
    fs::fchmod(&writable, fs::Mode::RUSR).unwrap();
    fs::fcntl_add_seals(&writable, seals).unwrap();
    // This test reproduces a protected parent's safe read-only reopen. Production never
    // opens a path from a supplied integer; it receives the original OwnedFd.
    let readonly: OwnedFd = std::fs::File::open(format!("/proc/self/fd/{}", writable.as_raw_fd()))
        .unwrap()
        .into();
    drop(writable);
    readonly
}
fn complete_seals() -> SealFlags {
    SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL
}

#[test]
fn actual_sealed_mapping_positioned_read_is_bounded_and_ignores_shared_offset() {
    let body = b"{\"version\":1}";
    let fd = sealed_local(body, complete_seals());
    fs::seek(&fd, fs::SeekFrom::Start(4)).unwrap();
    let bytes = read_mapping_bytes(fd.as_fd(), body.len() as i64).unwrap();
    assert_eq!(bytes, body);
    assert_eq!(fs::seek(&fd, fs::SeekFrom::Current(0)).unwrap(), 4);
    assert!(read_mapping_bytes(fd.as_fd(), 0).is_err());
    assert!(read_mapping_bytes(fd.as_fd(), MAX_MAPPING as i64 + 1).is_err());
    assert!(read_mapping_bytes(fd.as_fd(), body.len() as i64 - 1).is_err());
    assert!(read_mapping_bytes(fd.as_fd(), body.len() as i64 + 1).is_err());
    let info = fs::fstat(&fd).unwrap();
    let checked = require_mapping_metadata(
        &info,
        fs::fcntl_getfl(&fd).unwrap(),
        fs::fcntl_get_seals(&fd).unwrap(),
    );
    assert_eq!(
        checked.is_ok(),
        info.st_uid == 0,
        "nonroot sealed bytes do not become an authenticated root mapping"
    );
}

#[test]
fn mapping_refuses_mutable_readonly_files_missing_write_seal_and_descriptor_aliases() {
    let ordinary = tempfile::NamedTempFile::new().unwrap();
    fs::fchmod(ordinary.as_file(), fs::Mode::RUSR).unwrap();
    let readonly: OwnedFd = std::fs::File::open(ordinary.path()).unwrap().into();
    assert!(
        !fs::fcntl_get_seals(&readonly).is_ok_and(|seals| seals.contains(complete_seals())),
        "readonly permission alone cannot supply complete immutable seals"
    );
    let incomplete = sealed_local(
        b"body",
        SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    );
    assert!(
        !fs::fcntl_get_seals(&incomplete)
            .unwrap()
            .contains(SealFlags::WRITE)
    );
    let info = fs::fstat(&incomplete).unwrap();
    assert!(
        require_mapping_metadata(
            &info,
            fs::fcntl_getfl(&incomplete).unwrap(),
            fs::fcntl_get_seals(&incomplete).unwrap()
        )
        .is_err()
    );
    let exact = sealed_local(b"sealed", complete_seals());
    let alias = rustix::io::dup(&exact).unwrap();
    assert!(require_distinct(&[exact.as_fd(), alias.as_fd()]).is_err());
    let other = sealed_local(b"sealed", complete_seals());
    assert!(
        require_distinct(&[exact.as_fd(), other.as_fd()]).is_ok(),
        "equal bytes do not mean the same descriptor object"
    );
}

#[tokio::test]
async fn async_channel_mechanics_retain_packet_ownership_and_latch_failure() {
    // This is a private async-I/O control with the actual current-user peer.
    // It intentionally does not qualify or bypass ParentChannel::adopt.
    let (receive, send, expected) = receive_setup();
    let mut channel = ParentChannel {
        socket: crate::owned_fd::register(receive).unwrap(),
        peer: expected,
        failed: false,
    };
    {
        let pending = channel.receive();
        tokio::pin!(pending);
        assert!(futures_util::poll!(pending.as_mut()).is_pending());
    }
    send_packet(&send, b"whole packet after cancelled observation");
    let packet = tokio::time::timeout(std::time::Duration::from_secs(2), channel.receive())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(packet.bytes(), b"whole packet after cancelled observation");
    tokio::time::timeout(std::time::Duration::from_secs(2), channel.send(b"reply"))
        .await
        .unwrap()
        .unwrap();
    let mut body = [0; MAX_PACKET];
    let (copied, actual) = net::recv(&send, &mut body[..], RecvFlags::DONTWAIT).unwrap();
    assert_eq!((copied, actual), (5, 5));
    assert_eq!(&body[..copied], b"reply");
    send_packet(&send, &[b'x'; MAX_PACKET + 1]);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), channel.receive())
            .await
            .unwrap()
            .is_err()
    );
    assert!(channel.failed);
    assert!(channel.send(b"not replayed").await.is_err());
    assert_eq!(
        net::recv(&send, &mut body[..], RecvFlags::DONTWAIT)
            .err()
            .unwrap(),
        rustix::io::Errno::AGAIN
    );
    send_packet(&send, b"left in original channel");
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), channel.receive())
            .await
            .unwrap()
            .is_err()
    );
    let preserved = receive_one(channel.socket.get_ref().as_fd(), expected).unwrap();
    assert_eq!(
        preserved.bytes(),
        b"left in original channel",
        "failed owner cannot resume admission"
    );
}

#[test]
fn selected_input_seals_read_bound_offset_and_fifth_descriptor_alias_controls() {
    let body = b"selected input kernel control";
    let fd = sealed_local(body, complete_seals());
    fs::seek(&fd, fs::SeekFrom::Start(3)).unwrap();
    // Exercise positioned I/O and its limits at every uid. Root ownership is
    // a separate admission check, not a substitute for actually reading bytes.
    assert_eq!(
        read_bounded_bytes(fd.as_fd(), body.len() as i64, body.len()).unwrap(),
        body
    );
    assert_eq!(fs::seek(&fd, fs::SeekFrom::Current(0)).unwrap(), 3);
    assert!(read_bounded_bytes(fd.as_fd(), body.len() as i64, body.len() - 1).is_err());
    assert!(read_bounded_bytes(fd.as_fd(), body.len() as i64 + 1, body.len() + 1).is_err());
    assert!(read_bounded_bytes(fd.as_fd(), body.len() as i64 - 1, body.len()).is_err());
    assert_eq!(
        read_original_sealed(fd.as_fd(), body.len()).is_ok(),
        fs::fstat(&fd).unwrap().st_uid == 0
    );
    let others: Vec<_> = (0..4)
        .map(|_| sealed_local(body, complete_seals()))
        .collect();
    let mut distinct: Vec<_> = others.iter().map(AsFd::as_fd).collect();
    distinct.push(fd.as_fd());
    assert!(require_distinct(&distinct).is_ok());
    for original in &others {
        let alias = rustix::io::dup(original).unwrap();
        let mut descriptors: Vec<_> = others.iter().map(AsFd::as_fd).collect();
        descriptors.push(alias.as_fd());
        assert!(require_distinct(&descriptors).is_err());
    }
}

#[test]
#[ignore = "acceptance lane must invoke explicitly as uid 0 and retain the uid output"]
fn selected_input_root_sealed_read_and_incomplete_seal_refusal() {
    let uid = nix::unistd::geteuid().as_raw();
    eprintln!("selected_input_root_sealed_read effective_uid={uid}");
    assert_eq!(
        uid, 0,
        "this acceptance control must not pass vacuously as nonroot"
    );
    let body = b"selected input root kernel control";
    let fd = sealed_local(body, complete_seals());
    assert_eq!(fs::fstat(&fd).unwrap().st_uid, 0);
    fs::seek(&fd, fs::SeekFrom::Start(3)).unwrap();
    assert_eq!(read_original_sealed(fd.as_fd(), body.len()).unwrap(), body);
    assert_eq!(fs::seek(&fd, fs::SeekFrom::Current(0)).unwrap(), 3);
    assert!(read_original_sealed(fd.as_fd(), body.len() - 1).is_err());
    let incomplete = sealed_local(body, SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL);
    assert_eq!(fs::fstat(&incomplete).unwrap().st_uid, 0);
    assert!(read_original_sealed(incomplete.as_fd(), body.len()).is_err());
}
