//! Localhost loopback OAuth callback server.
//!
//! The host opens the authorize URL in the user's browser; the browser
//! redirects to the configured loopback callback URL. Anthropic and Gemini use
//! ephemeral loopback ports; OpenAI ChatGPT mirrors Codex's fixed localhost
//! callback contract. This module parses the query, validates the state, and
//! returns the code to the caller.
//!
//! Reference-CLI parity: Codex `codex-rs/login/src/server.rs`, Gemini CLI
//! `packages/core/src/code_assist/oauth2.ts:113-360`.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::serve::Listener;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use super::OAuthError;

#[derive(Clone)]
pub struct LoopbackOutcome {
    pub code: String,
    pub state: String,
}

impl std::fmt::Debug for LoopbackOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopbackOutcome").finish_non_exhaustive()
    }
}

/// One mechanical termination signal shared by the accepted streams. It never
/// admits, expires or consumes OAuth state; the native flow owner does that.
type IoTermination = Shared<BoxFuture<'static, ()>>;

struct CallbackListener<L> {
    inner: L,
    terminated: IoTermination,
}

impl<L: Listener> Listener for CallbackListener<L> {
    type Io = CallbackIo<L::Io>;
    type Addr = L::Addr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (inner, address) = self.inner.accept().await;
        (
            CallbackIo {
                inner,
                terminated: self.terminated.clone(),
                termination_observed: false,
            },
            address,
        )
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

/// Axum still owns every accepted connection task and its aggregate drain.
/// Termination wakes a blocked read/write/shutdown so that owner can finish;
/// it does not replace the drain with a socket counter or a task registry.
struct CallbackIo<I> {
    inner: I,
    terminated: IoTermination,
    termination_observed: bool,
}

impl<I> CallbackIo<I> {
    fn poll_termination(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        if self.termination_observed || Pin::new(&mut self.terminated).poll(cx).is_ready() {
            // Shared clones cannot be polled again after Ready. Termination is
            // permanent for this stream, including later flush/shutdown polls.
            self.termination_observed = true;
            Err(io::ErrorKind::ConnectionAborted.into())
        } else {
            Ok(())
        }
    }
}

impl<I: AsyncRead + Unpin> AsyncRead for CallbackIo<I> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.poll_termination(cx)?;
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<I: AsyncWrite + Unpin> AsyncWrite for CallbackIo<I> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.poll_termination(cx)?;
        Pin::new(&mut this.inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.poll_termination(cx)?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.poll_termination(cx)?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

/// The Axum callback task did not retire normally. Private: the public
/// surfaces report it as their own typed terminal, never as a receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CallbackRetirementFailed;

impl From<CallbackRetirementFailed> for OAuthError {
    fn from(_: CallbackRetirementFailed) -> Self {
        OAuthError::CallbackParse("callback task retirement failed".into())
    }
}

/// Owns transport mechanics, never OAuth flow authority. Explicit wait/close
/// retain and await Axum's actual accepted-connection drain. Drop can only signal
/// termination: the same Axum task continues draining, without a joined receipt.
struct CallbackServer {
    shutdown: Option<oneshot::Sender<()>>,
    terminate_io: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<io::Result<()>>>,
    /// Terminal join disposition. Once the task is joined, every later join
    /// reports this same result; a failed retirement never becomes success.
    retired: Option<Result<(), CallbackRetirementFailed>>,
    #[cfg(test)]
    drained: Option<oneshot::Receiver<bool>>,
}

impl CallbackServer {
    fn signal(&mut self, terminate_io: bool) {
        if terminate_io && let Some(terminate) = self.terminate_io.take() {
            let _ = terminate.send(());
        }
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }

    async fn join(&mut self, terminate_io: bool) -> Result<(), CallbackRetirementFailed> {
        self.signal(terminate_io);
        if let Some(retired) = self.retired {
            return retired;
        }
        let Some(task) = self.task.as_mut() else {
            // Only `retired` ends the task's ownership; an absent handle
            // without a recorded disposition is not a joined drain.
            return Err(CallbackRetirementFailed);
        };
        // Keep the handle in self across await. If this future is cancelled,
        // owned Drop can still terminate I/O; it never aborts Axum's drain.
        let retired = match task.await {
            Ok(Ok(())) => Ok(()),
            _ => Err(CallbackRetirementFailed),
        };
        self.retired = Some(retired);
        self.task = None;
        retired
    }
}

impl Drop for CallbackServer {
    fn drop(&mut self) {
        self.signal(true);
        // Dropping the JoinHandle leaves its existing task running. That task
        // retains Axum's connection-drain barrier until every accepted I/O owner
        // retires. Synchronous Drop cannot return an awaited cleanup receipt.
    }
}

type CallbackResult = Result<LoopbackOutcome, OAuthError>;

/// Where the single callback result is. Each value is handed out at most once.
enum CallbackSlot {
    /// Nothing taken from the receiver yet; a result may already be queued.
    Unreceived(oneshot::Receiver<CallbackResult>),
    /// Taken from the receiver, not yet handed to a caller.
    Retained(CallbackResult),
    /// Handed to a caller. The receiver is gone, so it is never polled again.
    Delivered,
    /// The publisher retired without producing a result.
    Closed,
}

impl CallbackSlot {
    /// Cancel-safe: the receiver stays in the slot until it completes, and the
    /// completed value is stored before any further await.
    async fn receive(&mut self) {
        if let Self::Unreceived(receiver) = self {
            *self = match receiver.await {
                Ok(result) => Self::Retained(result),
                Err(_) => Self::Closed,
            };
        }
    }

    /// Whether a published result was never handed to a caller. Called only
    /// after the joined drain, when no handler can publish any more.
    fn undelivered(&mut self) -> bool {
        match self {
            Self::Unreceived(receiver) => receiver.try_recv().is_ok(),
            Self::Retained(_) => true,
            Self::Delivered | Self::Closed => false,
        }
    }
}

/// Proof that the callback listener is closed and Axum's accepted-connection
/// drain was joined. Only [`LoopbackHandle::close`] and
/// [`LoopbackHandle::wait_or_cancel`] produce one, after that join returned
/// normally. It describes callback transport only: it neither authenticates
/// an account nor reverses anything the browser or provider already did.
#[must_use]
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopbackClosed {
    /// A callback reached the listener but was never handed to a caller. Its
    /// code and state are dropped with the handle and never exposed.
    pub undelivered_callback: bool,
}

/// Terminal result of [`LoopbackHandle::wait_or_cancel`]. Every variant except
/// `RetirementFailed` carries the joined [`LoopbackClosed`] receipt.
#[must_use]
#[non_exhaustive]
#[derive(Debug)]
pub enum LoopbackWaitEnd {
    /// The callback carried the expected state. Transport only: the flow
    /// owner still verifies and exchanges the code.
    Completed {
        outcome: LoopbackOutcome,
        closed: LoopbackClosed,
    },
    /// The callback was refused (state mismatch, provider denial, malformed
    /// callback), or no callback can be delivered any more.
    Refused {
        error: OAuthError,
        closed: LoopbackClosed,
    },
    /// The deadline passed first.
    TimedOut { closed: LoopbackClosed },
    /// The caller's cancellation was ready first.
    Cancelled { closed: LoopbackClosed },
    /// The callback task did not retire normally. No receipt exists.
    RetirementFailed,
}

/// Private terminal of one borrowed wait.
enum WaitStep {
    Delivered(CallbackResult),
    TimedOut,
    AlreadyDelivered,
    PublisherClosed,
    RetirementFailed,
}

pub struct LoopbackHandle {
    pub redirect_url: String,
    server: CallbackServer,
    callback: CallbackSlot,
    /// The earliest deadline any wait was given. A later deadline never
    /// extends it, so a re-wait cannot renew the login window.
    window: Option<tokio::time::Instant>,
}

pub struct LoopbackBinding {
    pub redirect_url: String,
    server: CallbackServer,
    receiver: oneshot::Receiver<Result<LoopbackOutcome, OAuthError>>,
    expected_state: Arc<Mutex<Option<String>>>,
}

impl LoopbackBinding {
    pub fn expect_state(self, expected_state: String) -> LoopbackHandle {
        *self.expected_state.lock() = Some(expected_state);
        LoopbackHandle {
            redirect_url: self.redirect_url,
            server: self.server,
            callback: CallbackSlot::Unreceived(self.receiver),
            window: None,
        }
    }

    pub async fn cancel(mut self) -> Result<(), OAuthError> {
        Ok(self.server.join(true).await?)
    }
}

impl LoopbackHandle {
    /// Await one callback using the existing login window. No separate cleanup
    /// timer extends the ceremony; physical retirement is still awaited. A timeout
    /// terminates accepted I/O and joins the same Axum drain owner. Caller
    /// abandonment signals termination via Drop but has no joined receipt.
    pub async fn wait(self, deadline: Duration) -> Result<LoopbackOutcome, OAuthError> {
        let deadline = tokio::time::Instant::now() + deadline;
        let mut handle = self;
        let waited = handle.wait_until(deadline).await;
        handle.server.join(true).await?;
        waited
    }

    /// Borrowed, cancel-safe wait. Dropping this future loses nothing: the
    /// receiver, a callback already taken from it and the server all stay in
    /// the handle, and the caller can still [`close`](Self::close) it for a
    /// joined receipt.
    ///
    /// The single absolute `deadline` bounds both receiving the callback and
    /// the graceful drain after it. When it passes, accepted I/O is terminated
    /// and the drain is still awaited, so this can return after the deadline.
    /// The earliest deadline given to any wait on this handle applies; a later
    /// one never renews the window. A callback is handed out at most once: a
    /// wait after delivery returns an error without the code or state.
    ///
    /// Ties: a deadline already passed on entry wins over a queued callback,
    /// which stays undelivered. Within one poll, a callback and drain that
    /// complete win over a deadline that expires in that same poll.
    pub async fn wait_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<LoopbackOutcome, OAuthError> {
        match self.wait_step(deadline).await {
            WaitStep::Delivered(result) => result,
            WaitStep::TimedOut => Err(OAuthError::Timeout),
            WaitStep::AlreadyDelivered => Err(OAuthError::CallbackParse(
                "callback already delivered".into(),
            )),
            WaitStep::PublisherClosed => Err(OAuthError::CallbackParse("receiver closed".into())),
            WaitStep::RetirementFailed => Err(CallbackRetirementFailed.into()),
        }
    }

    async fn wait_step(&mut self, deadline: tokio::time::Instant) -> WaitStep {
        let LoopbackHandle {
            server,
            callback,
            window,
            redirect_url: _,
        } = self;
        if matches!(server.retired, Some(Err(CallbackRetirementFailed))) {
            return WaitStep::RetirementFailed;
        }
        match callback {
            CallbackSlot::Delivered => return WaitStep::AlreadyDelivered,
            CallbackSlot::Closed => return WaitStep::PublisherClosed,
            CallbackSlot::Unreceived(_) | CallbackSlot::Retained(_) => {}
        }
        let deadline = window.map_or(deadline, |earliest| earliest.min(deadline));
        *window = Some(deadline);
        let graceful = if tokio::time::Instant::now() >= deadline {
            None
        } else {
            tokio::time::timeout_at(deadline, async {
                callback.receive().await;
                server.join(false).await
            })
            .await
            .ok()
        };
        let Some(joined) = graceful else {
            // The window ended first. Terminate accepted I/O and join the
            // drain; a callback already taken stays retained, not delivered.
            return match server.join(true).await {
                Ok(()) => WaitStep::TimedOut,
                Err(CallbackRetirementFailed) => WaitStep::RetirementFailed,
            };
        };
        if joined.is_err() {
            return WaitStep::RetirementFailed;
        }
        match std::mem::replace(callback, CallbackSlot::Delivered) {
            CallbackSlot::Retained(result) => WaitStep::Delivered(result),
            other => {
                *callback = other;
                WaitStep::PublisherClosed
            }
        }
    }

    /// Terminate accepted I/O, stop the listener and await Axum's actual
    /// connection drain. The receipt exists only after that join returned
    /// normally; a failed retirement stays an error on every later call.
    /// Dropping this future signals termination without a joined receipt.
    pub async fn close(self) -> Result<LoopbackClosed, OAuthError> {
        let mut handle = self;
        handle.server.join(true).await?;
        Ok(LoopbackClosed {
            undelivered_callback: handle.callback.undelivered(),
        })
    }

    /// [`wait_until`](Self::wait_until) raced against a caller-owned
    /// cancellation, then [`close`](Self::close). It always joins the drain
    /// before returning normally.
    ///
    /// Ties: a cancellation that is ready when polled wins, and a queued
    /// callback is then reported as undelivered, never delivered. Between
    /// callback and deadline, `wait_until`'s rules apply. Dropping this future
    /// signals termination without a joined receipt.
    pub async fn wait_or_cancel(
        self,
        deadline: tokio::time::Instant,
        cancel: impl Future<Output = ()> + Send,
    ) -> LoopbackWaitEnd {
        let mut handle = self;
        let step = tokio::select! {
            biased;
            () = cancel => None,
            step = handle.wait_step(deadline) => Some(step),
        };
        let closed = match handle.server.join(true).await {
            Ok(()) => LoopbackClosed {
                undelivered_callback: handle.callback.undelivered(),
            },
            Err(CallbackRetirementFailed) => return LoopbackWaitEnd::RetirementFailed,
        };
        match step {
            None => LoopbackWaitEnd::Cancelled { closed },
            Some(WaitStep::Delivered(Ok(outcome))) => {
                LoopbackWaitEnd::Completed { outcome, closed }
            }
            Some(WaitStep::Delivered(Err(error))) => LoopbackWaitEnd::Refused { error, closed },
            Some(WaitStep::TimedOut) => LoopbackWaitEnd::TimedOut { closed },
            Some(WaitStep::AlreadyDelivered) => LoopbackWaitEnd::Refused {
                error: OAuthError::CallbackParse("callback already delivered".into()),
                closed,
            },
            Some(WaitStep::PublisherClosed) => LoopbackWaitEnd::Refused {
                error: OAuthError::CallbackParse("receiver closed".into()),
                closed,
            },
            Some(WaitStep::RetirementFailed) => LoopbackWaitEnd::RetirementFailed,
        }
    }

    /// Close without a receipt. Kept for existing callers; see
    /// [`close`](Self::close).
    pub async fn cancel(self) -> Result<(), OAuthError> {
        self.close().await.map(|_| ())
    }
}

type ResultSender = oneshot::Sender<Result<LoopbackOutcome, OAuthError>>;

#[derive(Clone)]
struct CallbackState {
    expected_state: Arc<Mutex<Option<String>>>,
    result_tx: Arc<Mutex<Option<ResultSender>>>,
}

async fn callback_handler(
    State(state): State<CallbackState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(expected_state) = state.expected_state.lock().clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Html("<h1>Authorization callback is not ready</h1>".to_string()),
        );
    };
    let tx = state.result_tx.lock().take();
    match (
        params.get("code"),
        params.get("state"),
        params.get("error"),
        tx,
    ) {
        (_, actual_state, Some(err), Some(tx)) => {
            let err = match actual_state {
                None => OAuthError::CallbackParse("invalid callback".into()),
                Some(actual_state) if actual_state != &expected_state => OAuthError::StateMismatch,
                Some(_) if err == "access_denied" => OAuthError::UserDenied,
                Some(_) => OAuthError::CallbackParse("provider denied callback".into()),
            };
            let _ = tx.send(Err(err));
            (
                StatusCode::BAD_REQUEST,
                Html("<h1>Authorization error</h1>".to_string()),
            )
        }
        (Some(code), Some(actual_state), _, Some(tx)) => {
            if actual_state == &expected_state {
                let _ = tx.send(Ok(LoopbackOutcome {
                    code: code.clone(),
                    state: actual_state.clone(),
                }));
                (
                    StatusCode::OK,
                    Html(
                        "<h1>Authorization complete</h1><p>You may close this window.</p>"
                            .to_string(),
                    ),
                )
            } else {
                let _ = tx.send(Err(OAuthError::StateMismatch));
                (
                    StatusCode::BAD_REQUEST,
                    Html("<h1>State mismatch</h1>".to_string()),
                )
            }
        }
        (_, _, _, tx) => {
            if let Some(tx) = tx {
                let _ = tx.send(Err(OAuthError::CallbackParse("invalid callback".into())));
            }
            (
                StatusCode::BAD_REQUEST,
                Html("<h1>Invalid callback</h1>".to_string()),
            )
        }
    }
}

