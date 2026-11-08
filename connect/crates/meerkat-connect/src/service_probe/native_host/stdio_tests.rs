//! Actual stdio owner/pipe controls only. They do not initialize a provider,
//! construct Ready, authenticate a parent or fabricate any native tool result.
use super::*;
use crate::service_collection::{
    CollectionCursor, CollectionLimits, CollectionProgram, CollectionTarget,
    RequiredCollectionOperation, RequiredServiceCollection, ServiceCollectionSelection,
};
use crate::service_probe::{ServiceSelection, tests::binding};
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};

struct NeverProgram;
impl CollectionProgram for NeverProgram {
    fn accepts_target(&self, _: &CollectionTarget<'_>) -> bool {
        false
    }
    fn start(&self, _: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        panic!("pipe custody control cannot start a collection program")
    }
}

fn selection(root: &std::path::Path) -> Arc<ServiceCollectionSelection> {
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
    Arc::new(ServiceCollectionSelection::new(binding(root), vec![source]).unwrap())
}

struct HeldFlush {
    entered: Option<oneshot::Sender<()>>,
    release: oneshot::Receiver<bool>,
    dropped: Option<oneshot::Sender<()>>,
}
impl AsyncWrite for HeldFlush {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(entered) = this.entered.take() {
            let _ = entered.send(());
        }
        match Pin::new(&mut this.release).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(true)) => Poll::Ready(Ok(())),
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

async fn actual_pipe_flush(success: bool) {
    let root = tempfile::tempdir().unwrap();
    let selected = selection(root.path());
    let control = selected.control();
    let other = selection(root.path()).control();
    let (entered, observed) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (dropped, disposed) = oneshot::channel();
    let output = HeldFlush {
        entered: Some(entered),
        release: released,
        dropped: Some(dropped),
    };
    let mut task = tokio::spawn(run_io_composed(
        tokio::io::empty(),
        output,
        None,
        Some(ServiceSelection::Collections(selected)),
    ));
    let waiting = tokio::time::timeout(Duration::from_secs(2), observed).await;
    if !matches!(waiting, Ok(Ok(()))) {
        let _ = release.send(success);
        task.abort();
        let _ = task.await;
        let disposed = tokio::time::timeout(Duration::from_secs(2), disposed).await;
        panic!(
            "actual writer did not reach its flush owner; output disposed: {}",
            matches!(disposed, Ok(Ok(())))
        );
    }
    let fence = control.fence();
    let exact_fence = fence.as_ref().is_ok_and(|fence| fence.belongs_to(&control));
    let mut joined = fence.as_ref().ok().map(|fence| Box::pin(fence.joined()));
    let pending = match joined.as_mut() {
        Some(joined) => futures_util::poll!(joined.as_mut()).is_pending(),
        None => false,
    };
    let released = release.send(success).is_ok();
    let result = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
    if result.is_err() {
        task.abort();
        let _ = task.await;
    }
    let disposed = tokio::time::timeout(Duration::from_secs(2), disposed).await;
    // All deliberate holds are released and actual owners observed before any
    // oracle assertion, including on an unexpected implementation failure.
    assert!(exact_fence);
    assert!(pending, "fence is not a writer join");
    assert!(released);
    assert!(matches!(disposed, Ok(Ok(()))));
    let result = result.expect("actual invocation did not join").unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(2), joined.unwrap())
        .await
        .unwrap();
    if success {
        assert!(result.is_ok());
        let closed = closed.unwrap();
        assert!(closed.belongs_to(&control));
        assert!(!closed.belongs_to(&other));
    } else {
        assert!(result.is_err(), "retain actual output failure");
        assert!(
            closed.is_err(),
            "a joined failed writer is not confirmed cleanup"
        );
    }
}

#[tokio::test]
async fn native_close_acknowledgement_waits_for_actual_stdio_writer_join() {
    actual_pipe_flush(true).await;
}

#[tokio::test]
async fn actual_stdio_output_failure_cannot_become_native_close_acknowledgement() {
    actual_pipe_flush(false).await;
}

#[tokio::test]
async fn dropped_actual_stdio_invocation_stays_unconfirmed_after_late_writer_disposal() {
    let root = tempfile::tempdir().unwrap();
    let selected = selection(root.path());
    let control = selected.control();
    let (entered, observed) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (dropped, disposed) = oneshot::channel();
    let output = HeldFlush {
        entered: Some(entered),
        release: released,
        dropped: Some(dropped),
    };
    let task = tokio::spawn(run_io_composed(
        tokio::io::empty(),
        output,
        None,
        Some(ServiceSelection::Collections(selected)),
    ));
    let waiting = tokio::time::timeout(Duration::from_secs(2), observed).await;
    if !matches!(waiting, Ok(Ok(()))) {
        let _ = release.send(true);
        task.abort();
        let _ = task.await;
        let disposed = tokio::time::timeout(Duration::from_secs(2), disposed).await;
        panic!(
            "actual writer did not reach its flush owner; output disposed: {}",
            matches!(disposed, Ok(Ok(())))
        );
    }
    task.abort();
    let aborted = task.await;
    let _ = release.send(true);
    tokio::time::timeout(Duration::from_secs(2), disposed)
        .await
        .unwrap()
        .unwrap();
    assert!(aborted.unwrap_err().is_cancelled());
    assert!(
        control.fence().unwrap().joined().await.is_err(),
        "late output disposal cannot repair a lost native invocation owner"
    );
}
