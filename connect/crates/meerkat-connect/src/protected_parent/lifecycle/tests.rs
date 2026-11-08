//! Correlation and retained-owner controls. Actual root/nonroot packet and
//! installed native acceptance remain separate from these source controls.
use super::*;

fn nonce(byte: char) -> Nonce {
    Nonce::try_from(byte.to_string().repeat(64)).unwrap()
}
fn envelope() -> Envelope {
    Envelope {
        channel: nonce('1'),
        invocation: nonce('2'),
        reservation: nonce('3'),
        mapping_sha256: nonce('4'),
        request_id: nonce('5'),
    }
}
fn control() -> Control {
    Control {
        envelope: Envelope {
            request_id: nonce('6'),
            ..envelope()
        },
        control_id: nonce('7'),
    }
}

#[test]
fn controls_require_every_original_correlation_and_allocation() {
    let owner = envelope();
    assert!(checked_control(&control(), &owner, &nonce('7'), &[&owner.request_id]).is_ok());
    for field in 0..5 {
        let mut candidate = control();
        match field {
            0 => candidate.envelope.channel = nonce('8'),
            1 => candidate.envelope.invocation = nonce('8'),
            2 => candidate.envelope.reservation = nonce('8'),
            3 => candidate.envelope.mapping_sha256 = nonce('8'),
            _ => candidate.control_id = nonce('8'),
        }
        assert!(checked_control(&candidate, &owner, &nonce('7'), &[&owner.request_id]).is_err());
    }
}

#[test]
fn activation_and_fence_cannot_reuse_an_earlier_request() {
    let owner = envelope();
    let activation = control();
    let mut fence = control();
    fence.envelope.request_id = owner.request_id.clone();
    assert!(checked_control(&fence, &owner, &nonce('7'), &[&owner.request_id]).is_err());
    fence.envelope.request_id = activation.envelope.request_id.clone();
    assert!(
        checked_control(
            &fence,
            &owner,
            &nonce('7'),
            &[&owner.request_id, &activation.envelope.request_id]
        )
        .is_err()
    );
    fence.envelope.request_id = nonce('9');
    assert!(
        checked_control(
            &fence,
            &owner,
            &nonce('7'),
            &[&owner.request_id, &activation.envelope.request_id]
        )
        .is_ok()
    );
}

#[tokio::test]
async fn expired_deadline_refuses_before_polling_the_operation() {
    let polled = std::cell::Cell::new(false);
    let result = until(Instant::now(), async {
        polled.set(true);
        Ok::<_, ConnectError>(())
    })
    .await;
    assert!(result.is_err());
    assert!(!polled.get());
    assert!(authentication_deadline(0).is_err());
    assert!(authentication_deadline(now().saturating_sub(1)).is_err());
}

#[tokio::test]
async fn lost_observer_does_not_abort_its_retained_owner() {
    let (release, held) = tokio::sync::oneshot::channel();
    let (finished, observed) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        held.await.map_err(|_| refused())?;
        finished.send(()).map_err(|_| refused())?;
        Ok(())
    });
    // This exercises the actual public join observer, without fabricating a
    // native invocation, authentication packet or joined-disposal receipt.
    let owner = super::super::ProtectedServiceRun { task };
    let observer = tokio::spawn(owner.join());
    tokio::task::yield_now().await;
    observer.abort();
    let _ = observer.await;
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), observed)
        .await
        .unwrap()
        .unwrap();
}

// Linux post-activation integration only. The test pair uses its actual
// unprivileged process credentials, not a root parent or an authenticated login.
// Every packet still traverses ParentChannel send/receive, passcred and the real
// receive_one checks. The selection is the existing trusted Rust source fixture.
use crate::service_collection::{
    CollectionCursor, CollectionLimits, CollectionProgram, CollectionTarget,
    RequiredCollectionOperation, RequiredServiceCollection, ServiceCollectionSelection,
};
use futures_util::FutureExt;
use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, PollTimeout, poll},
    unistd::{pipe, read, write},
};
use serde_json::{Value, json};
use std::{
    io::Write,
    os::fd::{AsFd, OwnedFd},
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
};