/// Bind a loopback listener and return a `LoopbackHandle`. The caller
/// opens `handle.redirect_url` (with the right query params) in the
/// browser; `handle.wait(deadline)` returns the `(code, state)` pair or
/// an `OAuthError`.
pub async fn run_loopback_callback(
    expected_state: String,
    path: &str,
) -> Result<LoopbackHandle, OAuthError> {
    Ok(bind_loopback_callback(path)
        .await?
        .expect_state(expected_state))
}

/// Bind a loopback listener before the OAuth authority has minted state.
///
/// Some callers need the redirect URL before state can be admitted. They bind
/// first, use `redirect_url` to ask their authority to start the flow, then
/// call [`LoopbackBinding::expect_state`] before opening the browser.
pub async fn bind_loopback_callback(path: &str) -> Result<LoopbackBinding, OAuthError> {
    bind_loopback_callback_with_redirect(path, "127.0.0.1", &[0]).await
}

/// Bind a loopback listener while advertising a provider-specific redirect host
/// and preferred ports. A port value of `0` asks the OS for an ephemeral port.
pub async fn bind_loopback_callback_with_redirect(
    path: &str,
    redirect_host: &str,
    preferred_ports: &[u16],
) -> Result<LoopbackBinding, OAuthError> {
    let ports = if preferred_ports.is_empty() {
        &[0][..]
    } else {
        preferred_ports
    };
    let mut last_error = None;
    let mut listener = None;
    for port in ports {
        let bind_addr = format!("127.0.0.1:{port}");
        match TcpListener::bind(&bind_addr).await {
            Ok(bound) => {
                listener = Some(bound);
                break;
            }
            Err(err) => {
                last_error = Some(format!("{bind_addr}: {err}"));
            }
        }
    }
    let listener = listener.ok_or_else(|| {
        OAuthError::InvalidConfig(format!(
            "bind: {}",
            last_error.unwrap_or_else(|| "no callback ports configured".to_string())
        ))
    })?;
    start_loopback_callback(listener, path, redirect_host)
}

