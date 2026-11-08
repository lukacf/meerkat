//! Private close-mode source controls over actual OS pipes and writer custody.
//! No provider is initialized, Ready is never constructed, and no entered native
//! work/result is modeled. Parent authentication/activation remain unimplemented.
use super::*;
use crate::service_collection::{
    CollectionCursor, CollectionLimits, CollectionProgram, CollectionTarget,
    RequiredCollectionOperation, RequiredServiceCollection, ServiceCollectionSelection,
};
use crate::service_probe::{ServiceSelection, tests::binding};
use nix::{
    errno::Errno,
    fcntl::{FcntlArg, OFlag, fcntl},
    poll::{PollFd, PollFlags, PollTimeout, poll},
    unistd::{pipe, read, write},
};
use std::{
    future::Future,
    io,
    os::fd::{AsFd, OwnedFd},
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
};
use tokio::io::ReadBuf;

struct NeverProgram;
impl CollectionProgram for NeverProgram {
    fn accepts_target(&self, _: &CollectionTarget<'_>) -> bool {
        false
    }
    fn start(&self, _: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        panic!("protected close controls cannot start a collection program")
    }
}
fn selection(root: &std::path::Path, deadline: Option<u64>) -> Arc<ServiceCollectionSelection> {
    let operation = RequiredCollectionOperation::new(
        "read".into(),
        json!({"repository":"owner/one"}),
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
    let mut selected = binding(root);
    if let Some(deadline) = deadline {
        selected.expires_at_ms = deadline;
    }
    Arc::new(ServiceCollectionSelection::new(selected, vec![source]).unwrap())
}

// Count polls of the actual adopted pipe, not a synthetic EOF source.
struct CountedInput {
    pipe: transport::Pipe,
    reads: Arc<AtomicUsize>,
}
impl AsyncRead for CountedInput {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.reads.fetch_add(1, Ordering::SeqCst);
        Pin::new(&mut this.pipe).poll_read(cx, buf)
    }
}
fn pipes() -> (
    CountedInput,
    transport::Pipe,
    OwnedFd,
    OwnedFd,
    Arc<AtomicUsize>,
) {
    let (input, input_peer) = pipe().unwrap();
    let (output_peer, output) = pipe().unwrap();
    let (input, output) = transport::Pipe::owned_pair(input, output).unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    (
        CountedInput {
            pipe: input,
            reads: reads.clone(),
        },
        output,
        input_peer,
        output_peer,
        reads,
    )
}
fn reader_disposed(peer: &OwnedFd) -> bool {
    let mut descriptors = [PollFd::new(peer.as_fd(), PollFlags::POLLOUT)];
    poll(&mut descriptors, PollTimeout::ZERO).unwrap();
    let events = descriptors[0].revents().unwrap();
    !events.contains(PollFlags::POLLNVAL)
        && events.intersects(PollFlags::POLLERR | PollFlags::POLLHUP)
}
async fn output_disposed(peer: &OwnedFd) -> bool {
    let flags = OFlag::from_bits_truncate(fcntl(peer, FcntlArg::F_GETFL).unwrap());
    fcntl(peer, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut bytes = [0; 4096];
        loop {
            match read(peer, &mut bytes) {
                Ok(0) => break true,
                Ok(_) | Err(Errno::EAGAIN) => tokio::task::yield_now().await,
                Err(_) => break false,
            }
        }
    })
    .await
    .unwrap_or(false)
}

#[tokio::test]
async fn protected_fence_closes_exact_owned_pipes_without_reading_queued_frames() {
    let root = tempfile::tempdir().unwrap();
    let selected = selection(root.path(), None);
    let control = selected.control();
    let other = selection(root.path(), None).control();
    let (input, output, input_peer, output_peer, reads) = pipes();
    let mut invocation = Box::pin(run_io_composed_with_host_close(
        input,
        output,
        None,
        Some(ServiceSelection::Collections(selected)),
        true,
    ));
    let entered = futures_util::poll!(invocation.as_mut()).is_pending();
    if !entered {
        drop(invocation);
        let disposed = output_disposed(&output_peer).await;
        panic!("protected invocation did not enter; output disposed: {disposed}");
    }
    let foreign_fence = other.fence();
    let foreign_still_pending = futures_util::poll!(invocation.as_mut()).is_pending();
    if !foreign_still_pending {
        drop(invocation);
        let disposed = output_disposed(&output_peer).await;
        panic!("a foreign fence ended this invocation; output disposed: {disposed}");
    }
    // Queue an actual bounded frame while the caller still owns stdin. A close
    // must stop its frame source before it could process this later request.
    let queued = write(
        &input_peer,
        b"{\"jsonrpc\":\"2.0\",\"id\":\"c:late\",\"method\":\"later\"}\n",
    );
    let before = reads.load(Ordering::SeqCst);
    let fence = control.fence();
    if fence.is_err() {
        control.retire();
    }
    let result = tokio::time::timeout(Duration::from_secs(2), invocation.as_mut()).await;
    drop(invocation);
    let output_closed = output_disposed(&output_peer).await;
    let input_closed = reader_disposed(&input_peer);
    // Endpoint and writer disposal precede assertions, also on timeout/error.
    assert!(foreign_fence.is_ok() && foreign_still_pending);
    assert!(queued.is_ok());
    assert!(before > 0);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        before,
        "no frame read after the native fence"
    );
    assert!(
        input_closed && output_closed,
        "stdin remained open in the caller"
    );
    assert!(result.expect("protected close did not join").is_ok());
    let fence = fence.unwrap();
    assert!(fence.belongs_to(&control) && !fence.belongs_to(&other));
    let closed = tokio::time::timeout(Duration::from_secs(2), fence.joined())
        .await
        .unwrap()
        .unwrap();
    assert!(closed.belongs_to(&control) && !closed.belongs_to(&other));
}

