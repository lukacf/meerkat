//! Actual descriptor custody only: no provider, Ready, login or parent receipt.
use super::Pipe;
use crate::{
    service_collection::{
        CollectionCursor, CollectionLimits, CollectionProgram, CollectionTarget,
        RequiredCollectionOperation, RequiredServiceCollection, ServiceCollectionSelection,
    },
    service_probe::tests::binding,
    stdio::run_with_service_collections_on_owned_pipes,
};
use nix::{
    errno::Errno,
    fcntl::{FcntlArg, FdFlag, OFlag, fcntl},
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::stat::{Mode, fstat},
    unistd::{mkfifo, pipe, read, write},
};
use std::{
    io::Read,
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::{fs::OpenOptionsExt, net::UnixStream},
    },
    path::Path,
    sync::Arc,
    time::Duration,
};

struct NeverProgram;
impl CollectionProgram for NeverProgram {
    fn accepts_target(&self, _: &CollectionTarget<'_>) -> bool {
        false
    }
    fn start(&self, _: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        panic!("owned pipe controls cannot start a collection program")
    }
}

fn selection(root: &Path) -> ServiceCollectionSelection {
    let operation = RequiredCollectionOperation::new(
        "read".into(),
        serde_json::json!({"repository":"owner/one"}),
        vec!["read".into()],
        Arc::new(NeverProgram),
    )
    .unwrap();
    let source = RequiredServiceCollection::new(
        "one".into(),
        "github".into(),
        "enterprise".into(),
        "agent".into(),
        vec![operation],
        CollectionLimits::new(1, 1024, 1024).unwrap(),
    )
    .unwrap();
    ServiceCollectionSelection::new(binding(root), vec![source]).unwrap()
}

fn nonblocking(fd: &OwnedFd) {
    let flags = OFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFL).unwrap());
    fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
}

// Retain the actual opposite endpoints through these observations. Never probe
// a consumed raw fd number with EBADF: another test could already have reused it.
fn reader_disposed(writer: &OwnedFd) -> bool {
    let mut descriptors = [PollFd::new(writer.as_fd(), PollFlags::POLLOUT)];
    poll(&mut descriptors, PollTimeout::ZERO).unwrap();
    let events = descriptors[0].revents().unwrap();
    // macOS reports its pipe peer EOF as POLLHUP; Linux reports POLLERR.
    // The retained writer must remain valid in either case.
    !events.contains(PollFlags::POLLNVAL)
        && events.intersects(PollFlags::POLLERR | PollFlags::POLLHUP)
}

fn writer_disposed(reader: &OwnedFd) -> bool {
    let mut bytes = [0; 4096];
    // These controls can emit only one bounded protocol error. Drain actual
    // bytes before observing EOF; buffered data is not a live writer.
    for _ in 0..16 {
        match read(reader, &mut bytes) {
            Ok(0) => return true,
            Ok(_) => (),
            Err(Errno::EAGAIN) => return false,
            Err(error) => panic!("owned output observation failed: {error}"),
        }
    }
    false
}