fn start_loopback_callback<L>(
    listener: L,
    path: &str,
    redirect_host: &str,
) -> Result<LoopbackBinding, OAuthError>
where
    L: Listener<Addr = SocketAddr>,
{
    let (result_tx, result_rx) = oneshot::channel();
    let expected_state = Arc::new(Mutex::new(None));
    let state = CallbackState {
        expected_state: Arc::clone(&expected_state),
        result_tx: Arc::new(Mutex::new(Some(result_tx))),
    };

    let app = Router::new()
        .route(path, get(callback_handler))
        .with_state(state);
    let addr = listener
        .local_addr()
        .map_err(|e| OAuthError::InvalidConfig(format!("addr: {e}")))?;
    let redirect_url = format!("http://{}:{}{}", redirect_host, addr.port(), path);

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (terminate_tx, terminate_rx) = oneshot::channel();
    let terminated = async move {
        let _ = terminate_rx.await;
    }
    .boxed()
    .shared();
    let listener = CallbackListener {
        inner: listener,
        terminated,
    };
    #[cfg(test)]
    let (drained_tx, drained_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
        #[cfg(test)]
        let _ = drained_tx.send(result.is_ok());
        result
    });

    Ok(LoopbackBinding {
        redirect_url,
        server: CallbackServer {
            shutdown: Some(shutdown_tx),
            terminate_io: Some(terminate_tx),
            task: Some(task),
            retired: None,
            #[cfg(test)]
            drained: Some(drained_rx),
        },
        receiver: result_rx,
        expected_state,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod ownership_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::time::timeout;

    fn assert_connection_aborted<T>(result: Poll<io::Result<T>>) {
        assert!(
            matches!(result, Poll::Ready(Err(error)) if error.kind() == io::ErrorKind::ConnectionAborted),
            "terminated callback I/O did not refuse the operation"
        );
    }

    #[test]
    fn accepted_io_termination_remains_latched_across_repeated_io_polls() {
        let (inner, _peer) = tokio::io::duplex(16);
        let (terminate_tx, terminate_rx) = oneshot::channel();
        let terminated = async move {
            let _ = terminate_rx.await;
        }
        .boxed()
        .shared();
        let mut io = CallbackIo {
            inner,
            terminated,
            termination_observed: false,
        };
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut bytes = [0; 1];
        let mut buffer = ReadBuf::new(&mut bytes);
        assert!(
            Pin::new(&mut io)
                .poll_read(&mut cx, &mut buffer)
                .is_pending()
        );
        terminate_tx.send(()).unwrap();
        for _ in 0..3 {
            let mut buffer = ReadBuf::new(&mut bytes);
            assert_connection_aborted(Pin::new(&mut io).poll_read(&mut cx, &mut buffer));
            assert_connection_aborted(Pin::new(&mut io).poll_write(&mut cx, b"x"));
            assert_connection_aborted(Pin::new(&mut io).poll_flush(&mut cx));
            assert_connection_aborted(Pin::new(&mut io).poll_shutdown(&mut cx));
            assert!(buffer.filled().is_empty());
        }
    }

    #[derive(Default)]
    struct IoProbe {
        accepted: Mutex<Option<oneshot::Sender<()>>>,
        read_pending: Mutex<Option<oneshot::Sender<()>>>,
        shutdown_pending: Mutex<Option<oneshot::Sender<()>>>,
        dropped: Mutex<Option<oneshot::Sender<()>>>,
    }

    struct ProbeReceivers {
        accepted: oneshot::Receiver<()>,
        read_pending: oneshot::Receiver<()>,
        shutdown_pending: oneshot::Receiver<()>,
        dropped: oneshot::Receiver<()>,
        // Sender Drop releases the test gate even if an assertion unwinds.
        _release: oneshot::Sender<()>,
    }

    struct HeldListener {
        inner: TcpListener,
        probe: Arc<IoProbe>,
        released: IoTermination,
    }

    impl Listener for HeldListener {
        type Io = HeldIo;
        type Addr = SocketAddr;

        async fn accept(&mut self) -> (Self::Io, Self::Addr) {
            let (inner, address) = Listener::accept(&mut self.inner).await;
            if let Some(sender) = self.probe.accepted.lock().take() {
                let _ = sender.send(());
            }
            (
                HeldIo {
                    inner,
                    probe: Arc::clone(&self.probe),
                    released: self.released.clone(),
                    release_observed: false,
                    read_data: false,
                },
                address,
            )
        }

        fn local_addr(&self) -> io::Result<Self::Addr> {
            self.inner.local_addr()
        }
    }

    struct HeldIo {
        inner: TcpStream,
        probe: Arc<IoProbe>,
        released: IoTermination,
        release_observed: bool,
        read_data: bool,
    }

    impl AsyncRead for HeldIo {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let before = buf.filled().len();
            let result = Pin::new(&mut self.inner).poll_read(cx, buf);
            if buf.filled().len() > before {
                self.read_data = true;
            }
            if result.is_pending()
                && self.read_data
                && let Some(sender) = self.probe.read_pending.lock().take()
            {
                let _ = sender.send(());
            }
            result
        }
    }

    impl AsyncWrite for HeldIo {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut self.inner).poll_write(cx, buf)
        }
        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }
        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            // The completed probe request has reached native socket shutdown,
            // but the real accepted stream remains owned until I/O termination.
            if !self.release_observed {
                if Pin::new(&mut self.released).poll(cx).is_pending() {
                    if let Some(sender) = self.probe.shutdown_pending.lock().take() {
                        let _ = sender.send(());
                    }
                    return Poll::Pending;
                }
                self.release_observed = true;
            }
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    impl Drop for HeldIo {
        fn drop(&mut self) {
            if let Some(sender) = self.probe.dropped.lock().take() {
                let _ = sender.send(());
            }
        }
    }

    #[tokio::test]
    async fn held_io_release_remains_latched_across_repeated_shutdown_polls() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (peer, accepted) = tokio::join!(TcpStream::connect(address), listener.accept());
        let _peer = peer.unwrap();
        let (inner, _) = accepted.unwrap();
        let (release_tx, release_rx) = oneshot::channel();
        let released = async move {
            let _ = release_rx.await;
        }
        .boxed()
        .shared();
        let mut io = HeldIo {
            inner,
            probe: Arc::new(IoProbe::default()),
            released,
            release_observed: false,
            read_data: false,
        };
        futures::future::poll_fn(|cx| {
            assert!(Pin::new(&mut io).poll_shutdown(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        release_tx.send(()).unwrap();
        for _ in 0..3 {
            io.shutdown().await.unwrap();
        }
    }

    async fn observed_binding() -> (LoopbackBinding, ProbeReceivers) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (accepted_tx, accepted) = oneshot::channel();
        let (read_tx, read_pending) = oneshot::channel();
        let (shutdown_tx, shutdown_pending) = oneshot::channel();
        let (drop_tx, dropped) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let probe = Arc::new(IoProbe {
            accepted: Mutex::new(Some(accepted_tx)),
            read_pending: Mutex::new(Some(read_tx)),
            shutdown_pending: Mutex::new(Some(shutdown_tx)),
            dropped: Mutex::new(Some(drop_tx)),
        });
        let listener = HeldListener {
            inner: listener,
            probe,
            released: async move {
                let _ = release_rx.await;
            }
            .boxed()
            .shared(),
        };
        let binding = start_loopback_callback(listener, "/callback", "127.0.0.1").unwrap();
        (
            binding,
            ProbeReceivers {
                accepted,
                read_pending,
                shutdown_pending,
                dropped,
                _release: release_tx,
            },
        )
    }

    async fn incomplete_peer(binding: &LoopbackBinding, probe: &mut ProbeReceivers) -> TcpStream {
        let address = binding
            .redirect_url
            .strip_prefix("http://")
            .unwrap()
            .strip_suffix("/callback")
            .unwrap();
        let mut peer = TcpStream::connect(address).await.unwrap();
        // Leave the headers incomplete until the actual accepted stream has
        // observed pending input. The unmatched route never settles OAuth.
        peer.write_all(b"GET /ownership-probe HTTP/1.1\r\nHost: fixture\r\nConnection: close\r\n")
            .await
            .unwrap();
        timeout(Duration::from_secs(2), &mut probe.accepted)
            .await
            .unwrap()
            .unwrap();
        timeout(Duration::from_secs(2), &mut probe.read_pending)
            .await
            .unwrap()
            .unwrap();
        peer
    }

    async fn enter_held_shutdown(peer: &mut TcpStream, probe: &mut ProbeReceivers) {
        // Buffered partial headers remain an in-progress Hyper request during
        // graceful shutdown. Finish this unmatched request so the native 404
        // response reaches the separately held poll_shutdown seam. No callback
        // result is produced, and the test gate remains closed.
        peer.write_all(b"\r\n").await.unwrap();
        timeout(Duration::from_secs(2), &mut probe.shutdown_pending)
            .await
            .unwrap()
            .unwrap();
        let mut status = [0; b"HTTP/1.1 404 Not Found\r\n".len()];
        timeout(Duration::from_secs(2), peer.read_exact(&mut status))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&status, b"HTTP/1.1 404 Not Found\r\n");
    }

    async fn assert_peer_closed(mut peer: TcpStream) {
        let mut data = Vec::new();
        let result = timeout(Duration::from_secs(2), peer.read_to_end(&mut data))
            .await
            .unwrap();
        assert!(
            result.is_ok()
                || matches!(result, Err(ref error) if matches!(error.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted))
        );
    }

    #[tokio::test]
    async fn cancel_joins_accepted_connection_with_headers_still_incomplete() {
        let (mut binding, mut probe) = observed_binding().await;
        let peer = incomplete_peer(&binding, &mut probe).await;
        let mut drained = binding.server.drained.take().unwrap();
        assert!(matches!(
            probe.shutdown_pending.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            probe.dropped.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            drained.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        // Do not finish the headers or release the fixture gate. Native I/O
        // termination must end the pending read and retire its real owner.
        binding.cancel().await.unwrap();
        assert_eq!(probe.dropped.try_recv(), Ok(()));
        assert_eq!(
            drained.try_recv(),
            Ok(true),
            "actual Axum accepted-connection drain did not return normally"
        );
        assert_peer_closed(peer).await;
    }

    #[tokio::test]
    async fn cancel_joins_held_accepted_connection_before_returning() {
        let (mut binding, mut probe) = observed_binding().await;
        let mut peer = incomplete_peer(&binding, &mut probe).await;
        let mut drained = binding.server.drained.take().unwrap();
        // First establish accepted pending input, then native socket shutdown
        // held by the same stream. Idle sockets or an outer AbortHandle alone
        // would not falsify draft1.
        binding.server.signal(false);
        enter_held_shutdown(&mut peer, &mut probe).await;
        assert!(matches!(
            binding.receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            probe.dropped.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            drained.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        binding.cancel().await.unwrap();
        assert_eq!(probe.dropped.try_recv(), Ok(()));
        assert_eq!(
            drained.try_recv(),
            Ok(true),
            "actual Axum accepted-connection drain did not return normally"
        );
        assert_peer_closed(peer).await;
    }

    #[tokio::test]
    async fn existing_window_timeout_joins_held_accepted_connection() {
        let (mut binding, mut probe) = observed_binding().await;
        let mut peer = incomplete_peer(&binding, &mut probe).await;
        let mut drained = binding.server.drained.take().unwrap();
        binding.server.signal(false);
        enter_held_shutdown(&mut peer, &mut probe).await;
        assert!(matches!(
            binding.receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            probe.dropped.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        let pending = binding.expect_state("state".into());
        assert!(matches!(
            pending.wait(Duration::from_millis(1)).await,
            Err(OAuthError::Timeout)
        ));
        assert_eq!(probe.dropped.try_recv(), Ok(()));
        assert_eq!(
            drained.try_recv(),
            Ok(true),
            "actual Axum accepted-connection drain did not return normally"
        );
        assert_peer_closed(peer).await;
    }

    #[tokio::test]
    async fn abandoning_entered_join_keeps_axum_drain_owner_until_connections_retire() {
        let (mut binding, mut probe) = observed_binding().await;
        let mut peer = incomplete_peer(&binding, &mut probe).await;
        let mut drained = binding.server.drained.take().unwrap();
        let (entered_tx, entered_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut joining = Box::pin(binding.server.join(false));
            futures::future::poll_fn(|cx| {
                // Observe the actual join pending before announcing entry.
                assert!(joining.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            let _ = entered_tx.send(());
            joining.await
        });
        timeout(Duration::from_secs(2), entered_rx)
            .await
            .unwrap()
            .unwrap();
        enter_held_shutdown(&mut peer, &mut probe).await;
        assert!(matches!(
            drained.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            probe.dropped.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        // This is a later observation of the actual drain, not a joined receipt
        // from the abandoned public operation. An aborted Axum task closes this
        // channel without sending true and therefore fails the assertion.
        assert!(
            timeout(Duration::from_secs(2), drained)
                .await
                .unwrap()
                .unwrap()
        );
        assert_eq!(probe.dropped.try_recv(), Ok(()));
        assert_peer_closed(peer).await;
    }

    #[tokio::test]
    async fn actual_callback_task_is_joined_on_success_and_crossed_state_refuses() {
        let a = run_loopback_callback("state-a".into(), "/callback")
            .await
            .unwrap();
        let b = run_loopback_callback("state-b".into(), "/callback")
            .await
            .unwrap();
        let a_task = a.server.task.as_ref().unwrap().abort_handle();
        let b_task = b.server.task.as_ref().unwrap().abort_handle();
        let a_url = a.redirect_url.clone();
        let b_url = b.redirect_url.clone();
        let client = reqwest::Client::new();
        let crossed = client
            .get(&a_url)
            .query(&[("code", "secret-code-a"), ("state", "state-b")])
            .send()
            .await
            .unwrap();
        assert_eq!(crossed.status(), StatusCode::BAD_REQUEST);
        assert!(matches!(
            a.wait(Duration::from_secs(1)).await,
            Err(OAuthError::StateMismatch)
        ));
        assert!(a_task.is_finished());
        let exact = client
            .get(&b_url)
            .query(&[("code", "secret-code-b"), ("state", "state-b")])
            .send()
            .await
            .unwrap();
        assert_eq!(exact.status(), StatusCode::OK);
        let result = b.wait(Duration::from_secs(1)).await.unwrap();
        assert_eq!(result.code, "secret-code-b");
        assert!(!format!("{result:?}").contains("secret-code-b"));
        assert!(b_task.is_finished());
    }

    #[tokio::test]
    async fn timeout_and_pre_admission_cancellation_join_actual_callback_tasks() {
        let binding = bind_loopback_callback("/callback").await.unwrap();
        let task = binding.server.task.as_ref().unwrap().abort_handle();
        binding.cancel().await.unwrap();
        assert!(task.is_finished());
        let pending = run_loopback_callback("state".into(), "/callback")
            .await
            .unwrap();
        let task = pending.server.task.as_ref().unwrap().abort_handle();
        assert!(matches!(
            pending.wait(Duration::from_millis(1)).await,
            Err(OAuthError::Timeout)
        ));
        assert!(task.is_finished());
    }

    #[tokio::test]
    async fn abandoning_wait_signals_owned_drain_without_claiming_join_receipt() {
        let pending = run_loopback_callback("state".into(), "/callback")
            .await
            .unwrap();
        let task = pending.server.task.as_ref().unwrap().abort_handle();
        let wait = tokio::spawn(pending.wait(Duration::from_secs(300)));
        wait.abort();
        assert!(wait.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    async fn send_callback(redirect_url: &str, code: &str, state: &str) -> StatusCode {
        reqwest::Client::new()
            .get(redirect_url)
            .query(&[("code", code), ("state", state)])
            .send()
            .await
            .unwrap()
            .status()
    }

    fn after(duration: Duration) -> tokio::time::Instant {
        tokio::time::Instant::now() + duration
    }

    #[tokio::test]
    async fn dropped_borrowed_wait_then_close_joins_held_connection_with_receipt() {
        let (mut binding, mut probe) = observed_binding().await;
        let peer = incomplete_peer(&binding, &mut probe).await;
        let mut drained = binding.server.drained.take().unwrap();
        let mut handle = binding.expect_state("state".into());
        // Abandon an active wait; the handle keeps the receiver and server.
        assert!(
            timeout(
                Duration::from_millis(50),
                handle.wait_until(after(Duration::from_secs(300)))
            )
            .await
            .is_err()
        );
        assert!(matches!(
            probe.dropped.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            drained.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        let closed = handle.close().await.unwrap();
        assert!(!closed.undelivered_callback);
        assert_eq!(probe.dropped.try_recv(), Ok(()));
        assert_eq!(
            drained.try_recv(),
            Ok(true),
            "the receipt was returned before the actual Axum drain"
        );
        assert_peer_closed(peer).await;
    }

    #[tokio::test]
    async fn callback_before_first_wait_is_delivered_exactly_once() {
        let mut handle = run_loopback_callback("state".into(), "/callback")
            .await
            .unwrap();
        assert_eq!(
            send_callback(&handle.redirect_url, "secret-code", "state").await,
            StatusCode::OK
        );
        let outcome = handle
            .wait_until(after(Duration::from_secs(5)))
            .await
            .unwrap();
        assert_eq!(outcome.code, "secret-code");
        // The receiver completed; a second wait neither repolls nor redelivers.
        let again = handle
            .wait_until(after(Duration::from_secs(5)))
            .await
            .unwrap_err();
        assert!(matches!(again, OAuthError::CallbackParse(_)));
        assert!(!format!("{again} {again:?}").contains("secret-code"));
        let closed = handle.close().await.unwrap();
        assert!(!closed.undelivered_callback);
    }

    #[tokio::test]
    async fn close_counts_a_queued_callback_that_no_wait_took() {
        let handle = run_loopback_callback("state".into(), "/callback")
            .await
            .unwrap();
        assert_eq!(
            send_callback(&handle.redirect_url, "secret-code", "state").await,
            StatusCode::OK
        );
        let closed = handle.close().await.unwrap();
        assert!(closed.undelivered_callback);
        assert!(!format!("{closed:?}").contains("secret-code"));
    }

    #[tokio::test]
    async fn graceful_drain_held_across_the_deadline_times_out_and_keeps_the_callback() {
        let (mut binding, mut probe) = observed_binding().await;
        let peer = incomplete_peer(&binding, &mut probe).await;
        let mut drained = binding.server.drained.take().unwrap();
        let mut handle = binding.expect_state("state".into());
        assert_eq!(
            send_callback(&handle.redirect_url, "secret-code", "state").await,
            StatusCode::OK
        );
        // The callback is received before the deadline, but the held
        // connection keeps the graceful drain open across it.
        let deadline = after(Duration::from_millis(300));
        let waited = handle.wait_until(deadline).await;
        assert!(matches!(waited, Err(OAuthError::Timeout)));
        assert!(tokio::time::Instant::now() >= deadline);
        assert_eq!(probe.dropped.try_recv(), Ok(()));
        assert_eq!(drained.try_recv(), Ok(true));
        // A later deadline does not renew the window or deliver the callback.
        assert!(matches!(
            handle.wait_until(after(Duration::from_secs(300))).await,
            Err(OAuthError::Timeout)
        ));
        let closed = handle.close().await.unwrap();
        assert!(closed.undelivered_callback);
        assert_peer_closed(peer).await;
    }

    /// A callback task that fails instead of retiring.
    struct FailingListener;

    impl Listener for FailingListener {
        type Io = tokio::io::DuplexStream;
        type Addr = SocketAddr;

        #[allow(clippy::panic)]
        async fn accept(&mut self) -> (Self::Io, Self::Addr) {
            panic!("fixture: callback task fails")
        }

        fn local_addr(&self) -> io::Result<Self::Addr> {
            Ok(SocketAddr::from(([127, 0, 0, 1], 9)))
        }
    }

    fn failing_handle() -> LoopbackHandle {
        start_loopback_callback(FailingListener, "/callback", "127.0.0.1")
            .unwrap()
            .expect_state("state".into())
    }

    #[tokio::test]
    async fn failed_retirement_never_becomes_a_receipt_on_a_later_wait_or_close() {
        let retirement = OAuthError::from(CallbackRetirementFailed).to_string();
        let mut handle = failing_handle();
        let first = handle
            .wait_until(after(Duration::from_secs(5)))
            .await
            .unwrap_err();
        assert_eq!(first.to_string(), retirement);
        let again = handle
            .wait_until(after(Duration::from_secs(5)))
            .await
            .unwrap_err();
        assert_eq!(again.to_string(), retirement);
        assert_eq!(handle.close().await.unwrap_err().to_string(), retirement);
        let end = failing_handle()
            .wait_or_cancel(after(Duration::from_secs(5)), std::future::pending())
            .await;
        assert!(matches!(end, LoopbackWaitEnd::RetirementFailed), "{end:?}");
    }

    #[tokio::test]
    async fn ready_cancellation_wins_over_a_queued_callback_and_counts_it_undelivered() {
        let handle = run_loopback_callback("state".into(), "/callback")
            .await
            .unwrap();
        assert_eq!(
            send_callback(&handle.redirect_url, "secret-code", "state").await,
            StatusCode::OK
        );
        let end = handle
            .wait_or_cancel(after(Duration::from_secs(5)), std::future::ready(()))
            .await;
        assert!(
            matches!(end, LoopbackWaitEnd::Cancelled { closed } if closed.undelivered_callback),
            "{end:?}"
        );
        assert!(!format!("{end:?}").contains("secret-code"));
    }

    #[tokio::test]
    async fn elapsed_deadline_wins_over_a_queued_callback() {
        let mut handle = run_loopback_callback("state".into(), "/callback")
            .await
            .unwrap();
        assert_eq!(
            send_callback(&handle.redirect_url, "secret-code", "state").await,
            StatusCode::OK
        );
        assert!(matches!(
            handle.wait_until(tokio::time::Instant::now()).await,
            Err(OAuthError::Timeout)
        ));
        assert!(handle.close().await.unwrap().undelivered_callback);
    }

    #[tokio::test]
    async fn wait_or_cancel_reports_each_terminal_with_a_joined_receipt() {
        let handle = run_loopback_callback("state".into(), "/callback")
            .await
            .unwrap();
        assert_eq!(
            send_callback(&handle.redirect_url, "secret-code", "state").await,
            StatusCode::OK
        );
        let end = handle
            .wait_or_cancel(after(Duration::from_secs(5)), std::future::pending())
            .await;
        assert!(
            matches!(&end, LoopbackWaitEnd::Completed { outcome, closed }
                if outcome.code == "secret-code" && !closed.undelivered_callback),
            "{end:?}"
        );

        let handle = run_loopback_callback("state-a".into(), "/callback")
            .await
            .unwrap();
        assert_eq!(
            send_callback(&handle.redirect_url, "secret-code", "state-b").await,
            StatusCode::BAD_REQUEST
        );
        let end = handle
            .wait_or_cancel(after(Duration::from_secs(5)), std::future::pending())
            .await;
        assert!(
            matches!(&end, LoopbackWaitEnd::Refused { error: OAuthError::StateMismatch, closed }
                if !closed.undelivered_callback),
            "{end:?}"
        );

        let handle = run_loopback_callback("state".into(), "/callback")
            .await
            .unwrap();
        let end = handle
            .wait_or_cancel(after(Duration::from_millis(50)), std::future::pending())
            .await;
        assert!(
            matches!(end, LoopbackWaitEnd::TimedOut { closed } if !closed.undelivered_callback),
            "{end:?}"
        );

        let handle = run_loopback_callback("state".into(), "/callback")
            .await
            .unwrap();
        let task = handle.server.task.as_ref().unwrap().abort_handle();
        let end = handle
            .wait_or_cancel(
                after(Duration::from_secs(300)),
                tokio::time::sleep(Duration::from_millis(50)),
            )
            .await;
        assert!(
            matches!(end, LoopbackWaitEnd::Cancelled { closed } if !closed.undelivered_callback),
            "{end:?}"
        );
        assert!(task.is_finished());
    }

    #[tokio::test]
    async fn abandoning_wait_or_cancel_signals_owned_drain_without_receipt() {
        let handle = run_loopback_callback("state".into(), "/callback")
            .await
            .unwrap();
        let task = handle.server.task.as_ref().unwrap().abort_handle();
        let wait = tokio::spawn(
            handle.wait_or_cancel(after(Duration::from_secs(300)), std::future::pending()),
        );
        wait.abort();
        assert!(wait.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