#[tokio::test]
async fn protected_original_deadline_closes_with_stdin_still_open() {
    let root = tempfile::tempdir().unwrap();
    let deadline = now() + 200;
    let selected = selection(root.path(), Some(deadline));
    let control = selected.control();
    let (input, output, input_peer, output_peer, _) = pipes();
    let mut invocation = Box::pin(run_io_composed_with_host_close(
        input,
        output,
        None,
        Some(ServiceSelection::Collections(selected)),
        true,
    ));
    let result = tokio::time::timeout(Duration::from_secs(2), invocation.as_mut()).await;
    drop(invocation);
    let output_closed = output_disposed(&output_peer).await;
    assert!(now() >= deadline);
    assert!(reader_disposed(&input_peer) && output_closed);
    assert!(
        result
            .expect("original deadline did not close the invocation")
            .is_ok()
    );
    assert!(control.fence().unwrap().joined().await.is_ok());
}

#[tokio::test]
async fn protected_close_observes_retirement_after_prior_cancel_without_synthetic_eof() {
    // Driver state-machine control only: no entered task or completion is
    // invented to stand in for the future native-work integration fixture.
    let root = tempfile::tempdir().unwrap();
    let selected = selection(root.path(), None);
    let control = selected.control();
    let (output, _output_rx) = mpsc::channel(OUTPUT_FRAMES);
    let (mut driver, _notices, _prompts) =
        new_driver(output, Arc::new(Semaphore::new(MAX_FRAME * 2)));
    driver.collection_selection = Some(selected);
    driver.fence("cancelled");
    assert!(*driver.plain_cancel.borrow() && driver.retired && !driver.closing);
    assert!(
        !driver.watches_host_retirement(false),
        "public retire-only behavior is preserved"
    );
    assert!(driver.watches_host_retirement(true));
    let fence = control.fence().unwrap();
    driver.host_retired(true);
    assert!(driver.closing && driver.retired);
    assert!(
        !driver.eof && !driver.closed,
        "retirement is neither input EOF nor completed close"
    );
    assert!(!driver.watches_host_retirement(true));
    assert!(
        fence.joined().await.is_err(),
        "this component control did not enter an invocation"
    );
}

#[tokio::test]
async fn public_owned_pipe_entry_keeps_its_existing_retire_only_behavior() {
    let root = tempfile::tempdir().unwrap();
    let selected = Arc::try_unwrap(selection(root.path(), None)).ok().unwrap();
    let control = selected.control();
    let (input, input_peer) = pipe().unwrap();
    let (output_peer, output) = pipe().unwrap();
    let mut invocation = Box::pin(run_with_service_collections_on_owned_pipes(
        selected, input, output, None,
    ));
    let entered = futures_util::poll!(invocation.as_mut()).is_pending();
    if !entered {
        drop(invocation);
        let disposed = output_disposed(&output_peer).await;
        panic!("public invocation did not enter; output disposed: {disposed}");
    }
    let fence = control.fence();
    let still_pending = futures_util::poll!(invocation.as_mut()).is_pending();
    if !still_pending {
        drop(invocation);
        let disposed = output_disposed(&output_peer).await;
        panic!("public fence unexpectedly ended the invocation; output disposed: {disposed}");
    }
    let input_still_owned = !reader_disposed(&input_peer);
    drop(input_peer); // The unchanged public path needs real EOF/close.
    let result = tokio::time::timeout(Duration::from_secs(2), invocation.as_mut()).await;
    drop(invocation);
    let output_closed = output_disposed(&output_peer).await;
    assert!(still_pending && input_still_owned && output_closed);
    assert!(
        result
            .expect("public invocation did not join real EOF")
            .is_ok()
    );
    assert!(fence.unwrap().joined().await.is_ok());
}

#[tokio::test]
async fn protected_fence_before_entry_refuses_without_reading_or_claiming_close() {
    let root = tempfile::tempdir().unwrap();
    let selected = selection(root.path(), None);
    let control = selected.control();
    let fence = control.fence().unwrap();
    let (input, output, input_peer, output_peer, reads) = pipes();
    let result = run_io_composed_with_host_close(
        input,
        output,
        None,
        Some(ServiceSelection::Collections(selected)),
        true,
    )
    .await;
    let output_closed = output_disposed(&output_peer).await;
    assert!(result.is_err() && reader_disposed(&input_peer) && output_closed);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(
        fence.joined().await.is_err(),
        "before activation there is no entered invocation to join"
    );
}