const ACTIVE_TEST_BOUND: Duration = Duration::from_secs(5);
const MAX_FILL: usize = 1024 * 1024;
const INVALID_ORDINARY_FRAME: &[u8] =
    b"{\"jsonrpc\":\"2.0\",\"id\":\"h:held\",\"method\":\"not/a/native/method\",\"params\":{}}\n";

struct NoProgram;
impl CollectionProgram for NoProgram {
    fn accepts_target(&self, _: &CollectionTarget<'_>) -> bool {
        false
    }
    fn start(&self, _: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        panic!("post-activation pipe controls cannot start native provider work")
    }
}
fn active_selection(root: &Path, expires_at_ms: u64) -> ServiceCollectionSelection {
    let requirement = RequiredCollectionOperation::new(
        "read".into(),
        json!({"repository":"owner/one"}),
        vec!["read".into()],
        Arc::new(NoProgram),
    )
    .unwrap();
    let source = RequiredServiceCollection::new(
        "one".into(),
        "github".into(),
        "enterprise".into(),
        "agent".into(),
        vec![requirement],
        CollectionLimits::new(1, 1024, 1024).unwrap(),
    )
    .unwrap();
    let mut binding = crate::service_probe::tests::binding(root);
    binding.expires_at_ms = expires_at_ms;
    ServiceCollectionSelection::new(binding, vec![source]).unwrap()
}
fn parent_record(kind: &str, request_id: char) -> Vec<u8> {
    let owner = envelope();
    serde_json::to_vec(&json!({"version":1,"kind":kind,"channel":owner.channel,
        "invocation":owner.invocation,"reservation":owner.reservation,
        "mapping_sha256":owner.mapping_sha256,"request_id":nonce(request_id),
        "control_id":nonce('7')}))
    .unwrap()
}
fn expected_ack(kind: &str, disposition: Option<&str>) -> Value {
    let mut ack: Value = serde_json::from_slice(&parent_record(kind, '9')).unwrap();
    if let Some(disposition) = disposition {
        ack["disposition"] = json!(disposition);
    }
    ack
}
fn input_reader_disposed(peer: &OwnedFd) -> bool {
    let mut descriptors = [PollFd::new(peer.as_fd(), PollFlags::POLLOUT)];
    if poll(&mut descriptors, PollTimeout::ZERO).is_err() {
        return false;
    }
    let Some(events) = descriptors[0].revents() else {
        return false;
    };
    !events.contains(PollFlags::POLLNVAL)
        && events.intersects(PollFlags::POLLERR | PollFlags::POLLHUP)
}
async fn output_eof(peer: &OwnedFd) -> Option<Vec<u8>> {
    tokio::time::timeout(ACTIVE_TEST_BOUND, async {
        let mut output = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            match read(peer, &mut buffer) {
                Ok(0) => return Some(output),
                Ok(count) => {
                    output.extend_from_slice(&buffer[..count]);
                    if output.len() > MAX_FILL + 65536 {
                        return None;
                    }
                }
                Err(Errno::EAGAIN | Errno::EINTR) => tokio::task::yield_now().await,
                Err(_) => return None,
            }
        }
    })
    .await
    .ok()
    .flatten()
}
fn fill_output(output: &OwnedFd) -> usize {
    // Whole PIPE_BUF-sized writes first, then byte writes to prove EAGAIN even
    // for the small ordinary error response. No reader drains this before ACK.
    let mut total = 0;
    for size in [4096, 1] {
        let bytes = vec![b'x'; size];
        loop {
            match write(output, &bytes) {
                Ok(count) if count > 0 => {
                    total += count;
                    assert!(total <= MAX_FILL, "unexpected pipe capacity");
                }
                Err(Errno::EAGAIN) => break,
                Err(Errno::EINTR) => (),
                _ => panic!("could not fill actual output pipe"),
            }
        }
    }
    assert!(total > 0);
    total
}