async fn disposed(input_peer: Option<&OwnedFd>, output_peer: Option<&OwnedFd>) -> bool {
    if let Some(reader) = output_peer {
        nonblocking(reader);
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if input_peer.is_none_or(reader_disposed) && output_peer.is_none_or(writer_disposed) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn owned_pair_adopts_original_descriptors_and_disposes_both_endpoints() {
    let (input, input_peer) = pipe().unwrap();
    let (output_peer, output) = pipe().unwrap();
    let original_input = input.as_raw_fd();
    let original_output = output.as_raw_fd();
    let input_identity = fstat(&input).unwrap();
    let output_identity = fstat(&output).unwrap();
    let (input, output) = Pipe::owned_pair(input, output).unwrap();
    // The owners remain live during raw-number and fstat comparisons.
    assert_eq!(input.0.get_ref().as_raw_fd(), original_input);
    assert_eq!(output.0.get_ref().as_raw_fd(), original_output);
    for (owner, original) in [(&input, input_identity), (&output, output_identity)] {
        let actual = fstat(owner.0.get_ref()).unwrap();
        assert_eq!(
            (actual.st_dev, actual.st_ino, actual.st_mode),
            (original.st_dev, original.st_ino, original.st_mode)
        );
        assert!(
            FdFlag::from_bits_truncate(fcntl(owner.0.get_ref(), FcntlArg::F_GETFD).unwrap())
                .contains(FdFlag::FD_CLOEXEC)
        );
        assert!(
            OFlag::from_bits_truncate(fcntl(owner.0.get_ref(), FcntlArg::F_GETFL).unwrap())
                .contains(OFlag::O_NONBLOCK)
        );
    }
    drop((input, output));
    assert!(
        disposed(Some(&input_peer), Some(&output_peer)).await,
        "both original endpoints must actually close"
    );
}

#[tokio::test]
async fn inherited_pipe_sets_cloexec_on_duplicate_and_keeps_caller_descriptor() {
    let (original, peer) = pipe().unwrap();
    fcntl(&original, FcntlArg::F_SETFD(FdFlag::empty())).unwrap();
    let inherited = Pipe::inherited(&original).unwrap();
    assert_ne!(inherited.0.get_ref().as_raw_fd(), original.as_raw_fd());
    assert_eq!(fcntl(&original, FcntlArg::F_GETFD).unwrap(), 0);
    assert!(
        FdFlag::from_bits_truncate(fcntl(inherited.0.get_ref(), FcntlArg::F_GETFD).unwrap())
            .contains(FdFlag::FD_CLOEXEC)
    );
    drop(inherited);
    assert!(
        !reader_disposed(&peer),
        "the caller still owns its original endpoint"
    );
    assert_eq!(write(&peer, b"x").unwrap(), 1);
    let mut byte = [0];
    assert_eq!(read(&original, &mut byte).unwrap(), 1);
    assert_eq!(byte, *b"x");
    drop(original);
    assert!(reader_disposed(&peer));
}

#[tokio::test]
async fn owned_entry_refuses_non_fifo_on_either_side_before_invocation() {
    let root = tempfile::tempdir().unwrap();
    for wrong_input in [true, false] {
        let selected = selection(root.path());
        let control = selected.control();
        let (socket, mut peer) = UnixStream::pair().unwrap();
        peer.set_nonblocking(true).unwrap();
        let (input, input_peer) = pipe().unwrap();
        let (output_peer, output) = pipe().unwrap();
        let result = if wrong_input {
            drop(input);
            run_with_service_collections_on_owned_pipes(selected, socket.into(), output, None).await
        } else {
            drop(output);
            run_with_service_collections_on_owned_pipes(selected, input, socket.into(), None).await
        };
        let pipes_closed = disposed(Some(&input_peer), Some(&output_peer)).await;
        let socket_closed = peer.read(&mut [0]).is_ok_and(|count| count == 0);
        assert!(result.is_err());
        assert!(
            pipes_closed && socket_closed,
            "refusal must dispose all adopted endpoints"
        );
        assert!(
            control.fence().unwrap().joined().await.is_err(),
            "invalid descriptors cannot register a completed invocation"
        );
    }
}

#[tokio::test]
async fn owned_entry_refuses_wrong_direction_and_read_write_fifo_before_invocation() {
    let root = tempfile::tempdir().unwrap();
    // Wrong input direction: both surviving peers are readers.
    let selected = selection(root.path());
    let control = selected.control();
    let (input_peer, input) = pipe().unwrap();
    let (output_peer, output) = pipe().unwrap();
    let result = run_with_service_collections_on_owned_pipes(selected, input, output, None).await;
    let input_closed = disposed(None, Some(&input_peer)).await;
    let output_closed = disposed(None, Some(&output_peer)).await;
    assert!(result.is_err() && input_closed && output_closed);
    assert!(control.fence().unwrap().joined().await.is_err());

    // Wrong output direction: both surviving peers are writers.
    let selected = selection(root.path());
    let control = selected.control();
    let (input, input_peer) = pipe().unwrap();
    let (output, output_peer) = pipe().unwrap();
    let result = run_with_service_collections_on_owned_pipes(selected, input, output, None).await;
    assert!(result.is_err() && reader_disposed(&input_peer) && reader_disposed(&output_peer));
    assert!(control.fence().unwrap().joined().await.is_err());

    // A real FIFO opened read/write must not silently satisfy the input contract.
    let fifo = root.path().join("read-write-fifo");
    mkfifo(&fifo, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
    let input = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(OFlag::O_NONBLOCK.bits())
        .open(&fifo)
        .unwrap();
    // Observe the named FIFO through a retained reader: EOF proves its last
    // writer closed. Write-side polling has different semantics for named
    // FIFOs and anonymous pipes on Darwin.
    let input_peer: OwnedFd = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(OFlag::O_NONBLOCK.bits())
        .open(&fifo)
        .unwrap()
        .into();
    assert!(
        !writer_disposed(&input_peer),
        "the live read/write owner prevents EOF"
    );
    let (output_peer, output) = pipe().unwrap();
    let selected = selection(root.path());
    let control = selected.control();
    let result =
        run_with_service_collections_on_owned_pipes(selected, input.into(), output, None).await;
    let input_closed = disposed(None, Some(&input_peer)).await;
    let output_closed = disposed(None, Some(&output_peer)).await;
    assert!(result.is_err() && input_closed && output_closed);
    assert!(control.fence().unwrap().joined().await.is_err());
}

#[tokio::test]
async fn invalid_owned_pipe_store_root_disposes_descriptors_without_entering() {
    let root = tempfile::tempdir().unwrap();
    let selected = selection(root.path());
    let control = selected.control();
    let (input, input_peer) = pipe().unwrap();
    let (output_peer, output) = pipe().unwrap();
    let result = run_with_service_collections_on_owned_pipes(
        selected,
        input,
        output,
        Some("relative-store".into()),
    )
    .await;
    let closed = disposed(Some(&input_peer), Some(&output_peer)).await;
    assert!(result.is_err() && closed);
    assert!(control.fence().unwrap().joined().await.is_err());
}

#[tokio::test]
async fn entered_owned_pipe_invocation_joins_real_eof_and_disposes_output() {
    let root = tempfile::tempdir().unwrap();
    let selected = selection(root.path());
    let control = selected.control();
    let other = selection(root.path()).control();
    let (input, input_peer) = pipe().unwrap();
    let (output_peer, output) = pipe().unwrap();
    let mut invocation = Box::pin(run_with_service_collections_on_owned_pipes(
        selected, input, output, None,
    ));
    assert!(futures_util::poll!(invocation.as_mut()).is_pending());
    let fence = control.fence().unwrap();
    let mut joined = Box::pin(fence.joined());
    assert!(futures_util::poll!(joined.as_mut()).is_pending());
    // A fence alone does not close an open input. The real caller sends EOF.
    drop(input_peer);
    let result = tokio::time::timeout(Duration::from_secs(2), invocation.as_mut()).await;
    drop(invocation);
    let output_closed = disposed(None, Some(&output_peer)).await;
    let closed = tokio::time::timeout(Duration::from_secs(2), joined).await;
    assert!(result.expect("owned invocation did not join").is_ok());
    assert!(
        output_closed,
        "no output descriptor may survive successful native close"
    );
    let closed = closed.unwrap().unwrap();
    assert!(closed.belongs_to(&control));
    assert!(!closed.belongs_to(&other));
}

#[tokio::test]
async fn actual_owned_pipe_output_failure_never_confirms_native_close() {
    let root = tempfile::tempdir().unwrap();
    let selected = selection(root.path());
    let control = selected.control();
    let (input, input_peer) = pipe().unwrap();
    let (output_peer, output) = pipe().unwrap();
    drop(output_peer);
    // This real parse-error response reaches the original output pipe after
    // its reader has closed. No synthetic writer or native result is supplied.
    assert_eq!(write(&input_peer, b"not-json\n").unwrap(), 9);
    let mut invocation = Box::pin(run_with_service_collections_on_owned_pipes(
        selected, input, output, None,
    ));
    let result = tokio::time::timeout(Duration::from_secs(2), invocation.as_mut()).await;
    drop(invocation);
    let input_closed = disposed(Some(&input_peer), None).await;
    assert!(
        result
            .expect("failed writer invocation did not join")
            .is_err()
    );
    assert!(input_closed);
    assert!(
        control.fence().unwrap().joined().await.is_err(),
        "actual writer failure cannot become a successful native close"
    );
}

#[tokio::test]
async fn dropped_owned_pipe_invocation_disposes_originals_without_close_receipt() {
    let root = tempfile::tempdir().unwrap();
    let selected = selection(root.path());
    let control = selected.control();
    let (input, input_peer) = pipe().unwrap();
    let (output_peer, output) = pipe().unwrap();
    let mut invocation = Box::pin(run_with_service_collections_on_owned_pipes(
        selected, input, output, None,
    ));
    assert!(futures_util::poll!(invocation.as_mut()).is_pending());
    let fence = control.fence().unwrap();
    let mut joined = Box::pin(fence.joined());
    assert!(futures_util::poll!(joined.as_mut()).is_pending());
    drop(invocation);
    // The writer receives channel closure from the dropped invocation. Observe
    // its actual disposal, but do not turn that late drop into a joined owner.
    let closed = disposed(Some(&input_peer), Some(&output_peer)).await;
    let outcome = tokio::time::timeout(Duration::from_secs(2), joined).await;
    assert!(
        closed,
        "both original descriptors must be disposed after cancellation"
    );
    assert!(outcome.unwrap().is_err());
}