#[tokio::test]
async fn protected_mode_refuses_without_original_selected_lifetime() {
    let (input, output, input_peer, output_peer, reads) = pipes();
    let result = run_io_composed_with_host_close(input, output, None, None, true).await;
    let output_closed = output_disposed(&output_peer).await;
    assert!(result.is_err() && reader_disposed(&input_peer) && output_closed);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
}

// Hold the actual output owner at flush. Writes still go to the adopted OS pipe.
// A dropped invocation may detach its writer, so every test failure branch must
// release this hold and await the writer's actual disposal before panicking.
struct HeldFlush {
    pipe: transport::Pipe,
    entered: Option<oneshot::Sender<()>>,
    release: oneshot::Receiver<bool>,
    dropped: Option<oneshot::Sender<()>>,
}
impl AsyncWrite for HeldFlush {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().pipe).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(entered) = this.entered.take() {
            let _ = entered.send(());
        }
        match Pin::new(&mut this.release).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(true)) => Pin::new(&mut this.pipe).poll_flush(cx),
            Poll::Ready(Ok(false) | Err(_)) => {
                Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
            }
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}
impl Drop for HeldFlush {
    fn drop(&mut self) {
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(());
        }
    }
}
async fn held_writer(success: bool, lose_invocation: bool) {
    let root = tempfile::tempdir().unwrap();
    let selected = selection(root.path(), None);
    let control = selected.control();
    let (input, output, input_peer, output_peer, _) = pipes();
    let (entered, observed) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (dropped, disposed) = oneshot::channel();
    let output = HeldFlush {
        pipe: output,
        entered: Some(entered),
        release: released,
        dropped: Some(dropped),
    };
    let mut invocation = Box::pin(run_io_composed_with_host_close(
        input,
        output,
        None,
        Some(ServiceSelection::Collections(selected)),
        true,
    ));
    let entered = futures_util::poll!(invocation.as_mut()).is_pending();
    if !entered {
        drop(invocation);
        let _ = release.send(success);
        let disposed = tokio::time::timeout(Duration::from_secs(2), disposed).await;
        let output_closed = output_disposed(&output_peer).await;
        panic!(
            "protected invocation did not enter; output disposed: {}, pipe EOF: {output_closed}",
            matches!(disposed, Ok(Ok(())))
        );
    }
    let fence = control.fence();
    if fence.is_err() {
        control.retire();
    }
    let mut task = tokio::spawn(invocation);
    let reached_flush = tokio::time::timeout(Duration::from_secs(2), observed).await;
    if !matches!(reached_flush, Ok(Ok(()))) {
        let _ = release.send(success);
        task.abort();
        let _ = task.await;
        let disposed = tokio::time::timeout(Duration::from_secs(2), disposed).await;
        let output_closed = output_disposed(&output_peer).await;
        panic!(
            "actual writer did not reach flush; disposed: {}, pipe EOF: {output_closed}",
            matches!(disposed, Ok(Ok(())))
        );
    }
    let mut joined = fence.as_ref().ok().map(|fence| Box::pin(fence.joined()));
    let pending = match joined.as_mut() {
        Some(joined) => futures_util::poll!(joined.as_mut()).is_pending(),
        None => false,
    };
    let reader_closed_before_writer = reader_disposed(&input_peer);
    let aborted = if lose_invocation {
        task.abort();
        Some((&mut task).await)
    } else {
        None
    };
    let released = release.send(success).is_ok();
    let result = if lose_invocation {
        None
    } else {
        Some(tokio::time::timeout(Duration::from_secs(2), &mut task).await)
    };
    if result.as_ref().is_some_and(Result::is_err) {
        task.abort();
        let _ = task.await;
    }
    let disposed = tokio::time::timeout(Duration::from_secs(2), disposed).await;
    let output_closed = output_disposed(&output_peer).await;
    assert!(
        pending && reader_closed_before_writer,
        "reader disposal precedes the retained writer join"
    );
    assert!(released && matches!(disposed, Ok(Ok(()))) && output_closed);
    let closed = tokio::time::timeout(Duration::from_secs(2), joined.unwrap())
        .await
        .unwrap();
    if lose_invocation {
        assert!(aborted.unwrap().unwrap_err().is_cancelled());
        assert!(
            closed.is_err(),
            "late writer disposal cannot repair a lost invocation owner"
        );
    } else {
        let result = result
            .unwrap()
            .expect("actual invocation did not join")
            .unwrap();
        if success {
            assert!(result.is_ok());
            assert!(closed.unwrap().belongs_to(&control));
        } else {
            assert!(
                result.is_err() && closed.is_err(),
                "actual output failure stays unconfirmed"
            );
        }
    }
}

#[tokio::test]
async fn protected_close_waits_for_actual_held_writer_and_endpoint_disposal() {
    held_writer(true, false).await;
}
#[tokio::test]
async fn protected_output_failure_cannot_become_joined_close() {
    held_writer(false, false).await;
}
#[tokio::test]
async fn protected_invocation_loss_stays_unconfirmed_after_late_writer_disposal() {
    held_writer(true, true).await;
}