struct ActiveHarness {
    // The test retains all opposite endpoints and the exact native control.
    // Its cleanup always runs after the caught body, including assertion panic.
    _root: tempfile::TempDir,
    child: Option<ParentChannel>,
    parent: Option<ParentChannel>,
    pipes: Option<crate::stdio::ProtectedCollectionPipes>,
    selected: Option<ServiceCollectionSelection>,
    control: ServiceProbeControl,
    foreign: ServiceProbeControl,
    expires_at_ms: u64,
    provider_root: PathBuf,
    input_peer: Option<OwnedFd>,
    output_peer: OwnedFd,
    fill_bytes: usize,
    task: Option<tokio::task::JoinHandle<Result<(), ConnectError>>>,
}
impl ActiveHarness {
    fn new(hold_writer: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let expires_at_ms = now() + 60_000;
        let selected = active_selection(root.path(), expires_at_ms);
        let control = selected.control();
        let foreign = active_selection(root.path(), expires_at_ms).control();
        let provider_root = root.path().join("provider-unused");
        let (child, parent) = ParentChannel::unprivileged_test_pair().unwrap();
        let (input, input_peer) = pipe().unwrap();
        let (output_peer, output) = pipe().unwrap();
        super::super::transport::protect(input_peer.as_fd(), true).unwrap();
        super::super::transport::protect(output_peer.as_fd(), true).unwrap();
        super::super::transport::protect(output.as_fd(), true).unwrap();
        let fill_bytes = if hold_writer { fill_output(&output) } else { 0 };
        let pipes = crate::stdio::ProtectedCollectionPipes::adopt(input, output).unwrap();
        Self {
            _root: root,
            child: Some(child),
            parent: Some(parent),
            pipes: Some(pipes),
            selected: Some(selected),
            control,
            foreign,
            expires_at_ms,
            provider_root,
            input_peer: Some(input_peer),
            output_peer,
            fill_bytes,
            task: None,
        }
    }
    async fn send(&mut self, kind: &str, request_id: char) {
        tokio::time::timeout(
            ACTIVE_TEST_BOUND,
            self.parent
                .as_mut()
                .unwrap()
                .send(&parent_record(kind, request_id)),
        )
        .await
        .unwrap()
        .unwrap();
    }
    fn start(&mut self) {
        let mut child = self.child.take().unwrap();
        let pipes = self.pipes.take().unwrap();
        let selected = self.selected.take().unwrap();
        let provider_root = self.provider_root.clone();
        let expires_at_ms = self.expires_at_ms;
        self.task = Some(tokio::spawn(async move {
            // ACTIVATE is read from the real queued socket. This starts at the
            // already-bound stage; no attestation or root authentication claim.
            let packet = receive(&mut child, Instant::now() + ACTIVE_TEST_BOUND).await?;
            let ParentRecord::Activate(activation) =
                wire::decode(packet.bytes()).map_err(|_| refused())?
            else {
                return Err(refused());
            };
            let owner = envelope();
            let allocation = nonce('7');
            checked_control(&activation, &owner, &allocation, &[&owner.request_id])?;
            run_activated(
                &mut child,
                pipes,
                selected,
                provider_root,
                ActiveBinding {
                    owner,
                    activation,
                    allocation,
                    expires_at_ms,
                },
            )
            .await
        }));
    }
    fn queue_ordinary_error(&self) {
        assert_eq!(
            write(self.input_peer.as_ref().unwrap(), INVALID_ORDINARY_FRAME).unwrap(),
            INVALID_ORDINARY_FRAME.len()
        );
    }
    async fn input_drained(&self) {
        tokio::time::timeout(ACTIVE_TEST_BOUND, async {
            while rustix::io::ioctl_fionread(self.input_peer.as_ref().unwrap()).unwrap() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("actual stdio did not read the queued frame");
    }
    async fn reader_disposed(&self) {
        tokio::time::timeout(ACTIVE_TEST_BOUND, async {
            while !input_reader_disposed(self.input_peer.as_ref().unwrap()) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("native input owner was not disposed");
    }
    async fn ack(&mut self) -> Value {
        let packet =
            tokio::time::timeout(ACTIVE_TEST_BOUND, self.parent.as_mut().unwrap().receive())
                .await
                .expect("parent ACK timeout")
                .expect("parent ACK transport");
        serde_json::from_slice(packet.bytes()).expect("actual parent ACK JSON")
    }
    async fn ordinary_response(&self) -> Value {
        let bytes = tokio::time::timeout(ACTIVE_TEST_BOUND, async {
            let mut output = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                match read(&self.output_peer, &mut buffer) {
                    Ok(0) => panic!("ordinary response ended early"),
                    Ok(count) => {
                        output.extend_from_slice(&buffer[..count]);
                        assert!(output.len() <= 65536);
                        if output.ends_with(b"\n") {
                            return output;
                        }
                    }
                    Err(Errno::EAGAIN | Errno::EINTR) => tokio::task::yield_now().await,
                    Err(_) => panic!("ordinary response read failed"),
                }
            }
        })
        .await
        .expect("ordinary error response timeout");
        serde_json::from_slice(&bytes).unwrap()
    }
    async fn joined_task(&mut self) -> Result<(), ConnectError> {
        let joined = tokio::time::timeout(ACTIVE_TEST_BOUND, self.task.as_mut().unwrap())
            .await
            .expect("retained activation owner timeout");
        self.task.take(); // Its terminal JoinHandle was already observed once.
        joined.expect("retained activation task panicked")
    }
    async fn cleanup(&mut self) -> bool {
        self.control.retire();
        self.parent.take();
        self.child.take();
        self.pipes.take();
        self.selected.take();
        // Drain before joining so a body failure never leaves the actual writer
        // held by this test. If the invocation must be aborted, observe pipe EOF
        // again afterward: dropping its JoinHandle can otherwise detach writer.
        let first_eof = output_eof(&self.output_peer).await.is_some();
        let mut joined = true;
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(ACTIVE_TEST_BOUND, &mut task).await {
                Ok(outcome) => joined = outcome.is_ok(),
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    joined = false;
                }
            }
        }
        let final_eof = output_eof(&self.output_peer).await.is_some();
        let input_closed = self.input_peer.as_ref().is_none_or(input_reader_disposed);
        self.input_peer.take();
        first_eof && final_eof && joined && input_closed
    }
}
async fn finish_active_case(harness: &mut ActiveHarness, body: std::thread::Result<()>) {
    let cleanup = harness.cleanup().await;
    if let Err(panic) = body {
        if !cleanup {
            let _ = writeln!(
                std::io::stderr().lock(),
                "post-activation test cleanup unconfirmed"
            );
        }
        std::panic::resume_unwind(panic);
    }
    assert!(
        cleanup,
        "post-activation test did not dispose all actual owners"
    );
}
fn require_ordinary_error(response: &Value) {
    assert_eq!(response["id"], json!("h:held"));
    assert_eq!(response["error"]["code"], json!(-32601));
    assert!(response.get("result").is_none());
}

#[tokio::test]
async fn queued_activate_then_fence_registers_and_joins_the_actual_stdio_invocation() {
    let mut harness = ActiveHarness::new(false);
    let body = AssertUnwindSafe(async {
        // Both actual packets are queued before the child reads ACTIVATE.
        harness.send("ACTIVATE", '6').await;
        harness.send("FENCE", '9').await;
        harness.start();
        assert_eq!(harness.ack().await, expected_ack("FENCE_ACK", None));
        assert_eq!(
            harness.ack().await,
            expected_ack("DISPOSED_ACK", Some("joined"))
        );
        assert!(harness.joined_task().await.is_ok());
        assert!(
            input_reader_disposed(harness.input_peer.as_ref().unwrap()),
            "caller kept stdin open"
        );
        assert!(output_eof(&harness.output_peer).await.is_some());
        let fence = harness.control.fence().unwrap();
        assert!(fence.belongs_to(&harness.control) && !fence.belongs_to(&harness.foreign));
        let closed = fence.joined().await.unwrap();
        assert!(closed.belongs_to(&harness.control) && !closed.belongs_to(&harness.foreign));
    })
    .catch_unwind()
    .await;
    finish_active_case(&mut harness, body).await;
}

#[tokio::test]
async fn parent_eof_retires_and_joins_actual_owned_stdio_with_input_still_open() {
    let mut harness = ActiveHarness::new(false);
    let body = AssertUnwindSafe(async {
        harness.send("ACTIVATE", '6').await;
        harness.queue_ordinary_error();
        harness.start();
        require_ordinary_error(&harness.ordinary_response().await);
        assert!(!input_reader_disposed(harness.input_peer.as_ref().unwrap()));
        harness.parent.take(); // Actual peer EOF, not a forged control or stdin EOF.
        assert!(
            harness.joined_task().await.is_err(),
            "lost parent remains an error"
        );
        assert!(input_reader_disposed(harness.input_peer.as_ref().unwrap()));
        assert!(output_eof(&harness.output_peer).await.is_some());
        let fence = harness.control.fence().unwrap();
        let closed = fence.joined().await.unwrap();
        assert!(closed.belongs_to(&harness.control) && !closed.belongs_to(&harness.foreign));
    })
    .catch_unwind()
    .await;
    finish_active_case(&mut harness, body).await;
}

#[tokio::test]
async fn fence_ack_precedes_real_writer_disposal_and_disposed_ack_waits_for_pipe_drain() {
    let mut harness = ActiveHarness::new(true);
    let body = AssertUnwindSafe(async {
        harness.send("ACTIVATE", '6').await;
        harness.queue_ordinary_error();
        harness.start();
        harness.input_drained().await;
        harness.send("FENCE", '9').await;
        assert_eq!(harness.ack().await, expected_ack("FENCE_ACK", None));
        harness.reader_disposed().await;
        assert_eq!(
            rustix::io::ioctl_fionread(&harness.output_peer).unwrap(),
            harness.fill_bytes as u64
        );
        let fence = harness.control.fence().unwrap();
        assert!(fence.belongs_to(&harness.control) && !fence.belongs_to(&harness.foreign));
        let mut joined = Box::pin(fence.joined());
        assert!(
            futures_util::poll!(joined.as_mut()).is_pending(),
            "real writer is still held"
        );
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                harness.parent.as_mut().unwrap().receive()
            )
            .await
            .is_err(),
            "no DISPOSED_ACK while the actual writer cannot finish"
        );
        let output = output_eof(&harness.output_peer)
            .await
            .expect("actual output did not dispose after drain");
        assert!(output.len() > harness.fill_bytes);
        assert!(
            output[..harness.fill_bytes]
                .iter()
                .all(|byte| *byte == b'x')
        );
        require_ordinary_error(&serde_json::from_slice(&output[harness.fill_bytes..]).unwrap());
        assert_eq!(
            harness.ack().await,
            expected_ack("DISPOSED_ACK", Some("joined"))
        );
        assert!(harness.joined_task().await.is_ok());
        let closed = tokio::time::timeout(ACTIVE_TEST_BOUND, joined)
            .await
            .unwrap()
            .unwrap();
        assert!(closed.belongs_to(&harness.control) && !closed.belongs_to(&harness.foreign));
    })
    .catch_unwind()
    .await;
    finish_active_case(&mut harness, body).await;
}
